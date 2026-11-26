// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control API server.
//!
//! A [`Server`] speaks the control API of `ctlproto` to any number of
//! clients. It reaches the pool through a [`PoolLink`], keeps the
//! configuration file, and sends events to the clients that have subscribed
//! through its [`Hub`].
//!
//! The server runs on Tokio: each client has a task that reads its
//! requests and a task that writes what is queued for it. It does not know
//! what carries its connections. A transport is a [`Listener`] that yields
//! [`Connection`]s; the service gives it the named pipe, the tests give it
//! the in-memory duplex of [`memory`], and BLOCKED(phase9): TLS for remote
//! control is one more transport.
//!
//! Nothing here blocks the pool. Publishing an event only puts it on each
//! subscriber's bounded queue; a subscriber that does not keep up loses its
//! oldest `log` events first and is disconnected if it falls further
//! behind, and state events are never dropped for a client that stays.

#![forbid(unsafe_code)]

mod link;
pub mod memory;
mod queue;
mod server;

use std::future::Future;
use std::io;

use ctlproto::PortInfo;
use tokio::io::{AsyncRead, AsyncWrite};

pub use link::{PoolLink, PoolRequests, Request, pool_link};
pub use queue::{Hub, LOG_RING, Limits};
pub use server::{
    ApplyService, Clock, CpuMeter, LogFn, Server, ServerParts, Stop, never_stops, no_cpu_meter,
    restart_to_apply, system_clock,
};

/// Knows the serial ports of the machine. The service looks them up in
/// Windows; the tests and the demo service give a fixed list.
pub trait PortInventory: Send + Sync {
    /// The ports as they are now. The `line` of each is left empty: the
    /// server fills it in from the lines.
    fn ports(&self) -> Vec<PortInfo>;
}

/// A fixed list of ports.
impl PortInventory for Vec<PortInfo> {
    fn ports(&self) -> Vec<PortInfo> {
        self.clone()
    }
}

/// The two directions of a connection. The connection closes when both
/// have been dropped.
pub struct Halves {
    /// Bytes from the client.
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    /// Bytes to the client.
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
}

/// One client's connection.
pub trait Connection: Send + 'static {
    /// Who the client is, for the log.
    fn peer(&self) -> String;

    /// Splits the connection so that one task can read while another
    /// writes.
    fn split(self) -> Halves;
}

/// Something that yields connections.
pub trait Listener: Send {
    /// What it yields.
    type Connection: Connection;

    /// Waits for the next client. `None` means the listener has been shut
    /// down and there will be no more. Dropping the future gives up the
    /// wait and loses no client.
    fn accept(&mut self) -> impl Future<Output = io::Result<Option<Self::Connection>>> + Send;
}
