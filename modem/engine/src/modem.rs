// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem engine: a softmodem that answers and originates at 300 bit/s,
//! with V.8.
//!
//! # Answering
//!
//! 1. Silence, for the 1.8 to 2.5 s that V.25 clause 4.3 asks of an
//!    answering modem; V.8 clause 8.2 asks only for 0.2 s.
//! 2. Answer tone. With V.8 it is ANSam (V.8 clause 7.2), and the engine
//!    listens on V.21 channel No. 1 for CM. If CM comes, the V.8 exchange
//!    runs to its end and the modulation it agrees on is started. Without
//!    V.8 the tone is ANS (V.25 clause 2.2) for its 3.3 s.
//! 3. If no CM came, the fallback: after 75 ms of silence the engine sends
//!    the answer carrier of each allowed modulation in turn, for a time
//!    each, and listens all the while for the calling carrier of every one
//!    of them. Which carrier the caller answers with decides the modem.
//! 4. Once the modem is decided its carrier is sent, and when the far
//!    end's has been there for the carrier response time the engine
//!    reports the connection and carries data.
//!
//! # Originating
//!
//! 1. The engine listens. If it hears ANSam it runs V.8 as the calling
//!    modem. If it hears ANS, or the answer carrier of a modem with no
//!    tone before it, it waits for the tone to end and then tells by the
//!    answerer's carrier which modem it is, and sends that modem's calling
//!    carrier. If the tone ends and no carrier follows within a second it
//!    sends the calling carrier of its first allowed modulation, as V.25
//!    clause 3.10 has a duplex calling modem do.
//! 2. As 4 above.
//!
//! # The tick path
//!
//! Everything a call needs is built when the engine is told to answer or
//! originate, which is not on the media thread. [`ModemEngine::process`]
//! then allocates nothing, takes no lock, makes no system call and reads
//! no clock: its time is the samples it is given.

use dspkit::level::{LevelMeter, power_to_dbm0};
use fsk::{
    Band, Modulator, Receiver, Standard, StartStopTransmitter, TRANSMIT_DELAY_SAMPLES, Tones,
};
use g711::Law;
use tones::{
    ANS_DURATION_MS, ANSWER_SILENCE_MS, AnswerTone, AnswerToneDetector, AnswerToneKind,
    POST_TONE_SILENCE_MS,
};
use v8::{Answerer, Caller, Capabilities, Modulation, Modulations, Outcome, Sending};

use crate::diag::{Diagnostics, EngineState, SCOPE_SAMPLES, STATE_HISTORY, ScopeKind, StateChange};
use crate::{ConnectInfo, ModemEngine, ModemEvent};

/// The data from the line the engine holds for the host: about 68 seconds
/// of it at 300 bit/s.
const RX_CAPACITY: usize = 2048;

/// How long the receivers wait before they call a carrier a carrier, in
/// milliseconds. The carrier response time the host asks for is counted
/// on top of this by the engine itself.
const DETECT_MS: u32 = 20;

/// How long a carrier must be steady at mark before the fallback, or an
/// originating engine, takes it for a modem's idle carrier, in
/// milliseconds.
const IDENTIFY_MS: u32 = 100;

/// How far towards mark a receiver's output must be for a carrier to be
/// taken as that modem's idle one. A modem's own mark gives 0.72; the
/// other modem's mark, which is nearer its space, gives less than nothing.
const IDENTIFY_SOFT: f32 = 0.45;

/// How long an originating engine waits, after the answer tone has ended,
/// to learn from the answerer's carrier which modem it is, before it
/// sends its own, in milliseconds.
const ANSWER_CARRIER_WAIT_MS: u32 = 1000;

/// How long an engine that has chosen its modem waits for the far end's
/// carrier before it gives up, in milliseconds. The host's S7 is the
/// limit that matters; this one keeps an engine nobody is timing from
/// sending a carrier for ever.
const TRAINING_TIMEOUT_MS: u32 = 30_000;

/// How long an originating engine listens for any answer at all, in
/// milliseconds. As above, S7 is the limit that matters.
const LISTEN_TIMEOUT_MS: u32 = 120_000;

/// One sample of the demodulator's output in this many goes to the scope.
const SCOPE_DECIMATION: u32 = 4;

/// The settings of a line's modem.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModemConfig {
    /// The modulations allowed, in the order the fallback tries them.
    /// `None` entries are skipped.
    pub modulations: [Option<Standard>; 2],
    /// Whether V.8 is used. It is used only if V.21 is allowed: Bell 103
    /// is not in V.8.
    pub v8: bool,
    /// The level sent, in dBm0.
    pub tx_level_dbm0: f32,
    /// The silence before the answer tone, in milliseconds; 0 is the
    /// 2150 ms that is the middle of what V.25 clause 4.3 allows.
    pub answer_delay_ms: u32,
    /// How long the fallback sends each modulation's answer carrier, in
    /// milliseconds.
    pub fallback_ms: u32,
}

impl Default for ModemConfig {
    fn default() -> Self {
        ModemConfig {
            modulations: [Some(Standard::V21), Some(Standard::Bell103)],
            v8: true,
            tx_level_dbm0: -12.0,
            answer_delay_ms: 0,
            fallback_ms: 3000,
        }
    }
}

/// What a call asks of an engine: the host's settings as they are when
/// the call begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CallSettings {
    /// How long the far end's carrier must be there before the connection
    /// is reported, in milliseconds: S9, which is in tenths of a second.
    pub carrier_response_ms: u32,
    /// How long the far end's carrier may be gone before it is reported
    /// lost, in milliseconds: S10, in tenths of a second (V.250 clause
    /// 6.3.12).
    pub carrier_loss_ms: u32,
    /// The modulations the host allows for this call, if it has said, in
    /// place of the line's.
    pub modulations: Option<[Option<Standard>; 2]>,
    /// Whether the host allows V.8 for this call, if it has said.
    pub v8: Option<bool>,
}

impl Default for CallSettings {
    fn default() -> Self {
        CallSettings {
            // The factory values of S9 and S10: 6 and 14 tenths.
            carrier_response_ms: 600,
            carrier_loss_ms: 1400,
            modulations: None,
            v8: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Answer,
    Originate,
}

/// What the engine is doing, with what each state needs to know.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    AnswerSilence,
    /// Plain answer tone, without V.8.
    AnswerTone,
    /// The V.8 exchange as the answering modem, from ANSam on.
    V8Answer,
    /// The silence after plain answer tone.
    Gap,
    /// Sending the answer carrier of allowed modulation `index`.
    Fallback {
        index: usize,
    },
    /// Originating: listening for answer tone or a carrier.
    Listening,
    /// The V.8 exchange as the calling modem.
    V8Call,
    /// The modem is chosen; waiting for the far end's carrier.
    Training,
    Data,
    Ended,
}

/// The modem engine.
pub struct Modem {
    config: ModemConfig,
    call: CallSettings,
    law: Law,
    role: Role,
    phase: Phase,
    state: EngineState,
    // Samples since the engine was started, and since the phase began.
    samples: u64,
    in_phase: u32,

    // The modulations of this call, in order, and whether it uses V.8.
    allowed: [Option<Standard>; 2],
    use_v8: bool,
    // The modem the call is on, once that is decided.
    standard: Option<Standard>,

    tone: AnswerTone,
    detector: AnswerToneDetector,
    // Whether answer tone has been heard, and for how long it has not
    // been since.
    tone_heard: bool,
    tone_absent: u32,
    // Whether the V.8 exchange has been tried in this call: it is tried
    // once.
    v8_tried: bool,
    // The receiver's count of framing errors when the call connected.
    framing_base: u32,
    answerer: Answerer,
    caller: Caller,
    // A modulator and a receiver for each modem, in this end's bands:
    // index 0 is V.21 and 1 is Bell 103.
    tx: [Modulator; 2],
    rx: [Receiver; 2],
    // For how long each receiver has had a steady mark.
    steady: [u32; 2],

    // Data.
    transmitter: StartStopTransmitter,
    rx_data: [u8; RX_CAPACITY],
    rx_head: usize,
    rx_len: usize,
    // For how long the far end's carrier has been there, or gone.
    carrier_run: u32,
    carrier_gone: u32,
    carrier: bool,

    // The connection to report for each modem, made when the call begins
    // so that reporting it allocates nothing.
    connect: [Option<ConnectInfo>; 2],
    events: [Option<ModemEvent>; 4],

    // Diagnostics.
    tx_level: LevelMeter,
    history: [StateChange; STATE_HISTORY],
    changes: u8,
    scope: [i8; SCOPE_SAMPLES],
    scope_at: usize,
    scope_count: u32,
}

const fn index_of(standard: Standard) -> usize {
    match standard {
        Standard::V21 => 0,
        Standard::Bell103 => 1,
    }
}

const STANDARDS: [Standard; 2] = [Standard::V21, Standard::Bell103];

impl Modem {
    /// A stopped modem with these settings.
    pub fn new(config: ModemConfig) -> Modem {
        let law = Law::Ulaw;
        let level = config.tx_level_dbm0;
        let capabilities = Capabilities::data(Modulations::NONE);
        Modem {
            config,
            call: CallSettings::default(),
            law,
            role: Role::Answer,
            phase: Phase::Idle,
            state: EngineState::Idle,
            samples: 0,
            in_phase: 0,
            allowed: config.modulations,
            use_v8: false,
            standard: None,
            tone: AnswerTone::new(AnswerToneKind::AnsamReversing, level, law),
            detector: AnswerToneDetector::new(law),
            tone_heard: false,
            tone_absent: 0,
            v8_tried: false,
            framing_base: 0,
            answerer: Answerer::new(capabilities),
            caller: Caller::new(capabilities),
            tx: STANDARDS
                .map(|standard| Modulator::new(Tones::of(standard, Band::Answerer), level, law)),
            rx: STANDARDS.map(|standard| Receiver::new(standard, Band::Caller, DETECT_MS, law)),
            steady: [0; 2],
            transmitter: StartStopTransmitter::new(),
            rx_data: [0; RX_CAPACITY],
            rx_head: 0,
            rx_len: 0,
            carrier_run: 0,
            carrier_gone: 0,
            carrier: false,
            connect: [None, None],
            events: [None, None, None, None],
            tx_level: LevelMeter::new(0.02),
            history: [StateChange::default(); STATE_HISTORY],
            changes: 0,
            scope: [0; SCOPE_SAMPLES],
            scope_at: 0,
            scope_count: 0,
        }
    }

    /// The settings the modem was made with.
    pub fn config(&self) -> &ModemConfig {
        &self.config
    }

    /// Builds everything a call needs, for one role. This is where the
    /// engine does its allocating and its filter design: before the call,
    /// and not on the media thread.
    fn begin(&mut self, role: Role) {
        let law = self.law;
        let level = self.config.tx_level_dbm0;
        self.role = role;
        self.samples = 0;
        self.in_phase = 0;
        self.allowed = self.call.modulations.unwrap_or(self.config.modulations);
        let v21 = self.allowed.contains(&Some(Standard::V21));
        // Bell 103 is not in V.8: with V.21 not allowed there is nothing
        // to put in a menu.
        self.use_v8 = self.call.v8.unwrap_or(self.config.v8) && v21;
        self.standard = None;

        let (tx_band, rx_band) = match role {
            Role::Answer => (Band::Answerer, Band::Caller),
            Role::Originate => (Band::Caller, Band::Answerer),
        };
        self.tx =
            STANDARDS.map(|standard| Modulator::new(Tones::of(standard, tx_band), level, law));
        self.rx = STANDARDS.map(|standard| Receiver::new(standard, rx_band, DETECT_MS, law));
        self.steady = [0; 2];
        // With V.8 the tone is ANSam; without, ANS. Either has the phase
        // reversals that disable echo cancellers: V.25's Corrigendum 1
        // notes that calls fail on some networks without them.
        let kind = if self.use_v8 {
            AnswerToneKind::AnsamReversing
        } else {
            AnswerToneKind::AnsReversing
        };
        self.tone = AnswerTone::new(kind, level, law);
        self.detector = AnswerToneDetector::new(law);
        self.tone_heard = false;
        self.tone_absent = 0;
        self.v8_tried = false;
        self.framing_base = 0;
        // In this phase the menu is a data call with V.21. The other
        // modes of V.8 are coded and are switched on as they are built.
        let capabilities = Capabilities::data(Modulations::of(&[Modulation::V21]));
        self.answerer = Answerer::new(capabilities).with_transmit_delay(TRANSMIT_DELAY_SAMPLES);
        self.caller = Caller::new(capabilities).with_transmit_delay(TRANSMIT_DELAY_SAMPLES);

        self.transmitter.clear();
        self.rx_head = 0;
        self.rx_len = 0;
        self.carrier_run = 0;
        self.carrier_gone = 0;
        self.carrier = false;
        self.connect = STANDARDS.map(|standard| {
            Some(ConnectInfo {
                rate: fsk::BIT_RATE,
                carrier: Some(standard.carrier_name().to_owned()),
                protocol: None,
                compression: None,
            })
        });
        self.events = [None, None, None, None];
        self.tx_level.reset();
        self.history = [StateChange::default(); STATE_HISTORY];
        self.changes = 0;
        self.scope = [0; SCOPE_SAMPLES];
        self.scope_at = 0;
        self.scope_count = 0;

        if self.allowed.iter().all(Option::is_none) {
            // Nothing is allowed: there is no modem to be.
            self.fail();
            return;
        }
        match role {
            Role::Answer => self.enter(Phase::AnswerSilence, EngineState::AnswerSilence),
            Role::Originate => self.enter(Phase::Listening, EngineState::Listening),
        }
    }

    fn enter(&mut self, phase: Phase, state: EngineState) {
        self.phase = phase;
        self.in_phase = 0;
        self.show(state);
    }

    /// Notes a change of the state that is shown.
    fn show(&mut self, state: EngineState) {
        if self.state == state && self.changes != 0 {
            return;
        }
        self.state = state;
        let change = StateChange {
            state,
            at_ms: (self.samples / 8).min(u64::from(u32::MAX)) as u32,
        };
        if usize::from(self.changes) < STATE_HISTORY {
            self.history[usize::from(self.changes)] = change;
            self.changes += 1;
        } else {
            self.history.rotate_left(1);
            self.history[STATE_HISTORY - 1] = change;
        }
    }

    fn push(&mut self, event: ModemEvent) {
        if let Some(slot) = self.events.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(event);
        }
    }

    /// The attempt has failed, or the carrier is lost: the line is told,
    /// and nothing more is sent.
    fn fail(&mut self) {
        self.push(ModemEvent::CarrierLost);
        self.carrier = false;
        self.enter(Phase::Ended, EngineState::Ended);
    }

    fn ms(samples: u32) -> u32 {
        samples / 8
    }

    /// The modem is decided: its carrier is sent from here.
    fn train(&mut self, standard: Standard) {
        self.standard = Some(standard);
        self.tx[index_of(standard)].restart();
        self.carrier_run = 0;
        self.carrier_gone = 0;
        self.enter(Phase::Training, EngineState::Training);
    }

    /// The first allowed modulation at or after a place in the order.
    fn allowed_from(&self, index: usize) -> Option<(usize, Standard)> {
        (index..self.allowed.len()).find_map(|n| self.allowed[n].map(|standard| (n, standard)))
    }

    fn fallback(&mut self, index: usize) {
        match self.allowed_from(index) {
            Some((index, standard)) => {
                self.tx[index_of(standard)].restart();
                self.enter(Phase::Fallback { index }, EngineState::Fallback);
            }
            // Every allowed modulation has had its turn and no caller
            // answered.
            None => self.fail(),
        }
    }

    /// Runs every allowed modem's receiver on a sample and says which, if
    /// any, has now had a steady mark for long enough to be taken as that
    /// modem's idle carrier.
    #[inline]
    fn identify(&mut self, sample: f32, listen: bool) -> Option<Standard> {
        let mut found = None;
        for standard in STANDARDS {
            if !self.allowed.contains(&Some(standard)) {
                continue;
            }
            let n = index_of(standard);
            let out = self.rx[n].process(sample);
            if listen && out.carrier && out.soft > IDENTIFY_SOFT {
                self.steady[n] += 1;
                if self.steady[n] >= IDENTIFY_MS * 8 && found.is_none() {
                    found = Some(standard);
                }
            } else {
                self.steady[n] = 0;
            }
        }
        found
    }

    /// One sample in, one sample out.
    #[inline]
    fn step(&mut self, sample: f32) -> f32 {
        self.samples += 1;
        self.in_phase = self.in_phase.saturating_add(1);
        match self.phase {
            Phase::Idle | Phase::Ended => 0.0,
            Phase::AnswerSilence => {
                let delay = match self.config.answer_delay_ms {
                    // V.25 clause 4.3: 1.8 to 2.5 s.
                    0 => ANSWER_SILENCE_MS,
                    delay => delay,
                };
                if Modem::ms(self.in_phase) >= delay {
                    if self.use_v8 {
                        self.enter(Phase::V8Answer, EngineState::AnswerTone);
                    } else {
                        self.enter(Phase::AnswerTone, EngineState::AnswerTone);
                    }
                }
                0.0
            }
            Phase::AnswerTone => {
                // V.25 clause 2.2: 3.3 s of it, and then (clause 4.4)
                // 75 ms of silence.
                if Modem::ms(self.in_phase) >= ANS_DURATION_MS {
                    self.enter(Phase::Gap, EngineState::Gap);
                    return 0.0;
                }
                self.tone.next_sample()
            }
            Phase::Gap => {
                if Modem::ms(self.in_phase) >= POST_TONE_SILENCE_MS {
                    self.fallback(0);
                }
                0.0
            }
            Phase::V8Answer => self.step_v8_answer(sample),
            Phase::Fallback { index } => {
                let Some((_, sending)) = self.allowed_from(index) else {
                    self.fail();
                    return 0.0;
                };
                if let Some(standard) = self.identify(sample, true) {
                    self.train(standard);
                    return self.tx[index_of(standard)].next_sample(|| true);
                }
                if Modem::ms(self.in_phase) >= self.config.fallback_ms {
                    self.fallback(index + 1);
                    return 0.0;
                }
                self.tx[index_of(sending)].next_sample(|| true)
            }
            Phase::Listening => self.step_listening(sample),
            Phase::V8Call => self.step_v8_call(sample),
            Phase::Training => {
                let Some(standard) = self.standard else {
                    self.fail();
                    return 0.0;
                };
                let n = index_of(standard);
                let out = self.rx[n].process(sample);
                self.scope_sample(out.soft);
                self.carrier = out.carrier;
                // The connection is reported when the far end's carrier
                // has been there for the carrier response time.
                self.carrier_run = if out.carrier { self.carrier_run + 1 } else { 0 };
                if Modem::ms(self.carrier_run) >= self.call.carrier_response_ms {
                    if let Some(info) = self.connect[n].take() {
                        self.push(ModemEvent::Connect(info));
                    }
                    self.transmitter.clear();
                    self.carrier_gone = 0;
                    self.framing_base = self.rx[n].framing_errors();
                    self.enter(Phase::Data, EngineState::Data);
                } else if Modem::ms(self.in_phase) >= TRAINING_TIMEOUT_MS {
                    self.fail();
                    return 0.0;
                }
                self.tx[n].next_sample(|| true)
            }
            Phase::Data => {
                let Some(standard) = self.standard else {
                    self.fail();
                    return 0.0;
                };
                let n = index_of(standard);
                let out = self.rx[n].process(sample);
                self.scope_sample(out.soft);
                self.carrier = out.carrier;
                if let Some(character) = out.framed.character {
                    self.receive(character);
                }
                if out.carrier {
                    self.carrier_gone = 0;
                } else {
                    // The carrier is reported lost when it has been gone
                    // for S10 (V.250 clause 6.3.12); if it comes back
                    // before then the call goes on.
                    self.carrier_gone += 1;
                    if Modem::ms(self.carrier_gone) >= self.call.carrier_loss_ms {
                        self.fail();
                        return 0.0;
                    }
                }
                let transmitter = &mut self.transmitter;
                self.tx[n].next_sample(|| transmitter.next_bit())
            }
        }
    }

    /// The V.8 exchange as the answering modem (V.8 clause 8.2).
    #[inline]
    fn step_v8_answer(&mut self, sample: f32) -> f32 {
        let v21 = index_of(Standard::V21);
        let received = self.rx[v21].process(sample);
        self.carrier = received.carrier;
        if let Some(bit) = received.framed.bit {
            self.answerer.receive_bit(bit);
        }
        let out = match self.answerer.sending() {
            Sending::AnswerTone => self.tone.next_sample(),
            Sending::Bits => {
                let answerer = &mut self.answerer;
                self.tx[v21].next_sample(|| answerer.next_bit())
            }
            // What is left in the transmit filter of the bits sent.
            Sending::Silence => self.tx[v21].flush_sample(),
        };
        self.answerer.advance(1);
        while let Some(event) = self.answerer.poll() {
            match event {
                v8::Event::CallMenu(_) => self.show(EngineState::JointMenu),
                v8::Event::Terminator => self.show(EngineState::Gap),
                v8::Event::JointMenu(_) => {}
                v8::Event::Done(outcome) => {
                    self.after_v8(outcome);
                    return out;
                }
            }
        }
        if self.answerer.sending() == Sending::Silence && self.state == EngineState::AnswerTone {
            // ANSam ran its time with no CM.
            self.show(EngineState::Gap);
        }
        out
    }

    /// The V.8 exchange as the calling modem (V.8 clause 8.1).
    #[inline]
    fn step_v8_call(&mut self, sample: f32) -> f32 {
        let v21 = index_of(Standard::V21);
        let received = self.rx[v21].process(sample);
        self.carrier = received.carrier;
        if let Some(bit) = received.framed.bit {
            self.caller.receive_bit(bit);
        }
        let out = if self.caller.sending() == Sending::Bits {
            if self.state == EngineState::CallSilence {
                self.show(EngineState::CallMenu);
            }
            let caller = &mut self.caller;
            self.tx[v21].next_sample(|| caller.next_bit())
        } else {
            self.tx[v21].flush_sample()
        };
        self.caller.advance(1);
        while let Some(event) = self.caller.poll() {
            match event {
                v8::Event::JointMenu(_) => self.show(EngineState::Terminator),
                v8::Event::CallMenu(_) | v8::Event::Terminator => {}
                v8::Event::Done(outcome) => {
                    self.after_v8(outcome);
                    return out;
                }
            }
        }
        out
    }

    /// What follows the V.8 exchange.
    fn after_v8(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Agreed {
                modulation: Modulation::V21,
                ..
            } => self.train(Standard::V21),
            // A modulation this engine has not got cannot have been
            // agreed: it is not in the menu. Nothing in common is the
            // end of the call (V.8 clauses 8.1.2 and 8.2.3).
            Outcome::Agreed { .. } | Outcome::NothingInCommon => self.fail(),
            // The other modem has no V.8, or the exchange did not finish:
            // on as a modem without V.8 would go (clause 8.2.2).
            Outcome::NoResponse | Outcome::Incomplete => match self.role {
                Role::Answer => self.fallback(0),
                Role::Originate => {
                    self.tone_heard = true;
                    self.tone_absent = POST_TONE_SILENCE_MS * 8;
                    self.steady = [0; 2];
                    self.enter(Phase::Listening, EngineState::Listening);
                }
            },
        }
    }

    /// Originating: listening for answer tone, or for an answerer's
    /// carrier.
    #[inline]
    fn step_listening(&mut self, sample: f32) -> f32 {
        let status = self.detector.process(sample);
        if status.present {
            self.tone_heard = true;
            self.tone_absent = 0;
            // A calling modem sends CM only if it has detected ANSam
            // (V.8 clause 7.2).
            if status.modulated == Some(true) && self.use_v8 && !self.v8_tried {
                self.v8_tried = true;
                self.enter(Phase::V8Call, EngineState::CallSilence);
                return 0.0;
            }
        } else {
            self.tone_absent = self.tone_absent.saturating_add(1);
        }
        // The receivers are listened to only when the answer tone is not
        // there, and has not been for the 75 ms by which a calling modem
        // knows it has ended (V.25 clause 3.10): the tone is inside the
        // bands the answer carriers are in.
        let ended = self.tone_absent >= POST_TONE_SILENCE_MS * 8;
        if let Some(standard) = self.identify(sample, ended) {
            self.train(standard);
            return 0.0;
        }
        if self.tone_heard
            && Modem::ms(self.tone_absent) >= POST_TONE_SILENCE_MS + ANSWER_CARRIER_WAIT_MS
        {
            // The tone has ended and no carrier has said which modem the
            // answerer is: the calling modem puts its own carrier on the
            // line (V.25 clause 3.10), that of its first allowed
            // modulation.
            match self.allowed_from(0) {
                Some((_, standard)) => self.train(standard),
                None => self.fail(),
            }
            return 0.0;
        }
        if Modem::ms(self.in_phase) >= LISTEN_TIMEOUT_MS {
            self.fail();
        }
        0.0
    }

    #[inline]
    fn receive(&mut self, character: u8) {
        if self.rx_len < RX_CAPACITY {
            self.rx_data[(self.rx_head + self.rx_len) % RX_CAPACITY] = character;
            self.rx_len += 1;
        }
        // A host that has not read 2048 characters loses what comes next:
        // the line cannot be made to wait.
    }

    #[inline]
    fn scope_sample(&mut self, soft: f32) {
        self.scope_count += 1;
        if self.scope_count >= SCOPE_DECIMATION {
            self.scope_count = 0;
            self.scope[self.scope_at] = (soft.clamp(-1.0, 1.0) * 127.0) as i8;
            self.scope_at = (self.scope_at + 1) % SCOPE_SAMPLES;
        }
    }
}

impl ModemEngine for Modem {
    fn process(&mut self, rx: &[u8], tx: &mut [u8]) {
        let law = self.law;
        let len = rx.len().min(tx.len());
        for (octet, out) in rx[..len].iter().zip(tx.iter_mut()) {
            let sample = self.step(law.decode(*octet));
            self.tx_level.process(sample);
            *out = match self.phase {
                // Stopped, the engine sends the octet of silence itself.
                Phase::Idle | Phase::Ended => law.silence(),
                _ => law.encode(sample),
            };
        }
        // A caller that gives more room than octets gets silence in it.
        for out in &mut tx[len..] {
            *out = law.silence();
        }
    }

    fn dte_write(&mut self, data: &[u8]) -> usize {
        match self.phase {
            Phase::Data => self.transmitter.write(data),
            _ => 0,
        }
    }

    fn dte_read(&mut self, buf: &mut [u8]) -> usize {
        let count = buf.len().min(self.rx_len);
        for slot in &mut buf[..count] {
            *slot = self.rx_data[self.rx_head];
            self.rx_head = (self.rx_head + 1) % RX_CAPACITY;
        }
        self.rx_len -= count;
        count
    }

    fn poll_event(&mut self) -> Option<ModemEvent> {
        let event = self.events[0].take()?;
        self.events.rotate_left(1);
        Some(event)
    }

    fn answer(&mut self) {
        self.begin(Role::Answer);
    }

    fn originate(&mut self) {
        self.begin(Role::Originate);
    }

    fn hangup(&mut self) {
        self.phase = Phase::Idle;
        self.state = EngineState::Idle;
        self.carrier = false;
        self.events = [None, None, None, None];
        self.transmitter.clear();
        self.rx_head = 0;
        self.rx_len = 0;
    }

    fn set_law(&mut self, law: Law) {
        self.law = law;
    }

    fn set_call(&mut self, call: &CallSettings) {
        self.call = *call;
    }

    fn diagnostics(&self) -> Option<Diagnostics> {
        // The receiver that is being listened to: the call's modem, or
        // V.21's during V.8.
        let n = index_of(self.standard.unwrap_or(Standard::V21));
        let receiver = &self.rx[n];
        let listening = !matches!(
            self.phase,
            Phase::Idle | Phase::Ended | Phase::AnswerSilence
        );
        let mut scope = [0i8; SCOPE_SAMPLES];
        for (i, slot) in scope.iter_mut().enumerate() {
            *slot = self.scope[(self.scope_at + i) % SCOPE_SAMPLES];
        }
        let in_data = matches!(self.phase, Phase::Training | Phase::Data);
        Some(Diagnostics {
            state: self.state,
            rx_level_dbm0: if listening {
                receiver.demodulator().level_dbm0().max(-120.0)
            } else {
                -120.0
            },
            tx_level_dbm0: power_to_dbm0(self.tx_level.power(), self.law).max(-120.0),
            carrier: self.carrier,
            frequency_error_hz: if in_data {
                receiver.demodulator().frequency_error_hz()
            } else {
                0.0
            },
            quality: if in_data { receiver.eye_opening() } else { 0.0 },
            framing_errors: if in_data {
                receiver.framing_errors().saturating_sub(self.framing_base)
            } else {
                0
            },
            retrains: 0,
            history: self.history,
            changes: self.changes,
            scope_kind: if in_data {
                ScopeKind::Discriminator
            } else {
                ScopeKind::None
            },
            scope,
        })
    }
}
