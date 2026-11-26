// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The pool: every line, driven from one thread.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;

use at::{Action, HangupCause, Input, LineEvent, Output, Trace};
use config::{Config, LineConfig};
use ctlproto::{
    CallEvent, CallRecord, ConnectInfo, Direction, EndReason, Event, LineInfo, LineState, LogEvent,
    LogLevel, Stats,
};
use engine::ModemEvent;
use msg::{Message, Severity, message};
use serial::{ModemSignals, SerialEvent, SerialOperation, WaitHandle};

use crate::handle::Command;
use crate::line::{ActiveCall, Line, at_config};
use crate::media::{MediaEndpoint, MediaEvent, MediaRunner, MediaStats};
use crate::router::{CallRouter, LineDirectory, LineId, RouterEvent};
use crate::stats::Tallies;
use crate::{
    CALL_HISTORY, EngineFactory, EventSink, Now, PORT_RETRY_MS, ProfileStore, SerialFactory,
    TICK_MS,
};

/// How much is read from a port or an engine at a time.
const CHUNK: usize = 4096;

/// How much may wait in a line's buffers before the pool stops reading
/// more. This is what pushes back on a host or a far end that sends faster
/// than the other side takes.
const HIGH_WATER: usize = 8192;

/// How far behind its clock the pool may fall before it gives up catching
/// up tick by tick, in milliseconds.
const MAX_CATCH_UP_MS: u64 = 2000;

/// The tag of the pool itself in log lines.
const SOURCE: &str = "POOL";

/// What a pool is made from.
pub struct PoolParts {
    /// What stands between the lines and the network.
    pub router: Box<dyn CallRouter>,
    /// Carries the media of connected calls. It has to be one that takes
    /// the media endpoints the router gives.
    pub media: Box<dyn MediaRunner>,
    /// Opens the serial ports.
    pub serial: Box<dyn SerialFactory>,
    /// Makes the modem engines.
    pub engines: EngineFactory,
    /// Takes the events.
    pub sink: Box<dyn EventSink>,
    /// Keeps the lines' stored profiles.
    pub profiles: Box<dyn ProfileStore>,
    /// Where call records are appended, one JSON object a line.
    pub call_log: Option<Box<dyn Write + Send>>,
}

/// Every line, driven from one thread.
pub struct Pool {
    lines: Vec<Line>,
    router: Box<dyn CallRouter>,
    media: Box<dyn MediaRunner>,
    serial: Box<dyn SerialFactory>,
    engines: EngineFactory,
    sink: Box<dyn EventSink>,
    profiles: Box<dyn ProfileStore>,
    call_log: Option<Box<dyn Write + Send>>,
    /// The call log failed and the failure has been reported.
    call_log_failed: bool,
    history: VecDeque<CallRecord>,
    next_call_id: u64,
    /// What the interpreters have asked for and has not been done yet.
    actions: VecDeque<(LineId, Action)>,
    /// The reason recorded when the operator, not the host, ends a call.
    operator_reason: EndReason,
    now: Now,
    next_tick_ms: Option<u64>,
    scratch: Vec<Output>,
    tallies: Tallies,
    /// What the host side of each port is bound to, by port name in lower
    /// case, as the pool was last told.
    bindings: BTreeMap<String, String>,
}

/// The lines, as the router may ask about them.
struct Directory<'a>(&'a [Line]);

impl LineDirectory for Directory<'_> {
    fn state(&self, line: LineId) -> Option<LineState> {
        self.0.iter().find(|l| l.id() == line).map(Line::state)
    }
}

impl Pool {
    /// A pool with no lines. Give it lines with [`Pool::apply_config`].
    pub fn new(parts: PoolParts) -> Pool {
        Pool {
            lines: Vec::new(),
            router: parts.router,
            media: parts.media,
            serial: parts.serial,
            engines: parts.engines,
            sink: parts.sink,
            profiles: parts.profiles,
            call_log: parts.call_log,
            call_log_failed: false,
            history: VecDeque::new(),
            next_call_id: 1,
            actions: VecDeque::new(),
            operator_reason: EndReason::Operator,
            now: Now::default(),
            next_tick_ms: None,
            scratch: Vec::new(),
            tallies: Tallies::default(),
            bindings: BTreeMap::new(),
        }
    }

    /// Tells the pool the time without a turn of its loop. A pool is given
    /// its lines before its loop starts; this is how what it logs then
    /// gets the right time.
    pub fn set_now(&mut self, now: Now) {
        self.now = now;
    }

    // ----------------------------------------------------------------------
    // The loop
    // ----------------------------------------------------------------------

    /// One turn of the pool's loop. Call it at least every [`TICK_MS`], and
    /// sooner when a wait handle is signalled.
    pub fn service(&mut self, now: Now) {
        self.now = now;
        self.open_ports();

        // Read the serial ports and step the interpreters.
        for index in 0..self.lines.len() {
            self.serve_port(index);
        }
        self.settle();

        // Tick the interpreters' clocks and the engines of lines in a call.
        let mut next = *self.next_tick_ms.get_or_insert(now.mono_ms + TICK_MS);
        if now.mono_ms.saturating_sub(next) > MAX_CATCH_UP_MS {
            next = now.mono_ms;
        }
        while next <= now.mono_ms {
            self.tick();
            next += TICK_MS;
        }
        self.next_tick_ms = Some(next);

        self.finish();
    }

    /// The handles the pool's thread may sleep on until a port has work.
    pub fn wait_handles(&self) -> Vec<WaitHandle> {
        self.lines
            .iter()
            .filter_map(|line| line.serial.as_ref())
            .map(|serial| serial.wait_handle())
            .filter(|handle| handle.raw().is_some())
            .collect()
    }

    /// Moves data, drops calls offered to lines that cannot take them,
    /// applies the signals and publishes the states that have changed.
    fn finish(&mut self) {
        for index in 0..self.lines.len() {
            // A runner with a clock of its own reports whenever it has
            // something to say, not only in a tick.
            self.engine_events(index);
            self.pump(index);
        }
        self.settle();
        self.reject_unready();
        self.settle();
        for index in 0..self.lines.len() {
            self.flush(index);
        }
        self.publish_states();
    }

    /// One tick: 20 ms pass for every interpreter, and for the media of
    /// every connected call whose runner goes by the pool's clock.
    fn tick(&mut self) {
        for index in 0..self.lines.len() {
            if !self.lines[index].config.enabled {
                continue;
            }
            self.step(index, Input::Elapsed(TICK_MS as u32));
            if self.lines[index].media.is_some() {
                let id = self.lines[index].id();
                self.media.tick(id);
                self.engine_events(index);
            }
        }
        self.settle();
    }

    /// Carries out what the interpreters have asked for and what the router
    /// has to say, until neither has anything more.
    fn settle(&mut self) {
        // Each pass can only hang up, connect or fail calls that exist, so
        // this ends; the bound is a guard, not a limit that is reached.
        for _ in 0..256 {
            let mut progressed = false;
            while let Some((id, action)) = self.actions.pop_front() {
                progressed = true;
                self.perform(id, action);
            }
            while let Some(event) = self.router.poll(self.now.mono_ms, &Directory(&self.lines)) {
                progressed = true;
                self.route(event);
            }
            if !progressed {
                break;
            }
        }
    }

    // ----------------------------------------------------------------------
    // Interpreters
    // ----------------------------------------------------------------------

    /// Gives one input to a line's interpreter and takes in what it
    /// produces.
    fn step(&mut self, index: usize, input: Input<'_>) {
        let mut out = std::mem::take(&mut self.scratch);
        self.lines[index].interpreter.step(input, &mut out);
        self.absorb(index, &mut out);
        self.scratch = out;
    }

    /// Runs something on a line's interpreter that is not an input, and
    /// takes in what it produces.
    fn with_interpreter<T>(
        &mut self,
        index: usize,
        run: impl FnOnce(&mut at::AtInterpreter, &mut Vec<Output>) -> T,
    ) -> T {
        let mut out = std::mem::take(&mut self.scratch);
        let result = run(&mut self.lines[index].interpreter, &mut out);
        self.absorb(index, &mut out);
        self.scratch = out;
        result
    }

    fn absorb(&mut self, index: usize, out: &mut Vec<Output>) {
        for output in out.drain(..) {
            let line = &mut self.lines[index];
            match output {
                Output::DteBytes(bytes) => line.to_host.extend(bytes),
                Output::LineBytes(bytes) => {
                    // Data has somewhere to go only while the engine runs.
                    if line.media.is_some() {
                        let count = bytes.len() as u64;
                        line.counters.bytes_tx += count;
                        self.tallies.bytes_out(count);
                        if let Some(call) = &mut line.call {
                            call.bytes_tx += count;
                        }
                        line.to_line.extend(bytes);
                    }
                }
                Output::Action(action) => {
                    let id = line.id();
                    self.actions.push_back((id, action));
                }
                // The signals are read from the interpreter when the port
                // is flushed.
                Output::Signals(_) => {}
                Output::Trace(trace) => {
                    let source = line.source();
                    let text = match trace {
                        Trace::Command(text) => {
                            line.counters.at_commands += 1;
                            self.tallies.at_command();
                            format!("> {text}")
                        }
                        Trace::Result(text) => format!("< {text}"),
                    };
                    self.log(LogLevel::Debug, &source, text);
                }
            }
        }
    }

    /// Does what an interpreter asked for.
    fn perform(&mut self, id: LineId, action: Action) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let now_ms = self.now.mono_ms;
        match action {
            Action::Dial(number) => {
                // A line that dials while a call is being offered to it has
                // turned that call down.
                if self.lines[index].incoming {
                    self.lines[index].incoming = false;
                    self.end_call(index, EndReason::Failed);
                }
                self.start_call(index, Direction::Outgoing, number.clone());
                self.router
                    .dial(now_ms, id, &number, &Directory(&self.lines));
            }
            Action::Answer => {
                let line = &mut self.lines[index];
                if let Some(call) = &mut line.call
                    && line.incoming
                    && !call.answered
                {
                    call.answered = true;
                    self.tallies.answered();
                }
                self.router.answer(now_ms, id);
            }
            Action::Hangup(cause) => {
                let reason = match cause {
                    HangupCause::Command => EndReason::Hangup,
                    HangupCause::Dtr => EndReason::Dtr,
                    HangupCause::Abort => EndReason::Aborted,
                    HangupCause::Timeout => EndReason::NoAnswer,
                    HangupCause::Reset => EndReason::Reset,
                    HangupCause::Operator => self.operator_reason,
                };
                self.stop_engine(index);
                self.lines[index].incoming = false;
                self.end_call(index, reason);
                self.router.hangup(now_ms, id);
            }
            Action::StoreProfile(profile) => {
                let source = self.lines[index].source();
                // The interpreter has the profile already; this is what
                // makes it outlast the interpreter. The host has its OK
                // either way, so a failure can only be logged.
                let outcome = match self.profiles.store(id, &profile) {
                    Ok(()) => message("RMD1014", &[&id.to_string()]),
                    Err(problem) => problem,
                };
                self.log_message(&source, &outcome);
            }
        }
    }

    // ----------------------------------------------------------------------
    // The router and the engines
    // ----------------------------------------------------------------------

    /// Does what the router said.
    fn route(&mut self, event: RouterEvent) {
        let line = match &event {
            RouterEvent::Incoming { line, .. } | RouterEvent::Connected { line, .. } => *line,
            RouterEvent::Ring(line)
            | RouterEvent::RingEnd(line)
            | RouterEvent::Abandoned(line)
            | RouterEvent::Busy(line)
            | RouterEvent::NoAnswer(line)
            | RouterEvent::NoDialtone(line)
            | RouterEvent::Failed(line)
            | RouterEvent::Cleared(line) => *line,
        };
        let Some(index) = self.index_of(line) else {
            // The line was removed while the router still had a call for
            // it.
            self.router.hangup(self.now.mono_ms, line);
            return;
        };
        match event {
            RouterEvent::Incoming { from, .. } => {
                self.lines[index].incoming = true;
                self.start_call(index, Direction::Incoming, from);
            }
            RouterEvent::Ring(_) => self.step(index, Input::Event(LineEvent::Ring)),
            RouterEvent::RingEnd(_) => self.step(index, Input::Event(LineEvent::RingEnd)),
            RouterEvent::Abandoned(_) => {
                self.lines[index].incoming = false;
                self.step(index, Input::Event(LineEvent::RingEnd));
                self.end_call(index, EndReason::Abandoned);
            }
            RouterEvent::Connected {
                originate, media, ..
            } => {
                let line = &mut self.lines[index];
                line.incoming = false;
                if !line.interpreter.off_hook() {
                    // The line went back on hook before the call was
                    // through.
                    self.router.hangup(self.now.mono_ms, line.config.id);
                    return;
                }
                if !self.start_media(index, media, originate) {
                    // The runner cannot carry this call: it ends as one
                    // that could not be set up.
                    let id = self.lines[index].id();
                    let source = self.lines[index].source();
                    self.log_message(&source, &message("RMD6002", &[&id.to_string()]));
                    self.step(index, Input::Event(LineEvent::CarrierLost));
                    self.end_call(index, EndReason::Failed);
                    self.router.hangup(self.now.mono_ms, id);
                    return;
                }
                self.engine_events(index);
            }
            RouterEvent::Busy(_) => {
                self.tallies.rejected();
                self.step(index, Input::Event(LineEvent::Busy));
                self.end_call(index, EndReason::Busy);
            }
            RouterEvent::NoAnswer(_) => {
                self.step(index, Input::Event(LineEvent::NoAnswer));
                self.end_call(index, EndReason::NoAnswer);
            }
            RouterEvent::NoDialtone(_) => {
                self.step(index, Input::Event(LineEvent::NoDialtone));
                self.end_call(index, EndReason::NoDialtone);
            }
            RouterEvent::Failed(_) => {
                // A call a line made that never got through was turned
                // away: nothing was ready to take it.
                if let Some(call) = &self.lines[index].call
                    && call.direction == Direction::Outgoing
                    && call.connected_ms.is_none()
                {
                    self.tallies.rejected();
                }
                self.stop_engine(index);
                self.lines[index].incoming = false;
                self.step(index, Input::Event(LineEvent::CarrierLost));
                self.end_call(index, EndReason::Failed);
            }
            RouterEvent::Cleared(_) => {
                self.stop_engine(index);
                self.step(index, Input::Event(LineEvent::CarrierLost));
                self.end_call(index, EndReason::Remote);
            }
        }
    }

    /// Hands a line's engine and the far end of its call to the media
    /// runner. Returns whether the runner took them.
    fn start_media(&mut self, index: usize, endpoint: MediaEndpoint, originate: bool) -> bool {
        // A call that connects on top of one that is connected replaces
        // it.
        self.stop_engine(index);
        let id = self.lines[index].id();
        let engine = match self.lines[index].engine.take() {
            Some(engine) => engine,
            None => (self.engines)(&self.lines[index].config),
        };
        match self.media.start(id, engine, endpoint, originate) {
            Ok(call) => {
                self.lines[index].media = Some(call);
                true
            }
            Err(engine) => {
                self.lines[index].engine = Some(engine);
                false
            }
        }
    }

    /// Takes what the media of a line's call has to report.
    fn engine_events(&mut self, index: usize) {
        loop {
            let Some(call) = &mut self.lines[index].media else {
                return;
            };
            let Some(event) = call.poll_event() else {
                return;
            };
            let event = match event {
                MediaEvent::Modem(event) => event,
                // Nothing comes from the far end any more: to the line
                // that is the carrier lost.
                MediaEvent::Stalled => {
                    let id = self.lines[index].id();
                    let source = self.lines[index].source();
                    self.log_message(&source, &message("RMD6001", &[&id.to_string()]));
                    ModemEvent::CarrierLost
                }
            };
            match event {
                ModemEvent::Connect(info) => {
                    let connect = ConnectInfo {
                        rate: info.rate,
                        carrier: info.carrier.clone(),
                        protocol: info.protocol.clone(),
                        compression: info.compression.clone(),
                    };
                    self.step(index, Input::Event(LineEvent::Connected(info)));
                    let unix_ms = self.now.unix_ms;
                    let line = &mut self.lines[index];
                    let id = line.id();
                    if let Some(call) = &mut line.call {
                        call.connected_ms = Some(unix_ms);
                        call.connect = Some(connect.clone());
                        line.counters.connects += 1;
                        if call.direction == Direction::Incoming {
                            self.tallies.connected(&connect);
                        }
                        let call = call.info();
                        let source = line.source();
                        self.publish(Event::CallConnected(CallEvent { line: id, call }));
                        self.log_message(
                            &source,
                            &message("RMD1007", &[&id.to_string(), &connect.rate.to_string()]),
                        );
                    }
                }
                ModemEvent::CarrierLost => {
                    // The engine has stopped by itself.
                    self.stop_engine(index);
                    let id = self.lines[index].id();
                    self.step(index, Input::Event(LineEvent::CarrierLost));
                    self.end_call(index, EndReason::Remote);
                    self.router.hangup(self.now.mono_ms, id);
                }
                // There is nothing to show the host for these; they are
                // counted.
                ModemEvent::Retrain | ModemEvent::RateChange(_) => self.tallies.retrain(),
            }
        }
    }

    /// Ends the media of a line's call, if it has any, and takes the
    /// engine back from the runner.
    fn stop_engine(&mut self, index: usize) {
        let line = &mut self.lines[index];
        line.to_line.clear();
        let Some(call) = line.media.take() else {
            return;
        };
        let id = line.id();
        // A runner that lost the engine with whatever ran it gives none
        // back; the line gets a new one when it next needs it.
        self.lines[index].engine = self.media.stop(id, call);
    }

    // ----------------------------------------------------------------------
    // Calls
    // ----------------------------------------------------------------------

    fn start_call(&mut self, index: usize, direction: Direction, number: String) {
        if self.lines[index].call.is_some() {
            // A call cannot start on top of another; close the old one.
            self.end_call(index, EndReason::Failed);
        }
        let call = ActiveCall {
            id: self.next_call_id,
            direction,
            number,
            started_ms: self.now.unix_ms,
            connected_ms: None,
            connect: None,
            bytes_tx: 0,
            bytes_rx: 0,
            answered: false,
        };
        self.next_call_id += 1;
        let line = &mut self.lines[index];
        match direction {
            Direction::Outgoing => line.counters.calls_out += 1,
            Direction::Incoming => line.counters.calls_in += 1,
        }
        let id = line.id();
        let source = line.source();
        let info = call.info();
        let started = message(
            "RMD1006",
            &[&id.to_string(), direction.as_str(), &call.number],
        );
        line.call = Some(call);
        let in_use = self.lines_in_use();
        self.tallies.in_use(in_use);
        self.publish(Event::CallStarted(CallEvent {
            line: id,
            call: info,
        }));
        self.log_message(&source, &started);
    }

    fn end_call(&mut self, index: usize, reason: EndReason) {
        let line = &mut self.lines[index];
        let Some(call) = line.call.take() else {
            return;
        };
        let id = line.id();
        let source = line.source();
        let answered = call.answered;
        let record = call.record(id, self.now.unix_ms, reason);
        self.tallies.ended(&record, answered);
        let ended = message(
            "RMD1008",
            &[
                &id.to_string(),
                reason.as_str(),
                &record.bytes_tx.to_string(),
                &record.bytes_rx.to_string(),
            ],
        );

        if self.history.len() == CALL_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(record.clone());
        self.append_to_call_log(&record);
        self.publish(Event::CallEnded(record));
        self.log_message(&source, &ended);
    }

    fn append_to_call_log(&mut self, record: &CallRecord) {
        let Some(log) = &mut self.call_log else {
            return;
        };
        let written = serde_json::to_string(record)
            .map_err(std::io::Error::other)
            .and_then(|json| writeln!(log, "{json}"))
            .and_then(|()| log.flush());
        match written {
            Ok(()) => self.call_log_failed = false,
            Err(error) => {
                // Say so once, not for every call while the disk is full.
                if !self.call_log_failed {
                    self.call_log_failed = true;
                    self.log_message(SOURCE, &message("RMD1013", &[&error.to_string()]));
                }
            }
        }
    }

    /// Turns away a call offered to a line: the caller is told the call
    /// failed, and the line stops ringing.
    fn reject(&mut self, index: usize, reason: EndReason) {
        let line = &mut self.lines[index];
        if !line.incoming {
            return;
        }
        line.incoming = false;
        let id = line.id();
        self.step(index, Input::Event(LineEvent::RingEnd));
        self.end_call(index, reason);
        self.router.hangup(self.now.mono_ms, id);
    }

    /// A line that is disabled or not ready cannot be rung.
    fn reject_unready(&mut self) {
        for index in 0..self.lines.len() {
            let line = &self.lines[index];
            if line.incoming && matches!(line.state(), LineState::Disabled | LineState::NotReady) {
                self.reject(index, EndReason::Failed);
            }
        }
    }

    // ----------------------------------------------------------------------
    // Serial ports
    // ----------------------------------------------------------------------

    /// Opens the port of every enabled line that has none, when its retry
    /// time has come.
    fn open_ports(&mut self) {
        for index in 0..self.lines.len() {
            let line = &self.lines[index];
            if !line.config.enabled || line.serial.is_some() {
                continue;
            }
            if self.now.mono_ms < line.retry_at_ms {
                continue;
            }
            let source = line.source();
            match self.serial.open(&line.config) {
                Ok(serial) => {
                    let opened = message("RMD3008", &[&line.config.serial.port]);
                    let line = &mut self.lines[index];
                    line.serial = Some(serial);
                    line.error = None;
                    line.applied = None;
                    self.log_message(&source, &opened);
                }
                Err(problem) => {
                    let line = &mut self.lines[index];
                    line.retry_at_ms = self.now.mono_ms + PORT_RETRY_MS;
                    // Say so when the reason changes, not at every retry.
                    let changed = line.error.as_ref() != Some(&problem);
                    line.error = Some(problem.clone());
                    if changed {
                        self.log_message(&source, &problem);
                    }
                }
            }
        }
    }

    /// Takes what the host did to a line's port and what it wrote.
    fn serve_port(&mut self, index: usize) {
        loop {
            let Some(serial) = self.lines[index].serial.as_mut() else {
                return;
            };
            match serial.poll() {
                Ok(Some(event)) => self.serial_event(index, event),
                Ok(None) => break,
                Err(error) => {
                    self.port_failed(index, SerialOperation::Poll, &error);
                    return;
                }
            }
        }

        // Read no more than there is room for. Reading in step with what
        // the engine takes keeps the data arriving at the interpreter as
        // steadily as the host sent it, which the escape guard time needs:
        // a pause made here would look like silence from the host.
        let mut buffer = [0u8; CHUNK];
        loop {
            let line = &mut self.lines[index];
            let room = HIGH_WATER.saturating_sub(line.to_line.len()).min(CHUNK);
            if room == 0 {
                break;
            }
            let Some(serial) = line.serial.as_mut() else {
                return;
            };
            match serial.read(&mut buffer[..room]) {
                Ok(0) => break,
                Ok(count) => {
                    self.step(index, Input::DteBytes(&buffer[..count]));
                    if count < room {
                        break;
                    }
                }
                Err(error) => {
                    self.port_failed(index, SerialOperation::Read, &error);
                    return;
                }
            }
        }
    }

    fn serial_event(&mut self, index: usize, event: SerialEvent) {
        match event {
            SerialEvent::Dtr(level) => {
                self.lines[index].dtr = level;
                self.step(index, Input::Dtr(level));
            }
            // Flow control is stored and has no effect in phase 0.
            SerialEvent::Rts(_) => {}
            SerialEvent::HostOpened => {}
            SerialEvent::HostClosed => {
                // Nobody is there to read what was waiting.
                let line = &mut self.lines[index];
                line.to_host.clear();
                if line.dtr {
                    line.dtr = false;
                    self.step(index, Input::Dtr(false));
                }
            }
        }
    }

    /// A port stopped working: report it, close it, hang up, and try to
    /// open it again later.
    fn port_failed(&mut self, index: usize, operation: SerialOperation, error: &std::io::Error) {
        let line = &mut self.lines[index];
        let problem = serial::error_message(&line.config.serial.port, operation, error);
        let source = line.source();
        line.serial = None;
        line.applied = None;
        line.error = Some(problem.clone());
        line.retry_at_ms = self.now.mono_ms + PORT_RETRY_MS;
        line.to_host.clear();
        let had_dtr = std::mem::take(&mut line.dtr);
        self.log_message(&source, &problem);
        // Whatever &D says, a call cannot go on without its port. It is
        // recorded as failed, not as something the host or operator did.
        let reason = std::mem::replace(&mut self.operator_reason, EndReason::Failed);
        self.with_interpreter(index, |interpreter, out| interpreter.operator_hangup(out));
        self.settle();
        self.operator_reason = reason;
        if had_dtr {
            self.step(index, Input::Dtr(false));
        }
    }

    /// Moves data between a line's buffers and the media of its call.
    fn pump(&mut self, index: usize) {
        // Host to far end.
        let line = &mut self.lines[index];
        let Some(call) = &mut line.media else {
            return;
        };
        while !line.to_line.is_empty() {
            let (front, _) = line.to_line.as_slices();
            let taken = call.write(front);
            if taken == 0 {
                break;
            }
            line.to_line.drain(..taken);
        }
        // Far end to host.
        let mut buffer = [0u8; CHUNK];
        for _ in 0..4 {
            let line = &mut self.lines[index];
            if line.to_host.len() >= HIGH_WATER {
                break;
            }
            let Some(call) = &mut line.media else {
                break;
            };
            let count = call.read(&mut buffer);
            if count == 0 {
                break;
            }
            line.counters.bytes_rx += count as u64;
            self.tallies.bytes_in(count as u64);
            if let Some(call) = &mut line.call {
                call.bytes_rx += count as u64;
            }
            self.step(index, Input::LineBytes(&buffer[..count]));
        }
    }

    /// Writes what is waiting for the host and applies the signals.
    fn flush(&mut self, index: usize) {
        let line = &mut self.lines[index];
        let Some(serial) = line.serial.as_mut() else {
            line.to_host.clear();
            return;
        };
        while !line.to_host.is_empty() {
            let (front, _) = line.to_host.as_slices();
            match serial.write(front) {
                Ok(0) => break,
                Ok(taken) => {
                    line.to_host.drain(..taken);
                }
                Err(error) => {
                    self.port_failed(index, SerialOperation::Write, &error);
                    return;
                }
            }
        }
        let wanted = line.interpreter.signals();
        if line.applied != Some(wanted) {
            match serial.set_signals(wanted) {
                Ok(()) => line.applied = Some(wanted),
                Err(error) => self.port_failed(index, SerialOperation::SetSignals, &error),
            }
        }
    }

    // ----------------------------------------------------------------------
    // The operator
    // ----------------------------------------------------------------------

    /// Every line, in id order.
    pub fn lines(&self) -> Vec<LineInfo> {
        self.lines.iter().map(Line::info).collect()
    }

    /// One line.
    pub fn line(&self, id: LineId) -> Result<LineInfo, Message> {
        self.index(id).map(|index| self.lines[index].info())
    }

    /// Answers the call ringing on a line, as `ATA` from the host would.
    pub fn answer(&mut self, id: LineId) -> Result<(), Message> {
        let index = self.index(id)?;
        let line = &self.lines[index];
        if !line.config.enabled {
            return Err(message("RMD1002", &[&id.to_string()]));
        }
        let state = line.state();
        if state != LineState::Ringing {
            return Err(message("RMD1003", &[&id.to_string(), state.as_str()]));
        }
        self.with_interpreter(index, |interpreter, out| interpreter.operator_answer(out));
        self.settle();
        self.finish();
        Ok(())
    }

    /// Hangs up a line's call, or turns away the call ringing on it. The
    /// host is told `NO CARRIER`.
    pub fn hangup(&mut self, id: LineId) -> Result<(), Message> {
        let index = self.index(id)?;
        let line = &self.lines[index];
        if !line.incoming && !line.interpreter.off_hook() {
            return Err(message("RMD1004", &[&id.to_string()]));
        }
        self.reject(index, EndReason::Operator);
        self.with_interpreter(index, |interpreter, out| interpreter.operator_hangup(out));
        self.settle();
        self.finish();
        Ok(())
    }

    /// Resets a line: hangs up, and returns its modem settings to its
    /// stored profile, or to their defaults if it has none.
    pub fn reset(&mut self, id: LineId) -> Result<(), Message> {
        let index = self.index(id)?;
        let source = self.lines[index].source();
        self.reject(index, EndReason::Operator);
        self.with_interpreter(index, |interpreter, out| interpreter.operator_reset(out));
        self.settle();
        self.finish();
        self.log_message(&source, &message("RMD1009", &[&id.to_string()]));
        Ok(())
    }

    /// The calls that have ended, oldest first; at most the last `last`.
    pub fn calls(&self, last: Option<usize>) -> Vec<CallRecord> {
        let skip = match last {
            Some(last) => self.history.len().saturating_sub(last),
            None => 0,
        };
        self.history.iter().skip(skip).cloned().collect()
    }

    /// The statistics since the pool started.
    pub fn stats(&self) -> Stats {
        self.tallies.snapshot(self.lines_in_use())
    }

    /// The statistics of the media of a line's call, when the line has a
    /// call connected and its media runner keeps any.
    pub fn media_stats(&self, id: LineId) -> Option<MediaStats> {
        let index = self.index_of(id)?;
        self.lines[index].media.as_ref()?.stats()
    }

    fn lines_in_use(&self) -> u32 {
        self.lines.iter().filter(|line| line.call.is_some()).count() as u32
    }

    /// Tells the pool what the host side of each port is bound to: pairs
    /// of port name and text. A line whose binding changes is published,
    /// so that clients see it.
    pub fn set_bindings(&mut self, bindings: Vec<(String, String)>) {
        self.bindings = bindings
            .into_iter()
            .map(|(port, text)| (port.to_ascii_lowercase(), text))
            .collect();
        for index in 0..self.lines.len() {
            let binding = self.binding_of(&self.lines[index].config);
            let line = &mut self.lines[index];
            if line.binding != binding {
                line.binding = binding;
                let info = line.info();
                self.publish(Event::LineState(Box::new(info)));
            }
        }
    }

    fn binding_of(&self, config: &LineConfig) -> Option<String> {
        // Windows port names do not distinguish case.
        self.bindings
            .get(&config.serial.port.to_ascii_lowercase())
            .cloned()
    }

    /// Makes the lines match a configuration: adds the lines it has that
    /// the pool does not, removes the lines the pool has that it does not,
    /// and changes the ones that differ.
    pub fn apply_config(&mut self, config: &Config) {
        self.router.configure(self.now.mono_ms, config);

        // Remove.
        let mut index = 0;
        while index < self.lines.len() {
            let id = self.lines[index].id();
            if config.line(id).is_some() {
                index += 1;
                continue;
            }
            let source = self.lines[index].source();
            self.take_down(index);
            self.lines.remove(index);
            // A line added later with the same id is another line, and
            // does not start from what this one stored.
            if let Err(problem) = self.profiles.remove(id) {
                self.log_message(&source, &problem);
            }
            self.log_message(&source, &message("RMD1011", &[&id.to_string()]));
        }

        // Add and change.
        for wanted in &config.lines {
            match self.index_of(wanted.id) {
                None => {
                    let engine = (self.engines)(wanted);
                    let stored = self.profiles.load(wanted.id);
                    let mut line = Line::new(wanted.clone(), engine, stored);
                    line.binding = self.binding_of(wanted);
                    let source = line.source();
                    self.lines.push(line);
                    self.lines.sort_by_key(Line::id);
                    self.log_message(&source, &message("RMD1010", &[&wanted.id.to_string()]));
                }
                Some(index) if self.lines[index].config != *wanted => {
                    self.change_line(index, wanted);
                }
                Some(_) => {}
            }
        }
        self.finish();
    }

    fn change_line(&mut self, index: usize, wanted: &LineConfig) {
        let current = &self.lines[index].config;
        let reopen = current.serial != wanted.serial || (current.enabled && !wanted.enabled);
        if reopen {
            // The port changes, or the line goes out of service: the call
            // ends and the port is released.
            self.take_down(index);
        }
        if self.lines[index].config.engine != wanted.engine {
            // Another engine: the call the old one is carrying ends, and
            // the line gets the new one.
            let reason = self.operator_reason;
            self.reject(index, reason);
            self.with_interpreter(index, |interpreter, out| interpreter.operator_hangup(out));
            self.settle();
            self.lines[index].engine = Some((self.engines)(wanted));
        }
        let binding = self.binding_of(wanted);
        let line = &mut self.lines[index];
        line.config = wanted.clone();
        line.binding = binding;
        line.interpreter.set_config(at_config(wanted));
        line.retry_at_ms = 0;
        let source = line.source();
        self.log_message(&source, &message("RMD1012", &[&wanted.id.to_string()]));
    }

    /// Ends whatever a line is doing and releases its port.
    fn take_down(&mut self, index: usize) {
        let reason = self.operator_reason;
        self.reject(index, reason);
        self.with_interpreter(index, |interpreter, out| interpreter.operator_reset(out));
        self.settle();
        // Let the host see NO CARRIER and every signal go off before the
        // port closes.
        self.flush(index);
        let line = &mut self.lines[index];
        if let Some(serial) = line.serial.as_mut() {
            let _ = serial.set_signals(ModemSignals::default());
        }
        line.serial = None;
        line.applied = None;
        line.error = None;
        line.dtr = false;
        line.to_host.clear();
        line.to_line.clear();
    }

    /// Stops the pool: every call is hung up and every port released.
    pub fn shutdown(&mut self, now: Now) {
        self.now = now;
        self.operator_reason = EndReason::Shutdown;
        for index in 0..self.lines.len() {
            self.take_down(index);
        }
        self.settle();
        self.router.shutdown(now.mono_ms);
        self.operator_reason = EndReason::Operator;
        if let Some(log) = &mut self.call_log {
            let _ = log.flush();
        }
    }

    /// After [`Pool::shutdown`]: lets the router finish what it has in
    /// flight on the network, a BYE or the removal of a registration.
    /// Returns whether it still has something; call again a little later
    /// if so, for as long as there is time to.
    pub fn linger(&mut self, now: Now) -> bool {
        self.now = now;
        // The lines are down: whatever the router says now is about calls
        // that are over.
        while self
            .router
            .poll(now.mono_ms, &Directory(&self.lines))
            .is_some()
        {}
        self.router.busy()
    }

    /// Carries out a command from another thread.
    pub fn command(&mut self, command: Command) {
        // A caller that has stopped waiting for its answer is not an error.
        match command {
            Command::Lines(answer) => {
                let _ = answer.send(self.lines());
            }
            Command::Line(id, answer) => {
                let _ = answer.send(self.line(id));
            }
            Command::Answer(id, answer) => {
                let _ = answer.send(self.answer(id));
            }
            Command::Hangup(id, answer) => {
                let _ = answer.send(self.hangup(id));
            }
            Command::Reset(id, answer) => {
                let _ = answer.send(self.reset(id));
            }
            Command::Calls(last, answer) => {
                let _ = answer.send(self.calls(last));
            }
            Command::Stats(answer) => {
                let _ = answer.send(self.stats());
            }
            Command::SetBindings(bindings, answer) => {
                self.set_bindings(bindings);
                let _ = answer.send(());
            }
            Command::ApplyConfig(config, answer) => {
                self.apply_config(&config);
                let _ = answer.send(());
            }
        }
    }

    // ----------------------------------------------------------------------
    // Events
    // ----------------------------------------------------------------------

    fn publish_states(&mut self) {
        for index in 0..self.lines.len() {
            let line = &mut self.lines[index];
            let state = line.state();
            if line.published == Some(state) {
                continue;
            }
            line.published = Some(state);
            let info = line.info();
            let source = line.source();
            let changed = message("RMD1005", &[&info.id.to_string(), state.as_str()]);
            self.publish(Event::LineState(Box::new(info)));
            self.log_message(&source, &changed);
        }
    }

    fn publish(&mut self, event: Event) {
        self.sink.publish(event);
    }

    fn log(&mut self, level: LogLevel, source: &str, text: String) {
        self.sink.publish(Event::Log(LogEvent {
            time_ms: self.now.unix_ms,
            level,
            source: source.to_string(),
            text,
        }));
    }

    fn log_message(&mut self, source: &str, message: &Message) {
        let level = match message.severity {
            Severity::Info => LogLevel::Info,
            Severity::Warning => LogLevel::Warning,
            Severity::Error => LogLevel::Error,
        };
        self.log(level, source, message.to_string());
    }

    fn index_of(&self, id: LineId) -> Option<usize> {
        self.lines.iter().position(|line| line.id() == id)
    }

    fn index(&self, id: LineId) -> Result<usize, Message> {
        self.index_of(id)
            .ok_or_else(|| message("RMD1001", &[&id.to_string()]))
    }
}
