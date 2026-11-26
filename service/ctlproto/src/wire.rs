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

use crate::model::{CallRecord, ErrorInfo, Event, LineInfo};

/// The protocol number this version speaks.
pub const PROTOCOL: u32 = 1;

/// The longest control message accepted, in bytes, its line feed included.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// The name of every event, in the order the documentation lists them.
pub const EVENT_NAMES: [&str; 5] = [
    "line.state",
    "call.started",
    "call.connected",
    "call.ended",
    "log",
];

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
    /// `calls.list`
    CallsList(CallsQuery),
    /// `config.reload`
    ConfigReload,
    /// `subscribe`
    Subscribe(EventSelection),
    /// `unsubscribe`
    Unsubscribe(EventSelection),
}

impl Call {
    /// The name of every method, in the order the documentation lists them.
    pub const METHODS: [&'static str; 15] = [
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
        "calls.list",
        "config.reload",
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
            Call::CallsList(_) => "calls.list",
            Call::ConfigReload => "config.reload",
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
            Call::ServiceInfo | Call::LinesList | Call::ConfigReload => value(&NoParams {}),
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
            "calls.list" => Call::CallsList(read(method, params)?),
            "config.reload" => {
                read::<NoParams>(method, params)?;
                Call::ConfigReload
            }
            "subscribe" => Call::Subscribe(read(method, params)?),
            "unsubscribe" => Call::Unsubscribe(read(method, params)?),
            other => return Err(message("RMD2002", &[other])),
        })
    }
}
