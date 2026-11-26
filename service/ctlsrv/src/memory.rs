// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! An in-memory transport: a duplex of two byte pipes, and a listener that
//! hands out the server ends. The tests run the server over it.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{Closer, Connection, Halves, Listener};

/// How many bytes a pipe holds before a writer has to wait, by default.
pub const DEFAULT_CAPACITY: usize = 64 * 1024;

#[derive(Default)]
struct PipeState {
    data: VecDeque<u8>,
    closed: bool,
}

/// One direction: bytes written at one end are read at the other.
struct Pipe {
    state: Mutex<PipeState>,
    changed: Condvar,
    capacity: usize,
}

impl Pipe {
    fn new(capacity: usize) -> Arc<Pipe> {
        Arc::new(Pipe {
            state: Mutex::new(PipeState::default()),
            changed: Condvar::new(),
            capacity: capacity.max(1),
        })
    }

    fn lock(&self) -> MutexGuard<'_, PipeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }
}

/// The reading end of a pipe. Reads wait for data; once the pipe is closed
/// and empty they return 0.
pub struct PipeReader {
    pipe: Arc<Pipe>,
    timeout: Option<Duration>,
}

impl PipeReader {
    /// Makes reads that find nothing give up after this long with a
    /// `TimedOut` error, so that a test waiting for something that never
    /// comes fails instead of hanging.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = self.timeout.map(|timeout| Instant::now() + timeout);
        let mut state = self.pipe.lock();
        loop {
            if !state.data.is_empty() {
                let take = buf.len().min(state.data.len());
                for (slot, byte) in buf.iter_mut().zip(state.data.drain(..take)) {
                    *slot = byte;
                }
                self.pipe.changed.notify_all();
                return Ok(take);
            }
            if state.closed {
                return Ok(0);
            }
            state = match deadline {
                None => self
                    .pipe
                    .changed
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner()),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    self.pipe
                        .changed
                        .wait_timeout(state, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
            };
        }
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        self.pipe.close();
    }
}

/// The writing end of a pipe. Writes wait for room; once the pipe is
/// closed they fail.
pub struct PipeWriter(Arc<Pipe>);

impl Write for PipeWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut state = self.0.lock();
        loop {
            if state.closed {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let room = self.0.capacity.saturating_sub(state.data.len());
            if room > 0 {
                let take = data.len().min(room);
                state.data.extend(&data[..take]);
                self.0.changed.notify_all();
                return Ok(take);
            }
            state = self
                .0
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The client's end of an in-memory connection.
pub struct MemoryClient {
    /// Bytes from the server.
    pub reader: PipeReader,
    /// Bytes to the server.
    pub writer: PipeWriter,
}

/// The server's end of an in-memory connection.
pub struct MemoryConnection {
    reader: PipeReader,
    writer: PipeWriter,
}

struct MemoryCloser {
    to_server: Arc<Pipe>,
    to_client: Arc<Pipe>,
}

impl Closer for MemoryCloser {
    fn close(&self) {
        self.to_server.close();
        self.to_client.close();
    }
}

impl Connection for MemoryConnection {
    fn peer(&self) -> String {
        "memory".to_string()
    }

    fn split(self: Box<Self>) -> Halves {
        let closer = Arc::new(MemoryCloser {
            to_server: Arc::clone(&self.reader.pipe),
            to_client: Arc::clone(&self.writer.0),
        });
        Halves {
            reader: Box::new(self.reader),
            writer: Box::new(self.writer),
            closer,
        }
    }
}

/// Makes a connection whose pipes hold `capacity` bytes each way.
pub fn duplex(capacity: usize) -> (MemoryClient, MemoryConnection) {
    let to_server = Pipe::new(capacity);
    let to_client = Pipe::new(capacity);
    (
        MemoryClient {
            reader: PipeReader {
                pipe: Arc::clone(&to_client),
                timeout: None,
            },
            writer: PipeWriter(Arc::clone(&to_server)),
        },
        MemoryConnection {
            reader: PipeReader {
                pipe: to_server,
                timeout: None,
            },
            writer: PipeWriter(to_client),
        },
    )
}

/// The listening side: yields the server end of each connection made
/// through the connector.
pub struct MemoryListener {
    incoming: Receiver<MemoryConnection>,
}

/// The connecting side. When every connector has been dropped the listener
/// reports that it has shut down.
#[derive(Clone)]
pub struct MemoryConnector {
    outgoing: Sender<MemoryConnection>,
}

/// Makes a listener and the connector that reaches it.
pub fn listener() -> (MemoryListener, MemoryConnector) {
    let (outgoing, incoming) = mpsc::channel();
    (MemoryListener { incoming }, MemoryConnector { outgoing })
}

impl Listener for MemoryListener {
    fn accept(&mut self) -> io::Result<Option<Box<dyn Connection>>> {
        match self.incoming.recv() {
            Ok(connection) => Ok(Some(Box::new(connection))),
            Err(_) => Ok(None),
        }
    }
}

impl MemoryConnector {
    /// Connects with pipes of the default capacity.
    pub fn connect(&self) -> io::Result<MemoryClient> {
        self.connect_with_capacity(DEFAULT_CAPACITY)
    }

    /// Connects with pipes that hold `capacity` bytes each way. A small
    /// capacity and a client that does not read make a slow client.
    pub fn connect_with_capacity(&self, capacity: usize) -> io::Result<MemoryClient> {
        let (client, connection) = duplex(capacity);
        self.outgoing
            .send(connection)
            .map_err(|_| io::Error::from(io::ErrorKind::ConnectionRefused))?;
        Ok(client)
    }
}
