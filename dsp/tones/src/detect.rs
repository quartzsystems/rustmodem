// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The detectors: a tone at one frequency, answer tone with its phase
//! reversals and its 15 Hz modulation, and calling tone.
//!
//! # How a tone is told from everything else
//!
//! A tone is present when, over the last 10 ms, the power at its frequency
//! is above a threshold and is most of the power there is. The second
//! condition is what keeps speech and other signals from being taken for a
//! tone: V.25 clause 11 asks that answer tone not be recognised when it is
//! accompanied by another signal of comparable level. The thresholds are
//! those V.21 clause 8.3 gives a line signal detector, on at -43 dBm0 and
//! off at -48 dBm0, because V.25 gives none of its own. The window passes
//! a tone within about 40 Hz of the nominal frequency; the answer tone
//! detector then measures the frequency and holds it to 25 Hz, which is
//! the 15 Hz of V.25 clause 2.2 and room for the line.

use dspkit::goertzel::SlidingGoertzel;
use dspkit::level::dbm0_to_power;
use dspkit::math;
use dspkit::osc::Quadrature;
use dspkit::{MovingAverage, SAMPLE_RATE};
use g711::Law;

use crate::{
    ANSAM_MODULATION_DEPTH, ANSAM_MODULATION_HZ, ANSWER_TONE_HZ, ANSWER_TONE_RECOGNITION_MS,
    AnswerToneKind, CALLING_TONE_HZ, samples_of_ms,
};

/// The window of the tone detectors in samples: 10 ms, which is a whole
/// number of cycles of both 2100 Hz and 1300 Hz.
const WINDOW: usize = 80;

/// The level at or above which a tone may be recognised, in dBm0 (the
/// figure of V.21 clause 8.3 for a line signal detector).
const ON_DBM0: f32 = -43.0;

/// The level below which a tone is gone, in dBm0 (V.21 clause 8.3).
const OFF_DBM0: f32 = -48.0;

/// The least share of the power in the window that must be at the tone's
/// frequency. A tone 41 Hz off the frequency shows this much of itself.
const MIN_FRACTION: f32 = 0.62;

/// How long a tone may fail to be there before it is taken as gone, in
/// milliseconds. A phase reversal takes the tone out of the window for
/// under 20 ms.
const OFF_HOLD_MS: u32 = 30;

/// A detector of a tone at one frequency.
#[derive(Clone, Copy, Debug)]
pub struct ToneDetector {
    tone: SlidingGoertzel<WINDOW>,
    total: MovingAverage<WINDOW>,
    on_power: f32,
    off_power: f32,
    on_samples: u32,
    off_samples: u32,
    run: u32,
    present: bool,
    phasor: Quadrature,
    power: f32,
    clean: bool,
}

impl ToneDetector {
    /// A detector of `frequency_hz` that takes `on_ms` of tone to
    /// recognise it. The frequency should be a multiple of 100 Hz, so that
    /// the 10 ms window holds a whole number of its cycles.
    pub fn new(frequency_hz: f32, on_ms: u32, law: Law) -> ToneDetector {
        ToneDetector {
            tone: SlidingGoertzel::new(frequency_hz),
            total: MovingAverage::new(),
            on_power: dbm0_to_power(ON_DBM0, law),
            off_power: dbm0_to_power(OFF_DBM0, law),
            on_samples: samples_of_ms(on_ms).max(1),
            off_samples: samples_of_ms(OFF_HOLD_MS),
            run: 0,
            present: false,
            phasor: Quadrature::default(),
            power: 0.0,
            clean: false,
        }
    }

    /// Takes one sample and says whether the tone is present.
    #[inline]
    pub fn process(&mut self, sample: f32) -> bool {
        self.measure(sample);
        self.decide(self.clean);
        self.present
    }

    /// Takes a sample into the window without deciding anything.
    #[inline]
    fn measure(&mut self, sample: f32) {
        self.phasor = self.tone.process_phasor(sample);
        let total = self.total.process(sample * sample);
        // The power a sine wave at the frequency would have.
        self.power = 2.0 * (self.phasor.i * self.phasor.i + self.phasor.q * self.phasor.q);
        self.clean = self.power >= MIN_FRACTION * total;
    }

    /// Counts the sample towards the tone coming or going. `good` is
    /// whether what is in the window is the tone.
    #[inline]
    fn decide(&mut self, good: bool) {
        if self.present {
            if !good || self.power < self.off_power {
                self.run += 1;
                if self.run >= self.off_samples {
                    self.present = false;
                    self.run = 0;
                }
            } else {
                self.run = 0;
            }
        } else if good && self.power >= self.on_power {
            self.run += 1;
            if self.run >= self.on_samples {
                self.present = true;
                self.run = 0;
            }
        } else {
            self.run = 0;
        }
    }

    /// Whether the tone is present.
    #[inline]
    pub fn present(&self) -> bool {
        self.present
    }

    /// The power at the tone's frequency over the last 10 ms: the mean
    /// square a sine wave there would have.
    #[inline]
    pub fn power(&self) -> f32 {
        self.power
    }

    /// Forgets what it has heard.
    pub fn reset(&mut self) {
        self.tone.reset();
        self.total.reset();
        self.run = 0;
        self.present = false;
        self.phasor = Quadrature::default();
        self.power = 0.0;
        self.clean = false;
    }
}

/// A detector of calling tone: 1300 Hz (V.25 clause 2.1). It says when a
/// burst is there and counts the bursts.
#[derive(Clone, Copy, Debug)]
pub struct CallingToneDetector {
    tone: ToneDetector,
    bursts: u32,
}

impl CallingToneDetector {
    /// A detector that has heard nothing.
    pub fn new(law: Law) -> CallingToneDetector {
        CallingToneDetector {
            // A burst is 500 ms at the least, so 100 ms of it is taken as
            // one.
            tone: ToneDetector::new(CALLING_TONE_HZ, 100, law),
            bursts: 0,
        }
    }

    /// Takes one sample and says whether a burst of calling tone is
    /// present.
    #[inline]
    pub fn process(&mut self, sample: f32) -> bool {
        let before = self.tone.present();
        let now = self.tone.process(sample);
        if now && !before {
            self.bursts += 1;
        }
        now
    }

    /// The bursts heard so far.
    pub fn bursts(&self) -> u32 {
        self.bursts
    }

    /// Forgets what it has heard.
    pub fn reset(&mut self) {
        self.tone.reset();
        self.bursts = 0;
    }
}

/// The distance in samples over which the answer tone detector compares
/// the tone's phase with itself: the window and half as much again, so
/// that after a reversal there are 5 ms in which one window is wholly
/// before it and the other wholly after.
const SPAN: usize = 120;

/// The furthest from 2100 Hz the tone may be measured to be, in hertz.
/// Over [`SPAN`] samples that is a turn of 135 degrees, which is still far
/// from the 180 degrees a reversal makes.
const MAX_OFFSET_HZ: f32 = 25.0;

/// The window over which the envelope is examined for the 15 Hz of ANSam:
/// 200 ms, which is three cycles of it.
const ENVELOPE_WINDOW: usize = 1600;

/// The least modulation, as a share of the average amplitude, that is
/// taken for ANSam's: half of what V.8 clause 7.2 gives it.
const MIN_MODULATION: f32 = ANSAM_MODULATION_DEPTH / 2.0;

/// How long the modulation must be seen, or not seen, before the tone is
/// called ANSam, or ANS: 50 ms after the envelope window has filled. V.8
/// clause 7.2, Note 1, asks that a detector allow for the passing changes
/// of amplitude that networks make.
const KIND_HOLD_SAMPLES: u32 = 400;

/// What the answer tone detector knows after a sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnswerToneStatus {
    /// Whether answer tone is present.
    pub present: bool,
    /// Whether its amplitude is modulated at 15 Hz: `Some(true)` for
    /// ANSam, `Some(false)` for ANS, and `None` while it is too soon to
    /// say, which it is for about 350 ms.
    pub modulated: Option<bool>,
    /// Whether a phase reversal was recognised at this sample.
    pub reversal: bool,
}

/// A detector of answer tone, for the calling modem.
///
/// It is about 34 KB, most of it the 200 ms of envelope it keeps to look
/// for the modulation. Nothing is allocated.
#[derive(Clone, Debug)]
pub struct AnswerToneDetector {
    tone: ToneDetector,
    // The tone's phasor of the last SPAN samples.
    history: [Quadrature; SPAN],
    at: usize,
    // The turn of the phasor over SPAN samples, smoothed: its angle is the
    // tone's distance from 2100 Hz.
    turn: Quadrature,
    // Samples of good tone since it was last absent.
    heard: u32,
    flip_run: u32,
    flip_latched: bool,
    reversals: u32,
    // The envelope, and its mean and its 15 Hz component over 200 ms.
    envelope: f32,
    mean: MovingAverage<ENVELOPE_WINDOW>,
    modulation: SlidingGoertzel<ENVELOPE_WINDOW>,
    envelope_samples: u32,
    kind_run: u32,
    kind_candidate: bool,
    modulated: Option<bool>,
}

impl AnswerToneDetector {
    /// A detector that has heard nothing.
    pub fn new(law: Law) -> AnswerToneDetector {
        AnswerToneDetector {
            tone: ToneDetector::new(ANSWER_TONE_HZ, ANSWER_TONE_RECOGNITION_MS, law),
            history: [Quadrature::default(); SPAN],
            at: 0,
            turn: Quadrature::default(),
            heard: 0,
            flip_run: 0,
            flip_latched: false,
            reversals: 0,
            envelope: 0.0,
            mean: MovingAverage::new(),
            modulation: SlidingGoertzel::new(ANSAM_MODULATION_HZ),
            envelope_samples: 0,
            kind_run: 0,
            kind_candidate: false,
            modulated: None,
        }
    }

    /// Takes one sample.
    #[inline]
    pub fn process(&mut self, sample: f32) -> AnswerToneStatus {
        self.tone.measure(sample);
        let now = self.tone.phasor;
        let then = self.history[self.at];
        self.history[self.at] = now;
        self.at = if self.at + 1 == SPAN { 0 } else { self.at + 1 };

        // The phasor now against the phasor SPAN samples ago: how far the
        // tone's phase has turned against a 2100 Hz reference in that time.
        let turn = Quadrature {
            i: now.i * then.i + now.q * then.q,
            q: now.q * then.i - now.i * then.q,
        };
        // Within MAX_OFFSET_HZ the turn is under 135 degrees, which is to
        // say that its real part is above minus its imaginary part's size.
        let loud = self.tone.clean && self.tone.power >= self.tone.off_power;
        let in_tune = turn.i + turn.q.abs() > 0.0;
        let good = loud && in_tune;

        let reversal = self.look_for_reversal(loud, turn);
        if good {
            self.heard = self.heard.saturating_add(1);
            // An eighth of a second of smoothing.
            self.turn.i += (turn.i - self.turn.i) / 128.0;
            self.turn.q += (turn.q - self.turn.q) / 128.0;
        }
        self.tone.decide(good);
        if self.tone.present {
            self.look_for_modulation(good);
        } else if self.heard != 0 && !good && self.tone.run == 0 {
            self.forget();
        }

        AnswerToneStatus {
            present: self.tone.present,
            modulated: self.modulated,
            reversal,
        }
    }

    /// A reversal shows as the turn over SPAN samples pointing the other
    /// way from where it has been pointing, for the 5 ms in which the two
    /// windows are on either side of it.
    #[inline]
    fn look_for_reversal(&mut self, loud: bool, turn: Quadrature) -> bool {
        // Not before the usual turn is known: 50 ms of tone.
        let settled = self.heard >= 400;
        let along = turn.i * self.turn.i + turn.q * self.turn.q;
        let size = (turn.i * turn.i + turn.q * turn.q)
            * (self.turn.i * self.turn.i + self.turn.q * self.turn.q);
        // Within 60 degrees of the opposite direction.
        let flipped = settled && loud && along < 0.0 && along * along > 0.25 * size;
        if flipped {
            self.flip_run += 1;
            // 2 ms of it, and once for each reversal.
            if self.flip_run >= 16 && !self.flip_latched {
                self.flip_latched = true;
                self.reversals += 1;
                return true;
            }
        } else {
            self.flip_run = 0;
            self.flip_latched = false;
        }
        false
    }

    /// The envelope of ANSam swings at 15 Hz by a fifth of its average
    /// (V.8 clause 7.2); that of ANS does not.
    #[inline]
    fn look_for_modulation(&mut self, good: bool) {
        if good {
            // The amplitude of the tone in the window.
            self.envelope = (2.0 * self.tone.power).sqrt();
        }
        // While a reversal has the tone out of the window the envelope is
        // held, so that the dip is not taken for modulation.
        let mean = self.mean.process(self.envelope);
        let swing = self.modulation.process_phasor(self.envelope);
        if self.envelope_samples < ENVELOPE_WINDOW as u32 {
            self.envelope_samples += 1;
            return;
        }
        // The phasor's length is half the amplitude of the swing.
        let swing_squared = 4.0 * (swing.i * swing.i + swing.q * swing.q);
        let seen = swing_squared >= MIN_MODULATION * MIN_MODULATION * mean * mean;
        if self.kind_run == 0 || seen != self.kind_candidate {
            self.kind_candidate = seen;
            self.kind_run = 1;
        } else {
            self.kind_run = self.kind_run.saturating_add(1);
            if self.kind_run >= KIND_HOLD_SAMPLES {
                self.modulated = Some(seen);
            }
        }
    }

    /// The tone has gone: what was learned of it goes with it.
    fn forget(&mut self) {
        self.heard = 0;
        self.turn = Quadrature::default();
        self.flip_run = 0;
        self.flip_latched = false;
        self.envelope = 0.0;
        self.mean.reset();
        self.modulation.reset();
        self.envelope_samples = 0;
        self.kind_run = 0;
        self.modulated = None;
    }

    /// Whether answer tone is present.
    pub fn present(&self) -> bool {
        self.tone.present
    }

    /// The phase reversals recognised since the detector was made or
    /// reset.
    pub fn reversals(&self) -> u32 {
        self.reversals
    }

    /// The kind of answer tone, once it can be told: whether it is
    /// modulated, and whether a reversal has been seen so far.
    pub fn kind(&self) -> Option<AnswerToneKind> {
        let reversing = self.reversals > 0;
        self.modulated
            .map(|modulated| match (modulated, reversing) {
                (false, false) => AnswerToneKind::Ans,
                (false, true) => AnswerToneKind::AnsReversing,
                (true, false) => AnswerToneKind::Ansam,
                (true, true) => AnswerToneKind::AnsamReversing,
            })
    }

    /// The power of the tone over the last 10 ms.
    pub fn power(&self) -> f32 {
        self.tone.power
    }

    /// How far the tone is from 2100 Hz, in hertz. For diagnostics: it
    /// takes an arctangent.
    pub fn frequency_error_hz(&self) -> f32 {
        let angle = math::atan2(f64::from(self.turn.q), f64::from(self.turn.i));
        (angle / math::TAU * f64::from(SAMPLE_RATE) / SPAN as f64) as f32
    }

    /// Forgets what it has heard.
    pub fn reset(&mut self) {
        self.tone.reset();
        self.history = [Quadrature::default(); SPAN];
        self.at = 0;
        self.reversals = 0;
        self.forget();
    }
}

// The frequency check above is in terms of the turn's two parts, and holds
// for the span and offset it was written for.
const _: () = assert!(MAX_OFFSET_HZ * SPAN as f32 / SAMPLE_RATE as f32 == 0.375);
