// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The telephone side of the service: SIP.
//!
//! - [`hunt`]: which line an incoming call is offered to, from the routes,
//!   the hunt groups and the state of the lines. Pure.
//! - [`SipRouter`]: the call router that takes the place of the null
//!   switch when SIP is enabled. It accepts calls on the trunk listener
//!   and for registered lines, places the calls the lines dial, and
//!   registers lines to a PBX. Sans-I/O: it runs on the pool's task and
//!   is given datagrams and the time.
//! - [`SipTransport`]: where the router's datagrams come from and go to.
//!   The socket itself is the service's, in `service/svc`: this crate
//!   opens none and has no runtime.
//!
//! `docs/TELEPHONY.md` is the reference for what the router does.

#![forbid(unsafe_code)]

pub mod hunt;
mod router;
mod settings;
mod status;
mod transport;

pub use hunt::{Decision, Hunter};
pub use router::{ALLOW, RouterParts, SipRouter, system_random};
pub use settings::{Settings, host_and_port};
pub use status::{
    Direction, Observer, RegistrationState, RegistrationStatus, SipStatus, TelephonyHandle,
    Unobserved,
};
pub use transport::SipTransport;
