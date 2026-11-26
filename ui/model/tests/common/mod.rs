// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Lines, calls and services as the control API reports them, for the
//! tests of the model.

#![allow(dead_code)]

use ctlproto::{
    CallInfo, CallRecord, ConnectInfo, Counters, Direction, EndReason, ErrorInfo, LineInfo,
    LineState, LogEvent, LogLevel, PROTOCOL, ServiceInfo, ServiceMode,
};

/// When the test service started, in Unix milliseconds: 2027-01-15
/// 08:00:00 UTC.
pub const START_MS: u64 = 1_800_000_000_000;

/// A line in a state, on port `CNCB<id - 1>`, with nothing else about it.
pub fn line(id: u16, state: LineState) -> LineInfo {
    LineInfo {
        id,
        name: None,
        backend: "com0com".to_string(),
        port: format!("CNCB{}", id - 1),
        enabled: state != LineState::Disabled,
        state,
        auto_answer_rings: 0,
        unknown_command: "ok".to_string(),
        counters: Counters::default(),
        call: None,
        error: None,
        binding: None,
        engine: "null".to_string(),
        hunt_groups: Vec::new(),
        sip: ctlproto::LineSip::default(),
        capture: false,
    }
}

/// A call that started `started_s` seconds after the service did.
pub fn call(direction: Direction, number: &str, started_s: u64) -> CallInfo {
    CallInfo {
        id: started_s + 1,
        direction,
        number: number.to_string(),
        started_ms: START_MS + started_s * 1000,
        connected_ms: None,
        connect: None,
        bytes_tx: 0,
        bytes_rx: 0,
        media: None,
    }
}

/// A call that connected `connected_s` seconds after the service started,
/// at a rate and with a carrier.
pub fn connected(mut call: CallInfo, connected_s: u64, rate: u32, carrier: &str) -> CallInfo {
    call.connected_ms = Some(START_MS + connected_s * 1000);
    call.connect = Some(ConnectInfo {
        rate,
        carrier: (!carrier.is_empty()).then(|| carrier.to_string()),
        protocol: None,
        compression: None,
    });
    call
}

/// A line with a call, in the state that goes with it.
pub fn line_with_call(id: u16, state: LineState, call: CallInfo) -> LineInfo {
    let mut line = line(id, state);
    line.call = Some(call);
    line
}

/// A line whose port will not open.
pub fn broken_line(id: u16) -> LineInfo {
    let mut line = line(id, LineState::NotReady);
    line.error = Some(error("RMD3001", &[&line.port, "not found"]));
    line
}

/// An `RMD` message as the control API carries it.
pub fn error(id: &str, args: &[&str]) -> ErrorInfo {
    ErrorInfo::from(msg::message(id, args))
}

/// A service with `lines` lines that has been up for `uptime_s` seconds.
pub fn service(mode: ServiceMode, lines: u32, uptime_s: u64) -> ServiceInfo {
    ServiceInfo {
        version: "0.0.2".to_string(),
        protocol: PROTOCOL,
        pipe: "rustmodem-control".to_string(),
        config_path: r"C:\ProgramData\Quartz Systems\rustmodem\rustmodem.toml".to_string(),
        lines,
        started_ms: START_MS,
        uptime_ms: uptime_s * 1000,
        config_loaded_ms: START_MS,
        mode,
    }
}

/// A log line `second` seconds after the service started.
pub fn log(second: u64, level: LogLevel, source: &str, text: &str) -> LogEvent {
    LogEvent {
        time_ms: START_MS + second * 1000,
        level,
        source: source.to_string(),
        text: text.to_string(),
    }
}

/// A call that ended, with this id, on a line.
pub fn record(id: u64, line: u16, reason: EndReason) -> CallRecord {
    CallRecord {
        id,
        line,
        direction: Direction::Incoming,
        number: "5550100".to_string(),
        started_ms: START_MS + id * 60_000,
        connected_ms: None,
        ended_ms: START_MS + id * 60_000 + 5000,
        reason,
        connect: None,
        bytes_tx: 0,
        bytes_rx: 0,
        media: None,
    }
}
