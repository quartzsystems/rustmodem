// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The building blocks of the signal processing: oscillators, filters,
//! detectors, and level and gain.
//!
//! Every modulation of rustmodem is made of these. Each block is a plain
//! value with state of a fixed size and a `process` or `next` method that
//! takes a sample and gives a sample: nothing here allocates, locks, reads
//! a clock or calls the system, so all of it may run on a media thread's
//! tick path.
//!
//! # Conventions
//!
//! - The sample rate is 8000 Hz ([`SAMPLE_RATE`]), the rate of G.711
//!   (G.711 clause 2).
//! - Samples are `f32`, with full scale, plus or minus 1.0, at the
//!   overload point of the call's G.711 law.
//! - Levels are in dBm0, and the functions of [`level`] are the only
//!   conversion between dBm0 and the linear scale.
//! - A power is a mean square: one half for a sine wave of amplitude 1.0.
//! - Nothing here uses the sine, exponential or logarithm of the system's
//!   library, whose last bit may differ from one machine to another. The
//!   functions of [`math`] are used to build tables and coefficients, and
//!   give the same bits everywhere.

#![forbid(unsafe_code)]

pub mod biquad;
pub mod fir;
pub mod goertzel;
pub mod hilbert;
pub mod interp;
pub mod level;
pub mod math;
pub mod osc;

pub use biquad::{Biquad, Cascade, Coefficients, DcBlocker};
pub use fir::{Fir, Window};
pub use goertzel::{Goertzel, SlidingGoertzel};
pub use hilbert::{FrequencyShifter, Hilbert};
pub use interp::{FractionalDelay, Resampled, Resampler};
pub use level::{
    Agc, AgcSettings, CarrierDetect, CarrierThresholds, LevelMeter, MovingAverage,
    amplitude_to_dbm0, dbm0_to_amplitude, dbm0_to_power, power_to_dbm0,
};
pub use osc::{Oscillator, Quadrature};

/// Samples a second (G.711 clause 2).
pub const SAMPLE_RATE: u32 = 8000;
