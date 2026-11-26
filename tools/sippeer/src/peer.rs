// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The peer: a user agent a test drives.
//!
//! Nothing here happens by itself except what a real far end does without
//! being asked: retransmissions, the ACK of a response to its INVITE, the
//! 200 to a BYE and to a CANCEL, and the answer to a REGISTER, which
//! follows the [`RegistrarPolicy`] the test set. Everything else is a
//! method the test calls, and everything that happens is a [`PeerEvent`]
//! the test reads.

use std::collections::VecDeque;
use std::net::SocketAddr;

use sdp::SessionDescription;
use sip::digest::{self, Algorithm};
use sip::{
    Credentials, Dialog, Message, Method, OkPoll, OkRetransmit, Output, Request, Response, SentBy,
    Transactions, TxId,
};

/// Names a call of the peer's to the test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Call(pub u32);

/// A digest challenge the peer makes.
#[derive(Clone, Debug)]
pub struct Challenge {
    pub realm: String,
    /// The user name and the password that answer it.
    pub credentials: Credentials,
    pub algorithm: Algorithm,
    /// Whether `qop=auth` is offered.
    pub qop: bool,
}

/// What the peer does with a REGISTER.
#[derive(Clone, Debug)]
pub enum RegistrarPolicy {
    /// 200, granting this many seconds, or what was asked for.
    Accept { expires: Option<u32> },
    /// 401 until the request carries credentials that answer the
    /// challenge, then 200.
    Challenge {
        challenge: Challenge,
        expires: Option<u32>,
    },
    /// 423 with this minimum for a request that asks for less, else 200.
    IntervalTooBrief { minimum: u32 },
    /// This status.
    Reject(u16),
    /// No answer at all.
    Silent,
}

/// A contact the peer has registered, as a registrar holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The user of the address of record.
    pub user: String,
    /// The contact URI.
    pub contact: String,
    /// The seconds granted.
    pub expires: u32,
    /// Where the REGISTER came from.
    pub from: SocketAddr,
}

/// Something that happened at the peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerEvent {
    /// An INVITE that begins a call arrived. Nothing has been sent for it.
    Incoming { call: Call, request: Request },
    /// A provisional response to an INVITE of ours.
    Progress { call: Call, response: Response },
    /// The 2xx to an INVITE of ours. Its ACK has been sent.
    Answered { call: Call, response: Response },
    /// An INVITE of ours failed with this response. Its ACK has been sent.
    Refused { call: Call, response: Response },
    /// A request of ours got no final response at all.
    TimedOut { call: Call, method: Method },
    /// The ACK of a 2xx of ours arrived.
    Acknowledged { call: Call, request: Request },
    /// A 2xx of ours was never acknowledged. Nothing more is sent.
    NotAcknowledged { call: Call },
    /// A CANCEL arrived for an INVITE we had not answered. The 200 and the
    /// 487 have been sent.
    Cancelled { call: Call },
    /// A BYE arrived. Its 200 has been sent.
    HungUp { call: Call },
    /// A request arrived inside a call that is not one of the above: a
    /// re-INVITE or anything else. Nothing has been sent for it.
    InCall {
        call: Call,
        tx: TxId,
        request: Request,
    },
    /// The final response to a request of ours inside a call that is not
    /// its first INVITE: a BYE or a re-INVITE. For a re-INVITE's 2xx the
    /// ACK has been sent.
    InCallAnswered {
        call: Call,
        method: Method,
        response: Response,
    },
    /// A REGISTER arrived and was answered with this status, by the
    /// policy. A status of 0 is no answer.
    Registered { request: Request, status: u16 },
    /// A request arrived outside any call that the peer does not answer
    /// by itself. Nothing has been sent for it.
    Request { tx: TxId, request: Request },
    /// The final response to a request of ours outside any call.
    Response { tx: TxId, response: Response },
}

struct PeerCall {
    id: Call,
    /// The INVITE that began the call: ours, without its Via, or theirs.
    invite: Request,
    /// Where the other end is.
    at: SocketAddr,
    dialog: Option<Dialog>,
    /// The transaction of their INVITE, until it is answered.
    server_tx: Option<TxId>,
    /// The transactions of requests of ours that are out.
    client_txs: Vec<(TxId, Method)>,
    local_tag: String,
    /// A 2xx of ours that waits for its ACK.
    ok: Option<OkRetransmit>,
    /// The ACK of the last 2xx to an INVITE of ours.
    ack: Option<(SocketAddr, Vec<u8>)>,
}

/// A scriptable user agent.
pub struct Peer {
    /// Our user name, in From and Contact.
    user: String,
    /// The address we say we are at.
    sent_by: SentBy,
    transactions: Transactions,
    calls: Vec<PeerCall>,
    next_call: u32,
    counter: u64,
    seed: u64,
    events: VecDeque<PeerEvent>,
    sent: Vec<(SocketAddr, Vec<u8>)>,
    /// Requests of ours outside any call.
    loose: Vec<TxId>,
    /// What is done with a REGISTER.
    pub registrar: RegistrarPolicy,
    /// What the registrar holds.
    pub bindings: Vec<Binding>,
    /// When set, an INVITE that begins a call is answered 407 until it
    /// carries credentials that answer this.
    pub challenge_invites: Option<Challenge>,
    nonce_count: u64,
    /// When set, the ACK of a 2xx to an INVITE of ours carries this: the
    /// answer to an offer that came in the 2xx.
    pub ack_sdp: Option<SessionDescription>,
}

impl Peer {
    /// A peer that says it is `user` at `host`:`port`. `seed` makes its
    /// tags and branches; two peers of a test take two seeds.
    pub fn new(user: &str, host: &str, port: u16, seed: u64) -> Peer {
        Peer {
            user: user.to_owned(),
            sent_by: SentBy {
                host: host.to_owned(),
                port,
            },
            transactions: Transactions::new(seed),
            calls: Vec::new(),
            next_call: 1,
            counter: 0,
            seed,
            events: VecDeque::new(),
            sent: Vec::new(),
            loose: Vec::new(),
            registrar: RegistrarPolicy::Accept { expires: None },
            bindings: Vec::new(),
            challenge_invites: None,
            nonce_count: 0,
            ack_sdp: None,
        }
    }

    fn token(&mut self) -> String {
        self.counter += 1;
        format!("{:x}-{}", self.seed, self.counter)
    }

    fn contact(&self) -> String {
        format!(
            "<sip:{}@{}:{}>",
            self.user, self.sent_by.host, self.sent_by.port
        )
    }

    fn call(&mut self, call: Call) -> &mut PeerCall {
        self.calls
            .iter_mut()
            .find(|c| c.id == call)
            .expect("the peer has the call")
    }

    /// The datagrams the peer has sent since the last call of this.
    pub fn take_sent(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        std::mem::take(&mut self.sent)
    }

    /// The next thing that happened.
    pub fn event(&mut self) -> Option<PeerEvent> {
        self.events.pop_front()
    }

    /// Everything that has happened since the last look.
    pub fn events(&mut self) -> Vec<PeerEvent> {
        self.events.drain(..).collect()
    }

    /// The far end's Contact of a call that has a dialog.
    pub fn remote_target(&self, call: Call) -> Option<String> {
        self.calls
            .iter()
            .find(|c| c.id == call)
            .and_then(|c| c.dialog.as_ref())
            .map(|dialog| dialog.remote_target().to_string())
    }

    fn run(&mut self, out: Vec<Output>) {
        for output in out {
            self.handle(output);
        }
    }

    // ------------------------------------------------------------------
    // What the test does
    // ------------------------------------------------------------------

    /// Calls `uri` at `to`, with this offer or with none.
    pub fn invite(
        &mut self,
        now_ms: u64,
        to: SocketAddr,
        uri: &str,
        offer: Option<&SessionDescription>,
    ) -> Call {
        let local_tag = self.token();
        let call_id = format!("{}@{}", self.token(), self.sent_by.host);
        let mut request = Request {
            method: Method::Invite,
            uri: uri.to_owned(),
            headers: sip::Headers::new(),
            body: Vec::new(),
        };
        let headers = &mut request.headers;
        headers.push("Max-Forwards", "70");
        headers.push(
            "From",
            format!("<sip:{}@{}>;tag={local_tag}", self.user, self.sent_by.host),
        );
        headers.push("To", format!("<{uri}>"));
        headers.push("Call-ID", call_id);
        headers.push("CSeq", "1 INVITE");
        headers.push("Contact", self.contact());
        if let Some(offer) = offer {
            headers.push("Content-Type", "application/sdp");
            request.body = offer.build().into_bytes();
        }
        self.invite_with(now_ms, to, request)
    }

    /// Calls with an INVITE the test made. It needs From with a tag, To,
    /// Call-ID, CSeq and Contact; the Via is added here.
    pub fn invite_with(&mut self, now_ms: u64, to: SocketAddr, request: Request) -> Call {
        let id = Call(self.next_call);
        self.next_call += 1;
        let local_tag = request
            .headers
            .from()
            .and_then(|from| from.tag().map(str::to_owned))
            .unwrap_or_default();
        let mut out = Vec::new();
        let tx =
            self.transactions
                .send_request(now_ms, request.clone(), to, &self.sent_by, &mut out);
        self.calls.push(PeerCall {
            id,
            invite: request,
            at: to,
            dialog: None,
            server_tx: None,
            client_txs: vec![(tx, Method::Invite)],
            local_tag,
            ok: None,
            ack: None,
        });
        self.run(out);
        id
    }

    /// Cancels an INVITE of ours that has no final response yet.
    pub fn cancel(&mut self, now_ms: u64, call: Call) -> bool {
        let Some(tx) = self
            .call(call)
            .client_txs
            .iter()
            .find(|(_, method)| *method == Method::Invite)
            .map(|(tx, _)| *tx)
        else {
            return false;
        };
        let mut out = Vec::new();
        let cancelled = self.transactions.cancel(now_ms, tx, &mut out);
        self.run(out);
        cancelled
    }

    /// Sends a request inside a call: a BYE, a re-INVITE with this body,
    /// or anything else.
    pub fn in_call(
        &mut self,
        now_ms: u64,
        call: Call,
        method: Method,
        sdp: Option<&SessionDescription>,
    ) {
        let contact = self.contact();
        let sent_by = self.sent_by.clone();
        let peer_call = self.call(call);
        let Some(dialog) = &mut peer_call.dialog else {
            panic!("the call has no dialog to send {method} in");
        };
        let mut request = dialog.request(method.clone());
        if method == Method::Invite {
            request.headers.push("Contact", contact);
        }
        if let Some(sdp) = sdp {
            request.headers.push("Content-Type", "application/sdp");
            request.body = sdp.build().into_bytes();
        }
        let to = peer_call.at;
        let mut out = Vec::new();
        let tx = self
            .transactions
            .send_request(now_ms, request, to, &sent_by, &mut out);
        self.call(call).client_txs.push((tx, method));
        self.run(out);
    }

    /// Hangs up a call that is through.
    pub fn bye(&mut self, now_ms: u64, call: Call) {
        self.in_call(now_ms, call, Method::Bye, None);
    }

    /// Sends a provisional response to their INVITE: 180, or 183.
    pub fn ring(&mut self, now_ms: u64, call: Call, status: u16) {
        let contact = self.contact();
        let peer_call = self.call(call);
        let Some(tx) = peer_call.server_tx else {
            return;
        };
        let mut response = peer_call
            .invite
            .response(status, Some(&peer_call.local_tag.clone()));
        response.headers.push("Contact", contact);
        self.respond(now_ms, tx, &response);
    }

    /// Answers their INVITE with a 200 and this session description. The
    /// 200 is sent again until its ACK comes.
    pub fn answer(&mut self, now_ms: u64, call: Call, sdp: Option<&SessionDescription>) {
        let contact = self.contact();
        let peer_call = self.call(call);
        let Some(tx) = peer_call.server_tx.take() else {
            return;
        };
        let mut response = peer_call
            .invite
            .response(200, Some(&peer_call.local_tag.clone()));
        response.headers.push("Contact", contact);
        if let Some(sdp) = sdp {
            response.headers.push("Content-Type", "application/sdp");
            response.body = sdp.build().into_bytes();
        }
        peer_call.dialog = Dialog::uas(&peer_call.invite, &response).ok();
        if let Some((to, datagram)) = self.respond(now_ms, tx, &response) {
            self.call(call).ok = Some(OkRetransmit::new(now_ms, datagram, to));
        }
    }

    /// Refuses their INVITE with this status.
    pub fn reject(&mut self, now_ms: u64, call: Call, status: u16) {
        let peer_call = self.call(call);
        let Some(tx) = peer_call.server_tx.take() else {
            return;
        };
        let response = peer_call
            .invite
            .response(status, Some(&peer_call.local_tag.clone()));
        self.respond(now_ms, tx, &response);
        self.calls.retain(|c| c.id != call);
    }

    /// Answers a request the peer left to the test: one of
    /// [`PeerEvent::InCall`] or [`PeerEvent::Request`].
    pub fn respond(
        &mut self,
        now_ms: u64,
        tx: TxId,
        response: &Response,
    ) -> Option<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        self.transactions.respond(now_ms, tx, response, &mut out);
        let sent = out.iter().find_map(|output| match output {
            Output::Send { to, datagram } => Some((*to, datagram.clone())),
            _ => None,
        });
        self.run(out);
        sent
    }

    /// Answers a re-INVITE of theirs with a 200 and this description, and
    /// sends it again until its ACK comes.
    pub fn answer_in_call(
        &mut self,
        now_ms: u64,
        call: Call,
        tx: TxId,
        request: &Request,
        sdp: Option<&SessionDescription>,
    ) {
        let mut response = request.response(200, None);
        response.headers.push("Contact", self.contact());
        if let Some(sdp) = sdp {
            response.headers.push("Content-Type", "application/sdp");
            response.body = sdp.build().into_bytes();
        }
        if let Some((to, datagram)) = self.respond(now_ms, tx, &response) {
            self.call(call).ok = Some(OkRetransmit::new(now_ms, datagram, to));
        }
    }

    /// Sends a request outside any call: an OPTIONS, or a method nobody
    /// has heard of. The request needs everything but its Via.
    pub fn request(&mut self, now_ms: u64, to: SocketAddr, request: Request) -> TxId {
        let mut out = Vec::new();
        let tx = self
            .transactions
            .send_request(now_ms, request, to, &self.sent_by, &mut out);
        self.loose.push(tx);
        self.run(out);
        tx
    }

    /// A request outside any call with the headers every request needs.
    pub fn new_request(&mut self, method: Method, uri: &str) -> Request {
        let tag = self.token();
        let call_id = format!("{}@{}", self.token(), self.sent_by.host);
        let mut request = Request {
            method: method.clone(),
            uri: uri.to_owned(),
            headers: sip::Headers::new(),
            body: Vec::new(),
        };
        let headers = &mut request.headers;
        headers.push("Max-Forwards", "70");
        headers.push(
            "From",
            format!("<sip:{}@{}>;tag={tag}", self.user, self.sent_by.host),
        );
        headers.push("To", format!("<{uri}>"));
        headers.push("Call-ID", call_id);
        headers.push("CSeq", format!("1 {method}"));
        request
    }

    // ------------------------------------------------------------------
    // What arrives, and time
    // ------------------------------------------------------------------

    /// A datagram arrived from `from`. One that is not SIP is dropped.
    pub fn receive(&mut self, now_ms: u64, from: SocketAddr, datagram: &[u8]) {
        let Ok(message) = Message::parse(datagram) else {
            return;
        };
        let mut out = Vec::new();
        let _ = self.transactions.receive(now_ms, from, message, &mut out);
        for output in out {
            self.handle_at(now_ms, output);
        }
    }

    /// Runs the timers that are due.
    pub fn poll(&mut self, now_ms: u64) {
        let mut out = Vec::new();
        self.transactions.poll(now_ms, &mut out);
        for output in out {
            self.handle_at(now_ms, output);
        }
        let mut gave_up = Vec::new();
        for call in &mut self.calls {
            match call.ok.as_mut().map(|ok| ok.poll(now_ms)) {
                Some(OkPoll::Resend) => {
                    if let Some(ok) = &call.ok {
                        self.sent.push((ok.to, ok.datagram.clone()));
                    }
                }
                Some(OkPoll::GiveUp) => {
                    call.ok = None;
                    gave_up.push(call.id);
                }
                Some(OkPoll::Wait) | None => {}
            }
        }
        for call in gave_up {
            self.events.push_back(PeerEvent::NotAcknowledged { call });
        }
    }

    fn handle(&mut self, output: Output) {
        // Sending is all that a call of the test's can cause at once.
        if let Output::Send { to, datagram } = output {
            self.sent.push((to, datagram));
        }
    }

    fn handle_at(&mut self, now_ms: u64, output: Output) {
        match output {
            Output::Send { to, datagram } => self.sent.push((to, datagram)),
            Output::Request { tx, from, request } => {
                self.request_arrived(now_ms, tx, from, request)
            }
            Output::Ack { request, .. } => {
                let found = self.calls.iter_mut().find(|call| {
                    call.dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.has_request(&request))
                });
                // An ACK that comes again, because the 2xx went again or
                // the network doubled it, is the same acknowledgement.
                if let Some(call) = found
                    && call.ok.take().is_some()
                {
                    let call = call.id;
                    self.events
                        .push_back(PeerEvent::Acknowledged { call, request });
                }
            }
            Output::Response { tx, response } => self.response_arrived(tx, response),
            Output::StrayResponse { response, .. } => {
                // A 2xx to an INVITE of ours again: its ACK again.
                let ack = self.calls.iter().find_map(|call| {
                    call.dialog
                        .as_ref()
                        .is_some_and(|dialog| dialog.has_response(&response))
                        .then(|| call.ack.clone())
                        .flatten()
                });
                if let (true, Some(ack)) = (response.is_success(), ack) {
                    self.sent.push(ack);
                }
            }
            Output::Timeout { tx } => {
                if let Some(at) = self.loose.iter().position(|loose| *loose == tx) {
                    self.loose.swap_remove(at);
                    return;
                }
                let found = self.calls.iter_mut().find_map(|call| {
                    let at = call.client_txs.iter().position(|(t, _)| *t == tx)?;
                    Some((call.id, call.client_txs.swap_remove(at).1))
                });
                if let Some((call, method)) = found {
                    self.events.push_back(PeerEvent::TimedOut { call, method });
                }
            }
            Output::NoAck { .. } => {}
        }
    }

    fn request_arrived(&mut self, now_ms: u64, tx: TxId, from: SocketAddr, request: Request) {
        let in_call = self
            .calls
            .iter()
            .position(|call| {
                call.dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.has_request(&request))
            })
            .map(|index| self.calls[index].id);

        match (&request.method, in_call) {
            (Method::Register, _) => self.register(now_ms, tx, from, request),
            (Method::Cancel, _) => {
                let invite_tx = self.transactions.invite_to_cancel(&request);
                let call = invite_tx.and_then(|invite_tx| {
                    self.calls
                        .iter()
                        .position(|call| call.server_tx == Some(invite_tx))
                });
                match (call, invite_tx) {
                    (Some(index), Some(invite_tx)) => {
                        self.respond(now_ms, tx, &request.response(200, None));
                        let call = self.calls.remove(index);
                        let terminated = call.invite.response(487, Some(&call.local_tag));
                        self.respond(now_ms, invite_tx, &terminated);
                        self.events
                            .push_back(PeerEvent::Cancelled { call: call.id });
                    }
                    _ => {
                        self.respond(now_ms, tx, &request.response(481, Some("none")));
                    }
                }
            }
            (Method::Bye, Some(call)) => {
                self.respond(now_ms, tx, &request.response(200, None));
                self.calls.retain(|c| c.id != call);
                self.events.push_back(PeerEvent::HungUp { call });
            }
            (_, Some(call)) => {
                if let Some(dialog) = &mut self.call(call).dialog {
                    let _ = dialog.accept(&request);
                }
                self.events
                    .push_back(PeerEvent::InCall { call, tx, request });
            }
            (Method::Invite, None) => self.invite_arrived(now_ms, tx, from, request),
            (_, None) => self.events.push_back(PeerEvent::Request { tx, request }),
        }
    }

    fn invite_arrived(&mut self, now_ms: u64, tx: TxId, from: SocketAddr, request: Request) {
        if let Some(challenge) = self.challenge_invites.clone() {
            let nonce = format!("invite-nonce-{}", self.nonce_count);
            let answered = request
                .headers
                .proxy_authorization()
                .iter()
                .any(|given| digest::verify(&challenge.credentials, given, &nonce, "INVITE"));
            if !answered {
                self.nonce_count += 1;
                let nonce = format!("invite-nonce-{}", self.nonce_count);
                let tag = self.token();
                let mut response = request.response(407, Some(&tag));
                response.headers.push(
                    "Proxy-Authenticate",
                    digest::challenge(
                        &challenge.realm,
                        &nonce,
                        challenge.algorithm,
                        challenge.qop,
                        false,
                    ),
                );
                self.respond(now_ms, tx, &response);
                return;
            }
        }
        let id = Call(self.next_call);
        self.next_call += 1;
        let local_tag = self.token();
        self.calls.push(PeerCall {
            id,
            invite: request.clone(),
            at: from,
            dialog: None,
            server_tx: Some(tx),
            client_txs: Vec::new(),
            local_tag,
            ok: None,
            ack: None,
        });
        self.events
            .push_back(PeerEvent::Incoming { call: id, request });
    }

    fn response_arrived(&mut self, tx: TxId, response: Response) {
        if let Some(at) = self.loose.iter().position(|loose| *loose == tx) {
            if !response.is_provisional() {
                self.loose.swap_remove(at);
                self.events.push_back(PeerEvent::Response { tx, response });
            }
            return;
        }
        let Some(index) = self
            .calls
            .iter()
            .position(|call| call.client_txs.iter().any(|(t, _)| *t == tx))
        else {
            return;
        };
        let id = self.calls[index].id;
        let at = self.calls[index]
            .client_txs
            .iter()
            .position(|(t, _)| *t == tx)
            .unwrap_or(0);
        let method = self.calls[index].client_txs[at].1.clone();
        if response.is_provisional() {
            if method == Method::Invite {
                self.events
                    .push_back(PeerEvent::Progress { call: id, response });
            }
            return;
        }
        self.calls[index].client_txs.swap_remove(at);
        let first_invite = method == Method::Invite && self.calls[index].dialog.is_none();

        if method == Method::Invite && response.is_success() {
            // The ACK of a 2xx is ours to send, outside the transaction.
            let call = &mut self.calls[index];
            let sequence = match &call.dialog {
                None => {
                    call.dialog = Dialog::uac(&call.invite, &response).ok();
                    call.invite.headers.cseq().map_or(1, |cseq| cseq.sequence)
                }
                Some(dialog) => dialog.local_sequence(),
            };
            if let Some(dialog) = &call.dialog {
                let mut ack = dialog.ack(sequence);
                // The answer to an offer that came in the 2xx goes in the
                // ACK (RFC 3264 section 5, RFC 3261 section 13.2.2.4).
                if let Some(answer) = &self.ack_sdp {
                    ack.headers.push("Content-Type", "application/sdp");
                    ack.body = answer.build().into_bytes();
                }
                let to = call.at;
                let mut out = Vec::new();
                let datagram = self
                    .transactions
                    .send_stateless(ack, to, &self.sent_by, &mut out);
                self.calls[index].ack = Some((to, datagram));
                self.run(out);
            }
        }

        let event = if first_invite && response.is_success() {
            PeerEvent::Answered { call: id, response }
        } else if first_invite {
            self.calls.remove(index);
            PeerEvent::Refused { call: id, response }
        } else {
            if method == Method::Bye {
                self.calls.remove(index);
            }
            PeerEvent::InCallAnswered {
                call: id,
                method,
                response,
            }
        };
        self.events.push_back(event);
    }

    /// Answers a REGISTER as the policy says.
    fn register(&mut self, now_ms: u64, tx: TxId, from: SocketAddr, request: Request) {
        let asked = request.headers.expires().unwrap_or(3600);
        let accept = |expires: Option<u32>| expires.map_or(asked, |granted| granted.min(asked));
        let (status, granted, challenge) = match self.registrar.clone() {
            RegistrarPolicy::Accept { expires } => (200, accept(expires), None),
            RegistrarPolicy::Challenge { challenge, expires } => {
                let nonce = format!("register-nonce-{}", self.nonce_count);
                let answered =
                    request.headers.authorization().iter().any(|given| {
                        digest::verify(&challenge.credentials, given, &nonce, "REGISTER")
                    });
                if answered {
                    (200, accept(expires), None)
                } else {
                    // A nonce is good for one registration: the next
                    // REGISTER is challenged again, and told its nonce is
                    // stale if it carried one.
                    let stale = !request.headers.authorization().is_empty();
                    self.nonce_count += 1;
                    let nonce = format!("register-nonce-{}", self.nonce_count);
                    let header = digest::challenge(
                        &challenge.realm,
                        &nonce,
                        challenge.algorithm,
                        challenge.qop,
                        stale,
                    );
                    (401, 0, Some(header))
                }
            }
            RegistrarPolicy::IntervalTooBrief { minimum } if asked != 0 && asked < minimum => {
                (423, minimum, None)
            }
            RegistrarPolicy::IntervalTooBrief { .. } => (200, asked, None),
            RegistrarPolicy::Reject(status) => (status, 0, None),
            RegistrarPolicy::Silent => {
                self.transactions.forget_server(tx);
                self.events
                    .push_back(PeerEvent::Registered { request, status: 0 });
                return;
            }
        };

        let tag = self.token();
        let mut response = request.response(status, Some(&tag));
        match status {
            200 => {
                let user = request
                    .headers
                    .to()
                    .and_then(|to| to.sip_uri().ok())
                    .and_then(|uri| uri.user)
                    .unwrap_or_default();
                if let Some(contact) = request.headers.contact() {
                    self.bindings.retain(|binding| {
                        !(binding.user == user && binding.contact == contact.uri)
                    });
                    if granted > 0 {
                        self.bindings.push(Binding {
                            user,
                            contact: contact.uri.clone(),
                            expires: granted,
                            from,
                        });
                    }
                    response
                        .headers
                        .push("Contact", format!("<{}>;expires={granted}", contact.uri));
                }
            }
            401 => {
                if let Some(header) = challenge {
                    response.headers.push("WWW-Authenticate", header);
                }
            }
            423 => response.headers.push("Min-Expires", granted.to_string()),
            _ => {}
        }
        self.respond(now_ms, tx, &response);
        self.events
            .push_back(PeerEvent::Registered { request, status });
    }
}
