// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! An adaptive equaliser: complex, fractionally spaced, adapted by the
//! least-mean-squares rule.
//!
//! A telephone line does not pass every frequency alike, and what it does
//! to a pulse spreads each symbol over its neighbours. The equaliser is a
//! filter that undoes it, and finds its own taps: each symbol, its output
//! is compared with the symbol that was sent, and every tap is moved a
//! little in the direction that would have made the error smaller.
//!
//! - **Fractionally spaced.** The delay line takes two samples a symbol
//!   and the output is taken once a symbol. Taps half a symbol apart can
//!   make up any timing phase and shape the whole band of the signal,
//!   which taps a symbol apart cannot.
//! - **Training and decision-directed.** The equaliser does not know
//!   which it is in: it is given the error. In training the caller takes
//!   it against the symbol it knows was sent, and afterwards against its
//!   own decision. The step is the caller's to change between the two.
//! - **Normalised.** The step is divided by the power in the delay line,
//!   so the same step does the same at every level. A step of one would
//!   take the whole error out of the next output; a few hundredths is
//!   usual. The power is not taken to be less than a floor the caller
//!   sets: when the signal goes, what is left in the delay line is
//!   nothing much, and a step divided by nothing much would make of the
//!   taps whatever turned that into a symbol.
//! - **Leak.** Two samples a symbol leave directions in which the taps can
//!   grow without changing the output, since the signal has no power
//!   there, and rounding walks them that way. Each update the taps are
//!   shrunk by a small part of themselves, which costs a small error and
//!   keeps them bounded. With a step of 0.02, a leak of a millionth costs
//!   nothing that can be measured and one of a hundred-thousandth costs
//!   1 dB where the error is 32 dB under the signal.
//!
//! The caller puts the carrier loop after the equaliser: the output is
//! turned by the loop's phase, decided, and the error turned back before
//! it is given here.

use crate::complex::Complex;

/// The time constant of the mean-squared error, in symbols.
const MSE_SYMBOLS: f32 = 64.0;

/// The power in the delay line below which nothing is adapted at all.
const POWER_FLOOR: f32 = 1.0e-12;

/// The equaliser, of `N` taps half a symbol apart.
#[derive(Clone, Copy, Debug)]
pub struct Equalizer<const N: usize> {
    taps: [Complex; N],
    // A ring: the newest sample is at `at`.
    line: [Complex; N],
    at: usize,
    step: f32,
    leak: f32,
    floor: f32,
    mse: f32,
}

impl<const N: usize> Equalizer<N> {
    /// An equaliser that passes its input: one tap of one, at the middle.
    pub fn new(step: f32, leak: f32) -> Equalizer<N> {
        let mut equalizer = Equalizer {
            taps: [Complex::ZERO; N],
            line: [Complex::ZERO; N],
            at: 0,
            step: 0.0,
            leak: 0.0,
            floor: POWER_FLOOR,
            mse: 0.0,
        };
        equalizer.set_step(step);
        equalizer.set_leak(leak);
        equalizer.reset();
        equalizer
    }

    /// The delay of the middle tap, in samples of half a symbol.
    pub const fn delay(&self) -> usize {
        N / 2
    }

    /// Changes the step: between none and one.
    pub fn set_step(&mut self, step: f32) {
        self.step = if step.is_nan() {
            0.0
        } else {
            step.clamp(0.0, 1.0)
        };
    }

    /// The step.
    pub fn step(&self) -> f32 {
        self.step
    }

    /// Changes the leak: the part of each tap taken off at every update.
    pub fn set_leak(&mut self, leak: f32) {
        self.leak = if leak.is_nan() {
            0.0
        } else {
            leak.clamp(0.0, 0.1)
        };
    }

    /// Changes the least the power in the delay line is taken to be when
    /// the step is divided by it: the sum of the squares of its `N`
    /// samples.
    pub fn set_power_floor(&mut self, floor: f32) {
        self.floor = if floor.is_nan() {
            POWER_FLOOR
        } else {
            floor.max(POWER_FLOOR)
        };
    }

    /// The taps, the one on the newest sample first.
    pub fn taps(&self) -> &[Complex; N] {
        &self.taps
    }

    /// Puts a sample in the delay line: twice a symbol.
    #[inline]
    pub fn push(&mut self, sample: Complex) {
        self.at = if self.at == 0 { N - 1 } else { self.at - 1 };
        self.line[self.at] = if sample.is_finite() {
            sample
        } else {
            Complex::ZERO
        };
    }

    /// The output for the samples in the delay line now: once a symbol.
    #[inline]
    pub fn output(&self) -> Complex {
        let (older, newer) = self.line.split_at(self.at);
        let mut sum = Complex::ZERO;
        for (&sample, &tap) in newer.iter().chain(older).zip(&self.taps) {
            sum += sample * tap;
        }
        sum
    }

    /// Adapts the taps to the error of the last output: the output less
    /// the symbol it should have been, both as they are at the
    /// equaliser's output. The delay line must not have been moved since
    /// that output was taken.
    #[inline]
    pub fn update(&mut self, error: Complex) {
        if !error.is_finite() {
            return;
        }
        self.mse += (error.norm_sqr() - self.mse) / MSE_SYMBOLS;
        if self.step <= 0.0 {
            return;
        }
        let mut power = 0.0;
        for sample in &self.line {
            power += sample.norm_sqr();
        }
        if power <= POWER_FLOOR {
            return;
        }
        let gain = error.scale(self.step / power.max(self.floor));
        let keep = 1.0 - self.leak;
        let (older, newer) = self.line.split_at(self.at);
        for (&sample, tap) in newer.iter().chain(older).zip(&mut self.taps) {
            *tap = tap.scale(keep) - gain * sample.conj();
        }
    }

    /// Counts an error in the mean-squared error without adapting.
    #[inline]
    pub fn observe(&mut self, error: Complex) {
        if error.is_finite() {
            self.mse += (error.norm_sqr() - self.mse) / MSE_SYMBOLS;
        }
    }

    /// The mean-squared error, smoothed over some sixty symbols.
    pub fn mse(&self) -> f32 {
        self.mse
    }

    /// The equaliser that passes its input, with silence in its delay
    /// line.
    pub fn reset(&mut self) {
        self.taps = [Complex::ZERO; N];
        if N > 0 {
            self.taps[N / 2] = Complex::ONE;
        }
        self.line = [Complex::ZERO; N];
        self.at = 0;
        self.mse = 0.0;
    }
}
