// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! One line: an AT interpreter, a modem engine and a serial port, with the
//! buffers between them and the line's counters.

use std::collections::VecDeque;

use at::{AtConfig, AtInterpreter, Mode, ModemSignals};
use config::{LineConfig, UnknownCommand};
use ctlproto::{CallInfo, CallRecord, ConnectInfo, Counters, Direction, EndReason, LineInfo};
use ctlproto::{ErrorInfo, LineState};
use engine::ModemEngine;
use msg::Message;
use serial::SerialBackend;

use crate::router::LineId;

/// The call a line is in, from dialling or being offered it until it ends.
#[derive(Clone, Debug)]
pub(crate) struct ActiveCall {
    pub id: u64,
    pub direction: Direction,
    pub number: String,
    pub started_ms: u64,
    pub connected_ms: Option<u64>,
    pub connect: Option<ConnectInfo>,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
}

impl ActiveCall {
    pub fn info(&self) -> CallInfo {
        CallInfo {
            id: self.id,
            direction: self.direction,
            number: self.number.clone(),
            started_ms: self.started_ms,
            connected_ms: self.connected_ms,
            connect: self.connect.clone(),
            bytes_tx: self.bytes_tx,
            bytes_rx: self.bytes_rx,
        }
    }

    pub fn record(self, line: LineId, ended_ms: u64, reason: EndReason) -> CallRecord {
        CallRecord {
            id: self.id,
            line,
            direction: self.direction,
            number: self.number,
            started_ms: self.started_ms,
            connected_ms: self.connected_ms,
            ended_ms,
            reason,
            connect: self.connect,
            bytes_tx: self.bytes_tx,
            bytes_rx: self.bytes_rx,
        }
    }
}

/// One line.
pub(crate) struct Line {
    pub config: LineConfig,
    pub interpreter: AtInterpreter,
    pub engine: Box<dyn ModemEngine>,
    /// The engine has been started for the current call.
    pub engine_running: bool,
    /// The serial port, while it is open.
    pub serial: Option<Box<dyn SerialBackend>>,
    /// When to try next to open the port, on the pool's clock.
    pub retry_at_ms: u64,
    /// Why the port is not open, if it would not open or failed.
    pub error: Option<Message>,
    /// The host has DTR on.
    pub dtr: bool,
    /// A call is being offered to the line and has not been answered.
    pub incoming: bool,
    /// Bytes for the host that the port has not taken yet.
    pub to_host: VecDeque<u8>,
    /// Data for the far end that the engine has not taken yet.
    pub to_line: VecDeque<u8>,
    /// The signals last set on the port.
    pub applied: Option<ModemSignals>,
    /// The state last published.
    pub published: Option<LineState>,
    pub counters: Counters,
    pub call: Option<ActiveCall>,
}

impl Line {
    pub fn new(config: LineConfig, engine: Box<dyn ModemEngine>) -> Line {
        let interpreter = AtInterpreter::new(at_config(&config));
        Line {
            config,
            interpreter,
            engine,
            engine_running: false,
            serial: None,
            retry_at_ms: 0,
            error: None,
            dtr: false,
            incoming: false,
            to_host: VecDeque::new(),
            to_line: VecDeque::new(),
            applied: None,
            published: None,
            counters: Counters::default(),
            call: None,
        }
    }

    pub fn id(&self) -> LineId {
        self.config.id
    }

    /// The tag of the line in log lines: `Line01`.
    pub fn source(&self) -> String {
        format!("Line{:02}", self.config.id)
    }

    /// The state of the line, from what its parts are doing now.
    pub fn state(&self) -> LineState {
        if !self.config.enabled {
            LineState::Disabled
        } else if self.interpreter.off_hook() {
            match self.interpreter.mode() {
                Mode::OnlineData | Mode::OnlineCommand => LineState::Online,
                _ => LineState::Handshaking,
            }
        } else if self.serial.is_none() || !self.dtr {
            // The port is closed, or the host has DTR off.
            LineState::NotReady
        } else if self.incoming {
            LineState::Ringing
        } else {
            LineState::Idle
        }
    }

    pub fn info(&self) -> LineInfo {
        LineInfo {
            id: self.config.id,
            name: self.config.name.clone(),
            backend: self.config.serial.backend.as_str().to_string(),
            port: self.config.serial.port.clone(),
            enabled: self.config.enabled,
            state: self.state(),
            auto_answer_rings: self.config.auto_answer_rings,
            unknown_command: self.config.unknown_command.as_str().to_string(),
            counters: self.counters,
            call: self.call.as_ref().map(ActiveCall::info),
            error: self.error.as_ref().map(ErrorInfo::from),
        }
    }
}

/// The interpreter settings a line's configuration asks for.
pub(crate) fn at_config(config: &LineConfig) -> AtConfig {
    AtConfig {
        unknown_command: match config.unknown_command {
            UnknownCommand::Ok => at::UnknownCommand::Ok,
            UnknownCommand::Error => at::UnknownCommand::Error,
        },
        auto_answer_rings: config.auto_answer_rings,
    }
}
