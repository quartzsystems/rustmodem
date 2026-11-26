// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control API's description checked against its types, and
//! `docs/CONTROL_API.md` checked against the description.

use std::collections::BTreeSet;
use std::path::Path;

use ctlproto::doc::{self, FieldDoc};
use ctlproto::{
    Call, CallEvent, CallInfo, CallList, CallRecord, ClientFrame, ConnectInfo, Counters, Direction,
    Done, EVENT_NAMES, EndReason, ErrorInfo, Event, Hello, LineInfo, LineList, LineState, LogEvent,
    LogLevel, PROTOCOL, Reloaded, RequestFrame, ServerFrame, ServiceInfo, Subscription,
    decode_client, decode_server, encode,
};
use serde::Serialize;
use serde_json::{Value, json};

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{value} is not an object"))
        .keys()
        .cloned()
        .collect()
}

fn names(fields: &[FieldDoc], required_only: bool) -> BTreeSet<String> {
    fields
        .iter()
        .filter(|f| f.required || !required_only)
        .map(|f| f.name.to_string())
        .collect()
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

// --------------------------------------------------------------------------
// Samples: one value of every type, with every optional field present
// (`full`) and with none (`bare`).
// --------------------------------------------------------------------------

fn connect(full: bool) -> ConnectInfo {
    ConnectInfo {
        rate: 2400,
        carrier: full.then(|| "V.22bis".to_string()),
        protocol: full.then(|| "LAPM".to_string()),
        compression: full.then(|| "V.42bis".to_string()),
    }
}

fn call_info(full: bool) -> CallInfo {
    CallInfo {
        id: 7,
        direction: Direction::Outgoing,
        number: "2".to_string(),
        started_ms: 1_800_000_000_000,
        connected_ms: full.then_some(1_800_000_003_000),
        connect: full.then(|| connect(full)),
        bytes_tx: 10,
        bytes_rx: 20,
    }
}

fn call_record(full: bool) -> CallRecord {
    CallRecord {
        id: 7,
        line: 1,
        direction: Direction::Incoming,
        number: "1".to_string(),
        started_ms: 1_800_000_000_000,
        connected_ms: full.then_some(1_800_000_003_000),
        ended_ms: 1_800_000_060_000,
        reason: EndReason::Remote,
        connect: full.then(|| connect(full)),
        bytes_tx: 10,
        bytes_rx: 20,
    }
}

fn error_info() -> ErrorInfo {
    ErrorInfo::from(msg::message("RMD1001", &["9"]))
}

fn line_info(full: bool) -> LineInfo {
    LineInfo {
        id: 1,
        name: full.then(|| "Line 1".to_string()),
        backend: "com0com".to_string(),
        port: "CNCB0".to_string(),
        enabled: true,
        state: LineState::Online,
        auto_answer_rings: 0,
        unknown_command: "ok".to_string(),
        counters: Counters {
            calls_out: 1,
            calls_in: 2,
            connects: 3,
            bytes_tx: 4,
            bytes_rx: 5,
        },
        call: full.then(|| call_info(full)),
        error: full.then(error_info),
    }
}

fn log_event() -> LogEvent {
    LogEvent {
        time_ms: 1_800_000_000_000,
        level: LogLevel::Info,
        source: "Line01".to_string(),
        text: "RMD1005: Line 1 state is now idle.".to_string(),
    }
}

/// A sample of the documented type `name`.
fn sample(name: &str, full: bool) -> Value {
    match name {
        "Hello" => to_value(&Hello {
            protocol: PROTOCOL,
            version: "0.0.1".to_string(),
        }),
        "Error" => to_value(&error_info()),
        "ServiceInfo" => to_value(&ServiceInfo {
            version: "0.0.1".to_string(),
            protocol: PROTOCOL,
            pipe: "rustmodem-control".to_string(),
            config_path: r"C:\ProgramData\Quartz Systems\rustmodem\rustmodem.toml".to_string(),
            lines: 2,
            started_ms: 1_800_000_000_000,
            uptime_ms: 60_000,
        }),
        "LineList" => to_value(&LineList {
            lines: vec![line_info(full)],
        }),
        "LineInfo" => to_value(&line_info(full)),
        "Counters" => to_value(&line_info(full).counters),
        "CallInfo" => to_value(&call_info(full)),
        "CallRecord" => to_value(&call_record(full)),
        "ConnectInfo" => to_value(&connect(full)),
        "CallList" => to_value(&CallList {
            calls: vec![call_record(full)],
        }),
        "CallEvent" => to_value(&CallEvent {
            line: 1,
            call: call_info(full),
        }),
        "LogEvent" => to_value(&log_event()),
        "Reloaded" => to_value(&Reloaded { lines: 2 }),
        "Subscription" => to_value(&Subscription {
            events: vec!["log".to_string()],
        }),
        "Done" => to_value(&Done {}),
        other => panic!("no sample for documented type {other}"),
    }
}

/// A sample of every event, in the order of `EVENT_NAMES`, with the name of
/// the documented type of its data.
fn event_samples(full: bool) -> Vec<(Event, &'static str)> {
    vec![
        (Event::LineState(Box::new(line_info(full))), "LineInfo"),
        (
            Event::CallStarted(CallEvent {
                line: 1,
                call: call_info(false),
            }),
            "CallEvent",
        ),
        (
            Event::CallConnected(CallEvent {
                line: 1,
                call: call_info(full),
            }),
            "CallEvent",
        ),
        (Event::CallEnded(call_record(full)), "CallRecord"),
        (Event::Log(log_event()), "LogEvent"),
    ]
}

// --------------------------------------------------------------------------
// Methods
// --------------------------------------------------------------------------

#[test]
fn every_method_is_documented_and_nothing_else() {
    let documented: Vec<&str> = doc::METHODS.iter().map(|m| m.name).collect();
    assert_eq!(documented, Call::METHODS);
}

#[test]
fn every_methods_parameters_are_the_documented_ones() {
    for method in doc::METHODS {
        let example: Value = serde_json::from_str(method.example)
            .unwrap_or_else(|e| panic!("{}: example is not JSON: {e}", method.name));
        // The example uses every parameter.
        assert_eq!(
            keys(&example),
            names(method.params, false),
            "{}: the example does not use exactly the documented parameters",
            method.name
        );
        let call = Call::from_parts(method.name, example.clone())
            .unwrap_or_else(|m| panic!("{}: example refused: {m}", method.name));
        assert_eq!(call.method(), method.name);
        // What the type writes back is exactly the documented parameters:
        // a field the documentation lacks would show up here.
        assert_eq!(
            keys(&call.params()),
            names(method.params, false),
            "{}: the type has other fields than the documentation",
            method.name
        );

        let object = example.as_object().unwrap();
        for field in method.params {
            let mut without = object.clone();
            without.remove(field.name);
            let result = Call::from_parts(method.name, Value::Object(without));
            if field.required {
                let refused = result.expect_err("a missing required parameter");
                assert_eq!(refused.id, "RMD2003", "{}.{}", method.name, field.name);
                assert!(refused.text.contains(method.name));
            } else {
                assert!(
                    result.is_ok(),
                    "{}.{} is documented as optional",
                    method.name,
                    field.name
                );
            }
        }

        let mut extra = object.clone();
        extra.insert("no_such_parameter".to_string(), json!(1));
        let refused = Call::from_parts(method.name, Value::Object(extra)).unwrap_err();
        assert_eq!(refused.id, "RMD2003", "{}", method.name);
    }
}

#[test]
fn every_method_round_trips_through_a_request_frame() {
    for (index, method) in doc::METHODS.iter().enumerate() {
        let example: Value = serde_json::from_str(method.example).unwrap();
        let call = Call::from_parts(method.name, example).unwrap();
        let line = encode(&ClientFrame::Request(call.request(index as u64)));
        assert!(!line.contains('\n'));
        let ClientFrame::Request(frame) = decode_client(&line).unwrap() else {
            panic!("{}: not read back as a request", method.name);
        };
        assert_eq!(frame.id, index as u64);
        assert_eq!(frame.method, method.name);
        assert_eq!(Call::from_parts(&frame.method, frame.params).unwrap(), call);
    }
}

#[test]
fn parameters_may_be_left_out_when_a_method_takes_none() {
    let line = r#"{"id":4,"method":"lines.list"}"#;
    let ClientFrame::Request(frame) = decode_client(line).unwrap() else {
        panic!("not a request");
    };
    assert_eq!(
        Call::from_parts(&frame.method, frame.params).unwrap(),
        Call::LinesList
    );
    assert_eq!(
        Call::from_parts("calls.list", Value::Null)
            .unwrap()
            .method(),
        "calls.list"
    );
    assert_eq!(
        Call::from_parts("lines.get", Value::Null).unwrap_err().id,
        "RMD2003"
    );
    assert_eq!(
        Call::from_parts("lines.get", json!({"id": "one"}))
            .unwrap_err()
            .id,
        "RMD2003"
    );
    assert_eq!(
        Call::from_parts("lines.get", json!([1])).unwrap_err().id,
        "RMD2003"
    );
}

#[test]
fn an_unknown_method_is_message_2002() {
    let refused = Call::from_parts("lines.explode", json!({})).unwrap_err();
    assert_eq!(refused.id, "RMD2002");
    assert!(refused.text.contains("lines.explode"));
}

#[test]
fn every_methods_result_is_a_documented_type() {
    let types: BTreeSet<&str> = doc::TYPES.iter().map(|t| t.name).collect();
    for method in doc::METHODS {
        assert!(types.contains(method.result), "{}", method.name);
    }
}

// --------------------------------------------------------------------------
// Types and values
// --------------------------------------------------------------------------

#[test]
fn every_types_fields_are_the_documented_ones() {
    for kind in doc::TYPES {
        assert_eq!(
            keys(&sample(kind.name, true)),
            names(kind.fields, false),
            "{}: every field present",
            kind.name
        );
        assert_eq!(
            keys(&sample(kind.name, false)),
            names(kind.fields, true),
            "{}: only the required fields present",
            kind.name
        );
    }
}

#[test]
fn every_field_kind_is_a_json_type_or_something_documented() {
    let mut known: BTreeSet<String> = ["integer", "string", "boolean"]
        .iter()
        .map(|k| k.to_string())
        .collect();
    known.extend(doc::TYPES.iter().map(|t| t.name.to_string()));
    known.extend(doc::ENUMS.iter().map(|e| e.name.to_string()));
    let all_fields = doc::TYPES
        .iter()
        .flat_map(|t| t.fields)
        .chain(doc::METHODS.iter().flat_map(|m| m.params));
    for field in all_fields {
        let kind = field.kind.strip_prefix("array of ").unwrap_or(field.kind);
        assert!(
            known.contains(kind),
            "{}: unknown kind {}",
            field.name,
            field.kind
        );
    }
}

#[test]
fn every_sample_reads_back_as_it_was_written() {
    for full in [true, false] {
        let line = line_info(full);
        assert_eq!(
            serde_json::from_value::<LineInfo>(to_value(&line)).unwrap(),
            line
        );
        let record = call_record(full);
        assert_eq!(
            serde_json::from_str::<CallRecord>(&encode(&record)).unwrap(),
            record
        );
    }
}

#[test]
fn every_enumeration_value_is_documented_and_nothing_else() {
    fn check<T: Serialize + Copy>(name: &str, all: &[T], as_str: fn(T) -> &'static str) {
        let documented = doc::ENUMS
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name} is not documented"));
        let values: Vec<&str> = documented.values.iter().map(|(v, _)| *v).collect();
        let actual: Vec<&str> = all.iter().map(|v| as_str(*v)).collect();
        assert_eq!(values, actual, "{name}");
        // What serde writes is what as_str says.
        for value in all {
            assert_eq!(to_value(value), json!(as_str(*value)), "{name}");
        }
    }
    check("LineState", &LineState::ALL, LineState::as_str);
    check("Direction", &Direction::ALL, Direction::as_str);
    check("EndReason", &EndReason::ALL, EndReason::as_str);
    check("LogLevel", &LogLevel::ALL, LogLevel::as_str);
    assert_eq!(doc::ENUMS.len(), 4);
}

// --------------------------------------------------------------------------
// Events and frames
// --------------------------------------------------------------------------

#[test]
fn every_event_is_documented_and_nothing_else() {
    let documented: Vec<&str> = doc::EVENTS.iter().map(|e| e.name).collect();
    assert_eq!(documented, EVENT_NAMES);

    for full in [true, false] {
        let samples = event_samples(full);
        assert_eq!(samples.len(), doc::EVENTS.len());
        for ((event, data_type), documented) in samples.iter().zip(doc::EVENTS) {
            assert_eq!(event.name(), documented.name);
            assert_eq!(*data_type, documented.data);
            let value = to_value(event);
            assert_eq!(keys(&value), ["data", "event"].map(String::from).into());
            assert_eq!(value["event"], json!(documented.name));
            // The data is the documented type.
            let kind = doc::TYPES
                .iter()
                .find(|t| t.name == documented.data)
                .unwrap();
            assert_eq!(
                keys(&value["data"]),
                names(kind.fields, !full),
                "{}",
                event.name()
            );

            let frame = ServerFrame::Event(event.clone());
            assert_eq!(decode_server(&encode(&frame)).unwrap(), frame);
        }
    }
    assert!(event_samples(true)[0].0.is_state());
    assert!(!event_samples(true)[4].0.is_state());
}

#[test]
fn frames_have_the_documented_shape() {
    let hello = Hello {
        protocol: 1,
        version: "0.0.1".to_string(),
    };
    let line = encode(&ClientFrame::Hello {
        hello: hello.clone(),
    });
    assert_eq!(line, r#"{"hello":{"protocol":1,"version":"0.0.1"}}"#);
    assert_eq!(
        decode_client(&line).unwrap(),
        ClientFrame::Hello {
            hello: hello.clone()
        }
    );
    assert_eq!(decode_server(&line).unwrap(), ServerFrame::Hello { hello });

    let request = RequestFrame {
        id: 7,
        method: "lines.get".to_string(),
        params: json!({"id": 1}),
    };
    assert_eq!(
        encode(&ClientFrame::Request(request)),
        r#"{"id":7,"method":"lines.get","params":{"id":1}}"#
    );

    let result = ServerFrame::Result {
        id: 7,
        result: json!({"lines": 2}),
    };
    assert_eq!(encode(&result), r#"{"id":7,"result":{"lines":2}}"#);
    assert_eq!(decode_server(&encode(&result)).unwrap(), result);

    let error = ServerFrame::Error {
        id: Some(7),
        error: error_info(),
    };
    let line = encode(&error);
    assert!(line.starts_with(
        r#"{"id":7,"error":{"msgid":"RMD1001","text":"Line 9 does not exist.","cause":""#
    ));
    assert_eq!(decode_server(&line).unwrap(), error);

    let unaddressed = ServerFrame::Error {
        id: None,
        error: error_info(),
    };
    let line = encode(&unaddressed);
    assert!(line.starts_with(r#"{"id":null,"error":"#));
    assert_eq!(decode_server(&line).unwrap(), unaddressed);
}

#[test]
fn what_cannot_be_read_is_a_message() {
    assert_eq!(decode_client("not json").unwrap_err().id, "RMD2004");
    assert_eq!(
        decode_client(r#"{"method":"lines.list"}"#).unwrap_err().id,
        "RMD2004"
    );
    assert_eq!(decode_client("[1,2]").unwrap_err().id, "RMD2004");
    assert_eq!(decode_server("not json").unwrap_err().id, "RMD2013");
    assert_eq!(decode_server(r#"{"neither":1}"#).unwrap_err().id, "RMD2013");
}

// --------------------------------------------------------------------------
// Errors and the generated file
// --------------------------------------------------------------------------

#[test]
fn every_documented_error_is_in_the_catalogue() {
    let ids = doc::error_ids();
    assert!(ids.len() > 15);
    for id in ids {
        let entry = msg::entries().find(|e| e.id == id);
        assert!(entry.is_some(), "{id} is not in the message catalogue");
    }
    // Every control API message that reaches a client is documented.
    for id in [
        "RMD2001", "RMD2002", "RMD2003", "RMD2004", "RMD2005", "RMD2010", "RMD2014",
    ] {
        assert!(doc::error_ids().contains(&id), "{id}");
    }
}

#[test]
fn the_generated_document_is_up_to_date() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/CONTROL_API.md");
    let rendered = doc::render();
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::write(&path, &rendered).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).unwrap().replace('\r', "");
    assert!(
        on_disk == rendered,
        "docs/CONTROL_API.md is out of date; run UPDATE_DOCS=1 cargo test -p ctlproto"
    );
    // Every method, event, type and value is in it.
    for method in doc::METHODS {
        assert!(rendered.contains(&format!("### `{}`", method.name)));
    }
    for event in doc::EVENTS {
        assert!(rendered.contains(&format!("| `{}` |", event.name)));
    }
    for kind in doc::TYPES {
        assert!(rendered.contains(&format!("### {}\n", kind.name)));
    }
}
