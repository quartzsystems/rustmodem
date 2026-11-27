// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem whole: the handshake of V.22bis clause 6.3, the fall back to
//! 1200 bit/s, the retrain of clause 6.4, and the data between.
//!
//! A [`Modem`] is one end of a call from the moment the V.25 answer
//! sequence is over. It is given the line a sample at a time and gives
//! the line a sample back, and while it is connected it takes the bits to
//! send from a [`BitIo`] and gives it the bits received.
//!
//! # The sequences
//!
//! The answering modem sends unscrambled binary 1 and the calling modem
//! is silent. What follows is told by what each hears (clause 6.3.1):
//!
//! | The calling modem | The answering modem | They work at |
//! |---|---|---|
//! | sends S1, then scrambled 1 | hears S1, sends S1, then scrambled 1 | 2400 bit/s |
//! | sends S1, then scrambled 1 | does not know S1; hears scrambled 1 | 1200 bit/s |
//! | sends scrambled 1 | hears scrambled 1 and no S1 | 1200 bit/s |
//!
//! Each transition below names its clause. Every time in the clauses is
//! of the signals on the line, so each timer here is shorter than the
//! clause's by what this modem's detector takes to see a signal and its
//! transmitter to put one out.
//!
//! # Bell 212A
//!
//! There is no Bell publication on the desk. What is built is what
//! V.22's own text says of those modems, and the parameters that are
//! common knowledge of them: the same carriers, coding and scrambler as
//! V.22 at 1200 bit/s, and an answering tone of 2225 Hz where V.22 has
//! unscrambled binary 1 (the note to clause 6.3.1.2.2). A modem allowed
//! Bell 212A and neither V.22 nor V.22bis answers with that tone. A
//! calling modem takes the tone or unscrambled 1 alike, since one
//! detector hears both. A call made at 1200 bit/s is called V.22 if this
//! modem is allowed V.22 or V.22bis, and Bell 212A otherwise: on the
//! line the two are the same.
//!
//! # Retrains
//!
//! At 2400 bit/s either modem may retrain (clause 6.4): it sends S1 and
//! goes back to scrambled 1 at 1200 bit/s, the other answers with S1, and
//! both come up to 2400 bit/s as they did at first. This modem begins one
//! when the mean-squared error of its decisions has been too high for
//! too long, and answers one whenever it hears S1. A retrain that is not
//! answered is tried again, and after a few tries the modem gives up.
//!
//! The optional rate change of clause 6.6 is not built: a modem that
//! asks for one is answered as for a retrain, at 2400 bit/s.

use dspkit::level::dbm0_to_amplitude;
use dspkit::osc::Oscillator;
use g711::Law;

use crate::detect::Detectors;
use crate::{Band, Demodulator, GuardTone, Modulator, Rate, Symbol, TRANSMIT_DELAY_SAMPLES};

/// Samples in a millisecond.
const MS: u64 = 8;

/// The Bell answering tone, in hertz.
const BELL_ANSWER_HZ: f32 = 2225.0;

/// What the transmitter takes from a change of signal to the line: its
/// filter, and half a symbol for the change to wait for the next one.
const TX_LATENCY: u64 = TRANSMIT_DELAY_SAMPLES as u64 + 7;

/// How long unscrambled binary 1 must have been on the line before the
/// calling modem takes it as detected (clause 6.3.1.1.1 b: 155 ms).
const ONES_DETECT_MS: u64 = 155;

/// What the detector of unscrambled 1 takes to see it from its coming on
/// the line: the line signal detector, in the main.
const ONES_LATENCY: u64 = 37 * MS;

/// What the line signal detector takes to come on from a signal's coming
/// on the line.
const CARRIER_LATENCY: u64 = 47 * MS;

/// How long unscrambled 1 must have been seen without a break for its
/// ending to be taken as the end of the answering modem's: 50 ms, thirty
/// symbols, which data never looks like.
const ONES_END_SAMPLES: u32 = 50 * MS as u32;

/// How long the calling modem stays silent after that (clause
/// 6.3.1.1.1 b: 456 ms).
const SILENCE_MS: u64 = 456;

/// The bits of S1: 100 ms at 1200 bit/s (clause 6.3.1.1.1 b).
const S1_BITS: u32 = 120;

/// How long S1 must be seen before it is taken as S1, and how long it
/// must be gone before it is taken as ended, in samples.
const S1_ON_SAMPLES: u32 = 30 * MS as u32;
const S1_OFF_SAMPLES: u32 = 4 * MS as u32;

/// What the detector of S1 takes to see that S1 has ended on the line.
const S1_END_LATENCY: u64 = 18 * MS;

/// After the end of the far end's S1: when the receiver begins to decide
/// sixteen ways, and when the transmitter begins scrambled 1 at 2400
/// bit/s (clause 6.3.1.1.1 d: 450 ms and 600 ms).
const SIXTEEN_WAY_MS: u64 = 450;
const SEND_2400_MS: u64 = 600;

/// The bits of scrambled 1 at 2400 bit/s before data: 200 ms (clause
/// 6.3.1.1.1 e).
const ONES_2400_BITS: u32 = 480;

/// The ones in a row at 2400 bit/s on which the receiver is ready
/// (clause 6.3.1.1.1 f).
const ONES_2400_RUN: u32 = 32;

/// How long scrambled binary 1 must have been on the line before it is
/// taken as detected (clause 6.3.1.2.1 c: 270 ms), and the ones in a row
/// that are asked for besides.
const SCRAMBLED_DETECT_MS: u64 = 270;
const SCRAMBLED_RUN: u32 = 64;

/// What the calling modem's receiver takes to see that unscrambled 1 has
/// given way to a scrambled signal, from its doing so on the line.
const SCRAMBLED_LATENCY: u64 = 65 * MS;

/// How long after scrambled 1 is detected, or begun, a modem at 1200
/// bit/s is ready for data (clauses 6.3.1.2.1 d and 6.3.1.2.2 c: 765 ms).
const READY_1200_MS: u64 = 765;

/// How long a modem waits for the far end's part of a handshake before
/// it gives up, once the handshake has begun.
const RESPONSE_MS: u64 = 5000;

/// How long after it begins to send at 2400 bit/s a modem waits for the
/// far end's 32 ones before it retrains.
const TRAIN_MS: u64 = 2000;

/// How long a modem that has sent S1 for a retrain waits for S1 from the
/// far end before it sends it again (clause 6.4: 1.2 s is recommended).
const RETRAIN_WAIT_MS: u64 = 1200;

/// How many times a retrain is begun before the modem gives up.
const RETRAIN_TRIES: u8 = 4;

/// The mean-squared error over which a modem at 2400 bit/s takes its
/// equalisation for lost, against a constellation of mean power one, and
/// for how many symbols it must stay over: 13 dB under the signal, where
/// one bit in a hundred is wrong, for a second.
const RETRAIN_MSE: f32 = 0.05;
const RETRAIN_SYMBOLS: u32 = 600;

/// The same for a modem at 1200 bit/s, which has no retrain to ask for
/// and finds the signal again by itself: 6 dB under the signal.
const LOST_MSE: f32 = 0.25;

/// Which end of the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The calling modem: it sends in the low channel.
    Calling,
    /// The answering modem: it sends in the high channel.
    Answering,
}

/// One of the three modems this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Standard {
    /// ITU-T V.22bis: 2400 bit/s.
    V22bis,
    /// ITU-T V.22: 1200 bit/s.
    V22,
    /// Bell 212A: 1200 bit/s.
    Bell212A,
}

impl Standard {
    /// The name a connection on this modem is reported by.
    pub const fn carrier_name(self) -> &'static str {
        match self {
            Standard::V22bis => "V.22bis",
            Standard::V22 => "V.22",
            Standard::Bell212A => "BELL212A",
        }
    }

    /// The rate of a connection on this modem.
    pub const fn rate(self) -> Rate {
        match self {
            Standard::V22bis => Rate::B2400,
            Standard::V22 | Standard::Bell212A => Rate::B1200,
        }
    }
}

/// Which of the three a modem may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Allowed {
    /// V.22bis: the modem sends and answers S1, and works at 2400 bit/s
    /// with a modem that does the same.
    pub v22bis: bool,
    /// V.22: the modem works at 1200 bit/s.
    pub v22: bool,
    /// Bell 212A: the modem works at 1200 bit/s.
    pub bell212a: bool,
}

impl Allowed {
    /// All three.
    pub const ALL: Allowed = Allowed {
        v22bis: true,
        v22: true,
        bell212a: true,
    };

    /// Whether a call at 1200 bit/s is allowed, and what it is called.
    const fn at_1200(self) -> Option<Standard> {
        if self.v22 {
            Some(Standard::V22)
        } else if self.bell212a {
            Some(Standard::Bell212A)
        } else {
            None
        }
    }

    /// Whether the answering modem sends the Bell answering tone where
    /// V.22 has unscrambled binary 1.
    const fn bell_answer(self) -> bool {
        self.bell212a && !self.v22 && !self.v22bis
    }
}

/// What a modem is made with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Which end of the call.
    pub role: Role,
    /// Which modems it may be.
    pub allowed: Allowed,
    /// The guard tone an answering modem sends.
    pub guard: GuardTone,
    /// The level it sends at, in dBm0.
    pub level_dbm0: f32,
    /// The law of the call's G.711.
    pub law: Law,
}

/// Why a modem gave up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Failure {
    /// The far end did not go on with the handshake.
    NoResponse,
    /// The far end is a modem at 1200 bit/s and this one is allowed 2400
    /// bit/s only.
    RateNotAllowed,
    /// The modems did not come up at 2400 bit/s, at first or after a
    /// retrain.
    Training,
}

/// What a modem has to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// The handshake is over and data passes both ways.
    Connected {
        /// Which modem the call is on.
        standard: Standard,
    },
    /// A retrain began, of this modem's asking or the far end's. No data
    /// passes until it is over.
    Retrain,
    /// The retrain is over and data passes again.
    Retrained,
    /// The modem gave up. It sends nothing more.
    Failed(Failure),
}

/// The signal a modem is sending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Signal {
    /// Nothing.
    Silence,
    /// The Bell answering tone.
    AnswerTone,
    /// Unscrambled binary 1 at 1200 bit/s.
    UnscrambledOnes,
    /// S1: the unscrambled double dibit 00 and 11 at 1200 bit/s.
    S1,
    /// Scrambled binary 1 at 1200 bit/s.
    ScrambledOnes1200,
    /// Scrambled binary 1 at 2400 bit/s.
    ScrambledOnes2400,
    /// Data.
    Data,
}

/// Where the bits of a connected modem come from and go to. The line is
/// synchronous: a bit is asked for at every bit time, and one is given.
pub trait BitIo {
    /// The next bit to send; binary 1 when there is nothing.
    fn tx_bit(&mut self) -> bool;
    /// A bit received.
    fn rx_bit(&mut self, bit: bool);
}

/// The part of the transmitter that says what the next bit is.
#[derive(Clone, Copy, Debug)]
struct Sender {
    signal: Signal,
    // Bits of the signal still to send, for the signals that are timed in
    // bits: S1 and scrambled 1 at 2400 bit/s.
    left: u32,
    // How many bits of the signal have been sent, for the pattern of S1.
    sent: u32,
    // Whether the signal ended with the last bit, and the next begins.
    ended: bool,
}

impl Sender {
    #[inline]
    fn next_bit(&mut self, io: &mut impl BitIo) -> bool {
        let bit = match self.signal {
            // The dibits 00 and 11 in turn.
            Signal::S1 => self.sent & 2 != 0,
            Signal::Data => return io.tx_bit(),
            _ => true,
        };
        self.sent = self.sent.wrapping_add(1);
        if self.left > 0 {
            self.left -= 1;
            if self.left == 0 {
                self.ended = true;
            }
        }
        bit
    }
}

/// Where a modem is in its sequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Calling: silent, until unscrambled 1 has been heard for long
    /// enough (clause 6.3.1.1.1 a).
    CallListen,
    /// Calling: silent for 456 ms more (clause 6.3.1.1.1 b).
    CallSilent { until: u64 },
    /// Sending, and waiting for the far end's S1 or its scrambled 1
    /// (clauses 6.3.1.1.1 c and 6.3.1.1.2 a).
    Wait { deadline: u64 },
    /// The far end's S1 is on the line.
    FarS1 { deadline: u64 },
    /// The far end's S1 has ended at `t0`: coming up to 2400 bit/s
    /// (clauses 6.3.1.1.1 d to f and 6.3.1.1.2 c to e).
    Train2400 { t0: u64 },
    /// Scrambled 1 at 1200 bit/s has been detected, or begun in answer:
    /// ready at `at` (clauses 6.3.1.2.1 d and 6.3.1.2.2 c).
    Train1200 { at: u64, standard: Standard },
    /// Connected.
    Data,
    /// S1 sent to ask for a retrain: waiting for the far end's (clause
    /// 6.4).
    RetrainWait { until: u64 },
    /// Given up.
    Failed,
}

/// One end of a call at 1200 or 2400 bit/s.
#[derive(Clone, Debug)]
pub struct Modem {
    role: Role,
    allowed: Allowed,
    modulator: Modulator,
    demodulator: Demodulator,
    detectors: Detectors,
    tone: Oscillator,
    tone_amplitude: f32,
    // How much of the tone is on, as the guard tone's is.
    tone_on: f32,
    state: State,
    sender: Sender,
    // The sample at which the signal now sent had its first bit taken.
    signal_since: u64,
    now: u64,
    // The detectors' runs, in samples.
    ones_run: u32,
    ones_ended: bool,
    s1_run: u32,
    s1_gap: u32,
    // When the signal received became a scrambled one, on the line.
    scrambled_since: Option<u64>,
    // The descrambled bits alike in a row: ones, and zeros.
    run_ones: u32,
    run_zeros: u32,
    // Whether S1 has been sent in this handshake or retrain.
    sent_s1: bool,
    sixteen_way: bool,
    sending_2400: bool,
    rx_ready: bool,
    // The sample at which the receiver was last made ready.
    rx_ready_at: Option<u64>,
    tx_ready: bool,
    connected: Option<Standard>,
    retraining: bool,
    tries: u8,
    // Symbols in a row whose error was too high.
    poor: u32,
    events: [Option<Event>; 4],
}

impl Modem {
    /// A modem at the start of its handshake: the answering modem begins
    /// to send at once, the calling modem listens.
    pub fn new(settings: Settings) -> Modem {
        let band = match settings.role {
            Role::Calling => Band::Low,
            Role::Answering => Band::High,
        };
        let mut demodulator = Demodulator::new(band.other(), settings.law);
        // Until the modems are connected, a carrier that comes is a new
        // signal to find.
        demodulator.set_resuming(false);
        let mut modem = Modem {
            role: settings.role,
            allowed: settings.allowed,
            modulator: Modulator::new(band, settings.level_dbm0, settings.guard, settings.law),
            demodulator,
            detectors: Detectors::new(),
            tone: Oscillator::new(BELL_ANSWER_HZ),
            tone_amplitude: dbm0_to_amplitude(settings.level_dbm0, settings.law),
            tone_on: 0.0,
            state: State::CallListen,
            sender: Sender {
                signal: Signal::Silence,
                left: 0,
                sent: 0,
                ended: false,
            },
            signal_since: 0,
            now: 0,
            ones_run: 0,
            ones_ended: false,
            s1_run: 0,
            s1_gap: 0,
            scrambled_since: None,
            run_ones: 0,
            run_zeros: 0,
            sent_s1: false,
            sixteen_way: false,
            sending_2400: false,
            rx_ready: false,
            rx_ready_at: None,
            tx_ready: false,
            connected: None,
            retraining: false,
            tries: 0,
            poor: 0,
            events: [None; 4],
        };
        match settings.role {
            Role::Calling => modem.send(Signal::Silence),
            Role::Answering => {
                // Clause 6.3.1.1.2 a: unscrambled binary 1; or, for a
                // modem that is Bell 212A and nothing else, its tone.
                modem.send(if settings.allowed.bell_answer() {
                    Signal::AnswerTone
                } else {
                    Signal::UnscrambledOnes
                });
                modem.state = State::Wait { deadline: u64::MAX };
            }
        }
        modem
    }

    /// The signal the modem is sending.
    pub fn signal(&self) -> Signal {
        self.sender.signal
    }

    /// The sample, counted from the modem's first, at which the signal it
    /// is sending began to go into the transmit filter. It is on the line
    /// [`TRANSMIT_DELAY_SAMPLES`] later.
    pub fn signal_since(&self) -> u64 {
        self.signal_since
    }

    /// Which modem the call is on, once it is connected.
    pub fn connected(&self) -> Option<Standard> {
        self.connected
    }

    /// Whether data passes: connected, and not in a retrain.
    pub fn data(&self) -> bool {
        self.state == State::Data
    }

    /// Whether the modem has given up.
    pub fn failed(&self) -> bool {
        self.state == State::Failed
    }

    /// The sample, counted from the modem's first, at which its receiver
    /// was last ready for data: at 1200 bit/s when scrambled binary 1 was
    /// detected (clause 6.3.1.2.1 c), at 2400 bit/s on the 32 ones
    /// (clause 6.3.1.1.1 f).
    pub fn receive_ready_at(&self) -> Option<u64> {
        self.rx_ready_at
    }

    /// Whether the far end's signal is on the line.
    pub fn carrier(&self) -> bool {
        self.demodulator.carrier()
    }

    /// The demodulator, for its measurements.
    pub fn demodulator(&self) -> &Demodulator {
        &self.demodulator
    }

    /// The next thing the modem has to say, if there is one.
    pub fn poll_event(&mut self) -> Option<Event> {
        let event = self.events[0].take()?;
        self.events.rotate_left(1);
        Some(event)
    }

    fn push_event(&mut self, event: Event) {
        if let Some(slot) = self.events.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(event);
        }
    }

    /// Asks for a retrain, as the modem itself does when its error is too
    /// high. Nothing happens unless the modems are connected at 2400
    /// bit/s.
    pub fn retrain(&mut self) {
        if self.state == State::Data && self.connected == Some(Standard::V22bis) {
            self.tries = 0;
            self.begin_retrain();
        }
    }

    /// Changes the signal sent, from the next symbol.
    fn send(&mut self, signal: Signal) {
        self.sender = Sender {
            signal,
            left: match signal {
                Signal::S1 => S1_BITS,
                Signal::ScrambledOnes2400 => ONES_2400_BITS,
                _ => 0,
            },
            sent: 0,
            ended: false,
        };
        self.signal_since = self.now;
        let silent = matches!(signal, Signal::Silence | Signal::AnswerTone);
        self.modulator.set_silent(silent);
        match signal {
            Signal::Silence | Signal::AnswerTone | Signal::Data => {}
            Signal::UnscrambledOnes | Signal::S1 => {
                self.modulator.set_rate(Rate::B1200);
                self.modulator.set_scrambled(false);
            }
            Signal::ScrambledOnes1200 => {
                self.modulator.set_rate(Rate::B1200);
                self.modulator.set_scrambled(true);
            }
            Signal::ScrambledOnes2400 => {
                self.modulator.set_rate(Rate::B2400);
                self.modulator.set_scrambled(true);
            }
        }
        if signal == Signal::S1 {
            self.sent_s1 = true;
        }
    }

    fn fail(&mut self, failure: Failure) {
        self.state = State::Failed;
        self.send(Signal::Silence);
        self.push_event(Event::Failed(failure));
    }

    /// Takes one sample of the line and gives one.
    #[inline]
    pub fn process(&mut self, sample: f32, io: &mut impl BitIo) -> f32 {
        // What is received.
        let out = self.demodulator.process(sample);
        let detected = self
            .detectors
            .process(self.demodulator.baseband(), f32::MIN_POSITIVE);
        let (ones, s1) = (out.carrier && detected.ones, out.carrier && detected.s1);
        self.track(ones, s1);
        if let Some(symbol) = out.symbol {
            self.receive(symbol, io);
        }
        self.step();

        // What is sent.
        let sender = &mut self.sender;
        let mut line = self.modulator.next_sample(|| sender.next_bit(io));
        if self.sender.ended {
            // The signals that are timed in bits give way to what follows
            // them: S1 to scrambled 1 (clause 6.3.1.1.1 b), scrambled 1 at
            // 2400 bit/s to data (clause 6.3.1.1.1 e).
            match self.sender.signal {
                Signal::S1 => self.send(Signal::ScrambledOnes1200),
                Signal::ScrambledOnes2400 => {
                    self.send(Signal::Data);
                    self.tx_ready = true;
                }
                _ => self.sender.ended = false,
            }
        }
        let tone = if self.sender.signal == Signal::AnswerTone {
            1.0
        } else {
            0.0
        };
        self.tone_on += (tone - self.tone_on).clamp(-0.025, 0.025);
        if self.tone_on > 0.0 {
            line += self.tone_amplitude * self.tone_on * self.tone.next_sine();
        }
        self.now += 1;
        line
    }

    /// Follows the detectors of the unscrambled signals.
    #[inline]
    fn track(&mut self, ones: bool, s1: bool) {
        if ones {
            self.ones_run = self.ones_run.saturating_add(1);
        } else {
            // The calling modem, once it has taken unscrambled 1 as
            // detected, waits for it to end. A few symbols of data can
            // look like it, so nothing but that is made of its ending.
            let waiting = self.role == Role::Calling
                && matches!(self.state, State::CallSilent { .. } | State::Wait { .. });
            if waiting && !self.ones_ended && self.ones_run >= ONES_END_SAMPLES {
                // What follows is S1 or a scrambled signal: the receiver
                // finds it from nothing, having learned nothing from a
                // signal that was one line.
                self.ones_ended = true;
                self.demodulator.retrain();
                self.scrambled_since = Some(self.now.saturating_sub(SCRAMBLED_LATENCY));
                self.run_ones = 0;
                self.run_zeros = 0;
            }
            self.ones_run = 0;
        }
        if s1 {
            self.s1_run = self.s1_run.saturating_add(1);
            self.s1_gap = 0;
        } else if self.s1_run > 0 {
            self.s1_gap += 1;
            if self.s1_gap >= S1_OFF_SAMPLES {
                self.s1_run = 0;
                self.s1_gap = 0;
            }
        }
    }

    /// Whether S1 is on the line, having been seen for long enough.
    #[inline]
    fn far_s1(&self) -> bool {
        self.s1_run >= S1_ON_SAMPLES
    }

    /// Takes a symbol from the demodulator.
    #[inline]
    fn receive(&mut self, symbol: Symbol, io: &mut impl BitIo) {
        for bit in symbol.data_bits() {
            if bit {
                self.run_ones = self.run_ones.saturating_add(1);
                self.run_zeros = 0;
            } else {
                self.run_zeros = self.run_zeros.saturating_add(1);
                self.run_ones = 0;
            }
            if self.state == State::Data {
                io.rx_bit(bit);
            }
        }
        if self.state == State::Data {
            self.watch_quality();
        }
    }

    /// Counts the symbols whose error is too high, and when there have
    /// been too many in a row, retrains (clause 6.4: "detection of loss
    /// of equalization").
    #[inline]
    fn watch_quality(&mut self) {
        let limit = match self.connected {
            Some(Standard::V22bis) => RETRAIN_MSE,
            _ => LOST_MSE,
        };
        if self.demodulator.mse() > limit && self.demodulator.tracking() {
            self.poor += 1;
        } else {
            self.poor = 0;
        }
        if self.poor < RETRAIN_SYMBOLS {
            return;
        }
        self.poor = 0;
        if self.connected == Some(Standard::V22bis) {
            self.tries = 0;
            self.begin_retrain();
        } else {
            // At 1200 bit/s there is no retrain to ask for. The receiver
            // finds the signal again by itself: four points can be found
            // from nothing.
            self.demodulator.retrain();
        }
    }

    /// Sends S1 to ask for a retrain (clause 6.4 a).
    fn begin_retrain(&mut self) {
        if self.tries >= RETRAIN_TRIES {
            self.fail(Failure::Training);
            return;
        }
        self.tries += 1;
        // Before the modems have ever been connected this is still the
        // handshake, and nothing is said of it.
        if !self.retraining && self.connected.is_some() {
            self.retraining = true;
            self.push_event(Event::Retrain);
        }
        self.rx_ready = false;
        self.tx_ready = false;
        self.sixteen_way = false;
        self.sending_2400 = false;
        self.sent_s1 = false;
        self.send(Signal::S1);
        // The far end answers with S1 within the time of the line, there
        // and back, of the end of this one.
        self.state = State::RetrainWait {
            until: self.now + (100 + RETRAIN_WAIT_MS) * MS,
        };
    }

    /// The sequences: what the state and what has been heard make happen.
    #[inline]
    fn step(&mut self) {
        match self.state {
            State::CallListen => {
                // Clause 6.3.1.1.1 b: after 155 ms of unscrambled binary 1.
                if u64::from(self.ones_run) + ONES_LATENCY >= ONES_DETECT_MS * MS {
                    self.state = State::CallSilent {
                        until: self.now + SILENCE_MS * MS - TX_LATENCY,
                    };
                }
            }
            State::CallSilent { until } => {
                if self.now >= until {
                    // Clause 6.3.1.1.1 b: S1 and then scrambled 1; clause
                    // 6.3.1.2.1 b: for a modem at 1200 bit/s, scrambled 1.
                    self.send(if self.allowed.v22bis {
                        Signal::S1
                    } else {
                        Signal::ScrambledOnes1200
                    });
                    self.state = State::Wait {
                        deadline: self.now + RESPONSE_MS * MS,
                    };
                }
            }
            State::Wait { deadline } => self.wait(deadline),
            State::FarS1 { deadline } => {
                if !self.far_s1() {
                    self.far_s1_ended();
                } else if self.now >= deadline {
                    self.fail(Failure::NoResponse);
                }
            }
            State::Train2400 { t0 } => self.train_2400(t0),
            State::Train1200 { at, standard } => {
                if self.now >= at {
                    // Clauses 6.3.1.2.1 d and 6.3.1.2.2 c.
                    self.send(Signal::Data);
                    self.connect(standard);
                }
            }
            State::Data => {
                // Clause 6.4: a retrain is begun by the far end's S1.
                if self.connected == Some(Standard::V22bis) && self.far_s1() {
                    if !self.retraining {
                        self.retraining = true;
                        self.push_event(Event::Retrain);
                    }
                    self.tries = 0;
                    self.rx_ready = false;
                    self.tx_ready = false;
                    self.sixteen_way = false;
                    self.sending_2400 = false;
                    self.sent_s1 = false;
                    self.state = State::FarS1 {
                        deadline: self.now + RESPONSE_MS * MS,
                    };
                }
            }
            State::RetrainWait { until } => {
                if self.far_s1() {
                    self.state = State::FarS1 {
                        deadline: self.now + RESPONSE_MS * MS,
                    };
                } else if self.now >= until {
                    // Clause 6.4: not answered within the time of the
                    // line there and back: the retrain signal again.
                    self.begin_retrain();
                }
            }
            State::Failed => {}
        }
    }

    /// Waiting for the far end's S1 or its scrambled 1.
    #[inline]
    fn wait(&mut self, deadline: u64) {
        if self.allowed.v22bis && self.far_s1() {
            // Clauses 6.3.1.1.1 c and 6.3.1.1.2 b: S1 is detected.
            self.state = State::FarS1 {
                deadline: self.now + RESPONSE_MS * MS,
            };
            return;
        }
        if self.role == Role::Answering && self.scrambled_since.is_none() {
            // The answering modem hears nothing until the calling modem
            // sends, so what comes is the calling modem's signal from its
            // start. If it is not S1 it is scrambled.
            if self.demodulator.carrier() && self.s1_run == 0 {
                self.scrambled_since = Some(self.now.saturating_sub(CARRIER_LATENCY));
            }
        } else if self.role == Role::Answering && !self.demodulator.carrier() {
            self.scrambled_since = None;
        }
        // Clauses 6.3.1.1.1 c and 6.3.1.2.1 c: scrambled binary 1 for
        // 270 ms. Clauses 6.3.1.1.2 b and 6.3.1.2.2 b: for the answering
        // modem, scrambled binary 1 or 0.
        let run = match self.role {
            Role::Calling => self.run_ones,
            Role::Answering => self.run_ones.max(self.run_zeros),
        };
        if let Some(since) = self.scrambled_since
            && self.s1_run == 0
            && run >= SCRAMBLED_RUN
            && self.now >= since + SCRAMBLED_DETECT_MS * MS
        {
            let Some(standard) = self.allowed.at_1200() else {
                self.fail(Failure::RateNotAllowed);
                return;
            };
            self.rx_ready = true;
            self.rx_ready_at = Some(self.now);
            match self.role {
                // Clause 6.3.1.2.1 d: ready to send 765 ms after.
                Role::Calling => {
                    self.state = State::Train1200 {
                        at: self.now + READY_1200_MS * MS,
                        standard,
                    };
                }
                // Clause 6.3.1.2.2 b and c: scrambled 1, and ready after
                // 765 ms of it.
                Role::Answering => {
                    self.send(Signal::ScrambledOnes1200);
                    self.state = State::Train1200 {
                        at: self.now + READY_1200_MS * MS,
                        standard,
                    };
                }
            }
            return;
        }
        if self.now >= deadline {
            self.fail(Failure::NoResponse);
        }
    }

    /// The far end's S1 has ended.
    fn far_s1_ended(&mut self) {
        let t0 = self.now.saturating_sub(S1_END_LATENCY);
        // What follows S1 is scrambled 1 at 1200 bit/s, and the receiver
        // finds it from nothing: at first because it has had no signal
        // but S1, in a retrain because its equalisation is lost.
        self.demodulator.set_rate(Rate::B1200);
        self.demodulator.retrain();
        self.run_ones = 0;
        self.run_zeros = 0;
        if !self.sent_s1 {
            // Clause 6.3.1.1.2 b, and clause 6.4 a for a retrain the far
            // end began: S1 in answer, then scrambled 1.
            self.send(Signal::S1);
        }
        self.state = State::Train2400 { t0 };
    }

    /// Coming up to 2400 bit/s after the far end's S1.
    #[inline]
    fn train_2400(&mut self, t0: u64) {
        // Clause 6.3.1.1.1 d: 450 ms after, sixteen-way decisions.
        if !self.sixteen_way && self.now >= t0 + SIXTEEN_WAY_MS * MS {
            self.sixteen_way = true;
            self.demodulator.set_rate(Rate::B2400);
            self.run_ones = 0;
        }
        // Clause 6.3.1.1.1 d: 600 ms after, scrambled 1 at 2400 bit/s.
        if !self.sending_2400 && self.now + TX_LATENCY >= t0 + SEND_2400_MS * MS {
            self.sending_2400 = true;
            self.send(Signal::ScrambledOnes2400);
        }
        // Clause 6.3.1.1.1 f: 32 ones in a row at 2400 bit/s.
        if self.sixteen_way && !self.rx_ready && self.run_ones >= ONES_2400_RUN {
            self.rx_ready = true;
            self.rx_ready_at = Some(self.now);
        }
        if self.rx_ready && self.tx_ready {
            self.connect(Standard::V22bis);
        } else if self.now >= t0 + (SEND_2400_MS + TRAIN_MS) * MS {
            // Clause 6.4: a modem that fails to synchronise on the
            // retrain sequence sends another.
            self.begin_retrain();
        }
    }

    /// The handshake, or the retrain, is over.
    fn connect(&mut self, standard: Standard) {
        self.state = State::Data;
        self.poor = 0;
        self.tries = 0;
        // From here a carrier that goes and comes back is the same
        // signal (clause 6.5).
        self.demodulator.set_resuming(true);
        self.retraining = false;
        if self.connected.is_some() {
            self.push_event(Event::Retrained);
        } else {
            self.connected = Some(standard);
            self.push_event(Event::Connected { standard });
        }
    }
}
