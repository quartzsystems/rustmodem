// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Pulse shaping: the square root of a raised cosine, as the filter that
//! makes a transmitter's symbols into a signal and as the filter matched
//! to it in the receiver.
//!
//! A raised cosine is a pulse that is zero at every symbol instant but its
//! own, so symbols shaped by it do not disturb one another where they are
//! sampled. Its spectrum is split evenly between the two ends: each has a
//! filter whose response is the square root of the raised cosine's, and
//! the receiver's is then also the filter matched to the transmitter's
//! pulse, which is the one that lets the least noise in.
//!
//! The roll-off is how much wider than the symbol rate the spectrum is:
//! with a roll-off of 0.75 at 600 baud the signal is 1050 Hz wide.
//!
//! # Rates
//!
//! The symbol rates of modems do not divide 8000 Hz: at 600 baud there are
//! thirteen samples and a third to a symbol. [`PulseShaper`] is therefore a
//! polyphase interpolator. It has the pulse at `PHASES` points a symbol and
//! steps through them `step` at a time, so that `PHASES / step` is the
//! number of samples in a symbol: 40 and 3 at 600 baud.

use crate::complex::Complex;
use crate::fir::Window;
use crate::math;

/// The square root of a raised cosine with this roll-off, at a time in
/// symbols from its peak. Its energy is one, and so is the peak of the
/// raised cosine that two of them make.
pub fn root_raised_cosine(t: f64, rolloff: f64) -> f64 {
    let b = if rolloff.is_nan() {
        0.0
    } else {
        rolloff.clamp(0.0, 1.0)
    };
    if t.abs() < 1.0e-9 {
        return 1.0 - b + 4.0 * b / math::PI;
    }
    let x = 4.0 * b * t;
    if b > 0.0 && (x.abs() - 1.0).abs() < 1.0e-9 {
        // Where the general form is nothing over nothing.
        let a = math::PI / (4.0 * b);
        return b / std::f64::consts::SQRT_2
            * ((1.0 + 2.0 / math::PI) * math::sin(a) + (1.0 - 2.0 / math::PI) * math::cos(a));
    }
    (math::sin(math::PI * t * (1.0 - b)) + x * math::cos(math::PI * t * (1.0 + b)))
        / (math::PI * t * (1.0 - x * x))
}

/// The window's value at a position from -1 at one edge to 1 at the
/// other, and nothing outside.
fn window_at(window: Window, position: f64) -> f64 {
    if position.abs() >= 1.0 {
        return 0.0;
    }
    match window {
        Window::Hamming => 0.54 + 0.46 * math::cos(math::PI * position),
        Window::Blackman => {
            0.42 + 0.5 * math::cos(math::PI * position)
                + 0.08 * math::cos(2.0 * math::PI * position)
        }
        Window::Kaiser(beta) => {
            let beta = f64::from(beta);
            let inside = (1.0 - position * position).max(0.0);
            math::bessel_i0(beta * inside.sqrt()) / math::bessel_i0(beta)
        }
    }
}

/// The taps of the receiver's matched filter: the pulse at
/// `samples_per_symbol` samples a symbol, centred in `N` taps and shaped
/// by the window.
///
/// They are scaled so that the filter gives a symbol back at the size it
/// was sent: the signal a [`PulseShaper`] of the same roll-off makes of a
/// symbol `a`, through this filter and sampled at the symbol's instant,
/// is `a`. `Window::Kaiser(0.0)` is no window.
pub fn matched_taps<const N: usize>(
    rolloff: f64,
    samples_per_symbol: f64,
    window: Window,
) -> [f32; N] {
    let middle = (N as f64 - 1.0) / 2.0;
    let half = N as f64 / 2.0;
    let mut pulse = [0f64; N];
    // Scaled against the pulse the transmitter sends, which has an energy
    // of a symbol's samples: the sum of the products is to be one.
    let mut gain = 0.0;
    for (n, tap) in pulse.iter_mut().enumerate() {
        let x = n as f64 - middle;
        let ideal = root_raised_cosine(x / samples_per_symbol, rolloff);
        *tap = ideal * window_at(window, x / half);
        gain += *tap * ideal;
    }
    if gain <= 0.0 {
        return [0.0; N];
    }
    pulse.map(|tap| (tap / gain) as f32)
}

/// The transmitter's shaping filter: symbols in, the baseband signal out,
/// at `PHASES / step` samples a symbol. `SPAN` is the length of the pulse
/// in symbols.
///
/// A stream of symbols of mean power one makes a signal of mean power
/// one. The signal is late by half of `SPAN` symbols.
///
/// Before each sample, [`PulseShaper::wants_symbol`] says whether the next
/// symbol is due, and if it is, it is given with [`PulseShaper::push`].
#[derive(Clone, Debug)]
pub struct PulseShaper<const PHASES: usize, const SPAN: usize> {
    // The pulse at each phase, for the newest symbol first.
    table: [[f32; SPAN]; PHASES],
    // A ring: the newest symbol is at `at`.
    symbols: [Complex; SPAN],
    at: usize,
    phase: usize,
    step: usize,
    due: bool,
}

impl<const PHASES: usize, const SPAN: usize> PulseShaper<PHASES, SPAN> {
    /// A shaper of this roll-off that moves `step` phases a sample.
    /// `step` is held between 1 and `PHASES`.
    pub fn new(rolloff: f64, step: usize, window: Window) -> PulseShaper<PHASES, SPAN> {
        let mut table = [[0f32; SPAN]; PHASES];
        let mut pulse = [[0f64; SPAN]; PHASES];
        let half = SPAN as f64 / 2.0;
        let mut energy = 0.0;
        for (phase, row) in pulse.iter_mut().enumerate() {
            for (age, tap) in row.iter_mut().enumerate() {
                // The time of this sample from the peak of the symbol's
                // pulse, in symbols: the newest symbol's peak is half the
                // span ahead.
                let t = phase as f64 / PHASES as f64 + age as f64 - half;
                *tap = root_raised_cosine(t, rolloff) * window_at(window, t / half);
                energy += *tap * *tap;
            }
        }
        // The power of the signal is the power of the symbols times the
        // energy of the pulse over a symbol's samples.
        let scale = if energy > 0.0 {
            (PHASES as f64 / energy).sqrt()
        } else {
            0.0
        };
        for (row, pulse) in table.iter_mut().zip(&pulse) {
            for (tap, value) in row.iter_mut().zip(pulse) {
                *tap = (value * scale) as f32;
            }
        }
        PulseShaper {
            table,
            symbols: [Complex::ZERO; SPAN],
            at: 0,
            phase: 0,
            step: step.clamp(1, PHASES.max(1)),
            due: true,
        }
    }

    /// The delay from a symbol's being given to the peak of its pulse, in
    /// symbols.
    pub const fn delay_symbols(&self) -> usize {
        SPAN / 2
    }

    /// Samples in a symbol.
    pub fn samples_per_symbol(&self) -> f64 {
        PHASES as f64 / self.step as f64
    }

    /// Whether a symbol is to be given before the next sample is taken.
    #[inline]
    pub fn wants_symbol(&self) -> bool {
        self.due
    }

    /// Gives the next symbol.
    #[inline]
    pub fn push(&mut self, symbol: Complex) {
        self.at = if self.at == 0 { SPAN - 1 } else { self.at - 1 };
        self.symbols[self.at] = symbol;
        self.due = false;
    }

    /// The next sample of the signal. If a symbol was due and was not
    /// given, the signal has silence in its place.
    #[inline]
    pub fn next_sample(&mut self) -> Complex {
        if self.due {
            self.push(Complex::ZERO);
        }
        let taps = &self.table[self.phase];
        let (older, newer) = self.symbols.split_at(self.at);
        let mut sum = Complex::ZERO;
        for (symbol, &tap) in newer.iter().chain(older).zip(taps) {
            sum.re += symbol.re * tap;
            sum.im += symbol.im * tap;
        }
        self.phase += self.step;
        if self.phase >= PHASES {
            self.phase -= PHASES;
            self.due = true;
        }
        sum
    }

    /// Silence in the filter, and a symbol due.
    pub fn reset(&mut self) {
        self.symbols = [Complex::ZERO; SPAN];
        self.at = 0;
        self.phase = 0;
        self.due = true;
    }
}

/// A filter of `N` real taps over complex samples: the receiver's matched
/// filter, and the delay line of an interpolator.
#[derive(Clone, Copy, Debug)]
pub struct ComplexFir<const N: usize> {
    taps: [f32; N],
    // A ring: the newest sample is at `at`.
    line: [Complex; N],
    at: usize,
}

impl<const N: usize> ComplexFir<N> {
    /// A filter with these taps and a delay line of silence. `taps[0]`
    /// multiplies the newest sample.
    pub fn new(taps: [f32; N]) -> ComplexFir<N> {
        ComplexFir {
            taps,
            line: [Complex::ZERO; N],
            at: 0,
        }
    }

    /// The taps.
    pub fn taps(&self) -> &[f32; N] {
        &self.taps
    }

    /// Puts a sample in the delay line without working out an output.
    #[inline]
    pub fn push(&mut self, sample: Complex) {
        self.at = if self.at == 0 { N - 1 } else { self.at - 1 };
        self.line[self.at] = sample;
    }

    /// The output for the samples in the delay line now.
    #[inline]
    pub fn output(&self) -> Complex {
        self.dot(&self.taps)
    }

    /// The samples in the delay line against another set of taps.
    #[inline]
    pub fn dot(&self, taps: &[f32; N]) -> Complex {
        let (older, newer) = self.line.split_at(self.at);
        let mut sum = Complex::ZERO;
        for (sample, &tap) in newer.iter().chain(older).zip(taps) {
            sum.re += sample.re * tap;
            sum.im += sample.im * tap;
        }
        sum
    }

    /// Takes one sample and gives one.
    #[inline]
    pub fn process(&mut self, sample: Complex) -> Complex {
        self.push(sample);
        self.output()
    }

    /// Silence in the delay line again.
    pub fn reset(&mut self) {
        self.line = [Complex::ZERO; N];
        self.at = 0;
    }
}
