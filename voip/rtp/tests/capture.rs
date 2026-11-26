// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The capture file: written and read, damaged, checked, and turned into
//! sound.

use proptest::prelude::*;
use rtp::RtpHeader;
use rtp::capture::{
    CaptureError, Header, Kind, MAX_PAYLOAD, Reader, Record, Report, Writer, check, write_wav,
};

fn header() -> Header {
    Header {
        payload_type: 0,
        ptime_ms: 20,
        line: 7,
        started_unix_ms: 1_800_000_000_123,
        call_id: "3b1c9e@10.10.20.11".to_owned(),
    }
}

/// Packet `n` of a stream of 20 ms packets: its header.
fn rtp(n: u32, ssrc: u32) -> RtpHeader {
    RtpHeader {
        marker: n == 0,
        payload_type: 0,
        sequence: (1000 + n) as u16,
        timestamp: 160 * n,
        ssrc,
    }
}

/// The far end's packet `n`, arriving at its time, filled with `n`.
fn received(n: u32) -> Record {
    Record::new(
        Kind::Received,
        u64::from(n) * 20_000_000,
        rtp(n, 0xFA),
        &[n as u8; 160],
    )
}

/// Our packet `n`, sent a moment after packet `at` arrived.
fn sent(n: u32, at: u32) -> Record {
    Record::new(
        Kind::Sent,
        u64::from(at) * 20_000_000 + 5000,
        rtp(n, 0x0A),
        &[0x80 | n as u8; 160],
    )
}

fn file(header: &Header, records: &[Record]) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new(), header).unwrap();
    for record in records {
        writer.record(record).unwrap();
    }
    writer.flush().unwrap();
    writer.into_inner()
}

fn report(header: &Header, records: &[Record]) -> Report {
    let bytes = file(header, records);
    check(&mut Reader::new(&bytes[..]).unwrap()).unwrap()
}

/// A call in which every packet that arrives is answered with one.
fn steady(packets: u32) -> Vec<Record> {
    (0..packets)
        .flat_map(|n| [received(n), sent(n, n)])
        .collect()
}

#[test]
fn what_is_written_is_what_is_read() {
    let records = [
        received(0),
        sent(0, 0),
        Record::gap(123_456_789, 17),
        Record::new(Kind::Received, u64::MAX, rtp(9, u32::MAX), &[]),
        Record::new(Kind::Sent, 1, rtp(3, 5), &[0xD5; 240]),
        Record::new(Kind::Sent, 2, rtp(4, 5), &[1; 80]),
    ];
    let bytes = file(&header(), &records);
    // The layout of `docs/CAPTURE.md`: a header of 22 octets and the call
    // id, then records of 24 octets and their payload.
    assert_eq!(&bytes[..6], b"RMCAP\0");
    assert_eq!(bytes[6..8], [1, 0]);
    assert_eq!(bytes[8..10], [0, 20]);
    assert_eq!(bytes[10..12], [7, 0]);
    assert_eq!(bytes[20..22], [18, 0]);
    assert_eq!(&bytes[22..40], b"3b1c9e@10.10.20.11");
    assert_eq!(bytes.len(), 40 + 6 * 24 + 160 + 160 + 240 + 80);

    let mut reader = Reader::new(&bytes[..]).unwrap();
    assert_eq!(*reader.header(), header());
    for record in &records {
        assert_eq!(reader.record().unwrap().as_ref(), Some(record));
    }
    assert!(reader.record().unwrap().is_none());
    assert!(reader.record().unwrap().is_none(), "the end stays the end");

    assert_eq!(records[2].dropped(), 17);
    assert_eq!(records[4].payload(), [0xD5; 240]);
    // A payload longer than a record holds is cut.
    let long = Record::new(Kind::Sent, 0, rtp(0, 0), &[9; 500]);
    assert_eq!(long.payload().len(), MAX_PAYLOAD);
}

#[test]
fn what_is_not_a_capture() {
    let good = file(&header(), &steady(2));

    for bytes in [
        &b""[..],
        b"RMCAP",
        b"RIFF....WAVEfmt ........",
        &[0u8; 64][..],
    ] {
        assert!(
            matches!(Reader::new(bytes), Err(CaptureError::NotACapture)),
            "{bytes:?}"
        );
    }
    let mut newer = good.clone();
    newer[6] = 2;
    assert!(matches!(
        Reader::new(&newer[..]),
        Err(CaptureError::Version(2))
    ));

    // Cut off inside the call id.
    assert!(matches!(
        Reader::new(&good[..30]),
        Err(CaptureError::Malformed(22))
    ));
    // A call id that is not text.
    let mut not_text = good.clone();
    not_text[25] = 0xFF;
    assert!(matches!(
        Reader::new(&not_text[..]),
        Err(CaptureError::Malformed(22))
    ));

    // Cut off inside a record, and inside its payload.
    for cut in [41, 50, 63, 64, 100, 223] {
        let mut reader = Reader::new(&good[..cut]).unwrap();
        let mut outcome = reader.record();
        while let Ok(Some(_)) = outcome {
            outcome = reader.record();
        }
        assert!(
            matches!(outcome, Err(CaptureError::Malformed(_))),
            "cut at {cut}: {outcome:?}"
        );
    }
    // Cut off between records is a capture that ends there.
    let mut reader = Reader::new(&good[..40 + 184]).unwrap();
    assert!(reader.record().unwrap().is_some());
    assert!(reader.record().unwrap().is_none());

    // A kind that does not exist, and a payload longer than there can be.
    let mut bad_kind = good.clone();
    bad_kind[40] = 9;
    assert!(matches!(
        Reader::new(&bad_kind[..]).unwrap().record(),
        Err(CaptureError::Malformed(40))
    ));
    let mut too_long = good.clone();
    too_long[40 + 22..40 + 24].copy_from_slice(&1000u16.to_le_bytes());
    assert!(matches!(
        Reader::new(&too_long[..]).unwrap().record(),
        Err(CaptureError::Malformed(40))
    ));
    assert_eq!(
        CaptureError::Malformed(40).to_string(),
        "the capture is damaged at octet 40"
    );
}

#[test]
fn a_call_in_which_every_frame_is_answered_is_clean() {
    let found = report(&header(), &steady(500));
    assert_eq!(
        found,
        Report {
            received: 500,
            sent: 500,
            largest_gap_us: 20_000,
            ..Report::default()
        }
    );
    assert!(found.is_clean());
    assert!(Report::default().is_clean());
}

#[test]
fn frames_sent_before_the_far_end_begins_are_the_local_timers() {
    let mut records = vec![sent(0, 0), sent(1, 1), sent(2, 2)];
    records.extend((0..10).flat_map(|n| [received(n), sent(n + 3, n)]));
    let found = report(&header(), &records);
    assert_eq!((found.free_run, found.slips, found.sent), (6, 0, 13));
    assert!(found.is_clean());
}

#[test]
fn loss_is_counted_and_what_was_filled_is_not_a_slip() {
    // Packets 3 and 4 never arrive. Packet 5 is answered with three
    // packets: two for the fill and its own.
    let mut records: Vec<Record> = (0..3).flat_map(|n| [received(n), sent(n, n)]).collect();
    records.extend([received(5), sent(3, 5), sent(4, 5), sent(5, 5)]);
    records.extend((6..10).flat_map(|n| [received(n), sent(n, n)]));
    let found = report(&header(), &records);
    assert_eq!((found.lost, found.received, found.sent), (2, 8, 10));
    assert_eq!(found.slips, 0);
    assert_eq!(found.largest_gap_us, 60_000);
    assert!(!found.is_clean(), "a lost packet is not a clean call");
}

#[test]
fn late_and_doubled_packets_are_counted_and_answered_with_nothing() {
    let mut records: Vec<Record> = (0..3).flat_map(|n| [received(n), sent(n, n)]).collect();
    // 4 before 3: 3 is filled, then arrives late; 4 comes twice.
    records.extend([
        received(4),
        sent(3, 4),
        sent(4, 4),
        received(3),
        received(4),
    ]);
    records.extend((5..8).flat_map(|n| [received(n), sent(n, n)]));
    let found = report(&header(), &records);
    assert_eq!((found.lost, found.late, found.duplicate), (1, 1, 1));
    assert_eq!((found.slips, found.resync), (0, 0));
}

#[test]
fn a_frame_that_is_not_answered_or_not_accounted_for_is_a_slip() {
    // A received packet with nothing sent for it: samples were lost.
    let mut records = steady(5);
    records.push(received(5));
    records.extend((6..10).flat_map(|n| [received(n), sent(n - 1, n)]));
    let found = report(&header(), &records);
    assert_eq!(found.slips, 1);
    assert!(!found.is_clean());

    // A packet sent that no received packet accounts for: samples were
    // invented.
    let mut records = steady(5);
    records.push(sent(5, 4));
    records.extend((5..10).flat_map(|n| [received(n), sent(n + 1, n)]));
    assert_eq!(report(&header(), &records).slips, 1);

    // The same at the very end of the capture.
    let mut records = steady(5);
    records.push(sent(5, 4));
    assert_eq!(report(&header(), &records).slips, 1);

    // A capture that ends with an arrival still to be answered did not
    // slip: it was only stopped there.
    let mut records = steady(5);
    records.push(received(5));
    assert_eq!(report(&header(), &records).slips, 0);
}

#[test]
fn a_new_stream_is_a_resynchronisation_and_not_a_slip() {
    let mut records = steady(5);
    // The far end starts again from other numbers.
    for n in 0..5u32 {
        let again = RtpHeader {
            sequence: 40_000 + n as u16,
            timestamp: 9_000_000 + 160 * n,
            ..rtp(n, 0xFB)
        };
        records.push(Record::new(
            Kind::Received,
            u64::from(5 + n) * 20_000_000,
            again,
            &[1; 160],
        ));
        records.push(sent(5 + n, 5 + n));
    }
    let found = report(&header(), &records);
    assert_eq!((found.resync, found.slips, found.lost), (1, 0, 0));
    assert!(!found.is_clean());
}

#[test]
fn other_packet_times() {
    // 10 ms each way.
    let mut ten = header();
    ten.ptime_ms = 10;
    let records: Vec<Record> = (0..50u32)
        .flat_map(|n| {
            let rx = RtpHeader {
                timestamp: 80 * n,
                ..rtp(n, 0xFA)
            };
            [
                Record::new(Kind::Received, u64::from(n) * 10_000_000, rx, &[0; 80]),
                Record::new(
                    Kind::Sent,
                    u64::from(n) * 10_000_000 + 1,
                    rtp(n, 1),
                    &[0; 80],
                ),
            ]
        })
        .collect();
    assert!(report(&ten, &records).is_clean());

    // 30 ms in, 20 ms out: a frame waits for the next arrival every other
    // time, and that is not a slip.
    let mut records = Vec::new();
    let mut out = 0;
    let mut waiting = 0;
    for n in 0..40u32 {
        let rx = RtpHeader {
            timestamp: 240 * n,
            ..rtp(n, 0xFA)
        };
        records.push(Record::new(
            Kind::Received,
            u64::from(n) * 30_000_000,
            rx,
            &[0; 240],
        ));
        waiting += 3;
        while waiting >= 2 {
            records.push(sent(out, n));
            out += 1;
            waiting -= 2;
        }
    }
    let found = report(&header(), &records);
    assert_eq!((found.slips, found.received, found.sent), (0, 40, 60));
}

#[test]
fn a_hole_in_the_capture_is_reported_and_the_count_starts_again() {
    let mut records = steady(5);
    records.push(Record::gap(100_000_000, 40));
    // What follows the hole is another stretch of the same call.
    records.extend((30..40).flat_map(|n| [received(n), sent(n, n)]));
    let found = report(&header(), &records);
    assert_eq!(found.dropped_records, 40);
    // Not counted as loss or as a slip: it is the capture that has the
    // hole, not the call.
    assert_eq!((found.lost, found.slips, found.resync), (0, 0, 0));
    assert!(!found.is_clean(), "but it cannot be called clean either");
}

#[test]
fn a_capture_as_sound() {
    // Our timer sends one packet before the far end begins; packet 2 of
    // the far end is lost.
    let records = [
        sent(0, 0),
        received(0),
        sent(1, 0),
        received(1),
        sent(2, 1),
        received(3),
        sent(3, 3),
        sent(4, 3),
    ];
    let bytes = file(&header(), &records);
    let mut wav = Vec::new();
    let frames = write_wav(&mut Reader::new(&bytes[..]).unwrap(), &mut wav).unwrap();
    assert_eq!(frames, 10);

    // RIFF, WAVE, a non-PCM format chunk for mu-law at 8000 Hz in stereo.
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
        wav.len() - 8
    );
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(wav[20..22], [7, 0], "format tag 7 is mu-law");
    assert_eq!(wav[22..24], [2, 0], "two channels");
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 8000);
    assert_eq!(&wav[38..42], b"fact");
    assert_eq!(u32::from_le_bytes(wav[46..50].try_into().unwrap()), 800);
    assert_eq!(&wav[50..54], b"data");
    assert_eq!(u32::from_le_bytes(wav[54..58].try_into().unwrap()), 1600);
    assert_eq!(wav.len(), 58 + 1600);

    let data = &wav[58..];
    let left: Vec<u8> = data.iter().step_by(2).copied().collect();
    let right: Vec<u8> = data.iter().skip(1).step_by(2).copied().collect();
    // Left: silence under what the timer sent, packets 0 and 1, silence
    // where packet 2 was lost, packet 3.
    let mut expected_left = vec![0xFF; 160];
    expected_left.extend([0u8; 160]);
    expected_left.extend([1u8; 160]);
    expected_left.extend([0xFFu8; 160]);
    expected_left.extend([3u8; 160]);
    assert!(left == expected_left);
    // Right: our five packets, end to end.
    let expected_right: Vec<u8> = (0..5u8).flat_map(|n| [0x80 | n; 160]).collect();
    assert!(right == expected_right);

    // A-law: format tag 6, and its own silence.
    let mut alaw = header();
    alaw.payload_type = 8;
    let record = |kind, n: u32| {
        let rtp = RtpHeader {
            payload_type: 8,
            ..rtp(n, 0xFA)
        };
        Record::new(kind, u64::from(n) * 20_000_000, rtp, &[0x55; 160])
    };
    let bytes = file(
        &alaw,
        &[record(Kind::Received, 0), record(Kind::Received, 2)],
    );
    let mut wav = Vec::new();
    write_wav(&mut Reader::new(&bytes[..]).unwrap(), &mut wav).unwrap();
    assert_eq!(wav[20..22], [6, 0]);
    let data = &wav[58..];
    assert_eq!(data[320], 0xD5, "the lost packet, on the left");
    assert_eq!(data[1], 0xD5, "nothing was sent, on the right");
}

fn any_record() -> impl Strategy<Value = Record> {
    (
        prop::sample::select(vec![Kind::Received, Kind::Sent, Kind::Gap]),
        any::<u64>(),
        (
            any::<bool>(),
            0u8..128,
            any::<u16>(),
            any::<u32>(),
            any::<u32>(),
        ),
        proptest::collection::vec(any::<u8>(), 0..=MAX_PAYLOAD),
    )
        .prop_map(
            |(kind, nanos, (marker, payload_type, sequence, timestamp, ssrc), payload)| {
                let header = RtpHeader {
                    marker,
                    payload_type,
                    sequence,
                    timestamp,
                    ssrc,
                };
                Record::new(kind, nanos, header, &payload)
            },
        )
}

proptest! {
    #[test]
    fn any_capture_written_is_read_back_the_same(
        payload_type in 0u8..128,
        ptime_ms: u8,
        line: u16,
        started_unix_ms: u64,
        call_id in "[ -~]{0,60}",
        records in proptest::collection::vec(any_record(), 0..30),
    ) {
        let header = Header { payload_type, ptime_ms, line, started_unix_ms, call_id };
        let bytes = file(&header, &records);
        let mut reader = Reader::new(&bytes[..]).unwrap();
        prop_assert_eq!(reader.header(), &header);
        for record in &records {
            let read = reader.record().unwrap();
            prop_assert_eq!(read.as_ref(), Some(record));
        }
        prop_assert!(reader.record().unwrap().is_none());
        // Checking it and making sound of it never panic, whatever is in it.
        let _ = check(&mut Reader::new(&bytes[..]).unwrap());
        let _ = write_wav(&mut Reader::new(&bytes[..]).unwrap(), &mut Vec::new());
    }

    #[test]
    fn no_octets_make_the_reader_panic(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let mut with_magic = b"RMCAP\0\x01\0".to_vec();
        with_magic.extend(&bytes);
        for input in [&bytes[..], &with_magic[..]] {
            if let Ok(mut reader) = Reader::new(input) {
                while let Ok(Some(_)) = reader.record() {}
            }
        }
    }
}
