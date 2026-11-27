// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem engine on a clean line: every way two of them connect, with
//! 64 KiB of data each way.

mod common;

use chansim::ChannelConfig;
use common::{Call, config, connection, default_config, random_bytes};
use engine::{CallSettings, EngineState, Law, Modem, ModemEngine, ModemEvent, Modulation};

/// 64 KiB each way, as the phase's exit criteria ask.
const KIB_64: usize = 64 * 1024;

fn clean() -> ChannelConfig {
    ChannelConfig::default()
}

/// Places a call, sees it connect on a modem, and passes 64 KiB each way.
fn connect_and_carry(
    name: &str,
    originator: engine::ModemConfig,
    answerer: engine::ModemConfig,
    channel: &ChannelConfig,
    standard: Modulation,
) -> Call {
    let mut call = Call::place(originator, answerer, channel, CallSettings::default());
    assert!(call.connect(40), "{name}: did not connect");
    let (a, b) = call.connections();
    assert_eq!(a, Some(connection(standard)), "{name}");
    assert_eq!(b, Some(connection(standard)), "{name}");

    let from_a = random_bytes(1, KIB_64);
    let from_b = random_bytes(2, KIB_64);
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
    for end in [&call.duplex.a, &call.duplex.b] {
        assert!(!end.carrier_lost(), "{name}");
        let diagnostics = end.engine.diagnostics().unwrap();
        assert_eq!(diagnostics.state, EngineState::Data, "{name}");
        assert_eq!(diagnostics.framing_errors, 0, "{name}");
        assert!(diagnostics.carrier, "{name}");
    }
    call
}

#[test]
fn v8_to_v21() {
    for (law_a, law_b) in [(Law::Ulaw, Law::Ulaw), (Law::Alaw, Law::Alaw)] {
        let channel = ChannelConfig {
            law_a,
            law_b,
            ..clean()
        };
        let call = connect_and_carry(
            "V.8 to V.21",
            default_config(),
            default_config(),
            &channel,
            Modulation::V21,
        );
        // The answering side went through the V.8 exchange: silence,
        // ANSam, JM, the gap, its carrier, data.
        let history: Vec<EngineState> = call
            .duplex
            .b
            .engine
            .diagnostics()
            .unwrap()
            .history()
            .iter()
            .map(|change| change.state)
            .collect();
        assert_eq!(
            history,
            [
                EngineState::AnswerSilence,
                EngineState::AnswerTone,
                EngineState::JointMenu,
                EngineState::Gap,
                EngineState::Training,
                EngineState::Data,
            ]
        );
        let history: Vec<EngineState> = call
            .duplex
            .a
            .engine
            .diagnostics()
            .unwrap()
            .history()
            .iter()
            .map(|change| change.state)
            .collect();
        assert_eq!(
            history,
            [
                EngineState::Listening,
                EngineState::CallSilence,
                EngineState::CallMenu,
                EngineState::Terminator,
                EngineState::Training,
                EngineState::Data,
            ]
        );
        // Silence of 2.15 s, ANSam known in 0.4 s, Te of 1 s, the
        // exchange in under a second, and then each carrier there for the
        // 0.6 s of S9: about five and a half seconds.
        let (a_at, b_at) = call.connected_at_ms();
        assert!((5000..6500).contains(&a_at), "{a_at}");
        assert!((5000..6500).contains(&b_at), "{b_at}");
    }
}

#[test]
fn a_v8_answerer_and_a_v21_caller_without_v8() {
    // The caller hears ANSam and, having no V.8, treats it as answer
    // tone: it waits for the tone to end. The answerer's ANSam runs its 5 s
    // with no CM, and it falls back to its V.21 carrier, which tells the
    // caller what it is.
    let call = connect_and_carry(
        "V.21 without V.8",
        config(&[Modulation::V21], false),
        default_config(),
        &clean(),
        Modulation::V21,
    );
    let history: Vec<EngineState> = call
        .duplex
        .b
        .engine
        .diagnostics()
        .unwrap()
        .history()
        .iter()
        .map(|change| change.state)
        .collect();
    assert_eq!(
        history,
        [
            EngineState::AnswerSilence,
            EngineState::AnswerTone,
            EngineState::Gap,
            EngineState::Fallback,
            EngineState::Training,
            EngineState::Data,
        ]
    );
    // 2.15 s of silence, 5 s of ANSam, 75 ms, and then the carriers.
    let (_, b_at) = call.connected_at_ms();
    assert!((7500..9500).contains(&b_at), "{b_at}");
}

#[test]
fn bell_103() {
    // A Bell 103 caller, and an answerer that allows V.21 first and then
    // Bell 103: after ANSam the answerer sends its V.21 carrier for its
    // time, which the caller does not answer, and then its Bell 103
    // carrier, which it does.
    let call = connect_and_carry(
        "Bell 103 by fallback",
        config(&[Modulation::Bell103], false),
        default_config(),
        &clean(),
        Modulation::Bell103,
    );
    let (_, b_at) = call.connected_at_ms();
    assert!((7500..13_000).contains(&b_at), "{b_at}");

    // Two Bell 103 modems, neither with V.21: plain answer tone, and the
    // one carrier.
    connect_and_carry(
        "Bell 103 alone",
        config(&[Modulation::Bell103], true),
        config(&[Modulation::Bell103], true),
        &clean(),
        Modulation::Bell103,
    );
}

#[test]
fn without_v8_at_either_end_it_is_answer_tone_and_carriers() {
    let call = connect_and_carry(
        "no V.8",
        config(&[Modulation::V21, Modulation::Bell103], false),
        config(&[Modulation::V21, Modulation::Bell103], false),
        &clean(),
        Modulation::V21,
    );
    // 2.15 s of silence, 3.3 s of ANS, 75 ms, and the carriers.
    let (_, b_at) = call.connected_at_ms();
    assert!((6000..8000).contains(&b_at), "{b_at}");
}

#[test]
fn the_fallback_order_is_the_lines() {
    // An answerer that prefers Bell 103 and a caller that will do either
    // and has no V.8: Bell 103, because the answerer's first carrier is
    // Bell's and the caller goes by the carrier it hears.
    connect_and_carry(
        "Bell first",
        config(&[Modulation::V21, Modulation::Bell103], false),
        config(&[Modulation::Bell103, Modulation::V21], false),
        &clean(),
        Modulation::Bell103,
    );
}

#[test]
fn modems_with_nothing_in_common_do_not_connect() {
    // A V.21 answerer and a Bell 103 caller.
    let mut call = Call::place(
        config(&[Modulation::Bell103], false),
        config(&[Modulation::V21], true),
        &clean(),
        CallSettings::default(),
    );
    assert!(!call.connect(60));
    // The answerer tried what it had and reported failure, which the
    // line turns into NO CARRIER.
    assert!(call.duplex.b.carrier_lost());
    assert!(!call.duplex.b.connected());
    let diagnostics = call.duplex.b.engine.diagnostics().unwrap();
    assert_eq!(diagnostics.state, EngineState::Ended);
    // Ended, it sends the octet of silence.
    let mut tx = [0u8; 80];
    call.duplex.b.engine.process(&[0xFF; 80], &mut tx);
    assert_eq!(tx, [0xFF; 80]);
}

#[test]
fn the_host_settings_of_the_call_override_the_lines() {
    // The line allows both and V.8; the host, with ATB1 or +MS, says
    // Bell 103 for this call.
    let settings = CallSettings {
        modulations: Some(engine::modulations(&[Modulation::Bell103])),
        ..CallSettings::default()
    };
    let mut call = Call::place(default_config(), default_config(), &clean(), settings);
    assert!(call.connect(40));
    assert_eq!(call.connections().0, Some(connection(Modulation::Bell103)));

    // S9 of 20 tenths: the connection is reported 1.4 s later than with
    // the factory 6.
    let slow = CallSettings {
        carrier_response_ms: 2000,
        ..CallSettings::default()
    };
    let mut quick_call = Call::place(
        default_config(),
        default_config(),
        &clean(),
        CallSettings::default(),
    );
    let mut slow_call = Call::place(default_config(), default_config(), &clean(), slow);
    assert!(quick_call.connect(40) && slow_call.connect(40));
    let (quick, _) = quick_call.connected_at_ms();
    let (late, _) = slow_call.connected_at_ms();
    assert!((1350..=1450).contains(&(late - quick)), "{quick} {late}");
}

#[test]
fn when_the_far_end_stops_the_carrier_is_lost_after_s10() {
    for tenths in [14u32, 5, 40] {
        let settings = CallSettings {
            carrier_loss_ms: tenths * 100,
            ..CallSettings::default()
        };
        let mut call = Call::place(default_config(), default_config(), &clean(), settings);
        assert!(call.connect(40));
        call.exchange(b"before", b"before");
        // The originator hangs up: it sends silence from here.
        let stopped = call.duplex.b.ticks;
        call.duplex.a.engine.hangup();
        call.duplex.run(8 * common::SECOND);
        let lost = call
            .duplex
            .b
            .first(|event| matches!(event, ModemEvent::CarrierLost))
            .expect("the answerer loses the carrier");
        // S10 after the carrier went, and the 50 ms or so the line signal
        // detector takes to say so.
        let after_ms = (lost.tick - stopped) * 10;
        let expected = u64::from(tenths) * 100;
        assert!(
            (expected..expected + 150).contains(&after_ms),
            "S10={tenths}: lost after {after_ms} ms"
        );
        assert_eq!(
            call.duplex.b.engine.diagnostics().unwrap().state,
            EngineState::Ended
        );
    }
}

#[test]
fn a_stopped_engine_is_silent_and_takes_no_data() {
    let mut modem = Modem::new(default_config());
    for law in [Law::Ulaw, Law::Alaw] {
        modem.set_law(law);
        let mut tx = [0u8; 160];
        modem.process(&[0x55; 160], &mut tx);
        assert!(tx.iter().all(|&octet| octet == law.silence()));
    }
    assert_eq!(modem.dte_write(b"data"), 0);
    assert_eq!(modem.dte_read(&mut [0u8; 16]), 0);
    assert_eq!(modem.poll_event(), None);
    assert_eq!(modem.diagnostics().unwrap().state, EngineState::Idle);
    assert_eq!(modem.config(), &default_config());
    // Before the connection the host's data is not taken.
    modem.answer();
    assert_eq!(modem.dte_write(b"data"), 0);
    modem.hangup();
    assert_eq!(modem.diagnostics().unwrap().state, EngineState::Idle);
    // Slices of unequal length do no harm.
    modem.set_law(Law::Ulaw);
    let mut tx = [0u8; 80];
    modem.process(&[0xFF; 40], &mut tx);
    assert_eq!(tx, [0xFF; 80]);
    // Nothing allowed: nothing to be.
    let mut none = Modem::new(config(&[], true));
    none.answer();
    assert_eq!(none.poll_event(), Some(ModemEvent::CarrierLost));
}

#[test]
fn the_character_format_of_the_call_is_the_lines() {
    use engine::Format;
    // +ICF=5,1 at both ends: seven data bits and even parity. What the
    // hosts give is carried in seven bits, and nothing is counted wrong.
    for format in [Format::E71, Format::O71] {
        let settings = CallSettings {
            format,
            ..CallSettings::default()
        };
        let mut call = Call::place(default_config(), default_config(), &clean(), settings);
        assert!(call.connect(40), "{format:?}");
        let from_a = random_bytes(3, 4096);
        let from_b = random_bytes(4, 4096);
        let (at_a, at_b) = call.exchange(&from_a, &from_b);
        let seven = |data: &[u8]| data.iter().map(|b| b & 0x7F).collect::<Vec<u8>>();
        assert_eq!(at_a, seven(&from_b), "{format:?}");
        assert_eq!(at_b, seven(&from_a), "{format:?}");
        for end in [&call.duplex.a, &call.duplex.b] {
            assert_eq!(end.engine.diagnostics().unwrap().framing_errors, 0);
        }
    }
}
