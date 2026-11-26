// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A scriptable SIP and RTP peer for tests: the far end rustmodem talks
//! to.
//!
//! It is built on `voip/sip`, `voip/sdp` and `voip/rtp`, the same crates
//! the service uses, and is not shipped. It is in layers:
//!
//! - [`Peer`] is a user agent a test drives by hand: it can register
//!   lines as a registrar would, call, answer, hang up, cancel and
//!   challenge with digest. [`MediaStream`] is the RTP of one call, with
//!   a clock of its own and the faults a test asks for. Both are
//!   sans-I/O, like the crates under them: they are given datagrams and
//!   the time and give datagrams back.
//! - [`Agent`] is the two over UDP sockets, one call at a time.
//! - [`script`] runs an agent from a script of one step a line, which is
//!   what the program `rustmodem-sippeer` does.

#![forbid(unsafe_code)]

mod agent;
mod media;
mod peer;
pub mod script;

pub use agent::{Agent, AgentConfig, MediaKind, loopback};
pub use media::{Impairments, MediaReport, MediaStream, Source, pattern_frame};
pub use peer::{Binding, Call, Challenge, Peer, PeerEvent, RegistrarPolicy};
