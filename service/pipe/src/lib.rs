// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The named pipe transport of the control API, both ends of it.
//!
//! - [`PipeListener`] is the service's end: a `ctlsrv::Listener` on
//!   `\\.\pipe\<name>`. The service and the demo service use it.
//! - [`PipeClient`] is a client's end, for a client that reads on one
//!   thread while it writes on another, as the manager does.
//! - [`Event`] and [`wait_any`] are the Win32 event and the wait the
//!   listener is stopped with; the service uses them for its own stop
//!   signal too.
//!
//! This is a crate of its own because the Win32 calls need `unsafe`, which
//! `ctlsrv` and the programs that use the pipe forbid or keep to
//! themselves. Every `unsafe` block here is one Win32 call and says why it
//! is sound. On anything but Windows the crate is empty.

#![cfg_attr(not(windows), forbid(unsafe_code))]

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod event;
#[cfg(windows)]
mod listener;

#[cfg(windows)]
pub use client::PipeClient;
#[cfg(windows)]
pub use event::{Event, MAX_WAIT_HANDLES, wait_any};
#[cfg(windows)]
pub use listener::{PipeConnection, PipeListener, PipeSecurity};
