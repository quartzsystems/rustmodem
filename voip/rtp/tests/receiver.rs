// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The receive session, case by case.

use rtp::{Dropped, RESYNC_SAMPLES, RtpHeader, RtpReceiver, Verdict};

const SSRC: u32 = 0x1111_2222;

/// Packet `n` of a 20 ms stream that starts at sequence 100, timestamp 5000.
fn packet(n: u32) -> RtpHeader {
    RtpHeader {
        marker: n == 0,
        payload_type: 0,
        sequence: (100 + n) as u16,
        timestamp: 5000 + 160 * n,
        ssrc: SSRC,
    }
}

fn deliver(fill_frames: u32, frames: u32) -> Verdict {
    Verdict::Deliver {
        fill_frames,
        frames,
        resync: false,
    }
}

fn resync(frames: u32) -> Verdict {
    Verdict::Deliver {
        fill_frames: 0,
        frames,
        resync: true,
    }
}

/// A receiver that has taken packets 0 to `n - 1` in order.
fn after(n: u32) -> RtpReceiver {
    let mut receiver = RtpReceiver::new(0);
    for i in 0..n {
        assert_eq!(receiver.receive(&packet(i), 160, 160 * i), deliver(0, 2));
    }
    receiver
}

#[test]
fn packets_in_order_are_delivered() {
    let receiver = after(50);
    let stats = receiver.stats();
    assert_eq!(stats.packets, 50);
    assert_eq!(stats.octets, 8000);
    assert_eq!(
        (
            stats.lost,
            stats.late,
            stats.duplicate,
            stats.resync,
            stats.ignored
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(stats.fill_frames, 0);
}

#[test]
fn the_first_packet_starts_the_stream_wherever_it_is() {
    let mut receiver = RtpReceiver::new(0);
    assert!(!receiver.is_synchronised());
    assert_eq!(receiver.receive(&packet(7), 160, 0), deliver(0, 2));
    assert!(receiver.is_synchronised());
    assert_eq!(receiver.receive(&packet(8), 160, 160), deliver(0, 2));
    assert_eq!(receiver.stats().resync, 0);
}

#[test]
fn a_payload_is_delivered_as_frames_of_80_octets() {
    for (len, frames) in [(80, 1), (160, 2), (240, 3), (480, 6)] {
        let mut receiver = RtpReceiver::new(0);
        assert_eq!(receiver.receive(&packet(0), len, 0), deliver(0, frames));
    }
}

#[test]
fn a_payload_that_is_not_whole_frames_is_ignored() {
    let mut receiver = after(1);
    for len in [0, 1, 79, 81, 200, 65_600] {
        assert_eq!(
            receiver.receive(&packet(1), len, 0),
            Verdict::Drop(Dropped::Ignored)
        );
    }
    assert_eq!(receiver.stats().ignored, 6);
    // The stream goes on as if they had not been sent.
    assert_eq!(receiver.receive(&packet(1), 160, 160), deliver(0, 2));
}

#[test]
fn a_gap_is_filled_before_the_packet() {
    let mut receiver = after(3);
    // Packets 3, 4 and 5 are lost: six frames of fill, then packet 6.
    assert_eq!(receiver.receive(&packet(6), 160, 960), deliver(6, 2));
    assert_eq!(receiver.stats().lost, 3);
    assert_eq!(receiver.stats().fill_frames, 6);
    assert_eq!(receiver.receive(&packet(7), 160, 1120), deliver(0, 2));
}

#[test]
fn a_duplicate_is_dropped() {
    let mut receiver = after(3);
    assert_eq!(
        receiver.receive(&packet(2), 160, 0),
        Verdict::Drop(Dropped::Duplicate)
    );
    assert_eq!(
        receiver.receive(&packet(0), 160, 0),
        Verdict::Drop(Dropped::Duplicate)
    );
    assert_eq!(receiver.stats().duplicate, 2);
    assert_eq!(receiver.stats().packets, 3);
    assert_eq!(receiver.receive(&packet(3), 160, 480), deliver(0, 2));
}

#[test]
fn a_packet_that_arrives_after_its_successor_is_late_and_was_filled() {
    let mut receiver = after(3);
    // 4 arrives before 3: the place of 3 is filled.
    assert_eq!(receiver.receive(&packet(4), 160, 640), deliver(2, 2));
    assert_eq!(
        receiver.receive(&packet(3), 160, 650),
        Verdict::Drop(Dropped::Late)
    );
    // A second copy of it is late too: it was never delivered.
    assert_eq!(
        receiver.receive(&packet(3), 160, 660),
        Verdict::Drop(Dropped::Late)
    );
    // And 4 again is a duplicate.
    assert_eq!(
        receiver.receive(&packet(4), 160, 670),
        Verdict::Drop(Dropped::Duplicate)
    );
    let stats = receiver.stats();
    assert_eq!((stats.lost, stats.late, stats.duplicate), (1, 2, 1));
    assert_eq!(receiver.receive(&packet(5), 160, 800), deliver(0, 2));
}

#[test]
fn a_packet_older_than_the_stream_is_late() {
    let mut receiver = RtpReceiver::new(0);
    assert_eq!(receiver.receive(&packet(5), 160, 0), deliver(0, 2));
    assert_eq!(
        receiver.receive(&packet(4), 160, 10),
        Verdict::Drop(Dropped::Late)
    );
}

#[test]
fn what_is_remembered_of_the_past_is_64_packets() {
    let mut receiver = after(100);
    assert_eq!(
        receiver.receive(&packet(36), 160, 0),
        Verdict::Drop(Dropped::Duplicate)
    );
    // Further back than that, a copy cannot be told from a late packet.
    assert_eq!(
        receiver.receive(&packet(35), 160, 0),
        Verdict::Drop(Dropped::Late)
    );
}

#[test]
fn another_ssrc_starts_the_stream_again() {
    let mut receiver = after(3);
    let other = RtpHeader {
        ssrc: 0x9999_0000,
        sequence: 7,
        timestamp: 123_456,
        ..packet(0)
    };
    assert_eq!(receiver.receive(&other, 160, 480), resync(2));
    assert_eq!(receiver.stats().resync, 1);
    let next = RtpHeader {
        sequence: 8,
        timestamp: 123_616,
        ..other
    };
    assert_eq!(receiver.receive(&next, 160, 640), deliver(0, 2));
    // The old stream is now the stranger.
    assert_eq!(receiver.receive(&packet(3), 160, 650), resync(2));
}

#[test]
fn a_jump_of_more_than_three_seconds_starts_the_stream_again() {
    // Exactly three seconds is still a gap: 150 packets of 20 ms.
    let mut receiver = after(1);
    assert_eq!(
        receiver.receive(&packet(151), 160, 0),
        deliver(RESYNC_SAMPLES / 80, 2)
    );
    assert_eq!(receiver.stats().lost, 150);

    let mut receiver = after(1);
    assert_eq!(receiver.receive(&packet(152), 160, 0), resync(2));
    assert_eq!(receiver.stats().lost, 0);
    assert_eq!(receiver.stats().resync, 1);
    assert_eq!(receiver.receive(&packet(153), 160, 160), deliver(0, 2));

    // Backwards as well, whatever the sequence number says.
    let mut receiver = after(1);
    let back = RtpHeader {
        timestamp: 5000u32.wrapping_sub(RESYNC_SAMPLES + 80),
        sequence: 50,
        ..packet(0)
    };
    assert_eq!(receiver.receive(&back, 160, 0), resync(2));
}

#[test]
fn a_sample_clock_that_stops_or_leaves_the_grid_starts_the_stream_again() {
    // The sequence goes on and the timestamp goes back.
    let mut receiver = after(3);
    let stuck = RtpHeader {
        timestamp: packet(2).timestamp,
        ..packet(3)
    };
    assert_eq!(receiver.receive(&stuck, 160, 480), resync(2));

    // A gap that is not a whole number of frames.
    let mut receiver = after(3);
    let off = RtpHeader {
        timestamp: packet(3).timestamp + 40,
        ..packet(3)
    };
    assert_eq!(receiver.receive(&off, 160, 480), resync(2));
    assert_eq!(receiver.stats().resync, 1);
}

#[test]
fn a_timestamp_gap_with_no_packet_lost_is_filled() {
    // A far end that suppressed silence in spite of us: its clock ran on.
    let mut receiver = after(3);
    let later = RtpHeader {
        timestamp: packet(3).timestamp + 800,
        marker: true,
        ..packet(3)
    };
    assert_eq!(receiver.receive(&later, 160, 1280), deliver(10, 2));
    assert_eq!(receiver.stats().lost, 0);
    assert_eq!(receiver.stats().fill_frames, 10);
}

#[test]
fn another_payload_type_is_ignored() {
    let mut receiver = after(3);
    for payload_type in [8, 13, 101] {
        let other = RtpHeader {
            payload_type,
            ..packet(3)
        };
        assert_eq!(
            receiver.receive(&other, 160, 0),
            Verdict::Drop(Dropped::Ignored)
        );
    }
    assert_eq!(receiver.stats().ignored, 3);
    assert_eq!(receiver.receive(&packet(3), 160, 480), deliver(0, 2));

    receiver.set_payload_type(8);
    assert_eq!(receiver.payload_type(), 8);
    let alaw = RtpHeader {
        payload_type: 8,
        ..packet(4)
    };
    assert_eq!(receiver.receive(&alaw, 160, 640), deliver(0, 2));
}

#[test]
fn the_sequence_number_and_the_timestamp_wrap() {
    let mut receiver = RtpReceiver::new(0);
    let at = |n: u32| RtpHeader {
        marker: false,
        payload_type: 0,
        sequence: 0xFFFEu16.wrapping_add(n as u16),
        timestamp: 0xFFFF_FF00u32.wrapping_add(160 * n),
        ssrc: SSRC,
    };
    assert_eq!(receiver.receive(&at(0), 160, 0), deliver(0, 2));
    assert_eq!(receiver.receive(&at(1), 160, 160), deliver(0, 2));
    assert_eq!(receiver.receive(&at(2), 160, 320), deliver(0, 2));
    // One lost across the wrap.
    assert_eq!(receiver.receive(&at(4), 160, 640), deliver(2, 2));
    assert_eq!(
        receiver.receive(&at(1), 160, 650),
        Verdict::Drop(Dropped::Duplicate)
    );
    assert_eq!(
        receiver.receive(&at(3), 160, 660),
        Verdict::Drop(Dropped::Late)
    );
    assert_eq!(receiver.stats().resync, 0);
}

#[test]
fn jitter_is_the_estimator_of_rfc_3550() {
    // Packets that arrive exactly as they were sent have none.
    assert_eq!(after(100).stats().jitter, 0);
    assert_eq!(after(100).stats().jitter_max, 0);

    // Every other packet 80 samples late: |D| is 80 each time, and the
    // estimate climbs towards it a sixteenth of the way a packet.
    let mut receiver = RtpReceiver::new(0);
    let mut estimate = 0.0f64;
    for n in 0..400u32 {
        let arrival = 160 * n + if n % 2 == 1 { 80 } else { 0 };
        receiver.receive(&packet(n), 160, arrival);
        if n > 0 {
            estimate += (80.0 - estimate) / 16.0;
        }
        // The estimate is kept in sixteenths and reported in whole samples,
        // rounded down, as RFC 3550 appendix A.8 does.
        let got = f64::from(receiver.stats().jitter);
        assert!(
            (got - estimate).abs() < 2.0,
            "packet {n}: {got} and {estimate}"
        );
    }
    assert!((79..=80).contains(&receiver.stats().jitter));
    assert_eq!(receiver.stats().jitter_max, receiver.stats().jitter);

    // Then steady again: it decays, and the maximum stays.
    for n in 400..800u32 {
        receiver.receive(&packet(n), 160, 160 * n);
    }
    assert!(receiver.stats().jitter <= 1);
    assert!(receiver.stats().jitter_max >= 79);
}

#[test]
fn the_arrival_clock_may_wrap() {
    let mut receiver = RtpReceiver::new(0);
    for n in 0..10u32 {
        receiver.receive(&packet(n), 160, 0xFFFF_FE00u32.wrapping_add(160 * n));
    }
    assert_eq!(receiver.stats().jitter, 0);
}
