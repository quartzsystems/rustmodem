// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Offer/answer (RFC 3264) for one audio stream of G.711.
//!
//! rustmodem carries modem signals, so the stream is PCMU or PCMA and
//! nothing else: no `telephone-event`, no comfort noise, no dynamic payload
//! type is ever offered or accepted, and silence suppression is refused
//! with `a=silenceSupp:off - - - -`.

use std::net::{Ipv4Addr, SocketAddrV4};

use crate::{Attribute, Codec, Connection, Media, Origin, SessionDescription};

/// The packet times, in milliseconds, that are accepted from an offer.
const PTIMES: [u32; 3] = [10, 20, 30];

/// What we are willing to carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioPrefs {
    /// The codecs, most preferred first.
    pub codecs: Vec<Codec>,
    /// Our packet time in milliseconds: 10, 20 or 30.
    pub ptime_ms: u32,
}

/// Our end of the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalEndpoint {
    /// The address the far end sends RTP to.
    pub address: Ipv4Addr,
    /// The RTP port.
    pub port: u16,
    /// The session id of our `o=` line: the same for the whole call.
    pub session_id: u64,
    /// The session version: one more for each description we send in the
    /// call (RFC 3264 section 8).
    pub session_version: u64,
}

/// The direction attribute of a stream (RFC 3264 section 5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
}

impl Direction {
    fn name(self) -> &'static str {
        match self {
            Direction::SendRecv => "sendrecv",
            Direction::SendOnly => "sendonly",
            Direction::RecvOnly => "recvonly",
            Direction::Inactive => "inactive",
        }
    }

    fn from_name(name: &str) -> Option<Direction> {
        match name {
            "sendrecv" => Some(Direction::SendRecv),
            "sendonly" => Some(Direction::SendOnly),
            "recvonly" => Some(Direction::RecvOnly),
            "inactive" => Some(Direction::Inactive),
            _ => None,
        }
    }

    /// What the answerer says to an offer of this direction (RFC 3264
    /// section 6.1).
    fn reversed(self) -> Direction {
        match self {
            Direction::SendRecv => Direction::SendRecv,
            Direction::SendOnly => Direction::RecvOnly,
            Direction::RecvOnly => Direction::SendOnly,
            Direction::Inactive => Direction::Inactive,
        }
    }
}

/// What was agreed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negotiated {
    pub codec: Codec,
    /// The packet time both ways, in milliseconds.
    pub ptime_ms: u32,
    /// Where the far end receives RTP. Its address is `0.0.0.0` when the
    /// far end put the call on hold the old way.
    pub remote: SocketAddrV4,
    /// The far end is not sending: `a=sendonly`, `a=inactive` or a
    /// `0.0.0.0` connection address (RFC 3264 section 8.4).
    pub held: bool,
}

/// Why a description cannot be used. The caller answers 488.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// There is no `m=audio` line over `RTP/AVP`.
    NoAudio,
    /// There is an `m=image` line with a port: T.38.
    Image,
    /// The audio stream has port zero.
    ZeroPort,
    /// The audio stream has neither PCMU nor PCMA, or none we are
    /// configured for.
    NoCommonCodec,
    /// The stream has no connection address, or one that is not IPv4.
    BadAddress,
}

/// Our offer: one audio stream with our codecs in order of preference.
pub fn offer(prefs: &AudioPrefs, local: &LocalEndpoint) -> SessionDescription {
    description(
        local,
        vec![audio(
            local.port,
            &prefs.codecs,
            prefs.ptime_ms,
            Direction::SendRecv,
        )],
    )
}

/// Our answer to an offer, and what it agrees.
///
/// The answer picks the first codec of ours that the offer contains. Every
/// other stream of the offer is refused with port zero (RFC 3264 section 6).
pub fn answer(
    prefs: &AudioPrefs,
    local: &LocalEndpoint,
    offer: &SessionDescription,
) -> Result<(SessionDescription, Negotiated), Refusal> {
    if offer
        .media
        .iter()
        .any(|m| m.kind.eq_ignore_ascii_case("image") && m.port != 0)
    {
        return Err(Refusal::Image);
    }
    let index = audio_stream(offer)?;
    let stream = &offer.media[index];
    let codec = prefs
        .codecs
        .iter()
        .copied()
        .find(|codec| has_format(stream, *codec))
        .ok_or(Refusal::NoCommonCodec)?;
    let (remote, direction) = remote_of(offer, stream)?;
    // An offered packet time we can keep is kept; otherwise ours is stated.
    let ptime_ms = ptime(stream).unwrap_or(prefs.ptime_ms);

    let media = offer
        .media
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if i == index {
                audio(local.port, &[codec], ptime_ms, direction.reversed())
            } else {
                refused(m)
            }
        })
        .collect();
    let negotiated = Negotiated {
        codec,
        ptime_ms,
        remote,
        held: held(remote, direction),
    };
    Ok((description(local, media), negotiated))
}

/// What the far end's answer to our offer agrees. Our offer has one stream,
/// so the answer's first stream is the one that answers it.
pub fn accept_answer(
    prefs: &AudioPrefs,
    answer: &SessionDescription,
) -> Result<Negotiated, Refusal> {
    let stream = answer.media.first().ok_or(Refusal::NoAudio)?;
    if !is_audio(stream) {
        return Err(Refusal::NoAudio);
    }
    if stream.port == 0 {
        return Err(Refusal::ZeroPort);
    }
    // RFC 3264 section 6.1: the answer lists formats in the answerer's
    // order of preference; the first we offered is the one in use.
    let codec = stream
        .formats
        .iter()
        .filter_map(|format| format.parse().ok().and_then(Codec::from_payload_type))
        .find(|codec| prefs.codecs.contains(codec))
        .ok_or(Refusal::NoCommonCodec)?;
    let (remote, direction) = remote_of(answer, stream)?;
    Ok(Negotiated {
        codec,
        ptime_ms: ptime(stream).unwrap_or(prefs.ptime_ms),
        remote,
        held: held(remote, direction),
    })
}

/// The index of the audio stream to answer: the first `m=audio` over
/// `RTP/AVP` with a port.
fn audio_stream(offer: &SessionDescription) -> Result<usize, Refusal> {
    let mut zero = false;
    for (index, stream) in offer.media.iter().enumerate() {
        if !is_audio(stream) {
            continue;
        }
        if stream.port == 0 {
            zero = true;
        } else {
            return Ok(index);
        }
    }
    Err(if zero {
        Refusal::ZeroPort
    } else {
        Refusal::NoAudio
    })
}

fn is_audio(stream: &Media) -> bool {
    stream.kind.eq_ignore_ascii_case("audio") && stream.proto.eq_ignore_ascii_case("RTP/AVP")
}

fn has_format(stream: &Media, codec: Codec) -> bool {
    stream
        .formats
        .iter()
        .any(|format| format.parse() == Ok(codec.payload_type()))
}

/// The stream's `a=ptime`, when it is one we accept.
fn ptime(stream: &Media) -> Option<u32> {
    stream
        .attributes
        .iter()
        .find(|a| a.name == "ptime")
        .and_then(|a| a.value.as_deref())
        .and_then(|value| value.trim().parse().ok())
        .filter(|ms| PTIMES.contains(ms))
}

/// Where the stream is received, and its direction. The media-level lines
/// win over the session-level ones (RFC 8866 sections 5.7 and 5.13).
fn remote_of(
    description: &SessionDescription,
    stream: &Media,
) -> Result<(SocketAddrV4, Direction), Refusal> {
    let address = stream
        .connection
        .as_ref()
        .or(description.connection.as_ref())
        .and_then(Connection::as_ipv4)
        .ok_or(Refusal::BadAddress)?;
    let direction = direction_of(&stream.attributes)
        .or_else(|| direction_of(&description.attributes))
        .unwrap_or(Direction::SendRecv);
    Ok((SocketAddrV4::new(address, stream.port), direction))
}

fn direction_of(attributes: &[Attribute]) -> Option<Direction> {
    attributes
        .iter()
        .filter(|a| a.value.is_none())
        .find_map(|a| Direction::from_name(&a.name))
}

fn held(remote: SocketAddrV4, direction: Direction) -> bool {
    remote.ip().is_unspecified() || matches!(direction, Direction::SendOnly | Direction::Inactive)
}

fn description(local: &LocalEndpoint, media: Vec<Media>) -> SessionDescription {
    SessionDescription {
        origin: Origin {
            username: "-".to_owned(),
            session_id: local.session_id.to_string(),
            session_version: local.session_version.to_string(),
            net_type: "IN".to_owned(),
            addr_type: "IP4".to_owned(),
            address: local.address.to_string(),
        },
        session_name: "rustmodem".to_owned(),
        connection: Some(Connection::ipv4(local.address)),
        time: (0, 0),
        attributes: Vec::new(),
        media,
        extra: Vec::new(),
    }
}

/// Our audio stream.
fn audio(port: u16, codecs: &[Codec], ptime_ms: u32, direction: Direction) -> Media {
    let mut attributes: Vec<Attribute> = codecs
        .iter()
        .map(|codec| {
            Attribute::with_value(
                "rtpmap",
                &format!("{} {}/8000", codec.payload_type(), codec.name()),
            )
        })
        .collect();
    attributes.push(Attribute::with_value("ptime", &ptime_ms.to_string()));
    attributes.push(Attribute::flag(direction.name()));
    attributes.push(Attribute::with_value("silenceSupp", "off - - - -"));
    Media {
        kind: "audio".to_owned(),
        port,
        port_count: None,
        proto: "RTP/AVP".to_owned(),
        formats: codecs
            .iter()
            .map(|codec| codec.payload_type().to_string())
            .collect(),
        connection: None,
        attributes,
        extra: Vec::new(),
    }
}

/// A stream of the offer that we do not take: the same line with port zero
/// (RFC 3264 section 6).
fn refused(stream: &Media) -> Media {
    Media {
        kind: stream.kind.clone(),
        port: 0,
        port_count: None,
        proto: stream.proto.clone(),
        formats: stream.formats.clone(),
        connection: None,
        attributes: Vec::new(),
        extra: Vec::new(),
    }
}
