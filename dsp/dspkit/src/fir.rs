// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Finite impulse response filters with a fixed number of taps, and the
//! windowed-sinc designs for them.

use crate::SAMPLE_RATE;
use crate::math;

/// A filter of `N` taps. Its state is a delay line of `N` samples; nothing
/// is allocated.
#[derive(Clone, Copy, Debug)]
pub struct Fir<const N: usize> {
    taps: [f32; N],
    // A ring: the newest sample is at `at` and the older ones follow it,
    // wrapping at the end.
    line: [f32; N],
    at: usize,
}

impl<const N: usize> Fir<N> {
    /// A filter with these taps and a delay line of silence. `taps[0]`
    /// multiplies the newest sample.
    pub fn new(taps: [f32; N]) -> Fir<N> {
        Fir {
            taps,
            line: [0.0; N],
            at: 0,
        }
    }

    /// The taps.
    pub fn taps(&self) -> &[f32; N] {
        &self.taps
    }

    /// Puts a sample in the delay line without working out an output.
    #[inline]
    pub fn push(&mut self, sample: f32) {
        self.at = if self.at == 0 { N - 1 } else { self.at - 1 };
        self.line[self.at] = sample;
    }

    /// The output for the samples in the delay line now.
    #[inline]
    pub fn output(&self) -> f32 {
        self.dot(&self.taps)
    }

    /// The samples in the delay line against another set of taps: for a
    /// filter bank that shares one delay line, such as an interpolator.
    #[inline]
    pub fn dot(&self, taps: &[f32; N]) -> f32 {
        let (older, newer) = self.line.split_at(self.at);
        let mut sum = 0.0;
        for (sample, tap) in newer.iter().chain(older).zip(taps) {
            sum += sample * tap;
        }
        sum
    }

    /// Takes one sample and gives one.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        self.push(sample);
        self.output()
    }

    /// The sample that went in `delay` samples ago; zero is the newest.
    #[inline]
    pub fn tap(&self, delay: usize) -> f32 {
        let index = self.at + delay % N;
        self.line[if index < N { index } else { index - N }]
    }

    /// Silence in the delay line again.
    pub fn reset(&mut self) {
        self.line = [0.0; N];
        self.at = 0;
    }
}

/// The window a windowed-sinc design is shaped by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Window {
    /// The Hamming window: a first side lobe 43 dB down.
    Hamming,
    /// The Blackman window: 58 dB down, and a transition half as wide
    /// again.
    Blackman,
    /// The Kaiser window with this beta. 5.65 gives 60 dB of stopband and
    /// 8.6 gives 90 dB.
    Kaiser(f32),
}

impl Window {
    /// The window's value at tap `n` of `len`.
    pub const fn at(self, n: usize, len: usize) -> f64 {
        if len < 2 {
            return 1.0;
        }
        // From -1 at the first tap to 1 at the last.
        let x = 2.0 * n as f64 / (len - 1) as f64 - 1.0;
        match self {
            Window::Hamming => 0.54 + 0.46 * math::cos(math::PI * x),
            Window::Blackman => {
                0.42 + 0.5 * math::cos(math::PI * x) + 0.08 * math::cos(2.0 * math::PI * x)
            }
            Window::Kaiser(beta) => {
                let beta = beta as f64;
                let inside = 1.0 - x * x;
                let inside = if inside < 0.0 { 0.0 } else { inside };
                math::bessel_i0(beta * sqrt(inside)) / math::bessel_i0(beta)
            }
        }
    }
}

/// A square root that can be taken at compile time, by Newton's method.
const fn sqrt(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut root = if x > 1.0 { x } else { 1.0 };
    let mut n = 0;
    while n < 60 {
        root = 0.5 * (root + x / root);
        n += 1;
    }
    root
}

/// The taps of a low-pass filter that passes up to `cutoff_hz`, where its
/// gain is a half. The gain at 0 Hz is exactly one.
pub const fn lowpass<const N: usize>(cutoff_hz: f32, window: Window) -> [f32; N] {
    let ideal = ideal_lowpass::<N>(cutoff_hz as f64, window);
    let mut sum = 0.0;
    let mut n = 0;
    while n < N {
        sum += ideal[n];
        n += 1;
    }
    let mut taps = [0f32; N];
    let mut n = 0;
    while n < N {
        taps[n] = (ideal[n] / sum) as f32;
        n += 1;
    }
    taps
}

/// The taps of a band-pass filter between `low_hz` and `high_hz`. The gain
/// at the middle of the band is exactly one.
pub const fn bandpass<const N: usize>(low_hz: f32, high_hz: f32, window: Window) -> [f32; N] {
    let upper = ideal_lowpass::<N>(high_hz as f64, window);
    let lower = ideal_lowpass::<N>(low_hz as f64, window);
    let centre = (low_hz as f64 + high_hz as f64) / 2.0;
    let middle = (N as f64 - 1.0) / 2.0;
    let mut ideal = [0f64; N];
    // The gain at the centre: the filter is symmetric, so its response
    // there is the sum of the taps against a cosine about the middle tap.
    let mut gain = 0.0;
    let mut n = 0;
    while n < N {
        ideal[n] = upper[n] - lower[n];
        gain += ideal[n] * math::cos(math::TAU * centre / SAMPLE_RATE as f64 * (n as f64 - middle));
        n += 1;
    }
    let mut taps = [0f32; N];
    let mut n = 0;
    while n < N {
        taps[n] = (ideal[n] / gain) as f32;
        n += 1;
    }
    taps
}

const fn ideal_lowpass<const N: usize>(cutoff_hz: f64, window: Window) -> [f64; N] {
    let middle = (N as f64 - 1.0) / 2.0;
    // The cutoff as a fraction of the sample rate, twice: the sinc's first
    // zero is at 1 / (2 fc) samples.
    let width = 2.0 * cutoff_hz / SAMPLE_RATE as f64;
    let mut taps = [0f64; N];
    let mut n = 0;
    while n < N {
        let x = n as f64 - middle;
        taps[n] = width * math::sinc(width * x) * window.at(n, N);
        n += 1;
    }
    taps
}

/// The gain of a set of taps at a frequency, as a factor: the magnitude of
/// the frequency response.
pub fn magnitude_at(taps: &[f32], frequency_hz: f32) -> f32 {
    let omega = math::TAU * f64::from(frequency_hz) / f64::from(SAMPLE_RATE);
    let mut re = 0.0;
    let mut im = 0.0;
    for (n, &tap) in taps.iter().enumerate() {
        re += f64::from(tap) * math::cos(omega * n as f64);
        im -= f64::from(tap) * math::sin(omega * n as f64);
    }
    (re * re + im * im).sqrt() as f32
}
