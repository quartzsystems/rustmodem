// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A complex number of two `f32`: a point of a constellation, a sample of
//! a baseband signal, a tap of an equaliser.

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use crate::osc::{Quadrature, cosine_of_phase, sine_of_phase};

/// A complex number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    /// The real part: the in-phase component.
    pub re: f32,
    /// The imaginary part: the quadrature component.
    pub im: f32,
}

impl Complex {
    /// Zero.
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };

    /// One.
    pub const ONE: Complex = Complex { re: 1.0, im: 0.0 };

    /// The number with these parts.
    #[inline]
    pub const fn new(re: f32, im: f32) -> Complex {
        Complex { re, im }
    }

    /// The number of length one at this phase, a fraction of a turn in 32
    /// bits, read from the oscillator's table.
    #[inline]
    pub fn from_phase(phase: u32) -> Complex {
        Complex {
            re: cosine_of_phase(phase),
            im: sine_of_phase(phase),
        }
    }

    /// The conjugate.
    #[inline]
    pub const fn conj(self) -> Complex {
        Complex {
            re: self.re,
            im: -self.im,
        }
    }

    /// The square of the length: the power of a sample.
    #[inline]
    pub fn norm_sqr(self) -> f32 {
        self.re * self.re + self.im * self.im
    }

    /// The length.
    #[inline]
    pub fn abs(self) -> f32 {
        self.norm_sqr().sqrt()
    }

    /// The number times a real factor.
    #[inline]
    pub fn scale(self, factor: f32) -> Complex {
        Complex {
            re: self.re * factor,
            im: self.im * factor,
        }
    }

    /// The number turned by a phase, a fraction of a turn in 32 bits.
    #[inline]
    pub fn rotate(self, phase: u32) -> Complex {
        self * Complex::from_phase(phase)
    }

    /// The number turned by a quarter turn, `quarters` times: exact.
    #[inline]
    pub fn rotate_quarters(self, quarters: u8) -> Complex {
        match quarters & 3 {
            0 => self,
            1 => Complex::new(-self.im, self.re),
            2 => Complex::new(-self.re, -self.im),
            _ => Complex::new(self.im, -self.re),
        }
    }

    /// Whether both parts are finite.
    #[inline]
    pub fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

impl From<Quadrature> for Complex {
    #[inline]
    fn from(sample: Quadrature) -> Complex {
        Complex {
            re: sample.i,
            im: sample.q,
        }
    }
}

impl Add for Complex {
    type Output = Complex;
    #[inline]
    fn add(self, other: Complex) -> Complex {
        Complex::new(self.re + other.re, self.im + other.im)
    }
}

impl AddAssign for Complex {
    #[inline]
    fn add_assign(&mut self, other: Complex) {
        self.re += other.re;
        self.im += other.im;
    }
}

impl Sub for Complex {
    type Output = Complex;
    #[inline]
    fn sub(self, other: Complex) -> Complex {
        Complex::new(self.re - other.re, self.im - other.im)
    }
}

impl SubAssign for Complex {
    #[inline]
    fn sub_assign(&mut self, other: Complex) {
        self.re -= other.re;
        self.im -= other.im;
    }
}

impl Mul for Complex {
    type Output = Complex;
    #[inline]
    fn mul(self, other: Complex) -> Complex {
        Complex::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }
}

impl Neg for Complex {
    type Output = Complex;
    #[inline]
    fn neg(self) -> Complex {
        Complex::new(-self.re, -self.im)
    }
}
