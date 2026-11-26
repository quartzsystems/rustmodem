// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! In a call: BYE both ways, re-INVITE, OPTIONS, and the methods that are
//! not taken.

mod common;

use std::net::{Ipv4Addr, SocketAddrV4};

use common::{ROUTER, World, body_sdp, description, statuses};
use pool::CallRouter;
use sip::{Message, Method};
use sippeer::PeerEvent;

/// A call from the gateway that line 1 has answered.
fn connected() -> (World, sippeer::Call) {
    let mut world = World::new();
    let call = world.call("5551000");
    world.answer(1);
    assert_eq!(
        world.said(),
        [
            "incoming 1 from 201",
            "ring 1",
            "ring end 1",
            "connected 1 answer"
        ]
    );
    world.gateway.events();
    (world, call)
}

fn heard(world: &mut World) -> Vec<String> {
    statuses(&world.gateway.events())
}

fn is_method(message: &Message, method: Method) -> bool {
    matches!(message, Message::Request(request) if request.method == method)
}

#[test]
fn a_bye_from_the_far_end_is_200_and_the_line_is_cleared() {
    let (mut world, call) = connected();
    world.advance(5000);
    world.gateway.bye(world.now, call);
    world.settle();
    assert_eq!(heard(&mut world), ["BYE answered 200"]);
    assert_eq!(world.said(), ["cleared 1"]);
    assert!(!world.router.busy());
    assert_eq!(world.handle.status().unwrap().calls, 0);
    // The line can take the next call.
    world.call("5551000");
    assert_eq!(world.said(), ["incoming 1 from 201", "ring 1"]);
}

#[test]
fn a_line_that_hangs_up_sends_a_bye() {
    let (mut world, _) = connected();
    world.hangup(1);
    assert_eq!(heard(&mut world), ["hung up"]);
    // The line did it itself: it is told nothing.
    assert!(world.said().is_empty());
    assert!(!world.router.busy(), "the BYE was answered");
    // Hanging up again is hanging up nothing.
    world.hangup(1);
    assert!(heard(&mut world).is_empty());
}

#[test]
fn a_bye_that_is_not_answered_is_sent_again_and_then_given_up() {
    let (mut world, _) = connected();
    let mut byes = 0;
    world.lose_from_router = Some(Box::new(move |message| {
        if is_method(message, Method::Bye) {
            byes += 1;
            // The first two are lost.
            byes <= 2
        } else {
            false
        }
    }));
    world.hangup(1);
    assert!(heard(&mut world).is_empty());
    assert!(world.router.busy());
    world.advance(500);
    assert!(heard(&mut world).is_empty());
    world.advance(1000);
    assert_eq!(heard(&mut world), ["hung up"]);
    assert!(!world.router.busy());

    // Never answered at all: the router gives up at 64 * T1 and the call
    // is gone from it.
    let (mut world, _) = connected();
    world.lose_from_router = Some(Box::new(|message| is_method(message, Method::Bye)));
    world.hangup(1);
    world.advance(31_990);
    assert!(world.router.busy());
    world.advance(10);
    assert!(!world.router.busy());
    assert_eq!(world.handle.status().unwrap().calls, 0);
    assert!(world.said().is_empty());
}

#[test]
fn a_reinvite_that_is_still_g711_is_200_and_the_media_follows_it() {
    let (mut world, call) = connected();
    let params = std::sync::Arc::clone(&world.media[0].1.params);
    assert_eq!(params.get().remote.ip(), &Ipv4Addr::new(10, 10, 20, 11));

    // The far end moves to another address, to PCMA and to 10 ms.
    let moved = description(&[
        "m=audio 9000 RTP/AVP 8",
        "c=IN IP4 10.10.20.77",
        "a=ptime:10",
    ]);
    world
        .gateway
        .in_call(world.now, call, Method::Invite, Some(&moved));
    world.settle();
    let events = world.gateway.events();
    assert_eq!(statuses(&events), ["INVITE answered 200"]);
    let PeerEvent::InCallAnswered { response, .. } = &events[0] else {
        panic!("not a response");
    };
    let ours = body_sdp(&response.body);
    assert_eq!(ours.media[0].formats, ["8"]);
    assert_eq!(
        ours.origin.session_version, "2",
        "a new version of the same session"
    );
    assert!(ours.build().contains("a=ptime:10\r\n"));
    assert_eq!(
        response.headers.get("Contact"),
        Some("<sip:5551000@10.10.20.5:5060>")
    );

    let negotiation = params.get();
    assert_eq!(
        negotiation.remote,
        SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 77), 9000)
    );
    assert_eq!(
        (
            negotiation.payload_type,
            negotiation.frames,
            negotiation.held
        ),
        (8, 1, false)
    );
    // The line hears nothing of it, and the 200 is not sent again once
    // the peer has acknowledged it.
    world.advance(5000);
    assert!(world.said().is_empty());
    assert!(heard(&mut world).is_empty());
}

#[test]
fn a_reinvite_for_t38_or_with_no_codec_of_ours_is_488_and_the_call_goes_on() {
    for lines in [
        &["m=image 9000 udptl t38"][..],
        &["m=audio 0 RTP/AVP 0", "m=image 9000 udptl t38"][..],
        &["m=audio 9000 RTP/AVP 18"][..],
    ] {
        let (mut world, call) = connected();
        let before = world.media[0].1.params.get();
        world
            .gateway
            .in_call(world.now, call, Method::Invite, Some(&description(lines)));
        world.settle();
        assert_eq!(heard(&mut world), ["INVITE answered 488"], "{lines:?}");
        assert_eq!(
            world.media[0].1.params.get(),
            before,
            "the media is as it was"
        );
        assert!(world.said().is_empty());
        // The call is still there to be ended.
        world.gateway.bye(world.now, call);
        world.settle();
        assert_eq!(world.said(), ["cleared 1"]);
    }
}

#[test]
fn a_far_end_that_holds_the_call_is_accepted_and_the_media_is_told() {
    for (lines, direction) in [
        (&["m=audio 16482 RTP/AVP 0", "a=sendonly"][..], "a=recvonly"),
        (&["m=audio 16482 RTP/AVP 0", "a=inactive"][..], "a=inactive"),
        (
            &["m=audio 16482 RTP/AVP 0", "c=IN IP4 0.0.0.0"][..],
            "a=sendrecv",
        ),
    ] {
        let (mut world, call) = connected();
        world
            .gateway
            .in_call(world.now, call, Method::Invite, Some(&description(lines)));
        world.settle();
        let events = world.gateway.events();
        assert_eq!(statuses(&events), ["INVITE answered 200"], "{lines:?}");
        let PeerEvent::InCallAnswered { response, .. } = &events[0] else {
            panic!("not a response");
        };
        assert!(body_sdp(&response.body).build().contains(direction));
        assert!(world.media[0].1.params.get().held, "{lines:?}");

        // Off hold again.
        let resumed = description(&["m=audio 16482 RTP/AVP 0", "a=sendrecv"]);
        world
            .gateway
            .in_call(world.now, call, Method::Invite, Some(&resumed));
        world.settle();
        let negotiation = world.media[0].1.params.get();
        assert!(!negotiation.held);
        assert_eq!(negotiation.remote.to_string(), "10.10.20.11:16482");
        assert!(world.said().is_empty());
    }
}

#[test]
fn a_reinvite_with_no_sdp_is_offered_what_we_have() {
    let (mut world, call) = connected();
    world.gateway.in_call(world.now, call, Method::Invite, None);
    world.settle();
    let events = world.gateway.events();
    let PeerEvent::InCallAnswered { response, .. } = &events[0] else {
        panic!("not a response: {events:?}");
    };
    assert_eq!(response.status, 200);
    assert_eq!(body_sdp(&response.body).media[0].formats, ["0", "8"]);
}

#[test]
fn the_200_of_a_reinvite_is_sent_again_until_it_is_acknowledged() {
    let (mut world, call) = connected();
    let mut lost = 0;
    world.lose_from_router = Some(Box::new(move |message| {
        let ok = matches!(message, Message::Response(r) if r.status == 200);
        if ok && lost < 2 {
            lost += 1;
            true
        } else {
            false
        }
    }));
    let again = description(&["m=audio 16482 RTP/AVP 0"]);
    world
        .gateway
        .in_call(world.now, call, Method::Invite, Some(&again));
    world.settle();
    assert!(heard(&mut world).is_empty());
    world.advance(1500);
    assert_eq!(heard(&mut world), ["INVITE answered 200"]);
    world.advance(20_000);
    assert!(heard(&mut world).is_empty());
}

#[test]
fn options_is_200_with_what_is_allowed() {
    // Outside any call.
    let mut world = World::new();
    let options = world.gateway.new_request(Method::Options, "sip:10.10.20.5");
    world.gateway.request(world.now, ROUTER, options);
    world.settle();
    let events = world.gateway.events();
    let PeerEvent::Response { response, .. } = &events[0] else {
        panic!("not a response: {events:?}");
    };
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers.get("Allow"),
        Some("INVITE, ACK, BYE, CANCEL, OPTIONS")
    );
    assert_eq!(response.headers.get("Accept"), Some("application/sdp"));
    // No extension is advertised.
    assert!(response.headers.supported().is_empty());

    // And inside one.
    let (mut world, call) = connected();
    world
        .gateway
        .in_call(world.now, call, Method::Options, None);
    world.settle();
    assert_eq!(heard(&mut world), ["OPTIONS answered 200"]);
    assert!(world.said().is_empty());
}

#[test]
fn any_other_method_is_405_or_501_with_allow() {
    for (method, status) in [
        (Method::Register, 405),
        (Method::Other("INFO".to_owned()), 405),
        (Method::Other("REFER".to_owned()), 405),
        (Method::Other("UPDATE".to_owned()), 405),
        (Method::Other("SUBSCRIBE".to_owned()), 405),
        (Method::Other("MESSAGE".to_owned()), 405),
        (Method::Other("PRACK".to_owned()), 405),
        (Method::Other("FROBNICATE".to_owned()), 501),
        (Method::Other("invite".to_owned()), 501),
    ] {
        let mut world = World::new();
        let request = world
            .gateway
            .new_request(method.clone(), "sip:5551000@10.10.20.5");
        world.gateway.request(world.now, ROUTER, request);
        world.settle();
        let events = world.gateway.events();
        let PeerEvent::Response { response, .. } = &events[0] else {
            panic!("{method}: not a response: {events:?}");
        };
        assert_eq!(response.status, status, "{method}");
        assert_eq!(
            response.headers.get("Allow"),
            Some("INVITE, ACK, BYE, CANCEL, OPTIONS"),
            "{method}"
        );
        assert!(world.said().is_empty());
    }

    // Inside a call too, and the call goes on.
    let (mut world, call) = connected();
    world
        .gateway
        .in_call(world.now, call, Method::Other("INFO".to_owned()), None);
    world.settle();
    assert_eq!(heard(&mut world), ["INFO answered 405"]);
    assert!(world.said().is_empty());
}

#[test]
fn a_request_that_requires_an_extension_is_420() {
    let (mut world, _) = connected();
    let mut options = world.gateway.new_request(Method::Options, "sip:10.10.20.5");
    options.headers.push("Require", "timer");
    world.gateway.request(world.now, ROUTER, options);
    world.settle();
    let events = world.gateway.events();
    let PeerEvent::Response { response, .. } = &events[0] else {
        panic!("not a response: {events:?}");
    };
    assert_eq!(response.status, 420);
    assert_eq!(response.headers.get("Unsupported"), Some("timer"));
}

#[test]
fn a_bye_or_a_reinvite_for_a_call_that_is_not_there_is_481() {
    for method in [Method::Bye, Method::Invite] {
        let (mut world, _) = connected();
        let mut request = world
            .gateway
            .new_request(method.clone(), "sip:5551000@10.10.20.5");
        request
            .headers
            .set("To", "<sip:5551000@10.10.20.5>;tag=no-such-dialog");
        request.headers.push("Contact", "<sip:201@10.10.20.11>");
        world.gateway.request(world.now, ROUTER, request);
        world.settle();
        let events = world.gateway.events();
        let PeerEvent::Response { response, .. } = &events[0] else {
            panic!("{method}: not a response: {events:?}");
        };
        assert_eq!(response.status, 481, "{method}");
        assert!(world.said().is_empty(), "the call that is there goes on");
    }
}

#[test]
fn a_far_end_that_hangs_up_before_its_ack_fails_the_call() {
    let mut world = World::new();
    let call = world.call("5551000");
    world.said();
    world.gateway.events();
    // The ACK never arrives; the BYE does.
    world.lose_to_router = Some(Box::new(|message| is_method(message, Method::Ack)));
    world.answer(1);
    assert_eq!(world.said(), ["ring end 1"]);
    world.gateway.bye(world.now, call);
    world.settle();
    // The line was waiting for the call to be through: it failed.
    assert_eq!(world.said(), ["failed 1"]);
    assert_eq!(heard(&mut world), ["answered 200", "BYE answered 200"]);
    world.advance(40_000);
    assert!(world.said().is_empty());
    assert!(!world.router.busy());
}

#[test]
fn a_line_that_hangs_up_before_the_ack_sends_its_bye_after_it() {
    // The ACK is late.
    let mut world = World::new();
    world.call("5551000");
    world.said();
    world.gateway.events();
    let mut acks = 0;
    world.lose_to_router = Some(Box::new(move |message| {
        if is_method(message, Method::Ack) {
            acks += 1;
            acks == 1
        } else {
            false
        }
    }));
    world.answer(1);
    world.hangup(1);
    // No BYE yet: it may not go before the ACK (RFC 3261 section 15).
    assert_eq!(heard(&mut world), ["answered 200"]);
    world.advance(500);
    // The 200 went again, its ACK came, and the BYE followed.
    assert_eq!(heard(&mut world), ["hung up"]);
    assert_eq!(
        world.said(),
        ["ring end 1"],
        "the line is told nothing more"
    );
    assert!(world.media.is_empty(), "and no media was started");

    // The ACK never comes: the BYE goes when the wait for it is over.
    let mut world = World::new();
    world.call("5551000");
    world.lose_to_router = Some(Box::new(|message| is_method(message, Method::Ack)));
    world.answer(1);
    world.hangup(1);
    world.gateway.events();
    world.said();
    world.advance(32_000);
    assert_eq!(heard(&mut world), ["hung up"]);
    assert!(world.said().is_empty());
}
