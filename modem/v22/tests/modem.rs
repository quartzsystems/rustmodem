// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The 1200 and 2400 bit/s modems: the coding against the Recommendation,
//! the modulator measured, and the modulator into the demodulator through
//! the channel simulator, in data mode.

mod common;

use chansim::{ChannelConfig, Echo, Impairments, Noise};
use common::{Rig, compare};
use dspkit::Complex;
use dspkit::level::dbm0_to_amplitude;
use g711::Law;
use v22::{
    Band, CARRIER_OFF_DBM0, CARRIER_ON_DBM0, ELEMENT_1200, GuardTone, Modulator, Rate, SYMBOL_RATE,
    dibit_of_change, point, quadrant_and_element, quadrant_change,
};

const FS: f64 = 8000.0;
const TAU: f64 = std::f64::consts::TAU;
const LAW: Law = Law::Ulaw;

const RATES: [Rate; 2] = [Rate::B1200, Rate::B2400];

/// Ticks of 10 ms the ends are given to find each other's signal before
/// anything is counted: a second and a half.
const SETTLE: u64 = 150;

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

fn clean() -> ChannelConfig {
    ChannelConfig {
        seed: 1,
        ..ChannelConfig::default()
    }
}

fn both_ways(impairments: Impairments) -> ChannelConfig {
    ChannelConfig {
        seed: 1,
        ..ChannelConfig::symmetric(impairments)
    }
}

/// Runs a rig in data mode at a rate for some seconds and gives, for
/// each end, the data bits compared.
fn carry(config: &ChannelConfig, level: f32, rate: Rate, seconds: u64) -> [common::Compared; 2] {
    let mut rig = Rig::new(config, level);
    rig.start(rate, SETTLE);
    rig.run(seconds * 100);
    let a = compare(&rig.b.sent, &rig.a.received);
    let b = compare(&rig.a.sent, &rig.b.received);
    [
        a.unwrap_or_else(|| panic!("end A received nothing that was sent")),
        b.unwrap_or_else(|| panic!("end B received nothing that was sent")),
    ]
}

/// Asserts that both ends received what was sent, whole.
fn assert_carried(what: &str, config: &ChannelConfig, level: f32, rate: Rate, seconds: u64) {
    let [a, b] = carry(config, level, rate, seconds);
    let expected = (seconds * u64::from(rate.bits_per_second())) as usize;
    for (end, compared) in [("A", a), ("B", b)] {
        assert_eq!(
            compared.errors, 0,
            "{what}, {rate:?}, end {end}: {compared:?}"
        );
        assert!(
            compared.bits + 64 >= expected,
            "{what}, {rate:?}, end {end}: {compared:?} of {expected}"
        );
    }
}

// The coding.

/// Figure 2/V.22 bis, as it is drawn: for each point of the grid, the
/// last two bits of the quadbit. Rows from the imaginary part 3 down to
/// -3, columns from the real part -3 up to 3.
const FIGURE_2: [[u8; 4]; 4] = [
    [0b11, 0b01, 0b10, 0b11],
    [0b10, 0b00, 0b00, 0b01],
    [0b01, 0b00, 0b00, 0b10],
    [0b11, 0b10, 0b01, 0b11],
];

#[test]
fn the_coding_is_that_of_table_1_and_figure_2() {
    // Table 1/V.22 bis: 00 is +90 degrees, 01 none, 11 +270, 10 +180.
    assert_eq!(quadrant_change(0b00), 1);
    assert_eq!(quadrant_change(0b01), 0);
    assert_eq!(quadrant_change(0b11), 3);
    assert_eq!(quadrant_change(0b10), 2);
    for dibit in 0..4 {
        assert_eq!(dibit_of_change(quadrant_change(dibit)), dibit);
    }
    // The table's quadrant changes, as it lists them: 1 to 2, 2 to 3, 3
    // to 4 and 4 to 1 for 00, and so on, with the quadrants numbered
    // from 0 here.
    for from in 0..4u8 {
        assert_eq!((from + quadrant_change(0b00)) % 4, (from + 1) % 4);
        assert_eq!((from + quadrant_change(0b01)) % 4, from);
        assert_eq!((from + quadrant_change(0b11)) % 4, (from + 3) % 4);
        assert_eq!((from + quadrant_change(0b10)) % 4, (from + 2) % 4);
    }

    // Figure 2: every point of the grid.
    let mut seen = 0;
    for (row, elements) in FIGURE_2.iter().enumerate() {
        for (column, &element) in elements.iter().enumerate() {
            let at = Complex::new(2.0 * column as f32 - 3.0, 3.0 - 2.0 * row as f32);
            // Phase quadrant 1 is the upper right, 2 the upper left, 3
            // the lower left and 4 the lower right.
            let quadrant = match (at.re > 0.0, at.im > 0.0) {
                (true, true) => 0,
                (false, true) => 1,
                (false, false) => 2,
                (true, false) => 3,
            };
            assert_eq!(point(quadrant, element), at, "{at:?}");
            assert_eq!(quadrant_and_element(at), (quadrant, element), "{at:?}");
            seen += 1;
        }
    }
    assert_eq!(seen, 16);
    // Clause 2.5.2.2: at 1200 bit/s the element is 01, whatever the
    // quadrant; the four are a quarter turn apart and of the mean power
    // of the sixteen, which is 10.
    assert_eq!(ELEMENT_1200, 0b01);
    for quadrant in 0..4 {
        let at = point(quadrant, ELEMENT_1200);
        assert_eq!(at.norm_sqr(), 10.0);
        assert_eq!(at, point(0, ELEMENT_1200).rotate_quarters(quadrant));
    }
    let mean: f32 = (0..16).map(|n| point(n / 4, n % 4).norm_sqr()).sum::<f32>() / 16.0;
    assert_eq!(mean, 10.0);

    assert_eq!(SYMBOL_RATE, 600);
    assert_eq!(
        (Band::Low.carrier_hz(), Band::High.carrier_hz()),
        (1200.0, 2400.0)
    );
    assert_eq!(
        (Band::Low.other(), Band::High.other()),
        (Band::High, Band::Low)
    );
    assert_eq!(Rate::B1200.bits_per_second(), 1200);
    assert_eq!(Rate::B2400.bits_per_second(), 2400);
}

// The modulator.

fn line(modulator: &mut Modulator, samples: usize, seed: u64) -> Vec<f32> {
    let mut rng = chansim::Rng::new(seed);
    (0..samples)
        .map(|_| modulator.next_sample(|| rng.next_u64() & 1 == 1))
        .collect()
}

fn mean_square(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / samples.len() as f64
}

/// The power of the samples in a band a few hertz wide at a frequency,
/// averaged over stretches of the signal.
fn power_at(samples: &[f32], frequency: f64) -> f64 {
    const BLOCK: usize = 4000;
    let mut sum = 0.0;
    let mut blocks = 0;
    for block in samples.as_chunks::<BLOCK>().0 {
        let (mut re, mut im) = (0.0, 0.0);
        for (n, &x) in block.iter().enumerate() {
            // A Hann window, so that what is far off does not leak in.
            let window = 0.5 - 0.5 * (TAU * n as f64 / BLOCK as f64).cos();
            let angle = TAU * frequency * n as f64 / FS;
            re += f64::from(x) * window * angle.cos();
            im += f64::from(x) * window * angle.sin();
        }
        sum += re * re + im * im;
        blocks += 1;
    }
    sum / f64::from(blocks)
}

#[test]
fn the_modulator_sends_at_the_level_asked_for_at_600_baud() {
    for band in [Band::Low, Band::High] {
        for rate in RATES {
            for level in [-9.0f32, -12.0, -20.0] {
                let mut modulator = Modulator::new(band, level, GuardTone::Off, LAW);
                modulator.set_rate(rate);
                let mut asked = 0u32;
                let mut rng = chansim::Rng::new(3);
                let samples: Vec<f32> = (0..80_000)
                    .map(|_| {
                        modulator.next_sample(|| {
                            asked += 1;
                            rng.next_u64() & 1 == 1
                        })
                    })
                    .collect();
                // Ten seconds: 6000 symbols exactly (clause 2.5.1).
                assert_eq!(asked, 6000 * u32::from(rate.bits_per_symbol()));
                // The level is the mean power of a sine wave at it.
                let stated = f64::from(dbm0_to_amplitude(level, LAW)).powi(2) / 2.0;
                let measured = mean_square(&samples[800..]);
                assert!(
                    db(measured / stated).abs() < 0.15,
                    "{band:?} {rate:?} at {level}: {} dB off",
                    db(measured / stated)
                );
                // Nothing clips.
                assert!(samples.iter().all(|s| s.abs() < 0.95));
            }
        }
    }
}

/// Figure 1/V.22 bis: the limits of the transmitted spectrum, as hertz
/// from the carrier and the greatest and least amplitude in decibels
/// against that at the carrier.
const FIGURE_1: [(f64, f64, f64); 5] = [
    (125.0, 0.75, -0.75),
    (200.0, -0.75, -2.0),
    (300.0, -2.0, -4.0),
    (400.0, -5.0, -9.0),
    (450.0, -9.0, -16.3),
];

#[test]
fn the_spectrum_is_within_the_limits_of_figure_1() {
    for band in [Band::Low, Band::High] {
        for rate in RATES {
            let mut modulator = Modulator::new(band, -12.0, GuardTone::Off, LAW);
            modulator.set_rate(rate);
            let samples = line(&mut modulator, 1_600_000, 5);
            let carrier = f64::from(band.carrier_hz());
            // The level at the carrier: the mean of a few points in the
            // flat of the spectrum.
            let flat = [-50.0, -25.0, 0.0, 25.0, 50.0]
                .iter()
                .map(|offset| power_at(&samples, carrier + offset))
                .sum::<f64>()
                / 5.0;
            for (offset, most, least) in FIGURE_1 {
                for side in [-1.0, 1.0] {
                    let level = db(power_at(&samples, carrier + side * offset) / flat);
                    // A third of a decibel for what a measurement of
                    // random data over three minutes is out by.
                    assert!(
                        level < most + 0.35 && level > least - 0.35,
                        "{band:?} {rate:?} at {} Hz: {level:.2} dB, limits {most} and {least}",
                        side * offset
                    );
                }
            }
            // By 550 Hz from the carrier it is 20 dB down, the figure's
            // last limit, and far more.
            for offset in [550.0, 600.0] {
                for side in [-1.0, 1.0] {
                    let level = db(power_at(&samples, carrier + side * offset) / flat);
                    assert!(level < -30.0, "{band:?} {rate:?} at {offset} Hz: {level}");
                }
            }
            // In the other channel, and everywhere else, there is all but
            // nothing: this end's receiver listens there.
            let other = f64::from(band.other().carrier_hz());
            for offset in [-500.0, -300.0, 0.0, 300.0, 500.0] {
                let level = db(power_at(&samples, other + offset) / flat);
                assert!(
                    level < -70.0,
                    "{band:?} {rate:?}: {level} dB in the other channel"
                );
            }
        }
    }
}

#[test]
fn the_guard_tone_is_at_its_level_and_in_the_high_channel_only() {
    // Clause 2.2: the 1800 Hz tone 6 dB and the 550 Hz tone 3 dB below
    // the data, each within 1 dB; clause 2.1: in the high channel only.
    for (guard, hz, below) in [
        (GuardTone::Hz1800, 1800.0, 6.0),
        (GuardTone::Hz550, 550.0, 3.0),
    ] {
        let level = -12.0f32;
        let total = f64::from(dbm0_to_amplitude(level, LAW)).powi(2) / 2.0;

        let mut modulator = Modulator::new(Band::High, level, guard, LAW);
        let samples = line(&mut modulator, 160_000, 7);
        let samples = &samples[800..];
        // The tone alone: correlated out of the signal.
        let (mut s, mut c) = (0.0, 0.0);
        for (n, &x) in samples.iter().enumerate() {
            let (sin, cos) = (TAU * hz * n as f64 / FS).sin_cos();
            s += f64::from(x) * sin;
            c += f64::from(x) * cos;
        }
        let amplitude = 2.0 * s.hypot(c) / samples.len() as f64;
        let tone = amplitude * amplitude / 2.0;
        let all = mean_square(samples);
        let data = all - tone;
        // The whole is the level asked for (V.22 clause 2.2: the total
        // power is the same in either channel).
        assert!(
            db(all / total).abs() < 0.15,
            "{guard:?}: {}",
            db(all / total)
        );
        let measured = db(data / tone);
        assert!(
            (measured - below).abs() < 0.3,
            "{guard:?}: {measured} dB below"
        );
        // With the 1800 Hz tone the data is about 1 dB lower than it
        // would be alone (clause 2.2).
        if guard == GuardTone::Hz1800 {
            assert!((db(data / total) + 1.0).abs() < 0.15);
        }

        let mut modulator = Modulator::new(Band::Low, level, guard, LAW);
        let samples = line(&mut modulator, 80_000, 7);
        let all = mean_square(&samples[800..]);
        assert!(db(all / total).abs() < 0.15);
        assert!(db(power_at(&samples, hz) / power_at(&samples, 1200.0)) < -40.0);
    }
}

#[test]
fn a_silent_modulator_sends_nothing_and_comes_back_without_a_step() {
    let mut modulator = Modulator::new(Band::High, -12.0, GuardTone::Hz1800, LAW);
    let mut samples = line(&mut modulator, 4000, 9);
    modulator.set_silent(true);
    assert!(modulator.silent());
    let mut asked = false;
    for _ in 0..4000 {
        samples.push(modulator.next_sample(|| {
            asked = true;
            true
        }));
    }
    // No bits are taken in silence.
    assert!(!asked);
    modulator.set_silent(false);
    samples.extend(line(&mut modulator, 4000, 9));
    // Once the filter has emptied there is nothing at all.
    assert!(samples[4400..7900].iter().all(|&s| s == 0.0));
    // The signal ends and begins no more steeply than it runs.
    let step = |range: std::ops::Range<usize>| {
        samples[range]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0f32, f32::max)
    };
    let running = step(1000..4000);
    assert!(step(3900..4500) <= running * 1.05);
    assert!(step(7900..8500) <= running * 1.05);
}

// The modulator into the demodulator.

#[test]
fn data_is_carried_whole_on_a_clean_line() {
    for rate in RATES {
        assert_carried("clean", &clean(), -12.0, rate, 20);
    }
}

#[test]
fn the_points_are_tight_on_a_clean_line() {
    for rate in RATES {
        let mut rig = Rig::new(&clean(), -12.0);
        rig.start(rate, SETTLE);
        rig.run(500);
        for end in [&rig.a, &rig.b] {
            // Every point is near a point of the grid: through one G.711
            // coding, the error is some 35 dB under the signal.
            let snr = end.demodulator.snr_db();
            assert!(snr > 30.0, "{rate:?}: {snr} dB");
            let worst = end
                .points
                .iter()
                .map(|p| {
                    let (quadrant, element) = quadrant_and_element(*p);
                    (*p - point(quadrant, element)).abs()
                })
                .fold(0f32, f32::max);
            assert!(worst < 0.5, "{rate:?}: a point {worst} of a unit out");
            // And the demodulator knows what it is receiving.
            let level = end.demodulator.level_dbm0();
            assert!((level + 12.0).abs() < 0.5, "{rate:?}: level {level}");
            assert!(end.demodulator.carrier_offset_hz().abs() < 0.2);
            assert!(end.demodulator.timing_offset_ppm().abs() < 20.0);
            assert!(end.demodulator.tracking());
        }
        if rate == Rate::B2400 {
            // All sixteen points are used.
            let mut used = [false; 16];
            for p in &rig.a.points {
                let (quadrant, element) = quadrant_and_element(*p);
                used[usize::from(quadrant * 4 + element)] = true;
            }
            assert!(used.iter().all(|&u| u));
        }
    }
}

#[test]
fn data_is_carried_at_every_level_from_minus_9_to_minus_43_dbm0() {
    for rate in RATES {
        for level in [-9.0f32, -15.0, -21.0, -27.0, -33.0, -39.0, -43.0] {
            // The line's loss makes the level at the receiver; the
            // modems send at -9 dBm0.
            let config = both_ways(Impairments {
                gain_db: level + 9.0,
                ..Impairments::default()
            });
            let mut rig = Rig::new(&config, -9.0);
            rig.start(rate, SETTLE);
            rig.run(1000);
            for (end, sent) in [(&rig.a, &rig.b.sent), (&rig.b, &rig.a.sent)] {
                let compared = compare(sent, &end.received)
                    .unwrap_or_else(|| panic!("{rate:?} at {level}: nothing received"));
                assert_eq!(compared.errors, 0, "{rate:?} at {level} dBm0: {compared:?}");
                let read = end.demodulator.level_dbm0();
                assert!(
                    (read - level).abs() < 1.0,
                    "{rate:?} at {level}: read {read}"
                );
            }
        }
    }
}

#[test]
fn data_is_carried_with_the_carrier_7_hz_off() {
    for rate in RATES {
        for offset in [-7.0f32, 7.0] {
            let config = both_ways(Impairments {
                frequency_offset_hz: offset,
                gain_db: -10.0,
                ..Impairments::default()
            });
            let mut rig = Rig::new(&config, -12.0);
            rig.start(rate, SETTLE);
            rig.run(2000);
            for (end, sent) in [(&rig.a, &rig.b.sent), (&rig.b, &rig.a.sent)] {
                let compared = compare(sent, &end.received).expect("nothing received");
                assert_eq!(compared.errors, 0, "{rate:?} at {offset} Hz: {compared:?}");
                // And the demodulator says what the offset is.
                let read = end.demodulator.carrier_offset_hz();
                assert!((read - offset).abs() < 0.3, "{rate:?}: read {read} Hz");
            }
        }
    }
}

#[test]
fn data_is_carried_with_the_clocks_200_parts_in_a_million_apart() {
    for rate in RATES {
        for ppm in [-200.0, 200.0] {
            let config = ChannelConfig {
                clock_offset_ppm: ppm,
                ..both_ways(Impairments {
                    gain_db: -10.0,
                    ..Impairments::default()
                })
            };
            // Two minutes: the symbols of one end move through the
            // other's by fourteen whole symbols.
            let [a, b] = carry(&config, -12.0, rate, 120);
            assert_eq!(a.errors, 0, "{rate:?} at {ppm} ppm, end A: {a:?}");
            assert_eq!(b.errors, 0, "{rate:?} at {ppm} ppm, end B: {b:?}");
        }
    }
}

#[test]
fn data_is_carried_under_the_echo_of_a_poor_hybrid() {
    // This end's own transmission comes back only 6 dB down, with the far
    // end 20 dB down the line: the echo is 14 dB louder than the signal.
    // At 1200 bit/s also with the far end 30 dB down, the echo 24 dB
    // louder.
    //
    // The echo is in the other channel and the filters take it out, 77 dB
    // of it. What limits this is the coding: the echo joins the signal
    // before the receiver's G.711 coder, whose steps are then the size
    // the echo calls for, and their noise is in every channel. An echo
    // 24 dB louder leaves the signal 18 dB over that noise, which four
    // points bear and sixteen do not.
    for (rate, gain) in [
        (Rate::B1200, -20.0),
        (Rate::B1200, -30.0),
        (Rate::B2400, -20.0),
    ] {
        let config = both_ways(Impairments {
            gain_db: gain,
            echo: Some(Echo {
                delay_samples: 80,
                loss_db: 6.0,
            }),
            ..Impairments::default()
        });
        assert_carried("echo", &config, -12.0, rate, 60);
    }
}

#[test]
fn data_is_carried_through_a_loop_with_a_slope() {
    for rate in RATES {
        for slope in [3.0f32, 7.0] {
            let config = both_ways(Impairments {
                gain_db: -10.0,
                loop_slope_db: Some(slope),
                ..Impairments::default()
            });
            assert_carried("slope", &config, -12.0, rate, 20);
        }
    }
}

#[test]
fn data_is_carried_in_both_laws_and_through_a_second_coding() {
    for rate in RATES {
        for (law_a, law_b, second) in [
            (Law::Ulaw, Law::Ulaw, None),
            (Law::Alaw, Law::Alaw, None),
            (Law::Ulaw, Law::Alaw, None),
            (Law::Ulaw, Law::Ulaw, Some(Law::Alaw)),
            (Law::Alaw, Law::Alaw, Some(Law::Ulaw)),
        ] {
            let config = ChannelConfig {
                law_a,
                law_b,
                ..both_ways(Impairments {
                    gain_db: -10.0,
                    second_codec: second,
                    ..Impairments::default()
                })
            };
            assert_carried("laws", &config, -12.0, rate, 20);
        }
    }
}

#[test]
fn data_is_carried_with_everything_at_once() {
    // A long loop with a slope, a second coding in the other law, the
    // carrier and the clock both off, an echo, the guard tone, and noise
    // 30 dB under the signal.
    for rate in RATES {
        let impairments = Impairments {
            gain_db: -18.0,
            loop_slope_db: Some(5.0),
            second_codec: Some(Law::Alaw),
            frequency_offset_hz: 5.0,
            noise: Some(Noise::Snr {
                signal_dbm0: -30.0,
                snr_db: 30.0,
            }),
            echo: Some(Echo {
                delay_samples: 160,
                loss_db: 10.0,
            }),
            ..Impairments::default()
        };
        let config = ChannelConfig {
            clock_offset_ppm: 150.0,
            ..both_ways(impairments)
        };
        let mut rig = Rig::with_guard(&config, -12.0, GuardTone::Hz1800);
        rig.start(rate, SETTLE);
        rig.run(3000);
        for (end, sent) in [(&rig.a, &rig.b.sent), (&rig.b, &rig.a.sent)] {
            let compared = compare(sent, &end.received).expect("nothing received");
            assert_eq!(compared.errors, 0, "{rate:?}: {compared:?}");
        }
    }
}

// The line signal detector.

#[test]
fn the_line_signal_detector_has_the_thresholds_and_times_of_clause_3() {
    for band in [Band::Low, Band::High] {
        // Clause 3.3: on above -43 dBm, off below -48 dBm.
        for (level, expected) in [
            (CARRIER_ON_DBM0 + 1.0, true),
            (CARRIER_OFF_DBM0 - 1.0, false),
        ] {
            let mut modulator = Modulator::new(band, level, GuardTone::Off, LAW);
            let mut demodulator = v22::Demodulator::new(band, LAW);
            let mut carrier = false;
            let mut rng = chansim::Rng::new(11);
            for _ in 0..16_000 {
                let sample = modulator.next_sample(|| rng.next_u64() & 1 == 1);
                carrier = demodulator.process(LAW.decode(LAW.encode(sample))).carrier;
            }
            assert_eq!(carrier, expected, "{band:?} at {level} dBm0");
        }

        // Clause 3.2: off 40 to 65 ms after the signal falls below the
        // threshold, from the loudest signal and from one just over it.
        for level in [-9.0f32, -25.0, -41.0] {
            let mut modulator = Modulator::new(band, level, GuardTone::Off, LAW);
            let mut demodulator = v22::Demodulator::new(band, LAW);
            let mut rng = chansim::Rng::new(13);
            let mut came = None;
            let mut went = None;
            for n in 0..16_000u32 {
                // A second of signal, cut off at once.
                let sample = if n < 8000 {
                    modulator.next_sample(|| rng.next_u64() & 1 == 1)
                } else {
                    0.0
                };
                let carrier = demodulator.process(LAW.decode(LAW.encode(sample))).carrier;
                if carrier && came.is_none() {
                    came = Some(n);
                }
                if !carrier && came.is_some() && went.is_none() {
                    went = Some(n);
                }
            }
            // It comes 40 to 205 ms after the signal does.
            let came = f64::from(came.expect("the carrier never came")) / 8.0;
            assert!(
                (40.0..=205.0).contains(&came),
                "{band:?} at {level} dBm0: on after {came} ms"
            );
            let went = went.expect("the carrier never went") - 8000;
            let ms = f64::from(went) / 8.0;
            assert!(
                (40.0..=65.0).contains(&ms),
                "{band:?} at {level} dBm0: off after {ms} ms"
            );
        }
    }
}

/// A tone at a level, with its phase reversed every 450 ms as the answer
/// tone's is (V.25 clause 2.3), and off for the last sixth of every three
/// seconds: every step a tone of a call makes.
fn stepped_tone(hz: f64, level_dbm0: f32, n: u32) -> f32 {
    let amplitude = f64::from(dbm0_to_amplitude(level_dbm0, LAW));
    let sign = if (n / 3600).is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    let on = if n % 24_000 < 20_000 { 1.0 } else { 0.0 };
    (on * sign * amplitude * (TAU * hz * f64::from(n) / FS).sin()) as f32
}

#[test]
fn the_other_channel_and_the_tones_of_a_call_are_not_a_carrier() {
    // This end's own transmission is not the far end's, however loud.
    for band in [Band::Low, Band::High] {
        let mut demodulator = v22::Demodulator::new(band, LAW);
        let mut own = Modulator::new(band.other(), -9.0, GuardTone::Hz1800, LAW);
        let mut rng = chansim::Rng::new(17);
        for _ in 0..16_000 {
            let sample = own.next_sample(|| rng.next_u64() & 1 == 1);
            let out = demodulator.process(LAW.decode(LAW.encode(sample)));
            assert!(!out.carrier && out.symbol.is_none(), "{band:?}: own signal");
        }
    }
    // Clause 3.3: circuit 109 does not respond to the guard tones or to
    // the answer tone. The tones that are outside a channel are not a
    // carrier in it, with every step they make: the answer tone and the
    // guard tones in the low channel, the guard tones and the calling
    // tone in the high one.
    for (band, hz, level) in [
        (Band::Low, 2100.0, -9.0),
        (Band::Low, 1800.0, -15.0),
        (Band::Low, 550.0, -12.0),
        (Band::High, 1800.0, -15.0),
        (Band::High, 550.0, -12.0),
        (Band::High, 1300.0, -9.0),
    ] {
        let mut demodulator = v22::Demodulator::new(band, LAW);
        for n in 0..48_000 {
            let sample = LAW.decode(LAW.encode(stepped_tone(hz, level, n)));
            assert!(!demodulator.process(sample).carrier, "{band:?}: {hz} Hz");
        }
    }
    // The answer tone is inside the high channel, 300 Hz under its
    // carrier, and there the detector cannot but hear it. What is decided
    // from it is not the unscrambled ones a call begins with: a tone
    // 300 Hz under the carrier is half a turn a symbol, which is the
    // dibit 10 and never 11. The handshake tells them apart by that.
    let mut demodulator = v22::Demodulator::new(Band::High, LAW);
    let mut dibits = [0u32; 4];
    for n in 0..16_000 {
        let amplitude = f64::from(dbm0_to_amplitude(-12.0, LAW));
        let sample = (amplitude * (TAU * 2100.0 * f64::from(n) / FS).sin()) as f32;
        if let Some(symbol) = demodulator.process(sample).symbol {
            dibits[usize::from(symbol.line)] += 1;
        }
    }
    assert!(dibits[0b10] > 1000, "{dibits:?}");
    assert!(dibits[0b11] < 10, "{dibits:?}");
    // The idle octets of a lost packet are nothing.
    let mut demodulator = v22::Demodulator::new(Band::Low, LAW);
    for octet in [0xFFu8, 0x7F, 0xD5] {
        for _ in 0..8000 {
            assert!(!demodulator.process(LAW.decode(octet)).carrier);
        }
    }
}

#[test]
fn a_signal_that_goes_and_comes_back_is_taken_up_where_it_was_left() {
    // Clause 6.5. The far end is silent for 300 ms in the middle of the
    // data and then goes on: the data after is whole, without the loops
    // being found again.
    for rate in RATES {
        let mut rig = Rig::new(&clean(), -12.0);
        rig.start(rate, SETTLE);
        rig.run(300);
        rig.a.modulator.set_silent(true);
        rig.run(30);
        rig.a.modulator.set_silent(false);
        rig.run(20);
        // What end B received from 200 ms after the signal came back.
        rig.b.received.clear();
        rig.run(500);
        let compared = compare(&rig.a.sent, &rig.b.received).expect("nothing received");
        assert_eq!(compared.errors, 0, "{rate:?}: {compared:?}");
        // The detector went and came once each.
        let changes: Vec<bool> = rig.b.carrier_changes.iter().map(|c| c.1).collect();
        assert_eq!(changes, [true, false, true], "{rate:?}");
        // The other direction was not disturbed at all.
        let other = compare(&rig.b.sent, &rig.a.received).expect("nothing received");
        assert_eq!(other.errors, 0);
    }
}

#[test]
fn a_signal_that_becomes_much_quieter_or_louder_is_followed() {
    // The line's loss changes by 15 dB at once, in the middle of the
    // data, and later back. More than 10 dB down is at first taken for a
    // signal that is going, and nothing is learned from it; when it has
    // stayed for a tenth of a second it is the signal's level, and the
    // gain goes to it.
    for rate in RATES {
        let mut modulator = Modulator::new(Band::Low, -12.0, GuardTone::Off, LAW);
        let mut demodulator = v22::Demodulator::new(Band::Low, LAW);
        let mut rng = chansim::Rng::new(19);
        let mut scrambler =
            dspkit::scramble::Scrambler::with_guard(v22::SCRAMBLER, v22::SCRAMBLER_GUARD_ONES);
        let mut sent = Vec::new();
        let mut received = Vec::new();
        let quiet = 10f32.powf(-15.0 / 20.0);
        let mut levels = Vec::new();
        for n in 0..240_000u32 {
            if n == 12_000 {
                modulator.set_rate(rate);
                demodulator.set_rate(rate);
            }
            let gain = if (80_000..160_000).contains(&n) {
                quiet
            } else {
                1.0
            };
            let sample = modulator.next_sample(|| {
                let bit = rng.next_u64() & 1 == 1;
                sent.push(scrambler.bit(bit));
                bit
            });
            let out = demodulator.process(LAW.decode(LAW.encode(sample * gain)));
            assert!(out.carrier || n < 800, "the carrier went at sample {n}");
            // What is received from a second after each change of level
            // to the next.
            if let Some(symbol) = out.symbol
                && n % 80_000 >= 8000
                && n >= 24_000
            {
                received.push((n / 80_000, symbol));
            }
            if n % 80_000 == 79_999 {
                levels.push(demodulator.level_dbm0());
            }
        }
        for stretch in 0..3 {
            let bits: Vec<bool> = received
                .iter()
                .filter(|(at, _)| *at == stretch)
                .flat_map(|(_, symbol)| symbol.line_bits())
                .collect();
            let compared = compare(&sent, &bits).expect("nothing received");
            assert_eq!(
                compared.errors, 0,
                "{rate:?}, stretch {stretch}: {compared:?}"
            );
            assert!(compared.bits > 6 * rate.bits_per_second() as usize);
        }
        for (level, expected) in levels.iter().zip([-12.0, -27.0, -12.0]) {
            assert!(
                (level - expected).abs() < 0.5,
                "{rate:?}: levels {levels:?}"
            );
        }
    }
}

// The bit error rate.

/// Q(x): the chance that a normal variable is more than x deviations
/// above its mean, by Simpson's rule out to x + 12.
fn q(x: f64) -> f64 {
    let steps = 4000;
    let width = 12.0 / f64::from(steps);
    let density = |t: f64| (-t * t / 2.0).exp() / TAU.sqrt();
    let mut sum = density(x) + density(x + 12.0);
    for n in 1..steps {
        let weight = if n % 2 == 1 { 4.0 } else { 2.0 };
        sum += weight * density(x + f64::from(n) * width);
    }
    sum * width / 3.0
}

/// The bit error rate of the line's bits by theory, for coherent
/// detection, at a ratio of energy per bit to noise density.
///
/// A symbol is taken for a neighbour with the chance Q(d / 2 sigma), d
/// being the distance between the two. What that costs in bits is counted
/// from the coding: the last two bits by Figure 2, and for a neighbour in
/// another quadrant one bit of this symbol's first two and one of the
/// next symbol's, the quadrants being sent as changes (Table 1, in which
/// neighbouring changes differ in one bit).
fn theory(rate: Rate, eb_n0_db: f64) -> f64 {
    let eb_n0 = 10f64.powf(eb_n0_db / 10.0);
    match rate {
        Rate::B1200 => {
            // Four points: two neighbours each, both in another quadrant,
            // at a distance of the root of twice the symbol's energy.
            // Es = 2 Eb, and the noise in each dimension is N0 / 2.
            let neighbour = q((2.0 * eb_n0).sqrt());
            // Two neighbours, two bits wrong for each, two bits a symbol.
            2.0 * neighbour * 2.0 / 2.0
        }
        Rate::B2400 => {
            // Sixteen points two units apart, with a mean energy of ten
            // square units: Es = 4 Eb, so a unit squared is 0.4 Eb.
            let neighbour = q((0.8 * eb_n0).sqrt());
            let mut bits = 0u32;
            for row in 0..4i32 {
                for column in 0..4i32 {
                    for (dr, dc) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
                        let (r, c) = (row + dr, column + dc);
                        if !(0..4).contains(&r) || !(0..4).contains(&c) {
                            continue;
                        }
                        let here = FIGURE_2[row as usize][column as usize];
                        let there = FIGURE_2[r as usize][c as usize];
                        bits += (here ^ there).count_ones();
                        let crosses = (row < 2) != (r < 2) || (column < 2) != (c < 2);
                        if crosses {
                            bits += 2;
                        }
                    }
                }
            }
            // The mean over the sixteen points, four bits a symbol.
            neighbour * f64::from(bits) / 16.0 / 4.0
        }
    }
}

/// The Eb/N0 at which theory gives a bit error rate.
fn theory_eb_n0_db(rate: Rate, bit_error_rate: f64) -> f64 {
    let (mut low, mut high) = (0.0, 30.0);
    for _ in 0..60 {
        let middle = (low + high) / 2.0;
        if theory(rate, middle) > bit_error_rate {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.0
}

/// The bit error rate of the line's bits, both ways, at a ratio of
/// energy per bit to noise density.
fn bit_error_rate(rate: Rate, eb_n0_db: f64, seconds: u64, seed: u64) -> f64 {
    let level = -20.0f32;
    // The noise is white over the 4000 Hz to half the sample rate, and a
    // bit lasts one over the bit rate: Eb/N0 is the signal-to-noise ratio
    // times 4000 over the bit rate.
    let snr_db = eb_n0_db - db(4000.0 / f64::from(rate.bits_per_second()));
    let config = ChannelConfig {
        seed,
        ..both_ways(Impairments {
            noise: Some(Noise::Snr {
                signal_dbm0: level,
                snr_db: snr_db as f32,
            }),
            ..Impairments::default()
        })
    };
    let mut rig = Rig::new(&config, level);
    rig.start(rate, SETTLE);
    rig.run(seconds * 100);
    let a = compare(&rig.b.sent_line, &rig.a.received_line).expect("end A lost the signal");
    let b = compare(&rig.a.sent_line, &rig.b.received_line).expect("end B lost the signal");
    (a.errors + b.errors) as f64 / (a.bits + b.bits) as f64
}

/// The most the demodulator may need over theory, in dB of Eb/N0, for
/// the same bit error rate: its implementation loss.
const LOSS_DB: [(Rate, f64); 2] = [(Rate::B1200, 0.5), (Rate::B2400, 0.5)];

#[test]
fn the_bit_error_rate_is_within_a_stated_loss_of_theory() {
    // With no noise there are no errors at all.
    for rate in RATES {
        assert_eq!(bit_error_rate(rate, 60.0, 20, 1), 0.0);
    }
    println!("rate    Eb/N0   BER        theory     theory at    loss");
    for (rate, allowed) in LOSS_DB {
        let points: [f64; 3] = match rate {
            Rate::B1200 => [5.0, 7.0, 9.0],
            Rate::B2400 => [9.0, 11.0, 13.0],
        };
        for eb_n0 in points {
            let ber = bit_error_rate(rate, eb_n0, 600, 21);
            let at = theory_eb_n0_db(rate, ber);
            let loss = eb_n0 - at;
            println!(
                "{:<6}  {eb_n0:>4.1}   {ber:.3e}  {:.3e}  {at:>6.2} dB   {loss:+.2} dB",
                format!("{rate:?}"),
                theory(rate, eb_n0),
            );
            assert!(loss < allowed, "{rate:?} at {eb_n0} dB: loss {loss:.2} dB");
            // Nothing does better than theory by more than the theory is
            // rough: it counts nearest neighbours only. If it seems to,
            // the noise is not at the level the test thinks.
            assert!(
                loss > -0.3,
                "{rate:?} at {eb_n0} dB: {loss:.2} dB better than theory"
            );
        }
    }
}
