// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The connection to the service: the threads that own the pipe.
//!
//! A [`Link`] keeps one thread for as long as it lives. The thread opens
//! the control pipe, says hello, and then writes the requests the window
//! hands it, while a second thread reads what the service sends and
//! forwards each decoded message to the window through a
//! [`win32::UiSender`]. When the pipe cannot be opened, or the connection
//! is lost, the thread says so and tries again every [`RETRY`], until it
//! is stopped.
//!
//! Nothing here touches a window or the model. The window owns both and
//! is told what happened.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctlproto::{
    ClientFrame, ErrorInfo, Hello, MAX_MESSAGE_BYTES, PROTOCOL, ServerFrame, decode_server, encode,
};
use ctlsrv::{Closer, Halves};
use msg::message;
use pipe::PipeClient;
use win32::UiSender;

/// How long the manager waits before it tries to connect again.
pub const RETRY: Duration = Duration::from_secs(3);

/// How long to wait for a free instance of the pipe.
const BUSY_WAIT: Duration = Duration::from_secs(2);

/// What the link tells the window.
#[derive(Debug)]
pub enum Incoming {
    /// The link is trying to open the pipe.
    Connecting,
    /// The pipe is open and the service has said hello.
    Connected,
    /// The pipe could not be opened, or the hello failed. The link tries
    /// again by itself.
    Failed {
        /// Why.
        error: ErrorInfo,
        /// The pipe does not admit this user: the manager needs
        /// administrator rights.
        denied: bool,
    },
    /// A message from the service.
    Frame(Box<ServerFrame>),
    /// The connection was lost. The link tries again by itself.
    Lost(ErrorInfo),
}

/// What the link tells the window, with the number the window gave the
/// link: the window has a new link for every connection the user asks
/// for, and what an old link still says is not about the new one.
pub type Tagged = (u64, Incoming);

/// What the window, or the reading thread, tells the writing thread.
enum Order {
    /// Send this line to the service.
    Send(String),
    /// The reading thread of this connection ended, for this reason.
    ReaderEnded(u64, ErrorInfo),
    /// End.
    Stop,
}

/// What both sides of the link share.
struct Shared {
    stopped: AtomicBool,
    /// The means to close the connection that is open, from any thread.
    closer: Mutex<Option<Arc<dyn Closer>>>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn set_closer(&self, closer: Option<Arc<dyn Closer>>) {
        // The mutex guards one assignment; a thread that panicked holding
        // it left a whole value.
        *self.closer.lock().unwrap_or_else(|e| e.into_inner()) = closer;
    }

    fn close(&self) {
        let closer = self
            .closer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(closer) = closer {
            closer.close();
        }
    }
}

/// A connection to the service that keeps itself up.
pub struct Link {
    orders: Sender<Order>,
    shared: Arc<Shared>,
}

impl Link {
    /// Starts connecting to the control pipe `name`. Everything the link
    /// has to say goes to `sender`, tagged with `epoch`.
    pub fn start(name: &str, epoch: u64, sender: UiSender<Tagged>) -> Link {
        let (orders, receiver) = channel();
        let shared = Arc::new(Shared {
            stopped: AtomicBool::new(false),
            closer: Mutex::new(None),
        });
        let thread = Worker {
            name: name.to_string(),
            epoch,
            sender,
            orders: orders.clone(),
            shared: Arc::clone(&shared),
        };
        // The thread ends when the link is stopped; nothing waits for it.
        let _ = std::thread::Builder::new()
            .name("control link".to_string())
            .spawn(move || thread.run(&receiver));
        Link { orders, shared }
    }

    /// Sends a line to the service. While there is no connection the
    /// line is dropped: its request runs out of time like any other that
    /// gets no answer.
    pub fn send(&self, line: String) {
        let _ = self.orders.send(Order::Send(line));
    }

    /// Closes the connection that is open. The link says that it was
    /// lost, and tries again after its pause.
    pub fn hang_up(&self) {
        self.shared.close();
    }

    /// Closes the connection and ends the link's threads. They say
    /// nothing more to the window once they have seen this.
    pub fn stop(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        // A read or a write that is waiting returns.
        self.shared.close();
        let _ = self.orders.send(Order::Stop);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A failed attempt to connect.
struct Failure {
    error: ErrorInfo,
    denied: bool,
}

impl From<ErrorInfo> for Failure {
    fn from(error: ErrorInfo) -> Self {
        Failure {
            error,
            denied: false,
        }
    }
}

/// An open connection, after the hello.
struct Connection {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    closer: Arc<dyn Closer>,
}

/// Why the writing loop ended.
enum Ended {
    Stopped,
    Lost(ErrorInfo),
}

fn lost(reason: &str) -> ErrorInfo {
    ErrorInfo::from(message("RMD2009", &[reason]))
}

/// Writes one line and its newline.
fn write_line(writer: &mut dyn Write, line: &str) -> Result<(), ErrorInfo> {
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|e| lost(&e.to_string()))
}

/// Reads one message from the service.
fn read_frame(reader: &mut dyn BufRead) -> Result<ServerFrame, ErrorInfo> {
    let mut line = String::new();
    // One byte more than a message may have, so that one that is too
    // long is seen to be, and is not read without end.
    let read = reader
        .take(MAX_MESSAGE_BYTES as u64 + 1)
        .read_line(&mut line)
        .map_err(|e| lost(&e.to_string()))?;
    if read == 0 || !line.ends_with('\n') {
        if line.len() > MAX_MESSAGE_BYTES {
            let limit = MAX_MESSAGE_BYTES.to_string();
            return Err(ErrorInfo::from(message("RMD2014", &[&limit])));
        }
        return Err(lost("the service closed the connection"));
    }
    decode_server(line.trim_end()).map_err(ErrorInfo::from)
}

struct Worker {
    name: String,
    epoch: u64,
    sender: UiSender<Tagged>,
    /// For the reading thread, to say that it ended.
    orders: Sender<Order>,
    shared: Arc<Shared>,
}

impl Worker {
    /// Tells the window. Returns `false` when there is nobody to tell:
    /// the link was stopped, or the window has gone.
    fn tell(&self, what: Incoming) -> bool {
        !self.shared.stopped() && self.sender.send((self.epoch, what))
    }

    fn run(&self, orders: &Receiver<Order>) {
        let mut connections = 0u64;
        loop {
            if !self.tell(Incoming::Connecting) {
                return;
            }
            match self.connect() {
                Err(failure) => {
                    self.shared.close();
                    self.shared.set_closer(None);
                    self.tell(Incoming::Failed {
                        error: failure.error,
                        denied: failure.denied,
                    });
                }
                Ok(connection) => {
                    connections += 1;
                    let Connection {
                        reader,
                        mut writer,
                        closer,
                    } = connection;
                    self.tell(Incoming::Connected);
                    let reading = self.start_reader(reader, connections);
                    let ended = serve(orders, writer.as_mut(), connections);
                    closer.close();
                    self.shared.set_closer(None);
                    if let Some(reading) = reading {
                        let _ = reading.join();
                    }
                    match ended {
                        Ended::Stopped => return,
                        Ended::Lost(error) => {
                            self.tell(Incoming::Lost(error));
                        }
                    }
                }
            }
            if !pause(orders) {
                return;
            }
        }
    }

    /// Opens the pipe and says hello.
    fn connect(&self) -> Result<Connection, Failure> {
        let client = PipeClient::connect(&self.name, BUSY_WAIT).map_err(|e| Failure {
            error: ErrorInfo::from(message("RMD2008", &[&self.name, &e.to_string()])),
            denied: e.kind() == ErrorKind::PermissionDenied,
        })?;
        let Halves {
            reader,
            mut writer,
            closer,
        } = client.split();
        // From here the window can end a hello that never comes.
        self.shared.set_closer(Some(Arc::clone(&closer)));
        if self.shared.stopped() {
            closer.close();
        }
        let hello = ClientFrame::Hello {
            hello: Hello {
                protocol: PROTOCOL,
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        write_line(writer.as_mut(), &encode(&hello))?;
        let mut reader = BufReader::new(reader);
        match read_frame(&mut reader)? {
            ServerFrame::Hello { .. } => Ok(Connection {
                reader,
                writer,
                closer,
            }),
            // A service that speaks another protocol says so.
            ServerFrame::Error { error, .. } => Err(error.into()),
            _ => Err(ErrorInfo::from(message(
                "RMD2013",
                &["a message that is not hello came first"],
            ))
            .into()),
        }
    }

    /// Starts the thread that reads what the service sends.
    fn start_reader(
        &self,
        mut reader: BufReader<Box<dyn Read + Send>>,
        connection: u64,
    ) -> Option<std::thread::JoinHandle<()>> {
        let sender = self.sender.clone();
        let orders = self.orders.clone();
        let shared = Arc::clone(&self.shared);
        let epoch = self.epoch;
        std::thread::Builder::new()
            .name("control reader".to_string())
            .spawn(move || {
                let reason = loop {
                    match read_frame(&mut reader) {
                        Ok(frame) => {
                            if shared.stopped()
                                || !sender.send((epoch, Incoming::Frame(Box::new(frame))))
                            {
                                break lost("the manager closed the connection");
                            }
                        }
                        Err(error) => break error,
                    }
                };
                let _ = orders.send(Order::ReaderEnded(connection, reason));
            })
            .ok()
    }
}

/// Writes what the window sends until the connection ends.
fn serve(orders: &Receiver<Order>, writer: &mut dyn Write, connection: u64) -> Ended {
    loop {
        match orders.recv() {
            Ok(Order::Send(line)) => {
                if let Err(error) = write_line(writer, &line) {
                    return Ended::Lost(error);
                }
            }
            Ok(Order::ReaderEnded(ended, reason)) if ended == connection => {
                return Ended::Lost(reason);
            }
            // The end of an earlier connection.
            Ok(Order::ReaderEnded(..)) => {}
            Ok(Order::Stop) | Err(_) => return Ended::Stopped,
        }
    }
}

/// Waits out the pause before the next attempt. Returns `false` when the
/// link was stopped.
fn pause(orders: &Receiver<Order>) -> bool {
    let until = std::time::Instant::now() + RETRY;
    loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        match orders.recv_timeout(left) {
            Err(RecvTimeoutError::Timeout) => return true,
            Ok(Order::Stop) | Err(RecvTimeoutError::Disconnected) => return false,
            // There is no connection to send on, and the end of the last
            // one is known.
            Ok(Order::Send(_) | Order::ReaderEnded(..)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn a_message_is_one_line() {
        let mut input = Cursor::new(
            b"{\"hello\":{\"protocol\":1,\"version\":\"0.0.2\"}}\n{\"id\":4,\"result\":{}}\n"
                .to_vec(),
        );
        assert!(matches!(
            read_frame(&mut input),
            Ok(ServerFrame::Hello { .. })
        ));
        assert!(matches!(
            read_frame(&mut input),
            Ok(ServerFrame::Result { id: 4, .. })
        ));
        // The end of the stream is the end of the connection.
        let error = read_frame(&mut input).unwrap_err();
        assert_eq!(error.msgid, "RMD2009");
    }

    #[test]
    fn half_a_line_and_nonsense_are_messages_of_their_own() {
        let mut cut = Cursor::new(b"{\"id\":4,\"resu".to_vec());
        assert_eq!(read_frame(&mut cut).unwrap_err().msgid, "RMD2009");
        let mut nonsense = Cursor::new(b"what\n".to_vec());
        let error = read_frame(&mut nonsense).unwrap_err();
        assert_ne!(error.msgid, "RMD2009", "{error}");
    }

    #[test]
    fn a_line_without_end_is_not_read_without_end() {
        let mut long = Cursor::new(vec![b'x'; MAX_MESSAGE_BYTES + 100]);
        assert_eq!(read_frame(&mut long).unwrap_err().msgid, "RMD2014");
    }

    #[test]
    fn a_line_goes_out_with_its_newline() {
        let mut out = Vec::new();
        write_line(&mut out, "{\"id\":1}").unwrap();
        assert_eq!(out, b"{\"id\":1}\n");
    }
}
