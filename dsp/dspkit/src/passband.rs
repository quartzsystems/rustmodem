// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Between baseband and the line: a quadrature modulator and demodulator
//! at a carrier.
//!
//! The modulator makes the real signal whose two phases at the carrier
//! are the two parts of the baseband signal. The demodulator multiplies
//! the line by the carrier in both phases, which brings the signal back to
//! baseband and leaves a copy of it at twice the carrier: the low-pass
//! filter that follows, the receiver's matched filter, removes the copy.
//!
//! Each is scaled by the square root of two, so that power is the same on
//! both sides: a baseband signal of mean power one is a line signal of
//! mean square one, and comes back at the size it went.

use crate::complex::Complex;
use crate::osc::Oscillator;

const SQRT_2: f32 = std::f32::consts::SQRT_2;

/// Baseband to the line.
#[derive(Clone, Copy, Debug)]
pub struct Modulator {
    carrier: Oscillator,
}

impl Modulator {
    /// A modulator at this carrier.
    pub fn new(carrier_hz: f32) -> Modulator {
        Modulator {
            carrier: Oscillator::new(carrier_hz),
        }
    }

    /// The line's sample for one sample of baseband.
    #[inline]
    pub fn process(&mut self, baseband: Complex) -> f32 {
        let carrier = self.carrier.next_quadrature();
        SQRT_2 * (baseband.re * carrier.i - baseband.im * carrier.q)
    }
}

/// The line to baseband, with the copy at twice the carrier still in it.
#[derive(Clone, Copy, Debug)]
pub struct Demodulator {
    carrier: Oscillator,
}

impl Demodulator {
    /// A demodulator at this carrier.
    pub fn new(carrier_hz: f32) -> Demodulator {
        Demodulator {
            carrier: Oscillator::new(carrier_hz),
        }
    }

    /// The baseband sample for one sample of the line.
    #[inline]
    pub fn process(&mut self, sample: f32) -> Complex {
        let carrier = self.carrier.next_quadrature();
        let scaled = SQRT_2 * sample;
        Complex::new(scaled * carrier.i, -scaled * carrier.q)
    }
}
