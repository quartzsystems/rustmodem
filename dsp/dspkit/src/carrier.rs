// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Carrier recovery: a phase-locked loop directed by the receiver's own
//! decisions.
//!
//! A telephone connection may shift every frequency by a few hertz, and
//! the far modem's carrier is not exactly ours, so the constellation the
//! receiver sees turns slowly. Each symbol, the loop is given the point
//! received and the point it was decided to be; the angle between them is
//! the error, and the loop turns its own phase to take it out. The point
//! is turned back by the loop's phase before it is decided.
//!
//! # The detector
//!
//! The error is the imaginary part of the received point times the
//! conjugate of the decision, over the mean power of the constellation.
//! For a small angle that is the angle in radians, weighted by how far
//! out the point is: a point near the middle says less about the phase
//! than one at a corner, and is believed less.
//!
//! # The loop
//!
//! Proportional and integral, updated once a symbol. The integral is the
//! frequency offset, so a constant offset is followed with no standing
//! error. [`CarrierSettings::with_bandwidth`] gives the gains of a loop
//! with a stated noise bandwidth; at 600 baud a bandwidth of 0.03 of the
//! symbol rate is 18 Hz, which pulls in an offset of 10 Hz on four points
//! within a second.
//!
//! The loop's phase is a fraction of a turn in 32 bits, and its rotations
//! read the oscillator's table: nothing here calls a sine.

use crate::complex::Complex;
use crate::timing::loop_gains;

const TAU: f32 = std::f32::consts::TAU;

/// The gains of the loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarrierSettings {
    /// How much of an error, in radians, is taken out of the phase at
    /// once.
    pub proportional: f32,
    /// How much of an error goes into the estimate of the frequency.
    pub integral: f32,
    /// The most the frequency offset may be taken to be, in turns a
    /// symbol.
    pub max_frequency: f32,
}

impl CarrierSettings {
    /// The gains of a loop with this noise bandwidth, as a fraction of the
    /// symbol rate, and this damping.
    pub fn with_bandwidth(bandwidth: f32, damping: f32) -> CarrierSettings {
        let (proportional, integral) = loop_gains(bandwidth, damping, 1.0);
        CarrierSettings {
            proportional,
            integral,
            max_frequency: 0.05,
        }
    }
}

/// The carrier loop of one receiver.
#[derive(Clone, Copy, Debug)]
pub struct CarrierLoop {
    settings: CarrierSettings,
    // The phase of the received carrier against ours, a fraction of a
    // turn in 32 bits.
    phase: u32,
    // The frequency offset, in turns a symbol.
    frequency: f32,
    error: f32,
}

impl CarrierLoop {
    /// A loop at rest: no phase and no offset.
    pub fn new(settings: CarrierSettings) -> CarrierLoop {
        CarrierLoop {
            settings,
            phase: 0,
            frequency: 0.0,
            error: 0.0,
        }
    }

    /// Changes the gains; the loop keeps its phase and frequency.
    pub fn set_settings(&mut self, settings: CarrierSettings) {
        self.settings = settings;
    }

    /// The received point with the loop's phase taken out: the point to
    /// decide.
    #[inline]
    pub fn derotate(&self, received: Complex) -> Complex {
        received.rotate(self.phase.wrapping_neg())
    }

    /// A point of the constellation, or an error measured there, with the
    /// loop's phase put in: as it is where the equaliser works.
    #[inline]
    pub fn rotate(&self, point: Complex) -> Complex {
        point.rotate(self.phase)
    }

    /// The error of a derotated point against its decision, in radians
    /// for a small error. `mean_power` is that of the constellation.
    #[inline]
    pub fn phase_error(derotated: Complex, decision: Complex, mean_power: f32) -> f32 {
        if mean_power <= 0.0 {
            return 0.0;
        }
        let error = (derotated.im * decision.re - derotated.re * decision.im) / mean_power;
        if error.is_finite() {
            error.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }

    /// Moves the loop for one symbol, given the error of that symbol's
    /// point.
    #[inline]
    pub fn update(&mut self, error: f32) {
        self.error = error;
        let limit = self.settings.max_frequency.abs();
        self.frequency =
            (self.frequency + self.settings.integral * error / TAU).clamp(-limit, limit);
        let turns = self.frequency + self.settings.proportional * error / TAU;
        self.phase = self.phase.wrapping_add(turns_to_phase(turns));
    }

    /// Moves the loop for one symbol with no error to go by: the phase
    /// runs on at the frequency found.
    #[inline]
    pub fn coast(&mut self) {
        self.phase = self.phase.wrapping_add(turns_to_phase(self.frequency));
    }

    /// The phase, a fraction of a turn in 32 bits.
    pub fn phase(&self) -> u32 {
        self.phase
    }

    /// Turns the phase by a step, at once.
    pub fn shift_phase(&mut self, by: u32) {
        self.phase = self.phase.wrapping_add(by);
    }

    /// The frequency offset found, in hertz, at this symbol rate.
    pub fn frequency_hz(&self, symbol_rate: f32) -> f32 {
        self.frequency * symbol_rate
    }

    /// The last error given.
    pub fn error(&self) -> f32 {
        self.error
    }

    /// No phase and no offset again.
    pub fn reset(&mut self) {
        self.phase = 0;
        self.frequency = 0.0;
        self.error = 0.0;
    }
}

/// A small number of turns, either way, as a step of a 32-bit phase.
#[inline]
fn turns_to_phase(turns: f32) -> u32 {
    // Within a turn either way the product fits an `i64`, and the cast to
    // 32 bits wraps as the phase does.
    let turns = if turns.is_finite() {
        turns.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (f64::from(turns) * 4_294_967_296.0) as i64 as u32
}
