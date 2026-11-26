// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The peer's RTP by itself, on a clock the test turns.

use engine::{ModemEngine, NullConfig, NullEngine};
use rtp::{HEADER_LEN, RtpPacket};
use sdp::Codec;
use sippeer::{Impairments, MediaStream, Source, pattern_frame};

const MS: u64 = 1000;

fn stream(source: Source, impairments: Impairments) -> MediaStream {
    MediaStream::new(0, Codec::Pcmu, 20, 7, source, impairments)
}

/// Everything the stream sends in `ms` milliseconds, a millisecond a
/// turn.
fn sent_in(stream: &mut MediaStream, ms: u64) -> Vec<Vec<u8>> {
    (0..=ms).flat_map(|at| stream.poll(at * MS)).collect()
}

fn sequence(datagram: &[u8]) -> u16 {
    RtpPacket::parse(datagram).unwrap().header.sequence
}

#[test]
fn no_frame_of_the_pattern_reads_as_silence_and_no_two_are_alike() {
    let frames: Vec<_> = (0..2000).map(|index| pattern_frame(9, index)).collect();
    for frame in &frames {
        assert!(!frame.contains(&0xFF) && !frame.contains(&0xD5));
    }
    let mut sorted = frames.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), frames.len());
    assert_ne!(pattern_frame(9, 0), pattern_frame(10, 0));
}

#[test]
fn a_packet_is_made_every_packet_time() {
    let mut stream = stream(Source::Silence, Impairments::default());
    assert_eq!(stream.next_due_us(), 0);
    let sent = sent_in(&mut stream, 1000);
    assert_eq!(sent.len(), 51, "at 0, 20, ... 1000 ms");
    assert_eq!(stream.next_due_us(), 1020 * MS);
    for (n, datagram) in sent.iter().enumerate() {
        let packet = RtpPacket::parse(datagram).unwrap();
        assert_eq!(datagram.len(), HEADER_LEN + 160);
        assert_eq!(packet.header.payload_type, 0);
        assert_eq!(
            packet.header.sequence,
            sequence(&sent[0]).wrapping_add(n as u16)
        );
        assert!(packet.payload.iter().all(|octet| *octet == 0xFF));
    }
}

#[test]
fn a_fast_clock_makes_more_packets_and_a_slow_one_fewer() {
    let count = |clock_ppm| {
        let mut stream = stream(
            Source::Silence,
            Impairments {
                clock_ppm,
                ..Impairments::default()
            },
        );
        stream.poll(100_000 * MS);
        stream.report().made
    };
    assert_eq!(count(0), 5001);
    assert_eq!(count(10_000), 5051);
    assert_eq!(count(-10_000), 4951);
}

#[test]
fn the_faults_are_at_their_rates_and_the_same_from_the_same_seed() {
    let faults = Impairments {
        loss: 100,
        duplicate: 50,
        reorder: 50,
        jitter_ms: 5,
        clock_ppm: 0,
    };
    let mut first = stream(Source::Pattern(3), faults);
    let mut second = stream(Source::Pattern(3), faults);
    let sent = sent_in(&mut first, 100_000);
    assert!(sent == sent_in(&mut second, 100_000));

    let report = first.report();
    assert_eq!(report.made, 5001);
    assert!((400..600).contains(&report.lost), "{report:?}");
    assert!((170..330).contains(&report.duplicated), "{report:?}");
    assert!((170..330).contains(&report.reordered), "{report:?}");
    // What is still held back at the end has not been sent.
    let all = report.made - report.lost + report.duplicated;
    assert!(report.sent <= all && report.sent + 4 > all, "{report:?}");
    assert_eq!(report.sent, sent.len() as u64);

    // Packets were held back past the ones after them.
    let first_sequence = sequence(&sent[0]);
    let out_of_order = sent
        .windows(2)
        .filter(|pair| {
            sequence(&pair[1]).wrapping_sub(first_sequence)
                < sequence(&pair[0]).wrapping_sub(first_sequence)
        })
        .count();
    assert!(out_of_order > 100, "{out_of_order} out of order");
}

#[test]
fn a_pattern_that_comes_back_is_counted_and_anything_else_is_not() {
    let mut stream = stream(Source::Pattern(11), Impairments::default());
    let sent = sent_in(&mut stream, 2000);
    // A far end that begins with silence, then echoes all but one packet,
    // which it loses, and spoils one.
    let mut at = 2000 * MS;
    let mut back = Vec::new();
    let silence = {
        let mut datagram = sent[0].clone();
        datagram[HEADER_LEN..].fill(0xFF);
        datagram
    };
    back.push(silence);
    back.extend(sent.iter().skip(1).cloned());
    back.remove(40);
    back[60][HEADER_LEN + 3] ^= 0x10;
    for datagram in back {
        at += 20 * MS;
        stream.receive(at, &datagram);
    }
    let report = stream.report();
    assert_eq!(report.received.packets, 100);
    assert_eq!(report.received.lost, 1);
    assert_eq!(report.silent_frames, 2);
    assert_eq!(report.mismatched_frames, 1);
    assert_eq!(report.echoed_frames, 200 - 2 - 1);
}

#[test]
fn a_file_is_sent_and_then_silence() {
    let octets: Vec<u8> = (0..400u32).map(|n| (n % 200) as u8).collect();
    let mut stream = stream(Source::File(octets.clone()), Impairments::default());
    let first = sent_in(&mut stream, 20);
    assert!(!stream.report().finished);
    let rest = sent_in(&mut stream, 100);
    assert!(stream.report().finished);
    let payload: Vec<u8> = first
        .iter()
        .chain(&rest)
        .flat_map(|datagram| datagram[HEADER_LEN..].to_vec())
        .collect();
    assert_eq!(payload[..400], octets[..]);
    assert!(payload[400..].iter().all(|octet| *octet == 0xFF));
}

#[test]
fn two_streams_with_null_engines_carry_data_between_them() {
    let engine = |originate: bool| {
        let mut engine = NullEngine::new(NullConfig::default());
        if originate {
            engine.originate();
        } else {
            engine.answer();
        }
        Source::Engine(Box::new(engine))
    };
    let mut caller = stream(engine(true), Impairments::default());
    let mut callee = stream(engine(false), Impairments::default());
    let down: Vec<u8> = (0..5000u32).map(|n| (n * 31 + n / 255) as u8).collect();
    let up: Vec<u8> = (0..5000u32).map(|n| (n * 17 + n / 255) as u8).collect();
    let (mut sent_down, mut sent_up) = (0, 0);
    let (mut got_down, mut got_up) = (Vec::new(), Vec::new());
    let mut buffer = [0u8; 512];
    for ms in 0..4000 {
        let now = ms * MS;
        sent_down += caller.engine().unwrap().dte_write(&down[sent_down..]);
        sent_up += callee.engine().unwrap().dte_write(&up[sent_up..]);
        for datagram in caller.poll(now) {
            callee.receive(now, &datagram);
        }
        for datagram in callee.poll(now) {
            caller.receive(now, &datagram);
        }
        let len = callee.engine().unwrap().dte_read(&mut buffer);
        got_down.extend_from_slice(&buffer[..len]);
        let len = caller.engine().unwrap().dte_read(&mut buffer);
        got_up.extend_from_slice(&buffer[..len]);
    }
    assert!(got_down == down);
    assert!(got_up == up);
    assert!(
        stream(Source::Silence, Impairments::default())
            .engine()
            .is_none()
    );
}
