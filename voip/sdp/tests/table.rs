// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Table tests of the parser and the builder.

use sdp::{Attribute, Codec, Connection, Line, MAX_LEN, SdpError, SessionDescription};

/// The example of RFC 8866 section 5, with a second media section.
const SEMINAR: &str = "v=0\r\n\
o=jdoe 3724394400 3724394405 IN IP4 198.51.100.1\r\n\
s=Call to John Smith\r\n\
i=SDP Offer #1\r\n\
u=http://www.jdoe.example.com/home.html\r\n\
e=Jane Doe <jane@jdoe.example.com>\r\n\
p=+1 617 555-6011\r\n\
c=IN IP4 198.51.100.1\r\n\
t=0 0\r\n\
m=audio 49170 RTP/AVP 0\r\n\
m=audio 49180 RTP/AVP 0\r\n\
m=video 51372 RTP/AVP 99\r\n\
c=IN IP6 2001:db8::2\r\n\
a=rtpmap:99 h263-1998/90000\r\n";

/// What an analogue telephone adapter typically offers.
const ADAPTER: &str = "v=0\r\n\
o=- 16264 16264 IN IP4 10.10.20.11\r\n\
s=-\r\n\
c=IN IP4 10.10.20.11\r\n\
t=0 0\r\n\
m=audio 16482 RTP/AVP 0 8 101\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-15\r\n\
a=ptime:20\r\n\
a=sendrecv\r\n";

fn parse(text: &str) -> SessionDescription {
    SessionDescription::parse(text.as_bytes()).expect("the description parses")
}

#[test]
fn the_rfc_example_parses() {
    let d = parse(SEMINAR);
    assert_eq!(d.origin.username, "jdoe");
    assert_eq!(d.origin.session_id, "3724394400");
    assert_eq!(d.origin.session_version, "3724394405");
    assert_eq!(d.origin.address, "198.51.100.1");
    assert_eq!(d.session_name, "Call to John Smith");
    assert_eq!(
        d.connection.as_ref().and_then(Connection::as_ipv4),
        Some("198.51.100.1".parse().unwrap())
    );
    assert_eq!(d.time, (0, 0));
    assert_eq!(d.media.len(), 3);
    assert_eq!(d.media[0].kind, "audio");
    assert_eq!(d.media[0].port, 49170);
    assert_eq!(d.media[0].proto, "RTP/AVP");
    assert_eq!(d.media[0].formats, ["0"]);
    assert_eq!(d.media[2].kind, "video");
    assert_eq!(d.media[2].connection.as_ref().unwrap().addr_type, "IP6");
    assert_eq!(d.media[2].connection.as_ref().unwrap().as_ipv4(), None);
    assert_eq!(
        d.media[2].attributes,
        [Attribute::with_value("rtpmap", "99 h263-1998/90000")]
    );
}

#[test]
fn unknown_lines_are_kept_and_written_again() {
    let d = parse(SEMINAR);
    let kinds: String = d.extra.iter().map(|line| line.kind).collect();
    assert_eq!(kinds, "iuep");
    assert_eq!(d.extra[0].value, "SDP Offer #1");
    assert_eq!(d.build(), SEMINAR);
}

#[test]
fn an_adapter_offer_is_built_as_it_was_written() {
    let d = parse(ADAPTER);
    assert_eq!(d.media[0].formats, ["0", "8", "101"]);
    assert_eq!(d.media[0].attributes.len(), 6);
    assert_eq!(d.media[0].attributes[5], Attribute::flag("sendrecv"));
    assert_eq!(d.build(), ADAPTER);
}

#[test]
fn lines_may_end_in_a_line_feed_alone_and_blank_lines_are_skipped() {
    let bare = ADAPTER.replace("\r\n", "\n");
    assert_eq!(parse(&bare), parse(ADAPTER));
    let spaced = ADAPTER.replace("\r\n", "\r\n\r\n");
    assert_eq!(parse(&spaced), parse(ADAPTER));
    let unfinished = ADAPTER.trim_end();
    assert_eq!(parse(unfinished), parse(ADAPTER));
}

#[test]
fn a_missing_name_and_time_are_given_their_defaults() {
    let d = parse("v=0\no=- 1 1 IN IP4 10.0.0.1\nm=audio 4000 RTP/AVP 8\n");
    assert_eq!(d.session_name, "-");
    assert_eq!(d.time, (0, 0));
    assert_eq!(
        d.build(),
        "v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 4000 RTP/AVP 8\r\n"
    );
    assert_eq!(
        parse("v=0\no=- 1 1 IN IP4 10.0.0.1\ns=\n").session_name,
        "-"
    );
}

#[test]
fn media_lines() {
    let cases: &[(&str, u16, Option<u16>, &[&str])] = &[
        ("m=audio 49170 RTP/AVP 0", 49170, None, &["0"]),
        ("m=audio 49170/2 RTP/AVP 0 8", 49170, Some(2), &["0", "8"]),
        ("m=audio 0 RTP/AVP 0", 0, None, &["0"]),
        ("m=image 4000 udptl t38", 4000, None, &["t38"]),
        ("m=audio  65535\tRTP/AVP   8", 65535, None, &["8"]),
    ];
    for (line, port, count, formats) in cases {
        let d = parse(&format!("v=0\no=- 1 1 IN IP4 10.0.0.1\n{line}\n"));
        let m = &d.media[0];
        assert_eq!((m.port, m.port_count), (*port, *count), "{line}");
        assert_eq!(m.formats, *formats, "{line}");
    }
}

#[test]
fn attribute_lines() {
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("a=sendrecv", "sendrecv", None),
        ("a=ptime:20", "ptime", Some("20")),
        (
            "a=silenceSupp:off - - - -",
            "silenceSupp",
            Some("off - - - -"),
        ),
        ("a=fmtp:101 0-15", "fmtp", Some("101 0-15")),
        ("a=x:a:b", "x", Some("a:b")),
        ("a=x:", "x", Some("")),
        ("a=", "", None),
    ];
    for (line, name, value) in cases {
        let d = parse(&format!("v=0\no=- 1 1 IN IP4 10.0.0.1\n{line}\n"));
        assert_eq!(d.attributes[0].name, *name, "{line}");
        assert_eq!(d.attributes[0].value.as_deref(), *value, "{line}");
    }
}

#[test]
fn a_repeated_line_is_kept_and_the_first_is_the_one_read() {
    let d = parse(
        "v=0\no=- 1 1 IN IP4 10.0.0.1\ns=one\nc=IN IP4 10.0.0.1\nc=IN IP4 10.0.0.2\n\
         t=1 2\nt=3 4\nr=7d 1h 0 25h\n",
    );
    assert_eq!(d.connection.unwrap().address, "10.0.0.1");
    assert_eq!(d.time, (1, 2));
    assert_eq!(
        d.extra,
        [
            Line {
                kind: 'c',
                value: "IN IP4 10.0.0.2".to_owned()
            },
            Line {
                kind: 't',
                value: "3 4".to_owned()
            },
            Line {
                kind: 'r',
                value: "7d 1h 0 25h".to_owned()
            },
        ]
    );
}

#[test]
fn what_is_refused() {
    let o = "o=- 1 1 IN IP4 10.0.0.1\n";
    let cases: Vec<(String, SdpError)> = vec![
        (String::new(), SdpError::MissingVersion),
        (format!("{o}v=0\n"), SdpError::MissingVersion),
        (format!("v=1\n{o}"), SdpError::UnsupportedVersion),
        ("v=0\ns=-\n".to_owned(), SdpError::MissingOrigin),
        ("v=0\no=- 1 1 IN IP4\n".to_owned(), SdpError::BadOrigin(2)),
        (
            "v=0\no=- 1 1 IN IP4 10.0.0.1 more\n".to_owned(),
            SdpError::BadOrigin(2),
        ),
        (format!("v=0\n{o}c=IN IP4\n"), SdpError::BadConnection(3)),
        (format!("v=0\n{o}t=0\n"), SdpError::BadTime(3)),
        (format!("v=0\n{o}t=0 -1\n"), SdpError::BadTime(3)),
        (
            format!("v=0\n{o}m=audio 4000 RTP/AVP\n"),
            SdpError::BadMedia(3),
        ),
        (
            format!("v=0\n{o}m=audio 65536 RTP/AVP 0\n"),
            SdpError::BadMedia(3),
        ),
        (
            format!("v=0\n{o}m=audio +400 RTP/AVP 0\n"),
            SdpError::BadMedia(3),
        ),
        (
            format!("v=0\n{o}m=audio 4000/x RTP/AVP 0\n"),
            SdpError::BadMedia(3),
        ),
        (
            format!("v=0\n{o}m=audio 4000 RTP/AVP 0\nc=IN\n"),
            SdpError::BadConnection(4),
        ),
        (format!("v=0\n{o}no equals sign\n"), SdpError::BadLine(3)),
        (format!("v=0\n{o}1=digit\n"), SdpError::BadLine(3)),
        (format!("v=0\n{o}a =space\n"), SdpError::BadLine(3)),
        (format!("v=0\n{o}a=x\ry\n"), SdpError::BadLine(3)),
        (format!("v=0\n{o}a=x\0y\n"), SdpError::BadLine(3)),
    ];
    for (text, error) in cases {
        assert_eq!(
            SessionDescription::parse(text.as_bytes()),
            Err(error),
            "{text:?}"
        );
    }
}

#[test]
fn what_is_too_long_or_not_text_is_refused() {
    let mut long = ADAPTER.as_bytes().to_vec();
    long.resize(MAX_LEN + 1, b'\n');
    assert_eq!(SessionDescription::parse(&long), Err(SdpError::TooLong));

    let mut many = b"v=0\no=- 1 1 IN IP4 10.0.0.1\n".to_vec();
    for _ in 0..sdp::MAX_LINES {
        many.extend_from_slice(b"a=x\n");
    }
    assert_eq!(SessionDescription::parse(&many), Err(SdpError::TooLong));

    assert_eq!(
        SessionDescription::parse(b"v=0\no=- 1 1 IN IP4 10.0.0.1\ns=\xff\n"),
        Err(SdpError::NotUtf8)
    );
}

#[test]
fn a_value_cannot_start_a_line_of_its_own() {
    let mut d = parse(ADAPTER);
    d.session_name = "x\r\nm=audio 9 RTP/AVP 0".to_owned();
    d.attributes
        .push(Attribute::with_value("note", "a\nb\rc\0d"));
    let again = parse(&d.build());
    assert_eq!(again.media.len(), 1);
    assert_eq!(again.session_name, "x  m=audio 9 RTP/AVP 0");
    assert_eq!(again.attributes[0].value.as_deref(), Some("a b c d"));
}

#[test]
fn the_codecs() {
    assert_eq!(Codec::Pcmu.payload_type(), 0);
    assert_eq!(Codec::Pcma.payload_type(), 8);
    assert_eq!(Codec::from_payload_type(0), Some(Codec::Pcmu));
    assert_eq!(Codec::from_payload_type(8), Some(Codec::Pcma));
    assert_eq!(Codec::from_payload_type(13), None);
    assert_eq!(Codec::from_payload_type(101), None);
    assert_eq!(Codec::Pcmu.silence(), 0xFF);
    assert_eq!(Codec::Pcma.silence(), 0xD5);
    assert_eq!(Codec::Pcma.to_string(), "PCMA");
}
