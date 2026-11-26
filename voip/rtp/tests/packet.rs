// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Packet parse and build, and the sender.

use proptest::prelude::*;
use rtp::{HEADER_LEN, RtpError, RtpHeader, RtpPacket, RtpSender};

const HEADER: RtpHeader = RtpHeader {
    marker: false,
    payload_type: 8,
    sequence: 0x1234,
    timestamp: 0x89AB_CDEF,
    ssrc: 0xDEAD_BEEF,
};

/// The fixed header of `HEADER` with these first two octets.
fn datagram(first: u8, second: u8, rest: &[u8]) -> Vec<u8> {
    let mut out = vec![
        first, second, 0x12, 0x34, 0x89, 0xAB, 0xCD, 0xEF, 0xDE, 0xAD, 0xBE, 0xEF,
    ];
    out.extend_from_slice(rest);
    out
}

#[test]
fn a_plain_packet() {
    let bytes = datagram(0x80, 0x08, &[1, 2, 3]);
    let packet = RtpPacket::parse(&bytes).unwrap();
    assert_eq!(packet.header, HEADER);
    assert_eq!(packet.payload, [1, 2, 3]);
}

#[test]
fn the_marker_and_the_payload_type() {
    let bytes = datagram(0x80, 0x80 | 101, &[]);
    let packet = RtpPacket::parse(&bytes).unwrap();
    assert!(packet.header.marker);
    assert_eq!(packet.header.payload_type, 101);
    assert!(packet.payload.is_empty());
}

#[test]
fn padding_is_taken_off() {
    // P set: the last octet counts the padding, itself included.
    let bytes = datagram(0xA0, 0x08, &[1, 2, 3, 0, 0, 3]);
    assert_eq!(RtpPacket::parse(&bytes).unwrap().payload, [1, 2, 3]);
    let all = datagram(0xA0, 0x08, &[0, 2]);
    assert!(RtpPacket::parse(&all).unwrap().payload.is_empty());
    for bad in [&[1, 2, 0][..], &[1, 2, 4][..], &[][..]] {
        assert_eq!(
            RtpPacket::parse(&datagram(0xA0, 0x08, bad)),
            Err(RtpError::BadPadding)
        );
    }
}

#[test]
fn the_csrc_list_is_skipped() {
    let bytes = datagram(0x82, 0x08, &[9, 9, 9, 9, 8, 8, 8, 8, 1, 2]);
    assert_eq!(RtpPacket::parse(&bytes).unwrap().payload, [1, 2]);
    let short = datagram(0x82, 0x08, &[9, 9, 9, 9, 8, 8, 8]);
    assert_eq!(RtpPacket::parse(&short), Err(RtpError::Truncated));
}

#[test]
fn the_header_extension_is_skipped() {
    // X set: profile, length 2 words, two words, then the payload.
    let bytes = datagram(
        0x90,
        0x08,
        &[0xBE, 0xDE, 0, 2, 1, 1, 1, 1, 2, 2, 2, 2, 7, 7],
    );
    assert_eq!(RtpPacket::parse(&bytes).unwrap().payload, [7, 7]);
    let empty = datagram(0x90, 0x08, &[0xBE, 0xDE, 0, 0, 7]);
    assert_eq!(RtpPacket::parse(&empty).unwrap().payload, [7]);
    for short in [
        &[0xBE, 0xDE, 0][..],
        &[0xBE, 0xDE, 0, 2, 1, 1, 1, 1][..],
        &[0xBE, 0xDE, 0xFF, 0xFF][..],
    ] {
        assert_eq!(
            RtpPacket::parse(&datagram(0x90, 0x08, short)),
            Err(RtpError::Truncated)
        );
    }
    // CSRC, extension and padding together.
    let all = datagram(0xB1, 0x08, &[9, 9, 9, 9, 0, 0, 0, 1, 5, 5, 5, 5, 42, 0, 2]);
    assert_eq!(RtpPacket::parse(&all).unwrap().payload, [42]);
}

#[test]
fn what_is_not_rtp() {
    assert_eq!(RtpPacket::parse(&[]), Err(RtpError::TooShort));
    assert_eq!(RtpPacket::parse(&[0x80; 11]), Err(RtpError::TooShort));
    for first in [0x00, 0x40, 0xC0] {
        assert_eq!(
            RtpPacket::parse(&datagram(first, 0x08, &[1])),
            Err(RtpError::BadVersion)
        );
    }
}

#[test]
fn a_packet_is_written_without_allocating_and_only_if_it_fits() {
    let mut out = [0u8; 20];
    assert_eq!(HEADER.write_packet(&[1, 2, 3], &mut out), Some(15));
    assert_eq!(out[..15], datagram(0x80, 0x08, &[1, 2, 3])[..]);
    assert_eq!(HEADER.write_packet(&[0; 8], &mut out), Some(20));
    assert_eq!(HEADER.write_packet(&[0; 9], &mut out), None);
    assert_eq!(HEADER.write_packet(&[], &mut out[..11]), None);
}

#[test]
fn the_sender_numbers_its_packets() {
    let mut sender = RtpSender::new(0, 0xAABB_CCDD, 0xFFFE, 0xFFFF_FF00);
    let first = sender.next(160);
    assert_eq!(
        first,
        RtpHeader {
            marker: true,
            payload_type: 0,
            sequence: 0xFFFE,
            timestamp: 0xFFFF_FF00,
            ssrc: 0xAABB_CCDD,
        }
    );
    let second = sender.next(160);
    assert!(!second.marker);
    assert_eq!(second.sequence, 0xFFFF);
    // Both wrap.
    assert_eq!(second.timestamp, 0xFFFF_FFA0);
    let third = sender.next(80);
    assert_eq!((third.sequence, third.timestamp), (0, 0x40));
    let fourth = sender.next(240);
    assert_eq!((fourth.sequence, fourth.timestamp), (1, 0x90));
    assert_eq!(sender.stats().packets, 4);
    assert_eq!(sender.stats().octets, 640);
    assert_eq!(sender.ssrc(), 0xAABB_CCDD);
}

proptest! {
    #[test]
    fn build_then_parse_is_the_identity(
        marker: bool,
        payload_type in 0u8..128,
        sequence: u16,
        timestamp: u32,
        ssrc: u32,
        payload in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        let header = RtpHeader { marker, payload_type, sequence, timestamp, ssrc };
        let mut out = [0u8; HEADER_LEN + 400];
        let len = header.write_packet(&payload, &mut out).unwrap();
        prop_assert_eq!(len, HEADER_LEN + payload.len());
        let packet = RtpPacket::parse(&out[..len]).unwrap();
        prop_assert_eq!(packet.header, header);
        prop_assert_eq!(packet.payload, &payload[..]);
    }

    #[test]
    fn no_datagram_makes_the_parser_panic(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        if let Ok(packet) = RtpPacket::parse(&bytes) {
            prop_assert!(packet.payload.len() <= bytes.len() - HEADER_LEN);
            prop_assert!(packet.header.payload_type < 128);
        }
    }
}
