// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Lines registered to a PBX as extensions: registering, the challenge,
//! refreshing, failing and recovering, calls for a registered line, and
//! what happens when the configuration changes and when the service stops.

mod common;

use common::{PBX, ROUTER, World, offer, statuses};
use pool::CallRouter;
use sip::digest::Algorithm;
use sip::{Credentials, Message, Method};
use sippeer::{Challenge, PeerEvent, RegistrarPolicy};
use telephony::RegistrationState;

fn challenge(password: &str) -> RegistrarPolicy {
    RegistrarPolicy::Challenge {
        challenge: Challenge {
            realm: "pbx".to_owned(),
            credentials: Credentials {
                username: "7003".to_owned(),
                password: password.to_owned(),
            },
            algorithm: Algorithm::Md5,
            qop: true,
        },
        expires: Some(120),
    }
}

/// The REGISTER requests the PBX has had, and how it answered each.
fn registers(world: &mut World) -> Vec<(sip::Request, u16)> {
    world
        .pbx
        .events()
        .into_iter()
        .filter_map(|event| match event {
            PeerEvent::Registered { request, status } => Some((request, status)),
            _ => None,
        })
        .collect()
}

fn state(world: &World) -> RegistrationState {
    world.handle.status().unwrap().registrations[0].state
}

/// The id of the message the registration failed with.
fn error(world: &World) -> Option<String> {
    world.handle.status().unwrap().registrations[0]
        .error
        .as_ref()
        .map(|message| message.id.clone())
}

#[test]
fn a_line_with_register_registers_when_the_router_starts() {
    let mut world = World::new();
    let registers = registers(&mut world);
    assert_eq!(registers.len(), 1);
    let (request, status) = &registers[0];
    assert_eq!(*status, 200);
    assert_eq!(request.uri, "sip:pbx.example.lan");
    assert_eq!(
        request.headers.to().unwrap().uri,
        "sip:7003@pbx.example.lan"
    );
    assert_eq!(
        request.headers.from().unwrap().uri,
        "sip:7003@pbx.example.lan"
    );
    // The contact says which line it is.
    assert_eq!(
        request.headers.get("Contact"),
        Some("<sip:7003@10.10.20.5:5060;line=3>")
    );
    assert_eq!(request.headers.expires(), Some(300));
    assert_eq!(request.headers.get("User-Agent"), Some("rustmodem/0.0.3"));
    assert_eq!(world.pbx.bindings.len(), 1);
    assert_eq!(world.pbx.bindings[0].user, "7003");

    // The control API sees it, and so does whoever watches.
    let status = world.handle.status().unwrap();
    assert_eq!(status.listen.to_string(), "127.0.0.1:5060");
    assert_eq!(status.trusted_peers, ["10.10.20.11"]);
    assert_eq!(status.outbound_peer, "10.10.20.11");
    assert_eq!(
        status.registrations.len(),
        1,
        "only the line that registers"
    );
    let registration = &status.registrations[0];
    assert_eq!(
        (
            registration.line,
            registration.server.as_str(),
            registration.user.as_str()
        ),
        (3, "pbx.example.lan", "7003")
    );
    assert_eq!(registration.state, RegistrationState::Registered);
    assert_eq!(registration.expires_in_s, Some(300));
    assert_eq!(registration.error, None);
    assert_eq!(world.startup_logs, ["RMD5003"]);
    let seen = world.seen.lock().unwrap();
    let states: Vec<_> = seen.registrations.iter().map(|r| r.state).collect();
    assert_eq!(
        states,
        [
            RegistrationState::Registering,
            RegistrationState::Registered
        ]
    );
}

#[test]
fn a_challenge_is_answered_and_the_password_is_never_on_the_wire() {
    let mut world = World::unsettled(common::CONFIG);
    world.pbx.registrar = challenge("secret");
    world.settle();
    let registers = registers(&mut world);
    let statuses: Vec<u16> = registers.iter().map(|(_, status)| *status).collect();
    assert_eq!(statuses, [401, 200]);
    let (second, _) = &registers[1];
    assert_eq!(second.headers.cseq().unwrap().sequence, 2);
    assert_eq!(
        second.headers.get("Call-ID"),
        registers[0].0.headers.get("Call-ID")
    );
    let given = &second.headers.authorization()[0];
    assert_eq!(given.get("username"), Some("7003"));
    assert_eq!(given.get("uri"), Some("sip:pbx.example.lan"));
    assert_eq!(state(&world), RegistrationState::Registered);
    // The registrar granted less than was asked for.
    assert_eq!(
        world.handle.status().unwrap().registrations[0].expires_in_s,
        Some(120)
    );
    // What is traced is what was on the wire: the response, never the
    // password.
    let wire = world.seen.lock().unwrap().wire.clone();
    assert!(wire.contains("Authorization: Digest username=\"7003\""));
    assert!(!wire.contains("secret"));
}

#[test]
fn the_registration_is_refreshed_at_90_percent_of_the_time_granted() {
    let mut world = World::new();
    world.pbx.events();
    world.advance(269_990);
    assert!(registers(&mut world).is_empty());
    world.advance(10);
    let refreshed = registers(&mut world);
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].0.headers.cseq().unwrap().sequence, 2);
    assert_eq!(state(&world), RegistrationState::Registered);
    assert_eq!(
        world.handle.status().unwrap().registrations[0].expires_in_s,
        Some(300)
    );
    // Refreshing is not news.
    assert!(world.logged().is_empty());
}

#[test]
fn a_refused_registration_is_tried_again_and_recovers() {
    let mut world = World::unsettled(common::CONFIG);
    world.pbx.registrar = RegistrarPolicy::Reject(403);
    world.settle();
    assert_eq!(state(&world), RegistrationState::Failed);
    assert_eq!(error(&world).as_deref(), Some("RMD5007"));
    assert_eq!(world.logged(), ["RMD5007"]);
    assert_eq!(registers(&mut world).len(), 1);

    // After 5, 10 and 20 seconds, and so on.
    world.advance(4990);
    assert!(registers(&mut world).is_empty());
    world.advance(10);
    assert_eq!(registers(&mut world).len(), 1);
    world.advance(10_000);
    assert_eq!(registers(&mut world).len(), 1);
    // The same failure again is not said again.
    assert!(world.logged().is_empty());

    // The registrar comes round.
    world.pbx.registrar = RegistrarPolicy::Accept { expires: None };
    world.advance(20_000);
    assert_eq!(state(&world), RegistrationState::Registered);
    assert_eq!(error(&world), None);
    assert_eq!(world.logged(), ["RMD5003"]);
}

#[test]
fn a_registrar_that_does_not_answer_is_tried_again() {
    let mut world = World::unsettled(common::CONFIG);
    world.pbx.registrar = RegistrarPolicy::Silent;
    world.settle();
    assert_eq!(state(&world), RegistrationState::Registering);
    world.advance(31_990);
    assert_eq!(state(&world), RegistrationState::Registering);
    world.advance(10);
    assert_eq!(state(&world), RegistrationState::Failed);
    assert_eq!(error(&world).as_deref(), Some("RMD5004"));
    assert_eq!(world.logged(), ["RMD5004"]);

    world.pbx.events();
    world.pbx.registrar = RegistrarPolicy::Accept { expires: None };
    world.advance(5000);
    assert_eq!(state(&world), RegistrationState::Registered);
}

#[test]
fn a_wrong_password_and_a_challenge_we_cannot_answer() {
    let mut world = World::unsettled(common::CONFIG);
    world.pbx.registrar = challenge("something else");
    world.settle();
    assert_eq!(state(&world), RegistrationState::Failed);
    assert_eq!(error(&world).as_deref(), Some("RMD5005"));

    // A line with no password is asked for one.
    let config = common::CONFIG.replace(", password = \"secret\"", "");
    let mut world = World::unsettled(&config);
    world.pbx.registrar = challenge("secret");
    world.settle();
    assert_eq!(error(&world).as_deref(), Some("RMD5005"));
}

#[test]
fn the_registrars_minimum_is_taken() {
    let mut world = World::unsettled(common::CONFIG);
    world.pbx.registrar = RegistrarPolicy::IntervalTooBrief { minimum: 1800 };
    world.settle();
    let registers = registers(&mut world);
    let seen: Vec<(Option<u32>, u16)> = registers
        .iter()
        .map(|(request, status)| (request.headers.expires(), *status))
        .collect();
    assert_eq!(seen, [(Some(300), 423), (Some(1800), 200)]);
    assert_eq!(state(&world), RegistrationState::Registered);
    assert_eq!(
        world.handle.status().unwrap().registrations[0].expires_in_s,
        Some(1800)
    );
}

#[test]
fn a_registrar_whose_name_does_not_resolve() {
    let config = common::CONFIG.replace("pbx.example.lan", "nowhere.example");
    let mut world = World::with_config(&config);
    assert_eq!(state(&world), RegistrationState::Failed);
    assert_eq!(error(&world).as_deref(), Some("RMD5008"));
    assert_eq!(world.startup_logs, ["RMD5008"]);
    assert!(registers(&mut world).is_empty());
    // It is looked up again every minute, and not said again.
    world.advance(120_000);
    assert_eq!(state(&world), RegistrationState::Failed);
    assert!(world.logged().is_empty());
    // A line that cannot find its registrar cannot dial through it.
    world.dial(3, "5551234");
    assert_eq!(world.said(), ["failed 3"]);
}

#[test]
fn a_call_for_a_registered_line_goes_to_that_line_only() {
    let mut world = World::new();
    world.pbx.events();
    // The PBX calls the contact the line registered.
    world.pbx.invite(
        world.now,
        ROUTER,
        "sip:7003@10.10.20.5:5060;line=3",
        Some(&offer(8000)),
    );
    world.settle();
    assert_eq!(world.said(), ["incoming 3 from pbx", "ring 3"]);
    let events = world.pbx.events();
    let PeerEvent::Progress { response, .. } = &events[1] else {
        panic!("not ringing: {events:?}");
    };
    assert_eq!(
        response.headers.get("Contact"),
        Some("<sip:7003@10.10.20.5:5060;line=3>")
    );
    world.answer(3);
    assert_eq!(world.said(), ["ring end 3", "connected 3 answer"]);

    // While the line is in that call, another for it is busy: it does not
    // go hunting.
    world.lines.insert(3, ctlproto::LineState::Online);
    world.pbx.invite(
        world.now,
        ROUTER,
        "sip:7003@10.10.20.5:5060;line=3",
        Some(&offer(8002)),
    );
    world.settle();
    assert_eq!(statuses(&world.pbx.events()).last().unwrap(), "refused 486");
    assert!(world.said().is_empty());
}

#[test]
fn the_registrar_is_trusted_for_its_line_and_for_nothing_else() {
    let mut world = World::new();
    world.pbx.events();
    // The pilot number of the hunt group, from the PBX, which is not a
    // trusted peer: dropped.
    world.pbx.invite(
        world.now,
        ROUTER,
        "sip:5551000@10.10.20.5",
        Some(&offer(8000)),
    );
    world.settle();
    world.advance(1000);
    assert!(world.said().is_empty());
    assert!(
        statuses(&world.pbx.events())
            .iter()
            .all(|e| !e.starts_with("progress"))
    );
    // The INVITE, and the once it was sent again in that second.
    assert_eq!(world.handle.status().unwrap().dropped_untrusted, 2);

    // The line's contact with another line's number, or another user's
    // name: not the line's, so not the registrar's to send.
    for uri in [
        "sip:7003@10.10.20.5:5060;line=1",
        "sip:7009@10.10.20.5:5060;line=3",
        "sip:7003@10.10.20.5:5060",
    ] {
        let mut world = World::new();
        world.pbx.invite(world.now, ROUTER, uri, Some(&offer(8000)));
        world.settle();
        assert!(world.said().is_empty(), "{uri}");
    }

    // A trusted peer may call the line's contact too.
    let mut world = World::new();
    world.gateway.invite(
        world.now,
        ROUTER,
        "sip:7003@10.10.20.5:5060;line=3",
        Some(&offer(8000)),
    );
    world.settle();
    assert_eq!(world.said(), ["incoming 3 from 201", "ring 3"]);
}

#[test]
fn a_line_registers_again_when_it_is_asked_to() {
    let mut world = World::new();
    world.pbx.events();
    world.handle.reregister(Some(3));
    world.settle();
    assert_eq!(registers(&mut world).len(), 1);
    // A line that does not register has nothing to do.
    world.handle.reregister(Some(1));
    world.settle();
    assert!(registers(&mut world).is_empty());
    // Every line.
    world.handle.reregister(None);
    world.settle();
    assert_eq!(registers(&mut world).len(), 1);

    // It cuts short the wait after a failure.
    let mut world = World::unsettled(common::CONFIG);
    world.pbx.registrar = RegistrarPolicy::Reject(503);
    world.settle();
    world.pbx.events();
    world.pbx.registrar = RegistrarPolicy::Accept { expires: None };
    world.handle.reregister(None);
    world.settle();
    assert_eq!(state(&world), RegistrationState::Registered);
}

#[test]
fn a_change_of_the_configuration_changes_the_registrations() {
    let mut world = World::new();
    world.pbx.events();

    // The same configuration again changes nothing.
    world.configure(common::CONFIG);
    assert!(registers(&mut world).is_empty());

    // The line's register settings are removed: the binding is too.
    let without = common::CONFIG.replace(
        "register = { server = \"pbx.example.lan\", user = \"7003\", password = \"secret\" }",
        "",
    );
    world.configure(&without);
    let removal = registers(&mut world);
    assert_eq!(removal.len(), 1);
    assert_eq!(removal[0].0.headers.expires(), Some(0));
    assert!(world.pbx.bindings.is_empty());
    assert!(world.handle.status().unwrap().registrations.is_empty());

    // They are put back, with another extension: registered afresh.
    let other = common::CONFIG.replace("7003", "7004");
    world.configure(&other);
    let registered = registers(&mut world);
    assert_eq!(registered.len(), 1);
    assert_eq!(
        registered[0].0.headers.get("Contact"),
        Some("<sip:7004@10.10.20.5:5060;line=3>")
    );
    assert_eq!(world.pbx.bindings[0].user, "7004");

    // A line taken out of service gives up its registration.
    let disabled = other.replace("port = \"CNCB2\" }", "port = \"CNCB2\" }\nenabled = false");
    world.configure(&disabled);
    assert_eq!(registers(&mut world)[0].0.headers.expires(), Some(0));
    assert!(world.handle.status().unwrap().registrations.is_empty());

    // Routes and hunt groups are taken as they are now.
    let rerouted = common::CONFIG.replace("called = \"5552*\"", "called = \"777*\"");
    world.configure(&rerouted);
    world.call("7771234");
    assert_eq!(world.said(), ["incoming 2 from 201", "ring 2"]);
}

#[test]
fn stopping_ends_every_call_and_removes_every_registration() {
    let mut world = World::new();
    // One call connected, one ringing, one being placed.
    let connected = world.call("5551000");
    world.answer(1);
    world.call("5552000");
    world.dial(3, "5551234");
    world.said();
    world.gateway.events();
    let pbx = world.pbx.events();
    let Some(PeerEvent::Incoming { call: placed, .. }) = pbx.last() else {
        panic!("the PBX was not called: {pbx:?}");
    };
    world.pbx.ring(world.now, *placed, 180);
    world.settle();

    world.router.shutdown(world.now);
    assert!(
        world.router.busy(),
        "there is something on the network to wait for"
    );
    world.settle();

    // The connected call got a BYE, the ringing one a 603, the one being
    // placed a CANCEL, and the registration was removed.
    assert_eq!(
        statuses(&world.gateway.events()),
        ["hung up", "refused 603"]
    );
    let pbx = world.pbx.events();
    assert!(statuses(&pbx).contains(&"cancelled".to_owned()), "{pbx:?}");
    let removal = pbx.iter().find_map(|event| match event {
        PeerEvent::Registered { request, status } => Some((request.headers.expires(), *status)),
        _ => None,
    });
    assert_eq!(removal, Some((Some(0), 200)));
    assert!(world.pbx.bindings.is_empty());
    assert!(!world.router.busy(), "and then nothing");
    assert_eq!(state(&world), RegistrationState::Unregistered);
    assert!(world.logged().contains(&"RMD5009".to_owned()));
    // The lines were taken down by the pool: the router says nothing to
    // them.
    assert!(world.said().is_empty());
    let _ = connected;
}

#[test]
fn a_registrar_that_does_not_answer_the_removal_does_not_hold_the_service() {
    let mut world = World::new();
    world.pbx.registrar = RegistrarPolicy::Silent;
    world.router.shutdown(world.now);
    world.settle();
    assert!(world.router.busy());
    world.advance(32_000);
    assert!(!world.router.busy());
}

#[test]
fn every_message_is_traced_with_its_direction_and_the_other_end() {
    let mut world = World::new();
    let trace = world.seen.lock().unwrap().trace.clone();
    assert_eq!(
        trace,
        [
            (
                telephony::Direction::Sent,
                PBX,
                "REGISTER sip:pbx.example.lan SIP/2.0".to_owned()
            ),
            (
                telephony::Direction::Received,
                PBX,
                "SIP/2.0 200 OK".to_owned()
            ),
        ]
    );
    world.call("5559999");
    let trace = world.seen.lock().unwrap().trace.clone();
    let lines: Vec<&str> = trace[2..]
        .iter()
        .map(|(_, _, line)| line.as_str())
        .collect();
    assert_eq!(
        lines,
        [
            "INVITE sip:5559999@10.10.20.5 SIP/2.0",
            "SIP/2.0 404 Not Found",
            "ACK sip:5559999@10.10.20.5 SIP/2.0",
        ]
    );
    let is_register =
        |m: &Message| matches!(m, Message::Request(r) if r.method == Method::Register);
    let _ = is_register;
}
