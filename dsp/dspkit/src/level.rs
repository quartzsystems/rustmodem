// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Levels: dBm0 and the linear scale, a level meter, and the comparator
//! that decides whether a carrier is there.
//!
//! Samples are `f32` with full scale, plus or minus 1.0, at the overload
//! point of the call's G.711 law. A sine wave of amplitude 1.0 is therefore
//! at the law's theoretical load capacity: +3.17 dBm0 for the u-law and
//! +3.14 dBm0 for the A-law (G.711 clause 5). Every conversion between a
//! level in dBm0 and a number on the linear scale is one of the four
//! functions here, and takes the law for that reason.

use g711::Law;

use crate::SAMPLE_RATE;
use crate::math;

/// The amplitude of the sine wave that is at `dbm0`.
pub fn dbm0_to_amplitude(dbm0: f32, law: Law) -> f32 {
    math::pow10(f64::from(dbm0 - law.overload_dbm0()) / 20.0) as f32
}

/// The level in dBm0 of a sine wave of this amplitude.
pub fn amplitude_to_dbm0(amplitude: f32, law: Law) -> f32 {
    (20.0 * math::log10(f64::from(amplitude.abs()))) as f32 + law.overload_dbm0()
}

/// The mean square of a signal at `dbm0`: its power on the linear scale,
/// where a sine wave of amplitude 1.0 has a power of one half.
pub fn dbm0_to_power(dbm0: f32, law: Law) -> f32 {
    (0.5 * math::pow10(f64::from(dbm0 - law.overload_dbm0()) / 10.0)) as f32
}

/// The level in dBm0 of a signal with this mean square.
pub fn power_to_dbm0(power: f32, law: Law) -> f32 {
    (10.0 * math::log10(2.0 * f64::from(power.max(0.0)))) as f32 + law.overload_dbm0()
}

/// The coefficient of a one-pole smoother with this time constant: the
/// share of each new sample in the running mean. After the time constant
/// the mean has covered 63 percent of a step.
pub fn smoothing(time_constant_s: f32) -> f32 {
    if time_constant_s <= 0.0 {
        return 1.0;
    }
    let samples = f64::from(time_constant_s) * f64::from(SAMPLE_RATE);
    (1.0 - math::exp(-1.0 / samples)) as f32
}

/// A meter of power: the mean square of a signal over a time constant.
#[derive(Clone, Copy, Debug)]
pub struct LevelMeter {
    alpha: f32,
    power: f32,
}

impl LevelMeter {
    /// A meter with this time constant, reading nothing.
    pub fn new(time_constant_s: f32) -> LevelMeter {
        LevelMeter {
            alpha: smoothing(time_constant_s),
            power: 0.0,
        }
    }

    /// Takes one sample and gives the mean square so far.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        self.power += self.alpha * (sample * sample - self.power);
        self.power
    }

    /// Takes one value that is already a power, such as the squared
    /// magnitude of a complex sample.
    #[inline]
    pub fn process_power(&mut self, power: f32) -> f32 {
        self.power += self.alpha * (power - self.power);
        self.power
    }

    /// The mean square.
    #[inline]
    pub fn power(&self) -> f32 {
        self.power
    }

    /// The reading in dBm0. Not for the tick path of anything but a
    /// diagnostic: it takes a logarithm.
    pub fn dbm0(&self, law: Law) -> f32 {
        power_to_dbm0(self.power, law)
    }

    /// Reads nothing again.
    pub fn reset(&mut self) {
        self.power = 0.0;
    }
}

/// Decides whether a carrier is present, from a power.
///
/// The carrier comes on when the power has been at or above the on
/// threshold for the on time without a break, and goes off when it has
/// been below the off threshold for the off time without a break. The off
/// threshold is the lower, which is the hysteresis V.21 clause 8.3 and its
/// like ask of a line signal detector, and between the two thresholds
/// nothing changes.
#[derive(Clone, Copy, Debug)]
pub struct CarrierDetect {
    on_power: f32,
    off_power: f32,
    on_samples: u32,
    off_samples: u32,
    run: u32,
    present: bool,
}

/// What a [`CarrierDetect`] is set up with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarrierThresholds {
    /// The level in dBm0 at or above which the carrier may come on.
    pub on_dbm0: f32,
    /// The level in dBm0 below which the carrier may go off.
    pub off_dbm0: f32,
    /// How long the level must stay up before the carrier is on, in
    /// seconds.
    pub on_time_s: f32,
    /// How long the level must stay down before the carrier is off, in
    /// seconds.
    pub off_time_s: f32,
}

impl CarrierDetect {
    /// A detector with no carrier.
    pub fn new(thresholds: CarrierThresholds, law: Law) -> CarrierDetect {
        let samples = |seconds: f32| (seconds.max(0.0) * SAMPLE_RATE as f32).round() as u32;
        CarrierDetect {
            on_power: dbm0_to_power(thresholds.on_dbm0, law),
            off_power: dbm0_to_power(thresholds.off_dbm0.min(thresholds.on_dbm0), law),
            on_samples: samples(thresholds.on_time_s),
            off_samples: samples(thresholds.off_time_s),
            run: 0,
            present: false,
        }
    }

    /// Takes the power at one sample and says whether the carrier is
    /// present.
    #[inline]
    pub fn process(&mut self, power: f32) -> bool {
        if self.present {
            if power < self.off_power {
                self.run += 1;
                if self.run >= self.off_samples {
                    self.present = false;
                    self.run = 0;
                }
            } else {
                self.run = 0;
            }
        } else if power >= self.on_power {
            self.run += 1;
            if self.run >= self.on_samples {
                self.present = true;
                self.run = 0;
            }
        } else {
            self.run = 0;
        }
        self.present
    }

    /// Whether the carrier is present.
    #[inline]
    pub fn present(&self) -> bool {
        self.present
    }

    /// No carrier again.
    pub fn reset(&mut self) {
        self.run = 0;
        self.present = false;
    }
}

/// Automatic gain control: brings a signal to a target level with a gain
/// that stays inside a range.
///
/// The gain falls quickly when the output is above the target (the attack)
/// and rises slowly when it is below (the decay), so that a burst of noise
/// does not leave the receiver deaf and a pause does not send the gain to
/// its ceiling at once.
#[derive(Clone, Copy, Debug)]
pub struct Agc {
    target_power: f32,
    min_gain: f32,
    max_gain: f32,
    attack: f32,
    decay: f32,
    meter: LevelMeter,
    gain: f32,
}

/// What an [`Agc`] is set up with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgcSettings {
    /// The level in dBm0 to bring the signal to.
    pub target_dbm0: f32,
    /// The least gain, in dB.
    pub min_gain_db: f32,
    /// The most gain, in dB.
    pub max_gain_db: f32,
    /// The time constant of a fall in gain, in seconds.
    pub attack_s: f32,
    /// The time constant of a rise in gain, in seconds.
    pub decay_s: f32,
    /// The time constant of the level measurement, in seconds.
    pub measure_s: f32,
}

impl Agc {
    /// A control at unity gain, or the nearest its range allows.
    pub fn new(settings: AgcSettings, law: Law) -> Agc {
        let gain = |db: f32| math::pow10(f64::from(db) / 20.0) as f32;
        let min_gain = gain(settings.min_gain_db);
        let max_gain = gain(settings.max_gain_db).max(min_gain);
        Agc {
            target_power: dbm0_to_power(settings.target_dbm0, law),
            min_gain,
            max_gain,
            attack: smoothing(settings.attack_s),
            decay: smoothing(settings.decay_s),
            meter: LevelMeter::new(settings.measure_s),
            gain: 1.0f32.clamp(min_gain, max_gain),
        }
    }

    /// Takes one sample and gives it back with the gain applied.
    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        let power = self.meter.process(sample);
        // The gain that would put the measured input at the target, held
        // inside the range. A silent input wants the most gain there is.
        let wanted = if power * self.max_gain * self.max_gain <= self.target_power {
            self.max_gain
        } else {
            (self.target_power / power)
                .sqrt()
                .clamp(self.min_gain, self.max_gain)
        };
        let rate = if wanted < self.gain {
            self.attack
        } else {
            self.decay
        };
        self.gain += rate * (wanted - self.gain);
        sample * self.gain
    }

    /// The gain now, as a factor.
    #[inline]
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// The least and the most gain, as factors.
    pub fn range(&self) -> (f32, f32) {
        (self.min_gain, self.max_gain)
    }
}

/// The mean of the last `N` values, exactly.
///
/// It is kept as a running sum, and a running sum in floating point
/// drifts, so every `N` values it is replaced by a sum made afresh over
/// exactly those values: the error never grows.
#[derive(Clone, Copy, Debug)]
pub struct MovingAverage<const N: usize> {
    values: [f32; N],
    at: usize,
    sum: f32,
    fresh: f32,
}

impl<const N: usize> MovingAverage<N> {
    /// An average over `N` zeros.
    pub fn new() -> MovingAverage<N> {
        MovingAverage {
            values: [0.0; N],
            at: 0,
            sum: 0.0,
            fresh: 0.0,
        }
    }

    /// Takes one value and gives the mean of the last `N`.
    #[inline]
    pub fn process(&mut self, value: f32) -> f32 {
        self.sum += value - self.values[self.at];
        self.fresh += value;
        self.values[self.at] = value;
        self.at += 1;
        if self.at == N {
            self.at = 0;
            self.sum = self.fresh;
            self.fresh = 0.0;
        }
        self.sum / N as f32
    }

    /// The mean of the last `N` values.
    #[inline]
    pub fn mean(&self) -> f32 {
        self.sum / N as f32
    }

    /// Zeros again.
    pub fn reset(&mut self) {
        *self = MovingAverage::new();
    }
}

impl<const N: usize> Default for MovingAverage<N> {
    fn default() -> Self {
        MovingAverage::new()
    }
}
