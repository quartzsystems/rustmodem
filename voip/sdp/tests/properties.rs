// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Property tests: a description that is built and parsed is the same
//! description, and no input makes the parser or the negotiation panic.

use std::net::Ipv4Addr;

use proptest::prelude::*;
use sdp::{
    Attribute, AudioPrefs, Codec, Connection, Line, LocalEndpoint, Media, Origin,
    SessionDescription, accept_answer, answer,
};

fn token() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9._/-]{1,12}"
}

/// Any printable text: a value runs to the end of its line.
fn value() -> impl Strategy<Value = String> {
    "[ -~]{0,20}"
}

fn connection() -> impl Strategy<Value = Connection> {
    (token(), token(), token()).prop_map(|(net_type, addr_type, address)| Connection {
        net_type,
        addr_type,
        address,
    })
}

fn attribute() -> impl Strategy<Value = Attribute> {
    ("[a-zA-Z0-9-]{0,12}", proptest::option::of(value()))
        .prop_map(|(name, value)| Attribute { name, value })
}

/// Kept lines of these type letters, in the order given: the order they are
/// written in.
fn kept(groups: &'static [&'static str]) -> impl Strategy<Value = Vec<Line>> {
    let groups: Vec<_> = groups
        .iter()
        .map(|letters| {
            let letters: Vec<char> = letters.chars().collect();
            proptest::collection::vec(
                (proptest::sample::select(letters), value())
                    .prop_map(|(kind, value)| Line { kind, value }),
                0..3,
            )
        })
        .collect();
    groups.prop_map(|groups| groups.into_iter().flatten().collect())
}

fn media() -> impl Strategy<Value = Media> {
    (
        token(),
        any::<u16>(),
        proptest::option::of(any::<u16>()),
        token(),
        proptest::collection::vec(token(), 1..4),
        proptest::option::of(connection()),
        proptest::collection::vec(attribute(), 0..4),
        kept(&["i", "bk"]),
    )
        .prop_map(
            |(kind, port, port_count, proto, formats, connection, attributes, extra)| Media {
                kind,
                port,
                port_count,
                proto,
                formats,
                connection,
                attributes,
                extra,
            },
        )
}

fn description() -> impl Strategy<Value = SessionDescription> {
    let origin = (token(), token(), token(), token(), token(), token()).prop_map(
        |(username, session_id, session_version, net_type, addr_type, address)| Origin {
            username,
            session_id,
            session_version,
            net_type,
            addr_type,
            address,
        },
    );
    (
        origin,
        "[ -~]{1,16}",
        proptest::option::of(connection()),
        any::<(u64, u64)>(),
        proptest::collection::vec(attribute(), 0..4),
        proptest::collection::vec(media(), 0..4),
        kept(&["iuep", "b", "rzk"]),
    )
        .prop_map(
            |(origin, session_name, connection, time, attributes, media, extra)| {
                SessionDescription {
                    origin,
                    session_name,
                    connection,
                    time,
                    attributes,
                    media,
                    extra,
                }
            },
        )
}

const LOCAL: LocalEndpoint = LocalEndpoint {
    address: Ipv4Addr::new(192, 0, 2, 1),
    port: 20000,
    session_id: 1,
    session_version: 1,
};

fn prefs() -> AudioPrefs {
    AudioPrefs {
        codecs: vec![Codec::Pcmu, Codec::Pcma],
        ptime_ms: 20,
    }
}

proptest! {
    #[test]
    fn build_then_parse_is_the_identity(d in description()) {
        let text = d.build();
        let parsed = SessionDescription::parse(text.as_bytes());
        prop_assert_eq!(parsed, Ok(d));
    }

    #[test]
    fn parse_then_build_then_parse_is_stable(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        if let Ok(d) = SessionDescription::parse(&bytes) {
            let again = SessionDescription::parse(d.build().as_bytes());
            prop_assert_eq!(again, Ok(d));
        }
    }

    #[test]
    fn lines_in_any_order_never_panic(
        lines in proptest::collection::vec(
            prop_oneof![
                Just("v=0".to_owned()),
                Just("o=- 1 1 IN IP4 10.0.0.1".to_owned()),
                Just("c=IN IP4 10.0.0.2".to_owned()),
                Just("c=IN IP4 0.0.0.0".to_owned()),
                Just("m=audio 4000 RTP/AVP 0 8 101".to_owned()),
                Just("m=audio 0 RTP/AVP 8".to_owned()),
                Just("m=image 4002 udptl t38".to_owned()),
                Just("a=sendonly".to_owned()),
                Just("a=ptime:10".to_owned()),
                "[a-z]=[ -~]{0,12}",
            ],
            0..12,
        )
    ) {
        let text = format!("v=0\r\n{}\r\n", lines.join("\r\n"));
        if let Ok(d) = SessionDescription::parse(text.as_bytes()) {
            let again = SessionDescription::parse(d.build().as_bytes());
            prop_assert_eq!(again.as_ref(), Ok(&d));
            if let Ok((ours, agreed)) = answer(&prefs(), &LOCAL, &d) {
                // An answer has a line for every line of the offer, and
                // exactly one of them is taken (RFC 3264 section 6).
                prop_assert_eq!(ours.media.len(), d.media.len());
                prop_assert_eq!(ours.media.iter().filter(|m| m.port != 0).count(), 1);
                prop_assert!([10, 20, 30].contains(&agreed.ptime_ms));
                let taken = ours.media.iter().find(|m| m.port != 0).unwrap();
                prop_assert_eq!(&taken.formats, &vec![agreed.codec.payload_type().to_string()]);
            }
            let _ = accept_answer(&prefs(), &d);
        }
    }
}
