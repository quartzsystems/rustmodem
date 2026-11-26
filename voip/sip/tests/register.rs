// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The registration client through its life: registering, the challenge,
//! refreshing, failing, recovering and removing the binding.

use sip::digest::{self, Algorithm};
use sip::register::BACKOFF_S;
use sip::{
    Auth, Credentials, RegFailure, RegState, Registration, RegistrationConfig, Request, Response,
    SipUri,
};

fn credentials() -> Credentials {
    Credentials {
        username: "7002".to_owned(),
        password: "secret".to_owned(),
    }
}

fn config(credentials: Option<Credentials>) -> RegistrationConfig {
    let mut contact = SipUri::new(Some("7002"), "10.10.20.5", Some(5060));
    contact.params.set("line", Some("2"));
    RegistrationConfig {
        registrar: SipUri::host("pbx.example.lan"),
        aor: SipUri::new(Some("7002"), "pbx.example.lan", None),
        contact,
        expires_s: 300,
        credentials,
        user_agent: "rustmodem/0.0.3".to_owned(),
        call_id: "reg-2@10.10.20.5".to_owned(),
        from_tag: "tag2".to_owned(),
    }
}

fn registration() -> Registration {
    Registration::new(config(Some(credentials())))
}

/// The registrar's 200, with this in its Contact after our address.
fn ok(request: &Request, contact_params: &str) -> Response {
    let mut response = request.response(200, Some("registrar"));
    response.headers.push(
        "Contact",
        format!("<sip:7002@10.10.20.5:5060;line=2>{contact_params}"),
    );
    response
}

fn challenge(request: &Request, nonce: &str, stale: bool) -> Response {
    let mut response = request.response(401, Some("registrar"));
    response.headers.push(
        "WWW-Authenticate",
        digest::challenge("pbx", nonce, Algorithm::Md5, true, stale),
    );
    response
}

fn authorization(request: &Request) -> Option<Auth> {
    request.headers.authorization().into_iter().next()
}

/// Registers with no challenge, at `now`, for 300 seconds.
fn registered(now: u64) -> Registration {
    let mut r = Registration::new(config(None));
    let request = r.poll(now, "c").expect("the first REGISTER");
    assert_eq!(r.on_response(now, &ok(&request, ";expires=300"), "c"), None);
    r
}

#[test]
fn the_first_register() {
    let mut r = registration();
    assert_eq!(r.state(), RegState::Unregistered);
    assert_eq!(r.next_deadline(), Some(0));

    let request = r.poll(1000, "c").unwrap();
    assert_eq!(
        String::from_utf8(request.to_bytes()).unwrap(),
        "REGISTER sip:pbx.example.lan SIP/2.0\r\n\
Max-Forwards: 70\r\n\
From: <sip:7002@pbx.example.lan>;tag=tag2\r\n\
To: <sip:7002@pbx.example.lan>\r\n\
Call-ID: reg-2@10.10.20.5\r\n\
CSeq: 1 REGISTER\r\n\
Contact: <sip:7002@10.10.20.5:5060;line=2>\r\n\
Expires: 300\r\n\
User-Agent: rustmodem/0.0.3\r\n\
Content-Length: 0\r\n\
\r\n"
    );
    assert_eq!(r.state(), RegState::Registering);
    // One REGISTER at a time.
    assert_eq!(r.poll(2000, "c"), None);
    assert_eq!(r.next_deadline(), None);
    assert_eq!(r.expires_in_s(2000), None);
    assert_eq!(r.config().expires_s, 300);
}

#[test]
fn registered_and_refreshed_at_90_percent() {
    let mut r = registration();
    let request = r.poll(1000, "c").unwrap();
    assert_eq!(
        r.on_response(1200, &ok(&request, ";expires=120"), "c"),
        None
    );
    assert_eq!(r.state(), RegState::Registered { until_ms: 121_200 });
    assert_eq!(r.expires_in_s(1200), Some(120));
    assert_eq!(r.expires_in_s(61_200), Some(60));

    // 90 percent of 120 seconds.
    assert_eq!(r.next_deadline(), Some(1200 + 108_000));
    assert_eq!(r.poll(1200 + 107_999, "c"), None);
    let refresh = r.poll(1200 + 108_000, "c").unwrap();
    assert_eq!(refresh.headers.get("CSeq"), Some("2 REGISTER"));
    assert_eq!(refresh.headers.get("Call-ID"), Some("reg-2@10.10.20.5"));
    assert_eq!(refresh.headers.expires(), Some(300));
    // The binding still stands while the refresh is out.
    assert_eq!(r.state(), RegState::Registered { until_ms: 121_200 });

    r.on_response(110_000, &ok(&refresh, ";expires=120"), "c");
    assert_eq!(r.state(), RegState::Registered { until_ms: 230_000 });
}

#[test]
fn the_time_granted() {
    // (what the 200 says, the seconds granted)
    let grant = |build: &dyn Fn(&Request) -> Response| {
        let mut r = registration();
        let request = r.poll(0, "c").unwrap();
        r.on_response(0, &build(&request), "c");
        r.expires_in_s(0)
    };
    // The expires of our own contact wins over the header.
    assert_eq!(
        grant(&|request| {
            let mut response = ok(request, ";expires=90");
            response.headers.push("Expires", "600");
            response
        }),
        Some(90)
    );
    // Ours among the bindings of others.
    assert_eq!(
        grant(&|request| {
            let mut response = request.response(200, None);
            response.headers.push(
                "Contact",
                "<sip:7002@10.10.20.77>;expires=3600, <sip:7002@10.10.20.5:5060;line=2>;expires=45",
            );
            response
                .headers
                .push("Contact", "<sip:7003@10.10.20.5:5060>;expires=7");
            response
        }),
        Some(45)
    );
    // No expires on the contact: the Expires header.
    assert_eq!(
        grant(&|request| {
            let mut response = ok(request, "");
            response.headers.push("Expires", "600");
            response
        }),
        Some(600)
    );
    // Neither: what we asked for.
    assert_eq!(grant(&|request| ok(request, "")), Some(300));
    assert_eq!(grant(&|request| request.response(200, None)), Some(300));
    // A registrar that grants nothing has not registered us.
    let mut r = registration();
    let request = r.poll(0, "c").unwrap();
    r.on_response(0, &ok(&request, ";expires=0"), "c");
    assert_eq!(
        r.state(),
        RegState::Failed {
            reason: RegFailure::Status(200),
            retry_at_ms: 5000
        }
    );
}

#[test]
fn a_provisional_response_changes_nothing() {
    let mut r = registration();
    let request = r.poll(0, "c").unwrap();
    assert_eq!(r.on_response(10, &request.response(100, None), "c"), None);
    assert_eq!(r.state(), RegState::Registering);
    assert_eq!(r.poll(20, "c"), None);
}

#[test]
fn a_challenge_is_answered_and_the_nonce_is_used_for_the_refresh() {
    let mut r = registration();
    let first = r.poll(0, "c1").unwrap();
    assert_eq!(authorization(&first), None);

    let second = r
        .on_response(100, &challenge(&first, "nonce-1", false), "c2")
        .expect("the REGISTER again, with credentials");
    assert_eq!(second.headers.get("CSeq"), Some("2 REGISTER"));
    assert_eq!(second.headers.get("Call-ID"), first.headers.get("Call-ID"));
    let given = authorization(&second).unwrap();
    assert!(digest::verify(
        &credentials(),
        &given,
        "nonce-1",
        "REGISTER"
    ));
    assert_eq!(given.get("uri"), Some("sip:pbx.example.lan"));
    assert_eq!(given.get("nc"), Some("00000001"));
    assert_eq!(given.get("cnonce"), Some("c2"));
    assert_eq!(r.state(), RegState::Registering);

    r.on_response(200, &ok(&second, ";expires=100"), "c3");
    assert_eq!(r.state(), RegState::Registered { until_ms: 100_200 });

    // The refresh carries credentials without being asked.
    let refresh = r.poll(90_200, "c4").unwrap();
    let given = authorization(&refresh).unwrap();
    assert!(digest::verify(
        &credentials(),
        &given,
        "nonce-1",
        "REGISTER"
    ));
    assert_eq!(given.get("nc"), Some("00000002"));

    // The nonce has run out and the registrar says so: answered again.
    let again = r
        .on_response(90_300, &challenge(&refresh, "nonce-2", true), "c5")
        .unwrap();
    let given = authorization(&again).unwrap();
    assert!(digest::verify(
        &credentials(),
        &given,
        "nonce-2",
        "REGISTER"
    ));
    r.on_response(90_400, &ok(&again, ";expires=100"), "c6");
    assert_eq!(r.state(), RegState::Registered { until_ms: 190_400 });
}

#[test]
fn a_second_challenge_is_a_failure() {
    let mut r = registration();
    let first = r.poll(0, "c").unwrap();
    let second = r
        .on_response(100, &challenge(&first, "n1", false), "c")
        .unwrap();
    assert_eq!(
        r.on_response(200, &challenge(&second, "n2", false), "c"),
        None
    );
    assert_eq!(
        r.state(),
        RegState::Failed {
            reason: RegFailure::Credentials,
            retry_at_ms: 5200
        }
    );
    // The next try starts from nothing: no credentials until it is asked.
    let retry = r.poll(5200, "c").unwrap();
    assert_eq!(authorization(&retry), None);
    assert_eq!(r.state(), RegState::Registering);
}

#[test]
fn challenges_that_cannot_be_answered() {
    // No credentials are configured.
    let mut r = Registration::new(config(None));
    let request = r.poll(0, "c").unwrap();
    assert_eq!(
        r.on_response(0, &challenge(&request, "n", false), "c"),
        None
    );
    assert!(matches!(
        r.state(),
        RegState::Failed {
            reason: RegFailure::Credentials,
            ..
        }
    ));

    // A scheme we do not have.
    let mut r = registration();
    let request = r.poll(0, "c").unwrap();
    let mut basic = request.response(401, None);
    basic
        .headers
        .push("WWW-Authenticate", "Basic realm=\"pbx\"");
    assert_eq!(r.on_response(0, &basic, "c"), None);
    assert!(matches!(
        r.state(),
        RegState::Failed {
            reason: RegFailure::Challenge,
            ..
        }
    ));
    assert_eq!(
        RegFailure::Challenge.to_string(),
        "the registrar asked for authentication that is not supported"
    );
}

#[test]
fn interval_too_brief_is_answered_once_with_the_registrars_minimum() {
    let mut r = registration();
    let first = r.poll(0, "c").unwrap();
    let mut brief = first.response(423, None);
    brief.headers.push("Min-Expires", "3600");
    let second = r.on_response(100, &brief, "c").unwrap();
    assert_eq!(second.headers.expires(), Some(3600));
    assert_eq!(second.headers.get("CSeq"), Some("2 REGISTER"));

    // Again in the same try: a failure.
    let mut brief = second.response(423, None);
    brief.headers.push("Min-Expires", "7200");
    assert_eq!(r.on_response(200, &brief, "c"), None);
    assert_eq!(
        r.state(),
        RegState::Failed {
            reason: RegFailure::Status(423),
            retry_at_ms: 5200
        }
    );
    // The next try asks for the minimum from the start, and may be told
    // a new one.
    let third = r.poll(5200, "c").unwrap();
    assert_eq!(third.headers.expires(), Some(3600));
    let fourth = r.on_response(5300, &brief, "c").unwrap();
    assert_eq!(fourth.headers.expires(), Some(7200));
    r.on_response(5400, &ok(&fourth, ";expires=7200"), "c");
    assert_eq!(r.expires_in_s(5400), Some(7200));

    // A 423 with no minimum, or one we already meet, is a failure.
    for min in [None, Some("300"), Some("60")] {
        let mut r = registration();
        let request = r.poll(0, "c").unwrap();
        let mut brief = request.response(423, None);
        if let Some(min) = min {
            brief.headers.push("Min-Expires", min);
        }
        assert_eq!(r.on_response(0, &brief, "c"), None, "{min:?}");
        assert!(matches!(r.state(), RegState::Failed { .. }));
    }
}

#[test]
fn failures_back_off_and_a_success_starts_the_count_again() {
    let mut r = registration();
    let mut now = 0;
    let mut waits = Vec::new();
    for _ in 0..7 {
        r.poll(now, "c").expect("a try is due");
        now += 32_000;
        r.on_timeout(now);
        let RegState::Failed {
            reason,
            retry_at_ms,
        } = r.state()
        else {
            panic!("not failed");
        };
        assert_eq!(reason, RegFailure::Timeout);
        assert_eq!(r.next_deadline(), Some(retry_at_ms));
        // Not a moment before.
        assert_eq!(r.poll(retry_at_ms - 1, "c"), None);
        waits.push((retry_at_ms - now) / 1000);
        now = retry_at_ms;
    }
    assert_eq!(waits, [5, 10, 20, 40, 60, 60, 60]);
    assert_eq!(BACKOFF_S, [5, 10, 20, 40, 60]);

    // Recovery.
    let request = r.poll(now, "c").unwrap();
    r.on_response(now, &ok(&request, ";expires=300"), "c");
    assert!(matches!(r.state(), RegState::Registered { .. }));

    // The next failure waits five seconds again.
    let refresh = r.poll(now + 270_000, "c").unwrap();
    r.on_response(now + 270_100, &refresh.response(503, None), "c");
    assert_eq!(
        r.state(),
        RegState::Failed {
            reason: RegFailure::Status(503),
            retry_at_ms: now + 275_100
        }
    );
}

#[test]
fn a_refusal_is_a_failure_with_its_status() {
    for status in [403, 404, 500, 603] {
        let mut r = registration();
        let request = r.poll(0, "c").unwrap();
        assert_eq!(r.on_response(0, &request.response(status, None), "c"), None);
        assert_eq!(
            r.state(),
            RegState::Failed {
                reason: RegFailure::Status(status),
                retry_at_ms: 5000
            }
        );
    }
    assert_eq!(
        RegFailure::Status(403).to_string(),
        "the registrar answered 403"
    );
}

#[test]
fn registering_again_on_demand() {
    let mut r = registered(0);
    // Not due for a long time.
    assert_eq!(r.poll(1000, "c"), None);
    r.reregister(1000);
    let request = r.poll(1000, "c").unwrap();
    assert_eq!(request.headers.get("CSeq"), Some("2 REGISTER"));
    // While one is out, asking again does nothing.
    r.reregister(1001);
    assert_eq!(r.poll(1001, "c"), None);
    r.on_response(1100, &ok(&request, ";expires=300"), "c");

    // It cuts a back-off short and starts the count again.
    let mut r = registration();
    for _ in 0..3 {
        let at = r.next_deadline().unwrap();
        r.poll(at, "c").unwrap();
        r.on_timeout(at + 32_000);
    }
    let waiting_until = r.next_deadline().unwrap();
    r.reregister(waiting_until - 15_000);
    r.poll(waiting_until - 15_000, "c").unwrap();
    r.on_timeout(waiting_until);
    assert_eq!(r.next_deadline(), Some(waiting_until + 5000));
}

#[test]
fn a_binding_that_runs_out_while_failing_is_gone() {
    let mut r = registered(0);
    let refresh = r.poll(270_000, "c").unwrap();
    r.on_response(270_100, &refresh.response(503, None), "c");
    assert!(matches!(r.state(), RegState::Failed { .. }));
    assert_eq!(r.expires_in_s(270_100), None);
    let retry = r.poll(275_100, "c").unwrap();
    assert_eq!(r.state(), RegState::Registering);
    r.on_response(275_200, &ok(&retry, ";expires=300"), "c");
    assert_eq!(r.state(), RegState::Registered { until_ms: 575_200 });
}

#[test]
fn the_binding_is_removed_on_shutdown() {
    let mut r = registered(0);
    let removal = r
        .unregister("c")
        .expect("a REGISTER that removes the binding");
    assert_eq!(removal.headers.expires(), Some(0));
    assert_eq!(removal.headers.get("CSeq"), Some("2 REGISTER"));
    assert_eq!(
        removal.headers.get("Contact"),
        Some("<sip:7002@10.10.20.5:5060;line=2>")
    );
    // Nothing else is sent meanwhile or afterwards.
    assert_eq!(r.poll(1_000_000, "c"), None);
    assert_eq!(r.on_response(100, &removal.response(200, None), "c"), None);
    assert_eq!(r.state(), RegState::Unregistered);
    assert_eq!(r.next_deadline(), None);
    assert_eq!(r.poll(2_000_000, "c"), None);

    // Until it is asked to register again.
    r.reregister(3_000_000);
    let request = r.poll(3_000_000, "c").unwrap();
    assert_eq!(request.headers.expires(), Some(300));
    assert_eq!(r.state(), RegState::Registering);
}

#[test]
fn removing_a_binding_that_needs_credentials() {
    let mut r = registration();
    let first = r.poll(0, "c").unwrap();
    let second = r
        .on_response(0, &challenge(&first, "n1", false), "c")
        .unwrap();
    r.on_response(0, &ok(&second, ";expires=300"), "c");

    let removal = r.unregister("c").unwrap();
    assert_eq!(removal.headers.expires(), Some(0));
    assert!(authorization(&removal).is_some());
    // The nonce went stale meanwhile.
    let again = r
        .on_response(100, &challenge(&removal, "n2", true), "c")
        .expect("the removal again");
    assert_eq!(again.headers.expires(), Some(0));
    assert!(digest::verify(
        &credentials(),
        &authorization(&again).unwrap(),
        "n2",
        "REGISTER"
    ));
    r.on_response(200, &again.response(200, None), "c");
    assert_eq!(r.state(), RegState::Unregistered);
}

#[test]
fn a_removal_that_fails_is_still_the_end() {
    for outcome in ["timeout", "refused", "rejected"] {
        let mut r = registered(0);
        let removal = r.unregister("c").unwrap();
        match outcome {
            "timeout" => r.on_timeout(32_000),
            "refused" => {
                r.on_response(100, &removal.response(500, None), "c");
            }
            _ => {
                r.on_response(100, &challenge(&removal, "n", false), "c");
            }
        }
        assert_eq!(r.state(), RegState::Unregistered, "{outcome}");
        assert_eq!(r.next_deadline(), None);
        assert_eq!(r.poll(10_000_000, "c"), None);
    }
}

#[test]
fn there_is_nothing_to_remove_before_registering() {
    let mut r = registration();
    assert_eq!(r.unregister("c"), None);
    assert_eq!(r.state(), RegState::Unregistered);
    assert_eq!(r.poll(0, "c"), None);

    // A failed registration has no binding either.
    let mut r = registration();
    r.poll(0, "c").unwrap();
    r.on_timeout(32_000);
    assert_eq!(r.unregister("c"), None);
    assert_eq!(r.state(), RegState::Unregistered);

    // One that is still being made may have one by the time we stop.
    let mut r = registration();
    r.poll(0, "c").unwrap();
    let removal = r.unregister("c").unwrap();
    assert_eq!(removal.headers.expires(), Some(0));
    assert_eq!(removal.headers.get("CSeq"), Some("2 REGISTER"));
}
