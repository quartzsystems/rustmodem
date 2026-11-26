// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control API server.
//!
//! A [`Server`] speaks the control API of `ctlproto` to any number of
//! clients. It reaches the pool through a `PoolHandle`, keeps the
//! configuration file, and sends events to the clients that have subscribed
//! through its [`Hub`].
//!
//! The server does not know what carries its connections. A transport is a
//! [`Listener`] that yields [`Connection`]s; the service gives it the named
//! pipe, the tests give it the in-memory duplex of [`memory`], and
//! BLOCKED(phase9): TLS for remote control is one more transport.
//!
//! Nothing here blocks the pool. Publishing an event only puts it on each
//! subscriber's bounded queue; a subscriber that does not keep up loses its
//! oldest `log` events first and is disconnected if it falls further
//! behind, and state events are never dropped for a client that stays.

#![forbid(unsafe_code)]

pub mod memory;
mod queue;
mod server;

use std::io::{self, Read, Write};
use std::sync::Arc;

pub use queue::{Hub, Limits};
pub use server::{LogFn, Server, ServerParts};

/// Closes a connection from another thread, so that a reader or writer
/// blocked on it returns. `close` must not block: the pool's thread calls
/// it when it disconnects a client that has fallen too far behind.
pub trait Closer: Send + Sync {
    /// Closes the connection. Closing twice is harmless.
    fn close(&self);
}

/// The two directions of a connection, and the means to close it.
pub struct Halves {
    /// Bytes from the client.
    pub reader: Box<dyn Read + Send>,
    /// Bytes to the client.
    pub writer: Box<dyn Write + Send>,
    /// Closes both directions.
    pub closer: Arc<dyn Closer>,
}

/// One client's connection.
pub trait Connection: Send {
    /// Who the client is, for the log.
    fn peer(&self) -> String;

    /// Splits the connection so that one thread can read while another
    /// writes.
    fn split(self: Box<Self>) -> Halves;
}

/// Something that yields connections.
pub trait Listener: Send {
    /// Waits for the next client. `None` means the listener has been shut
    /// down and there will be no more.
    fn accept(&mut self) -> io::Result<Option<Box<dyn Connection>>>;
}
