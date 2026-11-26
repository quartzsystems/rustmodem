// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Each impairment measured: the noise is at the level stated, the offset
//! is so many hertz, the clocks differ by so many parts in a million.

use chansim::{
    Channel, ChannelConfig, Duplex, Echo, Impairments, Noise, Packets, Rng, Side, SlipKind, Slips,
};
use dspkit::level::{dbm0_to_amplitude, power_to_dbm0};
use engine::{EchoEngine, ModemEvent, NullConfig, NullEngine};
use g711::Law;

const FS: f64 = 8000.0;
const TAU: f64 = std::f64::consts::TAU;

/// The frequency levels are measured at. Not 1000 Hz: that is an eighth of
/// the sample rate, so a 1000 Hz tone is the same eight samples over and
/// over, G.711 codes them the same way every time, and the error does not
/// average out. It is why the test tone of the telephone network is
/// 1004 Hz.
const TEST_HZ: f64 = 1004.0;

/// How far a level may be from the one stated, in dB, after a signal has
/// been through G.711 two or three times. One coding moves the level of a
/// sine wave by up to 0.04 dB, and by 0.1 dB at the bottom of the range,
/// according to where its peaks fall among the steps of the law.
const CODING_DB: f32 = 0.2;

/// A sine wave at a level in dBm0, as octets.
fn tone(frequency: f64, level: f32, law: Law, len: usize) -> Vec<u8> {
    let amplitude = f64::from(dbm0_to_amplitude(level, law));
    (0..len)
        .map(|n| law.encode((amplitude * (TAU * frequency * n as f64 / FS).sin()) as f32))
        .collect()
}

/// Sends octets from A and gives what B receives, tick by tick, with B
/// silent.
fn through(config: &ChannelConfig, sent: &[u8]) -> Vec<u8> {
    let mut channel = Channel::new(config);
    let silence = [config.law_b.silence(); 80];
    let mut received = Vec::new();
    for chunk in sent.chunks(80) {
        let mut a_rx = [0u8; 80];
        let mut b_rx = [0u8; 80];
        channel.tick(chunk, &silence[..chunk.len()], &mut a_rx, &mut b_rx);
        received.extend_from_slice(&b_rx[..chunk.len()]);
    }
    received
}

fn decode(octets: &[u8], law: Law) -> Vec<f32> {
    octets.iter().map(|&octet| law.decode(octet)).collect()
}

/// The amplitude of the sine wave of a frequency in some samples.
fn amplitude(samples: &[f32], frequency: f64) -> f64 {
    let (mut s, mut c) = (0.0, 0.0);
    for (n, &x) in samples.iter().enumerate() {
        let (sin, cos) = (TAU * frequency * n as f64 / FS).sin_cos();
        s += f64::from(x) * sin;
        c += f64::from(x) * cos;
    }
    2.0 * s.hypot(c) / samples.len() as f64
}

fn level_dbm0(samples: &[f32], law: Law) -> f32 {
    let power = samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / samples.len() as f64;
    power_to_dbm0(power as f32, law)
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

fn one_way(impairments: Impairments) -> ChannelConfig {
    ChannelConfig {
        a_to_b: impairments,
        ..ChannelConfig::default()
    }
}

#[test]
fn a_clean_channel_alters_no_octet() {
    for law in [Law::Ulaw, Law::Alaw] {
        let config = ChannelConfig {
            law_a: law,
            law_b: law,
            ..ChannelConfig::default()
        };
        // Every octet there is, and then some at random.
        let mut rng = Rng::new(5);
        let mut sent: Vec<u8> = (0..=255).collect();
        sent.extend((0..7744).map(|_| rng.next_u64() as u8));
        let received = through(&config, &sent);
        // One tick late, which is the delay, behind a tick of silence.
        assert!(received[..80].iter().all(|&octet| octet == law.silence()));
        assert_eq!(&received[80..], &sent[..sent.len() - 80], "{law:?}");
    }
}

#[test]
fn the_two_ends_may_have_different_laws() {
    let config = ChannelConfig {
        law_a: Law::Ulaw,
        law_b: Law::Alaw,
        ..ChannelConfig::default()
    };
    let sent = tone(TEST_HZ, -12.0, Law::Ulaw, 8000);
    let received = through(&config, &sent);
    for (n, &octet) in received.iter().enumerate().skip(80) {
        assert_eq!(octet, Law::Alaw.encode(Law::Ulaw.decode(sent[n - 80])));
    }
    // The level is the same in the receiver's law, to the 0.03 dB the two
    // overload points differ by and the coding.
    let level = level_dbm0(&decode(&received[800..], Law::Alaw), Law::Alaw);
    assert!((level + 12.0).abs() < CODING_DB, "{level}");
}

#[test]
fn gain_and_loss_are_what_they_say() {
    for gain in [-30.0f32, -10.0, -3.0, 6.0] {
        let config = one_way(Impairments {
            gain_db: gain,
            ..Impairments::default()
        });
        let received = decode(
            &through(&config, &tone(TEST_HZ, -15.0, Law::Ulaw, 16_080)),
            Law::Ulaw,
        );
        let level = level_dbm0(&received[80..], Law::Ulaw);
        assert!(
            (level - (-15.0 + gain)).abs() < CODING_DB,
            "{gain}: {level}"
        );
    }
}

#[test]
fn a_digital_pad_attenuates_and_codes_again() {
    let config = one_way(Impairments {
        pad_db: 6.0,
        ..Impairments::default()
    });
    let sent = tone(TEST_HZ, -12.0, Law::Ulaw, 16_080);
    let received = through(&config, &sent);
    let level = level_dbm0(&decode(&received[80..], Law::Ulaw), Law::Ulaw);
    assert!((level + 18.0).abs() < CODING_DB, "{level}");
    // Octet for octet it is the decoded sample, scaled and coded.
    let factor = 10f32.powf(-6.0 / 20.0);
    let mut exact = 0;
    for (n, &octet) in received.iter().enumerate().skip(80) {
        let expected = Law::Ulaw.encode(Law::Ulaw.decode(sent[n - 80]) * factor);
        // The factor here is the library's power, and the channel's is its
        // own: on a decision value they may differ by one step.
        assert!(octet.abs_diff(expected) <= 1, "{n}");
        if octet == expected {
            exact += 1;
        }
    }
    assert!(exact > 15_900, "{exact}");
}

#[test]
fn a_second_codec_pass_adds_its_quantizing_noise() {
    let sent = tone(TEST_HZ, -12.0, Law::Ulaw, 16_080);
    let config = one_way(Impairments {
        second_codec: Some(Law::Alaw),
        ..Impairments::default()
    });
    let received = through(&config, &sent);
    let mut changed = 0;
    for (n, &octet) in received.iter().enumerate().skip(80) {
        let middle = Law::Alaw.decode(Law::Alaw.encode(Law::Ulaw.decode(sent[n - 80])));
        assert_eq!(octet, Law::Ulaw.encode(middle), "{n}");
        if octet != sent[n - 80] {
            changed += 1;
        }
    }
    // The A-law's steps are not the u-law's, so some octets move.
    assert!(changed > 50, "{changed}");
    // The same law a second time changes nothing: it is already coded.
    let config = one_way(Impairments {
        second_codec: Some(Law::Ulaw),
        ..Impairments::default()
    });
    let received = through(&config, &sent);
    assert_eq!(&received[80..], &sent[..sent.len() - 80]);
}

#[test]
fn the_noise_is_at_the_level_stated_and_is_white() {
    for level in [-20.0f32, -35.0, -50.0] {
        let config = one_way(Impairments {
            noise: Some(Noise::Level(level)),
            ..Impairments::default()
        });
        // A minute of silence from A.
        let received = decode(&through(&config, &vec![0xFF; 480_000]), Law::Ulaw);
        let measured = level_dbm0(&received[80..], Law::Ulaw);
        // G.711 is coarse at the bottom of its range, which adds a little
        // to very quiet noise.
        let allowed = if level < -45.0 { 0.3 } else { 0.1 };
        assert!((measured - level).abs() < allowed, "{level}: {measured}");
    }
    // White: the same power in each eighth of the band. Measured on the
    // noise at -20 dBm0 by the power of each band's worth of tones.
    let config = one_way(Impairments {
        noise: Some(Noise::Level(-20.0)),
        ..Impairments::default()
    });
    let received = decode(&through(&config, &vec![0xFF; 160_080]), Law::Ulaw);
    let mut bands = [0f64; 8];
    for block in received[80..].as_chunks::<800>().0 {
        // Bins 10 Hz apart.
        for bin in 1..400 {
            let a = amplitude(block, f64::from(bin) * 10.0);
            bands[(bin / 50) as usize] += a * a;
        }
    }
    let mean = bands.iter().sum::<f64>() / 8.0;
    for (n, band) in bands.iter().enumerate() {
        assert!((10.0 * (band / mean).log10()).abs() < 0.5, "band {n}");
    }
}

#[test]
fn a_signal_to_noise_ratio_is_that_ratio() {
    let noise = Noise::Snr {
        signal_dbm0: -15.0,
        snr_db: 12.0,
    };
    assert_eq!(noise.level_dbm0(), -27.0);
    let config = one_way(Impairments {
        noise: Some(noise),
        ..Impairments::default()
    });
    let received = decode(
        &through(&config, &tone(TEST_HZ, -15.0, Law::Ulaw, 160_080)),
        Law::Ulaw,
    );
    let received = &received[80..];
    let signal = amplitude(received, TEST_HZ);
    let signal_power = signal * signal / 2.0;
    let total = received.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / received.len() as f64;
    let snr = 10.0 * (signal_power / (total - signal_power)).log10();
    assert!((snr - 12.0).abs() < 0.15, "{snr}");
}

#[test]
fn the_noise_comes_from_the_seed() {
    let run = |seed: u64| {
        let config = ChannelConfig {
            seed,
            ..ChannelConfig::symmetric(Impairments {
                noise: Some(Noise::Level(-30.0)),
                ..Impairments::default()
            })
        };
        let mut channel = Channel::new(&config);
        let (mut a_rx, mut b_rx) = ([0u8; 80], [0u8; 80]);
        let mut all = Vec::new();
        for _ in 0..50 {
            channel.tick(&[0xFF; 80], &[0xFF; 80], &mut a_rx, &mut b_rx);
            all.extend_from_slice(&a_rx);
            all.extend_from_slice(&b_rx);
        }
        (all, a_rx, b_rx)
    };
    let (first, a_rx, b_rx) = run(1);
    assert_eq!(first, run(1).0);
    assert_ne!(first, run(2).0);
    // The two directions do not have the same noise.
    assert_ne!(a_rx, b_rx);
}

#[test]
fn the_loop_filter_keeps_the_voice_band_and_slopes_across_it() {
    let gain_at = |slope: f32, frequency: f64| {
        let config = one_way(Impairments {
            loop_slope_db: Some(slope),
            ..Impairments::default()
        });
        // What arrives over what was sent, both as G.711 carries them,
        // and a little off the round frequency, for the reason TEST_HZ is.
        let frequency = frequency * 1.004;
        let sent = tone(frequency, -15.0, Law::Ulaw, 16_800);
        let received = decode(&through(&config, &sent), Law::Ulaw);
        let sent = decode(&sent, Law::Ulaw);
        db(amplitude(&received[800..], frequency) / amplitude(&sent[800..], frequency))
    };
    for slope in [0.0f32, 3.0, 6.0, 7.0] {
        // One at 1000 Hz, and the slope further down at 3000 Hz. The
        // band's upper corner takes a little more off at 3000 Hz.
        let at_1000 = gain_at(slope, 1000.0);
        let at_3000 = gain_at(slope, 3000.0);
        assert!(at_1000.abs() < 0.1, "{slope}: {at_1000}");
        let band_alone = gain_at(0.0, 3000.0);
        assert!(
            (at_1000 - at_3000 - f64::from(slope) + band_alone).abs() < 0.1,
            "{slope}: {at_3000} with the band at {band_alone}"
        );
    }
    // The band: about 3 dB down at its corners, and well down outside.
    assert!((gain_at(0.0, 180.0) + 3.0).abs() < 0.4);
    assert!((gain_at(0.0, 3500.0) + 3.0).abs() < 0.4);
    assert!(gain_at(0.0, 50.0) < -20.0);
    assert!(gain_at(0.0, 300.0) > -0.6);
    assert!(gain_at(0.0, 3000.0) > -1.3);
}

#[test]
fn the_frequency_offset_is_so_many_hertz() {
    for offset in [7.0f32, -7.0, 3.0] {
        let config = one_way(Impairments {
            frequency_offset_hz: offset,
            ..Impairments::default()
        });
        let stated = f64::from(dbm0_to_amplitude(-12.0, Law::Ulaw));
        for frequency in [980.0, 1750.0, 2100.0] {
            let received = decode(
                &through(&config, &tone(frequency, -12.0, Law::Ulaw, 33_000)),
                Law::Ulaw,
            );
            // Four seconds, so that a hertz is told from the next.
            let window = &received[1000..];
            let moved = amplitude(window, frequency + f64::from(offset));
            assert!((moved / stated - 1.0).abs() < 0.01, "{frequency} {offset}");
            for wrong in [0.0, -f64::from(offset), f64::from(offset) + 1.0] {
                let other = amplitude(window, frequency + wrong);
                assert!(db(other / stated) < -40.0, "{frequency} {offset} {wrong}");
            }
        }
    }
}

#[test]
fn the_clock_offset_is_so_many_parts_in_a_million() {
    for ppm in [200.0, -200.0, 1000.0] {
        let config = ChannelConfig {
            clock_offset_ppm: ppm,
            ..ChannelConfig::default()
        };
        let mut channel = Channel::new(&config);
        assert_eq!(channel.clock_offset_ppm(), ppm);
        // A sends a million of its samples: B's clock, slower by the
        // offset, has that many fewer in the same time.
        let sent = tone(2000.0, -12.0, Law::Ulaw, 1_000_000);
        let before = channel.stats(Side::B).queued;
        for chunk in sent.chunks(80) {
            channel.send(Side::A, chunk);
        }
        let made = (channel.stats(Side::B).queued - before) as f64;
        let expected = 1_000_000.0 / (1.0 + ppm * 1.0e-6);
        assert!(
            (made - expected).abs() <= 1.0,
            "{ppm}: {made} against {expected}"
        );

        // And A's tone, which is 2000 Hz by A's clock, is higher by the
        // offset on B's.
        let mut received = vec![0u8; 400_000];
        for chunk in received.chunks_mut(80) {
            channel.receive(Side::B, chunk);
        }
        assert_eq!(channel.stats(Side::B).underruns, 0);
        let received = decode(&received[8000..], Law::Ulaw);
        let stated = f64::from(dbm0_to_amplitude(-12.0, Law::Ulaw));
        let right = amplitude(&received, 2000.0 * (1.0 + ppm * 1.0e-6));
        let wrong = amplitude(&received, 2000.0);
        let opposite = amplitude(&received, 2000.0 * (1.0 - ppm * 1.0e-6));
        assert!((right / stated - 1.0).abs() < 0.01, "{ppm}: {right}");
        assert!(wrong < 0.2 * stated && opposite < 0.2 * stated, "{ppm}");
    }
}

#[test]
fn a_slip_drops_or_repeats_one_sample() {
    // A ramp of octets, so that every sample is known by its value.
    let sent: Vec<u8> = (0..4000u32).map(|n| 0x80 + (n % 100) as u8).collect();
    for kind in [SlipKind::Drop, SlipKind::Repeat] {
        // A dropped sample is one more taken from the path than a tick
        // puts in, so the path needs more than a tick of delay to give it.
        // The assertions below are written for the delay of one tick, and
        // the ramp repeats every 100 samples, which is the other 100 here.
        let config = ChannelConfig {
            delay_samples: 180,
            ..one_way(Impairments {
                slips: Some(Slips {
                    every_samples: 1000,
                    kind,
                }),
                ..Impairments::default()
            })
        };
        let received = through(&config, &sent);
        // The first slip is at the receiver's sample 999, which without it
        // would be the sender's 919.
        assert_eq!(received[998], sent[918]);
        match kind {
            SlipKind::Drop => {
                assert_eq!(received[999], sent[920]);
                assert_eq!(received[1500], sent[1421]);
                // After the second, two samples early.
                assert_eq!(received[2500], sent[2422]);
            }
            SlipKind::Repeat => {
                assert_eq!(received[999], sent[918]);
                assert_eq!(received[1000], sent[919]);
                assert_eq!(received[1500], sent[1419]);
                assert_eq!(received[2500], sent[2418]);
            }
        }
        let mut channel = Channel::new(&config);
        let mut rx = [0u8; 80];
        for chunk in sent.chunks(80) {
            channel.send(Side::A, chunk);
            channel.receive(Side::B, &mut rx);
        }
        assert_eq!(channel.stats(Side::B).slips, 4);
        // Nothing slips the other way.
        assert_eq!(channel.stats(Side::A).slips, 0);
    }
}

#[test]
fn a_lost_packet_is_a_packet_of_the_idle_octet() {
    for (law, idle) in [(Law::Ulaw, 0xFFu8), (Law::Alaw, 0xD5)] {
        for ptime in [10u32, 20, 30] {
            let config = ChannelConfig {
                law_a: law,
                law_b: law,
                a_to_b: Impairments {
                    packets: Some(Packets {
                        ptime_ms: ptime,
                        lost: vec![3, 4, 20],
                        loss_probability: 0.0,
                    }),
                    ..Impairments::default()
                },
                ..ChannelConfig::default()
            };
            let sent = vec![0x33u8; 8000];
            let received = through(&config, &sent);
            let packet = (ptime * 8) as usize;
            for (n, &octet) in received.iter().enumerate().skip(80) {
                let lost = [3, 4, 20].contains(&(n / packet));
                assert_eq!(octet, if lost { idle } else { 0x33 }, "{law:?} {ptime} {n}");
            }
        }
    }
    // By chance: the share stated, and the same packets from the same seed.
    let config = ChannelConfig {
        // No delay, so that each packet sent is the packet received.
        delay_samples: 0,
        ..one_way(Impairments {
            packets: Some(Packets {
                ptime_ms: 20,
                lost: Vec::new(),
                loss_probability: 0.05,
            }),
            ..Impairments::default()
        })
    };
    let mut channel = Channel::new(&config);
    let mut rx = [0u8; 160];
    let mut lost = 0;
    for _ in 0..20_000 {
        channel.send(Side::A, &[0x33; 160]);
        channel.receive(Side::B, &mut rx);
        // A packet is lost whole or not at all.
        assert!(rx.iter().all(|&o| o == 0xFF) || rx.iter().all(|&o| o == 0x33));
        if rx[0] == 0xFF {
            lost += 1;
        }
    }
    let stats = channel.stats(Side::B);
    assert_eq!(stats.packets, 20_000);
    assert_eq!(stats.packets_lost, lost);
    assert!((900..1100).contains(&lost), "{lost}");
    assert_eq!(
        through(&config, &[0x33; 8000]),
        through(&config, &[0x33; 8000])
    );
}

#[test]
fn the_echo_is_the_ends_own_signal_late_and_quieter() {
    let config = ChannelConfig {
        b_to_a: Impairments {
            echo: Some(Echo {
                delay_samples: 200,
                loss_db: 15.0,
            }),
            ..Impairments::default()
        },
        ..ChannelConfig::default()
    };
    // A sends a tone and B is silent: what A receives is its own echo.
    let sent = tone(TEST_HZ, -12.0, Law::Ulaw, 16_000);
    let mut channel = Channel::new(&config);
    let mut a_received = Vec::new();
    let mut b_received = Vec::new();
    for chunk in sent.chunks(80) {
        let (mut a_rx, mut b_rx) = ([0u8; 80], [0u8; 80]);
        channel.tick(chunk, &[0xFF; 80], &mut a_rx, &mut b_rx);
        a_received.extend_from_slice(&a_rx);
        b_received.extend_from_slice(&b_rx);
    }
    let echo = decode(&a_received, Law::Ulaw);
    // Nothing for the delay, and then the tone, 15 dB down.
    assert!(echo[..200].iter().all(|&x| x == 0.0));
    let level = level_dbm0(&echo[200..], Law::Ulaw);
    assert!((level + 27.0).abs() < CODING_DB, "{level}");
    let scale = 10f32.powf(-15.0 / 20.0);
    for n in 200..4000 {
        let expected = Law::Ulaw.decode(sent[n - 200]) * scale;
        assert!((echo[n] - expected).abs() < 0.01 * scale + 2.0e-4, "{n}");
    }
    // B has no echo and hears A's tone as it was sent.
    assert_eq!(&b_received[80..], &sent[..sent.len() - 80]);
}

#[test]
fn everything_at_once_is_still_the_same_on_every_run() {
    let config = ChannelConfig {
        seed: 77,
        law_a: Law::Alaw,
        law_b: Law::Ulaw,
        delay_samples: 240,
        clock_offset_ppm: 150.0,
        a_to_b: Impairments {
            pad_db: 3.0,
            gain_db: -12.0,
            loop_slope_db: Some(4.0),
            second_codec: Some(Law::Ulaw),
            frequency_offset_hz: 5.0,
            noise: Some(Noise::Level(-45.0)),
            echo: Some(Echo {
                delay_samples: 160,
                loss_db: 20.0,
            }),
            slips: Some(Slips {
                every_samples: 30_000,
                kind: SlipKind::Repeat,
            }),
            packets: Some(Packets {
                ptime_ms: 20,
                lost: vec![100],
                loss_probability: 0.001,
            }),
        },
        b_to_a: Impairments::default(),
    };
    let run = || {
        let mut duplex = Duplex::new(
            Box::new(EchoEngine::new()),
            Box::new(EchoEngine::new()),
            &config,
        );
        duplex.a.engine.originate();
        duplex.b.engine.answer();
        duplex.run(3000);
        let b = duplex.channel().stats(Side::B);
        let a = duplex.channel().stats(Side::A);
        (a, b, duplex.a.ticks, duplex.b.ticks)
    };
    let first = run();
    assert_eq!(first, run());
    let (a, b, a_ticks, b_ticks) = first;
    assert_eq!(a.underruns + b.underruns, 0);
    assert!(b.packets_lost > 0);
    assert_eq!(b_ticks, 3000);
    // A's clock is fast: it has made its share more ticks.
    assert!((3000..=3002).contains(&a_ticks), "{a_ticks}");
}

#[test]
fn a_duplex_carries_two_engines_data_both_ways() {
    let mut duplex = Duplex::new(
        Box::new(NullEngine::new(NullConfig::default())),
        Box::new(NullEngine::new(NullConfig::default())),
        &ChannelConfig::default(),
    );
    duplex.a.engine.originate();
    duplex.b.engine.answer();
    let mut rng = Rng::new(3);
    let from_a: Vec<u8> = (0..20_000).map(|_| rng.next_u64() as u8).collect();
    let from_b: Vec<u8> = (0..20_000).map(|_| rng.next_u64() as u8).collect();
    duplex.a.send(&from_a);
    duplex.b.send(&from_b);
    let done = duplex.run_until(2000, |d| {
        d.a.received.len() == 20_000 && d.b.received.len() == 20_000
    });
    assert!(done);
    assert_eq!(duplex.a.received, from_b);
    assert_eq!(duplex.b.received, from_a);
    assert!(duplex.a.connected() && duplex.b.connected());
    assert!(!duplex.a.carrier_lost());
    let connect = duplex
        .b
        .first(|e| matches!(e, ModemEvent::Connect(_)))
        .unwrap();
    assert_eq!(connect.tick, 0);
    // The two ends ticked in turn.
    assert!(duplex.a.ticks.abs_diff(duplex.b.ticks) <= 1);

    // One end stops: the other loses the carrier.
    duplex.a.engine.hangup();
    duplex.run(100);
    assert!(duplex.b.carrier_lost());
}

#[test]
fn a_fast_end_ticks_more_often_and_nothing_underruns() {
    for ppm in [200.0, -200.0] {
        let config = ChannelConfig {
            clock_offset_ppm: ppm,
            ..ChannelConfig::default()
        };
        let mut duplex = Duplex::new(
            Box::new(EchoEngine::new()),
            Box::new(EchoEngine::new()),
            &config,
        );
        duplex.a.engine.originate();
        duplex.b.engine.answer();
        duplex.run(100_000);
        // In B's 100,000 ticks, A has made 200 parts in a million more,
        // or fewer.
        let expected = 100_000.0 * (1.0 + ppm * 1.0e-6);
        assert!(
            (duplex.a.ticks as f64 - expected).abs() <= 1.0,
            "{ppm}: {}",
            duplex.a.ticks
        );
        for side in [Side::A, Side::B] {
            let stats = duplex.channel().stats(side);
            assert_eq!(stats.underruns, 0, "{ppm} {side:?}");
            // What waits between the ends stays a few ticks, for ever.
            assert!(stats.queued < 500, "{ppm} {side:?}: {}", stats.queued);
        }
    }
}
