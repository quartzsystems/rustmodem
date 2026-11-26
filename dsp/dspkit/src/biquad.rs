// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Second-order recursive filter sections, and a DC blocker.
//!
//! The designs are the bilinear transforms of the analogue second-order
//! prototypes, in the form Robert Bristow-Johnson's "Cookbook formulae for
//! audio EQ biquad filter coefficients" gives them.

use crate::SAMPLE_RATE;
use crate::math;

/// The coefficients of a section, with the leading feedback coefficient
/// already divided out:
/// `y[n] = b0 x[n] + b1 x[n-1] + b2 x[n-2] - a1 y[n-1] - a2 y[n-2]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefficients {
    /// Feed-forward, on the newest input.
    pub b0: f32,
    /// Feed-forward, on the input before.
    pub b1: f32,
    /// Feed-forward, on the input two before.
    pub b2: f32,
    /// Feedback, on the output before.
    pub a1: f32,
    /// Feedback, on the output two before.
    pub a2: f32,
}

impl Coefficients {
    /// A section that changes nothing.
    pub const IDENTITY: Coefficients = Coefficients {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    const fn normalised(b: [f64; 3], a: [f64; 3]) -> Coefficients {
        Coefficients {
            b0: (b[0] / a[0]) as f32,
            b1: (b[1] / a[0]) as f32,
            b2: (b[2] / a[0]) as f32,
            a1: (a[1] / a[0]) as f32,
            a2: (a[2] / a[0]) as f32,
        }
    }

    /// A low-pass section: 3 dB down at `corner_hz` when `q` is 0.7071,
    /// which is the Butterworth response, and a gain of one at 0 Hz.
    pub const fn lowpass(corner_hz: f32, q: f32) -> Coefficients {
        let (cos, alpha) = prototype(corner_hz, q);
        Coefficients::normalised(
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    /// A high-pass section: 3 dB down at `corner_hz` when `q` is 0.7071,
    /// and a gain of one at half the sample rate.
    pub const fn highpass(corner_hz: f32, q: f32) -> Coefficients {
        let (cos, alpha) = prototype(corner_hz, q);
        Coefficients::normalised(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    /// A band-pass section with a gain of one at `centre_hz`. `q` is that
    /// of the analogue prototype; the bilinear transform narrows the band,
    /// and it is 3 dB down over `8000 sin(w0) / (2 pi q)` hertz, with `w0`
    /// the centre's angle a sample: nearly `centre_hz / q` low in the band
    /// and less higher up.
    pub const fn bandpass(centre_hz: f32, q: f32) -> Coefficients {
        let (cos, alpha) = prototype(centre_hz, q);
        Coefficients::normalised([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * cos, 1.0 - alpha])
    }

    /// A notch: nothing at `centre_hz`, and a gain of one far from it. It
    /// is as wide at 3 dB as the band-pass of the same `q`.
    pub const fn notch(centre_hz: f32, q: f32) -> Coefficients {
        let (cos, alpha) = prototype(centre_hz, q);
        Coefficients::normalised(
            [1.0, -2.0 * cos, 1.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    /// The gain of the section at a frequency, as a factor.
    pub fn magnitude_at(&self, frequency_hz: f32) -> f32 {
        let omega = math::TAU * f64::from(frequency_hz) / f64::from(SAMPLE_RATE);
        let (c1, s1) = (math::cos(omega), math::sin(omega));
        let (c2, s2) = (math::cos(2.0 * omega), math::sin(2.0 * omega));
        let (b0, b1, b2) = (f64::from(self.b0), f64::from(self.b1), f64::from(self.b2));
        let (a1, a2) = (f64::from(self.a1), f64::from(self.a2));
        let num_re = b0 + b1 * c1 + b2 * c2;
        let num_im = -(b1 * s1 + b2 * s2);
        let den_re = 1.0 + a1 * c1 + a2 * c2;
        let den_im = -(a1 * s1 + a2 * s2);
        let num = num_re * num_re + num_im * num_im;
        let den = den_re * den_re + den_im * den_im;
        (num / den).sqrt() as f32
    }
}

/// The cosine of the corner's angle and the cookbook's alpha.
const fn prototype(frequency_hz: f32, q: f32) -> (f64, f64) {
    let omega = math::TAU * frequency_hz as f64 / SAMPLE_RATE as f64;
    (math::cos(omega), math::sin(omega) / (2.0 * q as f64))
}

/// One second-order section, in the transposed direct form II: two state
/// variables.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    c: Coefficients,
    s1: f32,
    s2: f32,
}

impl Biquad {
    /// A section at rest.
    pub const fn new(coefficients: Coefficients) -> Biquad {
        Biquad {
            c: coefficients,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// The coefficients.
    pub fn coefficients(&self) -> &Coefficients {
        &self.c
    }

    /// Takes one sample and gives one.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.c.b0 * x + self.s1;
        self.s1 = self.c.b1 * x - self.c.a1 * y + self.s2;
        self.s2 = self.c.b2 * x - self.c.a2 * y;
        y
    }

    /// At rest again.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// `N` sections one after another.
#[derive(Clone, Copy, Debug)]
pub struct Cascade<const N: usize> {
    sections: [Biquad; N],
}

impl<const N: usize> Cascade<N> {
    /// The sections, in the order the signal passes through them.
    pub fn new(coefficients: [Coefficients; N]) -> Cascade<N> {
        Cascade {
            sections: coefficients.map(Biquad::new),
        }
    }

    /// Takes one sample and gives one.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let mut y = x;
        for section in &mut self.sections {
            y = section.process(y);
        }
        y
    }

    /// The gain of the whole cascade at a frequency, as a factor.
    pub fn magnitude_at(&self, frequency_hz: f32) -> f32 {
        self.sections
            .iter()
            .map(|section| section.c.magnitude_at(frequency_hz))
            .product()
    }

    /// At rest again.
    pub fn reset(&mut self) {
        for section in &mut self.sections {
            section.reset();
        }
    }
}

/// Removes a constant offset: a zero at 0 Hz and a pole just inside it.
/// `y[n] = x[n] - x[n-1] + r y[n-1]`.
#[derive(Clone, Copy, Debug)]
pub struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    /// A blocker that is 3 dB down at about `corner_hz`.
    pub fn new(corner_hz: f32) -> DcBlocker {
        let r = 1.0 - math::TAU * f64::from(corner_hz) / f64::from(SAMPLE_RATE);
        DcBlocker {
            r: r.clamp(0.0, 0.999_999) as f32,
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// Takes one sample and gives one.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    /// At rest again.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}
