// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The transaction layer against a clock the test moves: every state and
//! every timer of the four machines, with messages lost and sent twice in
//! each state.

use std::net::SocketAddr;

use sip::transaction::{
    MAGIC_COOKIE, Output, ReceiveError, SentBy, T1_MS, T2_MS, T4_MS, TIMEOUT_MS, TIMER_D_MS,
    Transactions, TxId,
};
use sip::{Invalid, Message, Method, Request, Response, SipUri};

fn peer() -> SocketAddr {
    "10.10.20.11:5060".parse().unwrap()
}

fn us() -> SentBy {
    SentBy {
        host: "10.10.20.5".to_owned(),
        port: 5060,
    }
}

/// A request of ours, without its Via.
fn ours(method: Method) -> Request {
    let mut request = Request::new(
        method.clone(),
        &SipUri::new(Some("201"), "10.10.20.11", None),
    );
    request.headers.push("Max-Forwards", "70");
    request
        .headers
        .push("From", "<sip:7002@10.10.20.5>;tag=local");
    request.headers.push("To", "<sip:201@10.10.20.11>");
    request.headers.push("Call-ID", "call-1@10.10.20.5");
    request.headers.push("CSeq", format!("7 {method}"));
    request.headers.push("Route", "<sip:proxy.example.lan;lr>");
    request
}

/// A request from the peer, with its Via.
fn theirs(method: Method, branch: &str) -> Request {
    let mut request = Request::new(
        method.clone(),
        &SipUri::new(Some("5551000"), "10.10.20.5", None),
    );
    request.headers.push(
        "Via",
        format!("SIP/2.0/UDP 10.10.20.11:5060;branch={branch}"),
    );
    request
        .headers
        .push("From", "<sip:201@10.10.20.11>;tag=remote");
    request.headers.push("To", "<sip:5551000@10.10.20.5>");
    request.headers.push("Call-ID", "call-2@10.10.20.11");
    request.headers.push("CSeq", format!("3 {method}"));
    request
}

/// The layer under test, with the outputs of the last step.
struct Layer {
    t: Transactions,
    out: Vec<Output>,
}

impl Layer {
    fn new() -> Layer {
        Layer {
            t: Transactions::new(0xABCD),
            out: Vec::new(),
        }
    }

    fn send(&mut self, now: u64, request: Request) -> TxId {
        self.out.clear();
        self.t
            .send_request(now, request, peer(), &us(), &mut self.out)
    }

    fn receive(&mut self, now: u64, message: impl Into<Message>) {
        self.out.clear();
        self.t
            .receive(now, peer(), message.into(), &mut self.out)
            .expect("the message is taken");
    }

    fn poll(&mut self, now: u64) {
        self.out.clear();
        self.t.poll(now, &mut self.out);
    }

    fn respond(&mut self, now: u64, tx: TxId, response: &Response) -> bool {
        self.out.clear();
        self.t.respond(now, tx, response, &mut self.out)
    }

    /// The datagrams the last step sent, parsed.
    fn sent(&self) -> Vec<Message> {
        self.out
            .iter()
            .filter_map(|output| match output {
                Output::Send { datagram, .. } => Some(Message::parse(datagram).unwrap()),
                _ => None,
            })
            .collect()
    }

    fn sent_requests(&self) -> Vec<Request> {
        self.sent()
            .into_iter()
            .filter_map(|m| match m {
                Message::Request(r) => Some(r),
                Message::Response(_) => None,
            })
            .collect()
    }

    fn sent_statuses(&self) -> Vec<u16> {
        self.sent()
            .into_iter()
            .filter_map(|m| match m {
                Message::Response(r) => Some(r.status),
                Message::Request(_) => None,
            })
            .collect()
    }

    /// The outputs of the last step that are not datagrams.
    fn told(&self) -> Vec<&Output> {
        self.out
            .iter()
            .filter(|output| !matches!(output, Output::Send { .. }))
            .collect()
    }

    /// Runs the timers one deadline at a time up to `until`, and returns
    /// when each datagram was sent and each other output given.
    fn run(&mut self, until: u64) -> (Vec<u64>, Vec<(u64, Output)>) {
        let mut sends = Vec::new();
        let mut told = Vec::new();
        while let Some(deadline) = self.t.next_deadline() {
            if deadline > until {
                break;
            }
            self.poll(deadline);
            for output in self.out.drain(..) {
                match output {
                    Output::Send { .. } => sends.push(deadline),
                    other => told.push((deadline, other)),
                }
            }
        }
        (sends, told)
    }
}

/// The request we last sent, as the peer received it.
fn on_the_wire(layer: &Layer) -> Request {
    layer
        .sent_requests()
        .into_iter()
        .next()
        .expect("a request was sent")
}

/// The peer's response to a request of ours.
fn answer(request: &Request, status: u16) -> Response {
    request.response(status, (status > 100).then_some("remote"))
}

// ---------------------------------------------------------------------
// Our Via.

#[test]
fn a_request_of_ours_gets_a_via_with_a_new_branch_and_rport() {
    let mut layer = Layer::new();
    layer.send(0, ours(Method::Options));
    let first = on_the_wire(&layer);
    layer.send(0, ours(Method::Options));
    let second = on_the_wire(&layer);

    let via = first.headers.top_via().unwrap();
    assert_eq!((via.host.as_str(), via.port), ("10.10.20.5", Some(5060)));
    assert_eq!(via.transport, "UDP");
    assert!(via.branch().unwrap().starts_with(MAGIC_COOKIE));
    // RFC 3581 section 3: rport with no value.
    assert!(via.params.has("rport"));
    assert_eq!(via.params.get("rport"), None);
    assert_ne!(via.branch(), second.headers.top_via().unwrap().branch());
    assert_eq!(first.headers.vias().unwrap().len(), 1);
    assert!(matches!(&layer.out[0], Output::Send { to, .. } if *to == peer()));
}

// ---------------------------------------------------------------------
// The INVITE client transaction (section 17.1.1).

#[test]
fn invite_client_calling_sends_again_at_doubling_intervals_and_times_out() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Invite));
    assert_eq!(layer.sent_requests().len(), 1);
    layer.poll(T1_MS - 1);
    assert!(layer.out.is_empty());

    // Timer A: T1, then twice the interval each time. Timer B: 64 * T1.
    let (sends, told) = layer.run(u64::MAX);
    assert_eq!(sends, [500, 1500, 3500, 7500, 15_500, 31_500]);
    assert_eq!(told, [(TIMEOUT_MS, Output::Timeout { tx })]);
    assert!(layer.t.is_empty());
    assert_eq!(layer.t.next_deadline(), None);
}

#[test]
fn invite_client_proceeding_stops_sending_and_waits_without_limit() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Invite));
    let invite = on_the_wire(&layer);
    layer.poll(500);
    assert_eq!(layer.sent_requests().len(), 1);

    let trying = answer(&invite, 100);
    layer.receive(600, trying.clone());
    assert_eq!(
        layer.told(),
        [&Output::Response {
            tx,
            response: trying.clone()
        }]
    );
    // Timers A and B are off: nothing more is sent and nothing times out.
    assert_eq!(layer.t.next_deadline(), None);
    layer.poll(10 * 60 * 1000);
    assert!(layer.out.is_empty());

    // Every provisional response is passed up, a repeated one too.
    let ringing = answer(&invite, 180);
    layer.receive(700, ringing.clone());
    layer.receive(800, ringing.clone());
    assert_eq!(layer.told().len(), 1);

    // The 2xx ends the transaction; its ACK is not ours to send.
    let ok = answer(&invite, 200);
    layer.receive(20 * 60 * 1000, ok.clone());
    assert_eq!(
        layer.told(),
        [&Output::Response {
            tx,
            response: ok.clone()
        }]
    );
    assert!(layer.sent().is_empty());
    assert!(layer.t.is_empty());

    // The 2xx again belongs to no transaction: the dialog's to answer.
    layer.receive(20 * 60 * 1000 + 500, ok.clone());
    assert_eq!(
        layer.told(),
        [&Output::StrayResponse {
            from: peer(),
            response: ok
        }]
    );
}

#[test]
fn invite_client_a_2xx_while_calling_ends_the_transaction() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Invite));
    let ok = answer(&on_the_wire(&layer), 200);
    layer.receive(100, ok.clone());
    assert_eq!(layer.told(), [&Output::Response { tx, response: ok }]);
    assert!(layer.t.is_empty());
}

#[test]
fn invite_client_acks_a_failure_itself_and_again_when_it_comes_again() {
    for first_a_provisional in [false, true] {
        let mut layer = Layer::new();
        let tx = layer.send(0, ours(Method::Invite));
        let invite = on_the_wire(&layer);
        if first_a_provisional {
            layer.receive(50, answer(&invite, 180));
        }

        let busy = answer(&invite, 486);
        layer.receive(100, busy.clone());
        assert_eq!(
            layer.told(),
            [&Output::Response {
                tx,
                response: busy.clone()
            }]
        );

        // Section 17.1.1.3: the ACK.
        let acks = layer.sent_requests();
        assert_eq!(acks.len(), 1);
        let ack = &acks[0];
        assert_eq!(ack.method, Method::Ack);
        assert_eq!(ack.uri, invite.uri);
        assert_eq!(ack.headers.vias().unwrap().len(), 1);
        assert_eq!(
            ack.headers.top_via().unwrap().branch(),
            invite.headers.top_via().unwrap().branch()
        );
        assert_eq!(ack.headers.get("From"), invite.headers.get("From"));
        assert_eq!(
            ack.headers.get("To"),
            Some("<sip:201@10.10.20.11>;tag=remote")
        );
        assert_eq!(ack.headers.get("Call-ID"), invite.headers.get("Call-ID"));
        assert_eq!(ack.headers.get("CSeq"), Some("7 ACK"));
        assert_eq!(ack.headers.get("Route"), invite.headers.get("Route"));
        assert_eq!(ack.headers.max_forwards(), Some(70));
        assert!(ack.body.is_empty());

        // The response again, because our ACK was lost: the ACK again, and
        // nothing told.
        layer.receive(600, busy.clone());
        assert_eq!(layer.sent_requests(), acks);
        assert!(layer.told().is_empty());
        // A stray provisional or 2xx in this state is dropped.
        layer.receive(700, answer(&invite, 180));
        layer.receive(700, answer(&invite, 200));
        assert!(layer.out.is_empty());

        // Timer D, then the transaction is gone.
        assert_eq!(layer.t.next_deadline(), Some(100 + TIMER_D_MS));
        layer.poll(100 + TIMER_D_MS - 1);
        assert!(!layer.t.is_empty());
        layer.poll(100 + TIMER_D_MS);
        assert!(layer.out.is_empty());
        assert!(layer.t.is_empty());
        layer.receive(100 + TIMER_D_MS + 1, busy.clone());
        assert!(matches!(layer.told()[..], [Output::StrayResponse { .. }]));
    }
}

#[test]
fn a_response_is_matched_by_branch_and_cseq_method() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Invite));
    let invite = on_the_wire(&layer);

    let mut other_branch = answer(&invite, 180);
    other_branch
        .headers
        .set("Via", "SIP/2.0/UDP 10.10.20.5:5060;branch=z9hG4bKother");
    layer.receive(10, other_branch);
    assert!(matches!(layer.told()[..], [Output::StrayResponse { .. }]));

    let mut other_method = answer(&invite, 200);
    other_method.headers.set("CSeq", "7 BYE");
    layer.receive(20, other_method);
    assert!(matches!(layer.told()[..], [Output::StrayResponse { .. }]));

    let mut no_via = answer(&invite, 200);
    no_via.headers.remove("Via");
    layer.receive(30, no_via);
    assert!(matches!(layer.told()[..], [Output::StrayResponse { .. }]));

    layer.receive(40, answer(&invite, 180));
    assert!(matches!(layer.told()[..], [Output::Response { tx: t, .. }] if *t == tx));
}

// ---------------------------------------------------------------------
// The non-INVITE client transaction (section 17.1.2).

#[test]
fn non_invite_client_trying_sends_again_up_to_t2_and_times_out() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Register));
    // Timer E: T1, doubling, capped at T2. Timer F: 64 * T1.
    let (sends, told) = layer.run(u64::MAX);
    assert_eq!(
        sends,
        [
            500, 1500, 3500, 7500, 11_500, 15_500, 19_500, 23_500, 27_500, 31_500
        ]
    );
    assert_eq!(told, [(TIMEOUT_MS, Output::Timeout { tx })]);
    assert!(layer.t.is_empty());
}

#[test]
fn non_invite_client_proceeding_sends_again_every_t2_and_still_times_out() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Options));
    let request = on_the_wire(&layer);
    let trying = answer(&request, 100);
    layer.receive(200, trying.clone());
    assert_eq!(
        layer.told(),
        [&Output::Response {
            tx,
            response: trying
        }]
    );

    let (sends, told) = layer.run(u64::MAX);
    // The timer that was running fires as it was set, then every T2.
    assert_eq!(sends[0], 500);
    for pair in sends.windows(2) {
        assert_eq!(pair[1] - pair[0], T2_MS);
    }
    assert_eq!(sends.len(), 8);
    assert_eq!(told, [(TIMEOUT_MS, Output::Timeout { tx })]);
}

#[test]
fn non_invite_client_completed_absorbs_the_response_again_for_t4() {
    for first_a_provisional in [false, true] {
        let mut layer = Layer::new();
        let tx = layer.send(0, ours(Method::Bye));
        let request = on_the_wire(&layer);
        if first_a_provisional {
            layer.receive(50, answer(&request, 100));
        }
        let ok = answer(&request, 200);
        layer.receive(100, ok.clone());
        assert_eq!(
            layer.told(),
            [&Output::Response {
                tx,
                response: ok.clone()
            }]
        );

        // Timer K: nothing is sent again, and the response again is dropped.
        assert_eq!(layer.t.next_deadline(), Some(100 + T4_MS));
        layer.receive(600, ok.clone());
        layer.receive(700, answer(&request, 100));
        assert!(layer.out.is_empty());
        let (sends, told) = layer.run(u64::MAX);
        assert!(sends.is_empty() && told.is_empty());
        assert!(layer.t.is_empty());
        layer.receive(100 + T4_MS + 1, ok.clone());
        assert!(matches!(layer.told()[..], [Output::StrayResponse { .. }]));
    }
}

#[test]
fn non_invite_client_a_failure_is_a_final_response_like_any_other() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Register));
    let refused = answer(&on_the_wire(&layer), 403);
    layer.receive(100, refused.clone());
    assert_eq!(
        layer.told(),
        [&Output::Response {
            tx,
            response: refused
        }]
    );
    // No ACK: that is for INVITE only.
    assert!(layer.sent().is_empty());
}

// ---------------------------------------------------------------------
// CANCEL (section 9.1).

#[test]
fn a_cancel_waits_for_the_first_provisional_response() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Invite));
    let invite = on_the_wire(&layer);

    layer.out.clear();
    assert!(layer.t.cancel(100, tx, &mut layer.out));
    assert!(layer.out.is_empty(), "nothing is sent before a response");
    // The INVITE is still sent again meanwhile.
    layer.poll(500);
    assert_eq!(layer.sent_requests()[0].method, Method::Invite);

    let trying = answer(&invite, 100);
    layer.receive(600, trying.clone());
    let sent = layer.sent_requests();
    assert_eq!(sent.len(), 1);
    let cancel = &sent[0];
    assert_eq!(cancel.method, Method::Cancel);
    assert_eq!(cancel.uri, invite.uri);
    assert_eq!(cancel.headers.vias().unwrap().len(), 1);
    assert_eq!(cancel.headers.get("Via"), invite.headers.all("Via").next());
    assert_eq!(cancel.headers.get("From"), invite.headers.get("From"));
    assert_eq!(cancel.headers.get("To"), invite.headers.get("To"));
    assert_eq!(cancel.headers.get("Call-ID"), invite.headers.get("Call-ID"));
    assert_eq!(cancel.headers.get("CSeq"), Some("7 CANCEL"));
    assert_eq!(cancel.headers.get("Route"), invite.headers.get("Route"));
    assert_eq!(
        layer.told(),
        [&Output::Response {
            tx,
            response: trying
        }]
    );

    // The CANCEL is a transaction of its own and is sent again like one.
    layer.poll(600 + T1_MS);
    assert_eq!(layer.sent_requests()[0].method, Method::Cancel);

    // Its 200 is not told; the INVITE's 487 is, and is acknowledged.
    layer.receive(1200, answer(cancel, 200));
    assert!(layer.out.is_empty());
    let terminated = answer(&invite, 487);
    layer.receive(1300, terminated.clone());
    assert_eq!(
        layer.told(),
        [&Output::Response {
            tx,
            response: terminated
        }]
    );
    assert_eq!(layer.sent_requests()[0].method, Method::Ack);

    let (_, told) = layer.run(u64::MAX);
    assert!(
        told.is_empty(),
        "the CANCEL does not time out aloud: {told:?}"
    );
    assert!(layer.t.is_empty());
}

#[test]
fn a_cancel_goes_at_once_when_a_provisional_response_has_come() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Invite));
    let invite = on_the_wire(&layer);
    layer.receive(100, answer(&invite, 180));

    layer.out.clear();
    assert!(layer.t.cancel(200, tx, &mut layer.out));
    assert_eq!(layer.sent_requests()[0].method, Method::Cancel);

    // A CANCEL that is never answered is given up on quietly.
    let (sends, told) = layer.run(u64::MAX);
    assert_eq!(sends.len(), 10);
    assert!(told.is_empty());
    // The INVITE is still waiting for its final response.
    assert_eq!(layer.t.len(), (1, 0));
}

#[test]
fn there_is_nothing_to_cancel_once_the_invite_is_answered_or_unknown() {
    let mut layer = Layer::new();
    let tx = layer.send(0, ours(Method::Invite));
    let invite = on_the_wire(&layer);
    layer.receive(100, answer(&invite, 486));
    layer.out.clear();
    assert!(!layer.t.cancel(200, tx, &mut layer.out));
    assert!(!layer.t.cancel(200, TxId(999), &mut layer.out));
    let register = layer.send(300, ours(Method::Register));
    layer.out.clear();
    assert!(!layer.t.cancel(400, register, &mut layer.out));
    assert!(layer.out.is_empty());
}

// ---------------------------------------------------------------------
// The INVITE server transaction (section 17.2.1).

/// Takes a request from the peer and returns its transaction and the
/// request as the layer hands it up.
fn arrive(layer: &mut Layer, now: u64, request: Request) -> (TxId, Request) {
    layer.receive(now, request);
    match layer.out.as_slice() {
        [Output::Request { tx, from, request }] => {
            assert_eq!(*from, peer());
            (*tx, request.clone())
        }
        other => panic!("not a new request: {other:?}"),
    }
}

#[test]
fn invite_server_proceeding_answers_the_request_again_with_its_last_response() {
    let mut layer = Layer::new();
    let invite = theirs(Method::Invite, "z9hG4bKa");
    let (tx, request) = arrive(&mut layer, 0, invite.clone());

    // Before anything is sent there is nothing to send again.
    layer.receive(100, invite.clone());
    assert!(layer.out.is_empty());

    assert!(layer.respond(200, tx, &request.response(100, None)));
    assert_eq!(layer.sent_statuses(), [100]);
    layer.receive(300, invite.clone());
    assert_eq!(layer.sent_statuses(), [100]);

    assert!(layer.respond(400, tx, &request.response(180, Some("local"))));
    layer.receive(500, invite.clone());
    assert_eq!(layer.sent_statuses(), [180]);
    assert!(layer.told().is_empty());
    // Nothing runs on a timer while proceeding.
    assert_eq!(layer.t.next_deadline(), None);
}

#[test]
fn invite_server_a_2xx_is_sent_once_and_the_invite_again_is_absorbed() {
    let mut layer = Layer::new();
    let invite = theirs(Method::Invite, "z9hG4bKa");
    let (tx, request) = arrive(&mut layer, 0, invite.clone());
    assert!(layer.respond(100, tx, &request.response(200, Some("local"))));
    assert_eq!(layer.sent_statuses(), [200]);

    // RFC 6026: the INVITE again is not a new call, and the 2xx is the
    // dialog's to send again, not ours.
    layer.receive(600, invite.clone());
    assert!(layer.out.is_empty());
    assert!(!layer.respond(700, tx, &request.response(200, Some("local"))));

    // The ACK of a 2xx has a branch of its own and goes up.
    let ack = theirs(Method::Ack, "z9hG4bKack");
    layer.receive(800, ack.clone());
    assert_eq!(
        layer.told(),
        [&Output::Ack {
            from: peer(),
            request: ack
        }]
    );

    // Timer L, then it is gone, and the INVITE once more is a new call.
    let (sends, told) = layer.run(u64::MAX);
    assert!(sends.is_empty() && told.is_empty());
    assert!(layer.t.is_empty());
    arrive(&mut layer, 100 + TIMEOUT_MS + 1, invite);
}

#[test]
fn invite_server_a_failure_is_sent_again_until_the_ack() {
    let mut layer = Layer::new();
    let invite = theirs(Method::Invite, "z9hG4bKa");
    let (tx, request) = arrive(&mut layer, 0, invite.clone());
    layer.respond(0, tx, &request.response(180, Some("local")));
    assert!(layer.respond(1000, tx, &request.response(486, Some("local"))));
    assert_eq!(layer.sent_statuses(), [486]);
    assert!(!layer.respond(1001, tx, &request.response(200, Some("local"))));

    // Timer G: T1, doubling, capped at T2.
    layer.poll(1000 + T1_MS);
    assert_eq!(layer.sent_statuses(), [486]);
    layer.poll(1000 + 3 * T1_MS);
    assert_eq!(layer.sent_statuses(), [486]);
    // The INVITE again gets the response again too.
    layer.receive(2600, invite.clone());
    assert_eq!(layer.sent_statuses(), [486]);

    // The ACK has the INVITE's branch. It stops the sending.
    let ack = theirs(Method::Ack, "z9hG4bKa");
    layer.receive(2700, ack.clone());
    assert!(layer.out.is_empty());
    assert_eq!(layer.t.next_deadline(), Some(2700 + T4_MS));
    // Timer I: the ACK and the INVITE again are absorbed.
    layer.receive(2800, ack);
    layer.receive(2900, invite.clone());
    assert!(layer.out.is_empty());
    let (sends, told) = layer.run(u64::MAX);
    assert!(sends.is_empty() && told.is_empty());
    assert!(layer.t.is_empty());
}

#[test]
fn invite_server_a_failure_never_acknowledged_is_reported() {
    let mut layer = Layer::new();
    let (tx, request) = arrive(&mut layer, 0, theirs(Method::Invite, "z9hG4bKa"));
    layer.respond(0, tx, &request.response(603, Some("local")));
    let (sends, told) = layer.run(u64::MAX);
    // Timer G, and timer H at 64 * T1.
    assert_eq!(
        sends,
        [
            500, 1500, 3500, 7500, 11_500, 15_500, 19_500, 23_500, 27_500, 31_500
        ]
    );
    assert_eq!(told, [(TIMEOUT_MS, Output::NoAck { tx })]);
    assert!(layer.t.is_empty());
}

// ---------------------------------------------------------------------
// The non-INVITE server transaction (section 17.2.2).

#[test]
fn non_invite_server_states() {
    let mut layer = Layer::new();
    let options = theirs(Method::Options, "z9hG4bKb");
    let (tx, request) = arrive(&mut layer, 0, options.clone());

    // Trying: the request again is dropped.
    layer.receive(100, options.clone());
    assert!(layer.out.is_empty());

    // Proceeding: the request again gets the provisional response again.
    assert!(layer.respond(200, tx, &request.response(100, None)));
    layer.receive(300, options.clone());
    assert_eq!(layer.sent_statuses(), [100]);

    // Completed: the final response again, for 64 * T1 (timer J).
    assert!(layer.respond(400, tx, &request.response(200, Some("local"))));
    assert_eq!(layer.sent_statuses(), [200]);
    assert!(!layer.respond(401, tx, &request.response(500, Some("local"))));
    layer.receive(500, options.clone());
    layer.receive(30_000, options.clone());
    assert_eq!(layer.sent_statuses(), [200]);
    assert!(layer.told().is_empty());
    assert_eq!(layer.t.next_deadline(), Some(400 + TIMEOUT_MS));
    let (sends, told) = layer.run(u64::MAX);
    assert!(sends.is_empty() && told.is_empty());
    assert!(layer.t.is_empty());

    // After that the same request is a new one.
    arrive(&mut layer, 400 + TIMEOUT_MS + 1, options);
}

#[test]
fn non_invite_server_answered_at_once() {
    let mut layer = Layer::new();
    let bye = theirs(Method::Bye, "z9hG4bKc");
    let (tx, request) = arrive(&mut layer, 0, bye.clone());
    assert!(layer.respond(0, tx, &request.response(200, None)));
    layer.receive(500, bye);
    assert_eq!(layer.sent_statuses(), [200]);
    assert!(!layer.respond(600, TxId(4242), &request.response(200, None)));
}

// ---------------------------------------------------------------------
// Matching requests (section 17.2.3) and CANCEL (section 9.2).

#[test]
fn a_cancel_is_its_own_transaction_and_finds_its_invite() {
    let mut layer = Layer::new();
    let (invite_tx, invite) = arrive(&mut layer, 0, theirs(Method::Invite, "z9hG4bKa"));
    layer.respond(0, invite_tx, &invite.response(180, Some("local")));

    let (cancel_tx, cancel) = arrive(&mut layer, 100, theirs(Method::Cancel, "z9hG4bKa"));
    assert_ne!(cancel_tx, invite_tx);
    assert_eq!(layer.t.invite_to_cancel(&cancel), Some(invite_tx));

    // A CANCEL for another branch, or from another sent-by, finds nothing.
    assert_eq!(
        layer
            .t
            .invite_to_cancel(&theirs(Method::Cancel, "z9hG4bKother")),
        None
    );
    let mut elsewhere = theirs(Method::Cancel, "z9hG4bKa");
    elsewhere
        .headers
        .set("Via", "SIP/2.0/UDP 10.10.20.12:5060;branch=z9hG4bKa");
    assert_eq!(layer.t.invite_to_cancel(&elsewhere), None);

    assert!(layer.respond(100, cancel_tx, &cancel.response(200, None)));
    assert!(layer.respond(100, invite_tx, &invite.response(487, Some("local"))));
    // Once the INVITE has its final response there is nothing to cancel.
    assert_eq!(layer.t.invite_to_cancel(&cancel), None);
}

#[test]
fn requests_are_matched_by_branch_sent_by_and_method() {
    let mut layer = Layer::new();
    let (first, _) = arrive(&mut layer, 0, theirs(Method::Options, "z9hG4bKa"));
    // Another branch, another method, another sent-by: each a new one.
    let (second, _) = arrive(&mut layer, 0, theirs(Method::Options, "z9hG4bKb"));
    let (third, _) = arrive(&mut layer, 0, theirs(Method::Bye, "z9hG4bKa"));
    let mut elsewhere = theirs(Method::Options, "z9hG4bKa");
    elsewhere
        .headers
        .set("Via", "SIP/2.0/UDP 10.10.20.11:5062;branch=z9hG4bKa");
    let (fourth, _) = arrive(&mut layer, 0, elsewhere);
    let ids = [first, second, third, fourth];
    for (n, id) in ids.iter().enumerate() {
        assert!(!ids[..n].contains(id));
    }
    assert_eq!(layer.t.len(), (0, 4));
    // The host of the sent-by is compared without regard to case, and a
    // port that is left out is 5060.
    let mut same = theirs(Method::Options, "z9hG4bKb");
    same.headers
        .set("Via", "SIP/2.0/UDP 10.10.20.11;branch=z9hG4bKb");
    layer.receive(10, same);
    assert!(layer.out.is_empty());
}

#[test]
fn a_branch_without_the_cookie_is_matched_with_the_call_and_the_sequence() {
    let mut layer = Layer::new();
    let old = theirs(Method::Options, "1");
    let (tx, request) = arrive(&mut layer, 0, old.clone());
    layer.respond(0, tx, &request.response(200, None));
    // The same request again is the same transaction.
    layer.receive(100, old.clone());
    assert_eq!(layer.sent_statuses(), [200]);

    // The same branch on another call, or on a later request, is not.
    let mut other_call = old.clone();
    other_call.headers.set("Call-ID", "another@10.10.20.11");
    arrive(&mut layer, 200, other_call);
    let mut later = old.clone();
    later.headers.set("CSeq", "4 OPTIONS");
    arrive(&mut layer, 300, later);
    let mut no_branch = old;
    no_branch.headers.set("Via", "SIP/2.0/UDP 10.10.20.11:5060");
    arrive(&mut layer, 400, no_branch);
}

#[test]
fn a_request_without_what_it_is_matched_by_is_refused() {
    let mut layer = Layer::new();
    let mut request = theirs(Method::Options, "z9hG4bKa");
    request.headers.remove("CSeq");
    assert_eq!(
        layer
            .t
            .receive(0, peer(), Message::Request(request), &mut layer.out),
        Err(ReceiveError::Invalid(Invalid::Header("CSeq")))
    );
    let mut request = theirs(Method::Options, "z9hG4bKa");
    request.headers.set("CSeq", "3 INVITE");
    assert_eq!(
        layer
            .t
            .receive(0, peer(), Message::Request(request), &mut layer.out),
        Err(ReceiveError::Invalid(Invalid::CSeqMethod))
    );
    assert!(layer.out.is_empty());
    assert!(layer.t.is_empty());
}

#[test]
fn a_server_transaction_can_be_forgotten() {
    let mut layer = Layer::new();
    let options = theirs(Method::Options, "z9hG4bKa");
    let (tx, _) = arrive(&mut layer, 0, options.clone());
    layer.t.forget_server(tx);
    assert!(layer.t.is_empty());
    arrive(&mut layer, 10, options);
}

// ---------------------------------------------------------------------
// Where responses go (section 18.2.2, RFC 3581).

fn reply_address(via: &str, from: &str) -> (SocketAddr, String) {
    let mut layer = Layer::new();
    let mut request = theirs(Method::Options, "z9hG4bKa");
    request.headers.set("Via", via);
    let from: SocketAddr = from.parse().unwrap();
    layer.out.clear();
    layer
        .t
        .receive(0, from, Message::Request(request), &mut layer.out)
        .unwrap();
    let [Output::Request { tx, request, .. }] = layer.out.as_slice() else {
        panic!("not a request");
    };
    let (tx, request) = (*tx, request.clone());
    let response = request.response(200, None);
    layer.respond(0, tx, &response);
    let [Output::Send { to, .. }] = layer.out.as_slice() else {
        panic!("nothing sent");
    };
    (*to, response.headers.get("Via").unwrap().to_owned())
}

#[test]
fn a_response_goes_back_as_the_via_asks() {
    // No rport: to the source address, at the port the Via names.
    assert_eq!(
        reply_address(
            "SIP/2.0/UDP 10.10.20.11:5062;branch=z9hG4bKa",
            "10.10.20.11:40000"
        ),
        (
            "10.10.20.11:5062".parse().unwrap(),
            "SIP/2.0/UDP 10.10.20.11:5062;branch=z9hG4bKa".to_owned()
        )
    );
    // No port in the Via: 5060.
    assert_eq!(
        reply_address(
            "SIP/2.0/UDP 10.10.20.11;branch=z9hG4bKa",
            "10.10.20.11:40000"
        )
        .0,
        "10.10.20.11:5060".parse().unwrap()
    );
    // The Via names another host than the packet came from: received.
    assert_eq!(
        reply_address(
            "SIP/2.0/UDP ata.example.lan:5062;branch=z9hG4bKa",
            "10.10.20.11:40000"
        ),
        (
            "10.10.20.11:5062".parse().unwrap(),
            "SIP/2.0/UDP ata.example.lan:5062;branch=z9hG4bKa;received=10.10.20.11".to_owned()
        )
    );
    // rport: back to the very address and port, and both are written in.
    assert_eq!(
        reply_address(
            "SIP/2.0/UDP 192.168.1.9:5062;rport;branch=z9hG4bKa",
            "10.10.20.11:40000"
        ),
        (
            "10.10.20.11:40000".parse().unwrap(),
            "SIP/2.0/UDP 192.168.1.9:5062;rport=40000;branch=z9hG4bKa;received=10.10.20.11"
                .to_owned()
        )
    );
}

#[test]
fn only_the_top_via_is_written_in() {
    let mut layer = Layer::new();
    let mut request = theirs(Method::Options, "z9hG4bKa");
    request.headers.set(
        "Via",
        "SIP/2.0/UDP proxy.example.lan;branch=z9hG4bKa;rport, SIP/2.0/UDP 192.168.1.9;branch=z9hG4bKz",
    );
    let (_, request) = arrive(&mut layer, 0, request);
    assert_eq!(
        request.headers.get("Via"),
        Some(
            "SIP/2.0/UDP proxy.example.lan;branch=z9hG4bKa;rport=5060;received=10.10.20.11, \
             SIP/2.0/UDP 192.168.1.9;branch=z9hG4bKz"
        )
    );
}

// ---------------------------------------------------------------------
// Requests outside a transaction.

#[test]
fn a_request_sent_statelessly_has_a_via_and_no_transaction() {
    let mut layer = Layer::new();
    let mut ack = ours(Method::Ack);
    ack.headers.set("CSeq", "7 ACK");
    let datagram = layer.t.send_stateless(ack, peer(), &us(), &mut layer.out);
    assert_eq!(
        layer.out,
        [Output::Send {
            to: peer(),
            datagram: datagram.clone()
        }]
    );
    let Ok(Message::Request(sent)) = Message::parse(&datagram) else {
        panic!("not a request");
    };
    assert!(
        sent.headers
            .top_via()
            .unwrap()
            .branch()
            .unwrap()
            .starts_with(MAGIC_COOKIE)
    );
    assert!(layer.t.is_empty());
    assert_eq!(layer.t.next_deadline(), None);
}
