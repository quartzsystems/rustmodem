// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control API's description checked against its types, and
//! `docs/CONTROL_API.md` checked against the description.

use std::collections::BTreeSet;
use std::path::Path;

use ctlproto::doc::{self, FieldDoc};
use ctlproto::{
    Call, CallEvent, CallInfo, CallList, CallRecord, ClientFrame, ConnectInfo, ConnectTally,
    ControlChange, ControlSettings, Counters, Direction, Done, EVENT_NAMES, EVENTS_BY_NAME_ONLY,
    EndReason, ErrorInfo, Event, Hello, HuntGroupInfo, HuntGroupList, LINE_DIAG_PER_SECOND,
    LineDiag, LineInfo, LineList, LineModem, LineModemSpec, LineSip, LineState, LinkInfo,
    ListenerState, LogEvent, LogLevel, LogTail, MediaChange, MediaInfo, MediaSettings, ModemChange,
    ModemInfo, ModemSettings, ModemStateChange, PROTOCOL, PortInfo, PortKind, PortList,
    RegisterChange, RegisterSpec, Registration, RegistrationState, Reloaded, RequestFrame,
    RouteInfo, RouteList, SelfTestResult, ServerFrame, ServiceChange, ServiceInfo, ServiceMode,
    ServiceSettings, Settings, SettingsResult, SipChange, SipMode, SipRegistration, SipSettings,
    SipStatus, SipTrace, Stats, Subscription, Tally, TraceDirection, decode_client, decode_server,
    encode,
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
        media: full.then(|| Box::new(media_info(full))),
        modem: full.then(|| modem_info(full)),
    }
}

fn modem_info(full: bool) -> ModemInfo {
    ModemInfo {
        state: "data".to_string(),
        rx_level_tenth_dbm0: full.then_some(-143),
        tx_level_tenth_dbm0: full.then_some(-120),
        carrier: true,
        frequency_error_mhz: -1250,
        quality_percent: 71,
        framing_errors: 2,
        retrains: 0,
        snr_tenth_db: full.then_some(312),
        equalizer_error_tenth_db: full.then_some(-305),
        timing_offset_ppm: full.then_some(-12),
        link: full.then(|| Box::new(link_info(full))),
    }
}

fn link_info(full: bool) -> LinkInfo {
    LinkInfo {
        stage: "lapm".to_string(),
        end: full.then(|| "remote".to_string()),
        frames_tx: 162,
        frames_rx: 161,
        retransmissions: 3,
        frame_errors: 2,
        window: 1,
        window_size: 15,
        peer_busy: false,
        own_busy: false,
        compression_tx: "compressed".to_string(),
        compression_rx: "transparent".to_string(),
        tx_octets_in: 20_000,
        tx_octets_out: 8_120,
        rx_octets_in: 20_074,
        rx_octets_out: 20_000,
        breaks: 0,
    }
}

fn line_modem() -> LineModem {
    LineModem {
        modulations: vec!["v22bis".to_string(), "v21".to_string()],
        v8: true,
        tx_level_dbm0: -12,
        error_control: "auto".to_string(),
        compression: true,
        guard_tone: "off".to_string(),
        own: vec!["v8".to_string()],
    }
}

fn line_diag(full: bool) -> LineDiag {
    LineDiag {
        line: 1,
        modem: full.then(|| modem_info(full)),
        history: vec![ModemStateChange {
            state: "answer-tone".to_string(),
            at_ms: 2150,
        }],
        scope_kind: "discriminator".to_string(),
        scope: vec![127, -127, 0, 64],
        constellation_bits: 4,
        constellation_unit: 30,
        constellation: vec![30, 30, -91, 29, 88, -93],
    }
}

fn modem_settings() -> ModemSettings {
    ModemSettings {
        modulations: vec!["v22bis".to_string(), "v21".to_string()],
        v8: true,
        tx_level_dbm0: -12,
        answer_delay_ms: 0,
        fallback_ms: 3000,
        error_control: "auto".to_string(),
        compression: true,
        guard_tone: "off".to_string(),
    }
}

fn media_info(full: bool) -> MediaInfo {
    MediaInfo {
        codec: "PCMU".to_string(),
        ptime_ms: 20,
        remote: full.then(|| "10.10.20.11:16384".to_string()),
        rx_packets: 3000,
        rx_octets: 480_000,
        tx_packets: 3000,
        tx_octets: 480_000,
        lost: 1,
        late: 2,
        duplicate: 3,
        resync: 0,
        ignored: 0,
        free_run: 4,
        slips: 0,
        jitter_us_avg: 250,
        jitter_us_max: 1900,
        tick_p50_us: 40,
        tick_p99_us: 120,
        tick_max_us: 900,
        overruns: 0,
    }
}

fn registration(full: bool) -> Registration {
    Registration {
        state: if full {
            RegistrationState::Failed
        } else {
            RegistrationState::Registered
        },
        expires_in_s: full.then_some(280),
        error: full.then(error_info),
    }
}

fn line_sip(full: bool) -> LineSip {
    LineSip {
        mode: if full {
            SipMode::Registered
        } else {
            SipMode::Trunk
        },
        extension: full.then(|| "203".to_string()),
        server: full.then(|| "pbx.example.com".to_string()),
        auth_user: full.then(|| "line203".to_string()),
        expires_s: full.then_some(300),
        registration: full.then(|| registration(full)),
        has_password: full,
    }
}

fn sip_registration(full: bool) -> SipRegistration {
    SipRegistration {
        line: 3,
        server: "pbx.example.com".to_string(),
        user: "203".to_string(),
        registration: registration(full),
    }
}

fn sip_status(full: bool) -> SipStatus {
    SipStatus {
        listener: ListenerState::Listening,
        listen: "0.0.0.0:5060".to_string(),
        trusted_peers: vec!["10.10.20.0/24".to_string()],
        outbound_peer: "10.10.20.11".to_string(),
        registrations: vec![sip_registration(full)],
        dropped_untrusted: 2,
        calls: 1,
    }
}

fn sip_trace() -> SipTrace {
    SipTrace {
        time_ms: 1_800_000_000_000,
        direction: TraceDirection::Received,
        peer: "10.10.20.11:5060".to_string(),
        text: "OPTIONS sip:rustmodem SIP/2.0\r\n\r\n".to_string(),
    }
}

fn hunt_group() -> HuntGroupInfo {
    HuntGroupInfo {
        name: "dialup".to_string(),
        pilot: "5551000".to_string(),
        lines: vec![1, 2],
        strategy: "first-idle".to_string(),
    }
}

fn route() -> RouteInfo {
    RouteInfo {
        index: 0,
        called: "555*".to_string(),
        target: "group:dialup".to_string(),
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
        media: full.then(|| Box::new(media_info(full))),
        modem: full.then(|| modem_info(full)),
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
            at_commands: 6,
        },
        call: full.then(|| call_info(full)),
        error: full.then(error_info),
        binding: full.then(|| "Standard 56000 bps Modem on COM4".to_string()),
        engine: "null".to_string(),
        hunt_groups: vec!["dialup".to_string()],
        sip: line_sip(full),
        capture: full,
        modem: line_modem(),
    }
}

fn settings() -> Settings {
    Settings {
        service: ServiceSettings {
            log_level: LogLevel::Info,
            log_dir: String::new(),
        },
        control: ControlSettings {
            pipe: "rustmodem-control".to_string(),
        },
        sip: SipSettings {
            enabled: true,
            listen: "0.0.0.0:5060".to_string(),
            address: String::new(),
            user_agent: String::new(),
            trusted_peers: vec!["10.10.20.0/24".to_string()],
            outbound_peer: "10.10.20.11".to_string(),
            ring_timeout_s: 60,
        },
        media: MediaSettings {
            codecs: vec!["pcmu".to_string(), "pcma".to_string()],
            ptime_ms: 20,
            rtp_ports: "20000-20999".to_string(),
            rtp_timeout_s: 10,
            capture_dir: String::new(),
        },
        modem: modem_settings(),
        path: r"C:\ProgramData\Quartz Systems\RustModem\rustmodem.toml".to_string(),
    }
}

fn connect_tally(full: bool) -> ConnectTally {
    ConnectTally {
        rate: 2400,
        carrier: full.then(|| "V.22bis".to_string()),
        count: 3,
    }
}

fn tally() -> Tally {
    Tally {
        name: "LAPM".to_string(),
        count: 2,
    }
}

fn port_info(full: bool) -> PortInfo {
    PortInfo {
        name: "CNCB0".to_string(),
        kind: PortKind::Com0com,
        pair: full.then(|| "CNCA0".to_string()),
        host_port: full.then(|| "COM4".to_string()),
        modem: full.then(|| "Standard 56000 bps Modem".to_string()),
        line: full.then_some(1),
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
            config_path: r"C:\ProgramData\Quartz Systems\RustModem\rustmodem.toml".to_string(),
            lines: 2,
            started_ms: 1_800_000_000_000,
            uptime_ms: 60_000,
            config_loaded_ms: 1_800_000_000_000,
            mode: ServiceMode::Service,
            capture_dir: full
                .then(|| r"C:\ProgramData\Quartz Systems\RustModem\logs\captures".to_string()),
        }),
        "Settings" => to_value(&settings()),
        "ServiceSettings" => to_value(&settings().service),
        "ControlSettings" => to_value(&settings().control),
        "ServiceChange" => to_value(&ServiceChange {
            log_level: full.then_some(LogLevel::Debug),
            log_dir: full.then(String::new),
        }),
        "ControlChange" => to_value(&ControlChange {
            pipe: full.then(|| "rustmodem-control".to_string()),
        }),
        "SettingsResult" => to_value(&SettingsResult {
            restart_required: true,
            config: settings(),
        }),
        "Stats" => to_value(&Stats {
            connects: vec![connect_tally(full)],
            protocols: vec![tally()],
            compressions: vec![tally()],
            ..Stats::default()
        }),
        "ConnectTally" => to_value(&connect_tally(full)),
        "Tally" => to_value(&tally()),
        "LogTail" => to_value(&LogTail {
            lines: vec![log_event()],
        }),
        "PortList" => to_value(&PortList {
            ports: vec![port_info(full)],
        }),
        "PortInfo" => to_value(&port_info(full)),
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
            line: full.then_some(1),
        }),
        "LineModem" => to_value(&line_modem()),
        "LineModemSpec" => to_value(&LineModemSpec {
            modulations: full.then(|| vec!["bell103".to_string()]),
            v8: full.then_some(false),
            tx_level_dbm0: full.then_some(-15),
            error_control: full.then(|| "required".to_string()),
            compression: full.then_some(false),
            guard_tone: full.then(|| "1800".to_string()),
        }),
        "ModemInfo" => to_value(&modem_info(full)),
        "LinkInfo" => to_value(&link_info(full)),
        "ModemStateChange" => to_value(&line_diag(full).history[0]),
        "LineDiag" => to_value(&line_diag(full)),
        "SelfTestResult" => to_value(&SelfTestResult {
            a: 1,
            b: 2,
            passed: !full,
            connect: full.then(|| connect(false)),
            connect_ms: full.then_some(5480),
            octets: 65_536,
            octets_a_to_b: 65_536,
            octets_b_to_a: 65_000,
            call_ms: 2_200_000,
            elapsed_ms: 7400,
            error: full.then(error_info),
        }),
        "ModemSettings" => to_value(&modem_settings()),
        "ModemChange" => to_value(&ModemChange {
            modulations: full.then(|| vec!["v21".to_string()]),
            v8: full.then_some(true),
            tx_level_dbm0: full.then_some(-12),
            answer_delay_ms: full.then_some(0),
            fallback_ms: full.then_some(3000),
            error_control: full.then(|| "off".to_string()),
            compression: full.then_some(true),
            guard_tone: full.then(|| "550".to_string()),
        }),
        "Done" => to_value(&Done {}),
        "LineSip" => to_value(&line_sip(full)),
        "Registration" => to_value(&registration(full)),
        "MediaInfo" => to_value(&media_info(full)),
        "SipSettings" => to_value(&settings().sip),
        "MediaSettings" => to_value(&settings().media),
        "SipChange" => to_value(&SipChange {
            enabled: full.then_some(true),
            listen: full.then(|| "0.0.0.0:5060".to_string()),
            address: full.then(String::new),
            user_agent: full.then(String::new),
            trusted_peers: full.then(Vec::new),
            outbound_peer: full.then(String::new),
            ring_timeout_s: full.then_some(60),
        }),
        "MediaChange" => to_value(&MediaChange {
            codecs: full.then(|| vec!["pcma".to_string()]),
            ptime_ms: full.then_some(10),
            rtp_ports: full.then(|| "30000-30099".to_string()),
            rtp_timeout_s: full.then_some(5),
            capture_dir: full.then(String::new),
        }),
        "RegisterSpec" => to_value(&RegisterSpec {
            server: "pbx.example.com".to_string(),
            user: "203".to_string(),
            password: "secret".to_string(),
            auth_user: full.then(|| "line203".to_string()),
            expires_s: full.then_some(300),
        }),
        "RegisterChange" => to_value(&RegisterChange {
            server: full.then(|| "pbx.example.com".to_string()),
            user: full.then(|| "203".to_string()),
            password: full.then(|| "secret".to_string()),
            auth_user: full.then(String::new),
            expires_s: full.then_some(300),
        }),
        "SipSummary" => to_value(&sip_status(full).summary()),
        "SipStatus" => to_value(&sip_status(full)),
        "SipRegistration" => to_value(&sip_registration(full)),
        "SipTrace" => to_value(&sip_trace()),
        "HuntGroupList" => to_value(&HuntGroupList {
            groups: vec![hunt_group()],
        }),
        "HuntGroupInfo" => to_value(&hunt_group()),
        "RouteList" => to_value(&RouteList {
            routes: vec![route()],
        }),
        "RouteInfo" => to_value(&route()),
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
        (
            Event::SipRegistration(sip_registration(full)),
            "SipRegistration",
        ),
        (Event::SipTrace(sip_trace()), "SipTrace"),
        (Event::LineDiag(line_diag(full)), "LineDiag"),
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
    let mut known: BTreeSet<String> = ["integer", "number", "string", "boolean"]
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
        let diag = line_diag(full);
        assert_eq!(
            serde_json::from_str::<LineDiag>(&encode(&diag)).unwrap(),
            diag
        );
        let result: SelfTestResult =
            serde_json::from_value(sample("SelfTestResult", full)).unwrap();
        assert_eq!(to_value(&result), sample("SelfTestResult", full));
    }
    // What an older service leaves out reads as nothing: a line with no
    // modem settings of its own, a call with no modem, diagnostics with
    // none.
    let mut old = to_value(&line_info(true));
    old.as_object_mut().unwrap().remove("modem");
    old["call"].as_object_mut().unwrap().remove("modem");
    let line: LineInfo = serde_json::from_value(old).unwrap();
    assert_eq!(line.modem, LineModem::default());
    assert_eq!(line.call.unwrap().modem, None);
    let bare: LineDiag = serde_json::from_value(json!({"line": 3})).unwrap();
    assert_eq!(
        (bare.modem, bare.scope_kind.as_str(), bare.scope.len()),
        (None, "none", 0)
    );
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
    check("ServiceMode", &ServiceMode::ALL, ServiceMode::as_str);
    check("PortKind", &PortKind::ALL, PortKind::as_str);
    check("SipMode", &SipMode::ALL, SipMode::as_str);
    check(
        "RegistrationState",
        &RegistrationState::ALL,
        RegistrationState::as_str,
    );
    check("ListenerState", &ListenerState::ALL, ListenerState::as_str);
    check(
        "TraceDirection",
        &TraceDirection::ALL,
        TraceDirection::as_str,
    );
    assert_eq!(doc::ENUMS.len(), 10);
}

#[test]
fn a_port_binding_names_the_modem_and_the_host_port() {
    assert_eq!(
        port_info(true).binding().as_deref(),
        Some("Standard 56000 bps Modem on COM4")
    );
    let mut port = port_info(true);
    port.modem = None;
    assert_eq!(port.binding().as_deref(), Some("COM4"));
    assert_eq!(port_info(false).binding(), None);
}

/// The compatibility rule of the documentation: a client reads a result or
/// an event that has fields it does not know.
#[test]
fn a_client_ignores_fields_it_does_not_know() {
    let mut line = to_value(&line_info(true));
    line["added_in_a_later_version"] = json!({"x": 1});
    line["counters"]["new_counter"] = json!(7);
    assert_eq!(
        serde_json::from_value::<LineInfo>(line).unwrap(),
        line_info(true)
    );
    let mut stats = sample("Stats", true);
    stats["later"] = json!(1);
    assert!(serde_json::from_value::<Stats>(stats).is_ok());
    let event = r#"{"event":"line.state","data":{"id":1,"backend":"com0com","port":"P1","enabled":true,"state":"idle","auto_answer_rings":0,"unknown_command":"ok","counters":{"calls_out":0,"calls_in":0,"connects":0,"bytes_tx":0,"bytes_rx":0,"at_commands":0},"later":true},"also_later":1}"#;
    assert!(matches!(
        decode_server(event).unwrap(),
        ServerFrame::Event(Event::LineState(_))
    ));
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
    // A registration is state; a traced message is not, and is the one
    // event that has to be asked for by name.
    assert!(event_samples(true)[5].0.is_state());
    assert!(!event_samples(true)[6].0.is_state());
    // A line's diagnostics are neither state nor sent unless asked for,
    // and come no faster than they can be read.
    assert!(!event_samples(true)[7].0.is_state());
    assert_eq!(EVENTS_BY_NAME_ONLY, ["sip.trace", "line.diag"]);
    assert_eq!(LINE_DIAG_PER_SECOND, 4);
    assert!(
        EVENTS_BY_NAME_ONLY
            .iter()
            .all(|name| EVENT_NAMES.contains(name))
    );
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
