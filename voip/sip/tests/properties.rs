// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Property tests of the message layer: a message that is built and
//! parsed is the same message, and no datagram makes the parser or the
//! typed getters panic.

use proptest::prelude::*;
use sip::{Header, Headers, Message, Method, Request, Response};

/// A header name in the spelling it is kept under.
fn name() -> impl Strategy<Value = String> {
    prop_oneof![
        proptest::sample::select(vec![
            "Via",
            "From",
            "To",
            "Call-ID",
            "CSeq",
            "Contact",
            "Max-Forwards",
            "Route",
            "Record-Route",
            "Expires",
            "Allow",
            "Supported",
            "Require",
            "User-Agent",
            "Content-Type",
            "WWW-Authenticate",
            "Proxy-Authenticate",
            "Authorization",
            "Proxy-Authorization",
            "Subject",
        ])
        .prop_map(str::to_owned),
        "X-[A-Za-z0-9!%*_+`'~.-]{1,10}",
    ]
}

/// A header value: any text on one line with no space at either end.
fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "[!-~\u{00a1}-\u{00ff}]",
        "[!-~][ -~\u{00a1}-\u{00ff}]{0,40}[!-~]",
    ]
}

fn headers() -> impl Strategy<Value = Headers> {
    proptest::collection::vec((name(), value()), 0..12).prop_map(|list| {
        Headers(
            list.into_iter()
                .map(|(name, value)| Header { name, value })
                .collect(),
        )
    })
}

fn body() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..200)
}

fn request() -> impl Strategy<Value = Request> {
    (
        "[A-Z!%*_+`'~.-]{1,12}",
        "[!#-;=?-~]{1,40}",
        headers(),
        body(),
    )
        .prop_map(|(method, uri, headers, body)| Request {
            method: Method::parse(&method).expect("a token is a method"),
            uri,
            headers,
            body,
        })
}

fn response() -> impl Strategy<Value = Response> {
    (
        100u16..700,
        "[ -~\u{00a1}-\u{00ff}]{0,30}",
        headers(),
        body(),
    )
        .prop_map(|(status, reason, headers, body)| Response {
            status,
            reason,
            headers,
            body,
        })
}

/// Calls every typed getter. None of them may panic, whatever is there.
fn poke(message: &Message) {
    let headers = message.headers();
    let _ = headers.vias();
    let _ = headers.top_via();
    let _ = headers.from();
    let _ = headers.to();
    let _ = headers.call_id();
    let _ = headers.cseq();
    let _ = headers.contacts();
    let _ = headers.contact();
    let _ = headers.max_forwards();
    let _ = headers.expires();
    let _ = headers.min_expires();
    let _ = headers.routes();
    let _ = headers.record_routes();
    let _ = headers.allow();
    let _ = headers.supported();
    let _ = headers.require();
    let _ = headers.user_agent();
    let _ = headers.content_type();
    let _ = headers.www_authenticate();
    let _ = headers.proxy_authenticate();
    let _ = headers.authorization();
    let _ = headers.proxy_authorization();
    match message {
        Message::Request(request) => {
            let _ = request.essentials();
            let _ = request.sip_uri();
        }
        Message::Response(response) => {
            let _ = response.essentials();
        }
    }
}

/// A datagram that parses must be the same message when it is built and
/// parsed again.
fn check(datagram: &[u8]) -> Result<(), TestCaseError> {
    if let Ok(message) = Message::parse(datagram) {
        poke(&message);
        let again = Message::parse(&message.to_bytes());
        prop_assert_eq!(again, Ok(message));
    }
    Ok(())
}

const INVITE: &[u8] = b"INVITE sip:5551000@10.10.20.5;user=phone SIP/2.0\r\n\
Via: SIP/2.0/UDP 10.10.20.11:5060;branch=z9hG4bK-524287-1;rport\r\n\
v: SIP / 2.0 / UDP relay.example.org ; branch = z9hG4bKb , SIP/2.0/UDP [2001:db8::1]:5062\r\n\
Max-Forwards: 70\r\n\
From: \"Front \\\"desk\\\"\" <sip:201@10.10.20.11>;tag=8f2a\r\n\
To: <sip:5551000@10.10.20.5>\r\n\
Call-ID: 3b1c9e@10.10.20.11\r\n\
CSeq: 101 INVITE\r\n\
Contact: <sip:201@10.10.20.11:5060>;expires=60, sip:b@example.org;q=0.1\r\n\
Route: <sip:p1.example.com;lr>,\r\n <sip:p2.example.com;lr>\r\n\
Proxy-Authorization: Digest username=\"201\", realm=\"x, y\", nonce=\"n\", uri=\"sip:x\", response=\"00\"\r\n\
Supported: replaces, timer\r\n\
Content-Type: application/sdp\r\n\
Content-Length: 5\r\n\
\r\n\
v=0\r\n";

proptest! {
    #[test]
    fn a_request_built_then_parsed_is_the_same(request in request()) {
        let parsed = Message::parse(&request.to_bytes());
        prop_assert_eq!(parsed, Ok(Message::Request(request)));
    }

    #[test]
    fn a_response_built_then_parsed_is_the_same(response in response()) {
        let parsed = Message::parse(&response.to_bytes());
        prop_assert_eq!(parsed, Ok(Message::Response(response)));
    }

    #[test]
    fn no_octets_make_the_parser_panic(datagram in proptest::collection::vec(any::<u8>(), 0..300)) {
        check(&datagram)?;
    }

    #[test]
    fn no_text_makes_the_parser_panic(text in "(INVITE|SIP/2.0|[A-Z]{1,6}) [ -~]{0,20}\r\n([A-Za-z-]{1,12} ?: ?[ -~]{0,30}\r\n( [ -~]{0,10}\r\n)?){0,6}\r\n[ -~]{0,10}") {
        check(text.as_bytes())?;
    }

    /// A real message with octets changed, put in and taken out.
    #[test]
    fn a_damaged_message_never_panics(
        edits in proptest::collection::vec((any::<prop::sample::Index>(), 0u8..3, any::<u8>()), 1..8)
    ) {
        let mut datagram = INVITE.to_vec();
        for (at, kind, octet) in edits {
            if datagram.is_empty() {
                break;
            }
            let at = at.index(datagram.len());
            match kind {
                0 => datagram[at] = octet,
                1 => datagram.insert(at, octet),
                _ => {
                    datagram.remove(at);
                }
            }
        }
        check(&datagram)?;
    }
}

#[test]
fn the_message_the_damage_starts_from_is_whole() {
    let message = Message::parse(INVITE).unwrap();
    poke(&message);
    let Message::Request(request) = &message else {
        panic!("not a request");
    };
    assert!(request.essentials().is_ok());
    assert_eq!(request.headers.vias().unwrap().len(), 3);
    assert_eq!(request.headers.routes().unwrap().len(), 2);
    assert_eq!(
        request.headers.proxy_authorization()[0].get("realm"),
        Some("x, y")
    );
    assert_eq!(Message::parse(&message.to_bytes()), Ok(message));
}
