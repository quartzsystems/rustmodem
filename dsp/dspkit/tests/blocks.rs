// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Each block against a closed-form result.

use dspkit::biquad::{Biquad, Cascade, Coefficients, DcBlocker};
use dspkit::fir::{self, Fir, Window};
use dspkit::goertzel::{Goertzel, SlidingGoertzel};
use dspkit::hilbert::{FrequencyShifter, Hilbert};
use dspkit::interp::{FractionalDelay, Resampler};
use dspkit::level::{
    Agc, AgcSettings, CarrierDetect, CarrierThresholds, LevelMeter, amplitude_to_dbm0,
    dbm0_to_amplitude, dbm0_to_power, power_to_dbm0, smoothing,
};
use dspkit::osc::{Oscillator, phase_of_degrees, phase_step};
use g711::Law;

const FS: f64 = 8000.0;
const TAU: f64 = std::f64::consts::TAU;

/// A sine wave by the library's sine, in double precision: the reference
/// the blocks are held to.
fn sine(frequency: f64, amplitude: f64, phase: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| (amplitude * (TAU * frequency * n as f64 / FS + phase).sin()) as f32)
        .collect()
}

/// The sine wave of a known frequency that fits the samples best.
struct Fit {
    amplitude: f64,
    /// Radians, of a sine: `amplitude * sin(w n + phase)`.
    phase: f64,
    /// The root mean square of what the fit leaves over.
    residual: f64,
}

fn fit(samples: &[f32], frequency: f64) -> Fit {
    // Least squares on a sin(wn) + b cos(wn).
    let (mut ss, mut cc, mut sc, mut xs, mut xc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (n, &x) in samples.iter().enumerate() {
        let (s, c) = (TAU * frequency * n as f64 / FS).sin_cos();
        ss += s * s;
        cc += c * c;
        sc += s * c;
        xs += f64::from(x) * s;
        xc += f64::from(x) * c;
    }
    let det = ss * cc - sc * sc;
    let a = (xs * cc - xc * sc) / det;
    let b = (xc * ss - xs * sc) / det;
    let mut residual = 0.0;
    for (n, &x) in samples.iter().enumerate() {
        let (s, c) = (TAU * frequency * n as f64 / FS).sin_cos();
        residual += (f64::from(x) - a * s - b * c).powi(2);
    }
    Fit {
        amplitude: a.hypot(b),
        phase: b.atan2(a),
        residual: (residual / samples.len() as f64).sqrt(),
    }
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// The difference of two angles, between minus pi and pi.
fn angle_between(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(TAU);
    if d > TAU / 2.0 { d - TAU } else { d }
}

// Oscillator.

#[test]
fn the_oscillator_is_a_sine_wave_of_the_frequency_asked_for() {
    for frequency in [
        15.0, 300.0, 980.0, 1180.0, 1650.0, 1850.0, 2100.0, 2225.0, 3400.0,
    ] {
        let mut oscillator = Oscillator::new(frequency as f32);
        let samples: Vec<f32> = (0..8000).map(|_| oscillator.next_sine()).collect();
        // Against the true sine wave, sample by sample, over a second.
        // The frequency is a whole number of steps of 8000 / 2^32 Hz, so
        // it may be half a step out, which in a second is 6 millionths of
        // a radian; the table adds under a millionth.
        let worst = samples
            .iter()
            .enumerate()
            .map(|(n, &x)| (f64::from(x) - (TAU * frequency * n as f64 / FS).sin()).abs())
            .fold(0.0, f64::max);
        assert!(worst < 7.0e-6, "{frequency} Hz: off by {worst}");
        let fit = fit(&samples, frequency);
        assert!((fit.amplitude - 1.0).abs() < 1.0e-6, "{frequency} Hz");
        assert!(fit.phase.abs() < 4.0e-6, "{frequency} Hz");
        assert!(
            db(fit.residual) < -115.0,
            "{frequency} Hz: {}",
            db(fit.residual)
        );
    }
}

#[test]
fn the_frequency_is_exact_over_a_long_time() {
    // After ten minutes the phase is still where the frequency puts it:
    // the accumulator does not drift.
    let mut oscillator = Oscillator::new(2100.0);
    for _ in 0..4_800_000 {
        oscillator.next_sine();
    }
    // 2100 Hz is 21 cycles in 80 samples, so the phase is back at zero.
    let turn = f64::from(oscillator.phase()) / 4_294_967_296.0;
    let from_zero = turn.min(1.0 - turn);
    assert!(from_zero < 1.0e-3, "{turn}");
    assert_eq!(phase_step(2000.0), 1 << 30);
    assert_eq!(phase_step(-2000.0), 3 << 30);
    assert_eq!(phase_of_degrees(180.0), 1 << 31);
    assert_eq!(phase_of_degrees(-90.0), 3 << 30);
}

#[test]
fn the_two_outputs_are_a_quarter_cycle_apart() {
    let mut oscillator = Oscillator::new(1234.0);
    for n in 0..4000 {
        let sample = oscillator.next_quadrature();
        let angle = TAU * 1234.0 * n as f64 / FS;
        assert!((f64::from(sample.i) - angle.cos()).abs() < 5.0e-6);
        assert!((f64::from(sample.q) - angle.sin()).abs() < 5.0e-6);
        let length = sample.i * sample.i + sample.q * sample.q;
        assert!((length - 1.0).abs() < 4.0e-6);
    }
}

#[test]
fn a_phase_step_happens_at_once_and_a_change_of_frequency_keeps_the_phase() {
    let mut oscillator = Oscillator::new(2100.0);
    let before: Vec<f32> = (0..400).map(|_| oscillator.next_sine()).collect();
    oscillator.reverse_phase();
    let after: Vec<f32> = (0..400).map(|_| oscillator.next_sine()).collect();
    // 400 samples is a whole number of cycles of 2100 Hz, so the second
    // run would repeat the first; reversed, it is its negative.
    for (a, b) in before.iter().zip(&after) {
        assert!((a + b).abs() < 4.0e-6);
    }
    oscillator.shift_phase(phase_of_degrees(90.0));
    let turned = oscillator.next_sine();
    assert!((f64::from(turned) + 1.0).abs() < 1.0e-6, "{turned}");

    // Frequency-shift keying with a continuous phase: no jump in the
    // output where the frequency changes.
    let mut oscillator = Oscillator::new(980.0);
    let mut last = oscillator.next_sine();
    let mut worst = 0f32;
    for n in 0..8000 {
        if n % 27 == 0 {
            oscillator.set_frequency(if (n / 27) % 2 == 0 { 1180.0 } else { 980.0 });
        }
        let sample = oscillator.next_sine();
        worst = worst.max((sample - last).abs());
        last = sample;
    }
    // The largest step a sine wave of 1180 Hz can take in one sample.
    let limit = 2.0 * (TAU * 1180.0 / FS / 2.0).sin();
    assert!(
        f64::from(worst) <= limit + 1.0e-5,
        "{worst} against {limit}"
    );
}

// Levels.

#[test]
fn full_scale_is_the_overload_point_of_the_law() {
    // A sine wave at the overload point is at Tmax (G.711 clause 5).
    assert!((dbm0_to_amplitude(3.17, Law::Ulaw) - 1.0).abs() < 1.0e-6);
    assert!((dbm0_to_amplitude(Law::Alaw.overload_dbm0(), Law::Alaw) - 1.0).abs() < 1.0e-6);
    assert!((amplitude_to_dbm0(1.0, Law::Ulaw) - 3.17).abs() < 1.0e-5);
    // The default transmit level.
    let amplitude = dbm0_to_amplitude(-12.0, Law::Ulaw);
    assert!((f64::from(amplitude) - 10f64.powf((-12.0 - 3.17) / 20.0)).abs() < 1.0e-7);
    for level in [-48.0f32, -43.0, -30.0, -12.0, -9.0, 0.0, 3.0] {
        for law in [Law::Ulaw, Law::Alaw] {
            let amplitude = dbm0_to_amplitude(level, law);
            assert!((amplitude_to_dbm0(amplitude, law) - level).abs() < 1.0e-4);
            let power = dbm0_to_power(level, law);
            assert!((power_to_dbm0(power, law) - level).abs() < 1.0e-4);
            // A sine wave's mean square is half its amplitude squared.
            assert!((power / (amplitude * amplitude / 2.0) - 1.0).abs() < 1.0e-5);
        }
    }
    // Nothing is a very low level, not a NaN.
    assert!(power_to_dbm0(0.0, Law::Ulaw) < -200.0);
    assert!(amplitude_to_dbm0(0.0, Law::Ulaw) < -200.0);
}

#[test]
fn the_level_meter_reads_a_known_sine_wave() {
    for law in [Law::Ulaw, Law::Alaw] {
        for level in [-43.0f32, -20.0, -12.0, 0.0] {
            let amplitude = f64::from(dbm0_to_amplitude(level, law));
            let mut meter = LevelMeter::new(0.05);
            let mut least = f32::MAX;
            let mut most = f32::MIN;
            for (n, &x) in sine(1000.0, amplitude, 0.3, 8000).iter().enumerate() {
                meter.process(x);
                if n > 4000 {
                    least = least.min(meter.dbm0(law));
                    most = most.max(meter.dbm0(law));
                }
            }
            assert!(
                least > level - 0.05 && most < level + 0.05,
                "{level}: {least} {most}"
            );
        }
    }
}

#[test]
fn the_level_meter_follows_a_step_with_its_time_constant() {
    let mut meter = LevelMeter::new(0.010);
    let mut reading = 0.0;
    for _ in 0..80 {
        reading = meter.process(1.0);
    }
    // After one time constant, 1 - 1/e of the way.
    assert!(
        (f64::from(reading) - (1.0 - (-1.0f64).exp())).abs() < 1.0e-3,
        "{reading}"
    );
    assert!((f64::from(smoothing(0.010)) - (1.0 - (-1.0f64 / 80.0).exp())).abs() < 1.0e-7);
    assert_eq!(smoothing(0.0), 1.0);
    meter.reset();
    assert_eq!(meter.power(), 0.0);
    assert!((meter.process_power(0.5) - 0.5 * smoothing(0.010)).abs() < 1.0e-7);
}

#[test]
fn the_carrier_detector_has_two_thresholds_and_two_times() {
    // The thresholds of V.21 clause 8.3.
    let thresholds = CarrierThresholds {
        on_dbm0: -43.0,
        off_dbm0: -48.0,
        on_time_s: 0.010,
        off_time_s: 0.020,
    };
    let law = Law::Ulaw;
    let mut detect = CarrierDetect::new(thresholds, law);
    let at = |level: f32| dbm0_to_power(level, law);

    // Below the on threshold, never.
    for _ in 0..8000 {
        assert!(!detect.process(at(-43.5)));
    }
    // At it, after the on time and not before.
    for n in 0..80 {
        assert_eq!(detect.process(at(-42.9)), n == 79, "{n}");
    }
    // Between the thresholds it stays as it is.
    for _ in 0..8000 {
        assert!(detect.process(at(-46.0)));
    }
    // A dip shorter than the off time is ridden out.
    for _ in 0..159 {
        assert!(detect.process(at(-60.0)));
    }
    assert!(detect.process(at(-46.0)));
    // Below the off threshold for the off time, it goes.
    for n in 0..160 {
        assert_eq!(detect.process(at(-48.5)), n != 159, "{n}");
    }
    // And between the thresholds it stays off.
    for _ in 0..8000 {
        assert!(!detect.process(at(-46.0)));
    }
    // A burst shorter than the on time does not bring it on.
    for _ in 0..79 {
        assert!(!detect.process(at(-20.0)));
    }
    assert!(!detect.process(at(-60.0)));
    for _ in 0..80 {
        detect.process(at(-20.0));
    }
    assert!(detect.present());
    detect.reset();
    assert!(!detect.present());
}

#[test]
fn the_gain_control_reaches_its_target_and_keeps_to_its_range() {
    let law = Law::Ulaw;
    let settings = AgcSettings {
        target_dbm0: -12.0,
        min_gain_db: -6.0,
        max_gain_db: 30.0,
        attack_s: 0.005,
        decay_s: 0.100,
        measure_s: 0.020,
    };
    let level_out = |level_in: f32| {
        let mut agc = Agc::new(settings, law);
        let mut meter = LevelMeter::new(0.05);
        let amplitude = f64::from(dbm0_to_amplitude(level_in, law));
        for &x in &sine(1100.0, amplitude, 0.0, 40_000) {
            meter.process(agc.process(x));
        }
        (meter.dbm0(law), 20.0 * agc.gain().log10())
    };
    // Inside the range, the output is at the target.
    for level in [-40.0f32, -30.0, -20.0, -12.0, -8.0] {
        let (out, gain) = level_out(level);
        assert!((out + 12.0).abs() < 0.1, "{level} dBm0 in, {out} dBm0 out");
        assert!((gain - (-12.0 - level)).abs() < 0.1, "{level}: {gain} dB");
    }
    // Beyond it, the gain stops at its limit.
    let (out, gain) = level_out(-50.0);
    assert!(
        (gain - 30.0).abs() < 0.01 && (out + 20.0).abs() < 0.1,
        "{out} {gain}"
    );
    let (out, gain) = level_out(0.0);
    assert!(
        (gain + 6.0).abs() < 0.01 && (out + 6.0).abs() < 0.1,
        "{out} {gain}"
    );
    let agc = Agc::new(settings, law);
    let (least, most) = agc.range();
    assert!((20.0 * least.log10() + 6.0).abs() < 1.0e-4);
    assert!((20.0 * most.log10() - 30.0).abs() < 1.0e-4);

    // The gain falls faster than it rises.
    let mut agc = Agc::new(settings, law);
    let quiet = sine(
        1100.0,
        f64::from(dbm0_to_amplitude(-30.0, law)),
        0.0,
        40_000,
    );
    let loud = sine(
        1100.0,
        f64::from(dbm0_to_amplitude(-12.0, law)),
        0.0,
        40_000,
    );
    for &x in &quiet {
        agc.process(x);
    }
    let high = agc.gain();
    let mut fall = 0;
    for &x in &loud {
        agc.process(x);
        fall += 1;
        if agc.gain() < high / 4.0 {
            break;
        }
    }
    for &x in &loud {
        agc.process(x);
    }
    let low = agc.gain();
    let mut rise = 0;
    for &x in &quiet {
        agc.process(x);
        rise += 1;
        if agc.gain() > low * 4.0 {
            break;
        }
    }
    assert!(fall * 4 < rise, "fell in {fall} samples and rose in {rise}");
    // Silence takes the gain to its ceiling and no further, and makes
    // nothing of nothing.
    for _ in 0..80_000 {
        assert_eq!(agc.process(0.0), 0.0);
    }
    assert!((agc.gain() - most).abs() < 1.0e-3);
}

// Filters.

/// The gain a filter shows a sine wave, measured by running one through
/// it.
fn measured_gain(mut filter: impl FnMut(f32) -> f32, frequency: f64) -> f64 {
    let out: Vec<f32> = sine(frequency, 0.5, 0.0, 4000)
        .into_iter()
        .map(&mut filter)
        .collect();
    fit(&out[2000..], frequency).amplitude / 0.5
}

#[test]
fn a_fir_filter_is_its_taps() {
    let mut filter = Fir::new([1.0, 2.0, 3.0, 4.0]);
    let out: Vec<f32> = [1.0, 0.0, 0.0, 0.0, 0.0, 5.0]
        .into_iter()
        .map(|x| filter.process(x))
        .collect();
    // The impulse response is the taps, newest sample first.
    assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 0.0, 5.0]);
    assert_eq!(filter.tap(0), 5.0);
    assert_eq!(filter.tap(5 % 4), 0.0);
    assert_eq!(filter.dot(&[0.0, 0.0, 0.0, 0.0]), 0.0);
    filter.reset();
    assert_eq!(filter.process(0.0), 0.0);
    assert_eq!(filter.taps(), &[1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn the_low_pass_design_has_the_response_asked_for() {
    let taps: [f32; 63] = fir::lowpass(1000.0, Window::Hamming);
    // Symmetric, so the phase is linear.
    for n in 0..63 {
        assert_eq!(taps[n], taps[62 - n]);
    }
    assert!((fir::magnitude_at(&taps, 0.0) - 1.0).abs() < 1.0e-6);
    // Flat in the pass band, a half at the cutoff, and 50 dB down past
    // the transition, which for this window is 3.3 / N of the sample
    // rate wide.
    for frequency in [100.0, 300.0, 500.0, 700.0] {
        assert!(
            (fir::magnitude_at(&taps, frequency) - 1.0).abs() < 0.005,
            "{frequency}"
        );
    }
    assert!((fir::magnitude_at(&taps, 1000.0) - 0.5).abs() < 0.01);
    for frequency in [1300.0, 1500.0, 2000.0, 3000.0, 3900.0] {
        assert!(
            db(f64::from(fir::magnitude_at(&taps, frequency))) < -50.0,
            "{frequency}"
        );
    }
    // The filter does to a sine wave what its response says.
    for frequency in [200.0, 900.0, 1000.0, 1100.0] {
        let mut filter = Fir::new(taps);
        let measured = measured_gain(|x| filter.process(x), frequency);
        let stated = f64::from(fir::magnitude_at(&taps, frequency as f32));
        assert!(
            (measured - stated).abs() < 1.0e-4,
            "{frequency}: {measured} {stated}"
        );
    }
}

#[test]
fn the_band_pass_design_has_the_response_asked_for() {
    // A band for V.21 channel 2, as the modem will want.
    let taps: [f32; 95] = fir::bandpass(1450.0, 2050.0, Window::Blackman);
    assert!((fir::magnitude_at(&taps, 1750.0) - 1.0).abs() < 1.0e-5);
    for frequency in [1650.0, 1850.0] {
        assert!(
            (fir::magnitude_at(&taps, frequency) - 1.0).abs() < 0.01,
            "{frequency}"
        );
    }
    for frequency in [0.0, 300.0, 980.0, 1180.0, 2500.0, 3400.0] {
        assert!(
            db(f64::from(fir::magnitude_at(&taps, frequency))) < -55.0,
            "{frequency}"
        );
    }
    let kaiser: [f32; 95] = fir::bandpass(1450.0, 2050.0, Window::Kaiser(8.6));
    for frequency in [300.0, 980.0, 2600.0, 3400.0] {
        assert!(
            db(f64::from(fir::magnitude_at(&kaiser, frequency))) < -80.0,
            "{frequency}"
        );
    }
}

/// The gain of a second-order Butterworth low-pass made by the bilinear
/// transform: the analogue response with the frequency axis warped.
fn butterworth_lowpass(frequency: f64, corner: f64) -> f64 {
    let ratio = (TAU * frequency / FS / 2.0).tan() / (TAU * corner / FS / 2.0).tan();
    1.0 / (1.0 + ratio.powi(4)).sqrt()
}

#[test]
fn the_biquad_designs_have_their_closed_form_responses() {
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let lowpass = Coefficients::lowpass(1000.0, q);
    let highpass = Coefficients::highpass(1000.0, q);
    for frequency in [50.0, 500.0, 1000.0, 1500.0, 2500.0, 3500.0] {
        let stated = butterworth_lowpass(frequency, 1000.0);
        assert!(
            (f64::from(lowpass.magnitude_at(frequency as f32)) - stated).abs() < 1.0e-5,
            "{frequency}"
        );
        let mut section = Biquad::new(lowpass);
        let measured = measured_gain(|x| section.process(x), frequency);
        assert!(
            (measured - stated).abs() < 1.0e-4,
            "{frequency}: {measured} {stated}"
        );
        // The high-pass is the low-pass with the warped axis turned over.
        let ratio = (TAU * 1000.0 / FS / 2.0).tan() / (TAU * frequency / FS / 2.0).tan();
        let stated = 1.0 / (1.0 + ratio.powi(4)).sqrt();
        let mut section = Biquad::new(highpass);
        let measured = measured_gain(|x| section.process(x), frequency);
        assert!(
            (measured - stated).abs() < 1.0e-4,
            "{frequency}: {measured} {stated}"
        );
    }
    // 3 dB down at the corner.
    assert!((db(f64::from(lowpass.magnitude_at(1000.0))) + 3.0103).abs() < 1.0e-3);

    // The band-pass: one at the centre, and 3 dB down at two edges that
    // the bilinear transform puts where
    // tan(w2 / 2) - tan(w1 / 2) = tan(w0 / 2) / Q. The edges are found by
    // search.
    let bandpass = Coefficients::bandpass(1750.0, 5.0);
    assert!((bandpass.magnitude_at(1750.0) - 1.0).abs() < 1.0e-6);
    let half_power = std::f64::consts::FRAC_1_SQRT_2;
    let edge = |from: f64, to: f64| {
        let above = |f: f64| f64::from(bandpass.magnitude_at(f as f32)) > half_power;
        let (mut a, mut b) = (from, to);
        for _ in 0..60 {
            let mid = (a + b) / 2.0;
            if above(mid) == above(a) {
                a = mid;
            } else {
                b = mid;
            }
        }
        (a + b) / 2.0
    };
    let (lower, upper) = (edge(500.0, 1750.0), edge(1750.0, 3000.0));
    let warped = |f: f64| (TAU * f / FS / 2.0).tan();
    let stated = warped(1750.0) / 5.0;
    assert!(
        ((warped(upper) - warped(lower)) / stated - 1.0).abs() < 1.0e-3,
        "{lower} to {upper}"
    );
    // Which is 8000 sin(w0) / (2 pi Q) hertz wide, near enough: 250 Hz,
    // where the analogue filter of the same Q is 350 Hz wide.
    let width = FS * (TAU * 1750.0 / FS).sin() / (TAU * 5.0);
    assert!(
        (upper - lower - width).abs() < 3.0,
        "{lower} to {upper}: {width}"
    );
    let mut section = Biquad::new(bandpass);
    assert!((measured_gain(|x| section.process(x), 1750.0) - 1.0).abs() < 1.0e-4);

    // The notch: nothing at its centre, everything far from it.
    let notch = Coefficients::notch(2100.0, 10.0);
    assert!(notch.magnitude_at(2100.0) < 1.0e-5);
    assert!((notch.magnitude_at(300.0) - 1.0).abs() < 0.01);
    assert!((notch.magnitude_at(3800.0) - 1.0).abs() < 0.01);
    let mut section = Biquad::new(notch);
    assert!(db(measured_gain(|x| section.process(x), 2100.0)) < -80.0);

    assert_eq!(Biquad::new(Coefficients::IDENTITY).process(0.25), 0.25);
}

#[test]
fn a_cascade_is_the_product_of_its_sections() {
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let sections = [
        Coefficients::highpass(300.0, q),
        Coefficients::lowpass(3400.0, q),
    ];
    let mut cascade = Cascade::new(sections);
    for frequency in [100.0, 300.0, 1000.0, 3400.0, 3900.0] {
        let stated = f64::from(sections[0].magnitude_at(frequency as f32))
            * f64::from(sections[1].magnitude_at(frequency as f32));
        assert!((f64::from(cascade.magnitude_at(frequency as f32)) - stated).abs() < 1.0e-6);
        cascade.reset();
        let measured = measured_gain(|x| cascade.process(x), frequency);
        assert!(
            (measured - stated).abs() < 1.0e-4,
            "{frequency}: {measured} {stated}"
        );
    }
}

#[test]
fn the_dc_blocker_removes_an_offset_and_leaves_the_signal() {
    let mut blocker = DcBlocker::new(20.0);
    let mut out = 1.0;
    for _ in 0..8000 {
        out = blocker.process(0.5);
    }
    assert!(out.abs() < 1.0e-6, "{out}");
    blocker.reset();
    // Its response is |1 - z^-1| / |1 - r z^-1|.
    let r = 1.0 - TAU * 20.0 / FS;
    for frequency in [300.0, 1000.0, 3000.0] {
        let w = TAU * frequency / FS;
        let stated = (2.0 - 2.0 * w.cos()).sqrt() / (1.0 + r * r - 2.0 * r * w.cos()).sqrt();
        let measured = measured_gain(|x| blocker.process(x), frequency);
        assert!(
            (measured - stated).abs() < 1.0e-4,
            "{frequency}: {measured} {stated}"
        );
        assert!((measured - 1.0).abs() < 0.01, "{frequency}");
    }
    // About 3 dB down at the corner.
    let measured = measured_gain(|x| blocker.process(x), 20.0);
    assert!((db(measured) + 3.0).abs() < 0.2, "{}", db(measured));
}

// Detectors.

/// The power a rectangular window of `len` samples shows of a sine wave
/// of amplitude `a` that is `offset` hertz from the detector's frequency:
/// the square of the Dirichlet kernel.
fn window_response(a: f64, offset: f64, len: f64) -> f64 {
    if offset == 0.0 {
        return a * a / 2.0;
    }
    let x = TAU / 2.0 * offset / FS;
    let kernel = (x * len).sin() / (len * x.sin());
    a * a / 2.0 * kernel * kernel
}

#[test]
fn goertzel_has_the_response_of_its_window() {
    let len = 400;
    // On the frequency.
    for (frequency, amplitude) in [(2100.0, 0.25), (1300.0, 1.0), (980.0, 0.01)] {
        let mut detector = Goertzel::new(frequency as f32, len);
        let mut result = None;
        for &x in &sine(frequency, amplitude, 1.0, len as usize) {
            result = detector.process(x);
        }
        let power = f64::from(result.expect("a result at the end of the block"));
        let stated = amplitude * amplitude / 2.0;
        assert!(
            (power / stated - 1.0).abs() < 1.0e-3,
            "{frequency}: {power} {stated}"
        );
    }
    // Off it: nothing at multiples of 8000 / len = 20 Hz, and the side
    // lobes of the kernel between them.
    for offset in [20.0, 40.0, 100.0, -20.0, -200.0] {
        let mut detector = Goertzel::new(2100.0, len);
        let mut result = None;
        for &x in &sine(2100.0 + offset, 0.5, 0.4, len as usize) {
            result = detector.process(x);
        }
        let power = f64::from(result.unwrap());
        assert!(power < 0.125 * 1.0e-6, "{offset} Hz off: {power}");
    }
    for offset in [5.0, 10.0, 30.0, 50.0, -10.0] {
        let mut detector = Goertzel::new(2100.0, len);
        let mut result = None;
        for &x in &sine(2100.0 + offset, 0.5, 0.0, len as usize) {
            result = detector.process(x);
        }
        let power = f64::from(result.unwrap());
        let stated = window_response(0.5, offset, f64::from(len));
        // The image at the negative frequency adds a little, more where
        // the response itself is small.
        assert!(
            (power - stated).abs() < 0.03 * stated + 2.0e-5,
            "{offset}: {power} {stated}"
        );
    }
    // A block starts afresh after its result.
    let mut detector = Goertzel::new(2100.0, len);
    let mut results = Vec::new();
    for &x in &sine(2100.0, 0.5, 0.0, 3 * len as usize) {
        results.extend(detector.process(x));
    }
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|&p| (p / 0.125 - 1.0).abs() < 1.0e-3));
    assert_eq!(detector.power(), 0.0);
}

#[test]
fn the_sliding_detector_answers_at_every_sample() {
    const N: usize = 160;
    let mut detector = SlidingGoertzel::<N>::new(2100.0);
    let samples = sine(2100.0, 0.5, 0.7, 4000);
    for (n, &x) in samples.iter().enumerate() {
        let power = f64::from(detector.process(x));
        if n >= N {
            // 2100 Hz is a whole number of cycles in 160 samples, so the
            // image at the negative frequency averages to nothing.
            assert!((power / 0.125 - 1.0).abs() < 1.0e-3, "{n}: {power}");
        }
    }
    // Silence empties it in exactly its length.
    let mut last = 1.0;
    for _ in 0..N {
        last = detector.process(0.0);
    }
    assert!(last < 1.0e-9, "{last}");

    // Off the frequency, the response of its window.
    for offset in [25.0, 50.0, 75.0, 100.0] {
        let mut detector = SlidingGoertzel::<N>::new(2100.0);
        let mut most = 0f64;
        for (n, &x) in sine(2100.0 + offset, 0.5, 0.0, 4000).iter().enumerate() {
            let power = f64::from(detector.process(x));
            if n >= N {
                most = most.max(power);
            }
        }
        let stated = window_response(0.5, offset, N as f64);
        assert!(most < stated * 1.1 + 1.0e-4, "{offset}: {most} {stated}");
    }

    // It does not drift: after ten minutes of a tone and then silence it
    // reads nothing.
    let mut detector = SlidingGoertzel::<N>::new(2100.0);
    let mut oscillator = Oscillator::new(2100.0);
    for _ in 0..4_800_000 {
        detector.process(0.9 * oscillator.next_sine());
    }
    let mut last = 1.0;
    for _ in 0..2 * N {
        last = detector.process(0.0);
    }
    assert!(last < 1.0e-9, "{last}");
    detector.reset();
    assert_eq!(detector.process(0.0), 0.0);
}

#[test]
fn the_sliding_detector_shows_a_phase_reversal() {
    const N: usize = 80;
    let mut detector = SlidingGoertzel::<N>::new(2100.0);
    let mut oscillator = Oscillator::new(2100.0);
    let mut before = None;
    for n in 0..2000 {
        if n == 1000 {
            oscillator.reverse_phase();
        }
        let phasor = detector.process_phasor(0.5 * oscillator.next_sine());
        if n == 999 {
            before = Some(phasor);
        }
        if n == 1000 + N {
            let before = before.unwrap();
            // The same length, the opposite direction.
            assert!((phasor.i + before.i).abs() < 1.0e-4);
            assert!((phasor.q + before.q).abs() < 1.0e-4);
            // Half the amplitude, as the sum of a sine against a complex
            // oscillator is.
            let length = f64::from(phasor.i).hypot(f64::from(phasor.q));
            assert!((length - 0.25).abs() < 1.0e-4, "{length}");
        }
    }
}

// Interpolation.

#[test]
fn the_fractional_delay_delays_by_what_it_says() {
    for fraction in [-0.5f32, -0.25, 0.0, 0.1, 0.37, 0.5] {
        for frequency in [100.0, 300.0, 1000.0, 2100.0, 3000.0, 3400.0] {
            let mut delay = FractionalDelay::<48>::new(fraction);
            let stated = delay.delay();
            assert!((stated - (23.5 + f64::from(fraction))).abs() < 1.0e-9);
            let out: Vec<f32> = sine(frequency, 0.5, 0.0, 2000)
                .into_iter()
                .map(|x| delay.process(x))
                .collect();
            // The output is the sine wave, `stated` samples late.
            let worst = out
                .iter()
                .enumerate()
                .skip(200)
                .map(|(n, &y)| {
                    let ideal = 0.5 * (TAU * frequency * (n as f64 - stated) / FS).sin();
                    (f64::from(y) - ideal).abs()
                })
                .fold(0.0, f64::max);
            assert!(
                db(worst / 0.5) < -70.0,
                "{fraction} {frequency}: {}",
                db(worst / 0.5)
            );
        }
    }
}

/// The worst error of a resampler's output against the ideal resampled
/// sine wave, relative to the sine wave's amplitude, in dB.
fn resampler_error(ratio: f64, frequency: f64) -> f64 {
    let mut resampler = Resampler::new(ratio);
    let delay = resampler.delay();
    let mut out = Vec::new();
    for &x in &sine(frequency, 0.5, 0.0, 20_000) {
        out.extend_from_slice(resampler.process(x).as_slice());
    }
    // Output sample m is the input at time m / ratio, late by the delay.
    // The first output sample is the point one sample before the first
    // input sample.
    let worst = out
        .iter()
        .enumerate()
        .skip(500)
        .map(|(m, &y)| {
            let time = m as f64 / ratio - 1.0 - (delay - 1.0);
            let ideal = 0.5 * (TAU * frequency * time / FS).sin();
            (f64::from(y) - ideal).abs()
        })
        .fold(0.0, f64::max);
    db(worst / 0.5)
}

#[test]
fn the_resampler_is_accurate_to_70_db_across_the_voice_band() {
    let mut worst = f64::MIN;
    for ratio in [
        1.0,
        1.0 + 200.0e-6,
        1.0 - 200.0e-6,
        1.0001,
        0.9999,
        1.01,
        0.99,
    ] {
        for frequency in [
            100.0, 300.0, 980.0, 1180.0, 1750.0, 2100.0, 2225.0, 3000.0, 3400.0,
        ] {
            let error = resampler_error(ratio, frequency);
            assert!(error < -70.0, "ratio {ratio}, {frequency} Hz: {error} dB");
            worst = worst.max(error);
        }
    }
    println!("resampler: worst error {worst:.1} dB");
}

#[test]
fn the_resampler_makes_the_number_of_samples_its_ratio_says() {
    for (ppm, expected) in [(100.0, 1_000_100i64), (-100.0, 999_900), (0.0, 1_000_000)] {
        let mut resampler = Resampler::with_ppm(ppm);
        let mut count = 0i64;
        for _ in 0..1_000_000 {
            count += resampler.process(0.0).as_slice().len() as i64;
        }
        assert!((count - expected).abs() <= 1, "{ppm} ppm: {count}");
    }
    // A ratio it cannot do is held to what it can.
    let mut resampler = Resampler::new(3.0);
    let mut count = 0;
    for _ in 0..100_000 {
        count += resampler.process(0.0).as_slice().len();
    }
    assert!((count as i64 - 105_000).abs() <= 1, "{count}");
    let mut resampler = Resampler::new(f64::NAN);
    assert_eq!(resampler.process(0.0).as_slice().len(), 1);
}

#[test]
fn the_resampler_moves_a_tone_by_its_ratio() {
    // Played back at the nominal rate, a tone resampled by 1 + e is
    // lower by the factor 1 + e.
    let ratio = 1.0 + 200.0e-6;
    let mut resampler = Resampler::new(ratio);
    let mut out = Vec::new();
    for &x in &sine(2000.0, 0.5, 0.0, 40_000) {
        out.extend_from_slice(resampler.process(x).as_slice());
    }
    let expected = 2000.0 / ratio;
    let right = fit(&out[1000..], expected);
    let wrong = fit(&out[1000..], 2000.0);
    assert!(
        db(right.residual / 0.5) < -70.0,
        "{}",
        db(right.residual / 0.5)
    );
    assert!(right.residual * 100.0 < wrong.residual);
}

// Hilbert transformer and frequency shifter.

#[test]
fn the_hilbert_transformer_turns_every_frequency_a_quarter_cycle() {
    for frequency in [300.0, 980.0, 1750.0, 2100.0, 3000.0, 3400.0] {
        let mut hilbert = Hilbert::<127>::new();
        assert_eq!(hilbert.delay(), 63);
        let mut i = Vec::new();
        let mut q = Vec::new();
        for &x in &sine(frequency, 0.5, 0.0, 4000) {
            let out = hilbert.process(x);
            i.push(out.i);
            q.push(out.q);
        }
        let fit_i = fit(&i[500..], frequency);
        let fit_q = fit(&q[500..], frequency);
        // The same amplitude, and the transform a quarter cycle behind.
        assert!((fit_i.amplitude - 0.5).abs() < 1.0e-6, "{frequency}");
        assert!(
            (fit_q.amplitude / 0.5 - 1.0).abs() < 2.0e-3,
            "{frequency}: {}",
            fit_q.amplitude
        );
        let lag = angle_between(fit_i.phase, fit_q.phase);
        assert!((lag - TAU / 4.0).abs() < 1.0e-3, "{frequency}: {lag}");
    }
    // An even number of taps is no transformer.
    assert!(
        dspkit::hilbert::hilbert_taps::<8>(Window::Hamming)
            .iter()
            .all(|&t| t == 0.0)
    );
}

#[test]
fn the_frequency_shifter_moves_a_tone_by_so_many_hertz() {
    for shift in [7.0, -7.0, 2.5, 20.0] {
        for frequency in [980.0, 1180.0, 1750.0, 2100.0] {
            let mut shifter = FrequencyShifter::<127>::new(shift as f32);
            assert_eq!(shifter.delay(), 63);
            let out: Vec<f32> = sine(frequency, 0.5, 0.0, 33_000)
                .into_iter()
                .map(|x| shifter.process(x))
                .collect();
            // Four seconds: a whole number of cycles of every frequency
            // here, so that one does not leak into the fit of another.
            let window = &out[1000..];
            let moved = fit(window, frequency + shift);
            assert!(
                (moved.amplitude / 0.5 - 1.0).abs() < 2.0e-3,
                "{frequency} {shift}"
            );
            // What is left is the other sideband, at the frequency less
            // the shift, and it is 50 dB down.
            let image = fit(window, frequency - shift);
            assert!(
                db(image.amplitude / 0.5) < -50.0,
                "{frequency} {shift}: {}",
                db(image.amplitude / 0.5)
            );
            assert!(db(moved.residual / 0.5) < -50.0);
        }
    }
}
