// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The RTP packet (RFC 3550 section 5.1).
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |V=2|P|X|  CC   |M|     PT      |       sequence number         |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |                           timestamp                           |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |           synchronization source (SSRC) identifier            |
//! +=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+=+
//! ```

use std::fmt;

/// The length of the fixed header, which is all we ever send.
pub const HEADER_LEN: usize = 12;

/// The fields of the fixed header that matter. The version is always 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtpHeader {
    pub marker: bool,
    /// Seven bits.
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

/// A parsed packet: its header and its payload, borrowed from the datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtpPacket<'a> {
    pub header: RtpHeader,
    pub payload: &'a [u8],
}

/// Why a datagram is not an RTP packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtpError {
    /// Shorter than the fixed header.
    TooShort,
    /// The version is not 2.
    BadVersion,
    /// The CSRC list or the header extension runs past the end.
    Truncated,
    /// The padding count is zero or more than the payload.
    BadPadding,
}

impl fmt::Display for RtpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RtpError::TooShort => "the datagram is shorter than an RTP header",
            RtpError::BadVersion => "the RTP version is not 2",
            RtpError::Truncated => "the RTP header runs past the end of the datagram",
            RtpError::BadPadding => "the RTP padding count is not valid",
        })
    }
}

impl std::error::Error for RtpError {}

impl<'a> RtpPacket<'a> {
    /// Parses a datagram. The CSRC list and a header extension are skipped
    /// and padding is taken off the payload.
    pub fn parse(datagram: &'a [u8]) -> Result<RtpPacket<'a>, RtpError> {
        if datagram.len() < HEADER_LEN {
            return Err(RtpError::TooShort);
        }
        let first = datagram[0];
        if first >> 6 != 2 {
            return Err(RtpError::BadVersion);
        }
        let padding = first & 0x20 != 0;
        let extension = first & 0x10 != 0;
        let csrc_count = usize::from(first & 0x0F);

        let mut at = HEADER_LEN + 4 * csrc_count;
        if extension {
            // RFC 3550 section 5.3.1: a 16-bit profile field, then a 16-bit
            // length in 32-bit words that does not count this header.
            let words = datagram.get(at + 2..at + 4).ok_or(RtpError::Truncated)?;
            at += 4 + 4 * usize::from(u16::from_be_bytes([words[0], words[1]]));
        }
        let mut payload = datagram.get(at..).ok_or(RtpError::Truncated)?;
        if padding {
            // The last octet counts the padding, itself included.
            let count = usize::from(*payload.last().ok_or(RtpError::BadPadding)?);
            if count == 0 || count > payload.len() {
                return Err(RtpError::BadPadding);
            }
            payload = &payload[..payload.len() - count];
        }

        Ok(RtpPacket {
            header: RtpHeader {
                marker: datagram[1] & 0x80 != 0,
                payload_type: datagram[1] & 0x7F,
                sequence: u16::from_be_bytes([datagram[2], datagram[3]]),
                timestamp: u32::from_be_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]),
                ssrc: u32::from_be_bytes([datagram[8], datagram[9], datagram[10], datagram[11]]),
            },
            payload,
        })
    }
}

impl RtpHeader {
    /// Writes the header into the first [`HEADER_LEN`] octets of `out`: no
    /// padding, no extension, no CSRC.
    pub fn write(&self, out: &mut [u8; HEADER_LEN]) {
        out[0] = 2 << 6;
        out[1] = (u8::from(self.marker) << 7) | (self.payload_type & 0x7F);
        out[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        out[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        out[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
    }

    /// Writes a whole packet into `out` and returns its length, or `None`
    /// when `out` is too small. Nothing is allocated.
    pub fn write_packet(&self, payload: &[u8], out: &mut [u8]) -> Option<usize> {
        let len = HEADER_LEN + payload.len();
        let packet = out.get_mut(..len)?;
        let (header, body) = packet.split_at_mut(HEADER_LEN);
        self.write(header.try_into().ok()?);
        body.copy_from_slice(payload);
        Some(len)
    }
}
