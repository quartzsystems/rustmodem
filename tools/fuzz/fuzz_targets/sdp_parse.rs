// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `sdp_parse`: no input makes the session description parser
//! or the offer/answer negotiation panic, and a description that parses is
//! the same description after it is built and parsed again.

#![no_main]

use std::net::Ipv4Addr;

use libfuzzer_sys::fuzz_target;
use sdp::{AudioPrefs, Codec, LocalEndpoint, SessionDescription, accept_answer, answer};

fuzz_target!(|data: &[u8]| {
    let Ok(description) = SessionDescription::parse(data) else {
        return;
    };
    let text = description.build();
    let again = SessionDescription::parse(text.as_bytes());
    assert_eq!(again.as_ref(), Ok(&description), "built as {text:?}");

    let prefs = AudioPrefs {
        codecs: vec![Codec::Pcma, Codec::Pcmu],
        ptime_ms: 20,
    };
    let local = LocalEndpoint {
        address: Ipv4Addr::new(192, 0, 2, 1),
        port: 20000,
        session_id: 1,
        session_version: 1,
    };
    if let Ok((ours, agreed)) = answer(&prefs, &local, &description) {
        assert_eq!(ours.media.len(), description.media.len());
        assert_eq!(ours.media.iter().filter(|m| m.port != 0).count(), 1);
        assert!(matches!(agreed.ptime_ms, 10 | 20 | 30));
        // Our own answer is one the far end's rules accept.
        let parsed = SessionDescription::parse(ours.build().as_bytes());
        assert_eq!(parsed.as_ref(), Ok(&ours));
    }
    let _ = accept_answer(&prefs, &description);
});
