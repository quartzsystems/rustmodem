// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `rtp_packet`: no datagram makes the RTP parser panic, a
//! packet that parses is the same packet when it is written again, and no
//! run of datagrams makes the receive session panic or deliver more than
//! it may.
//!
//! The input is read as a run of datagrams, each with a one-octet length
//! before it.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rtp::{HEADER_LEN, RtpPacket, RtpReceiver, Verdict};

fuzz_target!(|data: &[u8]| {
    let mut receiver = RtpReceiver::new(0);
    let mut rest = data;
    let mut arrival = 0u32;
    let mut scratch = [0u8; HEADER_LEN + 255];

    while let Some((len, after)) = rest.split_first() {
        let take = usize::from(*len).min(after.len());
        let (datagram, after) = after.split_at(take);
        rest = after;
        arrival = arrival.wrapping_add(160);

        let Ok(packet) = RtpPacket::parse(datagram) else {
            continue;
        };
        assert!(packet.payload.len() <= datagram.len() - HEADER_LEN);

        let written = packet
            .header
            .write_packet(packet.payload, &mut scratch)
            .expect("a payload no longer than its datagram fits");
        let again = RtpPacket::parse(&scratch[..written]).expect("what was written parses");
        assert_eq!(again, packet);

        let before = receiver.stats();
        match receiver.receive(&packet.header, packet.payload.len(), arrival) {
            Verdict::Deliver {
                fill_frames,
                frames,
                resync,
            } => {
                assert_eq!(frames as usize * 80, packet.payload.len());
                assert!(fill_frames <= 300);
                assert!(!resync || fill_frames == 0);
                assert_eq!(receiver.stats().packets, before.packets + 1);
            }
            Verdict::Drop(_) => assert_eq!(receiver.stats().packets, before.packets),
        }
    }
});
