// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The SIP URI (RFC 3261 section 19.1).

use proptest::prelude::*;
use sip::{Params, SipUri, UriError};

#[test]
fn the_parts_of_a_uri() {
    let uri = SipUri::parse(
        "sip:alice:secret@atlanta.example.com:5070;transport=udp;lr?subject=x&priority=urgent",
    )
    .unwrap();
    assert_eq!(uri.user.as_deref(), Some("alice"));
    assert_eq!(uri.password.as_deref(), Some("secret"));
    assert_eq!(uri.host, "atlanta.example.com");
    assert_eq!(uri.port, Some(5070));
    assert_eq!(uri.params.get("transport"), Some("udp"));
    assert!(uri.params.has("lr"));
    assert_eq!(uri.params.get("lr"), None);
    assert_eq!(
        uri.headers,
        [
            ("subject".to_owned(), "x".to_owned()),
            ("priority".to_owned(), "urgent".to_owned())
        ]
    );
    assert_eq!(uri.port_or_default(), 5070);
}

#[test]
fn uris_that_parse_and_are_written_as_they_were() {
    for text in [
        "sip:example.com",
        "sip:10.10.20.11",
        "sip:10.10.20.11:5062",
        "sip:5551000@10.10.20.5",
        "sip:7002@pbx.example.lan;line=2",
        "sip:[2001:db8::1]:5060",
        "sip:[::1]",
        "sip:+1-212-555-1212:1234@gateway.example.com;user=phone",
        "sip:user;par=u%40example.net@example.com",
        "sip:sip%3Auser%40example.com@company.example;other-param=summit",
        "sip:alice@example.com?Route=%3Csip:example.com%3E",
        "sip:a_b.c-d@host_name.example",
        "sip:1_unusual.INVITE@example.net",
        "sip:*77@example.com",
    ] {
        let uri = SipUri::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(uri.to_string(), text);
    }
    // The scheme is case insensitive and is written in lower case.
    assert_eq!(
        SipUri::parse("SIP:Alice@Example.COM").unwrap().to_string(),
        "sip:Alice@Example.COM"
    );
    assert_eq!(
        SipUri::parse("sip:example.com").unwrap().port_or_default(),
        5060
    );
}

#[test]
fn what_is_refused() {
    let cases = [
        ("", UriError::NoScheme),
        ("example.com", UriError::NoScheme),
        ("sips:alice@example.com", UriError::UnsupportedScheme),
        ("tel:+15551000", UriError::UnsupportedScheme),
        ("http://example.com/", UriError::UnsupportedScheme),
        (
            "nobodyKnowsThisScheme:totallyopaquecontent",
            UriError::UnsupportedScheme,
        ),
        ("sip:", UriError::BadHost),
        ("sip:@example.com", UriError::BadUser),
        ("sip:al[ice@example.com", UriError::BadUser),
        ("sip:alice@", UriError::BadHost),
        ("sip:alice@exa mple.com", UriError::BadCharacter),
        ("sip:alice@example.com\r\n", UriError::BadCharacter),
        ("sip:<alice@example.com>", UriError::BadCharacter),
        ("sip:alice@exam!ple.com", UriError::BadHost),
        ("sip:alice@[::1", UriError::BadHost),
        ("sip:alice@[]", UriError::BadHost),
        ("sip:alice@[::1]5060", UriError::BadHost),
        ("sip:alice@example.com:", UriError::BadPort),
        ("sip:alice@example.com:port", UriError::BadPort),
        ("sip:alice@example.com:65536", UriError::BadPort),
        ("sip:alice@example.com:-1", UriError::BadPort),
        ("sip:alice@example.com;=x", UriError::BadParams),
        ("sip:alice@example.com;a b=x", UriError::BadCharacter),
        ("sip:alice@example.com?=x", UriError::BadHeaders),
    ];
    for (text, error) in cases {
        assert_eq!(SipUri::parse(text), Err(error), "{text:?}");
    }
}

#[test]
fn the_called_number_is_the_user_part_with_its_escapes_undone() {
    let decoded = |text: &str| SipUri::parse(text).unwrap().user_decoded();
    assert_eq!(decoded("sip:5551000@10.0.0.1").as_deref(), Some("5551000"));
    assert_eq!(
        decoded("sip:%35551000@10.0.0.1").as_deref(),
        Some("5551000")
    );
    assert_eq!(
        decoded("sip:sip%3Auser%40example.com@company.example").as_deref(),
        Some("sip:user@example.com")
    );
    // Not escapes: kept as written.
    assert_eq!(decoded("sip:100%@10.0.0.1").as_deref(), Some("100%"));
    assert_eq!(decoded("sip:a%zzb@10.0.0.1").as_deref(), Some("a%zzb"));
    assert_eq!(decoded("sip:10.0.0.1"), None);
}

#[test]
fn building_a_uri() {
    let mut uri = SipUri::new(Some("7002"), "10.10.20.5", Some(5060));
    uri.params.set("line", Some("2"));
    assert_eq!(uri.to_string(), "sip:7002@10.10.20.5:5060;line=2");
    uri.params.set("LINE", Some("3"));
    uri.params.set("lr", None);
    assert_eq!(uri.to_string(), "sip:7002@10.10.20.5:5060;line=3;lr");
    assert_eq!(
        SipUri::host("pbx.example.lan").to_string(),
        "sip:pbx.example.lan"
    );
    assert!(Params::new().is_empty());
}

fn uri() -> impl Strategy<Value = SipUri> {
    let host = prop_oneof![
        "[a-z0-9]([a-z0-9.-]{0,20}[a-z0-9])?",
        (any::<[u8; 4]>()).prop_map(|a| std::net::Ipv4Addr::from(a).to_string()),
        Just("[2001:db8::7]".to_owned()),
    ];
    let param = (
        "[a-z][a-z0-9-]{0,8}",
        proptest::option::of("[a-zA-Z0-9.%-]{1,10}"),
    );
    (
        proptest::option::of((
            "[a-zA-Z0-9_.!~*'()%&=+$,;?/-]{1,16}",
            proptest::option::of("[a-zA-Z0-9]{0,8}"),
        )),
        host,
        proptest::option::of(any::<u16>()),
        proptest::collection::vec(param, 0..4),
        proptest::collection::vec(("[a-zA-Z-]{1,8}", "[a-zA-Z0-9%.]{0,10}"), 0..3),
    )
        .prop_map(|(userinfo, host, port, params, headers)| {
            let (user, password) = match userinfo {
                Some((user, password)) => (Some(user), password),
                None => (None, None),
            };
            SipUri {
                user,
                password,
                host,
                port,
                params: Params(params),
                headers,
            }
        })
}

proptest! {
    #[test]
    fn build_then_parse_is_the_identity(uri in uri()) {
        let text = uri.to_string();
        prop_assert_eq!(SipUri::parse(&text), Ok(uri), "{}", text);
    }

    #[test]
    fn no_text_makes_the_parser_panic(text in "\\PC{0,60}") {
        if let Ok(uri) = SipUri::parse(&text) {
            let again = SipUri::parse(&uri.to_string());
            prop_assert_eq!(again, Ok(uri));
        }
    }

    #[test]
    fn text_like_a_uri_never_panics(text in "sip:[a-z0-9@:;=?&%\\[\\].-]{0,40}") {
        if let Ok(uri) = SipUri::parse(&text) {
            let again = SipUri::parse(&uri.to_string());
            prop_assert_eq!(again, Ok(uri));
        }
    }
}
