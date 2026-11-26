// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Detection of one frequency: the Goertzel algorithm over a block, and a
//! sliding detector that has an answer at every sample.

use crate::SAMPLE_RATE;
use crate::math;
use crate::osc::{Oscillator, Quadrature};

/// The Goertzel algorithm: the power of one frequency in a block of
/// samples, for one multiplication a sample.
///
/// The frequency need not be a whole number of cycles in the block. The
/// response to a frequency `d` hertz away is that of a rectangular window
/// of the block's length: nothing at `d = 8000 / len` and its multiples.
#[derive(Clone, Copy, Debug)]
pub struct Goertzel {
    coefficient: f32,
    len: u32,
    s1: f32,
    s2: f32,
    count: u32,
}

impl Goertzel {
    /// A detector of `frequency_hz` over blocks of `len` samples.
    pub fn new(frequency_hz: f32, len: u32) -> Goertzel {
        let omega = math::TAU * f64::from(frequency_hz) / f64::from(SAMPLE_RATE);
        Goertzel {
            coefficient: (2.0 * math::cos(omega)) as f32,
            len: len.max(1),
            s1: 0.0,
            s2: 0.0,
            count: 0,
        }
    }

    /// Takes one sample. When it completes a block, gives the block's
    /// result and starts the next.
    #[inline]
    pub fn process(&mut self, sample: f32) -> Option<f32> {
        let s0 = sample + self.coefficient * self.s1 - self.s2;
        self.s2 = self.s1;
        self.s1 = s0;
        self.count += 1;
        if self.count < self.len {
            return None;
        }
        let power = self.power();
        self.reset();
        Some(power)
    }

    /// The result for the samples taken so far, as if the block ended
    /// here: the mean square a sine wave at the frequency would have to
    /// have to give this response. A sine wave of amplitude `a` at the
    /// frequency reads `a * a / 2`, as on a [`crate::LevelMeter`].
    pub fn power(&self) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        let magnitude_squared =
            self.s1 * self.s1 + self.s2 * self.s2 - self.coefficient * self.s1 * self.s2;
        // The magnitude of the sum is a n / 2 for a sine of amplitude a.
        let n = self.count as f32;
        2.0 * magnitude_squared.max(0.0) / (n * n)
    }

    /// Starts a block again.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
        self.count = 0;
    }
}

/// A detector of one frequency over the last `N` samples, with an answer
/// at every sample.
///
/// The input is multiplied by a local oscillator in both phases and each
/// product is averaged over the last `N` samples. The averages are kept as
/// running sums, and a running sum in floating point drifts, so each is
/// replaced every `N` samples by a sum made afresh over exactly those
/// samples: the error never grows.
#[derive(Clone, Copy, Debug)]
pub struct SlidingGoertzel<const N: usize> {
    oscillator: Oscillator,
    i: [f32; N],
    q: [f32; N],
    at: usize,
    sum: Quadrature,
    fresh: Quadrature,
}

impl<const N: usize> SlidingGoertzel<N> {
    /// A detector of `frequency_hz`.
    pub fn new(frequency_hz: f32) -> SlidingGoertzel<N> {
        SlidingGoertzel {
            oscillator: Oscillator::new(frequency_hz),
            i: [0.0; N],
            q: [0.0; N],
            at: 0,
            sum: Quadrature::default(),
            fresh: Quadrature::default(),
        }
    }

    /// Takes one sample and gives the power at the frequency over the
    /// last `N`: `a * a / 2` for a sine wave of amplitude `a`.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        let phase = self.process_phasor(sample);
        2.0 * (phase.i * phase.i + phase.q * phase.q)
    }

    /// Takes one sample and gives the mean over the last `N` of the input
    /// against the local oscillator, in both phases. Its angle is the
    /// phase of the tone against the oscillator, which is what shows a
    /// phase reversal; its length is half the tone's amplitude.
    #[inline]
    pub fn process_phasor(&mut self, sample: f32) -> Quadrature {
        let local = self.oscillator.next_quadrature();
        let i = sample * local.i;
        let q = -sample * local.q;
        self.sum.i += i - self.i[self.at];
        self.sum.q += q - self.q[self.at];
        self.fresh.i += i;
        self.fresh.q += q;
        self.i[self.at] = i;
        self.q[self.at] = q;
        self.at += 1;
        if self.at == N {
            self.at = 0;
            self.sum = self.fresh;
            self.fresh = Quadrature::default();
        }
        Quadrature {
            i: self.sum.i / N as f32,
            q: self.sum.q / N as f32,
        }
    }

    /// Forgets what it has heard.
    pub fn reset(&mut self) {
        self.i = [0.0; N];
        self.q = [0.0; N];
        self.at = 0;
        self.sum = Quadrature::default();
        self.fresh = Quadrature::default();
    }
}
