// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Session descriptions (RFC 8866) and offer/answer (RFC 3264) for the one
//! thing rustmodem negotiates: a single G.711 audio stream.
//!
//! The crate is pure. It parses and builds text and decides what an offer or
//! an answer means; it opens nothing and reads no clock.
//!
//! A description is kept as it was written: lines the crate has no use for
//! are held in `extra` and written out again, so that a description that is
//! parsed and built is the same description.

#![forbid(unsafe_code)]

mod build;
mod negotiate;
mod parse;

use std::fmt;
use std::net::Ipv4Addr;

pub use negotiate::{
    AudioPrefs, Direction, LocalEndpoint, Negotiated, Refusal, accept_answer, answer, offer,
};
pub use parse::{MAX_LEN, MAX_LINES, SdpError};

/// The two codecs rustmodem carries. Both are G.711 at 8000 octets a second
/// with a static payload type (RFC 3551 section 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// G.711 mu-law, payload type 0.
    Pcmu,
    /// G.711 A-law, payload type 8.
    Pcma,
}

impl Codec {
    /// The static RTP payload type (RFC 3551 section 6, table 4).
    pub fn payload_type(self) -> u8 {
        match self {
            Codec::Pcmu => 0,
            Codec::Pcma => 8,
        }
    }

    /// The codec with this static payload type, if it is one of ours.
    pub fn from_payload_type(payload_type: u8) -> Option<Codec> {
        match payload_type {
            0 => Some(Codec::Pcmu),
            8 => Some(Codec::Pcma),
            _ => None,
        }
    }

    /// The encoding name of `a=rtpmap`.
    pub fn name(self) -> &'static str {
        match self {
            Codec::Pcmu => "PCMU",
            Codec::Pcma => "PCMA",
        }
    }

    /// The octet that is digital silence in this codec.
    pub fn silence(self) -> u8 {
        match self {
            Codec::Pcmu => 0xFF,
            Codec::Pcma => 0xD5,
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A session description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionDescription {
    /// The `o=` line.
    pub origin: Origin,
    /// The `s=` line. Never empty: an empty name is written and read as `-`.
    pub session_name: String,
    /// The session-level `c=` line.
    pub connection: Option<Connection>,
    /// The first `t=` line: start and stop. Zero and zero when there is none.
    pub time: (u64, u64),
    /// The session-level `a=` lines, in order.
    pub attributes: Vec<Attribute>,
    /// The `m=` sections, in order.
    pub media: Vec<Media>,
    /// Session-level lines the crate has no use for, kept to be written
    /// again. In the order RFC 8866 section 5 puts them in.
    pub extra: Vec<Line>,
}

/// The `o=` line (RFC 8866 section 5.2). The fields are kept as text: some
/// devices send session ids that do not fit a 64-bit number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub username: String,
    pub session_id: String,
    pub session_version: String,
    pub net_type: String,
    pub addr_type: String,
    pub address: String,
}

/// A `c=` line (RFC 8866 section 5.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub net_type: String,
    pub addr_type: String,
    pub address: String,
}

impl Connection {
    /// An `IN IP4` connection to this address.
    pub fn ipv4(address: Ipv4Addr) -> Connection {
        Connection {
            net_type: "IN".to_owned(),
            addr_type: "IP4".to_owned(),
            address: address.to_string(),
        }
    }

    /// The address, when the line is `IN IP4` with a plain unicast address.
    pub fn as_ipv4(&self) -> Option<Ipv4Addr> {
        if self.net_type.eq_ignore_ascii_case("IN") && self.addr_type.eq_ignore_ascii_case("IP4") {
            self.address.parse().ok()
        } else {
            None
        }
    }
}

/// An `a=` line: `a=name` or `a=name:value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub value: Option<String>,
}

impl Attribute {
    /// `a=name`.
    pub fn flag(name: &str) -> Attribute {
        Attribute {
            name: name.to_owned(),
            value: None,
        }
    }

    /// `a=name:value`.
    pub fn with_value(name: &str, value: &str) -> Attribute {
        Attribute {
            name: name.to_owned(),
            value: Some(value.to_owned()),
        }
    }
}

/// An `m=` section (RFC 8866 section 5.14) and the lines under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Media {
    /// `audio`, `video`, `image` and so on.
    pub kind: String,
    pub port: u16,
    /// The `/count` after the port, when there is one.
    pub port_count: Option<u16>,
    /// The transport, `RTP/AVP` for ours.
    pub proto: String,
    /// The formats: for RTP, payload type numbers as text.
    pub formats: Vec<String>,
    /// The media-level `c=` line.
    pub connection: Option<Connection>,
    /// The media-level `a=` lines, in order.
    pub attributes: Vec<Attribute>,
    /// Media-level lines the crate has no use for.
    pub extra: Vec<Line>,
}

/// A line that is kept and not understood: its type letter and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: char,
    pub value: String,
}

impl SessionDescription {
    /// Parses a description. See [`SdpError`] for what is refused.
    pub fn parse(input: &[u8]) -> Result<SessionDescription, SdpError> {
        parse::parse(input)
    }

    /// The description as text, each line ended by CRLF.
    pub fn build(&self) -> String {
        build::build(self)
    }
}

impl fmt::Display for SessionDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.build())
    }
}

/// Where a kept session-level line is written: 0 before `c=`, 1 between
/// `c=` and `t=`, 2 after `t=` (RFC 8866 section 5).
pub(crate) fn session_rank(kind: char) -> u8 {
    match kind {
        'v' | 'o' | 's' | 'i' | 'u' | 'e' | 'p' => 0,
        'b' => 1,
        _ => 2,
    }
}

/// Where a kept media-level line is written: 0 before `c=`, 1 after it.
pub(crate) fn media_rank(kind: char) -> u8 {
    match kind {
        'i' => 0,
        _ => 1,
    }
}
