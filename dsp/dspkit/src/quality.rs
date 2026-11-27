// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Signal quality: the mean-squared error of a receiver's decisions as a
//! signal-to-noise ratio.
//!
//! Each symbol, the receiver has the point it decided and how far the
//! received point was from it. The power of the decisions over the power
//! of those errors is the ratio of signal to everything that is not
//! signal: noise, what the equaliser has left, jitter. It is the figure
//! that says how near the receiver is to making errors, and it is right
//! only while the decisions are: when most of them are wrong the errors
//! look smaller than they are.

use crate::complex::Complex;
use crate::math;

/// The least and the greatest ratio reported, in decibels.
const RANGE_DB: (f32, f32) = (0.0, 60.0);

/// A smoothed estimate of the signal-to-noise ratio.
#[derive(Clone, Copy, Debug)]
pub struct SignalQuality {
    signal: f32,
    error: f32,
    coefficient: f32,
    primed: bool,
}

impl SignalQuality {
    /// An estimate smoothed with a time constant of this many symbols.
    pub fn new(symbols: f32) -> SignalQuality {
        let symbols = if symbols.is_nan() { 1.0 } else { symbols };
        SignalQuality {
            signal: 0.0,
            error: 0.0,
            coefficient: 1.0 / symbols.max(1.0),
            primed: false,
        }
    }

    /// Counts one symbol: the point decided, and the received point less
    /// that.
    #[inline]
    pub fn update(&mut self, decision: Complex, error: Complex) {
        if !decision.is_finite() || !error.is_finite() {
            return;
        }
        if self.primed {
            self.signal += (decision.norm_sqr() - self.signal) * self.coefficient;
            self.error += (error.norm_sqr() - self.error) * self.coefficient;
        } else {
            // The first symbol is the whole of what is known.
            self.signal = decision.norm_sqr();
            self.error = error.norm_sqr();
            self.primed = true;
        }
    }

    /// The mean-squared error.
    pub fn mse(&self) -> f32 {
        self.error
    }

    /// The mean power of the decisions.
    pub fn signal_power(&self) -> f32 {
        self.signal
    }

    /// The ratio in decibels, held between 0 and 60; nothing counted is
    /// 0. This takes a logarithm and is for a report, not for every
    /// symbol.
    pub fn snr_db(&self) -> f32 {
        if !self.primed || self.signal <= 0.0 {
            return RANGE_DB.0;
        }
        if self.error <= self.signal * 1.0e-6 {
            return RANGE_DB.1;
        }
        let db = 10.0 * math::log10(f64::from(self.signal) / f64::from(self.error));
        (db as f32).clamp(RANGE_DB.0, RANGE_DB.1)
    }

    /// Nothing counted again.
    pub fn reset(&mut self) {
        self.signal = 0.0;
        self.error = 0.0;
        self.primed = false;
    }
}
