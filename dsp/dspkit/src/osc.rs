// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A numerically controlled oscillator.
//!
//! The phase is a 32-bit accumulator, one turn to the whole range, so it
//! wraps by itself and the frequency is exact to 8000 / 2^32 Hz. The sine
//! is read from a table built at compile time, with a straight line between
//! entries: the error is under one part in a million, which is 120 dB down.

use crate::SAMPLE_RATE;
use crate::math;

/// The entries of the sine table in one turn. A power of two.
const TABLE_BITS: u32 = 12;
const TABLE_SIZE: usize = 1 << TABLE_BITS;

/// One turn of a sine wave, and its first entry again at the end so that
/// the line between entries never looks past the table.
static SINE: [f32; TABLE_SIZE + 1] = sine_table();

const fn sine_table() -> [f32; TABLE_SIZE + 1] {
    let mut table = [0f32; TABLE_SIZE + 1];
    let mut i = 0;
    while i <= TABLE_SIZE {
        table[i] = math::sin(math::TAU * i as f64 / TABLE_SIZE as f64) as f32;
        i += 1;
    }
    // The points that are exact are made exact.
    table[0] = 0.0;
    table[TABLE_SIZE / 4] = 1.0;
    table[TABLE_SIZE / 2] = 0.0;
    table[3 * TABLE_SIZE / 4] = -1.0;
    table[TABLE_SIZE] = 0.0;
    table
}

/// The sine of a phase given as a fraction of a turn in 32 bits.
#[inline]
pub fn sine_of_phase(phase: u32) -> f32 {
    let index = (phase >> (32 - TABLE_BITS)) as usize;
    let fraction_bits = 32 - TABLE_BITS;
    let fraction = (phase & ((1 << fraction_bits) - 1)) as f32 / (1u32 << fraction_bits) as f32;
    let a = SINE[index];
    let b = SINE[index + 1];
    a + (b - a) * fraction
}

/// The cosine of a phase given as a fraction of a turn in 32 bits.
#[inline]
pub fn cosine_of_phase(phase: u32) -> f32 {
    sine_of_phase(phase.wrapping_add(1 << 30))
}

/// The phase step, in 32-bit fractions of a turn a sample, of a frequency
/// in hertz. A negative frequency turns the other way.
pub fn phase_step(frequency_hz: f32) -> u32 {
    let turns = f64::from(frequency_hz) / f64::from(SAMPLE_RATE);
    let turns = turns - math::floor(turns);
    (math::round(turns * 4_294_967_296.0) as u64 & 0xFFFF_FFFF) as u32
}

/// A fraction of a turn, in 32 bits, of an angle in degrees.
pub fn phase_of_degrees(degrees: f32) -> u32 {
    let turns = f64::from(degrees) / 360.0;
    let turns = turns - math::floor(turns);
    (math::round(turns * 4_294_967_296.0) as u64 & 0xFFFF_FFFF) as u32
}

/// A sample of an oscillator in both phases.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quadrature {
    /// The in-phase output: the cosine.
    pub i: f32,
    /// The quadrature output: the sine.
    pub q: f32,
}

/// The oscillator.
#[derive(Clone, Copy, Debug)]
pub struct Oscillator {
    phase: u32,
    step: u32,
}

impl Oscillator {
    /// An oscillator at this frequency, at phase zero.
    pub fn new(frequency_hz: f32) -> Oscillator {
        Oscillator {
            phase: 0,
            step: phase_step(frequency_hz),
        }
    }

    /// Changes the frequency. The phase runs on from where it is, so the
    /// output is continuous: this is how frequency-shift keying is made
    /// with a continuous phase.
    #[inline]
    pub fn set_frequency(&mut self, frequency_hz: f32) {
        self.step = phase_step(frequency_hz);
    }

    /// Changes the frequency to a step worked out before with
    /// [`phase_step`], for a tick path that must not do the division.
    #[inline]
    pub fn set_step(&mut self, step: u32) {
        self.step = step;
    }

    /// The phase step a sample.
    #[inline]
    pub fn step(&self) -> u32 {
        self.step
    }

    /// The phase, as a fraction of a turn in 32 bits.
    #[inline]
    pub fn phase(&self) -> u32 {
        self.phase
    }

    /// Sets the phase.
    #[inline]
    pub fn set_phase(&mut self, phase: u32) {
        self.phase = phase;
    }

    /// Moves the phase on by a step of this many 32-bit fractions of a
    /// turn, at once.
    #[inline]
    pub fn shift_phase(&mut self, by: u32) {
        self.phase = self.phase.wrapping_add(by);
    }

    /// Reverses the phase: a step of half a turn, as the answer tone makes
    /// (V.25 clause 2.3).
    #[inline]
    pub fn reverse_phase(&mut self) {
        self.shift_phase(1 << 31);
    }

    /// The next sample of the sine.
    #[inline]
    pub fn next_sine(&mut self) -> f32 {
        let sample = sine_of_phase(self.phase);
        self.phase = self.phase.wrapping_add(self.step);
        sample
    }

    /// The next sample in both phases.
    #[inline]
    pub fn next_quadrature(&mut self) -> Quadrature {
        let sample = Quadrature {
            i: cosine_of_phase(self.phase),
            q: sine_of_phase(self.phase),
        };
        self.phase = self.phase.wrapping_add(self.step);
        sample
    }
}
