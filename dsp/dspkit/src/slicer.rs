// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Slicers: which point of a constellation a received point is nearest.
//!
//! Three of them. [`Slicer4`] is for four points a quarter turn apart,
//! which need not lie on the diagonals. [`Slicer16`] is for the square of
//! sixteen points at 1 and 3 units each way. [`TableSlicer`] is for any
//! constellation given as a table, and looks at every point.
//!
//! None of them can fail: a point that is not a number is decided as
//! though it were zero.

use crate::complex::Complex;

/// What a slicer decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decision {
    /// Which point: its number in the slicer's order.
    pub index: u16,
    /// The point itself.
    pub point: Complex,
}

fn sane(received: Complex) -> Complex {
    if received.is_finite() {
        received
    } else {
        Complex::ZERO
    }
}

/// A slicer for four points a quarter turn apart.
#[derive(Clone, Copy, Debug)]
pub struct Slicer4 {
    point: Complex,
}

impl Slicer4 {
    /// The slicer whose point 0 is `point`; points 1, 2 and 3 are that
    /// point turned by one, two and three quarter turns anticlockwise.
    pub const fn new(point: Complex) -> Slicer4 {
        Slicer4 { point }
    }

    /// Point `index`, modulo 4.
    #[inline]
    pub fn point(&self, index: u8) -> Complex {
        self.point.rotate_quarters(index)
    }

    /// The mean power of the points.
    pub fn mean_power(&self) -> f32 {
        self.point.norm_sqr()
    }

    /// The nearest point.
    #[inline]
    pub fn slice(&self, received: Complex) -> Decision {
        // Seen from point 0, the four are on the axes, and the nearest is
        // told by which of the two parts is the larger and its sign.
        let seen = sane(received) * self.point.conj();
        let index = if seen.re.abs() >= seen.im.abs() {
            if seen.re >= 0.0 { 0 } else { 2 }
        } else if seen.im >= 0.0 {
            1
        } else {
            3
        };
        Decision {
            index,
            point: self.point.rotate_quarters(index as u8),
        }
    }
}

/// A slicer for the square of sixteen points: 1 and 3 units each way on
/// both axes.
#[derive(Clone, Copy, Debug)]
pub struct Slicer16 {
    unit: f32,
}

impl Slicer16 {
    /// The slicer for points at `unit` and three times `unit`.
    pub const fn new(unit: f32) -> Slicer16 {
        Slicer16 { unit }
    }

    /// The mean power of the points: ten times the square of the unit.
    pub fn mean_power(&self) -> f32 {
        10.0 * self.unit * self.unit
    }

    /// The level, of 0 to 3 from the most negative, nearest a part.
    #[inline]
    fn level(&self, part: f32) -> u16 {
        if part >= 0.0 {
            if part >= 2.0 * self.unit { 3 } else { 2 }
        } else if part < -2.0 * self.unit {
            0
        } else {
            1
        }
    }

    /// Point `index`, modulo 16: the level of the real part is the two
    /// low bits and of the imaginary part the next two.
    #[inline]
    pub fn point(&self, index: u16) -> Complex {
        let part = |level: u16| (2.0 * f32::from(level & 3) - 3.0) * self.unit;
        Complex::new(part(index), part(index >> 2))
    }

    /// The nearest point.
    #[inline]
    pub fn slice(&self, received: Complex) -> Decision {
        let received = sane(received);
        let index = self.level(received.re) | (self.level(received.im) << 2);
        Decision {
            index,
            point: self.point(index),
        }
    }
}

/// A slicer for a constellation given as a table of `N` points. It looks
/// at each, so its work is bounded by `N`.
#[derive(Clone, Copy, Debug)]
pub struct TableSlicer<const N: usize> {
    points: [Complex; N],
}

impl<const N: usize> TableSlicer<N> {
    /// The slicer for these points.
    pub const fn new(points: [Complex; N]) -> TableSlicer<N> {
        TableSlicer { points }
    }

    /// The points.
    pub fn points(&self) -> &[Complex; N] {
        &self.points
    }

    /// The mean power of the points.
    pub fn mean_power(&self) -> f32 {
        if N == 0 {
            return 0.0;
        }
        let mut sum = 0.0;
        for point in &self.points {
            sum += point.norm_sqr();
        }
        sum / N as f32
    }

    /// The nearest point; of two as near, the first in the table. A
    /// table of none gives zero.
    #[inline]
    pub fn slice(&self, received: Complex) -> Decision {
        let received = sane(received);
        let mut best = Decision {
            index: 0,
            point: Complex::ZERO,
        };
        let mut least = f32::INFINITY;
        for (index, &point) in self.points.iter().enumerate() {
            let distance = (received - point).norm_sqr();
            if distance < least {
                least = distance;
                best = Decision {
                    index: index as u16,
                    point,
                };
            }
        }
        best
    }
}
