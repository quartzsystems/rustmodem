// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem engine on impaired lines: two engines connect by V.8 to V.21
//! and pass 64 KiB each way intact through each impairment, and through
//! all of them at once.

mod common;

use chansim::{ChannelConfig, Echo, Impairments, Noise, Packets, Side};
use common::{Call, SECOND, connection, default_config, random_bytes};
use engine::{CallSettings, EngineState, Law, ModemConfig, ModemEvent, Modulation};

const KIB_64: usize = 64 * 1024;

/// Connects by V.8 to V.21 through a channel and passes 64 KiB each way.
fn carry(name: &str, channel: &ChannelConfig, modem: ModemConfig) -> Call {
    let mut call = Call::place(modem, modem, channel, CallSettings::default());
    assert!(call.connect(40), "{name}: did not connect");
    let (a, b) = call.connections();
    assert_eq!(a, Some(connection(Modulation::V21)), "{name}");
    assert_eq!(b, Some(connection(Modulation::V21)), "{name}");
    let from_a = random_bytes(11, KIB_64);
    let from_b = random_bytes(12, KIB_64);
    let (at_a, at_b) = call.exchange(&from_a, &from_b);
    assert_eq!(at_a.len(), KIB_64, "{name}: the originator received");
    assert_eq!(at_b.len(), KIB_64, "{name}: the answerer received");
    assert!(
        at_a == from_b,
        "{name}: the data to the originator is not intact"
    );
    assert!(
        at_b == from_a,
        "{name}: the data to the answerer is not intact"
    );
    for (end, side) in [(&call.duplex.a, Side::A), (&call.duplex.b, Side::B)] {
        assert!(!end.carrier_lost(), "{name}");
        assert_eq!(
            end.engine.diagnostics().unwrap().framing_errors,
            0,
            "{name}"
        );
        assert_eq!(call.duplex.channel().stats(side).underruns, 0, "{name}");
    }
    call
}

fn symmetric(impairments: Impairments) -> ChannelConfig {
    ChannelConfig::symmetric(impairments)
}

#[test]
fn noise() {
    // Each end hears the other at -30 dBm0 with white noise 6.75 dB below
    // it across the band, which is Eb/N0 of 18 dB: the least at which
    // 1.3 million bits pass without an error.
    let channel = ChannelConfig {
        seed: 5,
        ..symmetric(Impairments {
            gain_db: -18.0,
            noise: Some(Noise::Snr {
                signal_dbm0: -30.0,
                snr_db: 6.75,
            }),
            ..Impairments::default()
        })
    };
    let call = carry("noise", &channel, default_config());
    // The level shown is the signal's and the noise's in the band, which
    // is a seventh of the whole: within a decibel of the signal's.
    let level = call.duplex.b.engine.diagnostics().unwrap().rx_level_dbm0;
    assert!((level + 30.0).abs() < 1.5, "{level}");
}

#[test]
fn level_from_minus_10_to_minus_40_dbm0() {
    for (level, loss) in [(-10.0f32, 0.0f32), (-10.0, -15.0), (-10.0, -30.0)] {
        let channel = symmetric(Impairments {
            gain_db: loss,
            ..Impairments::default()
        });
        let modem = ModemConfig {
            tx_level_dbm0: level,
            ..default_config()
        };
        let name = format!("level {}", level + loss);
        let call = carry(&name, &channel, modem);
        // The diagnostics show the levels as they are.
        for end in [&call.duplex.a, &call.duplex.b] {
            let diagnostics = end.engine.diagnostics().unwrap();
            assert!(
                (diagnostics.rx_level_dbm0 - (level + loss)).abs() < 1.5,
                "{name}: receiving at {}",
                diagnostics.rx_level_dbm0
            );
            assert!(
                (diagnostics.tx_level_dbm0 - level).abs() < 0.5,
                "{name}: sending at {}",
                diagnostics.tx_level_dbm0
            );
        }
    }
}

#[test]
fn frequency_offset_of_7_hz() {
    for offset in [7.0f32, -7.0] {
        let channel = ChannelConfig {
            a_to_b: Impairments {
                gain_db: -15.0,
                frequency_offset_hz: offset,
                ..Impairments::default()
            },
            b_to_a: Impairments {
                gain_db: -15.0,
                frequency_offset_hz: -offset,
                ..Impairments::default()
            },
            ..ChannelConfig::default()
        };
        let call = carry(&format!("offset {offset}"), &channel, default_config());
        // Each end measures the offset of what it receives.
        let at_b = call
            .duplex
            .b
            .engine
            .diagnostics()
            .unwrap()
            .frequency_error_hz;
        let at_a = call
            .duplex
            .a
            .engine
            .diagnostics()
            .unwrap()
            .frequency_error_hz;
        assert!((at_b - offset).abs() < 2.0, "{offset}: {at_b}");
        assert!((at_a + offset).abs() < 2.0, "{offset}: {at_a}");
    }
}

#[test]
fn clock_offset_of_200_parts_in_a_million() {
    for ppm in [200.0, -200.0] {
        let channel = ChannelConfig {
            clock_offset_ppm: ppm,
            ..symmetric(Impairments {
                gain_db: -15.0,
                ..Impairments::default()
            })
        };
        let call = carry(&format!("clock {ppm}"), &channel, default_config());
        // The fast end has made its share more ticks: the call was the
        // better part of an hour.
        let (a, b) = (call.duplex.a.ticks as f64, call.duplex.b.ticks as f64);
        assert!(((a / b - 1.0) * 1.0e6 - ppm).abs() < 10.0, "{ppm}: {a} {b}");
    }
}

#[test]
fn near_echo() {
    // Each end hears the other 20 dB down, at -32 dBm0, and itself only
    // 8 dB down, at -20 dBm0: its own echo is 12 dB louder than the far
    // end, and is there through the answer tone and the V.8 exchange too.
    let channel = symmetric(Impairments {
        gain_db: -20.0,
        echo: Some(Echo {
            delay_samples: 240,
            loss_db: 8.0,
        }),
        ..Impairments::default()
    });
    carry("echo", &channel, default_config());
}

#[test]
fn everything_at_once_and_in_the_a_law() {
    let impairments = Impairments {
        gain_db: -22.0,
        noise: Some(Noise::Level(-56.0)),
        frequency_offset_hz: 4.0,
        loop_slope_db: Some(3.0),
        second_codec: Some(Law::Ulaw),
        echo: Some(Echo {
            delay_samples: 400,
            loss_db: 14.0,
        }),
        ..Impairments::default()
    };
    let channel = ChannelConfig {
        seed: 9,
        law_a: Law::Alaw,
        law_b: Law::Alaw,
        clock_offset_ppm: 100.0,
        ..symmetric(impairments)
    };
    carry("everything", &channel, default_config());
}

#[test]
fn a_packet_lost_in_the_v8_exchange_is_made_good() {
    // The call menu begins about 3.6 s in: a 20 ms packet is lost in it,
    // another in the joint menu, each way. The sequences are sent again
    // and the call connects a little later.
    let channel = symmetric(Impairments {
        packets: Some(Packets {
            ptime_ms: 20,
            lost: vec![183, 192, 204, 211],
            loss_probability: 0.0,
        }),
        ..Impairments::default()
    });
    let call = carry("loss in V.8", &channel, default_config());
    let (a_at, _) = call.connected_at_ms();
    assert!((5300..8500).contains(&a_at), "{a_at}");
    assert_eq!(call.duplex.channel().stats(Side::B).packets_lost, 4);
}

#[test]
fn a_packet_lost_in_the_data_is_a_burst_of_errors_and_no_more() {
    // Connect on a clean line that will lose one 20 ms packet, from the
    // originator to the answerer, twelve seconds in.
    let channel = ChannelConfig {
        a_to_b: Impairments {
            packets: Some(Packets {
                ptime_ms: 20,
                lost: vec![600],
                loss_probability: 0.0,
            }),
            ..Impairments::default()
        },
        ..ChannelConfig::default()
    };
    let mut call = Call::place(
        default_config(),
        default_config(),
        &channel,
        CallSettings::default(),
    );
    assert!(call.connect(40));
    let (a_at, _) = call.connected_at_ms();
    assert!(a_at < 8000);
    // Data both ways across the loss: 600 characters, 20 seconds.
    let from_a = random_bytes(21, 600);
    let from_b = random_bytes(22, 600);
    let (at_a, at_b) = call.exchange(&from_a, &from_b);
    assert_eq!(call.duplex.channel().stats(Side::B).packets_lost, 1);

    // The way that lost nothing is intact.
    assert!(at_a == from_b);
    // The way that lost the packet lost 20 ms, which is six bits, and
    // the receiver takes a character or two more to find a start bit
    // again: at the most five characters are wrong or missing, and
    // everything before and after is as it was sent.
    let same_at_start = at_b
        .iter()
        .zip(&from_a)
        .take_while(|(got, sent)| got == sent)
        .count();
    let same_at_end = at_b
        .iter()
        .rev()
        .zip(from_a.iter().rev())
        .take_while(|(got, sent)| got == sent)
        .count();
    assert!(same_at_start > 50, "{same_at_start}");
    assert!(same_at_end > 50, "{same_at_end}");
    assert!(
        same_at_start + same_at_end >= from_a.len() - 5,
        "{same_at_start} and {same_at_end} of {}",
        from_a.len()
    );
    assert!(at_b.len().abs_diff(from_a.len()) <= 5, "{}", at_b.len());
    // The carrier was not lost, and the call goes on: what is sent now
    // arrives whole.
    assert!(!call.duplex.a.carrier_lost() && !call.duplex.b.carrier_lost());
    let more = random_bytes(23, 300);
    let (_, at_b) = call.exchange(&more, &[]);
    assert!(at_b == more);
    assert_eq!(
        call.duplex.b.engine.diagnostics().unwrap().state,
        EngineState::Data
    );
}

#[test]
fn a_carrier_that_comes_back_within_s10_is_not_lost() {
    // Half a second of nothing from the originator, which is 25 packets,
    // and S10 at its factory 1.4 s.
    let channel = ChannelConfig {
        a_to_b: Impairments {
            packets: Some(Packets {
                ptime_ms: 20,
                lost: (600..625).collect(),
                loss_probability: 0.0,
            }),
            ..Impairments::default()
        },
        ..ChannelConfig::default()
    };
    let mut call = Call::place(
        default_config(),
        default_config(),
        &channel,
        CallSettings::default(),
    );
    assert!(call.connect(40));
    call.duplex.run(10 * SECOND);
    assert!(!call.duplex.b.carrier_lost());
    let more = random_bytes(31, 200);
    let (_, at_b) = call.exchange(&more, &[]);
    assert!(at_b == more);

    // With S10 at 3 tenths the same gap is the end of the call.
    let short = CallSettings {
        carrier_loss_ms: 300,
        ..CallSettings::default()
    };
    let mut call = Call::place(default_config(), default_config(), &channel, short);
    assert!(call.connect(40));
    call.duplex.run(10 * SECOND);
    assert!(
        call.duplex
            .b
            .first(|event| matches!(event, ModemEvent::CarrierLost))
            .is_some()
    );
}
