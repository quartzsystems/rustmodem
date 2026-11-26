// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A scriptable SIP peer for tests: the far end rustmodem talks to.
//!
//! It is built on `voip/sip` and `voip/sdp`, the same crates the service
//! uses, and is not shipped. [`Peer`] is a user agent a test drives by
//! hand: it can register lines as a registrar would, call, answer, hang
//! up, cancel and challenge with digest. Like the crates under it, it is
//! sans-I/O: it is given datagrams and the time and gives datagrams back.

#![forbid(unsafe_code)]

mod peer;

pub use peer::{Binding, Call, Challenge, Peer, PeerEvent, RegistrarPolicy};
