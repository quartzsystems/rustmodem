// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! SIP (RFC 3261) for a UDP user agent, sans-I/O.
//!
//! The crate never opens a socket, reads a clock or starts a thread. It is
//! given datagrams and the time and gives back datagrams to send, timers
//! to set and events. Random values (branches, tags, Call-IDs, client
//! nonces) are given by the caller, so a test is the same every run.
//!
//! - The message layer: the parser and the builder for requests, responses
//!   and `sip:` URIs, and typed access to the headers a user agent reads.
//! - [`transaction`]: the four transaction state machines and their
//!   timers.
//! - [`dialog`]: dialogs, requests inside them, and the 2xx that is sent
//!   again until its ACK.
//! - [`digest`]: digest authentication.
//! - [`register`]: the registration client.

#![forbid(unsafe_code)]

pub mod dialog;
pub mod digest;
mod headers;
mod message;
pub mod register;
mod text;
pub mod transaction;
mod uri;

pub use dialog::{Dialog, DialogError, OkPoll, OkRetransmit};
pub use digest::{Credentials, DigestError, DigestSession};
pub use headers::{Auth, CSeq, Contacts, Header, Headers, NameAddr, Via};
pub use message::{
    Essentials, Invalid, MAX_DATAGRAM, MAX_HEADERS, MAX_LINE, Message, Method, ParseError, Request,
    Response, reason_phrase,
};
pub use register::{RegFailure, RegState, Registration, RegistrationConfig};
pub use text::Params;
pub use transaction::{Output, ReceiveError, SentBy, Transactions, TxId};
pub use uri::{SipUri, UriError};
