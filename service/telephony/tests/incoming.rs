// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! An incoming call: every row of the table in `docs/TELEPHONY.md`, with
//! messages lost and doubled at each step.

mod common;

use std::net::SocketAddrV4;

use common::{
    GATEWAY, ROUTER, STRANGER, World, answer_sdp, body_sdp, description, offer, statuses,
};
use ctlproto::LineState;
use sdp::Codec;
use sip::{Message, Method};
use sippeer::PeerEvent;

/// The gateway's events, as statuses.
fn heard(world: &mut World) -> Vec<String> {
    statuses(&world.gateway.events())
}

#[test]
fn an_invite_routed_to_an_idle_line_is_100_then_180_and_the_line_rings() {
    let mut world = World::new();
    world.call("5551000");

    let events = world.gateway.events();
    assert_eq!(statuses(&events), ["progress 100", "progress 180"]);
    let PeerEvent::Progress {
        response: ringing, ..
    } = &events[1]
    else {
        panic!("not a response");
    };
    // The 180 begins the dialog: it has our tag and our Contact.
    assert!(ringing.headers.to().unwrap().tag().is_some());
    assert_eq!(
        ringing.headers.get("Contact"),
        Some("<sip:5551000@10.10.20.5:5060>")
    );
    assert_eq!(ringing.headers.get("Server"), Some("rustmodem/0.0.3"));
    // The 100 has no tag (RFC 3261 section 8.2.6.2).
    let PeerEvent::Progress {
        response: trying, ..
    } = &events[0]
    else {
        panic!("not a response");
    };
    assert_eq!(trying.headers.to().unwrap().tag(), None);

    // The first idle line of the group is offered the call and rung.
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);

    // The cadence: two seconds on, four seconds off.
    world.advance(1990);
    assert!(world.said().is_empty());
    world.advance(10);
    assert_eq!(world.said(), ["ring end 1"]);
    world.advance(3990);
    assert!(world.said().is_empty());
    world.advance(10);
    assert_eq!(world.said(), ["ring 1"]);
    world.advance(2000);
    assert_eq!(world.said(), ["ring end 1"]);
    // Nothing more goes to the caller while it rings.
    assert!(heard(&mut world).is_empty());
}

#[test]
fn the_host_answers_and_the_call_connects_on_the_ack() {
    let mut world = World::new();
    world.call("5551000");
    world.said();
    world.gateway.events();

    world.answer(1);
    let events = world.gateway.events();
    assert_eq!(statuses(&events), ["answered 200"]);
    let PeerEvent::Answered { response, .. } = &events[0] else {
        panic!("not a response");
    };
    assert_eq!(
        response.headers.content_type().as_deref(),
        Some("application/sdp")
    );
    assert_eq!(
        response.headers.get("Allow"),
        Some("INVITE, ACK, BYE, CANCEL, OPTIONS")
    );

    // The answer: the first of our codecs that the offer has, the offer's
    // packet time, our address, and nothing we do not carry.
    let answer = body_sdp(&response.body);
    let text = answer.build();
    assert_eq!(answer.connection.as_ref().unwrap().address, "10.10.20.5");
    let audio = &answer.media[0];
    assert_eq!(audio.formats, ["0"]);
    assert!(text.contains("a=ptime:20\r\n"));
    assert!(text.contains("a=sendrecv\r\n"));
    assert!(text.contains("a=silenceSupp:off - - - -\r\n"));
    assert!(!text.contains("telephone-event"));

    // The peer acknowledged at once; the line is told the call is
    // through, as the answering modem, and is given the media.
    assert_eq!(world.said(), ["ring end 1", "connected 1 answer"]);
    let (line, endpoint) = &world.media[0];
    assert_eq!(*line, 1);
    let negotiation = endpoint.params.get();
    assert_eq!(
        negotiation.remote,
        "10.10.20.11:16482".parse::<SocketAddrV4>().unwrap()
    );
    assert_eq!(
        (
            negotiation.payload_type,
            negotiation.frames,
            negotiation.held
        ),
        (0, 2, false)
    );
    assert_eq!(endpoint.timeout_ms, 10_000);
    // The socket is the one the answer names, an even port of the range.
    let port = endpoint.socket.local_addr().unwrap().port();
    assert_eq!(port, audio.port);
    assert_eq!(port % 2, 0);
    let range = common::rtp_ports();
    let (first, last) = range.split_once('-').unwrap();
    assert!((first.parse().unwrap()..=last.parse().unwrap()).contains(&port));

    // The 200 is not sent again once it is acknowledged.
    world.advance(5000);
    assert!(heard(&mut world).is_empty());
    assert!(world.said().is_empty());
}

#[test]
fn the_codec_and_the_packet_time_follow_the_offer() {
    let mut world = World::new();
    let alaw = description(&["m=audio 7000 RTP/AVP 8", "a=ptime:10"]);
    world
        .gateway
        .invite(world.now, ROUTER, "sip:5551000@10.10.20.5", Some(&alaw));
    world.settle();
    world.answer(1);
    let negotiation = world.media[0].1.params.get();
    assert_eq!((negotiation.payload_type, negotiation.frames), (8, 1));
    assert_eq!(negotiation.remote.port(), 7000);
}

#[test]
fn an_invite_with_unusable_sdp_is_488() {
    let cases: &[&[&str]] = &[
        &["m=audio 7000 RTP/AVP 18", "a=rtpmap:18 G729/8000"],
        &["m=image 7000 udptl t38"],
        &["m=audio 7000 RTP/AVP 0", "m=image 7002 udptl t38"],
        &["m=audio 0 RTP/AVP 0"],
        &["m=video 7000 RTP/AVP 99"],
        &["m=audio 7000 RTP/SAVP 0"],
        &["m=audio 7000 RTP/AVP 0", "c=IN IP6 2001:db8::1"],
    ];
    for lines in cases {
        let mut world = World::new();
        world.gateway.invite(
            world.now,
            ROUTER,
            "sip:5551000@10.10.20.5",
            Some(&description(lines)),
        );
        world.settle();
        assert_eq!(heard(&mut world), ["refused 488"], "{lines:?}");
        assert!(world.said().is_empty(), "no line is rung");
        assert_eq!(world.logged(), ["RMD5011"]);
    }

    // A body that is not a session description at all.
    let mut world = World::new();
    let mut request = world
        .gateway
        .new_request(Method::Invite, "sip:5551000@10.10.20.5");
    request.headers.push("Contact", "<sip:201@10.10.20.11>");
    request.headers.push("Content-Type", "application/sdp");
    request.body = b"this is not SDP".to_vec();
    world.gateway.invite_with(world.now, ROUTER, request);
    world.settle();
    assert_eq!(heard(&mut world), ["refused 488"]);

    // And one that does not claim to be: 415.
    let mut world = World::new();
    let mut request = world
        .gateway
        .new_request(Method::Invite, "sip:5551000@10.10.20.5");
    request.headers.push("Contact", "<sip:201@10.10.20.11>");
    request.headers.push("Content-Type", "text/plain");
    request.body = b"hello".to_vec();
    world.gateway.invite_with(world.now, ROUTER, request);
    world.settle();
    assert_eq!(heard(&mut world), ["refused 415"]);
}

#[test]
fn an_invite_with_no_sdp_is_offered_ours_in_the_200_and_answered_in_the_ack() {
    let mut world = World::new();
    world
        .gateway
        .invite(world.now, ROUTER, "sip:5551000@10.10.20.5", None);
    world.settle();
    assert_eq!(heard(&mut world), ["progress 100", "progress 180"]);
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);

    // The peer's answer goes in its ACK.
    world.gateway.ack_sdp = Some(answer_sdp(7000, Codec::Pcma));
    world.answer(1);
    let events = world.gateway.events();
    let PeerEvent::Answered { response, .. } = &events[0] else {
        panic!("not answered: {events:?}");
    };
    // Our offer: both codecs, in our order of preference.
    let ours = body_sdp(&response.body);
    assert_eq!(ours.media[0].formats, ["0", "8"]);
    assert_eq!(world.said(), ["ring end 1", "connected 1 answer"]);
    let negotiation = world.media[0].1.params.get();
    assert_eq!(negotiation.payload_type, 8);
    assert_eq!(negotiation.remote.port(), 7000);
}

#[test]
fn an_answer_in_the_ack_that_is_no_use_ends_the_call() {
    for answer in [None, Some(description(&["m=audio 7000 RTP/AVP 18"]))] {
        let mut world = World::new();
        world
            .gateway
            .invite(world.now, ROUTER, "sip:5551000@10.10.20.5", None);
        world.settle();
        world.said();
        world.gateway.events();
        world.gateway.ack_sdp = answer;
        world.answer(1);
        assert_eq!(world.said(), ["ring end 1", "failed 1"]);
        assert_eq!(heard(&mut world), ["answered 200", "hung up"]);
        assert!(world.media.is_empty());
    }
}

#[test]
fn a_call_with_nowhere_to_go_is_404_486_or_480() {
    // No route and no pilot: 404.
    let mut world = World::new();
    world.call("5559999");
    assert_eq!(heard(&mut world), ["refused 404"]);
    assert_eq!(world.logged(), ["RMD5011"]);

    // Every member in a call: 486.
    let mut world = World::new();
    world.lines.insert(1, LineState::Online);
    world.lines.insert(2, LineState::Handshaking);
    world.call("5551000");
    assert_eq!(heard(&mut world), ["refused 486"]);

    // No member ready: 480.
    let mut world = World::new();
    world.lines.insert(1, LineState::NotReady);
    world.lines.insert(2, LineState::Disabled);
    world.call("5551000");
    assert_eq!(heard(&mut world), ["refused 480"]);

    // One in a call and the other idle: the idle one.
    let mut world = World::new();
    world.lines.insert(1, LineState::Online);
    world.call("5551000");
    assert_eq!(world.said(), ["incoming 2 from 201", "ring 2"]);

    // A route is before the pilots, and a route to a busy line is busy.
    let mut world = World::new();
    world.call("5552345");
    assert_eq!(world.said(), ["incoming 2 from 201", "ring 2"]);
    let mut world = World::new();
    world.lines.insert(2, LineState::Online);
    world.call("5552345");
    assert_eq!(heard(&mut world), ["refused 486"]);
    assert!(world.said().is_empty());
}

#[test]
fn a_cancel_before_the_answer_is_200_and_487_and_the_line_stops_ringing() {
    let mut world = World::new();
    let call = world.call("5551000");
    world.said();
    world.gateway.events();

    assert!(world.gateway.cancel(world.now, call));
    world.settle();
    // The 200 to the CANCEL is the peer's own business; the INVITE ends
    // with 487.
    assert_eq!(heard(&mut world), ["refused 487"]);
    assert_eq!(world.said(), ["ring end 1", "abandoned 1"]);
    // The line can be rung again.
    world.call("5551000");
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);

    // A CANCEL while the ring is in its off interval says only that the
    // caller gave up.
    let mut world = World::new();
    let call = world.call("5551000");
    world.advance(3000);
    world.said();
    world.gateway.cancel(world.now, call);
    world.settle();
    assert_eq!(world.said(), ["abandoned 1"]);
}

#[test]
fn a_cancel_for_nothing_is_481() {
    let mut world = World::new();
    let mut cancel = world
        .gateway
        .new_request(Method::Cancel, "sip:5551000@10.10.20.5");
    cancel.headers.set("CSeq", "1 CANCEL");
    world.gateway.request(world.now, ROUTER, cancel);
    world.settle();
    assert_eq!(heard(&mut world), ["response 481"]);
}

#[test]
fn a_line_nobody_answers_is_480_after_the_ring_timeout() {
    let mut world = World::new();
    world.call("5551000");
    world.advance(59_990);
    world.gateway.events();
    world.said();
    world.advance(10);
    assert_eq!(heard(&mut world), ["refused 480"]);
    assert_eq!(world.said(), ["abandoned 1"]);
    // Answering now is answering nothing.
    world.answer(1);
    assert_eq!(world.said(), ["failed 1"]);
}

#[test]
fn a_line_that_hangs_up_while_ringing_declines_the_call() {
    let mut world = World::new();
    world.call("5551000");
    world.said();
    world.gateway.events();
    world.hangup(1);
    assert_eq!(heard(&mut world), ["refused 603"]);
    // The line did it itself: it is told nothing.
    assert!(world.said().is_empty());
    world.advance(10_000);
    assert!(world.said().is_empty());
}

#[test]
fn answering_when_nothing_is_calling_gets_nothing() {
    let mut world = World::new();
    world.answer(2);
    assert_eq!(world.said(), ["failed 2"]);
}

#[test]
fn two_calls_racing_for_the_last_idle_line() {
    // One idle line in the group: the calls arrive in the same instant.
    let mut world = World::new();
    world.lines.insert(1, LineState::Online);
    let first = world.gateway.invite(
        world.now,
        ROUTER,
        "sip:5551000@10.10.20.5",
        Some(&offer(7000)),
    );
    let second = world.gateway.invite(
        world.now,
        ROUTER,
        "sip:5551000@10.10.20.5",
        Some(&offer(7002)),
    );
    world.settle();
    // The pool has heard of neither yet. The first has the line; the
    // second is busy.
    assert_eq!(world.said(), ["incoming 2 from 201", "ring 2"]);
    let events = world.gateway.events();
    let of = |call| {
        events
            .iter()
            .filter(|event| match event {
                PeerEvent::Progress { call: c, .. } | PeerEvent::Refused { call: c, .. } => {
                    *c == call
                }
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(statuses(&of(first)), ["progress 100", "progress 180"]);
    assert_eq!(statuses(&of(second)), ["refused 486"]);

    // Two idle lines: each call gets its own.
    let mut world = World::new();
    world.gateway.invite(
        world.now,
        ROUTER,
        "sip:5551000@10.10.20.5",
        Some(&offer(7000)),
    );
    world.gateway.invite(
        world.now,
        ROUTER,
        "sip:5551000@10.10.20.5",
        Some(&offer(7002)),
    );
    world.settle();
    assert_eq!(
        world.said(),
        [
            "incoming 1 from 201",
            "ring 1",
            "incoming 2 from 201",
            "ring 2"
        ]
    );
}

#[test]
fn a_request_from_an_address_that_is_not_trusted_gets_no_answer() {
    let mut world = World::new();
    world.call("5551000");
    world.said();
    let invite = world.seen.lock().unwrap().wire.clone();
    let invite = &invite[..invite.find("SIP/2.0 100").unwrap()];
    let before = world.seen.lock().unwrap().trace.len();

    // The same INVITE from somewhere else, with another branch so that it
    // is no retransmission.
    let other = invite.replace("branch=z9hG4bK", "branch=z9hG4bKx");
    world.inject(STRANGER, other.as_bytes());
    world.settle();
    world.advance(1000);
    assert!(world.said().is_empty(), "no line is rung");
    // Nothing was sent to anybody for it, and it was not even traced.
    assert_eq!(world.seen.lock().unwrap().trace.len(), before);
    assert_eq!(world.handle.status().unwrap().dropped_untrusted, 1);
    assert_eq!(world.logged(), ["RMD5010"]);

    // An OPTIONS from there is dropped the same; the count goes up and
    // the log is not filled.
    for n in 0..150 {
        let options = format!(
            "OPTIONS sip:10.10.20.5 SIP/2.0\r\nVia: SIP/2.0/UDP 203.0.113.9;branch=z9hG4bKo{n}\r\n\
             From: <sip:x@203.0.113.9>;tag=1\r\nTo: <sip:10.10.20.5>\r\nCall-ID: o{n}\r\nCSeq: 1 OPTIONS\r\n\r\n"
        );
        world.inject(STRANGER, options.as_bytes());
    }
    world.settle();
    assert_eq!(world.handle.status().unwrap().dropped_untrusted, 151);
    assert_eq!(world.logged(), ["RMD5010"]);
    assert_eq!(world.seen.lock().unwrap().trace.len(), before);
}

#[test]
fn with_no_trusted_peers_and_no_registrations_nothing_is_accepted() {
    let config = common::CONFIG
        .replace("trusted_peers = [\"10.10.20.11\"]", "trusted_peers = []")
        .replace(
            "register = { server = \"pbx.example.lan\", user = \"7003\", password = \"secret\" }",
            "",
        );
    let mut world = World::with_config(&config);
    world.call("5551000");
    world.advance(2000);
    assert!(world.said().is_empty());
    assert!(heard(&mut world).is_empty(), "the gateway hears nothing");
    assert!(world.handle.status().unwrap().dropped_untrusted >= 1);
}

#[test]
fn a_block_of_addresses_can_be_trusted() {
    let config = common::CONFIG.replace("[\"10.10.20.11\"]", "[\"192.0.2.1\", \"10.10.20.0/24\"]");
    let mut world = World::with_config(&config);
    world.call("5551000");
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);
}

#[test]
fn requests_that_cannot_begin_a_call() {
    // An extension is required: 420, naming it.
    let mut world = World::new();
    let mut request = world
        .gateway
        .new_request(Method::Invite, "sip:5551000@10.10.20.5");
    request.headers.push("Contact", "<sip:201@10.10.20.11>");
    request.headers.push("Require", "100rel, timer");
    world.gateway.invite_with(world.now, ROUTER, request);
    world.settle();
    let events = world.gateway.events();
    assert_eq!(statuses(&events), ["refused 420"]);
    let PeerEvent::Refused { response, .. } = &events[0] else {
        panic!("not refused");
    };
    assert_eq!(response.headers.get("Unsupported"), Some("100rel, timer"));

    // A Request-URI that is not sip: 416.
    let mut world = World::new();
    world
        .gateway
        .invite(world.now, ROUTER, "tel:+15551000", Some(&offer(7000)));
    world.settle();
    assert_eq!(heard(&mut world), ["refused 416"]);

    // No Contact to send the dialog's requests to: 400.
    let mut world = World::new();
    let request = world
        .gateway
        .new_request(Method::Invite, "sip:5551000@10.10.20.5");
    world.gateway.invite_with(world.now, ROUTER, request);
    world.settle();
    assert_eq!(heard(&mut world), ["refused 400"]);
    assert!(world.said().is_empty());
}

// ---------------------------------------------------------------------
// Messages lost and doubled.

/// Loses the first message that matches, and no other.
fn lose_first(mut matches: impl FnMut(&Message) -> bool + 'static) -> common::Filter {
    let mut lost = false;
    Box::new(move |message| {
        if !lost && matches(message) {
            lost = true;
            true
        } else {
            false
        }
    })
}

fn is_status(message: &Message, status: u16) -> bool {
    matches!(message, Message::Response(response) if response.status == status)
}

fn is_method(message: &Message, method: Method) -> bool {
    matches!(message, Message::Request(request) if request.method == method)
}

#[test]
fn a_lost_invite_is_sent_again_and_rings_once() {
    let mut world = World::new();
    world.lose_to_router = Some(lose_first(|m| is_method(m, Method::Invite)));
    world.call("5551000");
    assert!(world.said().is_empty());
    world.advance(500);
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);
    assert_eq!(heard(&mut world), ["progress 100", "progress 180"]);
}

#[test]
fn a_lost_100_and_180_are_sent_again_when_the_invite_is() {
    let mut world = World::new();
    let mut lost = 0;
    world.lose_from_router = Some(Box::new(move |message| {
        // The first 100 and the first 180.
        if lost < 2 && (is_status(message, 100) || is_status(message, 180)) {
            lost += 1;
            true
        } else {
            false
        }
    }));
    world.call("5551000");
    assert!(heard(&mut world).is_empty());
    // The line rings all the same, once.
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);
    // The peer sends its INVITE again and gets the last response again.
    world.advance(500);
    assert_eq!(heard(&mut world), ["progress 180"]);
    assert!(
        world.said().is_empty(),
        "the INVITE again is not a new call"
    );
}

#[test]
fn a_lost_200_is_sent_again_until_it_is_acknowledged() {
    let mut world = World::new();
    world.call("5551000");
    world.said();
    world.gateway.events();
    world.lose_from_router = Some(lose_first(|m| is_status(m, 200)));
    world.answer(1);
    assert_eq!(world.said(), ["ring end 1"]);
    assert!(heard(&mut world).is_empty());
    // After T1 it goes again, and this time it arrives.
    world.advance(500);
    assert_eq!(heard(&mut world), ["answered 200"]);
    assert_eq!(world.said(), ["connected 1 answer"]);
    world.advance(10_000);
    assert!(heard(&mut world).is_empty());
}

#[test]
fn a_lost_ack_is_sent_again_when_the_200_is() {
    let mut world = World::new();
    world.call("5551000");
    world.said();
    world.gateway.events();
    world.lose_to_router = Some(lose_first(|m| is_method(m, Method::Ack)));
    world.answer(1);
    assert_eq!(heard(&mut world), ["answered 200"]);
    assert_eq!(
        world.said(),
        ["ring end 1"],
        "not connected without the ACK"
    );
    world.advance(500);
    assert_eq!(world.said(), ["connected 1 answer"]);
    assert_eq!(world.media.len(), 1, "and connected once");
}

#[test]
fn a_200_that_is_never_acknowledged_ends_the_call_with_a_bye() {
    let mut world = World::new();
    world.call("5551000");
    world.said();
    world.lose_to_router = Some(Box::new(|m| is_method(m, Method::Ack)));
    world.answer(1);
    world.gateway.events();
    world.advance(31_990);
    assert_eq!(world.said(), ["ring end 1"]);
    world.advance(10);
    // 64 * T1: the line is told the call failed, and the far end, which
    // thinks it has a call, is sent a BYE.
    assert_eq!(world.said(), ["failed 1"]);
    assert_eq!(heard(&mut world), ["hung up"]);
    assert_eq!(world.logged(), ["RMD5013"]);
    assert!(world.media.is_empty());
}

#[test]
fn every_message_doubled_changes_nothing() {
    let mut world = World::new();
    world.duplicate = true;
    let call = world.call("5551000");
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);
    world.answer(1);
    assert_eq!(world.said(), ["ring end 1", "connected 1 answer"]);
    assert_eq!(world.media.len(), 1);
    world.advance(2000);
    world.gateway.bye(world.now, call);
    world.settle();
    assert_eq!(world.said(), ["cleared 1"]);
    world.advance(40_000);
    assert!(world.said().is_empty());
    let answered = heard(&mut world);
    assert_eq!(answered.iter().filter(|e| *e == "answered 200").count(), 1);
    assert_eq!(
        answered.iter().filter(|e| *e == "BYE answered 200").count(),
        1
    );
}

#[test]
fn the_caller_is_who_the_from_says() {
    let mut world = World::new();
    let mut request = world
        .gateway
        .new_request(Method::Invite, "sip:5551000@10.10.20.5");
    request.headers.set(
        "From",
        "\"Front desk\" <sip:+1%35551234@10.10.20.11>;tag=f1",
    );
    request.headers.push("Contact", "<sip:201@10.10.20.11>");
    world.gateway.invite_with(world.now, ROUTER, request);
    world.settle();
    assert_eq!(world.said(), ["incoming 1 from +15551234", "ring 1"]);

    // A From that is no sip: URI is nobody we can name.
    let mut world = World::new();
    let mut request = world
        .gateway
        .new_request(Method::Invite, "sip:5551000@10.10.20.5");
    request.headers.set("From", "<tel:+15551234>;tag=f1");
    request.headers.push("Contact", "<sip:201@10.10.20.11>");
    world.gateway.invite_with(world.now, ROUTER, request);
    world.settle();
    assert_eq!(world.said(), ["incoming 1 from anonymous", "ring 1"]);
    let _ = GATEWAY;
}
