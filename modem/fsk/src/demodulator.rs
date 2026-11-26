// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The demodulator: a line signal to a decision at every sample.
//!
//! # The method
//!
//! The signal first goes through a band-pass filter that keeps the band
//! of the far end and rejects the band of this end's own transmitter,
//! which is on the same line and may come back as an echo louder than the
//! far end. What is left goes to two correlators, one at the mark
//! frequency and one at the space frequency, each measuring the energy at
//! its frequency over the last bit. The output is the difference of the
//! two energies over their sum: positive for mark and negative for space.
//! This is non-coherent detection: it needs no knowledge of the carrier's
//! phase, and a frequency offset of the 12 Hz that V.21 clause 3 asks a
//! demodulator to bear costs it almost nothing.
//!
//! The two frequencies are 200 Hz apart, and at 300 baud a bit cannot
//! tell them wholly apart: a steady mark leaves a sixth of its energy in
//! the space correlator, so the output goes to 0.72 and not to 1.
//!
//! Because each correlator has summed a whole bit when the bit ends, its
//! output changes over where the bit after begins and is at its fullest
//! where the bit ends: the output is the bits, half a bit late and with
//! sloping sides. The start-stop receiver takes its timing from the
//! output's own crossings, so the lateness is of no account.

use dspkit::SAMPLE_RATE;
use dspkit::fir::{self, Fir, Window};
use dspkit::goertzel::SlidingGoertzel;
use dspkit::level::{CarrierDetect, CarrierThresholds, LevelMeter, power_to_dbm0};
use dspkit::math;
use dspkit::osc::Quadrature;
use g711::Law;

use crate::{
    BAND_BETA, BAND_MARGIN_HZ, BAND_TAPS, CARRIER_OFF_DBM0, CARRIER_OFF_MS, CARRIER_ON_DBM0, Tones,
};

/// The correlators' window in samples: a bit, which is 26 and two thirds.
const BIT_SAMPLES: usize = 27;

/// The time constant of the level meter the carrier is detected from, in
/// seconds. Short, so that the reading falls 45 dB in 26 ms when the
/// loudest carrier there can be goes: the detector has 80 ms in all to go
/// off (V.21 clause 8.2.2).
const LEVEL_TIME_S: f32 = 0.0025;

/// How far from zero the output must be for one frequency to be taken as
/// filling the correlators. A steady frequency gives 0.72, and less when
/// the line has moved it towards the other.
const STEADY: f32 = 0.5;

/// What the demodulator has after a sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Demodulated {
    /// From 1 for mark to -1 for space. Zero when there is no signal.
    pub soft: f32,
    /// Whether the line signal detector is on.
    pub carrier: bool,
}

/// The demodulator of one band.
#[derive(Clone, Debug)]
pub struct Demodulator {
    law: Law,
    band: Fir<BAND_TAPS>,
    mark: SlidingGoertzel<BIT_SAMPLES>,
    space: SlidingGoertzel<BIT_SAMPLES>,
    level: LevelMeter,
    carrier: CarrierDetect,
    // The phasors of the two correlators a bit ago, for the frequency
    // error, and for how many samples the output has been well to one
    // side: 1 for mark, -1 for space.
    mark_then: [Quadrature; BIT_SAMPLES],
    space_then: [Quadrature; BIT_SAMPLES],
    steady: i8,
    steady_run: u32,
    at: usize,
    // How far the received frequency turns against its nominal one in a
    // bit, smoothed.
    turn: Quadrature,
}

impl Demodulator {
    /// A demodulator for a band's two frequencies, with a line signal
    /// detector that takes `carrier_on_ms` to come on.
    pub fn new(tones: Tones, carrier_on_ms: u32, law: Law) -> Demodulator {
        let low = tones.mark_hz.min(tones.space_hz) - BAND_MARGIN_HZ;
        let high = tones.mark_hz.max(tones.space_hz) + BAND_MARGIN_HZ;
        Demodulator {
            law,
            band: Fir::new(fir::bandpass(low, high, Window::Kaiser(BAND_BETA))),
            mark: SlidingGoertzel::new(tones.mark_hz),
            space: SlidingGoertzel::new(tones.space_hz),
            level: LevelMeter::new(LEVEL_TIME_S),
            carrier: CarrierDetect::new(
                CarrierThresholds {
                    // V.21 clause 8.3.
                    on_dbm0: CARRIER_ON_DBM0,
                    off_dbm0: CARRIER_OFF_DBM0,
                    on_time_s: carrier_on_ms as f32 / 1000.0,
                    off_time_s: CARRIER_OFF_MS as f32 / 1000.0,
                },
                law,
            ),
            mark_then: [Quadrature::default(); BIT_SAMPLES],
            space_then: [Quadrature::default(); BIT_SAMPLES],
            steady: 0,
            steady_run: 0,
            at: 0,
            turn: Quadrature::default(),
        }
    }

    /// Takes one sample.
    #[inline]
    pub fn process(&mut self, sample: f32) -> Demodulated {
        let filtered = self.band.process(sample);
        let power = self.level.process(filtered);
        let carrier = self.carrier.process(power);

        let mark = self.mark.process_phasor(filtered);
        let space = self.space.process_phasor(filtered);
        let mark_energy = mark.i * mark.i + mark.q * mark.q;
        let space_energy = space.i * space.i + space.q * space.q;
        let sum = mark_energy + space_energy;
        let soft = if sum > 0.0 {
            (mark_energy - space_energy) / sum
        } else {
            0.0
        };

        self.follow_frequency(mark, space, soft);
        Demodulated { soft, carrier }
    }

    /// While one frequency has filled the correlators for a bit and still
    /// does, the turn of its phasor over that bit is the received
    /// frequency's distance from the nominal one.
    #[inline]
    fn follow_frequency(&mut self, mark: Quadrature, space: Quadrature, soft: f32) {
        let steady: i8 = if soft > STEADY {
            1
        } else if soft < -STEADY {
            -1
        } else {
            0
        };
        if steady != 0 && steady == self.steady {
            self.steady_run = self.steady_run.saturating_add(1);
        } else {
            self.steady = steady;
            self.steady_run = 0;
        }
        // Three bits of the one frequency: the correlators held nothing
        // else a bit ago, and hold nothing else now. In data that is a
        // run of four bits alike, and an idle line is all of it.
        if self.steady_run >= 3 * BIT_SAMPLES as u32 {
            let (now, then) = if steady > 0 {
                (mark, self.mark_then[self.at])
            } else {
                (space, self.space_then[self.at])
            };
            let turn = Quadrature {
                i: now.i * then.i + now.q * then.q,
                q: now.q * then.i - now.i * then.q,
            };
            // The turn's size goes with the signal's power, and only its
            // direction is wanted: bring it to a length of one before it
            // is averaged, so that a loud moment counts as a quiet one
            // does.
            let size = (turn.i * turn.i + turn.q * turn.q).sqrt();
            if size > 0.0 {
                self.turn.i += (turn.i / size - self.turn.i) / 256.0;
                self.turn.q += (turn.q / size - self.turn.q) / 256.0;
            }
        }
        self.mark_then[self.at] = mark;
        self.space_then[self.at] = space;
        self.at = if self.at + 1 == BIT_SAMPLES {
            0
        } else {
            self.at + 1
        };
    }

    /// Whether the line signal detector is on.
    pub fn carrier(&self) -> bool {
        self.carrier.present()
    }

    /// The power in the band: the mean square, over a few milliseconds.
    pub fn power(&self) -> f32 {
        self.level.power()
    }

    /// The level in the band in dBm0. For diagnostics: it takes a
    /// logarithm.
    pub fn level_dbm0(&self) -> f32 {
        power_to_dbm0(self.level.power(), self.law)
    }

    /// How far the received frequencies are from their nominal values, in
    /// hertz. For diagnostics: it takes an arctangent.
    pub fn frequency_error_hz(&self) -> f32 {
        let angle = math::atan2(f64::from(self.turn.q), f64::from(self.turn.i));
        (angle / math::TAU * f64::from(SAMPLE_RATE) / BIT_SAMPLES as f64) as f32
    }

    /// Forgets the signal.
    pub fn reset(&mut self) {
        self.band.reset();
        self.mark.reset();
        self.space.reset();
        self.level.reset();
        self.carrier.reset();
        self.mark_then = [Quadrature::default(); BIT_SAMPLES];
        self.space_then = [Quadrature::default(); BIT_SAMPLES];
        self.steady = 0;
        self.steady_run = 0;
        self.at = 0;
        self.turn = Quadrature::default();
    }
}
