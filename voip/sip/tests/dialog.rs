// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Dialogs (RFC 3261 section 12) and the 2xx that waits for its ACK.

use sip::transaction::{T1_MS, TIMEOUT_MS};
use sip::{Dialog, DialogError, Method, OkPoll, OkRetransmit, Request, Response, SipUri};

/// An INVITE from 201 at 10.10.20.11 to 5551000 at 10.10.20.5.
fn invite() -> Request {
    let mut request = Request::new(
        Method::Invite,
        &SipUri::new(Some("5551000"), "10.10.20.5", None),
    );
    let headers = &mut request.headers;
    headers.push("Via", "SIP/2.0/UDP 10.10.20.11:5060;branch=z9hG4bKa");
    headers.push("Max-Forwards", "70");
    headers.push("From", "\"Front desk\" <sip:201@10.10.20.11>;tag=caller");
    headers.push("To", "<sip:5551000@10.10.20.5>");
    headers.push("Call-ID", "call-1@10.10.20.11");
    headers.push("CSeq", "41 INVITE");
    headers.push("Contact", "<sip:201@10.10.20.11:5062>");
    request
}

/// The 200 that answers it, from the one called.
fn ok(invite: &Request) -> Response {
    let mut response = invite.response(200, Some("callee"));
    response
        .headers
        .push("Contact", "<sip:5551000@10.10.20.5:5060;line=1>");
    response
}

fn with(mut message: Request, name: &str, value: &str) -> Request {
    message.headers.push(name, value);
    message
}

#[test]
fn the_callers_dialog() {
    let invite = invite();
    let mut dialog = Dialog::uac(&invite, &ok(&invite)).unwrap();
    assert_eq!(dialog.call_id, "call-1@10.10.20.11");
    assert_eq!(dialog.local_tag, "caller");
    assert_eq!(dialog.remote_tag, "callee");
    assert_eq!(
        dialog.remote_target().to_string(),
        "sip:5551000@10.10.20.5:5060;line=1"
    );
    assert_eq!(dialog.next_hop(), *dialog.remote_target());
    assert_eq!(dialog.local_sequence(), 41);

    // Section 12.2.1.1: a request in the dialog.
    let bye = dialog.request(Method::Bye);
    assert_eq!(bye.method, Method::Bye);
    assert_eq!(bye.uri, "sip:5551000@10.10.20.5:5060;line=1");
    assert_eq!(
        bye.headers.get("From"),
        Some("\"Front desk\" <sip:201@10.10.20.11>;tag=caller")
    );
    assert_eq!(
        bye.headers.get("To"),
        Some("<sip:5551000@10.10.20.5>;tag=callee")
    );
    assert_eq!(bye.headers.get("Call-ID"), Some("call-1@10.10.20.11"));
    assert_eq!(bye.headers.get("CSeq"), Some("42 BYE"));
    assert_eq!(bye.headers.max_forwards(), Some(70));
    assert!(bye.headers.routes().unwrap().is_empty());
    assert!(!bye.headers.has("Via"), "the transaction adds the Via");
    assert_eq!(
        dialog.request(Method::Invite).headers.get("CSeq"),
        Some("43 INVITE")
    );
}

#[test]
fn the_ack_of_a_2xx_has_the_number_of_its_invite() {
    let invite = invite();
    let dialog = Dialog::uac(&invite, &ok(&invite)).unwrap();
    let ack = dialog.ack(41);
    assert_eq!(ack.method, Method::Ack);
    assert_eq!(ack.headers.get("CSeq"), Some("41 ACK"));
    assert_eq!(ack.uri, "sip:5551000@10.10.20.5:5060;line=1");
    assert_eq!(
        ack.headers.get("To"),
        Some("<sip:5551000@10.10.20.5>;tag=callee")
    );
    // It does not use up a number.
    assert_eq!(dialog.local_sequence(), 41);
}

#[test]
fn the_dialog_of_the_one_called() {
    let invite = invite();
    let mut dialog = Dialog::uas(&invite, &ok(&invite)).unwrap();
    assert_eq!(dialog.local_tag, "callee");
    assert_eq!(dialog.remote_tag, "caller");
    assert_eq!(
        dialog.remote_target().to_string(),
        "sip:201@10.10.20.11:5062"
    );

    let bye = dialog.request(Method::Bye);
    assert_eq!(bye.uri, "sip:201@10.10.20.11:5062");
    assert_eq!(
        bye.headers.get("From"),
        Some("<sip:5551000@10.10.20.5>;tag=callee")
    );
    assert_eq!(
        bye.headers.get("To"),
        Some("\"Front desk\" <sip:201@10.10.20.11>;tag=caller")
    );
    // Our own numbering starts at 1.
    assert_eq!(bye.headers.get("CSeq"), Some("1 BYE"));
}

#[test]
fn the_route_set_of_each_side() {
    let invite = with(
        with(invite(), "Record-Route", "<sip:p1.example.lan;lr>"),
        "Record-Route",
        "<sip:p2.example.lan;lr>",
    );
    let mut response = ok(&invite);
    for route in invite.headers.all("Record-Route") {
        response.headers.push("Record-Route", route);
    }

    // The one called takes the Record-Route in order.
    let mut callee = Dialog::uas(&invite, &response).unwrap();
    let bye = callee.request(Method::Bye);
    assert_eq!(
        bye.headers.all("Route").collect::<Vec<_>>(),
        ["<sip:p1.example.lan;lr>", "<sip:p2.example.lan;lr>"]
    );
    assert_eq!(bye.uri, "sip:201@10.10.20.11:5062");
    assert_eq!(callee.next_hop().to_string(), "sip:p1.example.lan;lr");

    // The caller takes it reversed (section 12.1.2).
    let mut caller = Dialog::uac(&invite, &response).unwrap();
    let bye = caller.request(Method::Bye);
    assert_eq!(
        bye.headers.all("Route").collect::<Vec<_>>(),
        ["<sip:p2.example.lan;lr>", "<sip:p1.example.lan;lr>"]
    );
    assert_eq!(bye.uri, "sip:5551000@10.10.20.5:5060;line=1");
    assert_eq!(caller.next_hop().to_string(), "sip:p2.example.lan;lr");
}

#[test]
fn a_strict_router_is_named_in_the_request_line() {
    // Section 12.2.1.1: the first route has no lr.
    let invite = with(
        with(invite(), "Record-Route", "<sip:strict.example.lan>"),
        "Record-Route",
        "<sip:p2.example.lan;lr>",
    );
    let mut dialog = Dialog::uas(&invite, &ok(&invite)).unwrap();
    let bye = dialog.request(Method::Bye);
    assert_eq!(bye.uri, "sip:strict.example.lan");
    assert_eq!(
        bye.headers.all("Route").collect::<Vec<_>>(),
        ["<sip:p2.example.lan;lr>", "<sip:201@10.10.20.11:5062>"]
    );
    assert_eq!(dialog.next_hop().to_string(), "sip:strict.example.lan");
}

#[test]
fn what_is_in_the_dialog_and_what_is_not() {
    let invite = invite();
    let response = ok(&invite);
    let mut caller = Dialog::uac(&invite, &response).unwrap();
    let mut callee = Dialog::uas(&invite, &response).unwrap();

    // What one side sends is in the other side's dialog.
    let bye = caller.request(Method::Bye);
    assert!(callee.has_request(&bye));
    assert!(!caller.has_request(&bye));
    let answer = bye.response(200, None);
    assert!(caller.has_response(&answer));
    assert!(!callee.has_response(&answer));
    let from_callee = callee.request(Method::Bye);
    assert!(caller.has_request(&from_callee));

    // Another call, or another tag, is not.
    let mut other = bye.clone();
    other.headers.set("Call-ID", "call-2@10.10.20.11");
    assert!(!callee.has_request(&other));
    let mut other = bye.clone();
    other
        .headers
        .set("To", "<sip:5551000@10.10.20.5>;tag=someone-else");
    assert!(!callee.has_request(&other));
    let mut other = bye.clone();
    other
        .headers
        .set("From", "<sip:201@10.10.20.11>;tag=forked");
    assert!(!callee.has_request(&other));
    // The first INVITE itself has no tag of ours yet.
    assert!(!callee.has_request(&invite));
}

#[test]
fn a_far_end_with_no_tag_is_still_a_dialog() {
    let mut invite = invite();
    invite.headers.set("From", "<sip:201@10.10.20.11>");
    let mut dialog = Dialog::uas(&invite, &ok(&invite)).unwrap();
    assert_eq!(dialog.remote_tag, "");
    let mut bye = Request::new(
        Method::Bye,
        &SipUri::new(Some("5551000"), "10.10.20.5", None),
    );
    bye.headers.push("From", "<sip:201@10.10.20.11>");
    bye.headers
        .push("To", "<sip:5551000@10.10.20.5>;tag=callee");
    bye.headers.push("Call-ID", "call-1@10.10.20.11");
    bye.headers.push("CSeq", "42 BYE");
    assert!(dialog.has_request(&bye));
    assert_eq!(dialog.accept(&bye), Ok(()));
}

#[test]
fn requests_in_a_dialog_are_taken_in_order() {
    let invite = invite();
    let response = ok(&invite);
    let mut caller = Dialog::uac(&invite, &response).unwrap();
    let mut callee = Dialog::uas(&invite, &response).unwrap();

    // The ACK carries the INVITE's number and is not counted.
    assert_eq!(callee.accept(&caller.ack(41)), Ok(()));

    let reinvite = with(
        caller.request(Method::Invite),
        "Contact",
        "<sip:201@10.10.20.99:5070>",
    );
    assert_eq!(callee.accept(&reinvite), Ok(()));
    // A re-INVITE moves the target (section 12.2.2).
    assert_eq!(
        callee.remote_target().to_string(),
        "sip:201@10.10.20.99:5070"
    );
    assert_eq!(callee.request(Method::Bye).uri, "sip:201@10.10.20.99:5070");

    // A number that goes backwards is refused with 500.
    let mut stale = caller.request(Method::Bye);
    stale.headers.set("CSeq", "40 BYE");
    assert_eq!(callee.accept(&stale), Err(500));
    // The same number again is let through: the transaction layer has
    // already absorbed what was a retransmission.
    let mut same = caller.request(Method::Bye);
    same.headers.set("CSeq", "42 BYE");
    assert_eq!(callee.accept(&same), Ok(()));
    let mut none = caller.request(Method::Bye);
    none.headers.remove("CSeq");
    assert_eq!(callee.accept(&none), Err(400));

    // A BYE with a Contact does not move the target.
    let bye = with(caller.request(Method::Bye), "Contact", "<sip:x@10.0.0.1>");
    assert_eq!(callee.accept(&bye), Ok(()));
    assert_eq!(
        callee.remote_target().to_string(),
        "sip:201@10.10.20.99:5070"
    );

    // The caller's side has seen nothing from the far end yet: any number
    // is a first.
    assert_eq!(caller.accept(&callee.request(Method::Bye)), Ok(()));
}

#[test]
fn what_does_not_make_a_dialog() {
    let invite = invite();

    let mut no_contact = ok(&invite);
    no_contact.headers.remove("Contact");
    assert_eq!(
        Dialog::uac(&invite, &no_contact),
        Err(DialogError::Header("Contact"))
    );
    let mut tel = ok(&invite);
    tel.headers.set("Contact", "<tel:+15551000>");
    assert_eq!(Dialog::uac(&invite, &tel), Err(DialogError::Contact));
    let mut bad_route = ok(&invite);
    bad_route.headers.push("Record-Route", "not an address <");
    assert_eq!(
        Dialog::uac(&invite, &bad_route),
        Err(DialogError::Header("Record-Route"))
    );

    // Our 2xx must carry our tag.
    let untagged = invite.response(200, None);
    assert_eq!(
        Dialog::uas(&invite, &untagged),
        Err(DialogError::Header("To"))
    );
    let mut caller_without_contact = invite.clone();
    caller_without_contact.headers.remove("Contact");
    assert_eq!(
        Dialog::uas(&caller_without_contact, &ok(&invite)),
        Err(DialogError::Header("Contact"))
    );
    let mut broken = invite.clone();
    broken.headers.remove("CSeq");
    assert_eq!(
        Dialog::uas(&broken, &ok(&invite)),
        Err(DialogError::Header("CSeq"))
    );
    assert_eq!(
        DialogError::Contact.to_string(),
        "the Contact is not a sip: URI"
    );
}

#[test]
fn a_2xx_is_sent_again_until_its_ack_or_64_t1() {
    let to = "10.10.20.11:5060".parse().unwrap();
    let mut ok = OkRetransmit::new(1000, b"SIP/2.0 200 OK\r\n\r\n".to_vec(), to);
    assert_eq!(ok.to, to);
    assert_eq!(ok.poll(1000), OkPoll::Wait);
    assert_eq!(ok.poll(1000 + T1_MS - 1), OkPoll::Wait);

    // Section 13.3.1.4: after T1, doubling up to T2, for 64 * T1.
    let mut resent = Vec::new();
    let gave_up = loop {
        let at = ok.next_deadline();
        match ok.poll(at) {
            OkPoll::Resend => resent.push(at - 1000),
            OkPoll::GiveUp => break at - 1000,
            OkPoll::Wait => panic!("nothing was due at its own deadline"),
        }
    };
    assert_eq!(
        resent,
        [
            500, 1500, 3500, 7500, 11_500, 15_500, 19_500, 23_500, 27_500, 31_500
        ]
    );
    assert_eq!(gave_up, TIMEOUT_MS);
    // It stays given up.
    assert_eq!(ok.poll(1000 + TIMEOUT_MS + 5000), OkPoll::GiveUp);
}
