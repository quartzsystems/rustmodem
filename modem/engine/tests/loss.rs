// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem engine at 2400 bit/s with error control, on a line that
//! loses packets, changes under the call, and is bad in every way at
//! once: what the host is given is what the other host sent, and the
//! call holds.

mod common;

use chansim::{ChannelConfig, Echo, Impairments, Noise, Packets};
use common::{Call, SECOND, random_bytes};
use engine::{
    CallSettings, ConnectInfo, EngineState, Law, LinkStage, ModemConfig, ModemEvent, Modulation,
};

/// The connection of two default modems.
fn expected() -> ConnectInfo {
    ConnectInfo {
        rate: 2400,
        carrier: Some(Modulation::V22bis.carrier_name().to_string()),
        protocol: Some("LAPM".to_string()),
        compression: Some("V.42bis".to_string()),
    }
}

/// Places a call between two default modems and sees it connect.
fn place(name: &str, channel: &ChannelConfig, settings: CallSettings) -> Call {
    let mut call = Call::place(
        ModemConfig::default(),
        ModemConfig::default(),
        channel,
        settings,
    );
    assert!(
        call.connect(90),
        "{name}: did not connect: {:?} and {:?}",
        call.duplex.a.events,
        call.duplex.b.events
    );
    assert_eq!(
        call.connections(),
        (Some(expected()), Some(expected())),
        "{name}"
    );
    call
}

/// What has arrived at each end is the start of what the other was
/// given, octet for octet.
fn intact(name: &str, call: &Call, from_a: &[u8], from_b: &[u8]) {
    let (at_a, at_b) = (&call.duplex.a.received, &call.duplex.b.received);
    assert!(
        at_a.len() <= from_b.len() && at_b.len() <= from_a.len(),
        "{name}"
    );
    assert!(
        at_a[..] == from_b[..at_a.len()],
        "{name}: the data to the originator is not intact"
    );
    assert!(
        at_b[..] == from_a[..at_b.len()],
        "{name}: the data to the answerer is not intact"
    );
}

fn count(end: &chansim::End, wanted: &ModemEvent) -> usize {
    end.events
        .iter()
        .filter(|event| &event.what == wanted)
        .count()
}

fn lossy(probability: f64, lost: Vec<u64>) -> Impairments {
    Impairments {
        packets: Some(Packets {
            ptime_ms: 20,
            lost,
            loss_probability: probability,
        }),
        ..Impairments::default()
    }
}

#[test]
fn one_packet_in_a_hundred_lost_for_ten_minutes_loses_no_data_and_not_the_call() {
    // The test that matters: packets of 20 ms, one in a hundred of them
    // lost at random each way, for ten minutes, with data flowing both
    // ways all the while.
    let channel = ChannelConfig {
        seed: 3,
        ..ChannelConfig::symmetric(lossy(0.01, Vec::new()))
    };
    let mut call = place("loss", &channel, CallSettings::default());
    // More than ten minutes of a clean line carry.
    let from_a = random_bytes(1, 200_000);
    let from_b = random_bytes(2, 200_000);
    call.duplex.a.send(&from_a);
    call.duplex.b.send(&from_b);
    call.duplex.run(600 * SECOND);

    intact("loss", &call, &from_a, &from_b);
    for end in [&call.duplex.a, &call.duplex.b] {
        assert!(!end.carrier_lost(), "{:?}", end.events);
        let diagnostics = end.engine.diagnostics().unwrap();
        assert_eq!(diagnostics.state, EngineState::Data);
        assert_eq!(diagnostics.link.stage, LinkStage::Lapm);
        // A frame of 128 octets is on the line for nearly half a second,
        // which is 22 packets: one frame in five is hit, and every one
        // of them was sent again.
        assert!(
            diagnostics.link.frame_errors > 100,
            "{:?}",
            diagnostics.link
        );
        assert!(
            diagnostics.link.retransmissions > 100,
            "{:?}",
            diagnostics.link
        );
        // And still more than a third of what a clean line carries got
        // through: 600 s at 300 octets a second is 180 000.
        assert!(end.received.len() > 60_000, "{}", end.received.len());
    }
    // Every loss was a packet of silence: 600 s is 30 000 packets each
    // way, and about 300 of them were lost.
    let stats = call.duplex.channel().stats(chansim::Side::B);
    assert!(
        stats.packets_lost > 200 && stats.packets_lost < 400,
        "{stats:?}"
    );

    // The line is clean again, and the rest arrives.
    call.duplex.change_channel(&ChannelConfig::default());
    call.duplex
        .run_until(call.duplex.b.ticks + 600 * SECOND, |d| {
            d.a.received.len() >= from_b.len() && d.b.received.len() >= from_a.len()
        });
    assert!(call.duplex.a.received == from_b);
    assert!(call.duplex.b.received == from_a);
}

#[test]
fn a_burst_of_lost_packets_is_ridden_out_and_the_data_goes_on_intact() {
    // A second of the line gone, one way, twenty seconds into the data:
    // fifty packets in a row. The carrier is back before S10 has run,
    // the receiver finds the signal where it left it, and LAPM sends
    // again what was lost.
    let burst: Vec<u64> = (1500..1550).collect();
    let channel = ChannelConfig {
        a_to_b: lossy(0.0, burst),
        ..ChannelConfig::default()
    };
    let mut call = place("burst", &channel, CallSettings::default());
    let from_a = random_bytes(1, 20_000);
    let from_b = random_bytes(2, 20_000);
    call.duplex.a.send(&from_a);
    call.duplex.b.send(&from_b);
    call.duplex
        .run_until(call.duplex.b.ticks + 200 * SECOND, |d| {
            d.a.received.len() >= from_b.len() && d.b.received.len() >= from_a.len()
        });
    assert!(call.duplex.a.received == from_b);
    assert!(call.duplex.b.received == from_a);
    assert_eq!(
        call.duplex.channel().stats(chansim::Side::B).packets_lost,
        50
    );
    for end in [&call.duplex.a, &call.duplex.b] {
        assert!(!end.carrier_lost());
    }
    // The answerer lost frames and the originator sent them again.
    let a = call.duplex.a.engine.diagnostics().unwrap();
    let b = call.duplex.b.engine.diagnostics().unwrap();
    assert!(b.link.frame_errors >= 1, "{:?}", b.link);
    assert!(a.link.retransmissions >= 1, "{:?}", a.link);
}

#[test]
fn a_line_that_changes_makes_the_modems_retrain_and_the_data_goes_on_intact() {
    // What forces a retrain is not silence, which the receiver rides
    // out, but a line that comes back another line: its loss 6 dB more
    // and its response changed, as when a network takes another route.
    // The equalisers are wrong for it; one end or the other finds its
    // error too high and retrains (V.22bis clause 6.4). The hosts see
    // their data stop for a moment and go on.
    let mut call = place(
        "retrain",
        &ChannelConfig::default(),
        CallSettings::default(),
    );
    let from_a = random_bytes(1, 60_000);
    let from_b = random_bytes(2, 60_000);
    call.duplex.a.send(&from_a);
    call.duplex.b.send(&from_b);
    call.duplex.run(30 * SECOND);
    intact("before the change", &call, &from_a, &from_b);
    let before = (call.duplex.a.received.len(), call.duplex.b.received.len());
    assert!(before.0 > 5000 && before.1 > 5000);

    // A burst of loss, and the line that comes back is another.
    let worse = ChannelConfig {
        seed: 5,
        a_to_b: Impairments {
            gain_db: -6.0,
            loop_slope_db: Some(6.0),
            ..lossy(0.0, (0..25).collect())
        },
        b_to_a: Impairments {
            gain_db: -6.0,
            loop_slope_db: Some(6.0),
            ..lossy(0.0, (0..25).collect())
        },
        ..ChannelConfig::default()
    };
    call.duplex.change_channel(&worse);
    call.duplex.run(10 * SECOND);
    for end in [&call.duplex.a, &call.duplex.b] {
        // The pool is told of the retrain; the host of nothing.
        assert!(count(end, &ModemEvent::Retrain) >= 1, "{:?}", end.events);
        assert!(!end.carrier_lost(), "{:?}", end.events);
        let diagnostics = end.engine.diagnostics().unwrap();
        assert_eq!(
            diagnostics.retrains as usize,
            count(end, &ModemEvent::Retrain)
        );
        assert_eq!(diagnostics.state, EngineState::Data);
        let history: Vec<EngineState> = diagnostics.history().iter().map(|c| c.state).collect();
        assert!(
            history.ends_with(&[EngineState::Retrain, EngineState::Data]),
            "{history:?}"
        );
        assert_eq!(diagnostics.link.stage, LinkStage::Lapm);
    }
    // The rest of the data, intact, through the retrain.
    call.duplex
        .run_until(call.duplex.b.ticks + 400 * SECOND, |d| {
            d.a.received.len() >= from_b.len() && d.b.received.len() >= from_a.len()
        });
    assert!(call.duplex.a.received == from_b);
    assert!(call.duplex.b.received == from_a);
}

#[test]
fn a_line_that_is_gone_for_longer_than_s10_ends_the_call() {
    // Three seconds of nothing, both ways. S10 is 1.4 s: each end
    // reports the carrier lost, once, and stops.
    let burst: Vec<u64> = (1500..1650).collect();
    let channel = ChannelConfig::symmetric(lossy(0.0, burst));
    let mut call = place("gone", &channel, CallSettings::default());
    let from_a = random_bytes(1, 20_000);
    call.duplex.a.send(&from_a);
    call.duplex.run(60 * SECOND);
    for end in [&call.duplex.a, &call.duplex.b] {
        assert_eq!(count(end, &ModemEvent::CarrierLost), 1, "{:?}", end.events);
        let lost = end
            .first(|event| matches!(event, ModemEvent::CarrierLost))
            .unwrap();
        // The burst begins 30 s into the call, and the carrier is
        // reported lost 1.4 s after it went, and the detector's time.
        assert!(
            lost.tick > 3140 && lost.tick < 3180,
            "at tick {}",
            lost.tick
        );
        assert_eq!(end.engine.diagnostics().unwrap().state, EngineState::Ended);
    }
    // What did arrive is what was sent.
    intact("gone", &call, &from_a, &[]);

    // With S10 at ten seconds the same burst is ridden out.
    let patient = CallSettings {
        carrier_loss_ms: 10_000,
        ..CallSettings::default()
    };
    let burst: Vec<u64> = (1500..1650).collect();
    let channel = ChannelConfig::symmetric(lossy(0.0, burst));
    let mut call = place("patient", &channel, patient);
    call.duplex.a.send(&from_a);
    call.duplex
        .run_until(call.duplex.b.ticks + 200 * SECOND, |d| {
            d.b.received.len() >= from_a.len()
        });
    assert!(call.duplex.b.received == from_a);
    for end in [&call.duplex.a, &call.duplex.b] {
        assert!(!end.carrier_lost(), "{:?}", end.events);
    }
}

#[test]
fn everything_at_once_and_in_the_a_law() {
    // A long loop each way, noise, a carrier system's frequency offset, a
    // second coding, an echo, two clocks 100 parts in a million apart,
    // and a packet lost in every five hundred.
    let impairments = Impairments {
        gain_db: -15.0,
        noise: Some(Noise::Snr {
            signal_dbm0: -27.0,
            snr_db: 30.0,
        }),
        frequency_offset_hz: 4.0,
        loop_slope_db: Some(3.0),
        second_codec: Some(Law::Ulaw),
        echo: Some(Echo {
            delay_samples: 400,
            loss_db: 14.0,
        }),
        ..lossy(0.002, Vec::new())
    };
    let channel = ChannelConfig {
        seed: 9,
        law_a: Law::Alaw,
        law_b: Law::Alaw,
        clock_offset_ppm: 100.0,
        ..ChannelConfig::symmetric(impairments)
    };
    let mut call = place("everything", &channel, CallSettings::default());
    let from_a = random_bytes(1, 64 * 1024);
    let from_b = random_bytes(2, 64 * 1024);
    call.duplex.a.send(&from_a);
    call.duplex.b.send(&from_b);
    call.duplex
        .run_until(call.duplex.b.ticks + 900 * SECOND, |d| {
            d.a.received.len() >= from_b.len() && d.b.received.len() >= from_a.len()
        });
    assert!(call.duplex.a.received == from_b);
    assert!(call.duplex.b.received == from_a);
    for end in [&call.duplex.a, &call.duplex.b] {
        assert!(!end.carrier_lost(), "{:?}", end.events);
        let diagnostics = end.engine.diagnostics().unwrap();
        assert_eq!(diagnostics.state, EngineState::Data);
        // The receiver measured the line.
        assert!(
            (diagnostics.frequency_error_hz.abs() - 4.0).abs() < 1.0,
            "{} Hz",
            diagnostics.frequency_error_hz
        );
        assert!(
            (diagnostics.timing_offset_ppm.abs() - 100.0).abs() < 40.0,
            "{} ppm",
            diagnostics.timing_offset_ppm
        );
        assert!(
            diagnostics.rx_level_dbm0 > -30.0 && diagnostics.rx_level_dbm0 < -24.0,
            "{} dBm0",
            diagnostics.rx_level_dbm0
        );
        // The noise is 30 dB under the signal across 4 kHz, and less of
        // it is in the modem's channel.
        assert!(
            diagnostics.snr_db > 25.0 && diagnostics.snr_db < 40.0,
            "{} dB",
            diagnostics.snr_db
        );
    }
}
