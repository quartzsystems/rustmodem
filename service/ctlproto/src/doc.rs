// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control API described as data, and `docs/CONTROL_API.md` rendered
//! from it.
//!
//! The tables here are the one description of the API for people. The tests
//! of this crate check them against the serde types: every method, every
//! parameter, every field of every result and event, and every value of
//! every enumeration must be here, and nothing may be here that the types
//! do not have. They also check that `docs/CONTROL_API.md` is exactly what
//! [`render`] produces; run the tests with `UPDATE_DOCS=1` to rewrite it.

use std::fmt::Write;

use crate::wire::{MAX_MESSAGE_BYTES, PROTOCOL};

/// One parameter of a method, or one field of a type.
pub struct FieldDoc {
    /// The JSON key.
    pub name: &'static str,
    /// The JSON type, or the name of a type or enumeration described here.
    pub kind: &'static str,
    /// Whether the key must be present.
    pub required: bool,
    /// What it means.
    pub text: &'static str,
}

/// One method.
pub struct MethodDoc {
    /// The method name.
    pub name: &'static str,
    /// What it does.
    pub summary: &'static str,
    /// Its parameters.
    pub params: &'static [FieldDoc],
    /// The type of its result.
    pub result: &'static str,
    /// The errors particular to it, by message id.
    pub errors: &'static [&'static str],
    /// Example parameters, as JSON, using every parameter.
    pub example: &'static str,
}

/// One event.
pub struct EventDoc {
    /// The event name.
    pub name: &'static str,
    /// The type of its data.
    pub data: &'static str,
    /// When it is sent.
    pub text: &'static str,
}

/// One object type.
pub struct TypeDoc {
    /// The name used for the type in this documentation.
    pub name: &'static str,
    /// What it is.
    pub text: &'static str,
    /// Its fields.
    pub fields: &'static [FieldDoc],
}

/// One enumeration: a string with a fixed set of values.
pub struct EnumDoc {
    /// The name used for the enumeration in this documentation.
    pub name: &'static str,
    /// What it is.
    pub text: &'static str,
    /// Its values and what each means.
    pub values: &'static [(&'static str, &'static str)],
}

const fn required(name: &'static str, kind: &'static str, text: &'static str) -> FieldDoc {
    FieldDoc {
        name,
        kind,
        required: true,
        text,
    }
}

const fn optional(name: &'static str, kind: &'static str, text: &'static str) -> FieldDoc {
    FieldDoc {
        name,
        kind,
        required: false,
        text,
    }
}

const LINE_ID: &[FieldDoc] = &[required("id", "integer", "The line id.")];

/// The errors any request can get.
pub const COMMON_ERRORS: &[&str] = &["RMD2002", "RMD2003", "RMD2004", "RMD2010", "RMD2014"];

/// The errors of the hello exchange.
pub const HELLO_ERRORS: &[&str] = &["RMD2001", "RMD2005"];

/// The errors a change to the configuration can get.
const CONFIG_ERRORS: &[&str] = &["RMD0011", "RMD0014", "RMD0017", "RMD0018", "RMD0019"];

/// Every method.
pub const METHODS: &[MethodDoc] = &[
    MethodDoc {
        name: "service.info",
        summary: "Reports the service's version, protocol, pipe name, configuration path, \
                  number of lines and how long it has been running.",
        params: &[],
        result: "ServiceInfo",
        errors: &[],
        example: "{}",
    },
    MethodDoc {
        name: "lines.list",
        summary: "Lists every line, in id order.",
        params: &[],
        result: "LineList",
        errors: &[],
        example: "{}",
    },
    MethodDoc {
        name: "lines.get",
        summary: "Reports one line.",
        params: LINE_ID,
        result: "LineInfo",
        errors: &["RMD1001"],
        example: r#"{"id":1}"#,
    },
    MethodDoc {
        name: "lines.answer",
        summary: "Answers the call ringing on a line, as `ATA` from the host would. The \
                  result is the line afterwards.",
        params: LINE_ID,
        result: "LineInfo",
        errors: &["RMD1001", "RMD1002", "RMD1003"],
        example: r#"{"id":1}"#,
    },
    MethodDoc {
        name: "lines.hangup",
        summary: "Hangs up the call on a line, or turns away the call ringing on it. The \
                  host is told `NO CARRIER`. The result is the line afterwards.",
        params: LINE_ID,
        result: "LineInfo",
        errors: &["RMD1001", "RMD1004"],
        example: r#"{"id":1}"#,
    },
    MethodDoc {
        name: "lines.reset",
        summary: "Resets a line: hangs up any call and returns the modem's settings to \
                  their defaults. The result is the line afterwards.",
        params: LINE_ID,
        result: "LineInfo",
        errors: &["RMD1001"],
        example: r#"{"id":1}"#,
    },
    MethodDoc {
        name: "lines.enable",
        summary: "Puts a line in service. The change is written to the configuration \
                  file. The result is the line afterwards.",
        params: LINE_ID,
        result: "LineInfo",
        errors: &["RMD0011", "RMD0019"],
        example: r#"{"id":1}"#,
    },
    MethodDoc {
        name: "lines.disable",
        summary: "Takes a line out of service: any call on it is hung up and its port is \
                  released. The change is written to the configuration file. The result \
                  is the line afterwards.",
        params: LINE_ID,
        result: "LineInfo",
        errors: &["RMD0011", "RMD0019"],
        example: r#"{"id":1}"#,
    },
    MethodDoc {
        name: "lines.add",
        summary: "Adds a line. The change is written to the configuration file. The \
                  result is the new line.",
        params: &[
            required("id", "integer", "The line id, 1 to 999, not in use."),
            optional("name", "string", "A name for people."),
            optional(
                "backend",
                "string",
                "The serial backend. Default `com0com`.",
            ),
            required("port", "string", "The serial port, for example `CNCB0`."),
            optional(
                "enabled",
                "boolean",
                "Whether the line is in service. Default true.",
            ),
            optional(
                "auto_answer_rings",
                "integer",
                "The initial S0, 0 to 255. Default 0.",
            ),
            optional(
                "unknown_command",
                "string",
                "What the line answers to an unknown AT command: `ok` or `error`. Default `ok`.",
            ),
        ],
        result: "LineInfo",
        errors: &["RMD0011", "RMD0014", "RMD0017", "RMD0018", "RMD0020"],
        example: r#"{"id":3,"name":"Line 3","backend":"com0com","port":"CNCB2","enabled":true,"auto_answer_rings":0,"unknown_command":"ok"}"#,
    },
    MethodDoc {
        name: "lines.edit",
        summary: "Changes the keys of a line that are given and leaves the others alone. \
                  Changing the port, or disabling the line, hangs up any call on it. The \
                  change is written to the configuration file. The result is the line \
                  afterwards.",
        params: &[
            required("id", "integer", "The line to change."),
            optional(
                "name",
                "string",
                "A new name. An empty string removes the name.",
            ),
            optional("backend", "string", "A new serial backend."),
            optional("port", "string", "A new serial port."),
            optional("enabled", "boolean", "Whether the line is in service."),
            optional(
                "auto_answer_rings",
                "integer",
                "A new initial S0, 0 to 255.",
            ),
            optional("unknown_command", "string", "`ok` or `error`."),
        ],
        result: "LineInfo",
        errors: CONFIG_ERRORS,
        example: r#"{"id":3,"name":"Front desk","backend":"com0com","port":"CNCB5","enabled":false,"auto_answer_rings":2,"unknown_command":"error"}"#,
    },
    MethodDoc {
        name: "lines.remove",
        summary: "Removes a line: any call on it is hung up and its port is released. \
                  The change is written to the configuration file.",
        params: LINE_ID,
        result: "Done",
        errors: &["RMD0011", "RMD0019"],
        example: r#"{"id":3}"#,
    },
    MethodDoc {
        name: "calls.list",
        summary: "Lists the calls that have ended, oldest first. The service remembers \
                  the last 1000.",
        params: &[optional(
            "last",
            "integer",
            "Return at most this many, the most recent. Default all.",
        )],
        result: "CallList",
        errors: &[],
        example: r#"{"last":20}"#,
    },
    MethodDoc {
        name: "config.reload",
        summary: "Reads the configuration file again and makes the lines match it. \
                  Lines whose configuration has not changed are not disturbed. Changes \
                  to `[service]` and `[control]` take effect when the service next \
                  starts. If the file is not valid nothing changes, and the error is \
                  the first problem found.",
        params: &[],
        result: "Reloaded",
        errors: &[
            "RMD0010", "RMD0012", "RMD0013", "RMD0014", "RMD0015", "RMD0016", "RMD0017", "RMD0018",
        ],
        example: "{}",
    },
    MethodDoc {
        name: "subscribe",
        summary: "Starts sending the client events. The result is everything the client \
                  is now subscribed to.",
        params: &[optional(
            "events",
            "array of string",
            "The event names to add. Default every event.",
        )],
        result: "Subscription",
        errors: &[],
        example: r#"{"events":["line.state","call.ended"]}"#,
    },
    MethodDoc {
        name: "unsubscribe",
        summary: "Stops sending the client events. The result is everything the client \
                  is still subscribed to.",
        params: &[optional(
            "events",
            "array of string",
            "The event names to remove. Default every event.",
        )],
        result: "Subscription",
        errors: &[],
        example: r#"{"events":["log"]}"#,
    },
];

/// Every event.
pub const EVENTS: &[EventDoc] = &[
    EventDoc {
        name: "line.state",
        data: "LineInfo",
        text: "A line changed state, or was added. The data is the whole line, with its \
               counters and any call in progress.",
    },
    EventDoc {
        name: "call.started",
        data: "CallEvent",
        text: "A line dialled, or a call was offered to a line.",
    },
    EventDoc {
        name: "call.connected",
        data: "CallEvent",
        text: "The modems of a call connected.",
    },
    EventDoc {
        name: "call.ended",
        data: "CallRecord",
        text: "A call ended.",
    },
    EventDoc {
        name: "log",
        data: "LogEvent",
        text: "A line was written to the service log, at the service's log level.",
    },
];

/// Every object type.
pub const TYPES: &[TypeDoc] = &[
    TypeDoc {
        name: "Hello",
        text: "The body of the `hello` message each side sends first.",
        fields: &[
            required(
                "protocol",
                "integer",
                "The protocol number the sender speaks.",
            ),
            required("version", "string", "The sender's version, for people."),
        ],
    },
    TypeDoc {
        name: "Error",
        text: "An `RMD` message.",
        fields: &[
            required("msgid", "string", "The message id: `RMD` and four digits."),
            required("text", "string", "The first-level text."),
            required("cause", "string", "The second-level text: the cause."),
            required("recovery", "string", "The second-level text: the recovery."),
        ],
    },
    TypeDoc {
        name: "ServiceInfo",
        text: "The result of `service.info`.",
        fields: &[
            required("version", "string", "The service version."),
            required(
                "protocol",
                "integer",
                "The protocol number the service speaks.",
            ),
            required("pipe", "string", "The name of the control pipe."),
            required(
                "config_path",
                "string",
                "The path of the configuration file.",
            ),
            required("lines", "integer", "How many lines the service has."),
            required(
                "started_ms",
                "integer",
                "When the service started, in milliseconds since the Unix epoch.",
            ),
            required(
                "uptime_ms",
                "integer",
                "How long the service has been running, in milliseconds.",
            ),
        ],
    },
    TypeDoc {
        name: "LineList",
        text: "The result of `lines.list`.",
        fields: &[required(
            "lines",
            "array of LineInfo",
            "Every line, in id order.",
        )],
    },
    TypeDoc {
        name: "LineInfo",
        text: "Everything about one line.",
        fields: &[
            required("id", "integer", "The line id."),
            optional("name", "string", "The line's name, if it has one."),
            required(
                "backend",
                "string",
                "The serial backend, for example `com0com`.",
            ),
            required("port", "string", "The serial port, for example `CNCB0`."),
            required(
                "enabled",
                "boolean",
                "Whether the line is in service by configuration.",
            ),
            required("state", "LineState", "The state of the line."),
            required("auto_answer_rings", "integer", "The configured initial S0."),
            required(
                "unknown_command",
                "string",
                "What the line answers to an unknown AT command: `ok` or `error`.",
            ),
            required("counters", "Counters", "The counters of the line."),
            optional("call", "CallInfo", "The call in progress, if there is one."),
            optional(
                "error",
                "Error",
                "Why the serial port is not open, if it should be and is not.",
            ),
        ],
    },
    TypeDoc {
        name: "Counters",
        text: "The counters of a line since the service started.",
        fields: &[
            required("calls_out", "integer", "Calls the line dialled."),
            required("calls_in", "integer", "Calls offered to the line."),
            required("connects", "integer", "Calls that connected."),
            required(
                "bytes_tx",
                "integer",
                "Data bytes from the host to the far end.",
            ),
            required(
                "bytes_rx",
                "integer",
                "Data bytes from the far end to the host.",
            ),
        ],
    },
    TypeDoc {
        name: "CallInfo",
        text: "A call in progress.",
        fields: &[
            required(
                "id",
                "integer",
                "The call id, unique since the service started.",
            ),
            required("direction", "Direction", "Which way the call went."),
            required(
                "number",
                "string",
                "The number dialled, or for an incoming call the caller.",
            ),
            required(
                "started_ms",
                "integer",
                "When the call started, in milliseconds since the Unix epoch.",
            ),
            optional(
                "connected_ms",
                "integer",
                "When the call connected, if it has.",
            ),
            optional(
                "connect",
                "ConnectInfo",
                "What is known about the connection, once connected.",
            ),
            required(
                "bytes_tx",
                "integer",
                "Data bytes from the host to the far end so far.",
            ),
            required(
                "bytes_rx",
                "integer",
                "Data bytes from the far end to the host so far.",
            ),
        ],
    },
    TypeDoc {
        name: "CallRecord",
        text: "A call that has ended. The same object is written, one a line, to \
               `calls.jsonl` in the log directory.",
        fields: &[
            required(
                "id",
                "integer",
                "The call id, unique since the service started.",
            ),
            required("line", "integer", "The line the call was on."),
            required("direction", "Direction", "Which way the call went."),
            required(
                "number",
                "string",
                "The number dialled, or for an incoming call the caller.",
            ),
            required(
                "started_ms",
                "integer",
                "When the call started, in milliseconds since the Unix epoch.",
            ),
            optional(
                "connected_ms",
                "integer",
                "When the call connected, if it did.",
            ),
            required("ended_ms", "integer", "When the call ended."),
            required("reason", "EndReason", "Why the call ended."),
            optional(
                "connect",
                "ConnectInfo",
                "What is known about the connection, if the call connected.",
            ),
            required(
                "bytes_tx",
                "integer",
                "Data bytes from the host to the far end.",
            ),
            required(
                "bytes_rx",
                "integer",
                "Data bytes from the far end to the host.",
            ),
        ],
    },
    TypeDoc {
        name: "ConnectInfo",
        text: "What is known about a connection. Phase 0 reports rate 0 and nothing else.",
        fields: &[
            required(
                "rate",
                "integer",
                "The line rate in bit/s; 0 means not known.",
            ),
            optional("carrier", "string", "The carrier (modulation) name."),
            optional("protocol", "string", "The error-control protocol name."),
            optional("compression", "string", "The data compression name."),
        ],
    },
    TypeDoc {
        name: "CallList",
        text: "The result of `calls.list`.",
        fields: &[required(
            "calls",
            "array of CallRecord",
            "The calls that have ended, oldest first.",
        )],
    },
    TypeDoc {
        name: "CallEvent",
        text: "The data of `call.started` and `call.connected`.",
        fields: &[
            required("line", "integer", "The line the call is on."),
            required("call", "CallInfo", "The call."),
        ],
    },
    TypeDoc {
        name: "LogEvent",
        text: "The data of `log`: one line of the service log.",
        fields: &[
            required(
                "time_ms",
                "integer",
                "When, in milliseconds since the Unix epoch.",
            ),
            required("level", "LogLevel", "How serious."),
            required(
                "source",
                "string",
                "Where from: `Line01`, `CTL`, `SVC`, `POOL`.",
            ),
            required("text", "string", "The text."),
        ],
    },
    TypeDoc {
        name: "Reloaded",
        text: "The result of `config.reload`.",
        fields: &[required(
            "lines",
            "integer",
            "How many lines the configuration has.",
        )],
    },
    TypeDoc {
        name: "Subscription",
        text: "The result of `subscribe` and `unsubscribe`.",
        fields: &[required(
            "events",
            "array of string",
            "The events the client is now subscribed to.",
        )],
    },
    TypeDoc {
        name: "Done",
        text: "The result of a method that has nothing to report: an empty object.",
        fields: &[],
    },
];

/// Every enumeration.
pub const ENUMS: &[EnumDoc] = &[
    EnumDoc {
        name: "LineState",
        text: "The state of a line.",
        values: &[
            ("disabled", "The line is out of service by configuration."),
            (
                "not_ready",
                "The serial port is closed, or the host has DTR off.",
            ),
            ("idle", "In service, with no call."),
            ("ringing", "A call is being offered to the line."),
            ("handshaking", "A call is being set up."),
            ("online", "A call is connected."),
        ],
    },
    EnumDoc {
        name: "Direction",
        text: "Which way a call went.",
        values: &[
            ("outgoing", "The line dialled out."),
            ("incoming", "The call was offered to the line."),
        ],
    },
    EnumDoc {
        name: "EndReason",
        text: "Why a call ended.",
        values: &[
            ("hangup", "The host hung up with an `H` command."),
            ("dtr", "The host dropped DTR."),
            (
                "aborted",
                "The host sent a character while the call was being set up.",
            ),
            ("no_answer", "Nothing answered in time."),
            ("busy", "The number dialled was busy."),
            ("no_dialtone", "There was no dial tone."),
            (
                "remote",
                "The far end cleared the call, or the carrier was lost.",
            ),
            ("abandoned", "The caller gave up before the line answered."),
            (
                "failed",
                "The call could not be set up or carried on: the number is not a line in \
                 service, the line called went away, or the line's serial port failed.",
            ),
            ("reset", "The host reset the modem with a `Z` command."),
            (
                "operator",
                "The operator hung the line up, reset it, disabled it or removed it.",
            ),
            ("shutdown", "The service stopped."),
        ],
    },
    EnumDoc {
        name: "LogLevel",
        text: "How serious a log line is.",
        values: &[
            ("error", "Something failed."),
            ("warning", "Something is wrong but the service went on."),
            ("info", "Something happened."),
            (
                "debug",
                "Detail, including every AT command line and result code.",
            ),
        ],
    },
];

/// Every message id the documentation lists as an error of the API.
pub fn error_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = HELLO_ERRORS
        .iter()
        .chain(COMMON_ERRORS)
        .chain(METHODS.iter().flat_map(|method| method.errors))
        .copied()
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn field_table(out: &mut String, heading: &str, fields: &[FieldDoc]) {
    let _ = writeln!(out, "| {heading} | Type | Required | Meaning |");
    let _ = writeln!(out, "|---|---|---|---|");
    for field in fields {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} |",
            field.name,
            field.kind,
            if field.required { "yes" } else { "no" },
            field.text
        );
    }
}

fn id_list(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders `docs/CONTROL_API.md`.
pub fn render() -> String {
    let mut out = String::new();
    let o = &mut out;
    let _ = writeln!(o, "# The rustmodem control API");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "This file is generated from `service/ctlproto/src/doc.rs` and checked against\n\
         the API's types by that crate's tests. Do not edit it by hand: change the\n\
         tables there and run `UPDATE_DOCS=1 cargo test -p ctlproto`."
    );
    let _ = writeln!(o);
    let _ = writeln!(o, "## Transport");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "The service listens on a Windows named pipe, `\\\\.\\pipe\\rustmodem-control` by\n\
         default; the name is `control.pipe` in the configuration. The pipe is open to\n\
         SYSTEM and Administrators only, so a client must run elevated. Remote clients\n\
         are refused.\n\
         \n\
         Each message is one JSON object on one line, in UTF-8, ended by a line feed.\n\
         A message may be at most {MAX_MESSAGE_BYTES} bytes long."
    );
    let _ = writeln!(o);
    let _ = writeln!(o, "## Hello");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "The first message each way is `hello`. The client sends its hello first:\n\
         \n\
         ```json\n\
         {{\"hello\":{{\"protocol\":{PROTOCOL},\"version\":\"0.0.1\"}}}}\n\
         ```\n\
         \n\
         The service answers with its own, in the same form. This version of the\n\
         service speaks protocol {PROTOCOL}. If the client's protocol is another, the service\n\
         answers with error `RMD2001` and closes the connection. If the first message\n\
         is not a hello, the service answers with error `RMD2005` and closes the\n\
         connection. Both errors are sent with a null `id`."
    );
    let _ = writeln!(o);
    let _ = writeln!(o, "## Requests, responses and events");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "A request names a method. The `id` is chosen by the client and comes back in\n\
         the answer; `params` may be left out when the method takes none.\n\
         \n\
         ```json\n\
         {{\"id\":7,\"method\":\"lines.get\",\"params\":{{\"id\":1}}}}\n\
         ```\n\
         \n\
         The answer is a result or an error:\n\
         \n\
         ```json\n\
         {{\"id\":7,\"result\":{{ ... }}}}\n\
         {{\"id\":7,\"error\":{{\"msgid\":\"RMD1001\",\"text\":\"Line 1 does not exist.\",\"cause\":\"...\",\"recovery\":\"...\"}}}}\n\
         ```\n\
         \n\
         An error is always an `RMD` message: its id, its text, and its cause and\n\
         recovery. When a message cannot be read as a request at all, the error has a\n\
         null `id`.\n\
         \n\
         Events are sent only to a client that has subscribed:\n\
         \n\
         ```json\n\
         {{\"event\":\"line.state\",\"data\":{{ ... }}}}\n\
         ```\n\
         \n\
         Answers come in the order of their requests. Events may come between them.\n\
         \n\
         A client that subscribes must keep reading. The service queues what a client\n\
         has not read yet. When the queue is full, the oldest `log` events are dropped\n\
         first; every other event reports state and is never dropped. A client that\n\
         falls further behind than that is disconnected (`RMD2006` in the service\n\
         log), and can connect again and ask for the current state."
    );
    let _ = writeln!(o);
    let _ = writeln!(o, "## Methods");
    let _ = writeln!(o);
    let _ = writeln!(o, "| Method | Result | Summary |");
    let _ = writeln!(o, "|---|---|---|");
    for method in METHODS {
        let first = method.summary.split(". ").next().unwrap_or(method.summary);
        let _ = writeln!(
            o,
            "| [`{}`](#{}) | {} | {}. |",
            method.name,
            method.name.replace('.', ""),
            method.result,
            first.trim_end_matches('.')
        );
    }
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "Every request can also get one of these errors: {}.",
        id_list(COMMON_ERRORS)
    );
    for method in METHODS {
        let _ = writeln!(o);
        let _ = writeln!(o, "### `{}`", method.name);
        let _ = writeln!(o);
        let _ = writeln!(o, "{}", method.summary);
        let _ = writeln!(o);
        if method.params.is_empty() {
            let _ = writeln!(o, "Parameters: none.");
        } else {
            field_table(o, "Parameter", method.params);
        }
        let _ = writeln!(o);
        let _ = writeln!(o, "Result: {}.", method.result);
        if !method.errors.is_empty() {
            let _ = writeln!(o);
            let _ = writeln!(o, "Errors: {}.", id_list(method.errors));
        }
        let _ = writeln!(o);
        let _ = writeln!(o, "```json");
        if method.example == "{}" {
            let _ = writeln!(o, "{{\"id\":1,\"method\":\"{}\"}}", method.name);
        } else {
            let _ = writeln!(
                o,
                "{{\"id\":1,\"method\":\"{}\",\"params\":{}}}",
                method.name, method.example
            );
        }
        let _ = writeln!(o, "```");
    }
    let _ = writeln!(o);
    let _ = writeln!(o, "## Events");
    let _ = writeln!(o);
    let _ = writeln!(o, "| Event | Data | When |");
    let _ = writeln!(o, "|---|---|---|");
    for event in EVENTS {
        let _ = writeln!(o, "| `{}` | {} | {} |", event.name, event.data, event.text);
    }
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "Removing a line sends no event of its own. A client that changes the lines\n\
         should list them again afterwards."
    );
    let _ = writeln!(o);
    let _ = writeln!(o, "## Types");
    for kind in TYPES {
        let _ = writeln!(o);
        let _ = writeln!(o, "### {}", kind.name);
        let _ = writeln!(o);
        let _ = writeln!(o, "{}", kind.text);
        if !kind.fields.is_empty() {
            let _ = writeln!(o);
            field_table(o, "Field", kind.fields);
        }
    }
    let _ = writeln!(o);
    let _ = writeln!(o, "## Values");
    for kind in ENUMS {
        let _ = writeln!(o);
        let _ = writeln!(o, "### {}", kind.name);
        let _ = writeln!(o);
        let _ = writeln!(o, "{} A string, one of:", kind.text);
        let _ = writeln!(o);
        let _ = writeln!(o, "| Value | Meaning |");
        let _ = writeln!(o, "|---|---|");
        for (value, text) in kind.values {
            let _ = writeln!(o, "| `{value}` | {text} |");
        }
    }
    let _ = writeln!(o);
    let _ = writeln!(o, "## Errors");
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "The errors the API returns, from the message catalogue\n\
         (`service/msg/messages.toml`). `&1`, `&2` and so on stand for the values the\n\
         service fills in."
    );
    let _ = writeln!(o);
    let _ = writeln!(o, "| Id | Text | Cause | Recovery |");
    let _ = writeln!(o, "|---|---|---|---|");
    let wanted = error_ids();
    for entry in msg::entries() {
        if wanted.contains(&entry.id.as_str()) {
            let _ = writeln!(
                o,
                "| `{}` | {} | {} | {} |",
                entry.id, entry.text, entry.cause, entry.recovery
            );
        }
    }
    out
}
