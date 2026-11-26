// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The registration client (RFC 3261 section 10): keeps one address of
//! record bound to one contact at one registrar.
//!
//! The machine makes REGISTER requests and reads their responses; sending
//! them, through a client transaction, is the caller's.
//!
//! - It registers at once, and again at 90 percent of the time granted.
//! - A 401 or a 407 is answered once with credentials (see
//!   [`crate::DigestSession`]).
//! - A 423 Interval Too Brief is answered once with the registrar's
//!   minimum (section 10.2.8).
//! - Anything else, or no answer, is a failure: it tries again after 5,
//!   10, 20 and 40 seconds and then every 60.
//! - On shutdown the binding is removed with `Expires: 0`.

use std::fmt;

use crate::digest::{Credentials, DigestError, DigestSession, Subject};
use crate::headers::Contacts;
use crate::message::{Method, Request, Response};
use crate::uri::SipUri;

/// The seconds between tries after each failure in a row; the last holds.
pub const BACKOFF_S: [u64; 5] = [5, 10, 20, 40, 60];

/// The registration time asked for when none is configured, in seconds.
pub const DEFAULT_EXPIRES_S: u32 = 300;

/// What a registration is made of.
#[derive(Clone, Debug)]
pub struct RegistrationConfig {
    /// The registrar, the Request-URI: `sip:pbx.example.lan`, with no user
    /// (section 10.2).
    pub registrar: SipUri,
    /// The address of record, the To and the From: `sip:7002@pbx.example.lan`.
    pub aor: SipUri,
    /// Where calls for the address are to be sent: us.
    pub contact: SipUri,
    /// The time to ask for, in seconds.
    pub expires_s: u32,
    pub credentials: Option<Credentials>,
    /// The User-Agent header; empty for none.
    pub user_agent: String,
    /// The Call-ID of every REGISTER of this registration (section 10.2:
    /// all registrations of a client should use the same one), random.
    pub call_id: String,
    /// The tag of our From, random.
    pub from_tag: String,
}

/// Why a registration failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegFailure {
    /// No answer from the registrar.
    Timeout,
    /// The registrar refused the user name or the password, or asked for
    /// credentials and there are none.
    Credentials,
    /// The registrar asked for authentication we cannot give.
    Challenge,
    /// The registrar answered with this status.
    Status(u16),
}

impl fmt::Display for RegFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegFailure::Timeout => write!(f, "the registrar did not answer"),
            RegFailure::Credentials => {
                write!(f, "the registrar refused the user name or the password")
            }
            RegFailure::Challenge => {
                write!(
                    f,
                    "the registrar asked for authentication that is not supported"
                )
            }
            RegFailure::Status(status) => write!(f, "the registrar answered {status}"),
        }
    }
}

/// The state of a registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegState {
    /// Not registered, and not trying: before the first try, or removed.
    Unregistered,
    /// A REGISTER is out and there is no binding.
    Registering,
    /// Bound until `until_ms`.
    Registered { until_ms: u64 },
    /// The last try failed; the next is at `retry_at_ms`.
    Failed {
        reason: RegFailure,
        retry_at_ms: u64,
    },
}

/// One registration.
#[derive(Clone, Debug)]
pub struct Registration {
    config: RegistrationConfig,
    state: RegState,
    sequence: u32,
    digest: Option<DigestSession>,
    /// The time to ask for: the configured one, or the registrar's minimum
    /// after a 423.
    expires_s: u32,
    /// A REGISTER is out.
    in_flight: bool,
    /// The REGISTER that is out removes the binding.
    removing: bool,
    /// The registrar's minimum was taken for the try in progress.
    raised: bool,
    /// When to send the next REGISTER.
    next_at: Option<u64>,
    /// Failures in a row.
    failures: usize,
}

impl Registration {
    /// A registration that registers at the first [`Registration::poll`].
    pub fn new(config: RegistrationConfig) -> Registration {
        let digest = config.credentials.clone().map(DigestSession::new);
        let expires_s = config.expires_s;
        Registration {
            config,
            state: RegState::Unregistered,
            sequence: 0,
            digest,
            expires_s,
            in_flight: false,
            removing: false,
            raised: false,
            next_at: Some(0),
            failures: 0,
        }
    }

    pub fn state(&self) -> RegState {
        self.state
    }

    pub fn config(&self) -> &RegistrationConfig {
        &self.config
    }

    /// The seconds left of the binding, when there is one.
    pub fn expires_in_s(&self, now_ms: u64) -> Option<u64> {
        match self.state {
            RegState::Registered { until_ms } => Some(until_ms.saturating_sub(now_ms) / 1000),
            _ => None,
        }
    }

    /// When [`Registration::poll`] next has something to send.
    pub fn next_deadline(&self) -> Option<u64> {
        if self.in_flight { None } else { self.next_at }
    }

    /// The REGISTER that is due, if one is: the first, a refresh or a new
    /// try. `cnonce` is a fresh random string, used if the request carries
    /// credentials.
    pub fn poll(&mut self, now_ms: u64, cnonce: &str) -> Option<Request> {
        if self.in_flight || self.next_at.is_none_or(|at| now_ms < at) {
            return None;
        }
        self.next_at = None;
        self.raised = false;
        // A binding that ran out while we were failing is gone.
        if matches!(self.state, RegState::Registered { until_ms } if now_ms >= until_ms)
            || !matches!(self.state, RegState::Registered { .. })
        {
            self.state = RegState::Registering;
        }
        Some(self.build(cnonce))
    }

    /// Registers again now: the operator asked, or the configuration
    /// changed. Nothing happens while a REGISTER is out.
    pub fn reregister(&mut self, now_ms: u64) {
        if !self.in_flight {
            self.failures = 0;
            self.next_at = Some(now_ms);
        }
    }

    /// Removes the binding, for a shutdown: the REGISTER with `Expires: 0`
    /// to send, if there is or may be a binding. The registration does
    /// nothing more afterwards until [`Registration::reregister`].
    pub fn unregister(&mut self, cnonce: &str) -> Option<Request> {
        self.next_at = None;
        let bound = self.in_flight || matches!(self.state, RegState::Registered { .. });
        if !bound {
            self.state = RegState::Unregistered;
            return None;
        }
        self.removing = true;
        Some(self.build(cnonce))
    }

    /// The transaction of the REGISTER that was out ended with no answer.
    pub fn on_timeout(&mut self, now_ms: u64) {
        if self.removing {
            self.removed();
        } else {
            self.fail(now_ms, RegFailure::Timeout);
        }
    }

    /// A response to the REGISTER that was out. Returns the REGISTER to
    /// send in its place, when the response asks for one.
    pub fn on_response(
        &mut self,
        now_ms: u64,
        response: &Response,
        cnonce: &str,
    ) -> Option<Request> {
        match response.status {
            100..=199 => None,
            200..=299 => {
                if let Some(digest) = &mut self.digest {
                    digest.accepted();
                }
                if self.removing {
                    self.removed();
                    return None;
                }
                let granted = self.granted(response);
                if granted == 0 {
                    // The registrar says there is no binding.
                    self.fail(now_ms, RegFailure::Status(response.status));
                    return None;
                }
                self.in_flight = false;
                self.failures = 0;
                let granted_ms = u64::from(granted) * 1000;
                self.state = RegState::Registered {
                    until_ms: now_ms + granted_ms,
                };
                // Refresh at 90 percent of the time granted.
                self.next_at = Some(now_ms + granted_ms / 10 * 9);
                None
            }
            401 | 407 => {
                let Some(digest) = &mut self.digest else {
                    self.failed(now_ms, RegFailure::Credentials);
                    return None;
                };
                match digest.challenged(response) {
                    Ok(()) => Some(self.build(cnonce)),
                    Err(DigestError::Rejected) => {
                        self.failed(now_ms, RegFailure::Credentials);
                        None
                    }
                    Err(DigestError::NoChallenge) => {
                        self.failed(now_ms, RegFailure::Challenge);
                        None
                    }
                }
            }
            // Section 10.2.8: the registrar's minimum is in Min-Expires.
            423 if !self.raised && !self.removing => match response.headers.min_expires() {
                Some(minimum) if minimum > self.expires_s => {
                    self.raised = true;
                    self.expires_s = minimum;
                    Some(self.build(cnonce))
                }
                _ => {
                    self.failed(now_ms, RegFailure::Status(423));
                    None
                }
            },
            status => {
                self.failed(now_ms, RegFailure::Status(status));
                None
            }
        }
    }

    /// A failure of the REGISTER that was out, whichever kind it was.
    fn failed(&mut self, now_ms: u64, reason: RegFailure) {
        if self.removing {
            self.removed();
        } else {
            self.fail(now_ms, reason);
        }
    }

    fn fail(&mut self, now_ms: u64, reason: RegFailure) {
        let wait_s = BACKOFF_S[self.failures.min(BACKOFF_S.len() - 1)];
        self.failures += 1;
        self.in_flight = false;
        let retry_at_ms = now_ms + wait_s * 1000;
        self.state = RegState::Failed {
            reason,
            retry_at_ms,
        };
        self.next_at = Some(retry_at_ms);
    }

    fn removed(&mut self) {
        self.in_flight = false;
        self.removing = false;
        self.next_at = None;
        self.state = RegState::Unregistered;
    }

    /// The time the registrar granted (section 10.2.4): the `expires` of
    /// our contact in the response, or its Expires header, or what we
    /// asked for.
    fn granted(&self, response: &Response) -> u32 {
        let ours = match response.headers.contacts() {
            Some(Contacts::List(contacts)) => contacts.into_iter().find(|contact| {
                contact.sip_uri().is_ok_and(|uri| {
                    uri.user == self.config.contact.user
                        && uri.host.eq_ignore_ascii_case(&self.config.contact.host)
                        && uri.port_or_default() == self.config.contact.port_or_default()
                })
            }),
            _ => None,
        };
        ours.and_then(|contact| contact.params.get("expires")?.parse().ok())
            .or(response.headers.expires())
            .unwrap_or(self.expires_s)
    }

    fn build(&mut self, cnonce: &str) -> Request {
        self.in_flight = true;
        self.sequence += 1;
        let mut request = Request::new(Method::Register, &self.config.registrar);
        let aor = &self.config.aor;
        let headers = &mut request.headers;
        headers.push("Max-Forwards", "70");
        headers.push("From", format!("<{aor}>;tag={}", self.config.from_tag));
        headers.push("To", format!("<{aor}>"));
        headers.push("Call-ID", self.config.call_id.clone());
        headers.push("CSeq", format!("{} REGISTER", self.sequence));
        headers.push("Contact", format!("<{}>", self.config.contact));
        let expires = if self.removing { 0 } else { self.expires_s };
        headers.push("Expires", expires.to_string());
        if !self.config.user_agent.is_empty() {
            headers.push("User-Agent", self.config.user_agent.clone());
        }
        if let Some(digest) = &mut self.digest {
            let subject = Subject {
                method: "REGISTER",
                uri: &request.uri,
            };
            if let Some((name, value)) = digest.authorization(subject, cnonce) {
                request.headers.push(name, value);
            }
        }
        request
    }
}
