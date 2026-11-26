// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The client side of the control API: hello, calls and events over any
//! pair of byte streams.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use ctlproto::{
    Call, ClientFrame, ErrorInfo, Event, Hello, PROTOCOL, ServerFrame, decode_server, encode,
};
use msg::{Message, message};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// How long to keep trying a pipe whose every instance is busy.
const BUSY_WAIT: Duration = Duration::from_secs(5);

/// `ERROR_PIPE_BUSY`: every instance of the pipe has a client.
const ERROR_PIPE_BUSY: i32 = 231;

/// Why a request did not get a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The service answered with an error.
    Api(ErrorInfo),
    /// The service could not be reached, or stopped answering.
    Connection(Message),
}

/// A connection to the service.
pub struct Client<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
    /// Events that arrived while waiting for an answer.
    events: VecDeque<Event>,
}

impl Client<BufReader<File>, File> {
    /// Opens the control pipe `\\.\pipe\<name>` and says hello.
    pub fn connect(name: &str) -> Result<Self, Failure> {
        let path = format!(r"\\.\pipe\{name}");
        let deadline = Instant::now() + BUSY_WAIT;
        let pipe = loop {
            match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(pipe) => break pipe,
                // The service makes a new instance as soon as a client
                // connects; wait for it.
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    return Err(Failure::Connection(message(
                        "RMD2008",
                        &[name, &e.to_string()],
                    )));
                }
            }
        };
        let reader = pipe
            .try_clone()
            .map_err(|e| Failure::Connection(message("RMD2008", &[name, &e.to_string()])))?;
        let mut client = Client::new(BufReader::new(reader), pipe);
        client.hello()?;
        Ok(client)
    }
}

impl<R: BufRead, W: Write> Client<R, W> {
    /// A client on a pair of streams. Nothing has been said yet.
    pub fn new(reader: R, writer: W) -> Self {
        Client {
            reader,
            writer,
            next_id: 1,
            events: VecDeque::new(),
        }
    }

    fn send(&mut self, frame: &ClientFrame) -> Result<(), Failure> {
        let line = encode(frame);
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.write_all(b"\n"))
            .and_then(|()| self.writer.flush())
            .map_err(|e| Failure::Connection(message("RMD2009", &[&e.to_string()])))
    }

    fn receive(&mut self) -> Result<ServerFrame, Failure> {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .map_err(|e| Failure::Connection(message("RMD2009", &[&e.to_string()])))?;
        if read == 0 || !line.ends_with('\n') {
            let closed = message("RMD2009", &["the service closed the connection"]);
            return Err(Failure::Connection(closed));
        }
        decode_server(line.trim_end()).map_err(Failure::Connection)
    }

    /// The hello exchange. A service that speaks another protocol answers
    /// with an error, which is returned as it is.
    pub fn hello(&mut self) -> Result<Hello, Failure> {
        self.send(&ClientFrame::Hello {
            hello: Hello {
                protocol: PROTOCOL,
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        })?;
        match self.receive()? {
            ServerFrame::Hello { hello } => Ok(hello),
            ServerFrame::Error { error, .. } => Err(Failure::Api(error)),
            other => Err(unexpected(&other)),
        }
    }

    /// Makes a call and returns its result as JSON.
    pub fn call_value(&mut self, call: &Call) -> Result<Value, Failure> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ClientFrame::Request(call.request(id)))?;
        loop {
            match self.receive()? {
                ServerFrame::Event(event) => self.events.push_back(event),
                ServerFrame::Result { id: got, result } if got == id => return Ok(result),
                ServerFrame::Error { id: got, error } if got == Some(id) || got.is_none() => {
                    return Err(Failure::Api(error));
                }
                other => return Err(unexpected(&other)),
            }
        }
    }

    /// Makes a call and reads its result as `T`.
    pub fn call<T: DeserializeOwned>(&mut self, call: &Call) -> Result<T, Failure> {
        let result = self.call_value(call)?;
        serde_json::from_value(result)
            .map_err(|e| Failure::Connection(message("RMD2013", &[&e.to_string()])))
    }

    /// The next event, waiting for it if none has arrived.
    pub fn next_event(&mut self) -> Result<Event, Failure> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        match self.receive()? {
            ServerFrame::Event(event) => Ok(event),
            other => Err(unexpected(&other)),
        }
    }
}

/// A frame that should not have come where it did.
fn unexpected(frame: &ServerFrame) -> Failure {
    let what = match frame {
        ServerFrame::Hello { .. } => "an unexpected hello",
        ServerFrame::Result { .. } => "an answer to another request",
        ServerFrame::Error { .. } => "an error for another request",
        ServerFrame::Event(_) => "an unexpected event",
    };
    Failure::Connection(message("RMD2013", &[what]))
}
