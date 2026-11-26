// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The far end of a call over RTP, as the SIP router hands it to the pool,
//! and the ports calls take their sockets from.

#![forbid(unsafe_code)]

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};

use rtp::FRAME;

use crate::os;

/// The media endpoint of a call over RTP: its socket, bound when the
/// session description was built, and what was negotiated.
#[derive(Debug)]
pub struct RtpEndpoint {
    /// The call's own socket.
    pub socket: UdpSocket,
    /// The far end, the codec and the packet time. The router keeps a
    /// clone and changes them when the far end renegotiates.
    pub params: std::sync::Arc<RtpParams>,
    /// How long the call may go without a packet from the far end before
    /// it is given up, in milliseconds.
    pub timeout_ms: u64,
    /// The SSRC of what we send, random (RFC 3550 section 5.1).
    pub ssrc: u32,
    /// The first sequence number we send, random.
    pub sequence: u16,
    /// The first timestamp we send, random.
    pub timestamp: u32,
    /// The SIP Call-ID of the call, for its capture.
    pub call_id: String,
}

/// What a call's media thread has to know of the negotiation, in one
/// atomic word so that the router can change it under a running call: the
/// far end's address and port, the payload type, the frames in a packet,
/// and whether the far end has the call on hold.
#[derive(Debug)]
pub struct RtpParams(AtomicU64);

/// The negotiation, read from the word of an [`RtpParams`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negotiation {
    /// Where the far end receives.
    pub remote: SocketAddrV4,
    pub payload_type: u8,
    /// The frames of [`FRAME`] octets in a packet: one to three.
    pub frames: usize,
    /// The far end is not sending and is not to be sent to. The call's
    /// engine is ticked from the local timer meanwhile, on silence.
    pub held: bool,
}

impl RtpParams {
    /// `ptime_ms` is 10, 20 or 30. The call is not on hold.
    pub fn new(remote: SocketAddrV4, payload_type: u8, ptime_ms: u32) -> RtpParams {
        RtpParams(AtomicU64::new(pack(remote, payload_type, ptime_ms, false)))
    }

    /// Changes all of it at once.
    pub fn set(&self, remote: SocketAddrV4, payload_type: u8, ptime_ms: u32, held: bool) {
        self.0.store(
            pack(remote, payload_type, ptime_ms, held),
            Ordering::Relaxed,
        );
    }

    /// The word as it is, to see whether it has changed.
    pub fn raw(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// The negotiation in a word read with [`RtpParams::raw`].
    pub fn unpack(raw: u64) -> Negotiation {
        Negotiation {
            remote: SocketAddrV4::new(Ipv4Addr::from((raw >> 32) as u32), (raw >> 16) as u16),
            payload_type: (raw >> 8) as u8,
            frames: usize::from(raw as u8 & 0x7F).clamp(1, 3),
            held: raw & HELD != 0,
        }
    }

    /// The negotiation now.
    pub fn get(&self) -> Negotiation {
        RtpParams::unpack(self.raw())
    }

    /// The far end's address now.
    pub fn remote(&self) -> SocketAddrV4 {
        self.get().remote
    }
}

/// The bit of the word that says the call is on hold.
const HELD: u64 = 0x80;

fn pack(remote: SocketAddrV4, payload_type: u8, ptime_ms: u32, held: bool) -> u64 {
    // A frame is 10 ms.
    let frames = (ptime_ms / (FRAME as u32 / 8)).clamp(1, 3);
    (u64::from(u32::from(*remote.ip())) << 32)
        | (u64::from(remote.port()) << 16)
        | (u64::from(payload_type) << 8)
        | if held { HELD } else { 0 }
        | u64::from(frames)
}

/// The ports RTP may use. A call's socket takes an even port (RFC 3550
/// section 11); the odd port after it is left free, as it would be for
/// RTCP.
#[derive(Clone, Debug)]
pub struct RtpPorts {
    first: u16,
    last: u16,
    /// Where the search for a free port starts.
    next: u16,
}

impl RtpPorts {
    /// The even ports from `first` to `last`, both included.
    pub fn new(first: u16, last: u16) -> RtpPorts {
        RtpPorts {
            first,
            last,
            next: first,
        }
    }

    /// Binds a socket to the next even port that is free, at `address`,
    /// with buffers that hold more than a second of packets. Each port is
    /// tried once; the error is that of the last when none is free.
    pub fn bind(&mut self, address: Ipv4Addr) -> io::Result<UdpSocket> {
        let span = u32::from(self.last.saturating_sub(self.first)) + 1;
        let mut failure = io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "the RTP port range has no even port",
        );
        for step in 0..span {
            let offset = (u32::from(self.next - self.first) + step) % span;
            let port = self.first + offset as u16;
            if !port.is_multiple_of(2) {
                continue;
            }
            match UdpSocket::bind(SocketAddrV4::new(address, port)) {
                Ok(socket) => {
                    // The buffers are sized now, while the socket is
                    // empty: the far end may send before the call's media
                    // thread has started.
                    os::size_buffers(&socket);
                    self.next = if port >= self.last {
                        self.first
                    } else {
                        port + 1
                    };
                    return Ok(socket);
                }
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }
}
