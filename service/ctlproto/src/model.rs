// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the service reports about itself: lines, calls and events. The
//! pool builds these; the control server sends them as they are.

use msg::Message;
use serde::{Deserialize, Serialize};

use crate::telephony::{LineSip, MediaInfo, SipRegistration, SipSummary, SipTrace};

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
    /// Every state.
    pub const ALL: [LineState; 6] = [
        LineState::Disabled,
        LineState::NotReady,
        LineState::Idle,
        LineState::Ringing,
        LineState::Handshaking,
        LineState::Online,
    ];

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
    /// Both directions.
    pub const ALL: [Direction; 2] = [Direction::Outgoing, Direction::Incoming];

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
    /// The call could not be set up or carried on: the number is not a line
    /// in service, the line called went away, or the line's port failed.
    Failed,
    /// The host reset the modem with a `Z` command.
    Reset,
    /// The operator hung the line up, reset it, disabled it or removed it.
    Operator,
    /// The service stopped.
    Shutdown,
}

impl EndReason {
    /// Every reason.
    pub const ALL: [EndReason; 12] = [
        EndReason::Hangup,
        EndReason::Dtr,
        EndReason::Aborted,
        EndReason::NoAnswer,
        EndReason::Busy,
        EndReason::NoDialtone,
        EndReason::Remote,
        EndReason::Abandoned,
        EndReason::Failed,
        EndReason::Reset,
        EndReason::Operator,
        EndReason::Shutdown,
    ];

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
    /// AT command lines parsed on the line.
    pub at_commands: u64,
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
    /// The RTP of the call, once it has media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Box<MediaInfo>>,
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
    /// The RTP of the call, if it had media.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Box<MediaInfo>>,
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
    /// What the host side of the line's port is bound to, for example
    /// `Standard 56000 bps Modem on COM4`. Absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    /// The modem engine of the line: `null` or `echo`.
    #[serde(default = "default_engine")]
    pub engine: String,
    /// The names of the hunt groups the line is in.
    #[serde(default)]
    pub hunt_groups: Vec<String>,
    /// The SIP side of the line.
    #[serde(default)]
    pub sip: LineSip,
    /// Whether the line's calls are captured.
    #[serde(default)]
    pub capture: bool,
}

fn default_engine() -> String {
    "null".to_string()
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
    /// Every level, most serious first.
    pub const ALL: [LogLevel; 4] = [
        LogLevel::Error,
        LogLevel::Warning,
        LogLevel::Info,
        LogLevel::Debug,
    ];

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

/// How the service is being run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceMode {
    /// Under the Windows service control manager.
    Service,
    /// In the foreground, with `--console`.
    Console,
    /// The demo service: memory ports and scripted calls.
    Demo,
}

impl ServiceMode {
    /// Every mode.
    pub const ALL: [ServiceMode; 3] = [
        ServiceMode::Service,
        ServiceMode::Console,
        ServiceMode::Demo,
    ];

    /// The mode as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceMode::Service => "service",
            ServiceMode::Console => "console",
            ServiceMode::Demo => "demo",
        }
    }
}

/// What kind of serial port a port is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    /// One end of a com0com null-modem pair.
    Com0com,
    /// A serial port with a `COM` name.
    Serial,
    /// Anything else the system lists as a serial device.
    Other,
}

impl PortKind {
    /// Every kind.
    pub const ALL: [PortKind; 3] = [PortKind::Com0com, PortKind::Serial, PortKind::Other];

    /// The kind as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            PortKind::Com0com => "com0com",
            PortKind::Serial => "serial",
            PortKind::Other => "other",
        }
    }
}

/// One serial port of the machine the service runs on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    /// The name the port is opened by, for example `CNCB0`.
    pub name: String,
    /// What kind of port it is.
    pub kind: PortKind,
    /// For com0com, the name of the other end of the pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<String>,
    /// The COM name applications see for the host side of the port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<String>,
    /// The friendly name of a modem device attached to the host port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modem: Option<String>,
    /// The id of the line that uses the port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u16>,
}

impl PortInfo {
    /// What the host side of the port is bound to, as `LineInfo.binding`
    /// says it: the modem and the host port, the host port alone, or
    /// nothing when neither is known.
    pub fn binding(&self) -> Option<String> {
        match (&self.modem, &self.host_port) {
            (Some(modem), Some(port)) => Some(format!("{modem} on {port}")),
            (None, Some(port)) => Some(port.clone()),
            (Some(modem), None) => Some(modem.clone()),
            (None, None) => None,
        }
    }
}

/// How many connected calls negotiated one rate and carrier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectTally {
    /// The line rate in bit/s; 0 means not known.
    pub rate: u32,
    /// The carrier (modulation) name, if the engine reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier: Option<String>,
    /// How many calls.
    pub count: u64,
}

/// How many connected calls negotiated something with a name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// The name.
    pub name: String,
    /// How many calls.
    pub count: u64,
}

/// The statistics of the service since it started. The call counts are of
/// calls offered to lines.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Calls a line answered.
    pub calls_answered: u64,
    /// Calls whose modems connected.
    pub calls_connected: u64,
    /// Calls answered that never connected.
    pub calls_failed: u64,
    /// Calls turned away: the line was busy, or no line was ready.
    pub calls_rejected: u64,
    /// Calls the caller gave up on while the line was ringing.
    pub calls_abandoned: u64,
    /// The mean length of the connected calls that have ended, in
    /// milliseconds.
    pub avg_call_ms: u64,
    /// The length of the longest connected call that has ended, in
    /// milliseconds.
    pub longest_call_ms: u64,
    /// The most lines that had a call at one time.
    pub peak_lines_in_use: u32,
    /// How many lines have a call now.
    pub lines_in_use: u32,
    /// Data bytes from the far ends to the hosts.
    pub bytes_in: u64,
    /// Data bytes from the hosts to the far ends.
    pub bytes_out: u64,
    /// AT command lines parsed.
    pub at_commands: u64,
    /// The connected calls by what they negotiated, largest count first.
    pub connects: Vec<ConnectTally>,
    /// Retrains and rate renegotiations.
    pub retrains: u64,
    /// The connected calls by error-control protocol, largest count first.
    pub protocols: Vec<Tally>,
    /// The connected calls by data compression, largest count first.
    pub compressions: Vec<Tally>,
    /// RTP in both directions over the last second or so, headers
    /// included, in kbit/s.
    #[serde(default)]
    pub rtp_kbps: u32,
    /// The mean interarrival jitter of the calls in progress, in
    /// milliseconds.
    #[serde(default)]
    pub jitter_ms_avg: f64,
    /// The most the jitter of a call in progress has been, in
    /// milliseconds.
    #[serde(default)]
    pub jitter_ms_max: f64,
    /// The packets lost, in a hundred expected, over every call since the
    /// service started.
    #[serde(default)]
    pub packet_loss_percent: f64,
    /// The codec the service prefers: `PCMU` or `PCMA`; empty when SIP is
    /// off.
    #[serde(default)]
    pub codec: String,
    /// The packet time the service asks for, in milliseconds; 0 when SIP
    /// is off.
    #[serde(default)]
    pub ptime_ms: u32,
    /// How much of one processor the service process has used over the
    /// last second or so, in a hundred.
    #[serde(default)]
    pub cpu_percent: f64,
    /// SIP in a line of numbers.
    #[serde(default)]
    pub sip: SipSummary,
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
    /// A line's registration changed state.
    #[serde(rename = "sip.registration")]
    SipRegistration(SipRegistration),
    /// A SIP message was sent or received.
    #[serde(rename = "sip.trace")]
    SipTrace(SipTrace),
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
            Event::SipRegistration(_) => "sip.registration",
            Event::SipTrace(_) => "sip.trace",
        }
    }

    /// Whether the event reports state that a client must not miss. Log
    /// lines and traced messages are the events that may be dropped for a
    /// slow client.
    pub fn is_state(&self) -> bool {
        !matches!(self, Event::Log(_) | Event::SipTrace(_))
    }
}
