// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The generators of answer tone and calling tone.

use dspkit::level::dbm0_to_amplitude;
use dspkit::osc::Oscillator;
use g711::Law;

use crate::{
    ANSAM_MODULATION_DEPTH, ANSAM_MODULATION_HZ, ANSWER_TONE_HZ, AnswerToneKind, CALLING_TONE_HZ,
    CALLING_TONE_OFF_MS, CALLING_TONE_ON_MS, REVERSAL_INTERVAL_MS, samples_of_ms,
};

/// Answer tone of any of the four kinds.
///
/// The generator gives samples for as long as it is asked: how long the
/// tone lasts, and what ends it, is for whoever runs the answer sequence.
#[derive(Clone, Copy, Debug)]
pub struct AnswerTone {
    carrier: Oscillator,
    modulation: Oscillator,
    kind: AnswerToneKind,
    amplitude: f32,
    until_reversal: u32,
}

impl AnswerTone {
    /// Answer tone at a level in dBm0. For ANSam the level is that of its
    /// average power, as V.8 clause 7.2 has it, so the carrier is a little
    /// below the level to leave room for the sidebands.
    pub fn new(kind: AnswerToneKind, level_dbm0: f32, law: Law) -> AnswerTone {
        let mut amplitude = dbm0_to_amplitude(level_dbm0, law);
        if kind.is_modulated() {
            // A carrier of amplitude a modulated to a depth m has the
            // power of the carrier and of two sidebands of amplitude
            // a m / 2: a^2 / 2 (1 + m^2 / 2).
            let depth = ANSAM_MODULATION_DEPTH;
            amplitude /= (1.0 + depth * depth / 2.0).sqrt();
        }
        AnswerTone {
            carrier: Oscillator::new(ANSWER_TONE_HZ),
            modulation: Oscillator::new(ANSAM_MODULATION_HZ),
            kind,
            amplitude,
            until_reversal: samples_of_ms(REVERSAL_INTERVAL_MS),
        }
    }

    /// The kind of tone.
    pub fn kind(&self) -> AnswerToneKind {
        self.kind
    }

    /// The next sample.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        if self.kind.is_reversing() {
            if self.until_reversal == 0 {
                // A step of half a turn between two samples: the phase is
                // there at once and the amplitude does not dip, which is
                // inside what V.25 clause 2.3 allows (within 10 degrees
                // in 1 ms, and not 3 dB down for more than 400 us).
                self.carrier.reverse_phase();
                self.until_reversal = samples_of_ms(REVERSAL_INTERVAL_MS);
            }
            self.until_reversal -= 1;
        }
        let carrier = self.carrier.next_sine();
        if self.kind.is_modulated() {
            // The envelope swings between 0.8 and 1.2 of its average
            // (V.8 clause 7.2).
            let envelope = 1.0 + ANSAM_MODULATION_DEPTH * self.modulation.next_sine();
            self.amplitude * envelope * carrier
        } else {
            self.amplitude * carrier
        }
    }

    /// Fills a buffer with the next samples.
    pub fn fill(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample = self.next_sample();
        }
    }
}

/// Calling tone: bursts of 1300 Hz with silence between them (V.25 clause
/// 2.1). It begins with a burst.
#[derive(Clone, Copy, Debug)]
pub struct CallingTone {
    oscillator: Oscillator,
    amplitude: f32,
    // Where in the cadence of a burst and a silence the next sample is.
    at: u32,
}

impl CallingTone {
    /// Calling tone whose bursts are at a level in dBm0.
    pub fn new(level_dbm0: f32, law: Law) -> CallingTone {
        CallingTone {
            oscillator: Oscillator::new(CALLING_TONE_HZ),
            amplitude: dbm0_to_amplitude(level_dbm0, law),
            at: 0,
        }
    }

    /// Whether the next sample is in a burst.
    pub fn is_on(&self) -> bool {
        self.at < samples_of_ms(CALLING_TONE_ON_MS)
    }

    /// The next sample.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let on = self.is_on();
        self.at += 1;
        if self.at == samples_of_ms(CALLING_TONE_ON_MS + CALLING_TONE_OFF_MS) {
            self.at = 0;
            // Each burst begins at phase zero, with no click.
            self.oscillator.set_phase(0);
        }
        if on {
            self.amplitude * self.oscillator.next_sine()
        } else {
            0.0
        }
    }

    /// Fills a buffer with the next samples.
    pub fn fill(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample = self.next_sample();
        }
    }
}
