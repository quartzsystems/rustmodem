// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Symbol timing recovery: where in the far end's symbols the samples
//! fall, and following its clock as it drifts from ours.
//!
//! The receiver's samples are taken by the call's clock and the far
//! modem's symbols are made by its own, so the symbol instants move
//! through the samples: at 200 parts in a million and 600 baud, by a whole
//! symbol every eight seconds. This block takes the output of the matched
//! filter at the sample rate and gives it back at two points a symbol, one
//! at the symbol instant and one midway, by interpolating between the
//! samples. That is the rate a fractionally spaced equaliser wants.
//!
//! # The detector
//!
//! The detector is Gardner's. For each symbol it takes the sample midway
//! between two symbol instants and multiplies it by the difference of the
//! two: where the signal crosses from one symbol to another the midway
//! sample is nothing if the timing is right, and leans towards the later
//! symbol if the timing is late. The real part of the product, summed over
//! the two phases, is the error. It was chosen because it needs no
//! decisions and does not care what the carrier's phase is, so it locks
//! before the equaliser and the carrier loop have and does not depend on
//! either; and because it works at the two samples a symbol that are
//! wanted anyway. Its price is a jitter of its own making, from symbols
//! whose midway point is not at zero, which a narrow loop averages away.
//!
//! The error is divided by the power of the signal, so the loop behaves
//! the same at every level. At a roll-off of 0.75 the slope of the error
//! against the timing, in symbols, is [`GARDNER_GAIN_75`].
//!
//! # The loop
//!
//! Proportional and integral. The integral is the far end's clock against
//! ours, and with it the loop follows a constant offset with no standing
//! error.

use crate::complex::Complex;
use crate::fir::Window;
use crate::interp::fractional_taps;
use crate::shape::ComplexFir;

/// The taps of the interpolator.
const TAPS: usize = 8;

/// The phases its table has between two samples.
const PHASES: usize = 32;

/// Where the interpolating low-pass is at half amplitude, as a fraction
/// of half the sample rate. The signals this is for are a few hundred
/// hertz wide at baseband.
const CUTOFF: f64 = 0.5;

/// The slope of the detector's error against the timing in symbols, for
/// a signal shaped with a roll-off of 0.75: the tests measure it.
pub const GARDNER_GAIN_75: f32 = 2.2;

/// The gains of the loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimingSettings {
    /// How much of an error is taken out at once, in symbols for each
    /// unit of the detector's output.
    pub proportional: f32,
    /// How much of an error goes into the estimate of the clock offset.
    pub integral: f32,
    /// The most the clock offset may be taken to be, as a fraction: 0.001
    /// is a thousand parts in a million.
    pub max_offset: f32,
    /// The time constant of the measure of the signal's power, in symbols.
    pub power_symbols: f32,
}

impl TimingSettings {
    /// The gains of a loop with this noise bandwidth, as a fraction of the
    /// symbol rate, and this damping, for a detector of this slope.
    pub fn with_bandwidth(bandwidth: f32, damping: f32, detector_gain: f32) -> TimingSettings {
        let (proportional, integral) = loop_gains(bandwidth, damping, detector_gain);
        TimingSettings {
            proportional,
            integral,
            max_offset: 1.0e-3,
            power_symbols: 32.0,
        }
    }
}

/// The proportional and integral gains of a second-order loop with a
/// noise bandwidth of `bandwidth` updates and this damping, for a detector
/// of this slope.
pub fn loop_gains(bandwidth: f32, damping: f32, detector_gain: f32) -> (f32, f32) {
    let damping = if damping > 0.0 { damping } else { 0.707 };
    let gain = if detector_gain > 0.0 {
        detector_gain
    } else {
        1.0
    };
    let theta = bandwidth.max(0.0) / (damping + 1.0 / (4.0 * damping));
    let d = 1.0 + 2.0 * damping * theta + theta * theta;
    (
        4.0 * damping * theta / d / gain,
        4.0 * theta * theta / d / gain,
    )
}

/// One of the two samples a symbol the block gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strobe {
    /// The signal at this point.
    pub sample: Complex,
    /// Whether this is the symbol instant; otherwise it is the point
    /// midway to the next.
    pub on_symbol: bool,
    /// When the point is: this part of a sample after the sample just
    /// given, less the interpolator's delay.
    pub fraction: f32,
}

/// The timing recovery of one receiver.
#[derive(Clone, Debug)]
pub struct TimingRecovery {
    table: [[f32; TAPS]; PHASES + 1],
    line: ComplexFir<TAPS>,
    settings: TimingSettings,
    // Samples in half a symbol, by our clock, if the far end's were ours.
    nominal: f64,
    // Where the next strobe is, in samples after the sample before the
    // newest. A strobe is made when this is below one.
    position: f64,
    // The far end's clock against ours, as a fraction: the loop's
    // integral.
    offset: f32,
    // What the loop takes out of the period now: the offset and the
    // proportional part.
    correction: f32,
    next_on_symbol: bool,
    previous: Complex,
    middle: Complex,
    power: f32,
    power_coefficient: f32,
    error: f32,
    frozen: bool,
}

impl TimingRecovery {
    /// A loop for a signal of `samples_per_symbol` samples a symbol, which
    /// is to be more than two.
    pub fn new(samples_per_symbol: f64, settings: TimingSettings) -> TimingRecovery {
        let mut table = [[0f32; TAPS]; PHASES + 1];
        let half = TAPS as f64 / 2.0;
        for (phase, taps) in table.iter_mut().enumerate() {
            let fraction = phase as f64 / PHASES as f64;
            *taps = fractional_taps::<TAPS>(half - fraction, CUTOFF, Window::Kaiser(6.0));
        }
        let samples = if samples_per_symbol.is_finite() {
            samples_per_symbol.max(2.5)
        } else {
            2.5
        };
        TimingRecovery {
            table,
            line: ComplexFir::new([0.0; TAPS]),
            settings,
            nominal: samples / 2.0,
            position: 0.0,
            offset: 0.0,
            correction: 0.0,
            next_on_symbol: false,
            previous: Complex::ZERO,
            middle: Complex::ZERO,
            power: 0.0,
            power_coefficient: 1.0 / settings.power_symbols.max(1.0),
            error: 0.0,
            frozen: false,
        }
    }

    /// Changes the gains; the loop keeps what it has learned.
    pub fn set_settings(&mut self, settings: TimingSettings) {
        self.settings = settings;
        self.power_coefficient = 1.0 / settings.power_symbols.max(1.0);
    }

    /// Stops the loop from learning, or lets it again. Frozen, it keeps
    /// strobing at the rate it has found: for a signal that has gone and
    /// may come back.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
        if frozen {
            self.correction = self.offset;
        }
    }

    /// The far end's symbol clock against ours, in parts in a million:
    /// positive when the far end's is the faster.
    pub fn offset_ppm(&self) -> f32 {
        self.offset * 1.0e6
    }

    /// The detector's last error.
    pub fn error(&self) -> f32 {
        self.error
    }

    /// The delay of the interpolator, in samples.
    pub fn delay(&self) -> f64 {
        TAPS as f64 / 2.0
    }

    /// Moves the strobes later by this part of a symbol, at once. For
    /// tests, and for a receiver that knows better than the loop.
    pub fn slip(&mut self, symbols: f64) {
        self.position += symbols * 2.0 * self.nominal;
    }

    /// Takes one sample of the matched filter's output, and gives the
    /// strobe that falls in it if one does.
    #[inline]
    pub fn process(&mut self, sample: Complex) -> Option<Strobe> {
        self.line.push(sample);
        let mut strobe = None;
        if self.position < 1.0 {
            let value = self.interpolate(self.position);
            let on_symbol = self.next_on_symbol;
            if on_symbol {
                self.detect(value);
                self.previous = value;
            } else {
                self.middle = value;
            }
            self.next_on_symbol = !on_symbol;
            let fraction = self.position.clamp(0.0, 1.0) as f32;
            self.position += self.nominal * (1.0 - f64::from(self.correction));
            strobe = Some(Strobe {
                sample: value,
                on_symbol,
                fraction,
            });
        }
        self.position -= 1.0;
        strobe
    }

    /// The detector and the loop, at a symbol instant.
    #[inline]
    fn detect(&mut self, value: Complex) {
        self.power += (value.norm_sqr() - self.power) * self.power_coefficient;
        if self.frozen {
            return;
        }
        let difference = value - self.previous;
        let product = difference.re * self.middle.re + difference.im * self.middle.im;
        let error = if self.power > 1.0e-12 && product.is_finite() {
            (product / self.power).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.error = error;
        let limit = self.settings.max_offset.abs();
        self.offset = (self.offset + self.settings.integral * error).clamp(-limit, limit);
        // A late strobe gives a positive error, and a shorter period
        // brings the next one earlier.
        self.correction = self.offset + self.settings.proportional * error;
    }

    #[inline]
    fn interpolate(&self, fraction: f64) -> Complex {
        let scaled = fraction.clamp(0.0, 1.0) * PHASES as f64;
        let phase = (scaled as usize).min(PHASES - 1);
        let blend = (scaled - phase as f64) as f32;
        let a = self.line.dot(&self.table[phase]);
        let b = self.line.dot(&self.table[phase + 1]);
        a + (b - a).scale(blend)
    }

    /// The loop as it was made, with silence in its delay line.
    pub fn reset(&mut self) {
        self.line.reset();
        self.position = 0.0;
        self.offset = 0.0;
        self.correction = 0.0;
        self.next_on_symbol = false;
        self.previous = Complex::ZERO;
        self.middle = Complex::ZERO;
        self.power = 0.0;
        self.error = 0.0;
        self.frozen = false;
    }
}
