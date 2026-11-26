// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Dialogs (RFC 3261 section 12): the state two user agents share once an
//! INVITE has been answered with a 2xx, and what is sent inside it.
//!
//! The dialog layer also owns what the transaction layer does not: the
//! 2xx of an INVITE is sent again until its ACK arrives
//! ([`OkRetransmit`], section 13.3.1.4), and the ACK of a 2xx is a request
//! of the dialog, sent outside any transaction (section 13.2.2.4).

use std::net::SocketAddr;

use crate::headers::{Headers, NameAddr};
use crate::message::{Method, Request, Response};
use crate::transaction::{T1_MS, T2_MS, TIMEOUT_MS};
use crate::uri::SipUri;

/// One dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    pub call_id: String,
    pub local_tag: String,
    /// Empty for a far end that sent no tag, as an RFC 2543 one may.
    pub remote_tag: String,
    /// Our From in requests we send: the header value, tag included.
    local: String,
    /// Our To in requests we send: the header value, tag included.
    remote: String,
    /// The sequence number of the last request we sent.
    local_sequence: u32,
    /// The sequence number of the last request the far end sent.
    remote_sequence: Option<u32>,
    /// Where requests go: the far end's Contact.
    remote_target: SipUri,
    /// The proxies requests go through, nearest first.
    route_set: Vec<NameAddr>,
}

/// Why a 2xx or an INVITE does not make a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogError {
    /// A header a dialog is made from is missing or not valid. The name is
    /// the header's.
    Header(&'static str),
    /// The Contact is not a `sip:` URI, so there is nowhere to send to.
    Contact,
}

impl std::fmt::Display for DialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialogError::Header(name) => write!(f, "the {name} header is missing or not valid"),
            DialogError::Contact => write!(f, "the Contact is not a sip: URI"),
        }
    }
}

impl std::error::Error for DialogError {}

fn target(headers: &Headers) -> Result<SipUri, DialogError> {
    headers
        .contact()
        .ok_or(DialogError::Header("Contact"))?
        .sip_uri()
        .map_err(|_| DialogError::Contact)
}

impl Dialog {
    /// The dialog of the caller, from its INVITE and the 2xx (section
    /// 12.1.2).
    pub fn uac(invite: &Request, response: &Response) -> Result<Dialog, DialogError> {
        let sent = invite
            .essentials()
            .map_err(|_| DialogError::Header("CSeq"))?;
        let from = response.headers.from().ok_or(DialogError::Header("From"))?;
        let to = response.headers.to().ok_or(DialogError::Header("To"))?;
        // The route set is the Record-Route of the response, reversed.
        let mut route_set = response
            .headers
            .record_routes()
            .ok_or(DialogError::Header("Record-Route"))?;
        route_set.reverse();
        Ok(Dialog {
            call_id: sent.call_id,
            local_tag: from.tag().unwrap_or_default().to_owned(),
            remote_tag: to.tag().unwrap_or_default().to_owned(),
            local: response.headers.get("From").unwrap_or_default().to_owned(),
            remote: response.headers.get("To").unwrap_or_default().to_owned(),
            local_sequence: sent.cseq.sequence,
            remote_sequence: None,
            remote_target: target(&response.headers)?,
            route_set,
        })
    }

    /// The dialog of the one called, from the INVITE and the 2xx it
    /// answers with (section 12.1.1). The response has our tag in its To.
    pub fn uas(invite: &Request, response: &Response) -> Result<Dialog, DialogError> {
        let received = invite
            .essentials()
            .map_err(|_| DialogError::Header("CSeq"))?;
        let to = response.headers.to().ok_or(DialogError::Header("To"))?;
        let route_set = invite
            .headers
            .record_routes()
            .ok_or(DialogError::Header("Record-Route"))?;
        Ok(Dialog {
            call_id: received.call_id,
            local_tag: to.tag().ok_or(DialogError::Header("To"))?.to_owned(),
            remote_tag: received.from.tag().unwrap_or_default().to_owned(),
            local: response.headers.get("To").unwrap_or_default().to_owned(),
            remote: invite.headers.get("From").unwrap_or_default().to_owned(),
            local_sequence: 0,
            remote_sequence: Some(received.cseq.sequence),
            remote_target: target(&invite.headers)?,
            route_set,
        })
    }

    /// Whether a request that arrived is in this dialog (section 12.2.2):
    /// the Call-ID, our tag in its To and the far end's in its From.
    pub fn has_request(&self, request: &Request) -> bool {
        request.headers.call_id() == Some(self.call_id.as_str())
            && tag_of(request.headers.to()) == self.local_tag
            && tag_of(request.headers.from()) == self.remote_tag
    }

    /// Whether a response that arrived is in this dialog: the Call-ID, our
    /// tag in its From and the far end's in its To.
    pub fn has_response(&self, response: &Response) -> bool {
        response.headers.call_id() == Some(self.call_id.as_str())
            && tag_of(response.headers.from()) == self.local_tag
            && tag_of(response.headers.to()) == self.remote_tag
    }

    /// Takes a request that arrived in the dialog (section 12.2.2). `Err`
    /// is the status to answer with: 500 for a sequence number that went
    /// backwards. A request that refreshes the target, a re-INVITE, moves
    /// it to its Contact.
    pub fn accept(&mut self, request: &Request) -> Result<(), u16> {
        let sequence = request.headers.cseq().ok_or(400u16)?.sequence;
        // An ACK and a CANCEL carry the number of their INVITE.
        if !matches!(request.method, Method::Ack | Method::Cancel) {
            if self.remote_sequence.is_some_and(|last| sequence < last) {
                return Err(500);
            }
            self.remote_sequence = Some(sequence);
        }
        if request.method == Method::Invite
            && let Ok(target) = target(&request.headers)
        {
            self.remote_target = target;
        }
        Ok(())
    }

    /// A new request in the dialog (section 12.2.1.1), with the next
    /// sequence number. The caller adds what the method needs and sends it
    /// to [`Dialog::next_hop`].
    pub fn request(&mut self, method: Method) -> Request {
        self.local_sequence += 1;
        self.build(method, self.local_sequence)
    }

    /// The ACK of a 2xx to the INVITE we sent with this sequence number
    /// (section 13.2.2.4).
    pub fn ack(&self, invite_sequence: u32) -> Request {
        self.build(Method::Ack, invite_sequence)
    }

    /// The sequence number of the last request we sent.
    pub fn local_sequence(&self) -> u32 {
        self.local_sequence
    }

    fn build(&self, method: Method, sequence: u32) -> Request {
        // Section 12.2.1.1: with a route set whose first proxy is a loose
        // router the request names the target and lists the routes; with a
        // strict router first, the request names that router and the
        // target goes last in the routes.
        let loose = self
            .route_set
            .first()
            .is_none_or(|first| first.sip_uri().is_ok_and(|uri| uri.params.has("lr")));
        let (uri, routes): (String, Vec<String>) = if loose {
            (
                self.remote_target.to_string(),
                self.route_set.iter().map(NameAddr::to_string).collect(),
            )
        } else {
            let mut routes: Vec<String> = self.route_set[1..]
                .iter()
                .map(NameAddr::to_string)
                .collect();
            routes.push(format!("<{}>", self.remote_target));
            (self.route_set[0].uri.clone(), routes)
        };
        let mut request = Request {
            method,
            uri,
            headers: Headers::new(),
            body: Vec::new(),
        };
        let headers = &mut request.headers;
        headers.push("Max-Forwards", "70");
        for route in routes {
            headers.push("Route", route);
        }
        headers.push("From", self.local.clone());
        headers.push("To", self.remote.clone());
        headers.push("Call-ID", self.call_id.clone());
        headers.push("CSeq", format!("{sequence} {}", request.method));
        request
    }

    /// Where requests of the dialog are sent: the first proxy of the route
    /// set, or the far end itself (section 12.2.1.1 and section 8.1.2).
    pub fn next_hop(&self) -> SipUri {
        self.route_set
            .first()
            .and_then(|first| first.sip_uri().ok())
            .unwrap_or_else(|| self.remote_target.clone())
    }

    /// The far end's Contact.
    pub fn remote_target(&self) -> &SipUri {
        &self.remote_target
    }
}

fn tag_of(address: Option<NameAddr>) -> String {
    address
        .as_ref()
        .and_then(NameAddr::tag)
        .unwrap_or_default()
        .to_owned()
}

/// What to do about a 2xx that waits for its ACK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OkPoll {
    /// Nothing yet.
    Wait,
    /// Send the 2xx again.
    Resend,
    /// No ACK came in 64 times T1: the call is over, and a BYE is due
    /// (section 13.3.1.4).
    GiveUp,
}

/// The 2xx of an INVITE, sent again until its ACK arrives (section
/// 13.3.1.4): after T1, then at twice the interval each time up to T2, for
/// 64 times T1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OkRetransmit {
    pub datagram: Vec<u8>,
    pub to: SocketAddr,
    next_at: u64,
    interval: u64,
    give_up_at: u64,
}

impl OkRetransmit {
    /// The 2xx was sent at `now_ms`.
    pub fn new(now_ms: u64, datagram: Vec<u8>, to: SocketAddr) -> OkRetransmit {
        OkRetransmit {
            datagram,
            to,
            next_at: now_ms + T1_MS,
            interval: T1_MS,
            give_up_at: now_ms + TIMEOUT_MS,
        }
    }

    pub fn poll(&mut self, now_ms: u64) -> OkPoll {
        if now_ms >= self.give_up_at {
            OkPoll::GiveUp
        } else if now_ms >= self.next_at {
            self.interval = (self.interval * 2).min(T2_MS);
            self.next_at = now_ms + self.interval;
            OkPoll::Resend
        } else {
            OkPoll::Wait
        }
    }

    /// When [`OkRetransmit::poll`] next has something to do.
    pub fn next_deadline(&self) -> u64 {
        self.next_at.min(self.give_up_at)
    }
}
