// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The blocks of a synchronous modem, each alone: the scrambler, pulse
//! shaping, passband conversion, timing recovery, carrier recovery, the
//! equaliser, the slicers and the measure of signal quality.

use dspkit::carrier::{CarrierLoop, CarrierSettings};
use dspkit::complex::Complex;
use dspkit::equalizer::Equalizer;
use dspkit::fir::Window;
use dspkit::interp::Resampler;
use dspkit::passband::{Demodulator, Modulator};
use dspkit::quality::SignalQuality;
use dspkit::scramble::{Descrambler, Polynomial, Scrambler};
use dspkit::shape::{ComplexFir, PulseShaper, matched_taps, root_raised_cosine};
use dspkit::slicer::{Slicer4, Slicer16, TableSlicer};
use dspkit::timing::{GARDNER_GAIN_75, TimingRecovery, TimingSettings};

const TAU: f64 = std::f64::consts::TAU;
const PI: f64 = std::f64::consts::PI;

/// The polynomial of V.22 clause 5.
const V22: Polynomial = Polynomial::new(14, 17);

/// The roll-off of V.22 clause 2.4.
const ROLLOFF: f64 = 0.75;

/// Samples in a symbol at 600 baud.
const SPS: f64 = 8000.0 / 600.0;

/// The matched filter: eight symbols long.
const MATCHED: usize = 107;

type Shaper = PulseShaper<40, 8>;

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// A generator of numbers from a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn bit(&mut self) -> bool {
        self.next() >> 63 == 1
    }

    fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Complex noise of this power.
    fn noise(&mut self, power: f64) -> Complex {
        let radius = (-power * self.uniform().ln()).sqrt();
        let angle = TAU * self.uniform();
        Complex::new((radius * angle.cos()) as f32, (radius * angle.sin()) as f32)
    }

    /// One of four points, of power one.
    fn four(&mut self) -> Complex {
        let part = |bit: bool| if bit { 1.0 } else { -1.0 } * std::f32::consts::FRAC_1_SQRT_2;
        Complex::new(part(self.bit()), part(self.bit()))
    }

    /// One of sixteen points, of mean power one.
    fn sixteen(&mut self) -> Complex {
        let unit = 1.0 / 10f32.sqrt();
        let mut part = || (2.0 * (self.next() >> 62) as f32 - 3.0) * unit;
        Complex::new(part(), part())
    }
}

fn matched_filter() -> ComplexFir<MATCHED> {
    ComplexFir::new(matched_taps::<MATCHED>(ROLLOFF, SPS, Window::Kaiser(0.0)))
}

/// The baseband signal of these symbols at 8000 Hz.
fn shape(symbols: &[Complex]) -> Vec<Complex> {
    let mut shaper = Shaper::new(ROLLOFF, 3, Window::Kaiser(0.0));
    let mut out = Vec::new();
    let mut next = 0;
    loop {
        if shaper.wants_symbol() {
            if next == symbols.len() {
                return out;
            }
            shaper.push(symbols[next]);
            next += 1;
        }
        out.push(shaper.next_sample());
    }
}

/// The same signal on a clock that many parts in a million faster.
fn on_another_clock(signal: &[Complex], ppm: f64) -> Vec<Complex> {
    // The far end's clock being fast is fewer of our samples to each of
    // its symbols.
    let mut re = Resampler::with_ppm(-ppm);
    let mut im = Resampler::with_ppm(-ppm);
    let mut out = Vec::with_capacity(signal.len() + 16);
    for sample in signal {
        let a = re.process(sample.re);
        let b = im.process(sample.im);
        for (&x, &y) in a.as_slice().iter().zip(b.as_slice()) {
            out.push(Complex::new(x, y));
        }
    }
    out
}

fn mean_square(signal: &[Complex]) -> f64 {
    signal.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / signal.len() as f64
}

// ---------------------------------------------------------------- scrambler

fn bits(text: &str) -> Vec<bool> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c == '1')
        .collect()
}

#[test]
fn the_scrambler_gives_the_sequences_worked_from_the_polynomial() {
    // Ones into a scrambler at rest. Each bit sent is one, added to the
    // bits sent 14 and 17 before:
    //   bits 0 to 13    nothing behind them yet: 1.
    //   bits 14 to 16   1 + 1 + 0 = 0.
    //   bits 17 to 27   1 + 1 + 1 = 1.
    //   bits 28 to 30   1 + 0 + 1 = 0  (14 back is one of 14 to 16).
    //   bits 31 to 33   1 + 1 + 0 = 0  (17 back is one of 14 to 16).
    //   bit 34          1 + 1 + 1 = 1.
    let expected = bits("11111111111111 000 11111111111 000 000 1");
    let mut scrambler = Scrambler::new(V22);
    let sent: Vec<bool> = expected.iter().map(|_| scrambler.bit(true)).collect();
    assert_eq!(sent, expected);

    // A single one and then zeros is the polynomial's reciprocal, the sum
    // of the powers of (x^14 + x^17): of the first power 14 and 17; of
    // the second 28, 31 twice, which is nothing, and 34; of the third 42,
    // 45 and 48 three times each, which is once, and 51.
    let ones_at = [0usize, 14, 17, 28, 34, 42, 45, 48, 51];
    let mut scrambler = Scrambler::new(V22);
    for n in 0..56 {
        assert_eq!(
            scrambler.bit(n == 0),
            ones_at.contains(&n),
            "bit {n} of the impulse"
        );
    }
}

#[test]
fn the_descrambler_gives_back_what_was_scrambled_for_any_polynomial() {
    for polynomial in [V22, Polynomial::new(18, 23), Polynomial::new(5, 23)] {
        let mut rng = Rng(7);
        let mut scrambler = Scrambler::new(polynomial);
        let mut descrambler = Descrambler::new(polynomial);
        let mut ones = 0;
        for _ in 0..20_000 {
            let data = rng.bit();
            let line = scrambler.bit(data);
            ones += u32::from(line);
            assert_eq!(descrambler.bit(line), data);
        }
        // What is sent is as much ones as zeros.
        assert!((9_500..10_500).contains(&ones), "{ones} ones of 20000");
    }
}

#[test]
fn the_descrambler_is_in_step_after_as_many_bits_as_the_polynomial_is_long() {
    let mut rng = Rng(11);
    let mut scrambler = Scrambler::new(V22);
    // Let the scrambler run, so that its history is not the
    // descrambler's.
    for _ in 0..100 {
        scrambler.bit(rng.bit());
    }
    let mut descrambler = Descrambler::new(V22);
    let mut wrong_after = 0;
    for n in 0..1000 {
        let data = rng.bit();
        if descrambler.bit(scrambler.bit(data)) != data {
            wrong_after = n + 1;
        }
    }
    assert!(wrong_after <= 17, "wrong as late as bit {wrong_after}");
}

#[test]
fn one_error_on_the_line_is_three_in_the_data_where_the_polynomial_says() {
    for (polynomial, delays) in [
        (V22, [0usize, 14, 17]),
        (Polynomial::new(18, 23), [0, 18, 23]),
    ] {
        let mut rng = Rng(13);
        let mut scrambler = Scrambler::new(polynomial);
        let mut descrambler = Descrambler::new(polynomial);
        let mut wrong = Vec::new();
        for n in 0..500usize {
            let data = rng.bit();
            let line = scrambler.bit(data) ^ (n == 200);
            if descrambler.bit(line) != data {
                wrong.push(n - 200);
            }
        }
        assert_eq!(wrong, delays);
    }
}

/// The data that makes a scrambler at rest send ones for ever: each bit
/// is one added to what the polynomial takes from the line, which is all
/// ones.
fn lockup(n: usize) -> bool {
    !((n >= 14) ^ (n >= 17))
}

#[test]
fn the_guard_breaks_a_run_of_64_ones_and_the_descrambler_puts_it_right() {
    // Without the guard the line is ones for as long as the data lasts.
    let mut scrambler = Scrambler::new(V22);
    assert!((0..300).all(|n| scrambler.bit(lockup(n))));

    // With it, the bit after 64 ones is inverted (V.22 clause 5.1), which
    // makes that bit of the line a zero.
    let mut scrambler = Scrambler::with_guard(V22, 64);
    let line: Vec<bool> = (0..300).map(|n| scrambler.bit(lockup(n))).collect();
    assert!(line[..64].iter().all(|&bit| bit));
    assert!(!line[64]);
    let longest = line
        .split(|&bit| !bit)
        .map(<[bool]>::len)
        .max()
        .unwrap_or(0);
    assert_eq!(longest, 64);

    // A descrambler with the guard gives the data back whole (clause
    // 5.2); one without has the inverted bit wrong and no other.
    let mut guarded = Descrambler::with_guard(V22, 64);
    let mut plain = Descrambler::new(V22);
    let mut wrong = Vec::new();
    for (n, &bit) in line.iter().enumerate() {
        assert_eq!(guarded.bit(bit), lockup(n), "bit {n}");
        if plain.bit(bit) != lockup(n) {
            wrong.push(n);
        }
    }
    assert!(wrong.contains(&64));

    // The guard is not in the way of ordinary data.
    let mut rng = Rng(17);
    let mut scrambler = Scrambler::with_guard(V22, 64);
    let mut descrambler = Descrambler::with_guard(V22, 64);
    for _ in 0..100_000 {
        let data = rng.bit();
        assert_eq!(descrambler.bit(scrambler.bit(data)), data);
    }
}

#[test]
fn an_unscrambled_signal_goes_through_and_is_remembered() {
    // After bits sent as they are, the scrambler goes on from the line's
    // history, so a descrambler that heard them is in step at once.
    let mut rng = Rng(19);
    let mut scrambler = Scrambler::new(V22);
    let mut descrambler = Descrambler::new(V22);
    for _ in 0..40 {
        let bit = rng.bit();
        assert_eq!(scrambler.bypass(bit), bit);
        descrambler.bit(bit);
    }
    for _ in 0..200 {
        let data = rng.bit();
        assert_eq!(descrambler.bit(scrambler.bit(data)), data);
    }
}

// ------------------------------------------------------------ pulse shaping

#[test]
fn the_pulse_is_the_closed_form_and_has_an_energy_of_one() {
    let b = ROLLOFF;
    assert!((root_raised_cosine(0.0, b) - (1.0 - b + 4.0 * b / PI)).abs() < 1e-12);
    // Where the general form is nothing over nothing, the value is the
    // limit from either side.
    let t = 1.0 / (4.0 * b);
    let at = root_raised_cosine(t, b);
    assert!((root_raised_cosine(t - 1e-6, b) - at).abs() < 1e-5);
    assert!((root_raised_cosine(t + 1e-6, b) - at).abs() < 1e-5);
    assert!((root_raised_cosine(-t, b) - at).abs() < 1e-12);
    // With no roll-off it is a sinc.
    assert!((root_raised_cosine(0.5, 0.0) - 2.0 / PI).abs() < 1e-12);

    let step = 1.0 / 64.0;
    let energy: f64 = (-64 * 40..=64 * 40)
        .map(|n| root_raised_cosine(n as f64 * step, b).powi(2) * step)
        .sum();
    assert!((energy - 1.0).abs() < 1e-4, "energy {energy}");

    // Two of them are a raised cosine: one at its peak and nothing at
    // every other symbol instant.
    for k in 0..6 {
        let value: f64 = (-64 * 40..=64 * 40)
            .map(|n| {
                let t = n as f64 * step;
                root_raised_cosine(t, b) * root_raised_cosine(f64::from(k) - t, b) * step
            })
            .sum();
        let expected = if k == 0 { 1.0 } else { 0.0 };
        assert!((value - expected).abs() < 1e-4, "at symbol {k}: {value}");
    }
}

/// The gain of real taps at a frequency.
fn response(taps: &[f32], frequency: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &tap) in taps.iter().enumerate() {
        let angle = TAU * frequency * n as f64 / 8000.0;
        re += f64::from(tap) * angle.cos();
        im -= f64::from(tap) * angle.sin();
    }
    re.hypot(im)
}

#[test]
fn the_matched_filter_has_the_spectrum_of_the_root_of_a_raised_cosine() {
    let taps = matched_taps::<MATCHED>(ROLLOFF, SPS, Window::Kaiser(0.0));
    let dc = response(&taps, 0.0);
    // Flat to (1 - 0.75) of half the symbol rate, 75 Hz.
    for frequency in [25.0, 50.0, 75.0] {
        let gain = 2.0 * db(response(&taps, frequency) / dc);
        assert!(gain.abs() < 0.1, "{frequency} Hz: {gain} dB");
    }
    // Half the power at half the symbol rate, 300 Hz.
    let gain = 2.0 * db(response(&taps, 300.0) / dc);
    assert!((gain + 3.01).abs() < 0.1, "300 Hz: {gain} dB");
    // The root of a raised cosine between: at 187.5 Hz, a quarter of the
    // way through the roll-off, the power is cos^2 of an eighth of a turn
    // by a quarter.
    let expected = db(0.5 * (1.0 + (PI * (187.5 - 75.0) / 450.0).cos()));
    let gain = 2.0 * db(response(&taps, 187.5) / dc);
    assert!((gain - expected).abs() < 0.1, "187.5 Hz: {gain} dB");
    // Nothing past (1 + 0.75) of half the symbol rate, 525 Hz.
    for frequency in [600.0, 800.0, 1200.0, 2400.0, 3600.0] {
        let gain = 2.0 * db(response(&taps, frequency) / dc);
        assert!(gain < -45.0, "{frequency} Hz: {gain} dB");
    }
}

#[test]
fn the_shaper_and_the_matched_filter_leave_no_symbol_in_another() {
    let mut rng = Rng(23);
    let symbols: Vec<Complex> = (0..3000).map(|_| rng.sixteen()).collect();
    let signal = shape(&symbols);

    // Symbols of power one are a signal of power one.
    let power = mean_square(&signal[200..signal.len() - 200]);
    assert!((db(power)).abs() < 0.1, "power {power}");
    // 13 and a third samples to a symbol.
    assert_eq!(signal.len(), symbols.len() * 40 / 3);

    let mut filter = matched_filter();
    let out: Vec<Complex> = signal.iter().map(|&s| filter.process(s)).collect();
    // Symbol k is at its peak (k + 4) symbols into the shaper's output,
    // and the matched filter is 53 samples late. Every third symbol falls
    // on a sample.
    let (mut error, mut count) = (0.0, 0);
    for k in (2..symbols.len() - 8).step_by(3) {
        let at = (k + 4) * 40 / 3 + (MATCHED - 1) / 2;
        assert_eq!((k + 4) * 40 % 3, 0);
        error += f64::from((out[at] - symbols[k]).norm_sqr());
        count += 1;
    }
    let error = db(error / f64::from(count));
    assert!(error < -40.0, "what the other symbols leave: {error} dB");
}

#[test]
fn a_signal_goes_to_a_carrier_and_comes_back() {
    for carrier in [1200.0, 2400.0] {
        let mut rng = Rng(29);
        let symbols: Vec<Complex> = (0..1500).map(|_| rng.sixteen()).collect();
        let signal = shape(&symbols);
        let mut modulator = Modulator::new(carrier);
        let line: Vec<f32> = signal.iter().map(|&s| modulator.process(s)).collect();

        // The power on the line is the power at baseband.
        let power = line[200..line.len() - 200]
            .iter()
            .map(|&s| f64::from(s * s))
            .sum::<f64>()
            / (line.len() - 400) as f64;
        assert!(db(power).abs() < 0.15, "{carrier} Hz: line power {power}");

        let mut demodulator = Demodulator::new(carrier);
        let mut filter = matched_filter();
        let out: Vec<Complex> = line
            .iter()
            .map(|&s| filter.process(demodulator.process(s)))
            .collect();
        let (mut error, mut count) = (0.0, 0);
        for k in (2..symbols.len() - 8).step_by(3) {
            let at = (k + 4) * 40 / 3 + (MATCHED - 1) / 2;
            error += f64::from((out[at] - symbols[k]).norm_sqr());
            count += 1;
        }
        let error = db(error / f64::from(count));
        assert!(error < -38.0, "{carrier} Hz: {error} dB");
    }
}

// ------------------------------------------------------------------- timing

/// What a timing loop made of a signal.
struct Timed {
    /// The on-symbol strobes.
    symbols: Vec<Complex>,
    /// When each was, in samples.
    times: Vec<f64>,
    offset_ppm: f32,
}

fn recover(signal: &[Complex], settings: TimingSettings, slip: f64) -> Timed {
    let mut filter = matched_filter();
    let mut timing = TimingRecovery::new(SPS, settings);
    timing.slip(slip);
    let mut timed = Timed {
        symbols: Vec::new(),
        times: Vec::new(),
        offset_ppm: 0.0,
    };
    for (n, &sample) in signal.iter().enumerate() {
        if let Some(strobe) = timing.process(filter.process(sample))
            && strobe.on_symbol
        {
            timed.symbols.push(strobe.sample);
            timed
                .times
                .push(n as f64 + f64::from(strobe.fraction) - timing.delay());
        }
    }
    timed.offset_ppm = timing.offset_ppm();
    timed
}

/// The lag that fits the recovered symbols to the ones sent best over a
/// stretch, and the error left there, in decibels under the signal.
fn fit_lag(recovered: &[Complex], sent: &[Complex], from: usize, len: usize) -> (i64, f64) {
    let mut best = (0, f64::INFINITY);
    for lag in -4i64..12 {
        let mut error = 0.0;
        for (n, &symbol) in recovered.iter().enumerate().skip(from).take(len) {
            let k = n as i64 - lag;
            if k < 0 || k as usize >= sent.len() {
                error = f64::INFINITY;
                break;
            }
            error += f64::from((symbol - sent[k as usize]).norm_sqr());
        }
        if error < best.1 {
            best = (lag, error);
        }
    }
    (best.0, db(best.1 / len as f64))
}

fn acquisition() -> TimingSettings {
    TimingSettings::with_bandwidth(0.02, 1.0, GARDNER_GAIN_75)
}

fn tracking() -> TimingSettings {
    TimingSettings::with_bandwidth(0.005, 1.0, GARDNER_GAIN_75)
}

#[test]
fn the_timing_detector_has_the_slope_it_is_said_to_have() {
    // With the loop open, the mean error against the timing is an S, and
    // its slope where it crosses nothing is the detector's gain. The
    // timing that is right is found by closing the loop first.
    let mut rng = Rng(31);
    let symbols: Vec<Complex> = (0..6000).map(|_| rng.four()).collect();
    let signal = shape(&symbols);
    let open = TimingSettings {
        proportional: 0.0,
        integral: 0.0,
        ..tracking()
    };
    let mean_error = |late: f64| {
        let mut filter = matched_filter();
        let mut timing = TimingRecovery::new(SPS, tracking());
        let mut sum = 0.0;
        let mut count = 0;
        for (n, &sample) in signal.iter().enumerate() {
            if n == 40_000 {
                timing.set_settings(open);
                timing.set_frozen(true);
                timing.set_frozen(false);
                timing.slip(late);
            }
            if let Some(strobe) = timing.process(filter.process(sample))
                && strobe.on_symbol
                && n > 41_000
            {
                sum += f64::from(timing.error());
                count += 1;
            }
        }
        sum / f64::from(count)
    };
    let slope = (mean_error(0.02) - mean_error(-0.02)) / 0.04;
    let stated = f64::from(GARDNER_GAIN_75);
    assert!(
        (slope / stated - 1.0).abs() < 0.1,
        "slope {slope}, stated {stated}"
    );
    // Late is positive.
    assert!(mean_error(0.05) > 0.0 && mean_error(-0.05) < 0.0);
}

#[test]
fn the_timing_loop_pulls_in_from_any_phase() {
    let mut rng = Rng(37);
    let symbols: Vec<Complex> = (0..1200).map(|_| rng.four()).collect();
    let signal = shape(&symbols);
    // From sixteen phases across the symbol, the worst of them the one
    // half a symbol out, where the detector says nothing at first.
    for step in 0..16 {
        let slip = f64::from(step) / 16.0;
        let timed = recover(&signal, acquisition(), slip);
        // Within 300 symbols, half a second, the strobes are the symbols.
        let (_, error) = fit_lag(&timed.symbols, &symbols, 300, 600);
        assert!(error < -28.0, "from {slip} of a symbol: {error} dB");
    }
}

#[test]
fn the_timing_loop_follows_a_clock_that_is_off_without_a_slip() {
    // A minute of signal at each offset: at 200 parts in a million the
    // far end's symbols move through ours by seven whole symbols.
    for ppm in [-500.0, -200.0, 0.0, 200.0, 500.0] {
        let mut rng = Rng(41);
        let symbols: Vec<Complex> = (0..36_000).map(|_| rng.sixteen()).collect();
        let signal = on_another_clock(&shape(&symbols), ppm);
        let mut settings = tracking();
        settings.max_offset = 1.0e-3;
        let timed = recover(&signal, settings, 0.3);

        // The same lag fits at the start and at the end, and all the way
        // between: no symbol was dropped and none made twice.
        let (lag, _) = fit_lag(&timed.symbols, &symbols, 1000, 500);
        let len = timed.symbols.len() - 1100;
        let mut error = 0.0;
        for (n, &symbol) in timed.symbols.iter().enumerate().skip(1000).take(len) {
            let k = (n as i64 - lag) as usize;
            error += f64::from((symbol - symbols[k]).norm_sqr());
        }
        let error = db(error / len as f64);
        assert!(error < -30.0, "{ppm} ppm: {error} dB");
        assert!(
            (timed.symbols.len() as i64 - symbols.len() as i64).abs() <= 12,
            "{ppm} ppm: {} strobes for {} symbols",
            timed.symbols.len(),
            symbols.len()
        );
        // And the loop says what the offset is.
        assert!(
            (f64::from(timed.offset_ppm) - ppm).abs() < 25.0,
            "{ppm} ppm: the loop says {}",
            timed.offset_ppm
        );
    }
}

#[test]
fn the_timing_loop_strobes_on_the_symbols_with_little_jitter() {
    // Sixteen points and noise 30 dB down. Where each strobe was is
    // compared with where the symbol was: symbol k is at its peak at
    // (k + 4) symbols and 53 samples.
    let mut rng = Rng(43);
    let symbols: Vec<Complex> = (0..12_000).map(|_| rng.sixteen()).collect();
    let mut signal = shape(&symbols);
    for sample in &mut signal {
        // The noise in the matched filter's band is 30 dB under the
        // signal: the band is a thirteenth of the sampled one.
        *sample += rng.noise(1.0e-3 * SPS);
    }
    let timed = recover(&signal, tracking(), 0.25);
    let peak0 = 4.0 * SPS + (MATCHED as f64 - 1.0) / 2.0;
    let residuals: Vec<f64> = timed.times[2000..]
        .iter()
        .map(|&t| {
            let symbols = (t - peak0) / SPS;
            symbols - symbols.round()
        })
        .collect();
    let mean = residuals.iter().sum::<f64>() / residuals.len() as f64;
    let jitter =
        (residuals.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / residuals.len() as f64).sqrt();
    assert!(mean.abs() < 0.01, "the strobes are {mean} of a symbol off");
    assert!(jitter < 0.01, "jitter of {jitter} of a symbol");
}

#[test]
fn a_frozen_timing_loop_keeps_its_rate_through_silence() {
    let mut rng = Rng(47);
    let symbols: Vec<Complex> = (0..9000).map(|_| rng.four()).collect();
    let mut signal = on_another_clock(&shape(&symbols), 200.0);
    // A second of nothing in the middle.
    for sample in &mut signal[60_000..68_000] {
        *sample = Complex::ZERO;
    }
    let mut filter = matched_filter();
    let mut timing = TimingRecovery::new(SPS, tracking());
    let mut recovered = Vec::new();
    for (n, &sample) in signal.iter().enumerate() {
        if n == 59_900 {
            timing.set_frozen(true);
        }
        if n == 68_200 {
            timing.set_frozen(false);
        }
        if let Some(strobe) = timing.process(filter.process(sample))
            && strobe.on_symbol
        {
            recovered.push(strobe.sample);
        }
    }
    // The symbols after the gap are where they would have been.
    let (before, _) = fit_lag(&recovered, &symbols, 3000, 800);
    let (after, error) = fit_lag(&recovered, &symbols, 5300, 800);
    assert_eq!(before, after);
    assert!(error < -28.0, "after the gap: {error} dB");
}

// ------------------------------------------------------------------ carrier

/// Runs a carrier loop over symbols turned by an offset, and gives the
/// error in degrees at each symbol and the decisions that were wrong.
struct CarrierRun {
    degrees: Vec<f64>,
    wrong: usize,
    frequency_hz: f32,
}

#[allow(clippy::too_many_arguments)]
fn carrier_run(
    settings: CarrierSettings,
    offset_hz: f64,
    start_degrees: f64,
    jump: Option<(usize, f64)>,
    four_for: usize,
    count: usize,
    noise: f64,
    seed: u64,
) -> CarrierRun {
    let mut rng = Rng(seed);
    let four = Slicer4::new(Complex::new(
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ));
    let sixteen = Slicer16::new(1.0 / 10f32.sqrt());
    let mut carrier = CarrierLoop::new(settings);
    let mut phase = start_degrees.to_radians();
    let mut run = CarrierRun {
        degrees: Vec::with_capacity(count),
        wrong: 0,
        frequency_hz: 0.0,
    };
    for n in 0..count {
        if let Some((at, degrees)) = jump
            && n == at
        {
            phase += degrees.to_radians();
        }
        let sent = if n < four_for {
            rng.four()
        } else {
            rng.sixteen()
        };
        let turn = Complex::new(phase.cos() as f32, phase.sin() as f32);
        let received = sent * turn + rng.noise(noise);
        let derotated = carrier.derotate(received);
        let decision = if n < four_for {
            four.slice(derotated).point
        } else {
            sixteen.slice(derotated).point
        };
        if n >= four_for && (decision - sent).norm_sqr() > 1.0e-6 {
            run.wrong += 1;
        }
        carrier.update(CarrierLoop::phase_error(derotated, decision, 1.0));
        // The loop's phase against the true one, to the nearest quarter
        // turn: four points look the same turned by a quarter.
        let loop_phase = f64::from(carrier.phase()) / 4_294_967_296.0 * 360.0;
        let behind = (phase.to_degrees() + offset_hz / 600.0 * 360.0 - loop_phase).rem_euclid(90.0);
        run.degrees
            .push(if behind > 45.0 { behind - 90.0 } else { behind });
        phase += TAU * offset_hz / 600.0;
    }
    run.frequency_hz = carrier.frequency_hz(600.0);
    run
}

fn carrier_settings() -> CarrierSettings {
    // 18 Hz at 600 baud.
    CarrierSettings::with_bandwidth(0.03, 0.707)
}

#[test]
fn the_carrier_loop_pulls_in_a_frequency_offset() {
    for offset in [-10.0, -7.0, -1.0, 0.0, 1.0, 7.0, 10.0] {
        for start in [0.0, 20.0, 40.0, 44.0] {
            let run = carrier_run(
                carrier_settings(),
                offset,
                start,
                None,
                2000,
                2000,
                1.0e-3,
                53,
            );
            // Inside a second it is within two degrees and stays there.
            let worst = run.degrees[600..]
                .iter()
                .fold(0f64, |worst, d| worst.max(d.abs()));
            assert!(
                worst < 2.0,
                "{offset} Hz from {start} degrees: {worst} degrees"
            );
            assert!(
                (f64::from(run.frequency_hz) - offset).abs() < 0.2,
                "{offset} Hz: the loop says {}",
                run.frequency_hz
            );
        }
    }
}

#[test]
fn the_carrier_loop_follows_sixteen_points_once_it_has_locked_on_four() {
    // As a handshake has it: four points first, then sixteen, with noise
    // 25 dB down, and the offset there all the while.
    for offset in [-10.0, 10.0] {
        let run = carrier_run(
            carrier_settings(),
            offset,
            30.0,
            None,
            600,
            30_000,
            3.2e-3,
            59,
        );
        assert_eq!(run.wrong, 0, "{offset} Hz: decisions wrong");
        let worst = run.degrees[600..]
            .iter()
            .fold(0f64, |worst, d| worst.max(d.abs()));
        assert!(worst < 4.0, "{offset} Hz: {worst} degrees");
    }
}

#[test]
fn the_carrier_loop_comes_back_from_a_phase_jump() {
    // Thirty degrees on four points, which is inside the 45 that a
    // decision allows.
    let run = carrier_run(
        carrier_settings(),
        5.0,
        0.0,
        Some((1000, 30.0)),
        3000,
        3000,
        1.0e-4,
        61,
    );
    assert!(run.degrees[1000].abs() > 25.0);
    let worst = run.degrees[1100..]
        .iter()
        .fold(0f64, |worst, d| worst.max(d.abs()));
    assert!(worst < 1.5, "a hundred symbols after: {worst} degrees");

    // Ten degrees on sixteen points, which is inside what their inner
    // decisions allow; no decision is wrong.
    let run = carrier_run(
        carrier_settings(),
        5.0,
        0.0,
        Some((1000, 10.0)),
        600,
        3000,
        1.0e-4,
        67,
    );
    assert_eq!(run.wrong, 0);
    let worst = run.degrees[1100..]
        .iter()
        .fold(0f64, |worst, d| worst.max(d.abs()));
    assert!(worst < 1.5, "a hundred symbols after: {worst} degrees");
}

#[test]
fn a_coasting_carrier_loop_runs_on_at_the_frequency_it_found() {
    let mut carrier = CarrierLoop::new(carrier_settings());
    let four = Slicer4::new(Complex::new(1.0, 1.0));
    let mut phase = 0.0f64;
    let step = TAU * 8.0 / 600.0;
    let mut rng = Rng(71);
    for _ in 0..2000 {
        let sent = rng.four().scale(std::f32::consts::SQRT_2);
        let received = sent * Complex::new(phase.cos() as f32, phase.sin() as f32);
        let derotated = carrier.derotate(received);
        let decision = four.slice(derotated).point;
        carrier.update(CarrierLoop::phase_error(derotated, decision, 2.0));
        phase += step;
    }
    let before = f64::from(carrier.phase()) / 4_294_967_296.0 * 360.0;
    for _ in 0..100 {
        carrier.coast();
    }
    // A sixth of a second later it has turned as the carrier has: 8 Hz
    // for 100 symbols is a turn and a third.
    let after = f64::from(carrier.phase()) / 4_294_967_296.0 * 360.0;
    let moved = (after - before).rem_euclid(360.0);
    let expected = (8.0 * 360.0 * 100.0 / 600.0f64).rem_euclid(360.0);
    let off = (moved - expected + 180.0).rem_euclid(360.0) - 180.0;
    assert!(off.abs() < 2.0, "{off} degrees after coasting");
}

// ---------------------------------------------------------------- equaliser

const EQ_TAPS: usize = 24;

/// The pulse at two samples a symbol, 33 taps.
fn half_symbol_pulse(shift: f64) -> Vec<f64> {
    (0..33)
        .map(|n| root_raised_cosine((f64::from(n) - 16.0 + shift) / 2.0, ROLLOFF))
        .collect()
}

/// Symbols through a channel, at two samples a symbol: shaped, through
/// the channel's taps (half a symbol apart), noise added, and through the
/// matched filter.
fn through_channel(
    symbols: &[Complex],
    channel: &[Complex],
    shift: f64,
    noise: f64,
    rng: &mut Rng,
) -> Vec<Complex> {
    let transmit = half_symbol_pulse(shift);
    let receive: Vec<f64> = half_symbol_pulse(0.0).iter().map(|tap| tap / 2.0).collect();
    let len = symbols.len() * 2;
    let mut shaped = vec![Complex::ZERO; len];
    for (k, &symbol) in symbols.iter().enumerate() {
        for (n, &tap) in transmit.iter().enumerate() {
            if 2 * k + n < len {
                shaped[2 * k + n] += symbol.scale(tap as f32);
            }
        }
    }
    let mut line = vec![Complex::ZERO; len];
    for n in 0..len {
        let mut sum = Complex::ZERO;
        for (m, &tap) in channel.iter().enumerate() {
            if n >= m {
                sum += shaped[n - m] * tap;
            }
        }
        line[n] = sum + rng.noise(noise);
    }
    let mut out = vec![Complex::ZERO; len];
    for n in 0..len {
        let mut sum = Complex::ZERO;
        for (m, &tap) in receive.iter().enumerate() {
            if n >= m {
                sum += line[n - m].scale(tap as f32);
            }
        }
        out[n] = sum;
    }
    out
}

/// The delay, in symbols, of the path above and the equaliser's middle
/// tap, for a channel whose main tap is its second: the pulse and the
/// matched filter are 16 samples each, the channel one, and the middle
/// tap is on the sample 12 before the newest, which is the second of its
/// symbol's two.
const PATH_DELAY: usize = (16 + 1 + 16 + EQ_TAPS / 2 - 1) / 2;

/// Runs the equaliser over the samples, with the first step and training
/// on the symbols sent for `train` symbols, and with the second step
/// after: deciding for itself if `decide`, and still training if not.
/// Gives the mean-squared error over the last `tail` symbols, in
/// decibels, and the decisions wrong after training.
fn equalise(
    samples: &[Complex],
    symbols: &[Complex],
    train: usize,
    steps: (f32, f32),
    decide: bool,
    tail: usize,
) -> (f64, usize) {
    let slicer = Slicer16::new(1.0 / 10f32.sqrt());
    let mut equalizer = Equalizer::<EQ_TAPS>::new(steps.0, 1.0e-6);
    let count = symbols.len();
    let (mut error_sum, mut wrong) = (0.0, 0);
    for m in 0..count {
        equalizer.push(samples[2 * m]);
        equalizer.push(samples[2 * m + 1]);
        if m < PATH_DELAY {
            continue;
        }
        let sent = symbols[m - PATH_DELAY];
        let output = equalizer.output();
        if m >= train {
            equalizer.set_step(steps.1);
        }
        let reference = if m < train || !decide {
            sent
        } else {
            let decision = slicer.slice(output).point;
            if (decision - sent).norm_sqr() > 1.0e-6 {
                wrong += 1;
            }
            decision
        };
        equalizer.update(output - reference);
        if m >= count - tail {
            error_sum += f64::from((output - sent).norm_sqr());
        }
    }
    (db(error_sum / tail as f64), wrong)
}

/// The least mean-squared error any filter of the equaliser's length can
/// reach on these samples: the Wiener filter, found by least squares.
// The matrix is clearer by its rows and columns.
#[allow(clippy::needless_range_loop)]
fn least_error(samples: &[Complex], symbols: &[Complex]) -> f64 {
    type C = (f64, f64);
    let mul = |a: C, b: C| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
    let n = EQ_TAPS;
    let mut r = vec![vec![(0.0, 0.0); n + 1]; n];
    let line = |m: usize, i: usize| -> C {
        let s = samples[2 * m + 1 - i];
        (f64::from(s.re), f64::from(s.im))
    };
    let range = EQ_TAPS..symbols.len();
    for m in range.clone() {
        let d = symbols[m - PATH_DELAY];
        let d = (f64::from(d.re), f64::from(d.im));
        for i in 0..n {
            let xi = line(m, i);
            let conj = (xi.0, -xi.1);
            for j in 0..n {
                let p = mul(conj, line(m, j));
                r[i][j].0 += p.0;
                r[i][j].1 += p.1;
            }
            let p = mul(conj, d);
            r[i][n].0 += p.0;
            r[i][n].1 += p.1;
        }
    }
    // Gaussian elimination with pivoting.
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&a, &b| {
                let size = |row: usize| r[row][col].0.hypot(r[row][col].1);
                size(a).total_cmp(&size(b))
            })
            .unwrap();
        r.swap(col, pivot);
        let p = r[col][col];
        let norm = p.0 * p.0 + p.1 * p.1;
        let inverse = (p.0 / norm, -p.1 / norm);
        for j in col..=n {
            r[col][j] = mul(r[col][j], inverse);
        }
        for row in 0..n {
            if row != col {
                let factor = r[row][col];
                for j in col..=n {
                    let p = mul(factor, r[col][j]);
                    r[row][j].0 -= p.0;
                    r[row][j].1 -= p.1;
                }
            }
        }
    }
    let taps: Vec<C> = (0..n).map(|i| r[i][n]).collect();
    let mut error = 0.0;
    for m in range.clone() {
        let mut y = (0.0, 0.0);
        for (i, &tap) in taps.iter().enumerate() {
            let p = mul(tap, line(m, i));
            y.0 += p.0;
            y.1 += p.1;
        }
        let d = symbols[m - PATH_DELAY];
        error += (y.0 - f64::from(d.re)).powi(2) + (y.1 - f64::from(d.im)).powi(2);
    }
    db(error / range.len() as f64)
}

/// Channels with known responses, their taps half a symbol apart and the
/// main one second.
fn channels() -> Vec<(&'static str, Vec<Complex>)> {
    vec![
        ("flat", vec![Complex::ZERO, Complex::ONE]),
        (
            "an echo after",
            vec![
                Complex::ZERO,
                Complex::ONE,
                Complex::new(0.0, 0.0),
                Complex::new(0.35, -0.2),
            ],
        ),
        (
            "an echo before and after, and a turn",
            vec![
                Complex::new(0.15, 0.1),
                Complex::new(0.6, 0.7),
                Complex::new(0.3, -0.25),
                Complex::new(-0.15, 0.1),
                Complex::new(0.05, 0.05),
            ],
        ),
    ]
}

#[test]
fn the_equaliser_converges_on_channels_of_known_response() {
    for (name, channel) in channels() {
        let mut rng = Rng(73);
        let symbols: Vec<Complex> = (0..8000).map(|_| rng.sixteen()).collect();
        let samples = through_channel(&symbols, &channel, 0.0, 0.0, &mut rng);

        // Untouched, the equaliser passes what the channel did.
        let (before, _) = equalise(&samples, &symbols, 8000, (0.0, 0.0), true, 2000);
        // Trained for 3000 symbols and left to its decisions for 5000.
        let (after, wrong) = equalise(&samples, &symbols, 3000, (0.1, 0.02), true, 2000);
        if name != "flat" {
            assert!(before > -12.0, "{name}: {before} dB before");
        }
        assert!(after < -35.0, "{name}: {after} dB after");
        assert_eq!(wrong, 0, "{name}: decisions wrong");
    }
}

#[test]
fn the_equaliser_makes_up_any_timing_phase() {
    // Samples taken a quarter and a half of a symbol from the right
    // place: taps half a symbol apart make the difference up.
    for shift in [0.5, 1.0, -0.7] {
        let mut rng = Rng(79);
        let symbols: Vec<Complex> = (0..6000).map(|_| rng.sixteen()).collect();
        let channel = [Complex::ZERO, Complex::ONE];
        let samples = through_channel(&symbols, &channel, shift, 0.0, &mut rng);
        let (after, wrong) = equalise(&samples, &symbols, 3000, (0.1, 0.02), true, 1500);
        assert!(after < -35.0, "{shift} samples off: {after} dB");
        assert_eq!(wrong, 0);
    }
}

#[test]
fn the_equaliser_in_noise_is_near_the_least_error_there_is() {
    for (name, channel) in channels() {
        for noise_db in [-20.0, -30.0] {
            let mut rng = Rng(83);
            let symbols: Vec<Complex> = (0..20_000).map(|_| rng.sixteen()).collect();
            let noise = 10f64.powf(noise_db / 10.0);
            let samples = through_channel(&symbols, &channel, 0.0, noise, &mut rng);
            let floor = least_error(&samples, &symbols);
            // A large step to get there and a small one to stay.
            let (reached, _) = equalise(&samples, &symbols, 10_000, (0.2, 0.02), false, 8000);
            println!("{name}, noise {noise_db} dB: reached {reached:.2} dB, least {floor:.2} dB");
            // The floor is the noise the equaliser lets through, with
            // what it must leave of the channel; adapting costs a little
            // over it.
            assert!(
                reached - floor < 0.6,
                "{name}, noise {noise_db} dB: reached {reached} dB, the least is {floor} dB"
            );
            assert!(reached >= floor - 0.05);
        }
    }
}

#[test]
fn the_equaliser_reports_its_error_and_keeps_its_taps_bounded() {
    let mut rng = Rng(89);
    let symbols: Vec<Complex> = (0..6000).map(|_| rng.sixteen()).collect();
    let channel = [Complex::ZERO, Complex::ONE];
    let noise = 1.0e-2;
    let samples = through_channel(&symbols, &channel, 0.0, noise, &mut rng);
    let mut equalizer = Equalizer::<EQ_TAPS>::new(0.02, 1.0e-4);
    let mut measured = 0.0;
    for m in 0..symbols.len() {
        equalizer.push(samples[2 * m]);
        equalizer.push(samples[2 * m + 1]);
        if m < PATH_DELAY {
            continue;
        }
        let error = equalizer.output() - symbols[m - PATH_DELAY];
        equalizer.update(error);
        if m >= 5000 {
            measured += f64::from(error.norm_sqr()) / 1000.0;
        }
    }
    // What it reports is what the test measured.
    let reported = f64::from(equalizer.mse());
    assert!(
        (db(reported) - db(measured)).abs() < 1.5,
        "reported {reported}, measured {measured}"
    );
    // The taps are a filter of about unit gain, not something that grew.
    let energy: f32 = equalizer.taps().iter().map(|tap| tap.norm_sqr()).sum();
    assert!((0.5..2.0).contains(&energy), "tap energy {energy}");

    // Samples that are not numbers are taken as silence and change
    // nothing for the worse.
    equalizer.push(Complex::new(f32::NAN, f32::INFINITY));
    equalizer.update(Complex::new(f32::NAN, 0.0));
    assert!(equalizer.output().is_finite());
    assert!(equalizer.taps().iter().all(|tap| tap.is_finite()));
}

// ------------------------------------------------------------------ slicers

#[test]
fn the_slicers_decide_the_nearest_point() {
    // Four points off the diagonals: (3, 1) and its quarter turns.
    let four = Slicer4::new(Complex::new(3.0, 1.0));
    let expected = [(3.0, 1.0), (-1.0, 3.0), (-3.0, -1.0), (1.0, -3.0)];
    for (index, &(re, im)) in expected.iter().enumerate() {
        assert_eq!(four.point(index as u8), Complex::new(re, im));
    }
    assert_eq!(four.mean_power(), 10.0);

    let sixteen = Slicer16::new(1.0);
    assert_eq!(sixteen.mean_power(), 10.0);
    let mut table = [Complex::ZERO; 16];
    for (index, point) in table.iter_mut().enumerate() {
        *point = sixteen.point(index as u16);
    }
    // The sixteen are the square of -3, -1, 1 and 3.
    let mut seen: Vec<(i32, i32)> = table.iter().map(|p| (p.re as i32, p.im as i32)).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 16);
    assert!(
        seen.iter()
            .all(|&(re, im)| [-3, -1, 1, 3].contains(&re) && [-3, -1, 1, 3].contains(&im))
    );
    let general = TableSlicer::new(table);
    assert_eq!(general.mean_power(), 10.0);
    let four_table = TableSlicer::new([four.point(0), four.point(1), four.point(2), four.point(3)]);

    // Against a search of every point, on points all over the plane.
    let mut rng = Rng(97);
    for _ in 0..200_000 {
        let received = Complex::new(
            (rng.uniform() * 12.0 - 6.0) as f32,
            (rng.uniform() * 12.0 - 6.0) as f32,
        );
        let fast = sixteen.slice(received);
        let slow = general.slice(received);
        assert_eq!(fast.point, slow.point, "{received:?}");
        assert_eq!(fast.index, slow.index);
        assert_eq!(sixteen.point(fast.index), fast.point);

        let fast = four.slice(received);
        let slow = four_table.slice(received);
        // On a boundary either is as near.
        let (a, b) = (
            (received - fast.point).norm_sqr(),
            (received - slow.point).norm_sqr(),
        );
        assert!((a - b).abs() <= 1.0e-3 * (1.0 + b), "{received:?}");
        assert_eq!(four.point(fast.index as u8), fast.point);
    }

    // What is not a number is decided all the same.
    let bad = Complex::new(f32::NAN, f32::NEG_INFINITY);
    assert!(four.slice(bad).point.is_finite());
    assert!(sixteen.slice(bad).point.is_finite());
    assert!(general.slice(bad).point.is_finite());
    assert_eq!(TableSlicer::<0>::new([]).slice(bad).point, Complex::ZERO);
}

// ------------------------------------------------------------------ quality

#[test]
fn the_signal_quality_is_the_ratio_of_signal_to_error() {
    for expected in [10.0, 20.0, 30.0, 40.0] {
        let mut rng = Rng(101);
        let mut quality = SignalQuality::new(256.0);
        let noise = 10f64.powf(-expected / 10.0);
        let mut readings = Vec::new();
        for n in 0..20_000 {
            let decision = rng.sixteen();
            quality.update(decision, rng.noise(noise));
            if n >= 2000 && n % 100 == 0 {
                readings.push(f64::from(quality.snr_db()));
            }
        }
        let mean = readings.iter().sum::<f64>() / readings.len() as f64;
        assert!((mean - expected).abs() < 0.3, "{expected} dB: read {mean}");
        // Smoothed: one reading is not far from another.
        let spread = readings.iter().fold(0f64, |s, r| s.max((r - mean).abs()));
        assert!(spread < 1.5, "{expected} dB: readings {spread} dB apart");
        assert!((db(f64::from(quality.mse())) + expected).abs() < 1.5);
    }

    // Nothing counted is nothing, no error at all is the most there is,
    // and what is not a number is not counted.
    let mut quality = SignalQuality::new(64.0);
    assert_eq!(quality.snr_db(), 0.0);
    quality.update(Complex::ONE, Complex::ZERO);
    assert_eq!(quality.snr_db(), 60.0);
    quality.update(Complex::new(f32::NAN, 0.0), Complex::ONE);
    assert_eq!(quality.snr_db(), 60.0);
    quality.reset();
    assert_eq!(quality.snr_db(), 0.0);
}
