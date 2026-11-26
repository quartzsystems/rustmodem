// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The sending side of a stream.

use crate::RtpHeader;

/// What a sender has sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendStats {
    pub packets: u64,
    /// Payload octets.
    pub octets: u64,
}

/// Numbers the packets of one outgoing stream (RFC 3550 section 5.1): the
/// sequence number goes up by one a packet and the timestamp by the samples
/// in it, and the first packet has the marker (RFC 3551 section 4.1).
#[derive(Clone, Debug)]
pub struct RtpSender {
    payload_type: u8,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    first: bool,
    stats: SendStats,
}

impl RtpSender {
    /// A sender. The SSRC and the first sequence number and timestamp
    /// should be random (RFC 3550 section 5.1); the caller draws them.
    pub fn new(payload_type: u8, ssrc: u32, sequence: u16, timestamp: u32) -> RtpSender {
        RtpSender {
            payload_type: payload_type & 0x7F,
            ssrc,
            sequence,
            timestamp,
            first: true,
            stats: SendStats::default(),
        }
    }

    /// The header of the next packet, which carries `samples` octets of
    /// G.711. The sender moves on to the packet after it.
    pub fn next(&mut self, samples: usize) -> RtpHeader {
        let header = RtpHeader {
            marker: std::mem::take(&mut self.first),
            payload_type: self.payload_type,
            sequence: self.sequence,
            timestamp: self.timestamp,
            ssrc: self.ssrc,
        };
        self.sequence = self.sequence.wrapping_add(1);
        // A payload is never longer than a datagram, so this does not
        // truncate.
        self.timestamp = self.timestamp.wrapping_add(samples as u32);
        self.stats.packets += 1;
        self.stats.octets += samples as u64;
        header
    }

    /// Changes the payload type, for a call whose codec was renegotiated.
    pub fn set_payload_type(&mut self, payload_type: u8) {
        self.payload_type = payload_type & 0x7F;
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    pub fn stats(&self) -> SendStats {
        self.stats
    }
}
