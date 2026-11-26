// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Samples between the samples: a fractional delay, and a resampler by a
//! ratio near one.
//!
//! Both are windowed-sinc interpolators. A band-limited signal is the sum
//! of a sinc at each of its samples, so its value at any time is the
//! samples around that time weighted by a sinc centred there; the window
//! cuts the sum to a number of taps.
//!
//! # Accuracy
//!
//! The resampler has 64 taps and 128 phases with a straight line between
//! neighbouring phases. For a sine wave anywhere from 100 Hz to 3400 Hz
//! and a ratio within one percent of one, its output is the ideal
//! resampled sine wave to better than 70 dB (the tests assert it). Above
//! 3400 Hz the response falls away, to half at 3750 Hz and to nothing by
//! the 4000 Hz that could not be told from its alias.

use crate::fir::{Fir, Window};
use crate::math;

/// The taps of the resampler: its span in input samples.
pub const RESAMPLER_TAPS: usize = 64;

/// The phases the resampler's table has between two input samples.
pub const RESAMPLER_PHASES: usize = 128;

/// The furthest from one a resampling ratio may be.
pub const RESAMPLER_MAX_OFFSET: f64 = 0.05;

/// Where the interpolating low-pass is at half amplitude, as a fraction of
/// half the input sample rate.
const CUTOFF: f64 = 3750.0 / 4000.0;

/// The Kaiser window's beta: about 80 dB of stopband.
const BETA: f32 = 8.0;

/// The taps that give the signal's value `delay` samples before the newest
/// sample in a delay line of `N`, for a `delay` that is not a whole
/// number. They sum to one.
///
/// The window is centred on the point asked for, so the taps are best when
/// that point is near the middle of the line: `delay` near `(N - 1) / 2`.
pub fn fractional_taps<const N: usize>(delay: f64, cutoff: f64, window: Window) -> [f32; N] {
    let mut taps = [0f64; N];
    let mut sum = 0.0;
    let half = N as f64 / 2.0;
    for (m, tap) in taps.iter_mut().enumerate() {
        let x = m as f64 - delay;
        // The window, of width N, centred on the point.
        let position = x / half;
        let shape = if position.abs() >= 1.0 {
            0.0
        } else {
            window_at(window, position)
        };
        *tap = cutoff * math::sinc(cutoff * x) * shape;
        sum += *tap;
    }
    taps.map(|tap| (tap / sum) as f32)
}

/// A window's value at a position from -1 at one edge to 1 at the other.
fn window_at(window: Window, position: f64) -> f64 {
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

/// A delay of a fixed number of samples that need not be whole.
#[derive(Clone, Copy, Debug)]
pub struct FractionalDelay<const N: usize> {
    fir: Fir<N>,
    delay: f64,
}

impl<const N: usize> FractionalDelay<N> {
    /// A delay of `(N - 1) / 2 + fraction` samples, for a `fraction` from
    /// minus a half to a half. The whole part is the interpolator's own
    /// delay, which cannot be had for less.
    pub fn new(fraction: f32) -> FractionalDelay<N> {
        let fraction = f64::from(fraction.clamp(-0.5, 0.5));
        let delay = (N as f64 - 1.0) / 2.0 + fraction;
        FractionalDelay {
            fir: Fir::new(fractional_taps::<N>(delay, 1.0, Window::Kaiser(BETA))),
            delay,
        }
    }

    /// The delay in samples.
    pub fn delay(&self) -> f64 {
        self.delay
    }

    /// Takes one sample and gives the sample of `delay` samples ago.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        self.fir.process(sample)
    }
}

/// What a [`Resampler`] gives for one input sample: none, one or two
/// output samples.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Resampled {
    samples: [f32; 2],
    len: u8,
}

impl Resampled {
    /// The output samples, oldest first.
    #[inline]
    pub fn as_slice(&self) -> &[f32] {
        &self.samples[..self.len as usize]
    }
}

/// Resamples by a ratio near one: the same signal on another clock.
///
/// With a ratio of `1 + 100e-6` it gives 1,000,100 samples for every
/// million it takes: the signal as a far end whose sample clock is 100
/// parts in a million fast would have sampled it.
///
/// The table of taps is in the value itself, 33 KB of it, and is built when
/// the resampler is made. Nothing is allocated.
#[derive(Clone, Debug)]
pub struct Resampler {
    table: [[f32; RESAMPLER_TAPS]; RESAMPLER_PHASES + 1],
    line: Fir<RESAMPLER_TAPS>,
    // Where the next output sample is, in input samples after the input
    // sample before the newest. An output is made while this is below one.
    position: f64,
    // Input samples for each output sample: one over the ratio.
    step: f64,
}

impl Resampler {
    /// A resampler that gives `ratio` output samples for each input
    /// sample. The ratio is held within five percent of one.
    pub fn new(ratio: f64) -> Resampler {
        let mut table = [[0f32; RESAMPLER_TAPS]; RESAMPLER_PHASES + 1];
        let half = RESAMPLER_TAPS as f64 / 2.0;
        for (phase, taps) in table.iter_mut().enumerate() {
            // The output point is `fraction` after the sample before the
            // newest when the line's middle is there, which puts it this
            // many samples before the newest sample in the line.
            let fraction = phase as f64 / RESAMPLER_PHASES as f64;
            *taps =
                fractional_taps::<RESAMPLER_TAPS>(half - fraction, CUTOFF, Window::Kaiser(BETA));
        }
        let mut resampler = Resampler {
            table,
            line: Fir::new([0.0; RESAMPLER_TAPS]),
            position: 0.0,
            step: 1.0,
        };
        resampler.set_ratio(ratio);
        resampler
    }

    /// A resampler for a clock that differs by this many parts in a
    /// million: positive for more output samples than input.
    pub fn with_ppm(ppm: f64) -> Resampler {
        Resampler::new(1.0 + ppm * 1.0e-6)
    }

    /// Changes the ratio from the next output sample on.
    pub fn set_ratio(&mut self, ratio: f64) {
        let ratio = if ratio.is_nan() { 1.0 } else { ratio };
        let ratio = ratio.clamp(1.0 - RESAMPLER_MAX_OFFSET, 1.0 + RESAMPLER_MAX_OFFSET);
        self.step = 1.0 / ratio;
    }

    /// The delay through the resampler, in input samples.
    pub fn delay(&self) -> f64 {
        RESAMPLER_TAPS as f64 / 2.0
    }

    /// Takes one input sample and gives the output samples that are due.
    #[inline]
    pub fn process(&mut self, sample: f32) -> Resampled {
        self.line.push(sample);
        let mut out = Resampled::default();
        while self.position < 1.0 && (out.len as usize) < out.samples.len() {
            out.samples[out.len as usize] = self.interpolate(self.position);
            out.len += 1;
            self.position += self.step;
        }
        self.position -= 1.0;
        out
    }

    #[inline]
    fn interpolate(&self, fraction: f64) -> f32 {
        let scaled = fraction.clamp(0.0, 1.0) * RESAMPLER_PHASES as f64;
        let phase = (scaled as usize).min(RESAMPLER_PHASES - 1);
        let blend = (scaled - phase as f64) as f32;
        let a = self.line.dot(&self.table[phase]);
        let b = self.line.dot(&self.table[phase + 1]);
        a + (b - a) * blend
    }
}
