// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `sip_message`: no datagram makes the SIP parser, the URI
//! parser or a typed header getter panic, and a message that parses is the
//! same message when it is built and parsed again.

#![no_main]

use libfuzzer_sys::fuzz_target;
use sip::{Message, SipUri};

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(uri) = SipUri::parse(text)
    {
        let written = uri.to_string();
        assert_eq!(SipUri::parse(&written).as_ref(), Ok(&uri), "{written:?}");
    }

    let Ok(message) = Message::parse(data) else {
        return;
    };
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
    match &message {
        Message::Request(request) => {
            let _ = request.essentials();
            let _ = request.sip_uri();
        }
        Message::Response(response) => {
            let _ = response.essentials();
        }
    }

    let built = message.to_bytes();
    let again = Message::parse(&built);
    assert_eq!(again.as_ref(), Ok(&message));
});
