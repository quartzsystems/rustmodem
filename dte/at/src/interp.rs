// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The interpreter: modes, command line assembly, the escape sequence,
//! ringing, timers, signals and result codes.

use crate::profile::Profile;
use crate::{
    Action, AtConfig, ConnectInfo, HangupCause, Input, LineEvent, MAX_COMMAND_LINE, MAX_HELD_DATA,
    Mode, ModemSignals, Output, RING_COUNT_RESET_MS, ResultCode, Trace, connect_number,
};

/// Where the line is, as the interpreter sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// On hook, accepting commands.
    OnHook,
    /// Off hook, waiting for the modems to connect.
    Handshaking { originate: bool, waited_ms: u32 },
    /// Connected, passing data.
    Online,
    /// Connected, accepting commands.
    OnlineCommand,
    /// Off hook after a dial string that ended in `;`: the call is in
    /// progress and the interpreter accepts commands.
    Dialled,
}

/// How far the command line prefix has got (V.250 clause 5.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for `A`.
    Idle,
    /// Seen `A`; waiting for `T` or `/`.
    SeenA,
    /// Seen `AT`; collecting the body up to the terminator.
    Body,
}

/// The Hayes-compatible command interpreter for one line.
#[derive(Clone, Debug)]
pub struct AtInterpreter {
    pub(crate) config: AtConfig,
    pub(crate) profile: Profile,
    /// The stored profile: what `&W` last wrote, or what the interpreter
    /// was created with.
    stored: Option<Profile>,
    pub(crate) state: State,
    phase: Phase,
    /// The command line body so far, without the prefix.
    line: Vec<u8>,
    /// The body ran past [`MAX_COMMAND_LINE`].
    overflow: bool,
    /// The last body executed, for `A/`.
    previous: Vec<u8>,
    /// Milliseconds since the last ring, while S1 is not zero.
    ring_idle_ms: u32,
    /// The ring-on interval is in progress.
    ri: bool,
    /// Milliseconds since the host last sent a byte in online data mode.
    silence_ms: u32,
    /// How many escape characters of a possible escape sequence have come.
    escape_count: u8,
    dtr: bool,
    /// Flow control back-pressure toward the host.
    flow_hold: bool,
    pub(crate) connect: Option<ConnectInfo>,
    /// Data received while in online command mode, given to the host when
    /// it returns to data mode.
    held: Vec<u8>,
    /// The signals last reported.
    signals: ModemSignals,
}

/// A profile as it is stored: S1 counts the rings of a call in progress,
/// which is not a setting, so it is stored as zero.
fn storable(mut profile: Profile) -> Profile {
    profile.set_s1(0);
    profile
}

impl AtInterpreter {
    /// A new interpreter with no stored profile, at its factory settings,
    /// on hook, with DTR off until an [`Input::Dtr`] says otherwise.
    pub fn new(config: AtConfig) -> AtInterpreter {
        AtInterpreter::with_profile(config, None)
    }

    /// A new interpreter that starts from a stored profile, as a modem
    /// does when it is switched on. With no stored profile, or with one
    /// that does not pass [`Profile::check`], it starts from its factory
    /// settings and has nothing stored.
    pub fn with_profile(config: AtConfig, stored: Option<Profile>) -> AtInterpreter {
        let stored = stored
            .filter(|profile| profile.check().is_ok())
            .map(storable);
        let mut interpreter = AtInterpreter {
            config,
            profile: stored
                .clone()
                .unwrap_or_else(|| Profile::factory(config.auto_answer_rings)),
            stored,
            state: State::OnHook,
            phase: Phase::Idle,
            line: Vec::new(),
            overflow: false,
            previous: Vec::new(),
            ring_idle_ms: 0,
            ri: false,
            silence_ms: 0,
            escape_count: 0,
            dtr: false,
            flow_hold: false,
            connect: None,
            held: Vec::new(),
            signals: ModemSignals::default(),
        };
        interpreter.signals = interpreter.wanted_signals();
        interpreter
    }

    /// Gives the interpreter one input and collects what it produces.
    pub fn step(&mut self, input: Input<'_>, out: &mut Vec<Output>) {
        match input {
            Input::DteBytes(bytes) => self.dte_bytes(bytes, out),
            Input::LineBytes(bytes) => self.line_bytes(bytes, out),
            Input::Elapsed(ms) => self.elapsed(ms, out),
            Input::Dtr(level) => self.dtr_changed(level, out),
            Input::Event(event) => self.event(event, out),
        }
        self.update_signals(out);
    }

    /// The current mode.
    pub fn mode(&self) -> Mode {
        match self.state {
            State::OnHook if self.profile.s1() > 0 => Mode::Ringing,
            State::OnHook | State::Dialled => Mode::Command,
            State::Handshaking { .. } => Mode::Handshaking,
            State::Online => Mode::OnlineData,
            State::OnlineCommand => Mode::OnlineCommand,
        }
    }

    /// Whether the line is off hook: dialling, answering or connected.
    pub fn off_hook(&self) -> bool {
        self.state != State::OnHook
    }

    /// The signals toward the host as they are now.
    pub fn signals(&self) -> ModemSignals {
        self.signals
    }

    /// The value of S-register `number`, if the interpreter has it.
    pub fn register(&self, number: u8) -> Option<u8> {
        self.profile.register(u32::from(number))
    }

    /// The settings in force now: what a call that begins now goes by.
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The stored profile, if there is one.
    pub fn stored_profile(&self) -> Option<&Profile> {
        self.stored.as_ref()
    }

    /// What is known about the current connection.
    pub fn connect_info(&self) -> Option<&ConnectInfo> {
        self.connect.as_ref()
    }

    /// Replaces the per-line settings. The unknown-command setting applies
    /// from the next command; the S0 value applies from the next return to
    /// the factory settings.
    pub fn set_config(&mut self, config: AtConfig) {
        self.config = config;
    }

    /// Flow control back-pressure: while held, and with `&K3` in effect,
    /// CTS is off. This is the hook for later phases; phase 0 never holds.
    pub fn set_flow_hold(&mut self, hold: bool, out: &mut Vec<Output>) {
        self.flow_hold = hold;
        self.update_signals(out);
    }

    /// Answers as an `A` command would, on behalf of the operator. Returns
    /// `false`, doing nothing, if the line is off hook.
    pub fn operator_answer(&mut self, out: &mut Vec<Output>) -> bool {
        if self.state != State::OnHook {
            return false;
        }
        self.answer(out);
        self.update_signals(out);
        true
    }

    /// Hangs up on behalf of the operator and tells the host `NO CARRIER`.
    /// Returns `false`, doing nothing, if the line is on hook.
    pub fn operator_hangup(&mut self, out: &mut Vec<Output>) -> bool {
        if self.state == State::OnHook {
            return false;
        }
        out.push(Output::Action(Action::Hangup(HangupCause::Operator)));
        self.go_on_hook();
        self.result(ResultCode::NoCarrier, out);
        self.update_signals(out);
        true
    }

    /// Resets the line on behalf of the operator, as switching a modem off
    /// and on would: hangs up any call (the host is told `NO CARRIER`),
    /// drops any partial command line and returns every setting to the
    /// stored profile, or to its factory value if nothing is stored.
    pub fn operator_reset(&mut self, out: &mut Vec<Output>) {
        if self.state != State::OnHook {
            out.push(Output::Action(Action::Hangup(HangupCause::Operator)));
            self.go_on_hook();
            self.result(ResultCode::NoCarrier, out);
        }
        self.drop_command_line();
        self.previous.clear();
        self.restore_profile();
        self.update_signals(out);
    }

    /// Checks what must always hold between the interpreter's parts. The
    /// fuzz target calls this after every step.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if self.line.len() > MAX_COMMAND_LINE {
            return Err("command line longer than the limit");
        }
        if self.previous.len() > MAX_COMMAND_LINE {
            return Err("previous command line longer than the limit");
        }
        if self.escape_count > 3 {
            return Err("more than three escape characters counted");
        }
        if self.profile.s3() > 127 || self.profile.s4() > 127 || self.profile.s5() > 127 {
            return Err("S3, S4 or S5 above 127");
        }
        if self.profile.x > 4 || self.profile.dtr_action > 3 {
            return Err("X or &D setting out of range");
        }
        if self.profile.volume > 3 || self.profile.speaker > 3 {
            return Err("L or M setting out of range");
        }
        if self.profile.flow != 0 && self.profile.flow != 3 {
            return Err("&K setting out of range");
        }
        if self.held.len() > MAX_HELD_DATA {
            return Err("more held data than the limit");
        }
        if !self.held.is_empty() && self.state != State::OnlineCommand {
            return Err("held data outside online command mode");
        }
        let connected = matches!(self.state, State::Online | State::OnlineCommand);
        if self.connect.is_some() != connected {
            return Err("connection information does not match the mode");
        }
        if self.ri && self.state != State::OnHook {
            return Err("ring indicator on while off hook");
        }
        let collecting = self.phase != Phase::Idle || !self.line.is_empty() || self.overflow;
        if collecting && matches!(self.state, State::Handshaking { .. } | State::Online) {
            return Err("command line in progress outside a command mode");
        }
        if self.escape_count != 0 && self.state != State::Online {
            return Err("escape sequence in progress outside online data mode");
        }
        if self.signals != self.wanted_signals() {
            return Err("reported signals are stale");
        }
        Ok(())
    }

    // ----------------------------------------------------------------------
    // Inputs
    // ----------------------------------------------------------------------

    fn dte_bytes(&mut self, bytes: &[u8], out: &mut Vec<Output>) {
        let mut at = 0;
        while at < bytes.len() {
            match self.state {
                State::Online => {
                    // Data is passed on as it arrives; the escape detector
                    // only watches it go by.
                    let data = &bytes[at..];
                    for byte in data {
                        self.escape_byte(*byte);
                    }
                    out.push(Output::LineBytes(data.to_vec()));
                    return;
                }
                State::Handshaking { .. } => {
                    // V.250 clause 5.6.1: a character from the DTE while a
                    // call is being set up aborts it. The character itself
                    // and whatever came with it are discarded.
                    out.push(Output::Action(Action::Hangup(HangupCause::Abort)));
                    self.go_on_hook();
                    self.result(ResultCode::NoCarrier, out);
                    return;
                }
                State::OnHook | State::OnlineCommand | State::Dialled => {
                    let byte = bytes[at];
                    at += 1;
                    self.command_byte(byte, out);
                    if matches!(self.state, State::Handshaking { .. }) {
                        // The line just executed started a call. Anything
                        // that arrived in the same write (a line feed after
                        // the carriage return, typically) is not an abort.
                        return;
                    }
                }
            }
        }
    }

    /// One character in a command mode (V.250 clauses 5.2.1 to 5.2.4).
    fn command_byte(&mut self, byte: u8, out: &mut Vec<Output>) {
        match self.phase {
            Phase::Idle => {
                self.echo(&[byte], out);
                if byte.eq_ignore_ascii_case(&b'A') {
                    self.phase = Phase::SeenA;
                }
            }
            Phase::SeenA => {
                self.echo(&[byte], out);
                if byte.eq_ignore_ascii_case(&b'T') {
                    self.phase = Phase::Body;
                    self.line.clear();
                    self.overflow = false;
                } else if byte == b'/' {
                    // V.250 clause 5.2.4: A/ repeats the previous line at
                    // once, with no terminator.
                    self.phase = Phase::Idle;
                    let previous = self.previous.clone();
                    self.execute(&previous, false, out);
                } else if !byte.eq_ignore_ascii_case(&b'A') {
                    self.phase = Phase::Idle;
                }
            }
            Phase::Body => {
                if byte == self.profile.s3() {
                    self.echo(&[byte], out);
                    self.phase = Phase::Idle;
                    let line = std::mem::take(&mut self.line);
                    let overflow = std::mem::take(&mut self.overflow);
                    if !overflow {
                        self.previous.clone_from(&line);
                    }
                    self.execute(&line, overflow, out);
                } else if byte == self.profile.s5() {
                    // V.250 clause 5.2.2: the editing character removes the
                    // previous character; the prefix cannot be removed.
                    if self.line.pop().is_some() {
                        self.echo(&[byte, b' ', byte], out);
                    }
                } else if byte < 0x20 {
                    // Other control characters are ignored (clause 5.2.2).
                } else {
                    self.echo(&[byte], out);
                    if self.line.len() < MAX_COMMAND_LINE {
                        self.line.push(byte);
                    } else {
                        self.overflow = true;
                    }
                }
            }
        }
    }

    /// One byte of data from the host in online data mode, as the escape
    /// detector sees it.
    ///
    /// The sequence is: at least the guard time of silence, three escape
    /// characters each less than the guard time after the one before, then
    /// the guard time of silence again. This counts the characters; the
    /// closing silence is seen by [`Self::elapsed`].
    fn escape_byte(&mut self, byte: u8) {
        let gap = std::mem::take(&mut self.silence_ms);
        let escape = self.profile.s2();
        // S2 above 127 disables the escape.
        if escape > 127 || byte != escape {
            self.escape_count = 0;
            return;
        }
        let guard = self.profile.guard_ms();
        self.escape_count = match self.escape_count {
            0 if gap >= guard => 1,
            1 | 2 if gap < guard || guard == 0 => self.escape_count + 1,
            // Too long after the one before: this one opens a new sequence.
            1 | 2 => 1,
            // No leading silence, or a fourth character.
            _ => 0,
        };
    }

    fn line_bytes(&mut self, bytes: &[u8], out: &mut Vec<Output>) {
        match self.state {
            State::Online => push_dte(out, bytes),
            State::OnlineCommand => {
                let room = MAX_HELD_DATA - self.held.len();
                let take = bytes.len().min(room);
                self.held.extend_from_slice(&bytes[..take]);
            }
            _ => {}
        }
    }

    fn elapsed(&mut self, ms: u32, out: &mut Vec<Output>) {
        // S1 counts rings and returns to zero when none has come for a
        // while.
        if self.profile.s1() > 0 {
            self.ring_idle_ms = self.ring_idle_ms.saturating_add(ms);
            if self.ring_idle_ms >= RING_COUNT_RESET_MS {
                self.profile.set_s1(0);
                self.ring_idle_ms = 0;
                self.ri = false;
            }
        }
        match self.state {
            State::Handshaking {
                originate,
                waited_ms,
            } => {
                // V.250 clause 6.3.10: S7 limits the wait for a connection.
                let waited_ms = waited_ms.saturating_add(ms);
                if waited_ms >= self.profile.connect_wait_ms() {
                    out.push(Output::Action(Action::Hangup(HangupCause::Timeout)));
                    self.go_on_hook();
                    let code = if originate {
                        ResultCode::NoAnswer
                    } else {
                        ResultCode::NoCarrier
                    };
                    self.result(code, out);
                } else {
                    self.state = State::Handshaking {
                        originate,
                        waited_ms,
                    };
                }
            }
            State::Online => {
                self.silence_ms = self.silence_ms.saturating_add(ms);
                if self.escape_count == 3 && self.silence_ms >= self.profile.guard_ms() {
                    self.escape_count = 0;
                    self.state = State::OnlineCommand;
                    self.result(ResultCode::Ok, out);
                }
            }
            State::OnHook | State::OnlineCommand | State::Dialled => {}
        }
    }

    /// The host changed DTR (V.250 clause 6.2.9).
    fn dtr_changed(&mut self, level: bool, out: &mut Vec<Output>) {
        if level == self.dtr {
            return;
        }
        self.dtr = level;
        if level {
            return;
        }
        // DTR fell. A half-typed command line belongs to a host that has
        // gone away.
        self.drop_command_line();
        match self.profile.dtr_action {
            1 => {
                if self.state == State::Online {
                    self.escape_count = 0;
                    self.state = State::OnlineCommand;
                    self.result(ResultCode::Ok, out);
                }
            }
            2 | 3 => {
                let was_off_hook = self.state != State::OnHook;
                if was_off_hook {
                    out.push(Output::Action(Action::Hangup(HangupCause::Dtr)));
                    self.go_on_hook();
                }
                // &D3 resets as Z does.
                if self.profile.dtr_action == 3 {
                    self.restore_profile();
                }
                if was_off_hook {
                    self.result(ResultCode::Ok, out);
                }
            }
            _ => {}
        }
    }

    fn event(&mut self, event: LineEvent, out: &mut Vec<Output>) {
        match event {
            LineEvent::Ring => {
                if self.state != State::OnHook {
                    return;
                }
                let rings = self.profile.s1().saturating_add(1);
                self.profile.set_s1(rings);
                self.ring_idle_ms = 0;
                self.ri = true;
                self.result(ResultCode::Ring, out);
                // V.250 clause 6.3.8: S0 rings before answering by itself.
                // A modem that acts on DTR does not answer for a host that
                // is not there.
                let s0 = self.profile.s0();
                let host_ready = self.dtr || self.profile.dtr_action == 0;
                if s0 != 0 && rings >= s0 && host_ready {
                    self.answer(out);
                }
            }
            LineEvent::RingEnd => self.ri = false,
            LineEvent::Connected(info) => match self.state {
                State::Handshaking { .. } => {
                    self.connect = Some(info);
                    self.enter_online();
                    self.result(ResultCode::Connect, out);
                }
                State::Dialled => {
                    // The host asked to stay in command mode after dialling.
                    self.connect = Some(info);
                    self.state = State::OnlineCommand;
                }
                _ => {}
            },
            LineEvent::CarrierLost => {
                if self.state != State::OnHook {
                    self.go_on_hook();
                    self.result(ResultCode::NoCarrier, out);
                }
            }
            LineEvent::Busy | LineEvent::NoAnswer | LineEvent::NoDialtone => {
                let dialling = matches!(
                    self.state,
                    State::Handshaking {
                        originate: true,
                        ..
                    } | State::Dialled
                );
                if dialling {
                    let code = match event {
                        LineEvent::Busy => ResultCode::Busy,
                        LineEvent::NoAnswer => ResultCode::NoAnswer,
                        _ => ResultCode::NoDialtone,
                    };
                    self.go_on_hook();
                    self.result(code, out);
                }
            }
        }
    }

    // ----------------------------------------------------------------------
    // Transitions
    // ----------------------------------------------------------------------

    /// Goes off hook to answer and starts waiting for the connection.
    pub(crate) fn answer(&mut self, out: &mut Vec<Output>) {
        self.start_handshake(false);
        out.push(Output::Action(Action::Answer));
    }

    pub(crate) fn start_handshake(&mut self, originate: bool) {
        self.state = State::Handshaking {
            originate,
            waited_ms: 0,
        };
        self.ri = false;
        self.drop_command_line();
    }

    /// Off hook after a dial string that ended in `;`.
    pub(crate) fn start_dialled(&mut self) {
        self.state = State::Dialled;
        self.ri = false;
    }

    pub(crate) fn enter_online(&mut self) {
        self.state = State::Online;
        self.silence_ms = 0;
        self.escape_count = 0;
        self.drop_command_line();
    }

    pub(crate) fn go_on_hook(&mut self) {
        self.state = State::OnHook;
        self.connect = None;
        self.held.clear();
        self.escape_count = 0;
    }

    /// `&F`: every setting returns to its factory value. The stored
    /// profile is left as it is.
    pub(crate) fn factory_profile(&mut self) {
        self.profile = Profile::factory(self.config.auto_answer_rings);
        self.ring_idle_ms = 0;
    }

    /// `Z`: every setting returns to the stored profile, or to its factory
    /// value if nothing is stored.
    pub(crate) fn restore_profile(&mut self) {
        match &self.stored {
            Some(stored) => {
                self.profile = stored.clone();
                self.ring_idle_ms = 0;
            }
            None => self.factory_profile(),
        }
    }

    /// `&W`: the active profile becomes the stored one, and the line is
    /// asked to keep it.
    pub(crate) fn store_profile(&mut self, out: &mut Vec<Output>) {
        let stored = storable(self.profile.clone());
        self.stored = Some(stored.clone());
        out.push(Output::Action(Action::StoreProfile(stored)));
    }

    fn drop_command_line(&mut self) {
        self.phase = Phase::Idle;
        self.line.clear();
        self.overflow = false;
    }

    /// Gives the host the data held while it was in online command mode.
    pub(crate) fn release_held(&mut self, out: &mut Vec<Output>) {
        if !self.held.is_empty() {
            let held = std::mem::take(&mut self.held);
            push_dte(out, &held);
        }
    }

    // ----------------------------------------------------------------------
    // Output
    // ----------------------------------------------------------------------

    fn echo(&mut self, bytes: &[u8], out: &mut Vec<Output>) {
        // V.250 clause 6.2.4.
        if self.profile.echo {
            push_dte(out, bytes);
        }
    }

    /// Issues a result code, as the `Q`, `V` and `X` settings shape it
    /// (V.250 clauses 5.7.1, 6.2.5, 6.2.6 and 6.2.7).
    pub(crate) fn result(&mut self, code: ResultCode, out: &mut Vec<Output>) {
        let x = self.profile.x;
        // The X setting limits which call progress codes are used; one that
        // is not in the set is reported as NO CARRIER.
        let code = match code {
            ResultCode::Busy if x < 3 => ResultCode::NoCarrier,
            ResultCode::NoDialtone if x < 2 || x == 3 => ResultCode::NoCarrier,
            ResultCode::NoAnswer if x == 0 => ResultCode::NoCarrier,
            other => other,
        };
        let rate = self.connect.as_ref().map_or(0, |info| info.rate);
        let (text, number) = if code == ResultCode::Connect && x >= 1 && rate != 0 {
            (format!("CONNECT {rate}"), connect_number(rate))
        } else {
            (code.text().to_string(), code.number())
        };
        if !self.profile.quiet {
            let (s3, s4) = (self.profile.s3(), self.profile.s4());
            let mut bytes = Vec::with_capacity(text.len() + 4);
            if self.profile.verbose {
                bytes.extend_from_slice(&[s3, s4]);
                bytes.extend_from_slice(text.as_bytes());
                bytes.extend_from_slice(&[s3, s4]);
            } else {
                bytes.extend_from_slice(number.to_string().as_bytes());
                bytes.push(s3);
            }
            push_dte(out, &bytes);
        }
        out.push(Output::Trace(Trace::Result(text)));
    }

    /// Issues information text: one or more lines (V.250 clause 5.7.1).
    /// With `V0` the header is left out; `Q` does not affect it.
    pub(crate) fn info(&mut self, lines: &[&str], out: &mut Vec<Output>) {
        let (s3, s4) = (self.profile.s3(), self.profile.s4());
        let mut bytes = Vec::new();
        if self.profile.verbose {
            bytes.extend_from_slice(&[s3, s4]);
        }
        for line in lines {
            bytes.extend_from_slice(line.as_bytes());
            bytes.extend_from_slice(&[s3, s4]);
        }
        push_dte(out, &bytes);
    }

    fn wanted_signals(&self) -> ModemSignals {
        let carrier = matches!(self.state, State::Online | State::OnlineCommand);
        ModemSignals {
            // V.250 clause 6.2.8.
            dcd: !self.profile.dcd_follows_carrier || carrier,
            dsr: true,
            cts: !(self.flow_hold && self.profile.flow == 3),
            ri: self.ri,
        }
    }

    fn update_signals(&mut self, out: &mut Vec<Output>) {
        let wanted = self.wanted_signals();
        if wanted != self.signals {
            self.signals = wanted;
            out.push(Output::Signals(wanted));
        }
    }
}

/// Appends bytes for the host, joining them to the output before if that
/// was also bytes for the host.
fn push_dte(out: &mut Vec<Output>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Some(Output::DteBytes(last)) = out.last_mut() {
        last.extend_from_slice(bytes);
    } else {
        out.push(Output::DteBytes(bytes.to_vec()));
    }
}
