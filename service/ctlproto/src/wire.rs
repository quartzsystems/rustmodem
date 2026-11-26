// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What goes over the control connection: one JSON object a line.
//!
//! The first message each way is `hello`. After that the client sends
//! requests and the service answers each with a result or an error; once a
//! client has subscribed, the service also sends it events.

use msg::{Message, message};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{
    CallRecord, ErrorInfo, Event, LineInfo, LogEvent, LogLevel, PortInfo, ServiceMode,
};

/// The protocol number this version speaks.
pub const PROTOCOL: u32 = 1;

/// The longest control message accepted, in bytes, its line feed included.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// The name of every event, in the order the documentation lists them.
pub const EVENT_NAMES: [&str; 7] = [
    "line.state",
    "call.started",
    "call.connected",
    "call.ended",
    "log",
    "sip.registration",
    "sip.trace",
];

/// The events a client gets only when it subscribes to them by name:
/// subscribing to every event leaves them out. They are many and of use to
/// one window.
pub const EVENTS_BY_NAME_ONLY: [&str; 1] = ["sip.trace"];

/// The body of a `hello` message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The protocol number the sender speaks.
    pub protocol: u32,
    /// The sender's version, for people.
    pub version: String,
}

/// A request as it is framed: `{ "id": 1, "method": "...", "params": {} }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestFrame {
    /// Chosen by the client; the answer carries the same id.
    pub id: u64,
    /// The method name.
    pub method: String,
    /// The method's parameters; may be left out when there are none.
    #[serde(default)]
    pub params: Value,
}

/// A message from a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClientFrame {
    /// `{ "hello": { "protocol": 1, "version": "..." } }`
    Hello {
        /// The hello body.
        hello: Hello,
    },
    /// A request.
    Request(RequestFrame),
}

/// A message from the service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerFrame {
    /// `{ "hello": { "protocol": 1, "version": "..." } }`
    Hello {
        /// The hello body.
        hello: Hello,
    },
    /// `{ "id": 1, "result": { ... } }`
    Result {
        /// The id of the request this answers.
        id: u64,
        /// The method's result.
        result: Value,
    },
    /// `{ "id": 1, "error": { "msgid": "RMD2002", ... } }`. The id is null
    /// when the error is not the answer to a request that could be read.
    Error {
        /// The id of the request this answers, if it had one.
        id: Option<u64>,
        /// The error.
        error: ErrorInfo,
    },
    /// `{ "event": "line.state", "data": { ... } }`
    Event(Event),
}

/// Writes a frame as one line of JSON, without the line feed.
pub fn encode<T: Serialize>(frame: &T) -> String {
    // The frame types hold only strings, numbers, booleans and maps with
    // string keys, none of which can fail to serialise.
    serde_json::to_string(frame).unwrap_or_else(|_| String::from("{}"))
}

/// Reads one line from a client.
pub fn decode_client(line: &str) -> Result<ClientFrame, Message> {
    serde_json::from_str(line).map_err(|e| message("RMD2004", &[&e.to_string()]))
}

/// Reads one line from the service.
pub fn decode_server(line: &str) -> Result<ServerFrame, Message> {
    serde_json::from_str(line).map_err(|e| message("RMD2013", &[&e.to_string()]))
}

// --------------------------------------------------------------------------
// Parameters
// --------------------------------------------------------------------------

/// Parameters that name one line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineRef {
    /// The line id.
    pub id: u16,
}

/// The parameters of `lines.add`: a new line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineSpec {
    /// The line id, 1 to 999, not in use.
    pub id: u16,
    /// A name for people.
    #[serde(default)]
    pub name: Option<String>,
    /// The serial backend; `com0com` if left out.
    #[serde(default)]
    pub backend: Option<String>,
    /// The serial port, for example `CNCB0`.
    pub port: String,
    /// Whether the line is in service; true if left out.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// The initial S0; 0 if left out.
    #[serde(default)]
    pub auto_answer_rings: Option<u8>,
    /// `ok` or `error`; `ok` if left out.
    #[serde(default)]
    pub unknown_command: Option<String>,
    /// The modem engine: `null` or `echo`; `null` if left out.
    #[serde(default)]
    pub engine: Option<String>,
    /// The line's registration to a PBX; none if left out.
    #[serde(default)]
    pub register: Option<RegisterSpec>,
}

/// The `register` part of `lines.add`: the line is an extension of a PBX.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterSpec {
    /// The registrar, as `host` or `host:port`.
    pub server: String,
    /// The extension.
    pub user: String,
    /// The password.
    pub password: String,
    /// The user name to authenticate with; the extension if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_user: Option<String>,
    /// The registration time to ask for, in seconds; 300 if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_s: Option<u32>,
}

impl std::fmt::Debug for RegisterSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The password is never written anywhere.
        f.debug_struct("RegisterSpec")
            .field("server", &self.server)
            .field("user", &self.user)
            .field("auth_user", &self.auth_user)
            .field("expires_s", &self.expires_s)
            .finish_non_exhaustive()
    }
}

/// The `register` part of `lines.edit`: the keys of the line's
/// registration to change.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterChange {
    /// A new registrar. An empty string removes the registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// A new extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// A new password. Left out, the password stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// A new user name to authenticate with; an empty string means the
    /// extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_user: Option<String>,
    /// A new registration time to ask for, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_s: Option<u32>,
}

impl std::fmt::Debug for RegisterChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The password is never written anywhere.
        f.debug_struct("RegisterChange")
            .field("server", &self.server)
            .field("user", &self.user)
            .field("auth_user", &self.auth_user)
            .field("expires_s", &self.expires_s)
            .finish_non_exhaustive()
    }
}

/// The parameters of `lines.edit`: the keys of a line to change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineChange {
    /// The line to change.
    pub id: u16,
    /// A new name; an empty string removes the name.
    #[serde(default)]
    pub name: Option<String>,
    /// A new serial backend.
    #[serde(default)]
    pub backend: Option<String>,
    /// A new serial port.
    #[serde(default)]
    pub port: Option<String>,
    /// Whether the line is in service.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// A new initial S0.
    #[serde(default)]
    pub auto_answer_rings: Option<u8>,
    /// `ok` or `error`.
    #[serde(default)]
    pub unknown_command: Option<String>,
    /// A new modem engine: `null` or `echo`.
    #[serde(default)]
    pub engine: Option<String>,
    /// Changes to the line's registration to a PBX.
    #[serde(default)]
    pub register: Option<RegisterChange>,
}

/// The parameters of `lines.capture`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSwitch {
    /// The line id.
    pub id: u16,
    /// Whether the line's calls are captured.
    pub on: bool,
}

/// The parameters of `sip.reregister`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReregisterQuery {
    /// The line to register again; every line that registers if left out.
    #[serde(default)]
    pub id: Option<u16>,
}

/// The parameters of `huntgroups.add`: a new hunt group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuntGroupSpec {
    /// The name, not in use.
    pub name: String,
    /// The number that calls the group.
    pub pilot: String,
    /// The lines of the group, in hunting order.
    pub lines: Vec<u16>,
    /// `first-idle`, `round-robin` or `least-recent`; `first-idle` if left
    /// out.
    #[serde(default)]
    pub strategy: Option<String>,
}

/// The parameters of `huntgroups.edit`: the keys of a hunt group to
/// change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuntGroupChange {
    /// The group to change.
    pub name: String,
    /// A new name. The routes to the group follow it.
    #[serde(default)]
    pub new_name: Option<String>,
    /// A new pilot number.
    #[serde(default)]
    pub pilot: Option<String>,
    /// The lines of the group, all of them, in hunting order.
    #[serde(default)]
    pub lines: Option<Vec<u16>>,
    /// A new strategy.
    #[serde(default)]
    pub strategy: Option<String>,
}

/// Parameters that name one hunt group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuntGroupRef {
    /// The name of the group.
    pub name: String,
}

/// The parameters of `routes.add`: a new route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteSpec {
    /// The called number: exact, or a prefix ending in `*`.
    pub called: String,
    /// Where its calls go: `line:<id>` or `group:<name>`.
    pub target: String,
    /// Where in the list the route goes, from 0; at the end if left out.
    #[serde(default)]
    pub index: Option<u32>,
}

/// The parameters of `routes.edit`: the keys of a route to change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteChange {
    /// Where the route is in the list, from 0.
    pub index: u32,
    /// A new called number.
    #[serde(default)]
    pub called: Option<String>,
    /// A new target.
    #[serde(default)]
    pub target: Option<String>,
}

/// Parameters that name one route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRef {
    /// Where the route is in the list, from 0.
    pub index: u32,
}

/// The parameters of `routes.move`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMove {
    /// Where the route is in the list, from 0.
    pub index: u32,
    /// Where it goes: its index once it has been moved.
    pub to: u32,
}

/// The parameters of `calls.list`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallsQuery {
    /// Return at most this many, the most recent; all if left out.
    #[serde(default)]
    pub last: Option<u32>,
}

/// The parameters of `subscribe` and `unsubscribe`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSelection {
    /// The event names; every event if left out.
    #[serde(default)]
    pub events: Option<Vec<String>>,
}

/// The parameters of `log.tail`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogTailQuery {
    /// Return at most this many lines, the most recent; all if left out.
    #[serde(default)]
    pub last: Option<u32>,
}

/// The `service` part of `config.set`: the keys of `[service]` to change.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceChange {
    /// A new log level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<LogLevel>,
    /// A new log directory; an empty string means the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
}

/// The `control` part of `config.set`: the keys of `[control]` to change.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlChange {
    /// A new control pipe name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipe: Option<String>,
}

/// The `sip` part of `config.set`: the keys of `[sip]` to change.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SipChange {
    /// Whether the service speaks SIP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// A new address for the listener, as `address:port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// A new address to advertise; an empty string means the local
    /// address that routes to the peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// A new User-Agent; an empty string means `rustmodem/<version>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// The trusted peers, all of them: addresses or CIDR blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_peers: Option<Vec<String>>,
    /// A new outbound peer, as `host` or `host:port`; an empty string
    /// means none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_peer: Option<String>,
    /// How long a line may ring, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring_timeout_s: Option<u32>,
}

/// The `media` part of `config.set`: the keys of `[media]` to change.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaChange {
    /// The codecs, all of them, most preferred first: `pcmu`, `pcma`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codecs: Option<Vec<String>>,
    /// A new packet time in milliseconds: 10, 20 or 30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ptime_ms: Option<u32>,
    /// The ports RTP may use, as `first-last`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtp_ports: Option<String>,
    /// How long a call may go without a packet, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtp_timeout_s: Option<u32>,
    /// A new capture directory; an empty string means the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_dir: Option<String>,
}

/// The parameters of `config.set`: the settings to change.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsChange {
    /// Changes to `[service]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceChange>,
    /// Changes to `[control]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlChange>,
    /// Changes to `[sip]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sip: Option<SipChange>,
    /// Changes to `[media]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaChange>,
}

/// Parameters of a method that takes none.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

// --------------------------------------------------------------------------
// Results
// --------------------------------------------------------------------------

/// The result of `service.info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// The service version.
    pub version: String,
    /// The protocol number the service speaks.
    pub protocol: u32,
    /// The name of the control pipe.
    pub pipe: String,
    /// The path of the configuration file.
    pub config_path: String,
    /// How many lines the service has.
    pub lines: u32,
    /// When the service started, in milliseconds since the Unix epoch.
    pub started_ms: u64,
    /// How long the service has been running, in milliseconds.
    pub uptime_ms: u64,
    /// When the configuration was last loaded, in milliseconds since the
    /// Unix epoch.
    pub config_loaded_ms: u64,
    /// How the service is being run.
    pub mode: ServiceMode,
}

/// The result of `lines.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineList {
    /// Every line, in id order.
    pub lines: Vec<LineInfo>,
}

/// The result of `calls.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallList {
    /// The calls that have ended, oldest first.
    pub calls: Vec<CallRecord>,
}

/// The result of `config.reload`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reloaded {
    /// How many lines the configuration has.
    pub lines: u32,
}

/// The result of `subscribe` and `unsubscribe`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    /// The events the client is now subscribed to.
    pub events: Vec<String>,
}

/// The result of `log.tail`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogTail {
    /// The lines, oldest first.
    pub lines: Vec<LogEvent>,
}

/// The `[service]` settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSettings {
    /// How much the service logs.
    pub log_level: LogLevel,
    /// The log directory; empty means the default one.
    pub log_dir: String,
}

/// The `[control]` settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlSettings {
    /// The control pipe name.
    pub pipe: String,
}

/// The `[sip]` settings.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SipSettings {
    /// Whether the service speaks SIP.
    pub enabled: bool,
    /// The address of the listener, as `address:port`.
    pub listen: String,
    /// The address to advertise; empty means the local address that
    /// routes to the peer.
    pub address: String,
    /// The User-Agent; empty means `rustmodem/<version>`.
    pub user_agent: String,
    /// The addresses that may send requests to the listener.
    pub trusted_peers: Vec<String>,
    /// Where lines that are not registered send the calls they dial;
    /// empty means nowhere.
    pub outbound_peer: String,
    /// How long a line may ring, in seconds.
    pub ring_timeout_s: u32,
}

/// The `[media]` settings.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaSettings {
    /// The codecs, most preferred first.
    pub codecs: Vec<String>,
    /// The packet time in milliseconds.
    pub ptime_ms: u32,
    /// The ports RTP may use, as `first-last`.
    pub rtp_ports: String,
    /// How long a call may go without a packet, in seconds.
    pub rtp_timeout_s: u32,
    /// The capture directory; empty means the default one.
    pub capture_dir: String,
}

/// The result of `config.get`: the settings that are not lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The `[service]` settings.
    pub service: ServiceSettings,
    /// The `[control]` settings.
    pub control: ControlSettings,
    /// The `[sip]` settings.
    #[serde(default)]
    pub sip: SipSettings,
    /// The `[media]` settings.
    #[serde(default)]
    pub media: MediaSettings,
    /// The path of the configuration file.
    pub path: String,
}

/// The result of `config.set`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsResult {
    /// Whether the service must be restarted for the change to take
    /// effect.
    pub restart_required: bool,
    /// The settings as the configuration file now has them.
    pub config: Settings,
}

/// The result of `ports.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortList {
    /// The serial ports of the machine, in name order.
    pub ports: Vec<PortInfo>,
}

/// The result of a method that has nothing to report.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Done {}

// --------------------------------------------------------------------------
// Methods
// --------------------------------------------------------------------------

/// A request, understood: a method and its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// `service.info`
    ServiceInfo,
    /// `lines.list`
    LinesList,
    /// `lines.get`
    LinesGet(LineRef),
    /// `lines.answer`
    LinesAnswer(LineRef),
    /// `lines.hangup`
    LinesHangup(LineRef),
    /// `lines.reset`
    LinesReset(LineRef),
    /// `lines.enable`
    LinesEnable(LineRef),
    /// `lines.disable`
    LinesDisable(LineRef),
    /// `lines.add`
    LinesAdd(LineSpec),
    /// `lines.edit`
    LinesEdit(LineChange),
    /// `lines.remove`
    LinesRemove(LineRef),
    /// `lines.capture`
    LinesCapture(CaptureSwitch),
    /// `calls.list`
    CallsList(CallsQuery),
    /// `config.reload`
    ConfigReload,
    /// `config.get`
    ConfigGet,
    /// `config.set`
    ConfigSet(SettingsChange),
    /// `stats.get`
    StatsGet,
    /// `log.tail`
    LogTail(LogTailQuery),
    /// `ports.list`
    PortsList,
    /// `sip.status`
    SipStatus,
    /// `sip.reregister`
    SipReregister(ReregisterQuery),
    /// `huntgroups.list`
    HuntGroupsList,
    /// `huntgroups.add`
    HuntGroupsAdd(HuntGroupSpec),
    /// `huntgroups.edit`
    HuntGroupsEdit(HuntGroupChange),
    /// `huntgroups.remove`
    HuntGroupsRemove(HuntGroupRef),
    /// `routes.list`
    RoutesList,
    /// `routes.add`
    RoutesAdd(RouteSpec),
    /// `routes.edit`
    RoutesEdit(RouteChange),
    /// `routes.remove`
    RoutesRemove(RouteRef),
    /// `routes.move`
    RoutesMove(RouteMove),
    /// `subscribe`
    Subscribe(EventSelection),
    /// `unsubscribe`
    Unsubscribe(EventSelection),
}

impl Call {
    /// The name of every method, in the order the documentation lists them.
    pub const METHODS: [&'static str; 32] = [
        "service.info",
        "lines.list",
        "lines.get",
        "lines.answer",
        "lines.hangup",
        "lines.reset",
        "lines.enable",
        "lines.disable",
        "lines.add",
        "lines.edit",
        "lines.remove",
        "lines.capture",
        "calls.list",
        "config.reload",
        "config.get",
        "config.set",
        "stats.get",
        "log.tail",
        "ports.list",
        "sip.status",
        "sip.reregister",
        "huntgroups.list",
        "huntgroups.add",
        "huntgroups.edit",
        "huntgroups.remove",
        "routes.list",
        "routes.add",
        "routes.edit",
        "routes.remove",
        "routes.move",
        "subscribe",
        "unsubscribe",
    ];

    /// The method name.
    pub fn method(&self) -> &'static str {
        match self {
            Call::ServiceInfo => "service.info",
            Call::LinesList => "lines.list",
            Call::LinesGet(_) => "lines.get",
            Call::LinesAnswer(_) => "lines.answer",
            Call::LinesHangup(_) => "lines.hangup",
            Call::LinesReset(_) => "lines.reset",
            Call::LinesEnable(_) => "lines.enable",
            Call::LinesDisable(_) => "lines.disable",
            Call::LinesAdd(_) => "lines.add",
            Call::LinesEdit(_) => "lines.edit",
            Call::LinesRemove(_) => "lines.remove",
            Call::LinesCapture(_) => "lines.capture",
            Call::CallsList(_) => "calls.list",
            Call::ConfigReload => "config.reload",
            Call::ConfigGet => "config.get",
            Call::ConfigSet(_) => "config.set",
            Call::StatsGet => "stats.get",
            Call::LogTail(_) => "log.tail",
            Call::PortsList => "ports.list",
            Call::SipStatus => "sip.status",
            Call::SipReregister(_) => "sip.reregister",
            Call::HuntGroupsList => "huntgroups.list",
            Call::HuntGroupsAdd(_) => "huntgroups.add",
            Call::HuntGroupsEdit(_) => "huntgroups.edit",
            Call::HuntGroupsRemove(_) => "huntgroups.remove",
            Call::RoutesList => "routes.list",
            Call::RoutesAdd(_) => "routes.add",
            Call::RoutesEdit(_) => "routes.edit",
            Call::RoutesRemove(_) => "routes.remove",
            Call::RoutesMove(_) => "routes.move",
            Call::Subscribe(_) => "subscribe",
            Call::Unsubscribe(_) => "unsubscribe",
        }
    }

    /// The parameters, as they go in a request.
    pub fn params(&self) -> Value {
        fn value<T: Serialize>(params: &T) -> Value {
            serde_json::to_value(params).unwrap_or(Value::Null)
        }
        match self {
            Call::ServiceInfo
            | Call::LinesList
            | Call::ConfigReload
            | Call::ConfigGet
            | Call::StatsGet
            | Call::PortsList
            | Call::SipStatus
            | Call::HuntGroupsList
            | Call::RoutesList => value(&NoParams {}),
            Call::LinesCapture(p) => value(p),
            Call::SipReregister(p) => value(p),
            Call::HuntGroupsAdd(p) => value(p),
            Call::HuntGroupsEdit(p) => value(p),
            Call::HuntGroupsRemove(p) => value(p),
            Call::RoutesAdd(p) => value(p),
            Call::RoutesEdit(p) => value(p),
            Call::RoutesRemove(p) => value(p),
            Call::RoutesMove(p) => value(p),
            Call::ConfigSet(p) => value(p),
            Call::LogTail(p) => value(p),
            Call::LinesGet(p)
            | Call::LinesAnswer(p)
            | Call::LinesHangup(p)
            | Call::LinesReset(p)
            | Call::LinesEnable(p)
            | Call::LinesDisable(p)
            | Call::LinesRemove(p) => value(p),
            Call::LinesAdd(p) => value(p),
            Call::LinesEdit(p) => value(p),
            Call::CallsList(p) => value(p),
            Call::Subscribe(p) | Call::Unsubscribe(p) => value(p),
        }
    }

    /// The request frame for this call.
    pub fn request(&self, id: u64) -> RequestFrame {
        RequestFrame {
            id,
            method: self.method().to_string(),
            params: self.params(),
        }
    }

    /// Understands a method name and its parameters. An unknown method is
    /// message `RMD2002`; parameters that do not fit are `RMD2003`.
    pub fn from_parts(method: &str, params: Value) -> Result<Call, Message> {
        fn read<T: DeserializeOwned>(method: &str, params: Value) -> Result<T, Message> {
            // Parameters left out are the same as an empty object.
            let params = if params.is_null() {
                Value::Object(serde_json::Map::new())
            } else {
                params
            };
            if !params.is_object() {
                return Err(message("RMD2003", &[method, "params is not an object"]));
            }
            serde_json::from_value(params)
                .map_err(|e| message("RMD2003", &[method, &e.to_string()]))
        }
        Ok(match method {
            "service.info" => {
                read::<NoParams>(method, params)?;
                Call::ServiceInfo
            }
            "lines.list" => {
                read::<NoParams>(method, params)?;
                Call::LinesList
            }
            "lines.get" => Call::LinesGet(read(method, params)?),
            "lines.answer" => Call::LinesAnswer(read(method, params)?),
            "lines.hangup" => Call::LinesHangup(read(method, params)?),
            "lines.reset" => Call::LinesReset(read(method, params)?),
            "lines.enable" => Call::LinesEnable(read(method, params)?),
            "lines.disable" => Call::LinesDisable(read(method, params)?),
            "lines.add" => Call::LinesAdd(read(method, params)?),
            "lines.edit" => Call::LinesEdit(read(method, params)?),
            "lines.remove" => Call::LinesRemove(read(method, params)?),
            "lines.capture" => Call::LinesCapture(read(method, params)?),
            "sip.status" => {
                read::<NoParams>(method, params)?;
                Call::SipStatus
            }
            "sip.reregister" => Call::SipReregister(read(method, params)?),
            "huntgroups.list" => {
                read::<NoParams>(method, params)?;
                Call::HuntGroupsList
            }
            "huntgroups.add" => Call::HuntGroupsAdd(read(method, params)?),
            "huntgroups.edit" => Call::HuntGroupsEdit(read(method, params)?),
            "huntgroups.remove" => Call::HuntGroupsRemove(read(method, params)?),
            "routes.list" => {
                read::<NoParams>(method, params)?;
                Call::RoutesList
            }
            "routes.add" => Call::RoutesAdd(read(method, params)?),
            "routes.edit" => Call::RoutesEdit(read(method, params)?),
            "routes.remove" => Call::RoutesRemove(read(method, params)?),
            "routes.move" => Call::RoutesMove(read(method, params)?),
            "calls.list" => Call::CallsList(read(method, params)?),
            "config.reload" => {
                read::<NoParams>(method, params)?;
                Call::ConfigReload
            }
            "config.get" => {
                read::<NoParams>(method, params)?;
                Call::ConfigGet
            }
            "config.set" => Call::ConfigSet(read(method, params)?),
            "stats.get" => {
                read::<NoParams>(method, params)?;
                Call::StatsGet
            }
            "log.tail" => Call::LogTail(read(method, params)?),
            "ports.list" => {
                read::<NoParams>(method, params)?;
                Call::PortsList
            }
            "subscribe" => Call::Subscribe(read(method, params)?),
            "unsubscribe" => Call::Unsubscribe(read(method, params)?),
            other => return Err(message("RMD2002", &[other])),
        })
    }
}
