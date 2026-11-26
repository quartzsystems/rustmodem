// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! An in-memory transport: `tokio::io::duplex`, and a listener that hands
//! out the server ends. The tests run the server over it.
//!
//! [`blocking`] gives a client's end to code that reads and writes as
//! `std::io` does, on a thread that is not one of the runtime's:
//! `rustmodemctl` is such a client, and its tests use it.

use std::io::{self, Read, Write};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::{Connection, Halves, Listener};

/// How many bytes a connection holds each way before a writer has to
/// wait, by default.
pub const DEFAULT_CAPACITY: usize = 64 * 1024;

/// The client's end of an in-memory connection.
pub struct MemoryClient {
    /// Bytes from the server. A read of 0 means the server has closed the
    /// connection.
    pub reader: ReadHalf<DuplexStream>,
    /// Bytes to the server. Dropping it tells the server the client has
    /// gone.
    pub writer: WriteHalf<DuplexStream>,
}

/// The server's end of an in-memory connection.
pub struct MemoryConnection(DuplexStream);

impl Connection for MemoryConnection {
    fn peer(&self) -> String {
        "memory".to_string()
    }

    fn split(self) -> Halves {
        let (reader, writer) = tokio::io::split(self.0);
        Halves {
            reader: Box::new(reader),
            writer: Box::new(writer),
        }
    }
}

/// Makes a connection that holds `capacity` bytes each way.
pub fn duplex(capacity: usize) -> (MemoryClient, MemoryConnection) {
    let (client, server) = tokio::io::duplex(capacity.max(1));
    let (reader, writer) = tokio::io::split(client);
    (MemoryClient { reader, writer }, MemoryConnection(server))
}

/// The listening side: yields the server end of each connection made
/// through the connector.
pub struct MemoryListener {
    incoming: mpsc::UnboundedReceiver<MemoryConnection>,
}

/// The connecting side. When every connector has been dropped the listener
/// reports that it has shut down.
#[derive(Clone)]
pub struct MemoryConnector {
    outgoing: mpsc::UnboundedSender<MemoryConnection>,
}

/// Makes a listener and the connector that reaches it.
pub fn listener() -> (MemoryListener, MemoryConnector) {
    let (outgoing, incoming) = mpsc::unbounded_channel();
    (MemoryListener { incoming }, MemoryConnector { outgoing })
}

impl Listener for MemoryListener {
    type Connection = MemoryConnection;

    async fn accept(&mut self) -> io::Result<Option<MemoryConnection>> {
        Ok(self.incoming.recv().await)
    }
}

impl MemoryConnector {
    /// Connects with the default capacity.
    pub fn connect(&self) -> io::Result<MemoryClient> {
        self.connect_with_capacity(DEFAULT_CAPACITY)
    }

    /// Connects with a connection that holds `capacity` bytes each way. A
    /// small capacity and a client that does not read make a slow client.
    pub fn connect_with_capacity(&self, capacity: usize) -> io::Result<MemoryClient> {
        let (client, connection) = duplex(capacity);
        self.outgoing
            .send(connection)
            .map_err(|_| io::Error::from(io::ErrorKind::ConnectionRefused))?;
        Ok(client)
    }
}

/// The reading end of a client's connection, as `std::io::Read`.
pub struct BlockingReader {
    reader: ReadHalf<DuplexStream>,
    runtime: Handle,
    timeout: Option<Duration>,
}

impl BlockingReader {
    /// Makes reads that find nothing give up after this long with a
    /// `TimedOut` error, so that a test waiting for something that never
    /// comes fails instead of hanging.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}

impl Read for BlockingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (reader, timeout) = (&mut self.reader, self.timeout);
        self.runtime.block_on(async move {
            match timeout {
                None => reader.read(buf).await,
                Some(timeout) => tokio::time::timeout(timeout, reader.read(buf))
                    .await
                    .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into())),
            }
        })
    }
}

/// The writing end of a client's connection, as `std::io::Write`.
pub struct BlockingWriter {
    writer: WriteHalf<DuplexStream>,
    runtime: Handle,
}

impl Write for BlockingWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.runtime.block_on(self.writer.write(data))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A client's end for code that blocks. `runtime` is the runtime the
/// server runs on, which must have threads of its own: the reads and
/// writes wait on the calling thread, which must not be one of them.
pub fn blocking(client: MemoryClient, runtime: &Handle) -> (BlockingReader, BlockingWriter) {
    (
        BlockingReader {
            reader: client.reader,
            runtime: runtime.clone(),
            timeout: None,
        },
        BlockingWriter {
            writer: client.writer,
            runtime: runtime.clone(),
        },
    )
}
