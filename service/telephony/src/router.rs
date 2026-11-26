// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The SIP call router: the [`CallRouter`] that stands between the lines
//! and a SIP network.
//!
//! It is sans-I/O. Everything happens in [`CallRouter::poll`], on the
//! pool's thread: the datagrams that have arrived are taken from the
//! [`SipTransport`], the timers are run against the time the pool gives,
//! and what there is to say to the lines is handed back one event at a
//! time. `docs/TELEPHONY.md` describes the behaviour, method by method.

use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;

use config::{Config, RegisterConfig};
use ctlproto::LineState;
use media::{RtpEndpoint, RtpParams, RtpPorts};
use msg::{Message as Rmd, message};
use pool::{
    CallRouter, LineDirectory, LineId, MediaEndpoint, RING_OFF_MS, RING_ON_MS, RouterEvent,
};
use sdp::{LocalEndpoint, Negotiated, SessionDescription};
use sip::digest::Subject;
use sip::{
    Credentials, Dialog, DigestSession, Message, Method, OkPoll, OkRetransmit, Output, RegFailure,
    RegState, Registration, RegistrationConfig, Request, Response, SentBy, SipUri, Transactions,
    TxId,
};

use crate::hunt::{Decision, Hunter};
use crate::settings::{Settings, host_and_port};
use crate::status::{
    Command, Direction, Observer, RegistrationState, RegistrationStatus, SipStatus, TelephonyHandle,
};
use crate::transport::SipTransport;

/// The methods the router takes, as it says in Allow.
pub const ALLOW: &str = "INVITE, ACK, BYE, CANCEL, OPTIONS";

/// Methods that exist and that the router does not take: 405. A method it
/// has never heard of is 501.
const KNOWN_METHODS: [&str; 10] = [
    "REGISTER",
    "SUBSCRIBE",
    "NOTIFY",
    "REFER",
    "UPDATE",
    "PRACK",
    "INFO",
    "MESSAGE",
    "PUBLISH",
    "ACK",
];

/// How long before a name that did not resolve is tried again.
const RESOLVE_RETRY_MS: u64 = 60_000;

/// How often the drop of untrusted requests is logged: the first, and
/// then every this many.
const DROP_LOG_EVERY: u64 = 100;

/// Where a call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Incoming: the line is being rung.
    Ringing,
    /// Incoming: the 200 is sent and its ACK is awaited.
    Answering,
    /// Outgoing: the INVITE is out.
    Calling,
    /// Both ends are through.
    Connected,
    /// Outgoing: the line gave up and the INVITE is being cancelled.
    Cancelling,
    /// A BYE is out.
    Ending,
}

struct Call {
    line: LineId,
    /// The line is done with the call. It goes on only to be cleared on
    /// the network, and nothing more is said to the line about it.
    released: bool,
    phase: Phase,
    /// The INVITE: the one that came, or the one we sent, without its Via.
    invite: Request,
    /// Where our requests go when the dialog does not say otherwise.
    peer: SocketAddrV4,
    /// The transaction of the INVITE that came, until it has its final
    /// response.
    server_tx: Option<TxId>,
    /// The transaction of our INVITE, or of our BYE.
    client_tx: Option<TxId>,
    local_tag: String,
    /// Our Contact header.
    contact: String,
    sent_by: SentBy,
    dialog: Option<Dialog>,
    /// Outgoing through a registrar: what answers its challenge.
    digest: Option<DigestSession>,
    /// The call's RTP socket, until the media runner has it.
    socket: Option<UdpSocket>,
    /// Our end of the stream, as our session descriptions give it.
    local: LocalEndpoint,
    negotiated: Option<Negotiated>,
    /// The far end, the codec and the packet time, shared with the media
    /// thread once the call is connected.
    params: Option<Arc<RtpParams>>,
    /// We offered in the 200 and the ACK brings the answer.
    answer_in_ack: bool,
    ring_since_ms: u64,
    ring_on: bool,
    /// A 2xx of ours that waits for its ACK.
    ok: Option<OkRetransmit>,
    /// The ACK of the 2xx to our INVITE, to send again when the 2xx comes
    /// again.
    ack: Option<(SocketAddr, Vec<u8>)>,
    /// The line hung up before the ACK of our 200: the BYE follows it.
    hangup_pending: bool,
}

/// A line's registration.
struct Reg {
    line: LineId,
    config: RegisterConfig,
    /// The machine, once the registrar's address is known.
    registration: Option<Registration>,
    server: Option<SocketAddrV4>,
    sent_by: Option<SentBy>,
    /// The REGISTER that is out.
    tx: Option<TxId>,
    /// When to look the registrar's name up again.
    resolve_at_ms: u64,
    /// Why there is no registration, when the name does not resolve.
    error: Option<Rmd>,
    /// Why the last try failed, until a try succeeds. While it is set a
    /// new try in flight is still a failed registration to whoever looks:
    /// the state does not flicker at every try.
    last_failure: Option<Rmd>,
    /// The state last told to the observer.
    reported: Option<(RegistrationState, Option<String>)>,
}

/// What a router is made from.
pub struct RouterParts {
    pub settings: Settings,
    pub transport: Box<dyn SipTransport>,
    /// Random numbers: tags, branches, Call-IDs, client nonces and the
    /// first numbers of RTP streams.
    pub random: Box<dyn FnMut() -> u64 + Send>,
    pub observer: Box<dyn Observer>,
    pub handle: TelephonyHandle,
    /// The service's version, for the User-Agent of a configuration that
    /// names none.
    pub version: String,
}

/// The SIP call router.
pub struct SipRouter {
    settings: Settings,
    transport: Box<dyn SipTransport>,
    random: Box<dyn FnMut() -> u64 + Send>,
    observer: Box<dyn Observer>,
    handle: TelephonyHandle,
    version: String,
    transactions: Transactions,
    hunter: Hunter,
    rtp_ports: RtpPorts,
    calls: Vec<Call>,
    registrations: Vec<Reg>,
    /// What there is to say to the lines.
    events: VecDeque<RouterEvent>,
    /// What the transaction layer wants done and has not been done yet.
    outbox: VecDeque<Output>,
    dropped_untrusted: u64,
}

/// A source of random numbers from the system.
pub fn system_random() -> Box<dyn FnMut() -> u64 + Send> {
    Box::new(|| {
        // A system with no random numbers is not one to place calls from;
        // the clock is better than a constant.
        getrandom::u64().unwrap_or_else(|_| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos() as u64)
        })
    })
}

impl SipRouter {
    pub fn new(mut parts: RouterParts) -> SipRouter {
        let seed = (parts.random)();
        let settings = parts.settings;
        let mut router = SipRouter {
            hunter: Hunter::new(&settings.routes, &settings.hunt_groups),
            rtp_ports: RtpPorts::new(settings.rtp_ports.first, settings.rtp_ports.last),
            registrations: Vec::new(),
            settings,
            transport: parts.transport,
            random: parts.random,
            observer: parts.observer,
            handle: parts.handle,
            version: parts.version,
            transactions: Transactions::new(seed),
            calls: Vec::new(),
            events: VecDeque::new(),
            outbox: VecDeque::new(),
            dropped_untrusted: 0,
        };
        router.set_registrations();
        router.publish_status(0);
        router
    }

    // ------------------------------------------------------------------
    // Small things
    // ------------------------------------------------------------------

    fn token(&mut self) -> String {
        format!("{:016x}", (self.random)())
    }

    /// The host we advertise to a peer, and our address for its media.
    fn local(&mut self, peer: SocketAddrV4) -> (String, Ipv4Addr) {
        let routed = self.transport.local_address(peer);
        if self.settings.address.is_empty() {
            return (routed.to_string(), routed);
        }
        let address = self.settings.address.parse().unwrap_or(routed);
        (self.settings.address.clone(), address)
    }

    fn sent_by(&mut self, peer: SocketAddrV4) -> SentBy {
        SentBy {
            host: self.local(peer).0,
            port: self.transport.local_port(),
        }
    }

    /// The line a request is for, when its Request-URI is the contact a
    /// line registered: the extension and the `line` parameter.
    fn registered_line(&self, uri: &SipUri) -> Option<LineId> {
        let line: LineId = uri.params.get("line")?.parse().ok()?;
        self.registrations
            .iter()
            .find(|reg| reg.line == line && uri.user.as_deref() == Some(reg.config.user.as_str()))
            .map(|reg| reg.line)
    }

    /// Whether a request may be taken from this address: from a trusted
    /// peer always, and from a line's registrar when it is for that line.
    fn trusted(&self, from: Ipv4Addr, request: &Request) -> bool {
        if self
            .settings
            .trusted_peers
            .iter()
            .any(|peer| peer.contains(from))
        {
            return true;
        }
        let Some(line) = request
            .sip_uri()
            .ok()
            .and_then(|uri| self.registered_line(&uri))
        else {
            return false;
        };
        self.registrations
            .iter()
            .any(|reg| reg.line == line && reg.server.is_some_and(|server| *server.ip() == from))
    }

    fn say(&mut self, call: usize, event: RouterEvent) {
        if !self.calls[call].released {
            self.events.push_back(event);
        }
    }

    // ------------------------------------------------------------------
    // The transaction layer
    // ------------------------------------------------------------------

    fn send_request(
        &mut self,
        now: u64,
        mut request: Request,
        to: SocketAddrV4,
        sent_by: &SentBy,
    ) -> TxId {
        request
            .headers
            .set("User-Agent", self.settings.user_agent.clone());
        let mut out = Vec::new();
        let tx =
            self.transactions
                .send_request(now, request, SocketAddr::V4(to), sent_by, &mut out);
        self.outbox.extend(out);
        tx
    }

    /// Answers a request. Returns where the response went and the
    /// datagram, for a 2xx that has to be sent again.
    fn respond(
        &mut self,
        now: u64,
        tx: TxId,
        mut response: Response,
    ) -> Option<(SocketAddr, Vec<u8>)> {
        response
            .headers
            .set("Server", self.settings.user_agent.clone());
        let mut out = Vec::new();
        self.transactions.respond(now, tx, &response, &mut out);
        let sent = out.iter().find_map(|output| match output {
            Output::Send { to, datagram } => Some((*to, datagram.clone())),
            _ => None,
        });
        self.outbox.extend(out);
        sent
    }

    /// Answers a request that begins no dialog with a status and nothing
    /// else.
    fn refuse(&mut self, now: u64, tx: TxId, request: &Request, status: u16) {
        let tag = self.token();
        let mut response = request.response(status, Some(&tag));
        if matches!(status, 405 | 501) {
            response.headers.push("Allow", ALLOW);
        }
        self.respond(now, tx, response);
    }

    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) {
        self.observer
            .trace(Direction::Sent, to, &String::from_utf8_lossy(datagram));
        self.transport.send(to, datagram);
    }

    /// Does what the transaction layer wants done, until it wants nothing.
    fn drain(&mut self, now: u64, lines: &dyn LineDirectory) {
        while let Some(output) = self.outbox.pop_front() {
            match output {
                Output::Send { to, datagram } => self.send_datagram(to, &datagram),
                Output::Request { tx, from, request } => {
                    self.request(now, tx, from, request, lines)
                }
                Output::Ack { request, .. } => self.ack(now, &request),
                Output::Response { tx, response } => self.response(now, tx, &response),
                Output::StrayResponse { response, .. } => self.stray_response(&response),
                Output::Timeout { tx } => self.timeout(now, tx),
                // A failure of ours that was never acknowledged: the call
                // it refused is over either way.
                Output::NoAck { .. } => {}
            }
        }
    }

    // ------------------------------------------------------------------
    // What arrives
    // ------------------------------------------------------------------

    fn datagram(&mut self, now: u64, from: SocketAddr, datagram: &[u8]) {
        let SocketAddr::V4(source) = from else {
            return;
        };
        // What does not parse cannot be answered: a keep-alive, or noise.
        let Ok(parsed) = Message::parse(datagram) else {
            return;
        };
        if let Message::Request(request) = &parsed
            && !self.trusted(*source.ip(), request)
        {
            // Dropped without an answer: an answer would tell a scanner
            // that something is here.
            self.dropped_untrusted += 1;
            if self.dropped_untrusted % DROP_LOG_EVERY == 1 {
                let dropped = message(
                    "RMD5010",
                    &[
                        &source.ip().to_string(),
                        &self.dropped_untrusted.to_string(),
                    ],
                );
                self.observer.log(None, &dropped);
            }
            return;
        }
        self.observer.trace(
            Direction::Received,
            from,
            &String::from_utf8_lossy(datagram),
        );
        let mut out = Vec::new();
        // A request without what a transaction is matched by cannot be
        // answered either.
        let _ = self.transactions.receive(now, from, parsed, &mut out);
        self.outbox.extend(out);
    }

    fn request(
        &mut self,
        now: u64,
        tx: TxId,
        from: SocketAddr,
        request: Request,
        lines: &dyn LineDirectory,
    ) {
        let SocketAddr::V4(from) = from else {
            return;
        };
        // No extension is supported, so one that is required is refused
        // (RFC 3261 section 8.2.2.3).
        let required = request.headers.require().join(", ");
        if !required.is_empty() && request.method != Method::Cancel {
            let tag = self.token();
            let mut response = request.response(420, Some(&tag));
            response.headers.push("Unsupported", required);
            self.respond(now, tx, response);
            return;
        }
        let in_dialog = request.headers.to().is_some_and(|to| to.tag().is_some());
        match &request.method {
            Method::Invite if in_dialog => self.reinvite(now, tx, &request),
            Method::Invite => self.invite(now, tx, from, request, lines),
            Method::Cancel => self.cancel(now, tx, &request),
            Method::Bye => self.bye(now, tx, &request),
            Method::Options => {
                let tag = self.token();
                let mut response = request.response(200, Some(&tag));
                response.headers.push("Allow", ALLOW);
                response.headers.push("Accept", "application/sdp");
                self.respond(now, tx, response);
            }
            Method::Register => self.refuse(now, tx, &request, 405),
            Method::Other(name) if KNOWN_METHODS.contains(&name.as_str()) => {
                self.refuse(now, tx, &request, 405);
            }
            Method::Other(_) | Method::Ack => self.refuse(now, tx, &request, 501),
        }
    }

    // ------------------------------------------------------------------
    // An incoming call
    // ------------------------------------------------------------------

    fn invite(
        &mut self,
        now: u64,
        tx: TxId,
        from: SocketAddrV4,
        request: Request,
        lines: &dyn LineDirectory,
    ) {
        let Ok(uri) = request.sip_uri() else {
            return self.refuse(now, tx, &request, 416);
        };
        // The dialog needs somewhere to send its requests.
        if request
            .headers
            .contact()
            .and_then(|contact| contact.sip_uri().ok())
            .is_none()
        {
            return self.refuse(now, tx, &request, 400);
        }
        let called = uri.user_decoded().unwrap_or_default();
        let caller = request
            .headers
            .from()
            .and_then(|from| from.sip_uri().ok())
            .and_then(|uri| uri.user_decoded())
            .unwrap_or_else(|| "anonymous".to_owned());

        // A line the router has a call for is taken, whatever the pool
        // has heard of it yet: two calls that arrive together do not get
        // the same line.
        let for_line = self.registered_line(&uri);
        let calls = &self.calls;
        let state = |line: LineId| {
            if calls.iter().any(|call| call.line == line && !call.released) {
                Some(LineState::Ringing)
            } else {
                lines.state(line)
            }
        };
        let decision = match for_line {
            Some(line) => self.hunter.route_to_line(line, &state),
            None => self.hunter.route(&called, &state),
        };
        let line = match decision {
            Decision::Line(line) => line,
            refused => {
                let status = refused.status().unwrap_or(480);
                return self.turn_away(now, tx, &request, &caller, &called, status);
            }
        };

        // The session description: an offer to answer, or none, and then
        // the offer is ours, in the 200.
        let offer = if request.body.is_empty() {
            None
        } else if request.headers.content_type().as_deref() != Some("application/sdp") {
            return self.turn_away(now, tx, &request, &caller, &called, 415);
        } else {
            match SessionDescription::parse(&request.body) {
                Ok(offer) => Some(offer),
                Err(_) => return self.turn_away(now, tx, &request, &caller, &called, 488),
            }
        };
        let Some((socket, local)) = self.bind_media(from) else {
            return self.turn_away(now, tx, &request, &caller, &called, 503);
        };
        let negotiated = match &offer {
            Some(offer) => match sdp::answer(&self.settings.prefs, &local, offer) {
                Ok((_, negotiated)) => Some(negotiated),
                Err(_) => return self.turn_away(now, tx, &request, &caller, &called, 488),
            },
            None => None,
        };

        let host = self.local(from).0;
        let port = self.transport.local_port();
        let user = uri.user.clone().unwrap_or_default();
        let contact = match for_line {
            Some(line) => format!("<sip:{user}@{host}:{port};line={line}>"),
            None => format!("<sip:{user}@{host}:{port}>"),
        };
        let local_tag = self.token();
        let sent_by = self.sent_by(from);

        self.respond(now, tx, request.response(100, None));
        let mut ringing = request.response(180, Some(&local_tag));
        ringing.headers.push("Contact", contact.clone());
        self.respond(now, tx, ringing);

        self.calls.push(Call {
            line,
            released: false,
            phase: Phase::Ringing,
            invite: request,
            peer: from,
            server_tx: Some(tx),
            client_tx: None,
            local_tag,
            contact,
            sent_by,
            dialog: None,
            digest: None,
            socket: Some(socket),
            local,
            negotiated,
            params: None,
            answer_in_ack: offer.is_none(),
            ring_since_ms: now,
            ring_on: true,
            ok: None,
            ack: None,
            hangup_pending: false,
        });
        self.events
            .push_back(RouterEvent::Incoming { line, from: caller });
        self.events.push_back(RouterEvent::Ring(line));
    }

    /// Refuses a call that was never offered to a line, and says so in
    /// the log.
    fn turn_away(
        &mut self,
        now: u64,
        tx: TxId,
        request: &Request,
        caller: &str,
        called: &str,
        status: u16,
    ) {
        let turned = message(
            "RMD5011",
            &[
                caller,
                called,
                &status.to_string(),
                sip::reason_phrase(status),
            ],
        );
        self.observer.log(None, &turned);
        self.refuse(now, tx, request, status);
    }

    /// A socket for a call's media and our end of the stream as our
    /// session descriptions give it.
    fn bind_media(&mut self, peer: SocketAddrV4) -> Option<(UdpSocket, LocalEndpoint)> {
        let bind_at = *self.settings.listen.ip();
        let socket = match self.rtp_ports.bind(bind_at) {
            Ok(socket) => socket,
            Err(_) => {
                let range = self.settings.rtp_ports.to_string();
                self.observer.log(None, &message("RMD5012", &[&range]));
                return None;
            }
        };
        let port = socket.local_addr().ok()?.port();
        let local = LocalEndpoint {
            address: self.local(peer).1,
            port,
            session_id: (self.random)() >> 1,
            session_version: 1,
        };
        Some((socket, local))
    }

    /// The line went off hook to answer: the 200.
    fn answer_call(&mut self, now: u64, line: LineId) {
        let Some(index) = self
            .calls
            .iter()
            .position(|call| call.line == line && !call.released && call.phase == Phase::Ringing)
        else {
            // Answering when nothing is calling gets nothing.
            self.events.push_back(RouterEvent::Failed(line));
            return;
        };
        let call = &mut self.calls[index];
        let description = match &call.negotiated {
            // What we answer: the codec that was agreed, at the packet
            // time that was agreed, in the direction the offer asks for.
            Some(_) => SessionDescription::parse(&call.invite.body)
                .ok()
                .and_then(|offer| sdp::answer(&self.settings.prefs, &call.local, &offer).ok())
                .map(|(answer, _)| answer),
            None => Some(sdp::offer(&self.settings.prefs, &call.local)),
        };
        let mut ok = call.invite.response(200, Some(&call.local_tag));
        ok.headers.push("Contact", call.contact.clone());
        ok.headers.push("Allow", ALLOW);
        let dialog = Dialog::uas(&call.invite, &ok).ok();
        let (Some(description), Some(dialog), Some(tx)) =
            (description, dialog, call.server_tx.take())
        else {
            // The INVITE was checked when it came, so this is not reached;
            // if it were, the caller is told and the line is let go.
            let tx = self.calls[index].server_tx;
            let invite = self.calls[index].invite.clone();
            if let Some(tx) = tx {
                self.refuse(now, tx, &invite, 500);
            }
            self.events.push_back(RouterEvent::Failed(line));
            self.calls.remove(index);
            return;
        };
        ok.headers.push("Content-Type", "application/sdp");
        ok.body = description.build().into_bytes();
        call.dialog = Some(dialog);
        call.phase = Phase::Answering;
        if std::mem::take(&mut call.ring_on) {
            self.events.push_back(RouterEvent::RingEnd(line));
        }
        let sent = self.respond(now, tx, ok);
        // The 2xx is sent again until its ACK comes (section 13.3.1.4).
        if let Some((to, datagram)) = sent {
            self.calls[index].ok = Some(OkRetransmit::new(now, datagram, to));
        }
    }

    /// The ACK of a 2xx of ours.
    fn ack(&mut self, now: u64, request: &Request) {
        let Some(index) = self.calls.iter().position(|call| {
            call.dialog
                .as_ref()
                .is_some_and(|dialog| dialog.has_request(request))
        }) else {
            return;
        };
        let call = &mut self.calls[index];
        call.ok = None;
        if call.phase != Phase::Answering {
            // The ACK of a re-INVITE's 200, or an ACK that came again.
            return;
        }
        if call.answer_in_ack {
            // We offered in the 200; this is the answer.
            call.negotiated = SessionDescription::parse(&request.body)
                .ok()
                .and_then(|answer| sdp::accept_answer(&self.settings.prefs, &answer).ok());
        }
        let line = call.line;
        if call.hangup_pending {
            self.bye_call(now, index);
            return;
        }
        match self.media_of(index) {
            Some(media) => {
                self.calls[index].phase = Phase::Connected;
                self.say(
                    index,
                    RouterEvent::Connected {
                        line,
                        originate: false,
                        media,
                    },
                );
            }
            None => {
                // The answer in the ACK was no use: there is no stream.
                self.say(index, RouterEvent::Failed(line));
                self.bye_call(now, index);
            }
        }
    }

    /// The media endpoint of a call that is through: its socket and what
    /// was negotiated.
    fn media_of(&mut self, index: usize) -> Option<MediaEndpoint> {
        let negotiated = self.calls[index].negotiated?;
        let socket = self.calls[index].socket.take()?;
        let params = Arc::new(RtpParams::new(
            negotiated.remote,
            negotiated.codec.payload_type(),
            negotiated.ptime_ms,
        ));
        if negotiated.held {
            params.set(
                negotiated.remote,
                negotiated.codec.payload_type(),
                negotiated.ptime_ms,
                true,
            );
        }
        self.calls[index].params = Some(Arc::clone(&params));
        let numbers = (self.random)();
        Some(MediaEndpoint::new(RtpEndpoint {
            socket,
            params,
            timeout_ms: self.settings.rtp_timeout_ms,
            ssrc: (numbers >> 32) as u32,
            sequence: numbers as u16,
            timestamp: (numbers >> 16) as u32,
            call_id: self.calls[index]
                .invite
                .headers
                .call_id()
                .unwrap_or_default()
                .to_owned(),
        }))
    }

    fn cancel(&mut self, now: u64, tx: TxId, request: &Request) {
        let invite_tx = self.transactions.invite_to_cancel(request);
        let index = invite_tx.and_then(|invite_tx| {
            self.calls
                .iter()
                .position(|call| call.server_tx == Some(invite_tx) && call.phase == Phase::Ringing)
        });
        let Some(index) = index else {
            // Nothing to cancel: the INVITE is not there, or it has its
            // final response already (RFC 3261 section 9.2).
            return self.refuse(now, tx, request, 481);
        };
        self.respond(now, tx, request.response(200, None));
        let call = &self.calls[index];
        let (line, tag, ring_on) = (call.line, call.local_tag.clone(), call.ring_on);
        let terminated = call.invite.response(487, Some(&tag));
        if let Some(invite_tx) = invite_tx {
            self.respond(now, invite_tx, terminated);
        }
        if ring_on {
            self.say(index, RouterEvent::RingEnd(line));
        }
        self.say(index, RouterEvent::Abandoned(line));
        self.calls.remove(index);
    }

    // ------------------------------------------------------------------
    // In a call
    // ------------------------------------------------------------------

    fn dialog_of(&self, request: &Request) -> Option<usize> {
        self.calls.iter().position(|call| {
            call.dialog
                .as_ref()
                .is_some_and(|dialog| dialog.has_request(request))
        })
    }

    fn bye(&mut self, now: u64, tx: TxId, request: &Request) {
        let Some(index) = self.dialog_of(request) else {
            return self.refuse(now, tx, request, 481);
        };
        let accepted = self.calls[index]
            .dialog
            .as_mut()
            .map_or(Ok(()), |dialog| dialog.accept(request));
        if let Err(status) = accepted {
            return self
                .respond(now, tx, request.response(status, None))
                .map_or((), |_| ());
        }
        self.respond(now, tx, request.response(200, None));
        let line = self.calls[index].line;
        // A line that is still being connected is told the call failed; one
        // that is in the call, that the far end cleared it.
        let event = match self.calls[index].phase {
            Phase::Connected => RouterEvent::Cleared(line),
            _ => RouterEvent::Failed(line),
        };
        self.say(index, event);
        self.hunter.released(line, now);
        self.calls.remove(index);
    }

    fn reinvite(&mut self, now: u64, tx: TxId, request: &Request) {
        let Some(index) = self.dialog_of(request) else {
            return self.refuse(now, tx, request, 481);
        };
        let accepted = self.calls[index]
            .dialog
            .as_mut()
            .map_or(Ok(()), |dialog| dialog.accept(request));
        if let Err(status) = accepted {
            self.respond(now, tx, request.response(status, None));
            return;
        }
        let call = &mut self.calls[index];
        let mut local = call.local;
        local.session_version += 1;
        let description = if request.body.is_empty() {
            // An offer is asked of us: what we have.
            Some(sdp::offer(&self.settings.prefs, &local))
        } else {
            let offered = SessionDescription::parse(&request.body)
                .ok()
                .and_then(|offer| sdp::answer(&self.settings.prefs, &local, &offer).ok());
            match offered {
                Some((answer, negotiated)) => {
                    // Still a G.711 stream: the media follows it.
                    call.negotiated = Some(negotiated);
                    if let Some(params) = &call.params {
                        params.set(
                            negotiated.remote,
                            negotiated.codec.payload_type(),
                            negotiated.ptime_ms,
                            negotiated.held,
                        );
                    }
                    Some(answer)
                }
                None => None,
            }
        };
        let Some(description) = description else {
            // T.38, or no codec of ours: refused, and the call goes on as
            // it was.
            self.respond(now, tx, request.response(488, None));
            return;
        };
        call.local = local;
        let mut ok = request.response(200, None);
        ok.headers.push("Contact", call.contact.clone());
        ok.headers.push("Allow", ALLOW);
        ok.headers.push("Content-Type", "application/sdp");
        ok.body = description.build().into_bytes();
        if let Some((to, datagram)) = self.respond(now, tx, ok) {
            self.calls[index].ok = Some(OkRetransmit::new(now, datagram, to));
        }
    }

    /// Where a request of a call's dialog goes.
    fn next_hop(&mut self, index: usize) -> SocketAddrV4 {
        let call = &self.calls[index];
        let peer = call.peer;
        let Some(hop) = call.dialog.as_ref().map(Dialog::next_hop) else {
            return peer;
        };
        self.transport
            .resolve(&hop.host, hop.port_or_default())
            .unwrap_or(peer)
    }

    /// Ends a call that has a dialog with a BYE. The line has been told
    /// whatever it is to be told.
    fn bye_call(&mut self, now: u64, index: usize) {
        let to = self.next_hop(index);
        let call = &mut self.calls[index];
        call.released = true;
        call.ok = None;
        call.socket = None;
        let Some(dialog) = &mut call.dialog else {
            self.calls.remove(index);
            return;
        };
        let bye = dialog.request(Method::Bye);
        let (line, sent_by) = (call.line, call.sent_by.clone());
        let tx = self.send_request(now, bye, to, &sent_by);
        let call = &mut self.calls[index];
        call.phase = Phase::Ending;
        call.client_tx = Some(tx);
        self.hunter.released(line, now);
    }

    /// The line went on hook, or can no longer take the call.
    fn hangup_line(&mut self, now: u64, line: LineId) {
        let Some(index) = self
            .calls
            .iter()
            .position(|call| call.line == line && !call.released)
        else {
            return;
        };
        match self.calls[index].phase {
            Phase::Ringing => {
                // Turned away while it rang: 603 Decline.
                let call = self.calls.remove(index);
                if let Some(tx) = call.server_tx {
                    let declined = call.invite.response(603, Some(&call.local_tag));
                    self.respond(now, tx, declined);
                }
            }
            Phase::Answering => {
                // The BYE may not go before the ACK of our 200 has come
                // (RFC 3261 section 15): it follows it.
                let call = &mut self.calls[index];
                call.released = true;
                call.hangup_pending = true;
                call.socket = None;
            }
            Phase::Calling => {
                let call = &mut self.calls[index];
                call.released = true;
                call.phase = Phase::Cancelling;
                call.socket = None;
                if let Some(tx) = call.client_tx {
                    let mut out = Vec::new();
                    self.transactions.cancel(now, tx, &mut out);
                    self.outbox.extend(out);
                }
            }
            Phase::Connected => self.bye_call(now, index),
            Phase::Cancelling | Phase::Ending => {}
        }
    }

    // ------------------------------------------------------------------
    // An outgoing call
    // ------------------------------------------------------------------

    fn start_outgoing(&mut self, now: u64, line: LineId, number: &str) {
        // A line that dials has left whatever call it was in.
        self.hangup_line(now, line);

        // A registered line calls through its registrar, as its own
        // extension; any other, through the outbound peer.
        let registered = self
            .registrations
            .iter()
            .find(|reg| reg.line == line)
            .map(|reg| (reg.config.clone(), reg.server));
        let (target, user, credentials) = match &registered {
            Some((config, _)) => (
                config.server.clone(),
                config.user.clone(),
                credentials_of(config),
            ),
            None if !self.settings.outbound_peer.is_empty() => {
                (self.settings.outbound_peer.clone(), line.to_string(), None)
            }
            None => {
                self.events.push_back(RouterEvent::NoDialtone(line));
                return;
            }
        };
        let Some(digits) = dial_user(number) else {
            self.events.push_back(RouterEvent::Failed(line));
            return;
        };
        let (host, port) = host_and_port(&target);
        let Some(to) = self.transport.resolve(host, port) else {
            self.observer
                .log(Some(line), &message("RMD5014", &[&line.to_string(), host]));
            self.events.push_back(RouterEvent::Failed(line));
            return;
        };
        let Some((socket, local)) = self.bind_media(to) else {
            self.events.push_back(RouterEvent::Failed(line));
            return;
        };

        let advertised = self.local(to).0;
        let sip_port = self.transport.local_port();
        let contact = match &registered {
            Some(_) => format!("<sip:{user}@{advertised}:{sip_port};line={line}>"),
            None => format!("<sip:{user}@{advertised}:{sip_port}>"),
        };
        let local_tag = self.token();
        let call_id = format!("{}@{advertised}", self.token());
        let sent_by = self.sent_by(to);

        let mut invite = Request {
            method: Method::Invite,
            uri: format!("sip:{digits}@{target}"),
            headers: sip::Headers::new(),
            body: sdp::offer(&self.settings.prefs, &local)
                .build()
                .into_bytes(),
        };
        let headers = &mut invite.headers;
        headers.push("Max-Forwards", "70");
        headers.push("From", format!("<sip:{user}@{host}>;tag={local_tag}"));
        headers.push("To", format!("<sip:{digits}@{host}>"));
        headers.push("Call-ID", call_id);
        headers.push("CSeq", "1 INVITE");
        headers.push("Contact", contact.clone());
        headers.push("Allow", ALLOW);
        headers.push("Content-Type", "application/sdp");

        let tx = self.send_request(now, invite.clone(), to, &sent_by);
        self.calls.push(Call {
            line,
            released: false,
            phase: Phase::Calling,
            invite,
            peer: to,
            server_tx: None,
            client_tx: Some(tx),
            local_tag,
            contact,
            sent_by,
            dialog: None,
            digest: credentials.map(DigestSession::new),
            socket: Some(socket),
            local,
            negotiated: None,
            params: None,
            answer_in_ack: false,
            ring_since_ms: now,
            ring_on: false,
            ok: None,
            ack: None,
            hangup_pending: false,
        });
    }

    fn response(&mut self, now: u64, tx: TxId, response: &Response) {
        if let Some(index) = self.registrations.iter().position(|reg| reg.tx == Some(tx)) {
            return self.registration_response(now, index, response);
        }
        let Some(index) = self
            .calls
            .iter()
            .position(|call| call.client_tx == Some(tx))
        else {
            return;
        };
        if response.is_provisional() {
            // 180 and 183 are noted; early media is not listened to.
            return;
        }
        let line = self.calls[index].line;
        match self.calls[index].phase {
            Phase::Calling => self.invite_answered(now, index, response),
            Phase::Cancelling => {
                // The far end answered before the CANCEL reached it: the
                // call it thinks it has is acknowledged and ended.
                if response.is_success() && self.acknowledge(index, response) {
                    self.bye_call(now, index);
                } else {
                    self.calls.remove(index);
                }
            }
            // The answer to our BYE, whatever it is.
            Phase::Ending => {
                self.calls.remove(index);
            }
            Phase::Ringing | Phase::Answering | Phase::Connected => {
                let _ = line;
            }
        }
    }

    /// The final response to our INVITE.
    fn invite_answered(&mut self, now: u64, index: usize, response: &Response) {
        let line = self.calls[index].line;
        match response.status {
            200..=299 => {
                if !self.acknowledge(index, response) {
                    // A 2xx we cannot make a dialog of cannot be
                    // acknowledged or ended either.
                    self.say(index, RouterEvent::Failed(line));
                    self.calls.remove(index);
                    return;
                }
                let call = &mut self.calls[index];
                call.negotiated = SessionDescription::parse(&response.body)
                    .ok()
                    .and_then(|answer| sdp::accept_answer(&self.settings.prefs, &answer).ok());
                match self.media_of(index) {
                    Some(media) => {
                        self.calls[index].phase = Phase::Connected;
                        self.say(
                            index,
                            RouterEvent::Connected {
                                line,
                                originate: true,
                                media,
                            },
                        );
                    }
                    None => {
                        // The answer is no stream we can carry.
                        self.say(index, RouterEvent::Failed(line));
                        self.bye_call(now, index);
                    }
                }
            }
            401 | 407 => {
                let call = &mut self.calls[index];
                let answered = call
                    .digest
                    .as_mut()
                    .is_some_and(|digest| digest.challenged(response).is_ok());
                if !answered {
                    self.say(index, RouterEvent::Failed(line));
                    self.calls.remove(index);
                    return;
                }
                // The INVITE again, with credentials and the next number.
                let cnonce = self.token();
                let call = &mut self.calls[index];
                let sequence = call.invite.headers.cseq().map_or(1, |cseq| cseq.sequence) + 1;
                call.invite
                    .headers
                    .set("CSeq", format!("{sequence} INVITE"));
                call.invite.headers.remove("Authorization");
                call.invite.headers.remove("Proxy-Authorization");
                let subject = Subject {
                    method: "INVITE",
                    uri: &call.invite.uri,
                };
                if let Some((name, value)) = call
                    .digest
                    .as_mut()
                    .and_then(|digest| digest.authorization(subject, &cnonce))
                {
                    call.invite.headers.push(name, value);
                }
                let (invite, to, sent_by) = (call.invite.clone(), call.peer, call.sent_by.clone());
                let tx = self.send_request(now, invite, to, &sent_by);
                self.calls[index].client_tx = Some(tx);
            }
            status => {
                let event = match status {
                    486 | 600 => RouterEvent::Busy(line),
                    480 | 408 => RouterEvent::NoAnswer(line),
                    _ => RouterEvent::Failed(line),
                };
                self.say(index, event);
                self.calls.remove(index);
            }
        }
    }

    /// Makes the dialog of a 2xx to our INVITE and sends its ACK. Returns
    /// whether there is a dialog.
    fn acknowledge(&mut self, index: usize, response: &Response) -> bool {
        let call = &mut self.calls[index];
        let Ok(dialog) = Dialog::uac(&call.invite, response) else {
            return false;
        };
        let sequence = call.invite.headers.cseq().map_or(1, |cseq| cseq.sequence);
        let ack = dialog.ack(sequence);
        call.dialog = Some(dialog);
        let sent_by = call.sent_by.clone();
        let to = SocketAddr::V4(self.next_hop(index));
        let mut out = Vec::new();
        let datagram = self
            .transactions
            .send_stateless(ack, to, &sent_by, &mut out);
        self.outbox.extend(out);
        self.calls[index].ack = Some((to, datagram));
        true
    }

    /// A 2xx to our INVITE that came again: its ACK was lost, and goes
    /// again (RFC 3261 section 13.2.2.4).
    fn stray_response(&mut self, response: &Response) {
        if !response.is_success() {
            return;
        }
        let ack = self.calls.iter().find_map(|call| {
            let dialog = call.dialog.as_ref()?;
            dialog
                .has_response(response)
                .then(|| call.ack.clone())
                .flatten()
        });
        if let Some((to, datagram)) = ack {
            self.send_datagram(to, &datagram);
        }
    }

    fn timeout(&mut self, now: u64, tx: TxId) {
        if let Some(index) = self.registrations.iter().position(|reg| reg.tx == Some(tx)) {
            let reg = &mut self.registrations[index];
            reg.tx = None;
            if let Some(registration) = &mut reg.registration {
                registration.on_timeout(now);
            }
            return self.report_registration(now, index);
        }
        let Some(index) = self
            .calls
            .iter()
            .position(|call| call.client_tx == Some(tx))
        else {
            return;
        };
        // No answer at all to an INVITE is no answer; to a BYE or a
        // CANCEL, the end of it.
        if self.calls[index].phase == Phase::Calling {
            let line = self.calls[index].line;
            self.say(index, RouterEvent::NoAnswer(line));
        }
        self.calls.remove(index);
    }

    // ------------------------------------------------------------------
    // Timers of calls
    // ------------------------------------------------------------------

    fn call_timers(&mut self, now: u64) {
        let mut index = 0;
        while index < self.calls.len() {
            if self.call_timer(now, index) {
                index += 1;
            }
        }
    }

    /// Runs one call's timers. Returns whether the call is still there.
    fn call_timer(&mut self, now: u64, index: usize) -> bool {
        let call = &mut self.calls[index];
        let line = call.line;
        if call.phase == Phase::Ringing {
            let ringing_for = now.saturating_sub(call.ring_since_ms);
            if ringing_for >= self.settings.ring_timeout_ms {
                // Nobody answered: 480, and the line stops ringing.
                let call = self.calls.remove(index);
                if let Some(tx) = call.server_tx {
                    let unavailable = call.invite.response(480, Some(&call.local_tag));
                    self.respond(now, tx, unavailable);
                }
                if call.ring_on {
                    self.events.push_back(RouterEvent::RingEnd(line));
                }
                self.events.push_back(RouterEvent::Abandoned(line));
                return false;
            }
            // The cadence: two seconds on, four off.
            let on = ringing_for % (RING_ON_MS + RING_OFF_MS) < RING_ON_MS;
            if on != call.ring_on {
                call.ring_on = on;
                self.events.push_back(if on {
                    RouterEvent::Ring(line)
                } else {
                    RouterEvent::RingEnd(line)
                });
            }
        }
        let resend = match self.calls[index]
            .ok
            .as_mut()
            .map(|ok| (ok.poll(now), ok.to))
        {
            Some((OkPoll::Resend, to)) => self.calls[index]
                .ok
                .as_ref()
                .map(|ok| (to, ok.datagram.clone())),
            Some((OkPoll::GiveUp, _)) => {
                // No ACK in 64 * T1: the call is ended with a BYE.
                self.observer
                    .log(Some(line), &message("RMD5013", &[&line.to_string()]));
                if self.calls[index].phase == Phase::Answering {
                    self.say(index, RouterEvent::Failed(line));
                }
                self.bye_call(now, index);
                None
            }
            Some((OkPoll::Wait, _)) | None => None,
        };
        if let Some((to, datagram)) = resend {
            self.send_datagram(to, &datagram);
        }
        true
    }

    // ------------------------------------------------------------------
    // Registrations
    // ------------------------------------------------------------------

    /// Makes the registrations match the settings: those of lines that no
    /// longer register, or register otherwise, are removed, and the new
    /// ones are begun.
    fn set_registrations(&mut self) {
        let wanted = self.settings.registrations.clone();
        let mut index = 0;
        while index < self.registrations.len() {
            let reg = &self.registrations[index];
            if wanted
                .iter()
                .any(|(line, config)| *line == reg.line && *config == reg.config)
            {
                index += 1;
                continue;
            }
            self.unregister(0, index);
            self.registrations.remove(index);
        }
        for (line, config) in wanted {
            if self.registrations.iter().any(|reg| reg.line == line) {
                continue;
            }
            self.registrations.push(Reg {
                line,
                config,
                registration: None,
                server: None,
                sent_by: None,
                tx: None,
                resolve_at_ms: 0,
                error: None,
                last_failure: None,
                reported: None,
            });
        }
        self.registrations.sort_by_key(|reg| reg.line);
    }

    /// Sends the REGISTER that removes a line's binding, if it has one.
    fn unregister(&mut self, now: u64, index: usize) {
        let cnonce = self.token();
        let reg = &mut self.registrations[index];
        let removal = reg
            .registration
            .as_mut()
            .and_then(|registration| registration.unregister(&cnonce));
        match (removal, reg.server, reg.sent_by.clone()) {
            (Some(request), Some(server), Some(sent_by)) => {
                let tx = self.send_request(now, request, server, &sent_by);
                self.registrations[index].tx = Some(tx);
            }
            _ => reg.tx = None,
        }
    }

    fn registration_timers(&mut self, now: u64) {
        for index in 0..self.registrations.len() {
            if self.registrations[index].registration.is_none()
                && now >= self.registrations[index].resolve_at_ms
            {
                self.begin_registration(now, index);
            }
            if self.registrations[index].tx.is_none() {
                let cnonce = self.token();
                let reg = &mut self.registrations[index];
                let due = reg
                    .registration
                    .as_mut()
                    .and_then(|registration| registration.poll(now, &cnonce));
                if let (Some(request), Some(server), Some(sent_by)) =
                    (due, reg.server, reg.sent_by.clone())
                {
                    let tx = self.send_request(now, request, server, &sent_by);
                    self.registrations[index].tx = Some(tx);
                }
            }
            self.report_registration(now, index);
        }
    }

    /// Finds the registrar and makes the registration machine.
    fn begin_registration(&mut self, now: u64, index: usize) {
        let config = self.registrations[index].config.clone();
        let line = self.registrations[index].line;
        let (host, port) = host_and_port(&config.server);
        let Some(server) = self.transport.resolve(host, port) else {
            let reg = &mut self.registrations[index];
            reg.resolve_at_ms = now + RESOLVE_RETRY_MS;
            reg.error = Some(message("RMD5008", &[&line.to_string(), host]));
            return;
        };
        let advertised = self.local(server).0;
        let sip_port = self.transport.local_port();
        let mut contact = SipUri::new(Some(&config.user), &advertised, Some(sip_port));
        contact.params.set("line", Some(&line.to_string()));
        let registrar = SipUri::new(None, host, (port != 5060).then_some(port));
        let registration = Registration::new(RegistrationConfig {
            registrar,
            aor: SipUri::new(Some(&config.user), host, None),
            contact,
            expires_s: config.expires_s,
            credentials: credentials_of(&config),
            user_agent: String::new(),
            call_id: format!("{}@{advertised}", self.token()),
            from_tag: self.token(),
        });
        let sent_by = self.sent_by(server);
        let reg = &mut self.registrations[index];
        reg.registration = Some(registration);
        reg.server = Some(server);
        reg.sent_by = Some(sent_by);
        reg.error = None;
    }

    fn registration_response(&mut self, now: u64, index: usize, response: &Response) {
        let cnonce = self.token();
        let reg = &mut self.registrations[index];
        if !response.is_provisional() {
            reg.tx = None;
        }
        let next = reg
            .registration
            .as_mut()
            .and_then(|registration| registration.on_response(now, response, &cnonce));
        if let (Some(request), Some(server), Some(sent_by)) =
            (next, reg.server, reg.sent_by.clone())
        {
            let tx = self.send_request(now, request, server, &sent_by);
            self.registrations[index].tx = Some(tx);
        }
        self.report_registration(now, index);
    }

    fn registration_status(&self, now: u64, index: usize) -> RegistrationStatus {
        let reg = &self.registrations[index];
        let line = reg.line.to_string();
        let server = reg.config.server.as_str();
        let (state, error) = match reg.registration.as_ref().map(Registration::state) {
            None if reg.error.is_some() => (RegistrationState::Failed, reg.error.clone()),
            None | Some(RegState::Registering) => match &reg.last_failure {
                Some(failure) => (RegistrationState::Failed, Some(failure.clone())),
                None => (RegistrationState::Registering, None),
            },
            Some(RegState::Unregistered) => (RegistrationState::Unregistered, None),
            Some(RegState::Registered { .. }) => (RegistrationState::Registered, None),
            Some(RegState::Failed { reason, .. }) => {
                let error = match reason {
                    RegFailure::Timeout => message("RMD5004", &[&line, server]),
                    RegFailure::Credentials => message("RMD5005", &[&line, server]),
                    RegFailure::Challenge => message("RMD5006", &[&line, server]),
                    RegFailure::Status(status) => {
                        message("RMD5007", &[&line, server, &status.to_string()])
                    }
                };
                (RegistrationState::Failed, Some(error))
            }
        };
        RegistrationStatus {
            line: reg.line,
            server: reg.config.server.clone(),
            user: reg.config.user.clone(),
            state,
            expires_in_s: reg
                .registration
                .as_ref()
                .and_then(|registration| registration.expires_in_s(now)),
            error,
        }
    }

    /// Tells the observer when a registration has changed state.
    fn report_registration(&mut self, now: u64, index: usize) {
        let status = self.registration_status(now, index);
        let now_reported = (
            status.state,
            status.error.as_ref().map(|error| error.id.clone()),
        );
        let reg = &mut self.registrations[index];
        match status.state {
            RegistrationState::Failed => reg.last_failure = status.error.clone(),
            RegistrationState::Registered | RegistrationState::Unregistered => {
                reg.last_failure = None;
            }
            RegistrationState::Registering => {}
        }
        if reg.reported.as_ref() == Some(&now_reported) {
            return;
        }
        let before = reg.reported.replace(now_reported);
        let line = status.line.to_string();
        let said = match status.state {
            RegistrationState::Registered => Some(message(
                "RMD5003",
                &[
                    &line,
                    &status.server,
                    &status.expires_in_s.unwrap_or(0).to_string(),
                ],
            )),
            RegistrationState::Failed => status.error.clone(),
            // Said only of a registration that had got somewhere.
            RegistrationState::Unregistered if before.is_some() => {
                Some(message("RMD5009", &[&line, &status.server]))
            }
            RegistrationState::Unregistered | RegistrationState::Registering => None,
        };
        if let Some(said) = said {
            self.observer.log(Some(status.line), &said);
        }
        self.observer.registration(&status);
        self.events
            .push_back(RouterEvent::Registration(status.line));
    }

    /// A registration as the control API reports it.
    fn registration_info(status: &RegistrationStatus) -> ctlproto::Registration {
        ctlproto::Registration {
            state: match status.state {
                RegistrationState::Unregistered => ctlproto::RegistrationState::Unregistered,
                RegistrationState::Registering => ctlproto::RegistrationState::Registering,
                RegistrationState::Registered => ctlproto::RegistrationState::Registered,
                RegistrationState::Failed => ctlproto::RegistrationState::Failed,
            },
            expires_in_s: status.expires_in_s,
            error: status.error.as_ref().map(ctlproto::ErrorInfo::from),
        }
    }

    /// Asks the registration of `line`, or of every line, to be made
    /// again now.
    fn register_again(&mut self, now: u64, line: Option<LineId>) {
        for reg in &mut self.registrations {
            if line.is_none_or(|line| line == reg.line) {
                reg.resolve_at_ms = now;
                if let Some(registration) = &mut reg.registration {
                    registration.reregister(now);
                }
            }
        }
    }

    fn publish_status(&mut self, now: u64) {
        let status = SipStatus {
            listen: self.settings.listen,
            trusted_peers: self
                .settings
                .trusted_peers
                .iter()
                .map(ToString::to_string)
                .collect(),
            outbound_peer: self.settings.outbound_peer.clone(),
            registrations: (0..self.registrations.len())
                .map(|index| self.registration_status(now, index))
                .collect(),
            dropped_untrusted: self.dropped_untrusted,
            calls: self.calls.len(),
        };
        self.handle.publish(status);
    }

    /// One turn: what was asked from outside, what has arrived, and what
    /// is due.
    fn work(&mut self, now: u64, lines: &dyn LineDirectory) {
        while let Some(command) = self.handle.take_command() {
            match command {
                Command::Reregister(line) => self.register_again(now, line),
            }
        }
        while let Some((from, datagram)) = self.transport.receive() {
            self.datagram(now, from, &datagram);
            self.drain(now, lines);
        }
        let mut out = Vec::new();
        self.transactions.poll(now, &mut out);
        self.outbox.extend(out);
        self.drain(now, lines);
        self.registration_timers(now);
        self.call_timers(now);
        self.drain(now, lines);
        self.publish_status(now);
    }
}

impl CallRouter for SipRouter {
    fn dial(&mut self, now_ms: u64, from: LineId, number: &str, lines: &dyn LineDirectory) {
        self.start_outgoing(now_ms, from, number);
        self.drain(now_ms, lines);
    }

    fn answer(&mut self, now_ms: u64, line: LineId) {
        self.answer_call(now_ms, line);
        self.flush(now_ms);
    }

    fn hangup(&mut self, now_ms: u64, line: LineId) {
        self.hangup_line(now_ms, line);
        self.flush(now_ms);
    }

    fn poll(&mut self, now_ms: u64, lines: &dyn LineDirectory) -> Option<RouterEvent> {
        // What is waiting to be said is said before more is taken in, so
        // that the pool hears of things in the order they happened.
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }
        self.work(now_ms, lines);
        self.events.pop_front()
    }

    fn configure(&mut self, now_ms: u64, config: &Config) {
        let settings = Settings::new(config, &self.version);
        if settings.rtp_ports != self.settings.rtp_ports {
            self.rtp_ports = RtpPorts::new(settings.rtp_ports.first, settings.rtp_ports.last);
        }
        self.hunter
            .reconfigure(&settings.routes, &settings.hunt_groups);
        self.settings = settings;
        self.set_registrations();
        self.flush(now_ms);
    }

    fn shutdown(&mut self, now_ms: u64) {
        let lines: Vec<LineId> = self
            .calls
            .iter()
            .filter(|call| !call.released)
            .map(|call| call.line)
            .collect();
        for line in lines {
            self.hangup_line(now_ms, line);
        }
        for index in 0..self.registrations.len() {
            self.unregister(now_ms, index);
        }
        self.flush(now_ms);
    }

    fn busy(&self) -> bool {
        self.calls.iter().any(|call| call.client_tx.is_some())
            || self.registrations.iter().any(|reg| reg.tx.is_some())
    }

    fn registration(&self, now_ms: u64, line: LineId) -> Option<ctlproto::Registration> {
        let index = self.registrations.iter().position(|reg| reg.line == line)?;
        Some(Self::registration_info(
            &self.registration_status(now_ms, index),
        ))
    }

    fn sip_status(&self, now_ms: u64) -> Option<ctlproto::SipStatus> {
        let mut registrations: Vec<ctlproto::SipRegistration> = (0..self.registrations.len())
            .map(|index| {
                let status = self.registration_status(now_ms, index);
                ctlproto::SipRegistration {
                    line: status.line,
                    registration: Self::registration_info(&status),
                    server: status.server,
                    user: status.user,
                }
            })
            .collect();
        registrations.sort_by_key(|registration| registration.line);
        Some(ctlproto::SipStatus {
            listener: ctlproto::ListenerState::Listening,
            listen: SocketAddrV4::new(*self.settings.listen.ip(), self.transport.local_port())
                .to_string(),
            trusted_peers: self
                .settings
                .trusted_peers
                .iter()
                .map(ToString::to_string)
                .collect(),
            outbound_peer: self.settings.outbound_peer.clone(),
            registrations,
            dropped_untrusted: self.dropped_untrusted,
            calls: self.calls.len() as u32,
        })
    }

    fn reregister(&mut self, now_ms: u64, line: Option<LineId>) {
        self.register_again(now_ms, line);
    }
}

impl SipRouter {
    /// Sends what is waiting to be sent. Used where the lines cannot be
    /// asked about: nothing that arrives is taken here, so no call is
    /// routed.
    fn flush(&mut self, _now: u64) {
        let mut later = VecDeque::new();
        while let Some(output) = self.outbox.pop_front() {
            match output {
                Output::Send { to, datagram } => self.send_datagram(to, &datagram),
                other => later.push_back(other),
            }
        }
        self.outbox = later;
    }
}

/// The credentials a line's registration answers a challenge with. A line
/// with no password has none.
fn credentials_of(config: &RegisterConfig) -> Option<Credentials> {
    (!config.password.is_empty()).then(|| Credentials {
        username: if config.auth_user.is_empty() {
            config.user.clone()
        } else {
            config.auth_user.clone()
        },
        password: config.password.clone(),
    })
}

/// The user part of the Request-URI for a number a host dialled: the
/// digits, `*` and `+` as they are, `#` escaped, and the pauses and the
/// waits for dial tone of a dial string left out. `None` when nothing is
/// left.
fn dial_user(number: &str) -> Option<String> {
    let mut user = String::new();
    for c in number.chars() {
        match c {
            '0'..='9' | '*' | '+' => user.push(c),
            '#' => user.push_str("%23"),
            _ => {}
        }
    }
    (!user.is_empty()).then_some(user)
}
