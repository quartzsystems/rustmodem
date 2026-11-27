// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The SerialBackend trait with the memory, com0com and vcom backends.
//!
//! A serial backend is the modem's side of a COM port: the DCE end. The
//! host (Windows, RRAS, a terminal) is on the other side. Everything here
//! is non-blocking; the pool calls in from its one thread and uses
//! [`SerialBackend::wait_handle`] to sleep until something happens.
//!
//! The trait and [`MemoryBackend`] are pure and build everywhere. The
//! com0com backend, and the vcom backend, which is the modem's side of a
//! RustModem virtual COM port, are for Windows only.

#![deny(unsafe_code)]

#[cfg(windows)]
#[allow(unsafe_code)]
mod com0com;
mod memory;
#[cfg(windows)]
#[allow(unsafe_code)]
mod vcom;

use std::io;

use msg::{Message, message};

pub use at::ModemSignals;
#[cfg(windows)]
pub use com0com::Com0comBackend;
pub use memory::{MemoryBackend, MemoryHost, memory_pair};
#[cfg(windows)]
pub use vcom::VcomBackend;

/// Something the host did to the port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SerialEvent {
    /// The host changed DTR.
    Dtr(bool),
    /// The host changed RTS.
    Rts(bool),
    /// The host opened the port.
    HostOpened,
    /// The host closed the port.
    HostClosed,
}

/// Something the pool can wait on until a port has work for it. On Windows
/// it is the raw value of an event handle that is signalled when the host
/// sends data or changes a signal. A backend with nothing to wait on gives
/// [`WaitHandle::NONE`], and the pool then relies on its timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WaitHandle(Option<isize>);

impl WaitHandle {
    /// Nothing to wait on.
    pub const NONE: WaitHandle = WaitHandle(None);

    /// A wait handle from a raw operating system handle value. The backend
    /// keeps ownership of the handle.
    pub fn from_raw(raw: isize) -> WaitHandle {
        WaitHandle(Some(raw))
    }

    /// The raw operating system handle value, if there is one.
    pub fn raw(&self) -> Option<isize> {
        self.0
    }
}

/// The modem's side of a COM port.
pub trait SerialBackend: Send {
    /// Reads bytes the host wrote. Non-blocking; 0 means nothing is there.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Writes bytes to the host. Non-blocking; may take fewer than offered,
    /// or none.
    fn write(&mut self, data: &[u8]) -> io::Result<usize>;

    /// Sets DCD, DSR, CTS and RI toward the host.
    fn set_signals(&mut self, signals: ModemSignals) -> io::Result<()>;

    /// The next thing the host did to the port, if anything.
    fn poll(&mut self) -> io::Result<Option<SerialEvent>>;

    /// Lets the pool sleep until something happens on this port.
    fn wait_handle(&self) -> WaitHandle;
}

/// What a backend was doing when an I/O error happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SerialOperation {
    /// Opening the port.
    Open,
    /// Reading from the host.
    Read,
    /// Writing to the host.
    Write,
    /// Setting the signals toward the host.
    SetSignals,
    /// Reading the signals from the host.
    Poll,
    /// Setting the port up after opening it.
    Configure,
}

/// The `RMD` message for an I/O error on a port.
pub fn error_message(port: &str, operation: SerialOperation, error: &io::Error) -> Message {
    let id = match operation {
        SerialOperation::Open => "RMD3001",
        SerialOperation::Read => "RMD3002",
        SerialOperation::Write => "RMD3003",
        SerialOperation::SetSignals => "RMD3004",
        SerialOperation::Poll => "RMD3005",
        SerialOperation::Configure => "RMD3006",
    };
    message(id, &[port, &error.to_string()])
}

/// The `RMD` message for a backend that this platform does not have.
pub fn unavailable_message(backend: &str) -> Message {
    message("RMD3007", &[backend])
}
