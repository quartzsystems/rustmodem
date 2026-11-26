// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The transcript format and its runner.
//!
//! A transcript is a text file, one directive per line. The first two
//! characters say what the line is:
//!
//! ```text
//! # comment           (blank lines are skipped too)
//! > bytes             the host writes these bytes to the modem
//! < bytes             the modem must have written exactly these to the host
//! @ event             something happens: an event or time passing
//! ! expectation       something else the modem must have done or be
//! ```
//!
//! Events (`@`):
//!
//! ```text
//! @ config unknown_command=error auto_answer_rings=2    first line only
//! @ elapsed 1000                 milliseconds pass (one Elapsed input)
//! @ ring | ringend | carrierlost | busy | noanswer | nodialtone
//! @ connected [rate [carrier [protocol [compression]]]]
//! @ dtr on|off
//! @ line bytes                   bytes arrive from the far end
//! ```
//!
//! Expectations (`!`):
//!
//! ```text
//! ! dial 5551234 | answer | hangup command|dtr|abort|timeout|reset
//! ! line bytes                   the modem sent these to the far end
//! ! mode command|ringing|handshaking|online|onlinecommand
//! ! signals dcd=1 ri=0           any of dcd, dsr, cts, ri
//! ! s 7=50                       the value of an S-register
//! ```
//!
//! Bytes are written as text with the escapes `\r`, `\n`, `\t`, `\\` and
//! `\xNN`; `${VERSION}` stands for the crate version.
//!
//! The runner is strict. Everything the modem writes to the host, every
//! byte it sends to the far end and every action it asks for must be
//! claimed by a `<` or `!` line before the next `>` or `@` line and before
//! the end of the transcript. A transcript starts with DTR on.

#![allow(dead_code)]

use std::collections::VecDeque;

use at::{
    Action, AtConfig, AtInterpreter, ConnectInfo, HangupCause, Input, LineEvent, Mode, Output,
    UnknownCommand,
};

/// Runs a transcript and panics, naming it, if the modem does not follow it.
pub fn run(name: &str, text: &str) {
    if let Err(problem) = try_run(text) {
        panic!("transcript {name}: {problem}");
    }
}

/// Runs a transcript.
pub fn try_run(text: &str) -> Result<(), String> {
    let mut runner = Runner::new(AtConfig::default());
    let mut first = true;
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let raw = raw.trim_end_matches('\r');
        if raw.trim().is_empty() || raw.starts_with('#') {
            continue;
        }
        let (kind, rest) = match (raw.get(..1), raw.get(1..2)) {
            (Some(kind), Some(" ")) => (kind, &raw[2..]),
            (Some(kind), None) => (kind, ""),
            _ => return Err(format!("line {number}: not a directive: {raw}")),
        };
        let step = match kind {
            ">" => runner
                .settled()
                .and_then(|()| unescape(rest))
                .map(|bytes| runner.feed(Input::DteBytes(&bytes))),
            "<" => unescape(rest).and_then(|bytes| runner.expect_dte(&bytes)),
            "@" => runner.settled().and_then(|()| runner.event(rest, first)),
            "!" => runner.expectation(rest),
            _ => Err(format!("not a directive: {raw}")),
        };
        step.map_err(|problem| format!("line {number}: {problem}"))?;
        first = false;
    }
    runner
        .settled()
        .map_err(|problem| format!("at the end: {problem}"))
}

struct Runner {
    interpreter: AtInterpreter,
    dte: Vec<u8>,
    line: Vec<u8>,
    actions: VecDeque<Action>,
}

impl Runner {
    fn new(config: AtConfig) -> Runner {
        let mut runner = Runner {
            interpreter: AtInterpreter::new(config),
            dte: Vec::new(),
            line: Vec::new(),
            actions: VecDeque::new(),
        };
        runner.feed(Input::Dtr(true));
        runner
    }

    fn feed(&mut self, input: Input<'_>) {
        let mut out = Vec::new();
        self.interpreter.step(input, &mut out);
        if let Err(problem) = self.interpreter.check_invariants() {
            panic!("invariant broken: {problem}");
        }
        for output in out {
            match output {
                Output::DteBytes(bytes) => self.dte.extend(bytes),
                Output::LineBytes(bytes) => self.line.extend(bytes),
                Output::Action(action) => self.actions.push_back(action),
                Output::Signals(_) | Output::Trace(_) => {}
            }
        }
    }

    /// Everything the modem produced has been claimed.
    fn settled(&mut self) -> Result<(), String> {
        if !self.dte.is_empty() {
            return Err(format!(
                "unclaimed bytes to the host: {}",
                escape(&self.dte)
            ));
        }
        if !self.line.is_empty() {
            return Err(format!(
                "unclaimed bytes to the far end: {}",
                escape(&self.line)
            ));
        }
        if let Some(action) = self.actions.front() {
            return Err(format!("unclaimed action: {action:?}"));
        }
        Ok(())
    }

    fn expect_dte(&mut self, expected: &[u8]) -> Result<(), String> {
        claim(&mut self.dte, expected, "to the host")
    }

    fn event(&mut self, text: &str, first: bool) -> Result<(), String> {
        let mut words = text.split_whitespace();
        let name = words.next().ok_or("empty event")?;
        match name {
            "config" => {
                if !first {
                    return Err("config must be the first line".to_string());
                }
                let mut config = AtConfig::default();
                for word in words {
                    match word.split_once('=') {
                        Some(("unknown_command", "ok")) => {
                            config.unknown_command = UnknownCommand::Ok;
                        }
                        Some(("unknown_command", "error")) => {
                            config.unknown_command = UnknownCommand::Error;
                        }
                        Some(("auto_answer_rings", value)) => {
                            config.auto_answer_rings = number(value)?;
                        }
                        _ => return Err(format!("unknown setting: {word}")),
                    }
                }
                *self = Runner::new(config);
            }
            "elapsed" => {
                let ms: u32 = number(words.next().ok_or("elapsed needs milliseconds")?)?;
                self.feed(Input::Elapsed(ms));
            }
            "ring" => self.feed(Input::Event(LineEvent::Ring)),
            "ringend" => self.feed(Input::Event(LineEvent::RingEnd)),
            "carrierlost" => self.feed(Input::Event(LineEvent::CarrierLost)),
            "busy" => self.feed(Input::Event(LineEvent::Busy)),
            "noanswer" => self.feed(Input::Event(LineEvent::NoAnswer)),
            "nodialtone" => self.feed(Input::Event(LineEvent::NoDialtone)),
            "connected" => {
                let info = ConnectInfo {
                    rate: words.next().map(number).transpose()?.unwrap_or(0),
                    carrier: words.next().map(str::to_string),
                    protocol: words.next().map(str::to_string),
                    compression: words.next().map(str::to_string),
                };
                self.feed(Input::Event(LineEvent::Connected(info)));
            }
            "dtr" => match words.next() {
                Some("on") => self.feed(Input::Dtr(true)),
                Some("off") => self.feed(Input::Dtr(false)),
                _ => return Err("dtr needs on or off".to_string()),
            },
            "line" => {
                let bytes = unescape(text.strip_prefix("line").unwrap_or("").trim_start())?;
                self.feed(Input::LineBytes(&bytes));
            }
            other => return Err(format!("unknown event: {other}")),
        }
        Ok(())
    }

    fn expectation(&mut self, text: &str) -> Result<(), String> {
        let mut words = text.split_whitespace();
        let name = words.next().ok_or("empty expectation")?;
        match name {
            "dial" => {
                let number = words.next().unwrap_or("");
                self.expect_action(Action::Dial(number.to_string()))
            }
            "answer" => self.expect_action(Action::Answer),
            "hangup" => {
                let cause = match words.next() {
                    Some("command") => HangupCause::Command,
                    Some("dtr") => HangupCause::Dtr,
                    Some("abort") => HangupCause::Abort,
                    Some("timeout") => HangupCause::Timeout,
                    Some("reset") => HangupCause::Reset,
                    Some("operator") => HangupCause::Operator,
                    _ => return Err("hangup needs a cause".to_string()),
                };
                self.expect_action(Action::Hangup(cause))
            }
            "line" => {
                let bytes = unescape(text.strip_prefix("line").unwrap_or("").trim_start())?;
                claim(&mut self.line, &bytes, "to the far end")
            }
            "mode" => {
                let wanted = match words.next() {
                    Some("command") => Mode::Command,
                    Some("ringing") => Mode::Ringing,
                    Some("handshaking") => Mode::Handshaking,
                    Some("online") => Mode::OnlineData,
                    Some("onlinecommand") => Mode::OnlineCommand,
                    _ => return Err("mode needs a mode name".to_string()),
                };
                let mode = self.interpreter.mode();
                if mode == wanted {
                    Ok(())
                } else {
                    Err(format!("mode is {mode:?}, expected {wanted:?}"))
                }
            }
            "signals" => {
                let signals = self.interpreter.signals();
                for word in words {
                    let (name, value) = word.split_once('=').ok_or("signal needs name=0|1")?;
                    let wanted = match value {
                        "0" => false,
                        "1" => true,
                        _ => return Err("signal needs name=0|1".to_string()),
                    };
                    let actual = match name {
                        "dcd" => signals.dcd,
                        "dsr" => signals.dsr,
                        "cts" => signals.cts,
                        "ri" => signals.ri,
                        _ => return Err(format!("unknown signal: {name}")),
                    };
                    if actual != wanted {
                        return Err(format!("signal {name} is {}", u8::from(actual)));
                    }
                }
                Ok(())
            }
            "s" => {
                let (register, value) = words
                    .next()
                    .and_then(|word| word.split_once('='))
                    .ok_or("s needs register=value")?;
                let register: u8 = number(register)?;
                let wanted: u8 = number(value)?;
                match self.interpreter.register(register) {
                    Some(actual) if actual == wanted => Ok(()),
                    Some(actual) => Err(format!("S{register} is {actual}, expected {wanted}")),
                    None => Err(format!("there is no S{register}")),
                }
            }
            other => Err(format!("unknown expectation: {other}")),
        }
    }

    fn expect_action(&mut self, wanted: Action) -> Result<(), String> {
        match self.actions.pop_front() {
            Some(action) if action == wanted => Ok(()),
            Some(action) => Err(format!("action was {action:?}, expected {wanted:?}")),
            None => Err(format!("no action, expected {wanted:?}")),
        }
    }
}

fn number<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    text.parse().map_err(|_| format!("not a number: {text}"))
}

/// Takes `expected` off the front of `pending`, or says how they differ.
fn claim(pending: &mut Vec<u8>, expected: &[u8], what: &str) -> Result<(), String> {
    if pending.starts_with(expected) {
        pending.drain(..expected.len());
        Ok(())
    } else {
        Err(format!(
            "bytes {what} were {}, expected {}",
            escape(pending),
            escape(expected)
        ))
    }
}

/// Reads the text form of bytes.
pub fn unescape(text: &str) -> Result<Vec<u8>, String> {
    let text = text.replace("${VERSION}", env!("CARGO_PKG_VERSION"));
    let mut bytes = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('r') => bytes.push(b'\r'),
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('\\') => bytes.push(b'\\'),
            Some('x') => {
                let digits: String = chars.by_ref().take(2).collect();
                let value = u8::from_str_radix(&digits, 16)
                    .ok()
                    .filter(|_| digits.len() == 2)
                    .ok_or_else(|| format!("bad escape: \\x{digits}"))?;
                bytes.push(value);
            }
            other => return Err(format!("bad escape: \\{}", other.unwrap_or(' '))),
        }
    }
    Ok(bytes)
}

/// Writes bytes in their text form.
pub fn escape(bytes: &[u8]) -> String {
    let mut text = String::new();
    for byte in bytes {
        match byte {
            b'\r' => text.push_str("\\r"),
            b'\n' => text.push_str("\\n"),
            b'\t' => text.push_str("\\t"),
            b'\\' => text.push_str("\\\\"),
            0x20..=0x7e => text.push(char::from(*byte)),
            _ => text.push_str(&format!("\\x{byte:02x}")),
        }
    }
    if text.is_empty() {
        text.push_str("(nothing)");
    }
    text
}
