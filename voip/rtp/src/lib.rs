// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! RTP for G.711 (RFC 3550, RFC 3551): packets, a sender, and the receive
//! session that the media tick loop is driven by.
//!
//! The crate is pure and allocates nothing after a session is made. It is
//! given datagrams, header fields and the arrival time; it opens no socket
//! and reads no clock. Random values (SSRC, first sequence number, first
//! timestamp) are given by the caller, so every run of a test is the same.
//!
//! There is no jitter buffer and no reorder buffer, by design. A modem
//! signal has to come out with exactly the sample count it went in with,
//! and the far end's packets are the only clock that knows that count:
//! [`RtpReceiver`] turns each arriving packet into the frames the tick loop
//! must run, filling what was lost and dropping what came twice or late.

#![forbid(unsafe_code)]

mod packet;
mod receiver;
mod sender;

pub use packet::{HEADER_LEN, RtpError, RtpHeader, RtpPacket};
pub use receiver::{Dropped, ReceiveStats, RtpReceiver, Verdict};
pub use sender::{RtpSender, SendStats};

/// The octets in a frame: 10 ms of G.711 at 8000 octets a second. Every
/// delivery is a whole number of frames.
pub const FRAME: usize = 80;

/// The sample rate of G.711 and the clock rate of its RTP timestamps
/// (RFC 3551 section 4.5.14).
pub const SAMPLE_RATE: u32 = 8000;

/// A timestamp that is further than this from the one expected, either
/// way, is a new stream and not a gap: three seconds of samples.
pub const RESYNC_SAMPLES: u32 = 3 * SAMPLE_RATE;
