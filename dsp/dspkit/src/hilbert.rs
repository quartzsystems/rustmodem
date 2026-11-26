// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A Hilbert transformer, and the single-sideband frequency shifter made
//! of one.
//!
//! The Hilbert transform of a signal is the signal with every frequency
//! turned a quarter of a cycle. The signal and its transform together are
//! the analytic signal, which has no negative frequencies, and multiplying
//! that by a complex oscillator moves every frequency in it by the same
//! number of hertz: what a carrier system whose two ends are a few hertz
//! apart does to a telephone call.

use crate::fir::{Fir, Window};
use crate::math;
use crate::osc::{Oscillator, Quadrature};

/// A Hilbert transformer of `N` taps. `N` is odd; an even `N` is not a
/// transformer of this kind and gives a filter with no output.
///
/// It is good where its window lets it be: with 127 taps, from about
/// 250 Hz to 3750 Hz.
#[derive(Clone, Copy, Debug)]
pub struct Hilbert<const N: usize> {
    fir: Fir<N>,
}

impl<const N: usize> Hilbert<N> {
    /// A transformer at rest.
    pub fn new() -> Hilbert<N> {
        Hilbert {
            fir: Fir::new(hilbert_taps::<N>(Window::Kaiser(6.0))),
        }
    }

    /// The delay of both outputs, in samples.
    pub const fn delay(&self) -> usize {
        (N - 1) / 2
    }

    /// Takes one sample and gives the analytic signal: `i` is the input,
    /// delayed, and `q` its Hilbert transform, so that a cosine in gives a
    /// cosine in `i` and a sine in `q`.
    #[inline]
    pub fn process(&mut self, sample: f32) -> Quadrature {
        let q = self.fir.process(sample);
        Quadrature {
            i: self.fir.tap((N - 1) / 2),
            q,
        }
    }
}

impl<const N: usize> Default for Hilbert<N> {
    fn default() -> Self {
        Hilbert::new()
    }
}

/// The taps of a Hilbert transformer: `2 / (pi k)` at an odd distance `k`
/// from the middle tap and nothing at an even one, under a window.
pub fn hilbert_taps<const N: usize>(window: Window) -> [f32; N] {
    let mut taps = [0f32; N];
    if N.is_multiple_of(2) {
        return taps;
    }
    let middle = (N as i64 - 1) / 2;
    for (n, tap) in taps.iter_mut().enumerate() {
        let k = n as i64 - middle;
        if k % 2 != 0 {
            // `taps[n]` multiplies the sample of n samples ago, which is
            // k samples older than the one the outputs are aligned to.
            *tap = (2.0 / (math::PI * k as f64) * window.at(n, N)) as f32;
        }
    }
    taps
}

/// Moves every frequency of a signal by the same number of hertz.
#[derive(Clone, Copy, Debug)]
pub struct FrequencyShifter<const N: usize> {
    hilbert: Hilbert<N>,
    oscillator: Oscillator,
}

impl<const N: usize> FrequencyShifter<N> {
    /// A shifter by `shift_hz`, up for a positive number and down for a
    /// negative one.
    pub fn new(shift_hz: f32) -> FrequencyShifter<N> {
        FrequencyShifter {
            hilbert: Hilbert::new(),
            oscillator: Oscillator::new(shift_hz),
        }
    }

    /// The delay through the shifter, in samples.
    pub const fn delay(&self) -> usize {
        self.hilbert.delay()
    }

    /// Takes one sample and gives one.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        let analytic = self.hilbert.process(sample);
        let local = self.oscillator.next_quadrature();
        // The real part of (i + jq)(cos + j sin).
        analytic.i * local.i - analytic.q * local.q
    }
}
