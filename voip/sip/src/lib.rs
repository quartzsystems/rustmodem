// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! SIP (RFC 3261) for a UDP user agent, sans-I/O.
//!
//! The crate never opens a socket, reads a clock or starts a thread. It is
//! given datagrams and the time and gives back datagrams to send, timers
//! to set and events.
//!
//! This is the message layer: the parser and the builder for requests,
//! responses and `sip:` URIs, and typed access to the headers a user agent
//! reads.

#![forbid(unsafe_code)]

mod headers;
mod message;
mod text;
mod uri;

pub use headers::{Auth, CSeq, Contacts, Header, Headers, NameAddr, Via};
pub use message::{
    Essentials, Invalid, MAX_DATAGRAM, MAX_HEADERS, MAX_LINE, Message, Method, ParseError, Request,
    Response, reason_phrase,
};
pub use text::Params;
pub use uri::{SipUri, UriError};
