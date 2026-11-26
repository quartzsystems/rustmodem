// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The memory backend: an in-process port for tests.
//!
//! [`memory_pair`] gives the two ends of one port. [`MemoryBackend`] is the
//! modem's end and implements [`SerialBackend`]. [`MemoryHost`] is a handle
//! that plays the host: it opens and closes the port, writes and reads
//! bytes, drives DTR and RTS, and reads DCD, DSR, CTS and RI.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{ModemSignals, SerialBackend, SerialEvent, WaitHandle};

/// What the two ends share.
#[derive(Debug, Default)]
struct Port {
    /// Bytes the host wrote, not yet read by the modem.
    to_modem: VecDeque<u8>,
    /// Bytes the modem wrote, not yet read by the host.
    to_host: VecDeque<u8>,
    /// The most `to_host` may hold; `None` for no limit.
    to_host_capacity: Option<usize>,
    open: bool,
    dtr: bool,
    rts: bool,
    signals: ModemSignals,
    events: VecDeque<SerialEvent>,
    /// The error every modem-side call returns, while set.
    failure: Option<io::ErrorKind>,
    /// The modem's end still exists.
    backend_alive: bool,
}

/// Makes the two ends of an in-memory port. The port starts closed, with
/// DTR and RTS off, as a COM port nobody has opened.
pub fn memory_pair() -> (MemoryBackend, MemoryHost) {
    let port = Arc::new(Mutex::new(Port {
        backend_alive: true,
        ..Port::default()
    }));
    (
        MemoryBackend {
            port: Arc::clone(&port),
        },
        MemoryHost { port },
    )
}

fn lock(port: &Mutex<Port>) -> MutexGuard<'_, Port> {
    // A test that panicked while holding the lock has already failed; the
    // state is still good enough to report with.
    port.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The modem's end of an in-memory port.
#[derive(Debug)]
pub struct MemoryBackend {
    port: Arc<Mutex<Port>>,
}

impl MemoryBackend {
    fn port(&self) -> io::Result<MutexGuard<'_, Port>> {
        let port = lock(&self.port);
        match port.failure {
            Some(kind) => Err(io::Error::new(kind, "injected failure")),
            None => Ok(port),
        }
    }
}

impl SerialBackend for MemoryBackend {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut port = self.port()?;
        let take = buf.len().min(port.to_modem.len());
        for (slot, byte) in buf.iter_mut().zip(port.to_modem.drain(..take)) {
            *slot = byte;
        }
        Ok(take)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut port = self.port()?;
        let room = match port.to_host_capacity {
            Some(capacity) => capacity.saturating_sub(port.to_host.len()),
            None => data.len(),
        };
        let take = data.len().min(room);
        port.to_host.extend(&data[..take]);
        Ok(take)
    }

    fn set_signals(&mut self, signals: ModemSignals) -> io::Result<()> {
        self.port()?.signals = signals;
        Ok(())
    }

    fn poll(&mut self) -> io::Result<Option<SerialEvent>> {
        Ok(self.port()?.events.pop_front())
    }

    fn wait_handle(&self) -> WaitHandle {
        WaitHandle::NONE
    }
}

impl Drop for MemoryBackend {
    fn drop(&mut self) {
        lock(&self.port).backend_alive = false;
    }
}

/// The host's end of an in-memory port, for tests.
#[derive(Clone, Debug)]
pub struct MemoryHost {
    port: Arc<Mutex<Port>>,
}

impl MemoryHost {
    /// Opens the port as a Windows program does: the open is seen, and DTR
    /// and RTS go on.
    pub fn open(&self) {
        let mut port = lock(&self.port);
        if !port.open {
            port.open = true;
            port.events.push_back(SerialEvent::HostOpened);
        }
        set_dtr(&mut port, true);
        set_rts(&mut port, true);
    }

    /// Closes the port: DTR and RTS go off and the close is seen. Bytes the
    /// host had written and the modem has not read are discarded.
    pub fn close(&self) {
        let mut port = lock(&self.port);
        set_dtr(&mut port, false);
        set_rts(&mut port, false);
        if port.open {
            port.open = false;
            port.events.push_back(SerialEvent::HostClosed);
        }
        port.to_modem.clear();
    }

    /// Drives DTR.
    pub fn set_dtr(&self, level: bool) {
        set_dtr(&mut lock(&self.port), level);
    }

    /// Drives RTS.
    pub fn set_rts(&self, level: bool) {
        set_rts(&mut lock(&self.port), level);
    }

    /// Writes bytes to the modem.
    pub fn write(&self, data: &[u8]) {
        lock(&self.port).to_modem.extend(data);
    }

    /// Reads everything the modem has written.
    pub fn read(&self) -> Vec<u8> {
        lock(&self.port).to_host.drain(..).collect()
    }

    /// Reads up to `most` bytes of what the modem has written.
    pub fn read_some(&self, most: usize) -> Vec<u8> {
        let mut port = lock(&self.port);
        let take = most.min(port.to_host.len());
        port.to_host.drain(..take).collect()
    }

    /// How many bytes the host has written that the modem has not read.
    pub fn unread_by_modem(&self) -> usize {
        lock(&self.port).to_modem.len()
    }

    /// DCD, DSR, CTS and RI as the modem last set them.
    pub fn signals(&self) -> ModemSignals {
        lock(&self.port).signals
    }

    /// Limits how many unread bytes the host side holds, so the modem's
    /// writes come up short. `None` removes the limit.
    pub fn set_capacity(&self, capacity: Option<usize>) {
        lock(&self.port).to_host_capacity = capacity;
    }

    /// Makes every call on the modem's end fail with this kind of error,
    /// until cleared with `None`.
    pub fn inject_failure(&self, kind: Option<io::ErrorKind>) {
        lock(&self.port).failure = kind;
    }

    /// Whether the modem's end of the port still exists.
    pub fn backend_alive(&self) -> bool {
        lock(&self.port).backend_alive
    }
}

fn set_dtr(port: &mut Port, level: bool) {
    if port.dtr != level {
        port.dtr = level;
        port.events.push_back(SerialEvent::Dtr(level));
    }
}

fn set_rts(port: &mut Port, level: bool) {
    if port.rts != level {
        port.rts = level;
        port.events.push_back(SerialEvent::Rts(level));
    }
}
