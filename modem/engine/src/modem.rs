// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem engine: a softmodem that answers and originates at 2400,
//! 1200 and 300 bit/s, with V.8, and at 2400 and 1200 bit/s with the
//! error control of V.42 and the data compression of V.42bis.
//!
//! # Answering
//!
//! 1. Silence, for the 1.8 to 2.5 s that V.25 clause 4.3 asks of an
//!    answering modem; V.8 clause 8.2 asks only for 0.2 s.
//! 2. Answer tone. With V.8 it is ANSam (V.8 clause 7.2), and the engine
//!    listens on V.21 channel No. 1 for CM. If CM comes, the V.8 exchange
//!    runs to its end and the modulation it agrees on is started: the
//!    highest that both have. Without V.8 the tone is ANS (V.25 clause
//!    2.2) for its 3.3 s.
//! 3. If no CM came, the fallback: after 75 ms of silence the engine
//!    takes each kind of modem it is allowed to be in turn, for a time
//!    each, in the order of its allowed modulations. For V.22bis, V.22
//!    and Bell 212A, which are one kind, it sends the answer sequence of
//!    V.22bis clause 6.3.1.1.2, and a caller is told apart by what it
//!    sends back: S1 is V.22bis, and scrambled binary 1 with no S1 a
//!    modem at 1200 bit/s. For V.21 and for Bell 103 it sends the answer
//!    carrier. All the while it listens for the calling carrier of every
//!    300 bit/s modem it is allowed to be. What the caller answers with
//!    decides the modem.
//! 4. A 300 bit/s modem, once decided, sends its carrier, and when the
//!    far end's has been there for the carrier response time the engine
//!    reports the connection and carries data, the host's characters as
//!    they are.
//! 5. A modem of the other kind runs its handshake to the end, and then
//!    the data link is made over it: the detection phase of V.42, and
//!    LAPM with V.42bis if the far end does them and the line's policy
//!    allows, or start-stop characters by V.14 if not. The connection is
//!    reported when the link is up, with what it is. A retrain stops the
//!    data and nothing else.
//!
//! # Originating
//!
//! 1. The engine listens. If it hears ANSam it runs V.8 as the calling
//!    modem. If it hears unscrambled binary 1 at 1200 bit/s in the high
//!    channel, or the Bell answering tone, it is the calling modem of
//!    V.22bis clause 6.3.1.1.1. If it hears ANS, or the answer carrier of
//!    a 300 bit/s modem with no tone before it, it waits for the tone to
//!    end and then tells by the answerer's carrier which modem it is, and
//!    sends that modem's calling carrier. If the tone ends and no carrier
//!    follows within a second, an engine that is allowed 300 bit/s
//!    modems only sends the calling carrier of the first of them, as
//!    V.25 clause 3.10 has a duplex calling modem do.
//! 2. As 4 and 5 above.
//!
//! The answer carrier of Bell 103 at mark is 2225 Hz, which is the Bell
//! answering tone and 25 Hz from the unscrambled binary 1 of V.22bis. An
//! originating engine that may be either takes it for the faster modem's
//! and begins that handshake; if nothing answers within a second and a
//! half it is Bell 103. An answering engine listens in every step of the
//! fallback for a calling modem that has done the same, and goes on with
//! the handshake from there. If the answerer does answer and the two
//! have no rate in common, that is the end of the call.
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
use v8::{Answerer, Caller, Capabilities, Modulations, Outcome, Sending};
use v14::Format;
use v22::GuardTone;

use crate::diag::{
    CONSTELLATION_POINTS, CONSTELLATION_UNIT, Diagnostics, EngineState, SCOPE_SAMPLES,
    STATE_HISTORY, ScopeKind, StateChange,
};
use crate::link::{Compression, DataLink, ErrorControl, LinkEvent, LinkSettings, TappedFrame};
use crate::modulation::{MODULATIONS, Modulation};
use crate::{ConnectInfo, ModemEngine, ModemEvent};

/// The name a connection with LAPM is reported by.
pub const PROTOCOL_LAPM: &str = "LAPM";

/// The names a connection with V.42bis is reported by: compressed both
/// ways, and where only what this end sends is, or only what it
/// receives.
pub const COMPRESSION_V42BIS: &str = "V.42bis";
pub const COMPRESSION_V42BIS_TRANSMIT: &str = "V.42bis TD";
pub const COMPRESSION_V42BIS_RECEIVE: &str = "V.42bis RD";

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

/// How much longer than its time the fallback stays with the answer
/// sequence of V.22bis while there is a signal in the calling modem's
/// channel that has not yet been told for what it is, in milliseconds.
/// A calling modem that began to answer as the time ran out is not cut
/// off; a signal that is no modem's does not hold the fallback for ever.
const SYNC_GRACE_MS: u32 = 2000;

/// How long an originating engine that has begun the handshake of
/// V.22bis waits for the answerer's part of it before it takes the tone
/// it answered for the carrier of a Bell 103 modem, if it may be one, in
/// milliseconds. The answerer's S1 comes within a quarter of a second
/// and its scrambled binary 1 within three quarters (V.22bis clauses
/// 6.3.1.1.2 b and 6.3.1.2.2 b); and an answerer that is our own gives
/// each modem three seconds by default, of which this leaves enough.
const SYNC_ANSWER_WAIT_MS: u32 = 1500;

/// The settings of a line's modem.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModemConfig {
    /// The modulations allowed, in the order the fallback tries them.
    /// `None` entries are skipped. V.22bis, V.22 and Bell 212A are tried
    /// together, where the first of them is.
    pub modulations: [Option<Modulation>; MODULATIONS],
    /// Whether V.8 is used. It is used only if a modulation that is in
    /// V.8 is allowed: Bell 103 and Bell 212A are not.
    pub v8: bool,
    /// The level sent, in dBm0.
    pub tx_level_dbm0: f32,
    /// The silence before the answer tone, in milliseconds; 0 is the
    /// 2150 ms that is the middle of what V.25 clause 4.3 allows.
    pub answer_delay_ms: u32,
    /// How long each step of the fallback lasts, in milliseconds: the
    /// answer sequence of V.22bis, and each 300 bit/s modem's answer
    /// carrier.
    pub fallback_ms: u32,
    /// What the line's policy says of error control, on a modulation
    /// that can have it.
    pub error_control: ErrorControl,
    /// Whether V.42bis is asked for and agreed to.
    pub compression: bool,
    /// The guard tone sent with the data when answering at 1200 or 2400
    /// bit/s (V.22bis clause 2.2).
    pub guard_tone: GuardTone,
}

impl Default for ModemConfig {
    fn default() -> Self {
        ModemConfig {
            modulations: Modulation::ALL.map(Some),
            v8: true,
            tx_level_dbm0: -12.0,
            answer_delay_ms: 0,
            fallback_ms: 3000,
            error_control: ErrorControl::Auto,
            compression: true,
            guard_tone: GuardTone::Off,
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
    pub modulations: Option<[Option<Modulation>; MODULATIONS]>,
    /// Whether the host allows V.8 for this call, if it has said.
    pub v8: Option<bool>,
    /// The character format of the line where characters go as
    /// start-stop characters: `+ICF`.
    pub format: Format,
    /// The error control the host asks for in this call, if it has said,
    /// in place of the line's: `+ES`.
    pub error_control: Option<ErrorControl>,
    /// The data compression the host allows in this call, if it has
    /// said: `+DS`.
    pub compression: Option<Compression>,
    /// Whether an originating modem runs the detection phase of V.42, if
    /// the host has said: `+ES`.
    pub detection: Option<bool>,
}

impl Default for CallSettings {
    fn default() -> Self {
        CallSettings {
            // The factory values of S9 and S10: 6 and 14 tenths.
            carrier_response_ms: 600,
            carrier_loss_ms: 1400,
            modulations: None,
            v8: None,
            format: Format::N81,
            error_control: None,
            compression: None,
            detection: None,
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
    /// Step `index` of the fallback: sending the answer carrier of a
    /// 300 bit/s modem, or the answer sequence of V.22bis.
    Fallback {
        index: usize,
    },
    /// Originating: listening for answer tone or a carrier.
    Listening,
    /// The V.8 exchange as the calling modem.
    V8Call,
    /// A 300 bit/s modem is chosen; waiting for the far end's carrier.
    Training,
    /// Connected at 300 bit/s.
    Data,
    /// A modem at 1200 or 2400 bit/s: its handshake, and then its data
    /// link and its data.
    Sync,
    Ended,
}

/// A step of the fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The answer carrier of a 300 bit/s modem.
    Fsk(Standard),
    /// The answer sequence of V.22bis, which V.22 and Bell 212A callers
    /// answer too.
    Sync,
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
    allowed: [Option<Modulation>; MODULATIONS],
    use_v8: bool,
    // The steps of the fallback, in order.
    steps: [Option<Step>; 3],
    // The modulation the call is on, once that is decided.
    modulation: Option<Modulation>,
    // The call's policy: the host's where it has said, or the line's.
    error_control: ErrorControl,
    compression: Compression,

    // The modem at 1200 and 2400 bit/s: as it is at the start of its
    // handshake, to begin one from; and the one that is run.
    sync_start: v22::Modem,
    sync: v22::Modem,
    // Whether any of its modulations is allowed, and whether, when
    // originating, it may yet be the call's modem.
    sync_allowed: bool,
    sync_alive: bool,
    // Whether its handshake is over and its link is started.
    sync_connected: bool,
    // The step of the fallback it was begun in, when answering.
    sync_step: Option<usize>,
    // Whether it was begun by V.8, after which there is nothing else to
    // be; whether the far end has answered its part of the handshake;
    // and for how long it has been sending without that.
    sync_by_v8: bool,
    sync_answered: bool,
    sync_unanswered: u32,
    link: DataLink,
    reported: bool,
    retrains: u32,

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

    // The names a connection is reported by, made when the call begins
    // so that reporting it allocates nothing.
    names: [Option<String>; MODULATIONS],
    protocol_name: Option<String>,
    // Compressed both ways, what is sent only, and what is received only.
    compression_names: [Option<String>; 3],
    events: [Option<ModemEvent>; 4],

    // Diagnostics.
    tx_level: LevelMeter,
    history: [StateChange; STATE_HISTORY],
    changes: u8,
    scope: [i8; SCOPE_SAMPLES],
    scope_at: usize,
    scope_count: u32,
    points: [[i8; 2]; CONSTELLATION_POINTS],
    points_at: usize,
    points_len: u8,
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
        let sync = v22::Modem::new(v22::Settings {
            role: v22::Role::Answering,
            allowed: v22::Allowed::ALL,
            guard: config.guard_tone,
            level_dbm0: level,
            law,
        });
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
            steps: [None; 3],
            modulation: None,
            error_control: config.error_control,
            compression: Compression::OFF,
            sync_start: sync.clone(),
            sync,
            sync_allowed: false,
            sync_alive: false,
            sync_connected: false,
            sync_step: None,
            sync_by_v8: false,
            sync_answered: false,
            sync_unanswered: 0,
            link: DataLink::new(),
            reported: false,
            retrains: 0,
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
            names: [const { None }; MODULATIONS],
            protocol_name: None,
            compression_names: [const { None }; 3],
            events: [None, None, None, None],
            tx_level: LevelMeter::new(0.02),
            history: [StateChange::default(); STATE_HISTORY],
            changes: 0,
            scope: [0; SCOPE_SAMPLES],
            scope_at: 0,
            scope_count: 0,
            points: [[0; 2]; CONSTELLATION_POINTS],
            points_at: 0,
            points_len: 0,
        }
    }

    /// Turns on or off the keeping of the frames the data link sends and
    /// receives, for a tool that shows them ([`Modem::poll_frame`]). It
    /// is off in a new modem.
    pub fn tap_frames(&mut self, on: bool) {
        self.link.set_tap(on);
    }

    /// The next frame of the data link, of those kept since
    /// [`Modem::tap_frames`] turned the keeping on. They are to be taken
    /// after every frame of samples: no more than a few are held.
    pub fn poll_frame(&mut self) -> Option<TappedFrame> {
        self.link.poll_frame()
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
        let has = |modulation: Modulation| self.allowed.contains(&Some(modulation));
        let sync = v22::Allowed {
            v22bis: has(Modulation::V22bis),
            v22: has(Modulation::V22),
            bell212a: has(Modulation::Bell212A),
        };
        // The menu of V.8 (Table 4/V.8): V.21, and V.22bis and V.22, which
        // are one entry of it. Bell 103 and Bell 212A are not in V.8: with
        // nothing else allowed there is nothing to put in a menu.
        let mut menu = Modulations::NONE;
        if has(Modulation::V21) {
            menu = menu.with(v8::Modulation::V21);
        }
        if sync.v22bis || sync.v22 {
            menu = menu.with(v8::Modulation::V22bis);
        }
        self.use_v8 = self.call.v8.unwrap_or(self.config.v8) && !menu.is_empty();
        self.modulation = None;
        self.error_control = self.call.error_control.unwrap_or(self.config.error_control);
        self.compression = self.call.compression.unwrap_or(if self.config.compression {
            Compression::BOTH
        } else {
            Compression::OFF
        });

        // The steps of the fallback, in the order of the modulations
        // allowed: the three that are one kind of modem are one step,
        // where the first of them is.
        self.steps = [None; 3];
        let mut count = 0;
        for modulation in self.allowed.into_iter().flatten() {
            let step = match modulation.fsk() {
                Some(standard) => Step::Fsk(standard),
                None => Step::Sync,
            };
            if !self.steps.contains(&Some(step)) && count < self.steps.len() {
                self.steps[count] = Some(step);
                count += 1;
            }
        }
        self.sync_allowed = sync.v22bis || sync.v22 || sync.bell212a;
        self.sync_alive = self.sync_allowed;
        self.sync_connected = false;
        self.sync_step = None;
        self.sync_by_v8 = false;
        self.sync_answered = false;
        self.sync_unanswered = 0;
        self.sync_start = v22::Modem::new(v22::Settings {
            role: match role {
                Role::Answer => v22::Role::Answering,
                Role::Originate => v22::Role::Calling,
            },
            allowed: sync,
            guard: self.config.guard_tone,
            level_dbm0: level,
            law,
        });
        self.sync = self.sync_start.clone();
        self.link.stop();
        self.reported = false;
        self.retrains = 0;

        let (tx_band, rx_band) = match role {
            Role::Answer => (Band::Answerer, Band::Caller),
            Role::Originate => (Band::Caller, Band::Answerer),
        };
        self.tx =
            STANDARDS.map(|standard| Modulator::new(Tones::of(standard, tx_band), level, law));
        self.rx = STANDARDS.map(|standard| Receiver::new(standard, rx_band, DETECT_MS, law));
        // V.8 reads the bits of the line for itself and is not touched by
        // the format: it is the data's.
        for receiver in &mut self.rx {
            receiver.set_format(self.call.format);
        }
        self.transmitter.set_format(self.call.format);
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
        let capabilities = Capabilities::data(menu);
        self.answerer = Answerer::new(capabilities).with_transmit_delay(TRANSMIT_DELAY_SAMPLES);
        self.caller = Caller::new(capabilities).with_transmit_delay(TRANSMIT_DELAY_SAMPLES);

        self.transmitter.clear();
        self.rx_head = 0;
        self.rx_len = 0;
        self.carrier_run = 0;
        self.carrier_gone = 0;
        self.carrier = false;
        self.names = Modulation::ALL.map(|modulation| Some(modulation.carrier_name().to_owned()));
        self.protocol_name = Some(PROTOCOL_LAPM.to_owned());
        self.compression_names = [
            COMPRESSION_V42BIS,
            COMPRESSION_V42BIS_TRANSMIT,
            COMPRESSION_V42BIS_RECEIVE,
        ]
        .map(|name| Some(name.to_owned()));
        self.events = [None, None, None, None];
        self.tx_level.reset();
        self.history = [StateChange::default(); STATE_HISTORY];
        self.changes = 0;
        self.scope = [0; SCOPE_SAMPLES];
        self.scope_at = 0;
        self.scope_count = 0;
        self.points = [[0; 2]; CONSTELLATION_POINTS];
        self.points_at = 0;
        self.points_len = 0;

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

    /// A 300 bit/s modem is decided: its carrier is sent from here.
    fn train(&mut self, standard: Standard) {
        self.modulation = Some(Modulation::of_fsk(standard));
        self.sync_alive = false;
        self.tx[index_of(standard)].restart();
        self.carrier_run = 0;
        self.carrier_gone = 0;
        self.enter(Phase::Training, EngineState::Training);
    }

    /// The modem at 1200 or 2400 bit/s is decided: the call is its
    /// handshake from here. `step` is the step of the fallback it was
    /// begun in, if it was.
    fn train_sync(&mut self, step: Option<usize>) {
        self.sync_step = step;
        self.sync_connected = false;
        self.sync_answered = false;
        self.sync_unanswered = 0;
        self.carrier_gone = 0;
        self.steady = [0; 2];
        self.enter(Phase::Sync, EngineState::Training);
    }

    /// The first allowed 300 bit/s modulation.
    fn first_fsk(&self) -> Option<Standard> {
        self.allowed
            .iter()
            .flatten()
            .find_map(|modulation| modulation.fsk())
    }

    /// Whether a 300 bit/s modem is allowed.
    fn fsk_allowed(&self, standard: Standard) -> bool {
        self.allowed.contains(&Some(Modulation::of_fsk(standard)))
    }

    fn fallback(&mut self, index: usize) {
        match self.steps.get(index).copied().flatten() {
            Some(Step::Fsk(standard)) => {
                self.tx[index_of(standard)].restart();
                self.enter(Phase::Fallback { index }, EngineState::Fallback);
            }
            // The answering modem of V.22bis has been listening since
            // the fallback began, and sending to nobody; now it is what
            // is sent (clause 6.3.1.1.2 a).
            Some(Step::Sync) => self.enter(Phase::Fallback { index }, EngineState::Fallback),
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
            if !self.fsk_allowed(standard) {
                continue;
            }
            // The answer carrier of Bell 103 at mark is the Bell answering
            // tone, and all but the unscrambled binary 1 of V.22bis: an
            // originating modem that may be either lets the faster one
            // try first, and takes the carrier for Bell 103's when that
            // has come to nothing.
            let listen = listen
                && !(self.role == Role::Originate
                    && self.sync_alive
                    && standard == Standard::Bell103);
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
                let Some(step) = self.steps.get(index).copied().flatten() else {
                    self.fail();
                    return 0.0;
                };
                let sending = match step {
                    Step::Fsk(sending) => sending,
                    Step::Sync => return self.step_fallback_sync(sample, index),
                };
                if self.sync_allowed {
                    // A calling modem at 1200 or 2400 bit/s may answer
                    // this carrier: that of Bell 103 at mark is all but
                    // the tone it waits for. Its S1 or its scrambled
                    // binary 1 is listened for in every step, and the
                    // handshake goes on from it.
                    let out = self.sync.process(sample, &mut self.link);
                    if self.sync.engaged() {
                        self.train_sync(Some(index));
                        return out;
                    }
                }
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
            Phase::Sync => self.step_sync(sample),
            Phase::Training => {
                let Some(standard) = self.modulation.and_then(Modulation::fsk) else {
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
                    self.report(Modulation::of_fsk(standard), false, (false, false));
                    self.transmitter.clear();
                    self.carrier_gone = 0;
                    self.framing_base = self.rx[n].framing_errors() + self.rx[n].parity_errors();
                    self.enter(Phase::Data, EngineState::Data);
                } else if Modem::ms(self.in_phase) >= TRAINING_TIMEOUT_MS {
                    self.fail();
                    return 0.0;
                }
                self.tx[n].next_sample(|| true)
            }
            Phase::Data => {
                let Some(standard) = self.modulation.and_then(Modulation::fsk) else {
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

    /// Reports the connection: on a modulation, with LAPM or without, and
    /// with what this end sends compressed, and what it receives.
    fn report(&mut self, modulation: Modulation, lapm: bool, compression: (bool, bool)) {
        if std::mem::replace(&mut self.reported, true) {
            return;
        }
        let info = ConnectInfo {
            rate: modulation.rate(),
            carrier: self.names[modulation as usize].take(),
            protocol: if lapm {
                self.protocol_name.take()
            } else {
                None
            },
            compression: match compression {
                (true, true) => self.compression_names[0].take(),
                (true, false) => self.compression_names[1].take(),
                (false, true) => self.compression_names[2].take(),
                (false, false) => None,
            },
        };
        self.push(ModemEvent::Connect(info));
    }

    /// The step of the fallback that is the answer sequence of V.22bis
    /// (clause 6.3.1.1.2): unscrambled binary 1, or for a modem that is
    /// Bell 212A and nothing else its answering tone, until a calling
    /// modem answers or the step has had its time.
    #[inline]
    fn step_fallback_sync(&mut self, sample: f32, index: usize) -> f32 {
        let out = self.sync.process(sample, &mut self.link);
        self.carrier = self.sync.carrier();
        if self.sync.engaged() {
            // S1, or scrambled binary 1: a calling modem of this kind.
            self.train_sync(Some(index));
            return out;
        }
        // The calling carrier of a 300 bit/s modem, from a caller that
        // does not wait for the answerer's.
        if let Some(standard) = self.identify(sample, true) {
            self.train(standard);
            return self.tx[index_of(standard)].next_sample(|| true);
        }
        let waited = Modem::ms(self.in_phase);
        let limit = self.config.fallback_ms;
        if (waited >= limit && !self.carrier) || waited >= limit.saturating_add(SYNC_GRACE_MS) {
            self.fallback(index + 1);
            return 0.0;
        }
        out
    }

    /// A modem at 1200 or 2400 bit/s: its handshake, and then its data.
    #[inline]
    fn step_sync(&mut self, sample: f32) -> f32 {
        let out = self.sync.process(sample, &mut self.link);
        self.carrier = self.sync.carrier();
        if let Some(point) = self.sync.point() {
            self.plot(point.re, point.im);
        }
        self.sync_answered |= self.sync.answered();
        while let Some(event) = self.sync.poll_event() {
            match event {
                v22::Event::Connected { standard } => {
                    let modulation = Modulation::of_v22(standard);
                    self.modulation = Some(modulation);
                    self.sync_connected = true;
                    self.carrier_gone = 0;
                    self.link.start(LinkSettings {
                        role: match self.role {
                            Role::Answer => v42::Role::Answerer,
                            Role::Originate => v42::Role::Originator,
                        },
                        error_control: self.error_control,
                        detection: self.call.detection.unwrap_or(true),
                        compression: self.compression,
                        format: self.call.format,
                        rate: modulation.rate(),
                    });
                    self.show(EngineState::Link);
                }
                v22::Event::Retrain => {
                    // The host is told nothing: its data waits (V.22bis
                    // clause 6.4).
                    self.retrains += 1;
                    self.push(ModemEvent::Retrain);
                    self.show(EngineState::Retrain);
                }
                v22::Event::Retrained => {
                    self.show(if self.link.up() {
                        EngineState::Data
                    } else {
                        EngineState::Link
                    });
                }
                v22::Event::Failed(_) => {
                    self.sync_failed();
                    return 0.0;
                }
            }
        }
        if self.sync_connected {
            if self.carrier {
                self.carrier_gone = 0;
            } else {
                // As at 300 bit/s: lost when it has been gone for S10.
                self.carrier_gone += 1;
                if Modem::ms(self.carrier_gone) >= self.call.carrier_loss_ms {
                    self.fail();
                    return 0.0;
                }
            }
        } else if Modem::ms(self.in_phase) >= TRAINING_TIMEOUT_MS {
            self.sync_failed();
            return 0.0;
        } else if self.role == Role::Originate && !self.sync_by_v8 && !self.sync_answered {
            // What was taken for the answer of this modem may have been
            // the carrier of a Bell 103 answerer at mark, which is the
            // same tone. If nothing answers the handshake and the engine
            // may be Bell 103, it is that.
            if self.sync.signal() != v22::Signal::Silence {
                self.sync_unanswered += 1;
            }
            if Modem::ms(self.sync_unanswered) >= SYNC_ANSWER_WAIT_MS
                && self.fsk_allowed(Standard::Bell103)
            {
                self.train(Standard::Bell103);
                return self.tx[index_of(Standard::Bell103)].next_sample(|| true);
            }
        }
        out
    }

    /// The modem at 1200 or 2400 bit/s gave up.
    fn sync_failed(&mut self) {
        self.sync_alive = false;
        if self.sync_connected || self.sync_by_v8 {
            // The call was on it, or V.8 had agreed on it: there is
            // nothing else to be (V.8 clause 8).
            self.fail();
            return;
        }
        match (self.role, self.sync_step) {
            // In the fallback: the next kind of modem has its turn, and
            // this one listens again from its start.
            (Role::Answer, Some(index)) => {
                self.sync.clone_from(&self.sync_start);
                self.fallback(index + 1);
            }
            // Originating, and the answerer never answered: it may yet
            // be a 300 bit/s modem, whose carrier is listened for. An
            // answerer that did answer is a modem of this kind with no
            // rate in common, and that is the end of the call.
            (Role::Originate, _) if !self.sync_answered && self.first_fsk().is_some() => {
                self.tone_heard = true;
                self.tone_absent = POST_TONE_SILENCE_MS * 8;
                self.steady = [0; 2];
                self.enter(Phase::Listening, EngineState::Listening);
            }
            _ => self.fail(),
        }
    }

    /// What the data link has to say, once a tick.
    fn link_events(&mut self) {
        while let Some(event) = self.link.poll_event() {
            match event {
                LinkEvent::Up { lapm, compression } => {
                    if let Some(modulation) = self.modulation {
                        self.report(modulation, lapm, compression);
                    }
                    self.show(EngineState::Data);
                }
                // No error control where the policy asks for it, or the
                // error-corrected connection has ended: the call is
                // cleared, and the host has NO CARRIER.
                LinkEvent::Down(_) => {
                    self.fail();
                    return;
                }
                // Counted by the link.
                LinkEvent::Break => {}
            }
        }
    }

    /// A received point for the constellation.
    #[inline]
    fn plot(&mut self, re: f32, im: f32) {
        let scale = f32::from(CONSTELLATION_UNIT);
        let place = |value: f32| (value * scale).clamp(-127.0, 127.0) as i8;
        self.points[self.points_at] = [place(re), place(im)];
        self.points_at = (self.points_at + 1) % CONSTELLATION_POINTS;
        if usize::from(self.points_len) < CONSTELLATION_POINTS {
            self.points_len += 1;
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
                modulation: v8::Modulation::V21,
                ..
            } => self.train(Standard::V21),
            // V.22bis or V.22: which, the handshake tells (V.22bis
            // clause 6.3.1). The answer tone has been sent, and the
            // handshake is begun from its start.
            Outcome::Agreed {
                modulation: v8::Modulation::V22bis,
                ..
            } if self.sync_allowed => {
                self.sync.clone_from(&self.sync_start);
                self.train_sync(None);
                self.sync_by_v8 = true;
            }
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
        if self.sync_alive {
            // The calling modem of V.22bis is silent until it has heard
            // unscrambled binary 1 for long enough (clause 6.3.1.1.1 a),
            // or the Bell answering tone.
            let out = self.sync.process(sample, &mut self.link);
            if self.sync.engaged() {
                self.train_sync(None);
                return out;
            }
        }
        if let Some(standard) = self.identify(sample, ended) {
            self.train(standard);
            return 0.0;
        }
        if self.tone_heard
            && !self.sync_allowed
            && Modem::ms(self.tone_absent) >= POST_TONE_SILENCE_MS + ANSWER_CARRIER_WAIT_MS
            && let Some(standard) = self.first_fsk()
        {
            // The tone has ended and no carrier has said which modem the
            // answerer is: the calling modem puts its own carrier on the
            // line (V.25 clause 3.10), that of its first allowed 300
            // bit/s modulation. An engine that may be a modem at 1200 or
            // 2400 bit/s does not: that modem has no carrier to put
            // there and answers what it hears, and a 300 bit/s carrier
            // sent meanwhile would be in its way.
            self.train(standard);
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
        if self.phase == Phase::Sync && self.sync_connected {
            // The data link, once a tick: its timers, and the octets
            // between it and the host. It is done before the samples, so
            // that an engine it stops is silent from the start of a
            // frame.
            self.link.tick(len as u32);
            self.link_events();
        }
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
            Phase::Sync => self.link.write(data),
            _ => 0,
        }
    }

    fn dte_read(&mut self, buf: &mut [u8]) -> usize {
        if self.phase == Phase::Sync {
            return self.link.read(buf);
        }
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
        self.link.stop();
    }

    fn set_law(&mut self, law: Law) {
        self.law = law;
    }

    fn set_call(&mut self, call: &CallSettings) {
        self.call = *call;
    }

    fn diagnostics(&self) -> Option<Diagnostics> {
        let mut scope = [0i8; SCOPE_SAMPLES];
        for (i, slot) in scope.iter_mut().enumerate() {
            *slot = self.scope[(self.scope_at + i) % SCOPE_SAMPLES];
        }
        let tx_level_dbm0 = power_to_dbm0(self.tx_level.power(), self.law).max(-120.0);
        let sync_step = matches!(
            self.phase,
            Phase::Fallback { index } if self.steps.get(index) == Some(&Some(Step::Sync))
        );
        if self.phase == Phase::Sync || sync_step {
            // The modem at 1200 and 2400 bit/s.
            let demodulator = self.sync.demodulator();
            let connected = self.sync_connected;
            let mut constellation = [[0i8; 2]; CONSTELLATION_POINTS];
            let len = usize::from(self.points_len);
            let first = (self.points_at + CONSTELLATION_POINTS - len) % CONSTELLATION_POINTS;
            for (i, slot) in constellation[..len].iter_mut().enumerate() {
                *slot = self.points[(first + i) % CONSTELLATION_POINTS];
            }
            let link = self.link.stats();
            let mse = demodulator.mse();
            return Some(Diagnostics {
                state: self.state,
                rx_level_dbm0: if self.carrier {
                    demodulator.level_dbm0().max(-120.0)
                } else {
                    -120.0
                },
                tx_level_dbm0,
                carrier: self.carrier,
                frequency_error_hz: if connected {
                    demodulator.carrier_offset_hz()
                } else {
                    0.0
                },
                // The error against the distance to a wrong decision at
                // 2400 bit/s, which is a unit of the grid: a unit is the
                // root of a tenth of the mean power.
                quality: if connected {
                    (1.0 - (mse * 10.0).sqrt()).clamp(0.0, 1.0)
                } else {
                    0.0
                },
                framing_errors: link.character_errors,
                retrains: self.retrains,
                history: self.history,
                changes: self.changes,
                scope_kind: ScopeKind::None,
                scope: [0; SCOPE_SAMPLES],
                snr_db: if connected { demodulator.snr_db() } else { 0.0 },
                equalizer_mse: if connected { mse } else { 0.0 },
                timing_offset_ppm: if connected {
                    demodulator.timing_offset_ppm()
                } else {
                    0.0
                },
                constellation_bits: match (self.phase, demodulator.rate()) {
                    (Phase::Sync, v22::Rate::B1200) => 2,
                    (Phase::Sync, v22::Rate::B2400) => 4,
                    _ => 0,
                },
                constellation_len: self.points_len,
                constellation,
                link,
            });
        }
        // The receiver that is being listened to: the call's modem, or
        // V.21's during V.8.
        let standard = self.modulation.and_then(Modulation::fsk);
        let n = index_of(standard.unwrap_or(Standard::V21));
        let receiver = &self.rx[n];
        let listening = !matches!(
            self.phase,
            Phase::Idle | Phase::Ended | Phase::AnswerSilence
        );
        let in_data = matches!(self.phase, Phase::Training | Phase::Data);
        Some(Diagnostics {
            state: self.state,
            rx_level_dbm0: if listening {
                receiver.demodulator().level_dbm0().max(-120.0)
            } else {
                -120.0
            },
            tx_level_dbm0,
            carrier: self.carrier,
            frequency_error_hz: if in_data {
                receiver.demodulator().frequency_error_hz()
            } else {
                0.0
            },
            quality: if in_data { receiver.eye_opening() } else { 0.0 },
            framing_errors: if in_data {
                (receiver.framing_errors() + receiver.parity_errors())
                    .saturating_sub(self.framing_base)
            } else {
                0
            },
            retrains: self.retrains,
            history: self.history,
            changes: self.changes,
            scope_kind: if in_data {
                ScopeKind::Discriminator
            } else {
                ScopeKind::None
            },
            scope,
            // What the data link counted stays to be read when the call
            // has ended, with how it ended.
            link: self.link.stats(),
            ..Diagnostics::default()
        })
    }
}
