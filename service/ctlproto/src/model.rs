// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the service reports about itself: lines, calls and events. The
//! pool builds these; the control server sends them as they are.

use msg::Message;
use serde::{Deserialize, Serialize};

/// The state of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineState {
    /// The line is out of service by configuration.
    Disabled,
    /// The serial port is closed, or the host has DTR off.
    NotReady,
    /// In service, with no call.
    Idle,
    /// A call is being offered to the line.
    Ringing,
    /// A call is being set up.
    Handshaking,
    /// A call is connected.
    Online,
}

impl LineState {
    /// The state as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            LineState::Disabled => "disabled",
            LineState::NotReady => "not_ready",
            LineState::Idle => "idle",
            LineState::Ringing => "ringing",
            LineState::Handshaking => "handshaking",
            LineState::Online => "online",
        }
    }
}

/// Which way a call went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The line dialled out.
    Outgoing,
    /// The call was offered to the line.
    Incoming,
}

impl Direction {
    /// The direction as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Outgoing => "outgoing",
            Direction::Incoming => "incoming",
        }
    }
}

/// Why a call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// The host hung up with an `H` command.
    Hangup,
    /// The host dropped DTR.
    Dtr,
    /// The host sent a character while the call was being set up.
    Aborted,
    /// Nothing answered in time.
    NoAnswer,
    /// The number dialled was busy.
    Busy,
    /// There was no dial tone.
    NoDialtone,
    /// The far end cleared the call, or the carrier was lost.
    Remote,
    /// The caller gave up before the line answered.
    Abandoned,
    /// The call could not be set up: the number does not exist or the line
    /// it names is not in service.
    Failed,
    /// The host reset the modem with a `Z` command.
    Reset,
    /// The operator hung the line up, reset it, disabled it or removed it.
    Operator,
    /// The service stopped.
    Shutdown,
}

impl EndReason {
    /// The reason as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            EndReason::Hangup => "hangup",
            EndReason::Dtr => "dtr",
            EndReason::Aborted => "aborted",
            EndReason::NoAnswer => "no_answer",
            EndReason::Busy => "busy",
            EndReason::NoDialtone => "no_dialtone",
            EndReason::Remote => "remote",
            EndReason::Abandoned => "abandoned",
            EndReason::Failed => "failed",
            EndReason::Reset => "reset",
            EndReason::Operator => "operator",
            EndReason::Shutdown => "shutdown",
        }
    }
}

/// What is known about a connection. Later phases fill in more of it.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConnectInfo {
    /// The line rate in bit/s; 0 means not known.
    pub rate: u32,
    /// The carrier (modulation) name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier: Option<String>,
    /// The error-control protocol name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// The data compression name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
}

/// The counters of a line since the service started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Counters {
    /// Calls the line dialled.
    pub calls_out: u64,
    /// Calls offered to the line.
    pub calls_in: u64,
    /// Calls that connected.
    pub connects: u64,
    /// Data bytes from the host to the far end.
    pub bytes_tx: u64,
    /// Data bytes from the far end to the host.
    pub bytes_rx: u64,
}

/// A call in progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallInfo {
    /// The call id, unique since the service started.
    pub id: u64,
    /// Which way the call went.
    pub direction: Direction,
    /// The number dialled, or for an incoming call the caller.
    pub number: String,
    /// When the call started, in milliseconds since the Unix epoch.
    pub started_ms: u64,
    /// When the call connected, if it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_ms: Option<u64>,
    /// What is known about the connection, once connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<ConnectInfo>,
    /// Data bytes from the host to the far end so far.
    pub bytes_tx: u64,
    /// Data bytes from the far end to the host so far.
    pub bytes_rx: u64,
}

/// A call that has ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallRecord {
    /// The call id, unique since the service started.
    pub id: u64,
    /// The line the call was on.
    pub line: u16,
    /// Which way the call went.
    pub direction: Direction,
    /// The number dialled, or for an incoming call the caller.
    pub number: String,
    /// When the call started, in milliseconds since the Unix epoch.
    pub started_ms: u64,
    /// When the call connected, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_ms: Option<u64>,
    /// When the call ended.
    pub ended_ms: u64,
    /// Why the call ended.
    pub reason: EndReason,
    /// What is known about the connection, if the call connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<ConnectInfo>,
    /// Data bytes from the host to the far end.
    pub bytes_tx: u64,
    /// Data bytes from the far end to the host.
    pub bytes_rx: u64,
}

/// An `RMD` message as the control API carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// The message id, `RMD` and four digits.
    pub msgid: String,
    /// The first-level text.
    pub text: String,
    /// The second-level text, part one.
    pub cause: String,
    /// The second-level text, part two.
    pub recovery: String,
}

impl From<&Message> for ErrorInfo {
    fn from(message: &Message) -> Self {
        ErrorInfo {
            msgid: message.id.clone(),
            text: message.text.clone(),
            cause: message.cause.clone(),
            recovery: message.recovery.clone(),
        }
    }
}

impl From<Message> for ErrorInfo {
    fn from(message: Message) -> Self {
        ErrorInfo::from(&message)
    }
}

impl std::fmt::Display for ErrorInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.msgid, self.text)
    }
}

/// Everything about one line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineInfo {
    /// The line id.
    pub id: u16,
    /// The line's name, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The serial backend, for example `com0com`.
    pub backend: String,
    /// The serial port, for example `CNCB0`.
    pub port: String,
    /// Whether the line is in service by configuration.
    pub enabled: bool,
    /// The state of the line.
    pub state: LineState,
    /// The configured initial S0.
    pub auto_answer_rings: u8,
    /// What the line answers to an unknown AT command: `ok` or `error`.
    pub unknown_command: String,
    /// The counters of the line.
    pub counters: Counters,
    /// The call in progress, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<CallInfo>,
    /// Why the serial port is not open, if it should be and is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

/// How serious a log line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Something failed.
    Error,
    /// Something is wrong but the service went on.
    Warning,
    /// Something happened.
    Info,
    /// Detail, including every AT command line and result code.
    Debug,
}

impl LogLevel {
    /// The level as the log and the control API spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warning => "warning",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }
}

/// One line of the service log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEvent {
    /// When, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    /// How serious.
    pub level: LogLevel,
    /// Where from: `Line01`, `CTL`, `SVC`.
    pub source: String,
    /// The text.
    pub text: String,
}

/// A call starting or connecting on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallEvent {
    /// The line the call is on.
    pub line: u16,
    /// The call.
    pub call: CallInfo,
}

/// Something that happened in the service, sent to clients that have
/// subscribed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum Event {
    /// A line changed state. The data is the whole line, counters included.
    #[serde(rename = "line.state")]
    LineState(Box<LineInfo>),
    /// A line dialled or was offered a call.
    #[serde(rename = "call.started")]
    CallStarted(CallEvent),
    /// A call connected.
    #[serde(rename = "call.connected")]
    CallConnected(CallEvent),
    /// A call ended.
    #[serde(rename = "call.ended")]
    CallEnded(CallRecord),
    /// A line was written to the service log.
    #[serde(rename = "log")]
    Log(LogEvent),
}

impl Event {
    /// The event name as the control API spells it.
    pub fn name(&self) -> &'static str {
        match self {
            Event::LineState(_) => "line.state",
            Event::CallStarted(_) => "call.started",
            Event::CallConnected(_) => "call.connected",
            Event::CallEnded(_) => "call.ended",
            Event::Log(_) => "log",
        }
    }

    /// Whether the event reports state that a client must not miss. Log
    /// lines are the only events that may be dropped for a slow client.
    pub fn is_state(&self) -> bool {
        !matches!(self, Event::Log(_))
    }
}
