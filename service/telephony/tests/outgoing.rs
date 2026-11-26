// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! An outgoing call: `ATD<number>` from a trunk line and from a registered
//! line, every answer the far end can give, and messages lost on the way.

mod common;

use common::{World, answer_sdp, body_sdp, description, statuses};
use pool::CallRouter;
use sdp::Codec;
use sip::digest::Algorithm;
use sip::{Credentials, Message, Method};
use sippeer::{Challenge, PeerEvent};

fn heard(world: &mut World) -> Vec<String> {
    statuses(&world.gateway.events())
}

/// The call the gateway was offered, and its INVITE.
fn offered(world: &mut World) -> (sippeer::Call, sip::Request) {
    match world.gateway.event() {
        Some(PeerEvent::Incoming { call, request }) => (call, request),
        other => panic!("the gateway was not called: {other:?}"),
    }
}

fn is_method(message: &Message, method: Method) -> bool {
    matches!(message, Message::Request(request) if request.method == method)
}

#[test]
fn a_trunk_line_with_no_outbound_peer_has_no_dial_tone() {
    let config = common::CONFIG.replace("outbound_peer = \"10.10.20.11\"", "outbound_peer = \"\"");
    let mut world = World::with_config(&config);
    world.dial(1, "5551234");
    assert_eq!(world.said(), ["no dialtone 1"]);
    assert!(heard(&mut world).is_empty());
    assert!(!world.router.busy());
}

#[test]
fn a_trunk_line_calls_through_the_outbound_peer() {
    let mut world = World::new();
    world.dial(1, "5551234");
    let (call, invite) = offered(&mut world);
    assert_eq!(invite.uri, "sip:5551234@10.10.20.11");
    assert_eq!(invite.headers.to().unwrap().uri, "sip:5551234@10.10.20.11");
    assert_eq!(invite.headers.from().unwrap().uri, "sip:1@10.10.20.11");
    assert!(invite.headers.from().unwrap().tag().is_some());
    assert_eq!(
        invite.headers.get("Contact"),
        Some("<sip:1@10.10.20.5:5060>")
    );
    assert_eq!(invite.headers.get("User-Agent"), Some("rustmodem/0.0.3"));
    assert_eq!(
        invite.headers.get("Allow"),
        Some("INVITE, ACK, BYE, CANCEL, OPTIONS")
    );
    assert_eq!(invite.headers.max_forwards(), Some(70));
    assert!(invite.headers.supported().is_empty());
    let via = invite.headers.top_via().unwrap();
    assert_eq!((via.host.as_str(), via.port), ("10.10.20.5", Some(5060)));
    assert!(via.params.has("rport"));

    // Our offer: G.711 in our order, 20 ms, and nothing else.
    let ours = body_sdp(&invite.body);
    let text = ours.build();
    assert_eq!(ours.media.len(), 1);
    assert_eq!(ours.media[0].formats, ["0", "8"]);
    assert_eq!(ours.connection.as_ref().unwrap().address, "10.10.20.5");
    assert!(text.contains("a=ptime:20\r\n"));
    assert!(text.contains("a=silenceSupp:off - - - -\r\n"));
    assert!(!text.contains("telephone-event"));

    // 180 and 183 are noted; nothing is said to the line, and what a 183
    // carries is not listened to.
    world.gateway.ring(world.now, call, 180);
    world.gateway.ring(world.now, call, 183);
    world.settle();
    assert!(world.said().is_empty());
    assert!(world.media.is_empty());

    // 200: the ACK goes, and the line is through as the originating modem.
    world
        .gateway
        .answer(world.now, call, Some(&answer_sdp(7000, Codec::Pcma)));
    world.settle();
    assert_eq!(world.said(), ["connected 1 originate"]);
    assert_eq!(heard(&mut world), ["acknowledged"]);
    let (line, endpoint) = &world.media[0];
    assert_eq!(*line, 1);
    let negotiation = endpoint.params.get();
    assert_eq!(negotiation.remote.to_string(), "10.10.20.11:7000");
    assert_eq!((negotiation.payload_type, negotiation.frames), (8, 2));
    assert_eq!(
        endpoint.socket.local_addr().unwrap().port(),
        ours.media[0].port
    );

    // The far end hangs up.
    world.gateway.bye(world.now, call);
    world.settle();
    assert_eq!(world.said(), ["cleared 1"]);
    assert!(!world.router.busy());
}

#[test]
fn what_the_far_end_refuses_with_is_what_the_line_hears() {
    for (status, said) in [
        (486, "busy 1"),
        (600, "busy 1"),
        (480, "no answer 1"),
        (408, "no answer 1"),
        (404, "failed 1"),
        (403, "failed 1"),
        (488, "failed 1"),
        (503, "failed 1"),
        (603, "failed 1"),
    ] {
        let mut world = World::new();
        world.dial(1, "5551234");
        let (call, _) = offered(&mut world);
        world.gateway.ring(world.now, call, 180);
        world.gateway.reject(world.now, call, status);
        world.settle();
        assert_eq!(world.said(), [said], "{status}");
        assert!(world.media.is_empty());
        // The failure was acknowledged and the call is gone.
        assert!(!world.router.busy(), "{status}");
        assert_eq!(world.handle.status().unwrap().calls, 0);
    }
}

#[test]
fn a_far_end_that_says_nothing_at_all_is_no_answer() {
    let mut world = World::new();
    world.lose_from_router = Some(Box::new(|_| true));
    world.dial(1, "5551234");
    world.advance(31_990);
    assert!(world.said().is_empty());
    world.advance(10);
    assert_eq!(world.said(), ["no answer 1"]);
    assert!(!world.router.busy());
}

#[test]
fn a_line_that_gives_up_cancels_the_call() {
    // The caller's S7 ran out, or the host hung up, after the far end
    // began to ring.
    let mut world = World::new();
    world.dial(1, "5551234");
    let (call, _) = offered(&mut world);
    world.gateway.ring(world.now, call, 180);
    world.settle();
    world.hangup(1);
    assert_eq!(heard(&mut world), ["cancelled"]);
    // The line gave up itself: it is told nothing, now or when the 487
    // has come.
    assert!(world.said().is_empty());
    assert!(!world.router.busy());

    // Before the far end has said anything the CANCEL has to wait for it
    // (RFC 3261 section 9.1).
    let mut world = World::new();
    world.dial(1, "5551234");
    let (call, _) = offered(&mut world);
    world.hangup(1);
    assert!(heard(&mut world).is_empty(), "no CANCEL yet");
    world.gateway.ring(world.now, call, 180);
    world.settle();
    assert_eq!(heard(&mut world), ["cancelled"]);
    assert!(world.said().is_empty());
    assert!(!world.router.busy());
}

#[test]
fn a_far_end_that_answers_as_the_line_gives_up_is_acknowledged_and_hung_up_on() {
    let mut world = World::new();
    world.dial(1, "5551234");
    let (call, _) = offered(&mut world);
    // The 200 is on its way when the line hangs up.
    world
        .gateway
        .answer(world.now, call, Some(&answer_sdp(7000, Codec::Pcmu)));
    world.hangup(1);
    assert_eq!(heard(&mut world), ["acknowledged", "hung up"]);
    assert!(world.said().is_empty());
    assert!(world.media.is_empty());
    assert!(!world.router.busy());
}

#[test]
fn a_registered_line_calls_through_its_registrar_and_answers_its_challenge() {
    let mut world = World::new();
    world.pbx.events();
    world.pbx.challenge_invites = Some(Challenge {
        realm: "pbx".to_owned(),
        credentials: Credentials {
            username: "7003".to_owned(),
            password: "secret".to_owned(),
        },
        algorithm: Algorithm::Sha256,
        qop: true,
    });
    world.dial(3, "5551234");

    // The PBX challenged the first INVITE and took the second.
    let events = world.pbx.events();
    let [PeerEvent::Incoming { call, request }] = events.as_slice() else {
        panic!("the PBX was not called: {events:?}");
    };
    assert_eq!(request.uri, "sip:5551234@pbx.example.lan");
    assert_eq!(
        request.headers.from().unwrap().uri,
        "sip:7003@pbx.example.lan"
    );
    assert_eq!(
        request.headers.get("Contact"),
        Some("<sip:7003@10.10.20.5:5060;line=3>")
    );
    assert_eq!(request.headers.cseq().unwrap().sequence, 2);
    let given = &request.headers.proxy_authorization()[0];
    assert_eq!(given.get("username"), Some("7003"));
    assert_eq!(given.get("uri"), Some("sip:5551234@pbx.example.lan"));
    assert_eq!(given.get("algorithm"), Some("SHA-256"));
    // The password is not on the wire.
    assert!(!world.seen.lock().unwrap().wire.contains("secret"));

    let call = *call;
    world
        .pbx
        .answer(world.now, call, Some(&answer_sdp(7000, Codec::Pcmu)));
    world.settle();
    assert_eq!(world.said(), ["connected 3 originate"]);
}

#[test]
fn a_challenge_that_cannot_be_answered_fails_the_call() {
    // The password is wrong.
    let mut world = World::new();
    world.pbx.challenge_invites = Some(Challenge {
        realm: "pbx".to_owned(),
        credentials: Credentials {
            username: "7003".to_owned(),
            password: "another".to_owned(),
        },
        algorithm: Algorithm::Md5,
        qop: true,
    });
    world.dial(3, "5551234");
    assert_eq!(world.said(), ["failed 3"]);
    assert!(!world.router.busy());

    // A trunk line has no credentials to answer with.
    let mut world = World::new();
    world.gateway.challenge_invites = world.pbx.challenge_invites.take().or(Some(Challenge {
        realm: "gw".to_owned(),
        credentials: Credentials {
            username: "1".to_owned(),
            password: "x".to_owned(),
        },
        algorithm: Algorithm::Md5,
        qop: false,
    }));
    world.dial(1, "5551234");
    assert_eq!(world.said(), ["failed 1"]);
}

#[test]
fn a_lost_ack_is_sent_again_when_the_200_comes_again() {
    let mut world = World::new();
    world.dial(1, "5551234");
    let (call, _) = offered(&mut world);
    let mut lost = false;
    world.lose_from_router = Some(Box::new(move |message| {
        if !lost && is_method(message, Method::Ack) {
            lost = true;
            true
        } else {
            false
        }
    }));
    world
        .gateway
        .answer(world.now, call, Some(&answer_sdp(7000, Codec::Pcmu)));
    world.settle();
    // The line is through on the 200, whatever became of the ACK.
    assert_eq!(world.said(), ["connected 1 originate"]);
    assert!(heard(&mut world).is_empty());
    // The far end sends its 200 again and gets the ACK again.
    world.advance(500);
    assert_eq!(heard(&mut world), ["acknowledged"]);
    assert!(world.said().is_empty());
    assert_eq!(world.media.len(), 1);
}

#[test]
fn a_lost_invite_and_a_lost_200_are_sent_again() {
    let mut world = World::new();
    let mut lost = false;
    world.lose_from_router = Some(Box::new(move |message| {
        if !lost && is_method(message, Method::Invite) {
            lost = true;
            true
        } else {
            false
        }
    }));
    world.dial(1, "5551234");
    assert!(heard(&mut world).is_empty());
    world.advance(500);
    let (call, _) = offered(&mut world);

    let mut lost = false;
    world.lose_to_router = Some(Box::new(move |message| {
        let ok = matches!(message, Message::Response(r) if r.status == 200);
        if !lost && ok {
            lost = true;
            true
        } else {
            false
        }
    }));
    world
        .gateway
        .answer(world.now, call, Some(&answer_sdp(7000, Codec::Pcmu)));
    world.settle();
    assert!(world.said().is_empty());
    world.advance(500);
    assert_eq!(world.said(), ["connected 1 originate"]);
    assert_eq!(heard(&mut world), ["acknowledged"]);
}

#[test]
fn every_message_doubled_changes_nothing() {
    let mut world = World::new();
    world.duplicate = true;
    world.dial(1, "5551234");
    let events = world.gateway.events();
    assert_eq!(statuses(&events), ["incoming sip:5551234@10.10.20.11"]);
    let PeerEvent::Incoming { call, .. } = events[0] else {
        panic!("not a call");
    };
    world.gateway.ring(world.now, call, 180);
    world
        .gateway
        .answer(world.now, call, Some(&answer_sdp(7000, Codec::Pcmu)));
    world.settle();
    assert_eq!(world.said(), ["connected 1 originate"]);
    assert_eq!(world.media.len(), 1);
    world.hangup(1);
    assert_eq!(heard(&mut world), ["acknowledged", "hung up"]);
    world.advance(40_000);
    assert!(world.said().is_empty());
    assert!(!world.router.busy());
}

#[test]
fn an_answer_we_cannot_carry_is_acknowledged_and_the_call_ended() {
    for answer in [
        Some(description(&["m=audio 7000 RTP/AVP 18"])),
        Some(description(&["m=audio 0 RTP/AVP 0"])),
        None,
    ] {
        let mut world = World::new();
        world.dial(1, "5551234");
        let (call, _) = offered(&mut world);
        world.gateway.answer(world.now, call, answer.as_ref());
        world.settle();
        assert_eq!(world.said(), ["failed 1"]);
        assert_eq!(heard(&mut world), ["acknowledged", "hung up"]);
        assert!(world.media.is_empty());
        assert!(!world.router.busy());
    }
}

#[test]
fn the_number_dialled_is_the_user_of_the_request_uri() {
    for (dialled, user) in [
        ("5551234", "5551234"),
        ("9,5551234", "95551234"),
        ("9W5551234", "95551234"),
        ("*70,5551234#", "*705551234%23"),
        ("+15551234", "+15551234"),
    ] {
        let mut world = World::new();
        world.dial(1, dialled);
        let (_, invite) = offered(&mut world);
        assert_eq!(invite.uri, format!("sip:{user}@10.10.20.11"), "{dialled}");
    }
    // Nothing in it that can be dialled.
    let mut world = World::new();
    world.dial(1, ",W,");
    assert_eq!(world.said(), ["failed 1"]);
    assert!(heard(&mut world).is_empty());
}

#[test]
fn the_outbound_peer_may_be_a_name_and_a_port() {
    let config = common::CONFIG.replace(
        "outbound_peer = \"10.10.20.11\"",
        "outbound_peer = \"gw.example.lan:5060\"",
    );
    let mut world = World::with_config(&config);
    world.dial(1, "5551234");
    let (_, invite) = offered(&mut world);
    assert_eq!(invite.uri, "sip:5551234@gw.example.lan:5060");
    assert_eq!(invite.headers.from().unwrap().uri, "sip:1@gw.example.lan");

    // A name that does not resolve: the call fails, and the log says why.
    let config = common::CONFIG.replace(
        "outbound_peer = \"10.10.20.11\"",
        "outbound_peer = \"nowhere.example\"",
    );
    let mut world = World::with_config(&config);
    world.dial(1, "5551234");
    assert_eq!(world.said(), ["failed 1"]);
    assert_eq!(world.logged(), ["RMD5014"]);
}

#[test]
fn a_line_that_dials_while_it_is_in_a_call_leaves_that_call() {
    let mut world = World::new();
    world.dial(1, "5551234");
    let (first, _) = offered(&mut world);
    world
        .gateway
        .answer(world.now, first, Some(&answer_sdp(7000, Codec::Pcmu)));
    world.settle();
    world.said();
    world.gateway.events();

    world.dial(1, "5559876");
    let events = world.gateway.events();
    assert_eq!(
        statuses(&events),
        ["hung up", "incoming sip:5559876@10.10.20.11"]
    );
    assert!(world.said().is_empty());
}

#[test]
fn a_200_that_makes_no_dialog_fails_the_call() {
    let mut world = World::new();
    world.dial(1, "5551234");
    world.gateway.events();
    // A 200 with no Contact: there is nowhere to send the ACK or a BYE.
    let wire = world.seen.lock().unwrap().wire.clone();
    let invite = &wire[wire.rfind("INVITE sip:5551234").unwrap()..];
    let Ok(Message::Request(invite)) = Message::parse(invite.as_bytes()) else {
        panic!("the INVITE was traced");
    };
    let mut ok = invite.response(200, Some("far"));
    ok.headers.push("Content-Type", "application/sdp");
    ok.body = answer_sdp(7000, Codec::Pcmu).build().into_bytes();
    world.inject(common::GATEWAY, &ok.to_bytes());
    world.settle();
    assert_eq!(world.said(), ["failed 1"]);
    assert!(world.media.is_empty());
}
