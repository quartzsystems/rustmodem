// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Offer/answer cases.

use std::net::{Ipv4Addr, SocketAddrV4};

use sdp::{
    AudioPrefs, Codec, LocalEndpoint, Negotiated, Refusal, SessionDescription, accept_answer,
    answer, offer,
};

const LOCAL: LocalEndpoint = LocalEndpoint {
    address: Ipv4Addr::new(10, 10, 20, 5),
    port: 20000,
    session_id: 77,
    session_version: 1,
};

fn prefs(codecs: &[Codec], ptime_ms: u32) -> AudioPrefs {
    AudioPrefs {
        codecs: codecs.to_vec(),
        ptime_ms,
    }
}

fn both() -> AudioPrefs {
    prefs(&[Codec::Pcmu, Codec::Pcma], 20)
}

/// An offer from 10.10.20.11 with these lines after `t=`.
fn offered(lines: &[&str]) -> SessionDescription {
    let mut text =
        "v=0\r\no=- 5 5 IN IP4 10.10.20.11\r\ns=-\r\nc=IN IP4 10.10.20.11\r\nt=0 0\r\n".to_owned();
    for line in lines {
        text.push_str(line);
        text.push_str("\r\n");
    }
    SessionDescription::parse(text.as_bytes()).expect("the offer parses")
}

fn remote(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 11), port)
}

#[test]
fn our_offer() {
    let text = offer(&both(), &LOCAL).build();
    assert_eq!(
        text,
        "v=0\r\n\
         o=- 77 1 IN IP4 10.10.20.5\r\n\
         s=rustmodem\r\n\
         c=IN IP4 10.10.20.5\r\n\
         t=0 0\r\n\
         m=audio 20000 RTP/AVP 0 8\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=ptime:20\r\n\
         a=sendrecv\r\n\
         a=silenceSupp:off - - - -\r\n"
    );
    let reversed = offer(&prefs(&[Codec::Pcma, Codec::Pcmu], 10), &LOCAL).build();
    assert!(reversed.contains("m=audio 20000 RTP/AVP 8 0\r\n"));
    assert!(reversed.contains("a=ptime:10\r\n"));
}

#[test]
fn our_offer_never_has_telephone_event_comfort_noise_or_a_dynamic_payload() {
    for codecs in [
        &[Codec::Pcmu][..],
        &[Codec::Pcma][..],
        &[Codec::Pcma, Codec::Pcmu][..],
    ] {
        let d = offer(&prefs(codecs, 20), &LOCAL);
        for format in &d.media[0].formats {
            assert!(format == "0" || format == "8");
        }
        let text = d.build();
        assert!(!text.contains("telephone-event"));
        assert!(!text.contains("CN/"));
    }
}

#[test]
fn the_answer_is_the_first_of_our_codecs_that_the_offer_has() {
    // (ours, the offer's formats, the codec agreed)
    let cases: &[(&[Codec], &str, Codec)] = &[
        (&[Codec::Pcmu, Codec::Pcma], "0 8", Codec::Pcmu),
        (&[Codec::Pcmu, Codec::Pcma], "8 0", Codec::Pcmu),
        (&[Codec::Pcma, Codec::Pcmu], "0 8", Codec::Pcma),
        (&[Codec::Pcmu, Codec::Pcma], "8", Codec::Pcma),
        (&[Codec::Pcmu, Codec::Pcma], "18 8 101", Codec::Pcma),
        (&[Codec::Pcma], "0 8 13", Codec::Pcma),
    ];
    for (ours, formats, codec) in cases {
        let o = offered(&[&format!("m=audio 16482 RTP/AVP {formats}")]);
        let (a, n) = answer(&prefs(ours, 20), &LOCAL, &o).expect("the offer is answered");
        assert_eq!(n.codec, *codec, "{formats}");
        assert_eq!(a.media[0].formats, [codec.payload_type().to_string()]);
    }
}

#[test]
fn a_full_answer() {
    let o = offered(&[
        "m=audio 16482 RTP/AVP 0 8 101",
        "a=rtpmap:101 telephone-event/8000",
        "a=fmtp:101 0-15",
        "a=ptime:20",
        "a=sendrecv",
    ]);
    let (a, n) = answer(&both(), &LOCAL, &o).unwrap();
    assert_eq!(
        n,
        Negotiated {
            codec: Codec::Pcmu,
            ptime_ms: 20,
            remote: remote(16482),
            held: false,
        }
    );
    assert_eq!(
        a.build(),
        "v=0\r\n\
         o=- 77 1 IN IP4 10.10.20.5\r\n\
         s=rustmodem\r\n\
         c=IN IP4 10.10.20.5\r\n\
         t=0 0\r\n\
         m=audio 20000 RTP/AVP 0\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=ptime:20\r\n\
         a=sendrecv\r\n\
         a=silenceSupp:off - - - -\r\n"
    );
}

#[test]
fn the_packet_time() {
    // (ours, the offer's a=ptime, the one agreed)
    let cases: &[(u32, Option<&str>, u32)] = &[
        (20, None, 20),
        (20, Some("10"), 10),
        (10, Some("20"), 20),
        (20, Some("30"), 30),
        (20, Some("40"), 20),
        (10, Some("60"), 10),
        (20, Some("0"), 20),
        (20, Some("fast"), 20),
        (30, Some(""), 30),
    ];
    for (ours, theirs, agreed) in cases {
        let ptime = theirs.map(|t| format!("a=ptime:{t}"));
        let mut lines = vec!["m=audio 16482 RTP/AVP 0"];
        if let Some(ptime) = &ptime {
            lines.push(ptime);
        }
        let (a, n) = answer(&prefs(&[Codec::Pcmu], *ours), &LOCAL, &offered(&lines)).unwrap();
        assert_eq!(n.ptime_ms, *agreed, "{theirs:?}");
        assert!(a.build().contains(&format!("a=ptime:{agreed}\r\n")));
    }
}

#[test]
fn what_is_refused() {
    let cases: &[(&[&str], Refusal)] = &[
        (&[], Refusal::NoAudio),
        (&["m=video 5000 RTP/AVP 99"], Refusal::NoAudio),
        (&["m=audio 5000 RTP/SAVP 0"], Refusal::NoAudio),
        (&["m=audio 0 RTP/AVP 0"], Refusal::ZeroPort),
        (&["m=audio 5000 RTP/AVP 18 101"], Refusal::NoCommonCodec),
        (&["m=audio 5000 RTP/AVP 13"], Refusal::NoCommonCodec),
        (
            &["m=audio 5000 RTP/AVP 96", "a=rtpmap:96 PCMU/8000"],
            Refusal::NoCommonCodec,
        ),
        (&["m=image 5000 udptl t38"], Refusal::Image),
        (
            &["m=audio 5000 RTP/AVP 0", "m=image 5002 udptl t38"],
            Refusal::Image,
        ),
        (
            &["m=audio 0 RTP/AVP 0", "m=image 5002 udptl t38"],
            Refusal::Image,
        ),
        (
            &["m=audio 5000 RTP/AVP 0", "c=IN IP6 2001:db8::1"],
            Refusal::BadAddress,
        ),
        (
            &["m=audio 5000 RTP/AVP 0", "c=IN IP4 host.example"],
            Refusal::BadAddress,
        ),
        (
            &["m=audio 5000 RTP/AVP 0", "c=IN IP4 224.2.1.1/127"],
            Refusal::BadAddress,
        ),
    ];
    for (lines, refusal) in cases {
        assert_eq!(
            answer(&both(), &LOCAL, &offered(lines)).map(|(_, n)| n),
            Err(*refusal),
            "{lines:?}"
        );
    }
    // Only PCMA is configured and only PCMU is offered.
    assert_eq!(
        answer(
            &prefs(&[Codec::Pcma], 20),
            &LOCAL,
            &offered(&["m=audio 5000 RTP/AVP 0"])
        )
        .map(|(_, n)| n),
        Err(Refusal::NoCommonCodec)
    );
}

#[test]
fn a_description_with_no_connection_address_is_refused() {
    let o = SessionDescription::parse(
        b"v=0\r\no=- 5 5 IN IP4 10.10.20.11\r\ns=-\r\nt=0 0\r\nm=audio 5000 RTP/AVP 0\r\n",
    )
    .unwrap();
    assert_eq!(
        answer(&both(), &LOCAL, &o).map(|(_, n)| n),
        Err(Refusal::BadAddress)
    );
}

#[test]
fn the_media_level_address_wins() {
    let o = offered(&["m=audio 5000 RTP/AVP 0", "c=IN IP4 10.10.20.99"]);
    let (_, n) = answer(&both(), &LOCAL, &o).unwrap();
    assert_eq!(
        n.remote,
        SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 99), 5000)
    );
}

#[test]
fn hold() {
    // (the offer's lines, held, the direction we answer)
    let cases: &[(&[&str], bool, &str)] = &[
        (&["m=audio 5000 RTP/AVP 0"], false, "sendrecv"),
        (&["m=audio 5000 RTP/AVP 0", "a=sendrecv"], false, "sendrecv"),
        (&["m=audio 5000 RTP/AVP 0", "a=sendonly"], true, "recvonly"),
        (&["m=audio 5000 RTP/AVP 0", "a=inactive"], true, "inactive"),
        (&["m=audio 5000 RTP/AVP 0", "a=recvonly"], false, "sendonly"),
        (
            &["m=audio 5000 RTP/AVP 0", "c=IN IP4 0.0.0.0"],
            true,
            "sendrecv",
        ),
        // The session-level attribute applies when the stream has none.
        (&["a=sendonly", "m=audio 5000 RTP/AVP 0"], true, "recvonly"),
        (
            &["a=sendonly", "m=audio 5000 RTP/AVP 0", "a=sendrecv"],
            false,
            "sendrecv",
        ),
    ];
    for (lines, held, direction) in cases {
        let (a, n) = answer(&both(), &LOCAL, &offered(lines)).unwrap();
        assert_eq!(n.held, *held, "{lines:?}");
        assert!(
            a.media[0]
                .attributes
                .iter()
                .any(|attr| attr.name == *direction && attr.value.is_none()),
            "{lines:?}"
        );
    }
}

#[test]
fn every_other_stream_is_refused_with_port_zero() {
    let o = offered(&[
        "m=video 6000 RTP/AVP 99",
        "a=rtpmap:99 H264/90000",
        "m=audio 5000 RTP/AVP 8",
        "m=audio 5002 RTP/AVP 0",
        "m=image 0 udptl t38",
    ]);
    let (a, n) = answer(&both(), &LOCAL, &o).unwrap();
    assert_eq!(n.codec, Codec::Pcma);
    assert_eq!(n.remote, remote(5000));
    let lines: Vec<(String, u16)> = a.media.iter().map(|m| (m.kind.clone(), m.port)).collect();
    assert_eq!(
        lines,
        [
            ("video".to_owned(), 0),
            ("audio".to_owned(), 20000),
            ("audio".to_owned(), 0),
            ("image".to_owned(), 0),
        ]
    );
    assert!(a.media[0].attributes.is_empty());
}

#[test]
fn an_answer_to_our_offer() {
    let agreed = |lines: &[&str]| accept_answer(&both(), &offered(lines));
    assert_eq!(
        agreed(&["m=audio 7000 RTP/AVP 8", "a=ptime:10"]),
        Ok(Negotiated {
            codec: Codec::Pcma,
            ptime_ms: 10,
            remote: remote(7000),
            held: false,
        })
    );
    // The answerer's order of preference decides.
    assert_eq!(
        agreed(&["m=audio 7000 RTP/AVP 8 0"]).map(|n| (n.codec, n.ptime_ms)),
        Ok((Codec::Pcma, 20))
    );
    assert_eq!(
        agreed(&["m=audio 7000 RTP/AVP 101 0"]).map(|n| n.codec),
        Ok(Codec::Pcmu)
    );
    assert_eq!(agreed(&["m=audio 0 RTP/AVP 0"]), Err(Refusal::ZeroPort));
    assert_eq!(
        agreed(&["m=audio 7000 RTP/AVP 18"]),
        Err(Refusal::NoCommonCodec)
    );
    assert_eq!(agreed(&["m=image 7000 udptl t38"]), Err(Refusal::NoAudio));
    assert_eq!(agreed(&[]), Err(Refusal::NoAudio));
    assert_eq!(
        agreed(&["m=audio 7000 RTP/AVP 0", "a=inactive"]).map(|n| n.held),
        Ok(true)
    );
    // A codec we did not offer is not taken from an answer.
    assert_eq!(
        accept_answer(
            &prefs(&[Codec::Pcmu], 20),
            &offered(&["m=audio 7000 RTP/AVP 8"])
        ),
        Err(Refusal::NoCommonCodec)
    );
}

#[test]
fn our_offer_answers_itself() {
    let ours = offer(&both(), &LOCAL);
    let parsed = SessionDescription::parse(ours.build().as_bytes()).unwrap();
    assert_eq!(parsed, ours);
    let (a, n) = answer(&both(), &LOCAL, &parsed).unwrap();
    assert_eq!(n.codec, Codec::Pcmu);
    assert_eq!(accept_answer(&both(), &a).map(|n| n.codec), Ok(Codec::Pcmu));
}
