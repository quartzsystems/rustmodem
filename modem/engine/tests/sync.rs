// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem engine at 1200 and 2400 bit/s on a clean line: every way two
//! of them connect, with the data link each way makes, and 1 MiB of data
//! each way in every one.

mod common;

use chansim::ChannelConfig;
use common::{Call, SECOND, config, random_bytes};
use engine::{
    CallSettings, Compression, ConnectInfo, EngineState, ErrorControl, GuardTone, LinkStage,
    ModemConfig, ModemEvent, Modulation,
};

/// 1 MiB each way, as the phase's exit criteria ask.
const MIB: usize = 1024 * 1024;

fn clean() -> ChannelConfig {
    ChannelConfig::default()
}

/// The connection an engine reports.
fn connection(modulation: Modulation, lapm: bool, compression: bool) -> ConnectInfo {
    ConnectInfo {
        rate: modulation.rate(),
        carrier: Some(modulation.carrier_name().to_string()),
        protocol: lapm.then(|| "LAPM".to_string()),
        compression: compression.then(|| "V.42bis".to_string()),
    }
}

/// What a call is expected to come to.
#[derive(Clone, Copy)]
struct Expect {
    /// The modulation the originator reports, and the answerer.
    modulations: (Modulation, Modulation),
    lapm: bool,
    compression: bool,
}

impl Expect {
    fn of(modulation: Modulation, lapm: bool, compression: bool) -> Expect {
        Expect {
            modulations: (modulation, modulation),
            lapm,
            compression,
        }
    }
}

/// Sends data both ways and runs until all of it has arrived or `seconds`
/// have passed, and a little longer. Returns what each end received.
fn carry(call: &mut Call, from_a: &[u8], from_b: &[u8], seconds: u64) -> (Vec<u8>, Vec<u8>) {
    let already_a = call.duplex.a.received.len();
    let already_b = call.duplex.b.received.len();
    call.duplex.a.send(from_a);
    call.duplex.b.send(from_b);
    let limit = call.duplex.b.ticks + seconds * SECOND;
    let (want_a, want_b) = (already_a + from_b.len(), already_b + from_a.len());
    call.duplex.run_until(limit, |d| {
        d.a.received.len() >= want_a && d.b.received.len() >= want_b
    });
    // A little longer, to see that nothing more arrives than was sent.
    call.duplex.run(SECOND / 2);
    (
        call.duplex.a.received[already_a..].to_vec(),
        call.duplex.b.received[already_b..].to_vec(),
    )
}

/// Places a call and sees it connect as expected.
///
/// An answerer that has no error control is, to an originator that has,
/// a modem without V.42: the originator's detection pattern, which is
/// the character DC1 with its parity even and then odd, goes to the
/// answerer's host as the characters it is (V.42 clause 7.2.1.2 chose
/// them to be harmless there). They are looked at here and taken away.
fn connect(name: &str, originator: ModemConfig, answerer: ModemConfig, expect: Expect) -> Call {
    let pattern = answerer.error_control == ErrorControl::Off
        && originator.error_control != ErrorControl::Off;
    let mut call = Call::place(originator, answerer, &clean(), CallSettings::default());
    assert!(
        call.connect(60),
        "{name}: did not connect: {:?} and {:?}",
        call.duplex.a.events,
        call.duplex.b.events
    );
    let (a, b) = call.connections();
    assert_eq!(
        a,
        Some(connection(
            expect.modulations.0,
            expect.lapm,
            expect.compression
        )),
        "{name}: the originator"
    );
    assert_eq!(
        b,
        Some(connection(
            expect.modulations.1,
            expect.lapm,
            expect.compression
        )),
        "{name}: the answerer"
    );
    // The timer of the detection phase and more: what is left of the
    // pattern has arrived.
    call.duplex.run(SECOND);
    assert!(call.duplex.a.received.is_empty(), "{name}");
    let at_b = std::mem::take(&mut call.duplex.b.received);
    if pattern {
        assert!(!at_b.is_empty(), "{name}");
        assert!(
            at_b.iter().all(|&octet| octet == 0x11 || octet == 0x91),
            "{name}: {at_b:02X?}"
        );
    } else {
        assert!(at_b.is_empty(), "{name}: {at_b:02X?}");
    }
    call
}

/// Places a call, sees it connect as expected, and passes 1 MiB each
/// way.
fn connect_and_carry(
    name: &str,
    originator: ModemConfig,
    answerer: ModemConfig,
    expect: Expect,
) -> Call {
    let mut call = connect(name, originator, answerer, expect);
    let from_a = random_bytes(1, MIB);
    let from_b = random_bytes(2, MIB);
    // By LAPM a line carries a little under an octet for every eight of
    // its bits; as start-stop characters, one for every ten. Half as
    // long again is allowed.
    let rate = u64::from(expect.modulations.0.rate());
    let seconds = MIB as u64 * 10 / rate * 3 / 2;
    let (at_a, at_b) = carry(&mut call, &from_a, &from_b, seconds);
    assert_eq!(at_a.len(), MIB, "{name}: the originator received");
    assert_eq!(at_b.len(), MIB, "{name}: the answerer received");
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
        assert!(diagnostics.carrier, "{name}");
        assert_eq!(diagnostics.retrains, 0, "{name}");
        assert_eq!(diagnostics.framing_errors, 0, "{name}");
        let link = diagnostics.link;
        assert_eq!(link.frame_errors, 0, "{name}");
        assert_eq!(link.retransmissions, 0, "{name}");
        if expect.lapm {
            assert_eq!(link.stage, LinkStage::Lapm, "{name}");
            // 1 MiB in frames of 128 octets at the most.
            assert!(link.frames_sent >= 8192, "{name}: {link:?}");
            assert!(link.frames_received >= 8192, "{name}: {link:?}");
        } else {
            assert_eq!(link.stage, LinkStage::Async, "{name}");
            assert_eq!(link.frames_sent, 0, "{name}");
        }
        assert_eq!(
            link.compression,
            (expect.compression, expect.compression),
            "{name}"
        );
    }
    call
}

fn states(end: &chansim::End) -> Vec<EngineState> {
    end.engine
        .diagnostics()
        .unwrap()
        .history()
        .iter()
        .map(|change| change.state)
        .collect()
}

/// The default modem: every modulation, V.8, error control if the far
/// end has it, compression.
fn default() -> ModemConfig {
    ModemConfig::default()
}

/// A modem that allows these modulations, with no V.8.
fn only(modulations: &[Modulation]) -> ModemConfig {
    config(modulations, false)
}

// The calls: 1 MiB each way in every one.

#[test]
fn v8_to_v22bis_with_lapm_and_v42bis() {
    let call = connect_and_carry(
        "V.8 to V.22bis",
        default(),
        default(),
        Expect::of(Modulation::V22bis, true, true),
    );
    // The answering side: silence, ANSam, JM, the gap, the handshake of
    // V.22bis, the data link, data.
    assert_eq!(
        states(&call.duplex.b),
        [
            EngineState::AnswerSilence,
            EngineState::AnswerTone,
            EngineState::JointMenu,
            EngineState::Gap,
            EngineState::Training,
            EngineState::Link,
            EngineState::Data,
        ]
    );
    assert_eq!(
        states(&call.duplex.a),
        [
            EngineState::Listening,
            EngineState::CallSilence,
            EngineState::CallMenu,
            EngineState::Terminator,
            EngineState::Training,
            EngineState::Link,
            EngineState::Data,
        ]
    );
    for end in [&call.duplex.a, &call.duplex.b] {
        let diagnostics = end.engine.diagnostics().unwrap();
        // A clean line: the sixteen points are tight.
        assert!(diagnostics.snr_db > 30.0, "{}", diagnostics.snr_db);
        assert!(diagnostics.equalizer_mse < 0.001);
        assert!(diagnostics.quality > 0.9, "{}", diagnostics.quality);
        assert!(diagnostics.timing_offset_ppm.abs() < 20.0);
        assert!(diagnostics.frequency_error_hz.abs() < 0.5);
        assert_eq!(diagnostics.constellation_bits, 4);
        let points = diagnostics.constellation();
        assert_eq!(points.len(), engine::CONSTELLATION_POINTS);
        let unit = f32::from(engine::CONSTELLATION_UNIT);
        for point in points {
            for part in point {
                // Each part is at 1 or 3 units, either way.
                let units = f32::from(*part).abs() / unit;
                let off = (units - 1.0).abs().min((units - 3.0).abs());
                assert!(off < 0.15, "a point at {point:?}");
            }
        }
        // Random data does not compress: it went as it is.
        let link = diagnostics.link;
        assert_eq!(link.compress_in, MIB as u32);
        assert_eq!(link.decompress_out, MIB as u32);
        assert!(link.compress_out < link.compress_in + link.compress_in / 64);
        assert_eq!(link.window_size, 15);
    }
}

#[test]
fn v8_to_v22bis_with_lapm_and_no_compression() {
    // One end does not compress: neither direction is.
    let answerer = ModemConfig {
        compression: false,
        ..default()
    };
    connect_and_carry(
        "no compression",
        default(),
        answerer,
        Expect::of(Modulation::V22bis, true, false),
    );
}

#[test]
fn v8_to_v22bis_with_no_error_control() {
    // The originator has none: start-stop characters at 2400 bit/s.
    let originator = ModemConfig {
        error_control: ErrorControl::Off,
        ..default()
    };
    connect_and_carry(
        "no error control",
        originator,
        default(),
        Expect::of(Modulation::V22bis, false, false),
    );
}

#[test]
fn a_caller_with_no_v8_is_answered_at_v22bis() {
    let call = connect_and_carry(
        "V.22bis by the fallback",
        only(&[Modulation::V22bis]),
        default(),
        Expect::of(Modulation::V22bis, true, true),
    );
    // ANSam for its time with no CM, the gap, and the answer sequence.
    assert_eq!(
        states(&call.duplex.b),
        [
            EngineState::AnswerSilence,
            EngineState::AnswerTone,
            EngineState::Gap,
            EngineState::Fallback,
            EngineState::Training,
            EngineState::Link,
            EngineState::Data,
        ]
    );
}

#[test]
fn a_v22_caller_is_answered_at_1200() {
    connect_and_carry(
        "V.22 by the fallback",
        only(&[Modulation::V22]),
        default(),
        Expect::of(Modulation::V22, true, true),
    );
}

#[test]
fn a_v22_caller_with_no_error_control_is_answered_at_1200() {
    let originator = ModemConfig {
        error_control: ErrorControl::Off,
        ..only(&[Modulation::V22])
    };
    connect_and_carry(
        "V.22 with start-stop characters",
        originator,
        default(),
        Expect::of(Modulation::V22, false, false),
    );
}

#[test]
fn a_bell_212a_caller_is_answered_at_1200() {
    // The answerer cannot tell a Bell 212A caller from a V.22 one: they
    // send the same signal. Each end calls the connection by the name of
    // its own modem.
    connect_and_carry(
        "Bell 212A by the fallback",
        only(&[Modulation::Bell212A]),
        default(),
        Expect {
            modulations: (Modulation::Bell212A, Modulation::V22),
            lapm: true,
            compression: true,
        },
    );
}

#[test]
fn a_bell_212a_answerer_is_called_at_1200() {
    // An answerer that is Bell 212A and nothing else sends the Bell
    // answering tone, and no error control: the caller, which could be
    // any modem, is that one's.
    let answerer = ModemConfig {
        error_control: ErrorControl::Off,
        ..only(&[Modulation::Bell212A])
    };
    connect_and_carry(
        "to a Bell 212A answerer",
        default(),
        answerer,
        Expect {
            modulations: (Modulation::V22, Modulation::Bell212A),
            lapm: false,
            compression: false,
        },
    );
}

#[test]
fn an_answerer_with_no_v8_is_called_at_v22bis() {
    // ANS and not ANSam, so no V.8, and then the answer sequence; and
    // the guard tone with it.
    let answerer = ModemConfig {
        v8: false,
        guard_tone: GuardTone::Hz1800,
        ..default()
    };
    connect_and_carry(
        "to an answerer with no V.8",
        default(),
        answerer,
        Expect::of(Modulation::V22bis, true, true),
    );
}

// The policies.

#[test]
fn every_policy_against_every_kind_of_peer() {
    use ErrorControl::{Auto, Off, Required};
    for (originator, answerer) in [
        (Auto, Auto),
        (Auto, Required),
        (Required, Auto),
        (Required, Required),
        (Auto, Off),
        (Off, Auto),
        (Off, Off),
        (Required, Off),
        (Off, Required),
    ] {
        for (compress_a, compress_b) in [(true, true), (true, false), (false, true)] {
            let name = format!(
                "{} calls {}, compression {compress_a} and {compress_b}",
                originator.name(),
                answerer.name()
            );
            let a = ModemConfig {
                error_control: originator,
                compression: compress_a,
                ..default()
            };
            let b = ModemConfig {
                error_control: answerer,
                compression: compress_b,
                ..default()
            };
            let lapm = originator != Off && answerer != Off;
            let cleared = (originator == Required && answerer == Off)
                || (originator == Off && answerer == Required);
            if !cleared {
                let expect = Expect::of(Modulation::V22bis, lapm, lapm && compress_a && compress_b);
                let mut call = connect(&name, a, b, expect);
                let from_a = random_bytes(3, 4096);
                let from_b = random_bytes(4, 4096);
                let (at_a, at_b) = carry(&mut call, &from_a, &from_b, 60);
                assert!(at_a == from_b && at_b == from_a, "{name}");
                continue;
            }
            // Error control is required at one end and the other has
            // none: the call is cleared, and the end that required it
            // never reports a connection. Its host has NO CARRIER.
            let mut call = Call::place(a, b, &clean(), CallSettings::default());
            call.duplex
                .run_until(60 * SECOND, |d| d.a.carrier_lost() && d.b.carrier_lost());
            let (strict, other) = if originator == Required {
                (&call.duplex.a, &call.duplex.b)
            } else {
                (&call.duplex.b, &call.duplex.a)
            };
            assert!(!strict.connected(), "{name}: {:?}", strict.events);
            assert!(strict.carrier_lost(), "{name}: {:?}", strict.events);
            // The other, with no error control, had connected as soon as
            // the modems had, and then lost the carrier.
            assert!(other.connected(), "{name}: {:?}", other.events);
            assert!(other.carrier_lost(), "{name}: {:?}", other.events);
            let ended = strict.engine.diagnostics().unwrap();
            assert_eq!(ended.state, EngineState::Ended, "{name}");
            assert_eq!(ended.link.end, engine::LinkEnd::Required, "{name}");
        }
    }
}

#[test]
fn the_host_settings_of_the_call_override_the_lines() {
    // The line has error control and compression; the host, with +ES and
    // +DS, says neither for this call, and V.22 with +MS.
    let settings = CallSettings {
        modulations: Some(engine::modulations(&[Modulation::V22])),
        error_control: Some(ErrorControl::Off),
        compression: Some(Compression::OFF),
        ..CallSettings::default()
    };
    let mut call = Call::place(default(), default(), &clean(), settings);
    assert!(call.connect(60));
    let expected = Some(connection(Modulation::V22, false, false));
    assert_eq!(call.connections(), (expected.clone(), expected));

    // And the other way: the line has none, and the host asks for it.
    let line = ModemConfig {
        error_control: ErrorControl::Off,
        compression: false,
        ..default()
    };
    let settings = CallSettings {
        error_control: Some(ErrorControl::Required),
        compression: Some(Compression::BOTH),
        ..CallSettings::default()
    };
    let mut call = Call::place(line, line, &clean(), settings);
    assert!(call.connect(60));
    let expected = Some(connection(Modulation::V22bis, true, true));
    assert_eq!(call.connections(), (expected.clone(), expected));
}

#[test]
fn the_host_says_which_way_is_compressed_and_with_what() {
    // +DS=1: what the originator sends only, with a dictionary of 512
    // and strings of 6. The answerer, whose host has said nothing, agrees
    // to what is asked of it and no more.
    let asked = Compression {
        transmit: true,
        receive: false,
        codewords: 512,
        string_length: 6,
    };
    let mut a: Box<dyn engine::ModemEngine> = Box::new(engine::Modem::new(default()));
    let mut b: Box<dyn engine::ModemEngine> = Box::new(engine::Modem::new(default()));
    a.set_call(&CallSettings {
        compression: Some(asked),
        // And no detection phase: the protocol at once.
        detection: Some(false),
        ..CallSettings::default()
    });
    a.originate();
    b.answer();
    let mut call = Call {
        duplex: chansim::Duplex::new(a, b, &clean()),
    };
    assert!(call.connect(60));
    let (at_a, at_b) = call.connections();
    assert_eq!(at_a.unwrap().compression.as_deref(), Some("V.42bis TD"));
    assert_eq!(at_b.unwrap().compression.as_deref(), Some("V.42bis RD"));
    // Without the detection phase the link is up a tenth of a second
    // sooner than the 7.2 s and 7.15 s it is with it: the answerer
    // follows the flags.
    let (at_a, at_b) = call.connected_at_ms();
    assert!(at_a < 7150 && at_b < 7100, "{at_a} and {at_b}");
    let from_a = text(20_000);
    let from_b = text(20_000);
    let (got_a, got_b) = carry(&mut call, &from_a, &from_b, 200);
    assert!(got_a == from_b && got_b == from_a);
    let a = call.duplex.a.engine.diagnostics().unwrap().link;
    let b = call.duplex.b.engine.diagnostics().unwrap().link;
    assert_eq!(a.compression, (true, false));
    assert_eq!(b.compression, (false, true));
    // Compressed one way, and with so small a dictionary not by much.
    assert_eq!(a.compress_in, 20_000);
    assert!(a.compress_out < 14_000 && a.compress_out > 4_000, "{a:?}");
    assert_eq!(b.compress_in, 0);
    assert_eq!(b.decompress_out, 20_000);
}

// The data link.

/// Text that compresses.
fn text(len: usize) -> Vec<u8> {
    let words = [
        "the ",
        "modem ",
        "answers ",
        "a ",
        "call ",
        "and ",
        "sends ",
        "its ",
        "tone ",
        "to ",
        "line ",
        "frame ",
        "octet ",
        "window ",
        "of ",
        "is ",
        "carrier\r\n",
    ];
    let mut rng = chansim::Rng::new(9);
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        out.extend_from_slice(words[(rng.next_u64() % words.len() as u64) as usize].as_bytes());
    }
    out.truncate(len);
    out
}

#[test]
fn data_that_compresses_goes_faster_than_the_line() {
    let expect = Expect::of(Modulation::V22bis, true, true);
    let mut call = connect("compression", default(), default(), expect);
    let from_a = text(MIB);
    let from_b = text(MIB / 2);
    let begun = call.duplex.b.ticks;
    let (at_a, at_b) = carry(&mut call, &from_a, &from_b, 3000);
    let seconds = (call.duplex.b.ticks - begun) / SECOND;
    assert!(at_a == from_b, "the data to the originator is not intact");
    assert!(at_b == from_a, "the data to the answerer is not intact");
    // 2400 bit/s carries 300 octets a second at the very most, and so
    // 1 MiB in 3495 s; this went at more than twice that.
    assert!(seconds < 1700, "{seconds} s");
    let link = call.duplex.a.engine.diagnostics().unwrap().link;
    assert_eq!(link.compress_in, MIB as u32);
    assert!(link.compress_out < link.compress_in * 2 / 5, "{link:?}");
    assert!(link.compressed_mode.0);
    assert_eq!(link.decompress_out, (MIB / 2) as u32);
}

#[test]
fn traffic_in_bursts_is_delivered_without_waiting_for_more() {
    // What PPP does: a frame one way, an idle line, a frame or two back.
    // Nothing may wait in the compressor or in the window for data that
    // is not coming.
    let expect = Expect::of(Modulation::V22bis, true, true);
    let mut call = connect("bursts", default(), default(), expect);
    let mut rng = chansim::Rng::new(77);
    let mut sent_a = Vec::new();
    let mut sent_b = Vec::new();
    for burst in 0..120 {
        // A frame of 1 to 300 octets, half of them text and half not.
        let len = 1 + (rng.next_u64() % 300) as usize;
        let mut frame = if burst % 2 == 0 {
            text(len)
        } else {
            random_bytes(burst, len)
        };
        frame.insert(0, 0x7E);
        frame.push(0x7E);
        let from_a = burst % 3 != 1;
        let (sender, receiver, sent) = if from_a {
            (&mut call.duplex.a, &call.duplex.b, &mut sent_a)
        } else {
            (&mut call.duplex.b, &call.duplex.a, &mut sent_b)
        };
        let before = receiver.received.len();
        sender.send(&frame);
        sent.extend_from_slice(&frame);
        // The time the frame takes at 2400 bit/s if every octet of it
        // goes as a codeword of the largest size and a half, which is
        // more than data that does not compress costs before the
        // compressor has seen that it does not; and 400 ms for the two
        // modems, the compressor's flush and the line.
        let ticks = (frame.len() as u64 * 12 * SECOND).div_ceil(2400) + 40;
        let want = before + frame.len();
        let begun = call.duplex.b.ticks;
        call.duplex.run_until(begun + 20 * SECOND, |d| {
            let receiver = if from_a { &d.b } else { &d.a };
            receiver.received.len() >= want
        });
        let took = call.duplex.b.ticks - begun;
        assert!(
            took <= ticks,
            "burst {burst} of {} octets was held for {took} ticks",
            frame.len()
        );
        // An idle gap.
        call.duplex.run(rng.next_u64() % 50);
    }
    assert!(call.duplex.b.received == sent_a);
    assert!(call.duplex.a.received == sent_b);
}

#[test]
fn a_host_that_does_not_read_stops_the_far_end_and_loses_nothing() {
    let expect = Expect::of(Modulation::V22bis, true, true);
    let mut call = connect("flow control", default(), default(), expect);
    // The answerer's host stops reading. The originator's goes on
    // offering data: what the answerer cannot hold, the originator keeps.
    call.duplex.b.reading = false;
    let data = random_bytes(5, 64 * 1024);
    call.duplex.a.send(&data);
    call.duplex.run(120 * SECOND);
    assert!(call.duplex.b.received.is_empty());
    let held = data.len() - call.duplex.a.to_send.len();
    // What was taken is what the two modems hold: their queues and the
    // window. It is far less than was offered.
    assert!(held < 16 * 1024, "{held} octets were taken");
    let link = call.duplex.b.engine.diagnostics().unwrap().link;
    assert!(link.own_busy, "{link:?}");
    let link = call.duplex.a.engine.diagnostics().unwrap().link;
    assert!(link.peer_busy, "{link:?}");
    for end in [&call.duplex.a, &call.duplex.b] {
        assert!(!end.carrier_lost());
    }
    // The host reads again, and all of it arrives.
    call.duplex.b.reading = true;
    call.duplex
        .run_until(call.duplex.b.ticks + 400 * SECOND, |d| {
            d.b.received.len() >= data.len()
        });
    assert!(call.duplex.b.received == data);
    let link = call.duplex.b.engine.diagnostics().unwrap().link;
    assert!(!link.own_busy);
}

#[test]
fn hanging_up_ends_the_call_at_the_far_end() {
    let expect = Expect::of(Modulation::V22bis, true, true);
    let mut call = connect("hang up", default(), default(), expect);
    call.duplex.a.engine.hangup();
    assert_eq!(
        call.duplex.a.engine.diagnostics().unwrap().state,
        EngineState::Idle
    );
    assert_eq!(call.duplex.a.engine.dte_write(b"data"), 0);
    // The carrier is gone for S10, 1.4 s, and the answerer reports it.
    call.duplex.run(3 * SECOND);
    assert!(call.duplex.b.carrier_lost());
    let lost = call
        .duplex
        .b
        .first(|event| matches!(event, ModemEvent::CarrierLost))
        .unwrap();
    let connected = call
        .duplex
        .b
        .first(|event| matches!(event, ModemEvent::Connect(_)))
        .unwrap();
    assert!(lost.tick > connected.tick);
    assert_eq!(
        call.duplex.b.engine.diagnostics().unwrap().state,
        EngineState::Ended
    );
    // The engine is used again for the next call.
    call.duplex.b.engine.answer();
    assert_eq!(
        call.duplex.b.engine.diagnostics().unwrap().state,
        EngineState::AnswerSilence
    );
    assert_eq!(
        call.duplex.b.engine.diagnostics().unwrap().link.stage,
        LinkStage::Idle
    );
}

// Every pairing.

/// What two modems that try the modem at 1200 and 2400 bit/s come to:
/// 2400 bit/s if both are V.22bis, and 1200 bit/s if each has a modem at
/// 1200 bit/s, which each calls by its own name. A modem that is V.22bis
/// and nothing else does not work at 1200 bit/s.
fn handshake(a: &[Modulation], b: &[Modulation]) -> Option<(Modulation, Modulation)> {
    let has = |list: &[Modulation], modulation| list.contains(&modulation);
    if has(a, Modulation::V22bis) && has(b, Modulation::V22bis) {
        return Some((Modulation::V22bis, Modulation::V22bis));
    }
    let at_1200 = |list: &[Modulation]| {
        if has(list, Modulation::V22) {
            Some(Modulation::V22)
        } else if has(list, Modulation::Bell212A) {
            Some(Modulation::Bell212A)
        } else {
            None
        }
    };
    Some((at_1200(a)?, at_1200(b)?))
}

/// What a call between two modems is specified to come to: the
/// modulation each end reports, or no connection.
fn specified(
    a: &[Modulation],
    b: &[Modulation],
    v8_a: bool,
    v8_b: bool,
) -> Option<(Modulation, Modulation)> {
    use Modulation::{Bell103, V21};
    let has = |list: &[Modulation], modulation| list.contains(&modulation);
    let fast = |list: &[Modulation]| {
        has(list, Modulation::V22bis)
            || has(list, Modulation::V22)
            || has(list, Modulation::Bell212A)
    };
    // The menu of V.8: V.22bis and V.22 are one entry, V.21 another, and
    // the Bell modems are not in it.
    let in_menu = |list: &[Modulation]| {
        (
            has(list, Modulation::V22bis) || has(list, Modulation::V22),
            has(list, Modulation::V21),
        )
    };
    let (menu_a, menu_b) = (in_menu(a), in_menu(b));
    let uses = |v8: bool, menu: (bool, bool)| v8 && (menu.0 || menu.1);
    if uses(v8_a, menu_a) && uses(v8_b, menu_b) {
        // V.8 decides, and there is nothing after it: the highest mode
        // that both have, or no call (V.8 clause 8.2.3).
        return if menu_a.0 && menu_b.0 {
            handshake(a, b)
        } else if menu_a.1 && menu_b.1 {
            Some((Modulation::V21, Modulation::V21))
        } else {
            None
        };
    }
    // Without V.8 the answerer takes each kind of modem it may be in
    // turn, in the order of its list, and listens in every step for the
    // calling carrier of any of them.
    let mut steps: Vec<Option<Modulation>> = Vec::new();
    for &modulation in b {
        let step = (!modulation.synchronous()).then_some(modulation);
        if !steps.contains(&step) {
            steps.push(step);
        }
    }
    let first_fsk = a
        .iter()
        .copied()
        .find(|modulation| !modulation.synchronous());
    if !fast(a) {
        // A caller that is a 300 bit/s modem only. It knows the answerer
        // by its carrier: V.21's, or Bell 103's at mark, for which it
        // also takes the answer of a modem at 1200 or 2400 bit/s, which
        // is the same tone or 25 Hz from it. If the carrier is of no
        // modem it may be, it sends the calling carrier of its first
        // modulation after a second (V.25 clause 3.10), and stays with
        // it.
        let heard = match steps.first().copied().flatten() {
            Some(V21) => V21,
            _ => Bell103,
        };
        let its = if has(a, heard) { heard } else { first_fsk? };
        return has(b, its).then_some((its, its));
    }
    // A caller that may be a modem at 1200 or 2400 bit/s takes the
    // answer of one, and the carrier of Bell 103 with it, for that
    // modem's and begins its handshake. An answerer that is one answers
    // in whatever step it is, and what the two come to is the call's: no
    // rate in common is the end of it. If nothing answers, the caller is
    // Bell 103 if it may be, and if not it listens on for V.21's
    // carrier.
    for step in steps {
        match step {
            None => return handshake(a, b),
            Some(Bell103) if fast(b) => return handshake(a, b),
            Some(Bell103) if has(a, Bell103) => return Some((Bell103, Bell103)),
            Some(V21) if has(a, V21) => return Some((V21, V21)),
            Some(_) => {}
        }
    }
    None
}

#[test]
fn every_pairing_of_modulations_reaches_its_mode() {
    use Modulation::{Bell103, Bell212A, V21, V22, V22bis};
    let lists: [&[Modulation]; 9] = [
        &[V22bis, V22, Bell212A, V21, Bell103],
        &[V22bis],
        &[V22],
        &[Bell212A],
        &[V21],
        &[Bell103],
        &[V22bis, V21],
        &[V22, Bell212A, Bell103],
        &[Bell103, V21, V22bis],
    ];
    let mut connected = 0;
    let mut failures = Vec::new();
    for a in lists {
        for b in lists {
            for (v8_a, v8_b) in [(true, true), (false, true), (true, false)] {
                let name = format!("{a:?} v8 {v8_a} calls {b:?} v8 {v8_b}");
                let expected = specified(a, b, v8_a, v8_b);
                // Error control is off: this is about the modems.
                let modem = |list: &[Modulation], v8: bool| ModemConfig {
                    error_control: ErrorControl::Off,
                    ..config(list, v8)
                };
                let mut call = Call::place(
                    modem(a, v8_a),
                    modem(b, v8_b),
                    &clean(),
                    CallSettings::default(),
                );
                let both = call.connect(60);
                let got = match call.connections() {
                    (Some(at_a), Some(at_b)) if both => Some((at_a, at_b)),
                    _ => None,
                };
                let wanted = expected.map(|(at_a, at_b)| {
                    (
                        connection(at_a, false, false),
                        connection(at_b, false, false),
                    )
                });
                if got != wanted {
                    failures.push(format!("{name}: {got:?}, not {wanted:?}"));
                    continue;
                }
                if got.is_none() {
                    continue;
                }
                connected += 1;
                // And a little data each way.
                let from_a = random_bytes(7, 600);
                let from_b = random_bytes(8, 600);
                let (at_a, at_b) = carry(&mut call, &from_a, &from_b, 60);
                if at_a != from_b || at_b != from_a {
                    failures.push(format!("{name}: the data is not intact"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(connected > 100, "{connected} calls connected");
}
