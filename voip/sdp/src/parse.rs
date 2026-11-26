// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The parser.
//!
//! It is lenient where devices are sloppy and nothing depends on it: lines
//! may end in LF alone, blank lines are skipped, `s=` and `t=` may be
//! missing. It is strict about what the negotiation reads: `v=0` first, an
//! `o=` line, and well-formed `m=` and `c=` lines.

use std::fmt;

use crate::{
    Attribute, Connection, Line, Media, Origin, SessionDescription, media_rank, session_rank,
};

/// The longest description that is read, in octets. A description arrives
/// in a SIP datagram, which cannot be longer.
pub const MAX_LEN: usize = 65_535;

/// The most lines a description may have.
pub const MAX_LINES: usize = 2048;

/// Why a description could not be parsed. Line numbers start at 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdpError {
    /// Longer than [`MAX_LEN`] octets or [`MAX_LINES`] lines.
    TooLong,
    /// Not UTF-8 (RFC 8866 section 5).
    NotUtf8,
    /// A line that is not `<letter>=<value>`.
    BadLine(usize),
    /// The first line is not `v=`.
    MissingVersion,
    /// The version is not 0 (RFC 8866 section 5.1).
    UnsupportedVersion,
    /// There is no `o=` line.
    MissingOrigin,
    /// An `o=` line without its six fields.
    BadOrigin(usize),
    /// A `c=` line without its three fields.
    BadConnection(usize),
    /// A `t=` line that is not two numbers.
    BadTime(usize),
    /// An `m=` line without a media, a port, a transport and a format.
    BadMedia(usize),
}

impl fmt::Display for SdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SdpError::TooLong => write!(f, "the description is too long"),
            SdpError::NotUtf8 => write!(f, "the description is not UTF-8"),
            SdpError::BadLine(n) => write!(f, "line {n} is not a description line"),
            SdpError::MissingVersion => write!(f, "the description does not start with v="),
            SdpError::UnsupportedVersion => write!(f, "the description is not version 0"),
            SdpError::MissingOrigin => write!(f, "the description has no o= line"),
            SdpError::BadOrigin(n) => write!(f, "the o= line at line {n} is not valid"),
            SdpError::BadConnection(n) => write!(f, "the c= line at line {n} is not valid"),
            SdpError::BadTime(n) => write!(f, "the t= line at line {n} is not valid"),
            SdpError::BadMedia(n) => write!(f, "the m= line at line {n} is not valid"),
        }
    }
}

impl std::error::Error for SdpError {}

pub(crate) fn parse(input: &[u8]) -> Result<SessionDescription, SdpError> {
    if input.len() > MAX_LEN {
        return Err(SdpError::TooLong);
    }
    let text = std::str::from_utf8(input).map_err(|_| SdpError::NotUtf8)?;

    let mut seen_version = false;
    let mut origin = None;
    let mut session_name = None;
    let mut connection = None;
    let mut time = None;
    let mut attributes = Vec::new();
    let mut extra = Vec::new();
    let mut media: Vec<Media> = Vec::new();

    for (index, raw) in text.split('\n').enumerate() {
        let number = index + 1;
        if number > MAX_LINES {
            return Err(SdpError::TooLong);
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        // RFC 8866 section 9: <type>=<value>, with no space around the "=".
        let mut chars = line.chars();
        let kind = chars.next().filter(char::is_ascii_alphabetic);
        let (Some(kind), Some('=')) = (kind, chars.next()) else {
            return Err(SdpError::BadLine(number));
        };
        let value = chars.as_str();
        if value.contains(['\r', '\0']) {
            return Err(SdpError::BadLine(number));
        }

        if !seen_version {
            if kind != 'v' {
                return Err(SdpError::MissingVersion);
            }
            if value != "0" {
                return Err(SdpError::UnsupportedVersion);
            }
            seen_version = true;
            continue;
        }

        if kind == 'm' {
            media.push(parse_media(value).ok_or(SdpError::BadMedia(number))?);
            continue;
        }

        if let Some(section) = media.last_mut() {
            match kind {
                'c' if section.connection.is_none() => {
                    section.connection =
                        Some(parse_connection(value).ok_or(SdpError::BadConnection(number))?);
                }
                'a' => section.attributes.push(parse_attribute(value)),
                _ => section.extra.push(Line {
                    kind,
                    value: value.to_owned(),
                }),
            }
            continue;
        }

        match kind {
            'o' if origin.is_none() => {
                origin = Some(parse_origin(value).ok_or(SdpError::BadOrigin(number))?);
            }
            's' if session_name.is_none() => session_name = Some(value.to_owned()),
            'c' if connection.is_none() => {
                connection = Some(parse_connection(value).ok_or(SdpError::BadConnection(number))?);
            }
            't' if time.is_none() => {
                time = Some(parse_time(value).ok_or(SdpError::BadTime(number))?);
            }
            'a' => attributes.push(parse_attribute(value)),
            _ => extra.push(Line {
                kind,
                value: value.to_owned(),
            }),
        }
    }

    if !seen_version {
        return Err(SdpError::MissingVersion);
    }
    let origin = origin.ok_or(SdpError::MissingOrigin)?;

    // The kept lines are put in the order they are written in, so that a
    // description that is built and parsed again is the same.
    extra.sort_by_key(|line| session_rank(line.kind));
    for section in &mut media {
        section.extra.sort_by_key(|line| media_rank(line.kind));
    }

    Ok(SessionDescription {
        origin,
        session_name: match session_name {
            Some(name) if !name.is_empty() => name,
            _ => "-".to_owned(),
        },
        connection,
        time: time.unwrap_or((0, 0)),
        attributes,
        media,
        extra,
    })
}

/// RFC 8866 section 5.2: `<username> <sess-id> <sess-version> <nettype>
/// <addrtype> <unicast-address>`.
fn parse_origin(value: &str) -> Option<Origin> {
    let mut fields = value.split_ascii_whitespace();
    let origin = Origin {
        username: fields.next()?.to_owned(),
        session_id: fields.next()?.to_owned(),
        session_version: fields.next()?.to_owned(),
        net_type: fields.next()?.to_owned(),
        addr_type: fields.next()?.to_owned(),
        address: fields.next()?.to_owned(),
    };
    fields.next().is_none().then_some(origin)
}

/// RFC 8866 section 5.7: `<nettype> <addrtype> <connection-address>`.
fn parse_connection(value: &str) -> Option<Connection> {
    let mut fields = value.split_ascii_whitespace();
    let connection = Connection {
        net_type: fields.next()?.to_owned(),
        addr_type: fields.next()?.to_owned(),
        address: fields.next()?.to_owned(),
    };
    fields.next().is_none().then_some(connection)
}

/// RFC 8866 section 5.9: `<start-time> <stop-time>`.
fn parse_time(value: &str) -> Option<(u64, u64)> {
    let mut fields = value.split_ascii_whitespace();
    let start = number(fields.next()?)?;
    let stop = number(fields.next()?)?;
    fields.next().is_none().then_some((start, stop))
}

/// RFC 8866 section 5.14: `<media> <port>[/<count>] <proto> <fmt> ...`.
fn parse_media(value: &str) -> Option<Media> {
    let mut fields = value.split_ascii_whitespace();
    let kind = fields.next()?.to_owned();
    let port_field = fields.next()?;
    let (port, port_count) = match port_field.split_once('/') {
        Some((port, count)) => (number(port)?, Some(number(count)?)),
        None => (number(port_field)?, None),
    };
    let proto = fields.next()?.to_owned();
    let formats: Vec<String> = fields.map(str::to_owned).collect();
    if formats.is_empty() {
        return None;
    }
    Some(Media {
        kind,
        port,
        port_count,
        proto,
        formats,
        connection: None,
        attributes: Vec::new(),
        extra: Vec::new(),
    })
}

/// RFC 8866 section 5.13: `<attribute>` or `<attribute>:<value>`.
fn parse_attribute(value: &str) -> Attribute {
    match value.split_once(':') {
        Some((name, value)) => Attribute {
            name: name.to_owned(),
            value: Some(value.to_owned()),
        },
        None => Attribute {
            name: value.to_owned(),
            value: None,
        },
    }
}

/// A decimal number of digits only: no sign, no space.
fn number<T: std::str::FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}
