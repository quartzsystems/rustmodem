// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Executing a command line: the commands of V.250 clause 6 that phase 0
//! has, in the basic syntax of clause 5.3 and the extended syntax of
//! clause 5.4.

use crate::interp::{AtInterpreter, State};
use crate::profile::{
    CharacterFraming, CompressionSelection, ErrorControlSelection, FACTORY_ICF, FACTORY_MS,
    ModulationSelection, RANGE_DS, RANGE_ES, RANGE_ICF, RANGE_MS, acceptable_parameter,
};
use crate::{Action, HangupCause, Output, Profile, ResultCode, Trace, UnknownCommand};

/// What `I0` reports: the product code.
const PRODUCT_CODE: &str = "rustmodem";

/// The manufacturer and the model, which `+GMI` and `+GMM` report and
/// which the modem's INF names the modem by.
pub const MANUFACTURER: &str = "Quartz Systems";
pub const MODEL: &str = "RustModem Softmodem";

/// What `+GSN` reports. A modem made of software has no serial number.
const SERIAL_NUMBER: &str = "00000000";

/// What `+GCAP` reports: the commands of V.250 this modem has for
/// modulation, error control and compression (V.250 clause 6.1.9).
const CAPABILITY_LIST: &str = "+GCAP: +MS,+ES,+DS";

/// What `I4` reports after the name of the line's engine: the rest of
/// the capability line.
const CAPABILITIES: &str = "MODEM,V.250,+MS,+ES,+DS";

/// What executing one command means for the rest of the line.
enum Next {
    /// Go on to the next command.
    Continue,
    /// Stop; the command ended the line and issued whatever result it has.
    Done,
    /// Stop with `ERROR`.
    Fail,
}

/// The compound extended-syntax parameters.
#[derive(Clone, Copy)]
enum Stored {
    Ms,
    Icf,
    Es,
    Ds,
}

/// A position in a command line body.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    /// A decimal number, or `None` if there is no digit here. A number too
    /// large to matter saturates.
    fn number(&mut self) -> Option<u32> {
        let mut value: Option<u32> = None;
        while let Some(digit) = self.peek().filter(u8::is_ascii_digit) {
            let digit = u32::from(digit - b'0');
            value = Some(value.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            self.at += 1;
        }
        value
    }

    /// Everything left.
    fn rest(&mut self) -> &'a [u8] {
        let rest = &self.bytes[self.at..];
        self.at = self.bytes.len();
        rest
    }

    /// Everything up to, but not including, the next byte `stop` accepts.
    fn until(&mut self, stop: impl Fn(u8) -> bool) -> &'a [u8] {
        let start = self.at;
        while self.peek().is_some_and(|b| !stop(b)) {
            self.at += 1;
        }
        &self.bytes[start..self.at]
    }
}

impl AtInterpreter {
    /// Executes a command line body (what came between the `AT` prefix and
    /// the terminator). `overflow` says the body ran past the length limit.
    pub(crate) fn execute(&mut self, body: &[u8], overflow: bool, out: &mut Vec<Output>) {
        let shown = format!("AT{}", String::from_utf8_lossy(body));
        out.push(Output::Trace(Trace::Command(shown)));
        if overflow {
            self.result(ResultCode::Error, out);
            return;
        }
        // V.250 clause 5.2.1: space characters are ignored, and commands
        // are the same in either case.
        let text: Vec<u8> = body
            .iter()
            .filter(|b| **b != b' ')
            .map(u8::to_ascii_uppercase)
            .collect();
        let mut cursor = Cursor {
            bytes: &text,
            at: 0,
        };
        // V.250 clause 5.3.1: commands run in the order given, and the
        // first one that fails ends the line with ERROR.
        while let Some(command) = cursor.next() {
            match self.command(command, &mut cursor, out) {
                Next::Continue => {}
                Next::Done => return,
                Next::Fail => {
                    self.result(ResultCode::Error, out);
                    return;
                }
            }
        }
        self.result(ResultCode::Ok, out);
    }

    fn command(&mut self, command: u8, cursor: &mut Cursor<'_>, out: &mut Vec<Output>) -> Next {
        match command {
            // V.250 clause 6.3.5: answer.
            b'A' => {
                if self.state != State::OnHook {
                    return Next::Fail;
                }
                self.answer(out);
                Next::Done
            }
            b'D' => self.dial(cursor, out),
            // The family of the carrier at 300 and 1200 bit/s: B0 is the
            // ITU's, V.21 and V.22, and B1 is Bell's, 103 and 212A. Not in
            // V.250; it is the command Hayes-compatible modems have for
            // it. It changes the carrier of +MS.
            b'B' => match parameter(cursor, 1) {
                Some(n) => {
                    self.profile.set_family(n == 1);
                    Next::Continue
                }
                None => Next::Fail,
            },
            // V.250 clause 6.2.4: command echo.
            b'E' => match parameter(cursor, 1) {
                Some(n) => {
                    self.profile.echo = n == 1;
                    Next::Continue
                }
                None => Next::Fail,
            },
            // V.250 clause 6.3.6: hook control. H1 is accepted and does
            // nothing.
            b'H' => match parameter(cursor, 1) {
                Some(0) => {
                    if self.state != State::OnHook {
                        out.push(Output::Action(Action::Hangup(HangupCause::Command)));
                        self.go_on_hook();
                    }
                    Next::Continue
                }
                Some(_) => Next::Continue,
                None => Next::Fail,
            },
            // V.250 clause 6.1.3: identification.
            b'I' => match parameter(cursor, 9) {
                Some(0) => {
                    self.info(&[PRODUCT_CODE], out);
                    Next::Continue
                }
                Some(3) => {
                    let version = format!("RustModem {}", env!("CARGO_PKG_VERSION"));
                    self.info(&[&version], out);
                    Next::Continue
                }
                Some(4) => {
                    let line = format!("{} {CAPABILITIES}", self.config.engine);
                    self.info(&[&line], out);
                    Next::Continue
                }
                // The numbers Windows asks for when it queries a modem,
                // each with something to show for it. A modem with a ROM
                // gives its checksum to I1; this one has none.
                Some(1) => {
                    self.info(&["000"], out);
                    Next::Continue
                }
                Some(5) => {
                    self.info(&[MANUFACTURER], out);
                    Next::Continue
                }
                Some(6) => {
                    self.info(&[MODEL], out);
                    Next::Continue
                }
                Some(7) => {
                    let line = format!("{MODEL} {}", env!("CARGO_PKG_VERSION"));
                    self.info(&[&line], out);
                    Next::Continue
                }
                Some(_) => Next::Continue,
                None => Next::Fail,
            },
            // V.250 clauses 6.3.13 and 6.3.14: speaker. Stored, no effect.
            b'L' => match parameter(cursor, 3) {
                Some(n) => {
                    self.profile.volume = n as u8;
                    Next::Continue
                }
                None => Next::Fail,
            },
            b'M' => match parameter(cursor, 3) {
                Some(n) => {
                    self.profile.speaker = n as u8;
                    Next::Continue
                }
                None => Next::Fail,
            },
            // V.250 clause 6.3.7: return to online data state.
            b'O' => {
                if parameter(cursor, 0).is_none() || self.state != State::OnlineCommand {
                    return Next::Fail;
                }
                self.enter_online();
                self.result(ResultCode::Connect, out);
                self.release_held(out);
                Next::Done
            }
            // V.250 clause 6.2.5: result code suppression.
            b'Q' => match parameter(cursor, 1) {
                Some(n) => {
                    self.profile.quiet = n == 1;
                    Next::Continue
                }
                None => Next::Fail,
            },
            b'S' => self.register_command(cursor, out),
            // V.250 clause 6.2.6: response format.
            b'V' => match parameter(cursor, 1) {
                Some(n) => {
                    self.profile.verbose = n == 1;
                    Next::Continue
                }
                None => Next::Fail,
            },
            // V.250 clause 6.2.7: result code selection.
            b'X' => match parameter(cursor, 4) {
                Some(n) => {
                    self.profile.x = n as u8;
                    Next::Continue
                }
                None => Next::Fail,
            },
            // V.250 clause 6.1.1: reset to the stored profile, or to the
            // factory one if nothing has been stored. Any call is cleared,
            // and the rest of the line is not processed.
            b'Z' => {
                if parameter(cursor, 0).is_none() {
                    return Next::Fail;
                }
                if self.state != State::OnHook {
                    out.push(Output::Action(Action::Hangup(HangupCause::Reset)));
                    self.go_on_hook();
                }
                self.restore_profile();
                self.result(ResultCode::Ok, out);
                Next::Done
            }
            b'&' => self.ampersand_command(cursor, out),
            b'+' => self.extended_command(cursor, out),
            // Commands of other vendors' sets. Those that start with one of
            // these characters take one more letter; by convention those
            // that start with # take a whole word.
            b'#' => {
                let name = cursor.until(|b| !b.is_ascii_alphabetic());
                if name == b"CLS" {
                    self.class_command(cursor, out)
                } else {
                    self.skip_unknown(cursor)
                }
            }
            // \N, error control, and %C, compression: the commands that
            // modems had for them before V.250, which hosts still send.
            // Each sets the +ES or the +DS that it is.
            b'\\' if cursor.peek() == Some(b'N') => {
                cursor.next();
                match parameter(cursor, 5) {
                    Some(n) => {
                        self.profile.set_reliability(n as u8);
                        Next::Continue
                    }
                    None => Next::Fail,
                }
            }
            b'%' if cursor.peek() == Some(b'C') => {
                cursor.next();
                match parameter(cursor, 3) {
                    Some(n) => {
                        self.profile.set_compression_mode(n as u8);
                        Next::Continue
                    }
                    None => Next::Fail,
                }
            }
            b'\\' | b'%' | b'*' | b'-' | b'$' | b'^' | b')' | b':' | b'@' => {
                cursor.next();
                self.skip_unknown(cursor)
            }
            _ => self.skip_unknown(cursor),
        }
    }

    /// `#CLS`, the service class command of modems older than `+FCLASS`,
    /// which Windows sends when it queries a modem. The one class is 0,
    /// data: `#CLS=?` and `#CLS?` print it, `#CLS=0` is accepted, and any
    /// other class is an error whatever the line does with commands it
    /// does not know, because this one it knows.
    fn class_command(&mut self, cursor: &mut Cursor<'_>, out: &mut Vec<Output>) -> Next {
        match cursor.next() {
            Some(b'?') => {
                self.info(&["0"], out);
                Next::Continue
            }
            Some(b'=') if cursor.peek() == Some(b'?') => {
                cursor.next();
                self.info(&["0"], out);
                Next::Continue
            }
            Some(b'=') => match cursor.number() {
                Some(0) | None => Next::Continue,
                Some(_) => Next::Fail,
            },
            _ => Next::Fail,
        }
    }

    /// Steps over what follows the name of a command this interpreter does
    /// not have: a number, then either `?` or `=` and a value.
    fn skip_unknown(&self, cursor: &mut Cursor<'_>) -> Next {
        cursor.number();
        match cursor.peek() {
            Some(b'?') => {
                cursor.next();
            }
            Some(b'=') => {
                cursor.next();
                cursor.until(|b| !(b.is_ascii_digit() || b == b','));
            }
            _ => {}
        }
        self.unknown()
    }

    /// A command this interpreter does not have: skipped or refused, as the
    /// line is configured.
    fn unknown(&self) -> Next {
        match self.config.unknown_command {
            UnknownCommand::Ok => Next::Continue,
            UnknownCommand::Error => Next::Fail,
        }
    }

    /// V.250 clause 6.3.1: dial. The dial string is the rest of the line.
    /// Digits, `*`, `#`, `+`, `,` and `W` are kept; everything else,
    /// including the `T` and `P` modifiers, is dropped. A final `;` asks to
    /// stay in command mode once the number has been dialled.
    fn dial(&mut self, cursor: &mut Cursor<'_>, out: &mut Vec<Output>) -> Next {
        if self.state != State::OnHook {
            return Next::Fail;
        }
        let rest = cursor.rest();
        let (string, stay_in_command) = match rest.strip_suffix(b";") {
            Some(string) => (string, true),
            None => (rest, false),
        };
        let number: String = string
            .iter()
            .filter(|b| b.is_ascii_digit() || matches!(**b, b'*' | b'#' | b'+' | b',' | b'W'))
            .map(|b| char::from(*b))
            .collect();
        out.push(Output::Action(Action::Dial(number)));
        if stay_in_command {
            self.start_dialled();
            self.result(ResultCode::Ok, out);
        } else {
            self.start_handshake(true);
        }
        Next::Done
    }

    /// V.250 clause 5.3.2: `S<n>=<v>` and `S<n>?`.
    fn register_command(&mut self, cursor: &mut Cursor<'_>, out: &mut Vec<Output>) -> Next {
        let Some(number) = cursor.number() else {
            return Next::Fail;
        };
        match cursor.next() {
            Some(b'?') => match self.profile.register(number) {
                Some(value) => {
                    self.info(&[&format!("{value:03}")], out);
                    Next::Continue
                }
                None => match self.config.unknown_command {
                    UnknownCommand::Ok => {
                        self.info(&["000"], out);
                        Next::Continue
                    }
                    UnknownCommand::Error => Next::Fail,
                },
            },
            Some(b'=') => {
                // A missing value is zero.
                let value = cursor.number().unwrap_or(0);
                if value > 255 {
                    return Next::Fail;
                }
                match number {
                    // S1 counts rings; a value written to it is not kept.
                    1 => Next::Continue,
                    // S3, S4 and S5 are characters of the 7-bit set.
                    3..=5 if value > 127 => Next::Fail,
                    // The registers of error control take the values
                    // that mean something, and set the +ES or +DS that
                    // each is.
                    36 if !matches!(value, 0 | 1 | 3 | 4 | 5 | 7) => Next::Fail,
                    46 if !matches!(value, 136 | 138) => Next::Fail,
                    48 if !matches!(value, 0 | 7 | 128) => Next::Fail,
                    36 | 46 | 48 => {
                        self.profile.set_register(number, value as u8);
                        self.profile.apply_register(number, value as u8);
                        Next::Continue
                    }
                    _ => {
                        if self.profile.set_register(number, value as u8) {
                            Next::Continue
                        } else {
                            self.unknown()
                        }
                    }
                }
            }
            _ => Next::Fail,
        }
    }

    fn ampersand_command(&mut self, cursor: &mut Cursor<'_>, out: &mut Vec<Output>) -> Next {
        match cursor.next() {
            // V.250 clause 6.2.8: circuit 109 (DCD) behaviour.
            Some(b'C') => match parameter(cursor, 1) {
                Some(n) => {
                    self.profile.dcd_follows_carrier = n == 1;
                    Next::Continue
                }
                None => Next::Fail,
            },
            // V.250 clause 6.2.9: circuit 108 (DTR) behaviour.
            Some(b'D') => match parameter(cursor, 3) {
                Some(n) => {
                    self.profile.dtr_action = n as u8;
                    Next::Continue
                }
                None => Next::Fail,
            },
            // V.250 clause 6.1.2: factory defaults.
            Some(b'F') => match parameter(cursor, 0) {
                Some(_) => {
                    self.factory_profile();
                    Next::Continue
                }
                None => Next::Fail,
            },
            // The communications mode, of modems before V.250: 0 direct,
            // 5 error control, 6 buffered. The synchronous modes, 1 to 4,
            // this modem has not got. It sets the +ES that it is.
            Some(b'Q') => match cursor.number().unwrap_or(0) {
                n @ (0 | 5 | 6) => {
                    self.profile.set_communications_mode(n as u8);
                    Next::Continue
                }
                _ => Next::Fail,
            },
            // Flow control: none, or RTS/CTS.
            Some(b'K') => match cursor.number().unwrap_or(0) {
                n @ (0 | 3) => {
                    self.profile.flow = n as u8;
                    Next::Continue
                }
                _ => Next::Fail,
            },
            Some(b'V') => match parameter(cursor, 0) {
                Some(_) => {
                    self.view(out);
                    Next::Continue
                }
                None => Next::Fail,
            },
            // Store the active profile. There is one stored profile, so
            // &W0 and &W1 write the same one.
            Some(b'W') => match parameter(cursor, 1) {
                Some(_) => {
                    self.store_profile(out);
                    Next::Continue
                }
                None => Next::Fail,
            },
            Some(_) => self.skip_unknown(cursor),
            None => self.unknown(),
        }
    }

    /// `&V`: the current settings and registers.
    fn view(&mut self, out: &mut Vec<Output>) {
        /// The lines that describe one profile.
        fn lines(p: &Profile) -> [String; 7] {
            let settings = format!(
                "E{} Q{} V{} X{} L{} M{} &C{} &D{} &K{}",
                u8::from(p.echo),
                u8::from(p.quiet),
                u8::from(p.verbose),
                p.x,
                p.volume,
                p.speaker,
                u8::from(p.dcd_follows_carrier),
                p.dtr_action,
                p.flow,
            );
            let registers = p
                .registers()
                .map(|(number, value)| format!("S{number:02}:{value:03}"))
                .collect::<Vec<_>>()
                .join(" ");
            [
                settings,
                registers,
                format!("+MS: {}", p.ms),
                format!("+ES: {}", p.es),
                format!("+DS: {}", p.ds),
                format!("+ICF: {}", p.icf),
                format!(
                    "+MR: {} +ER: {} +DR: {} +ILRR: {}",
                    u8::from(p.mr),
                    u8::from(p.er),
                    u8::from(p.dr),
                    u8::from(p.ilrr)
                ),
            ]
        }
        let mut text = vec!["ACTIVE PROFILE:".to_string()];
        text.extend(lines(&self.profile));
        // The stored profile is shown once there is one.
        if let Some(stored) = self.stored_profile() {
            text.push("STORED PROFILE:".to_string());
            text.extend(lines(stored));
        }
        let text: Vec<&str> = text.iter().map(String::as_str).collect();
        self.info(&text, out);
    }

    /// V.250 clause 5.4: an extended-syntax command, `+<name>` followed by
    /// `=<value>` (set), `?` (read) or `=?` (test), and ended by `;` or the
    /// end of the line. There are `+MS` (clause 6.4.1), `+ES` (6.5.1),
    /// `+DS` (6.6.1) and `+ICF` (6.2.11), which are compound, and the
    /// four that say what is reported when a call connects, `+MR`
    /// (6.4.3), `+ER` (6.5.5), `+DR` (6.6.2) and `+ILRR` (6.2.13), which
    /// are 0 or 1.
    fn extended_command(&mut self, cursor: &mut Cursor<'_>, out: &mut Vec<Output>) -> Next {
        enum Form<'a> {
            Set(&'a [u8]),
            Read,
            Test,
            Action,
        }
        let name = cursor.until(|b| matches!(b, b'=' | b'?' | b';'));
        let form = match cursor.peek() {
            Some(b'=') => {
                cursor.next();
                if cursor.peek() == Some(b'?') {
                    cursor.next();
                    Form::Test
                } else {
                    Form::Set(cursor.until(|b| b == b';'))
                }
            }
            Some(b'?') => {
                cursor.next();
                Form::Read
            }
            _ => Form::Action,
        };
        // Nothing may follow the command but its separator.
        let junk = cursor.until(|b| b == b';');
        if cursor.peek() == Some(b';') {
            cursor.next();
        }

        if let b"MR" | b"ER" | b"DR" | b"ILRR" = name {
            if !junk.is_empty() {
                return Next::Fail;
            }
            let label = String::from_utf8_lossy(name).into_owned();
            let flag = match name {
                b"MR" => &mut self.profile.mr,
                b"ER" => &mut self.profile.er,
                b"DR" => &mut self.profile.dr,
                _ => &mut self.profile.ilrr,
            };
            return match form {
                // An empty value is the default, which is 0.
                Form::Set(b"" | b"0") => {
                    *flag = false;
                    Next::Continue
                }
                Form::Set(b"1") => {
                    *flag = true;
                    Next::Continue
                }
                Form::Read => {
                    let line = format!("+{label}: {}", u8::from(*flag));
                    self.info(&[&line], out);
                    Next::Continue
                }
                Form::Test => {
                    self.info(&[&format!("+{label}: (0,1)")], out);
                    Next::Continue
                }
                Form::Set(_) | Form::Action => Next::Fail,
            };
        }
        // V.250 clauses 6.1.4 to 6.1.9: identification. Each is an action
        // that prints one line, and its test form is `OK` alone.
        let identity = match name {
            b"GMI" => Some(MANUFACTURER.to_string()),
            b"GMM" => Some(MODEL.to_string()),
            b"GMR" => Some(env!("CARGO_PKG_VERSION").to_string()),
            b"GSN" => Some(SERIAL_NUMBER.to_string()),
            b"GCAP" => Some(CAPABILITY_LIST.to_string()),
            _ => None,
        };
        if let Some(text) = identity {
            if !junk.is_empty() {
                return Next::Fail;
            }
            return match form {
                Form::Action => {
                    self.info(&[&text], out);
                    Next::Continue
                }
                Form::Test => Next::Continue,
                Form::Set(_) | Form::Read => Next::Fail,
            };
        }
        // T.31 clause 8.2: the service class. There is one, 0, data.
        if name == b"FCLASS" {
            if !junk.is_empty() {
                return Next::Fail;
            }
            return match form {
                Form::Read | Form::Test => {
                    self.info(&["0"], out);
                    Next::Continue
                }
                Form::Set(b"" | b"0") => Next::Continue,
                Form::Set(_) | Form::Action => Next::Fail,
            };
        }
        // V.250 clause 6.1.10: the country of installation, a T.35 code
        // in two hexadecimal digits. Windows sets it from its own
        // location. Kept and read back; nothing here differs by country.
        if name == b"GCI" {
            if !junk.is_empty() {
                return Next::Fail;
            }
            return match form {
                Form::Read => {
                    let line = format!("+GCI: {:02X}", self.country);
                    self.info(&[&line], out);
                    Next::Continue
                }
                Form::Test => {
                    self.info(&["+GCI: (00-FF)"], out);
                    Next::Continue
                }
                Form::Set(value) => {
                    let code = std::str::from_utf8(value)
                        .ok()
                        .filter(|text| !text.is_empty() && text.len() <= 2)
                        .and_then(|text| u8::from_str_radix(text, 16).ok());
                    match code {
                        Some(code) => {
                            self.country = code;
                            Next::Continue
                        }
                        None => Next::Fail,
                    }
                }
                Form::Action => Next::Fail,
            };
        }
        // The factory values of +ES and +DS are the line's.
        let line_factory = Profile::for_line(&self.config);
        let (stored, label, factory, range) = match name {
            b"MS" => (Stored::Ms, "+MS: ", FACTORY_MS, RANGE_MS),
            b"ES" => (Stored::Es, "+ES: ", line_factory.es.as_str(), RANGE_ES),
            b"DS" => (Stored::Ds, "+DS: ", line_factory.ds.as_str(), RANGE_DS),
            b"ICF" => (Stored::Icf, "+ICF: ", FACTORY_ICF, RANGE_ICF),
            _ => return self.unknown(),
        };
        if !junk.is_empty() {
            return Next::Fail;
        }
        match form {
            Form::Set(value) => {
                if !acceptable_parameter(value) {
                    return Next::Fail;
                }
                // The value is ASCII, checked just above. An empty value
                // returns the parameter to its default (clause 5.4.4.2).
                let value = if value.is_empty() {
                    factory.to_string()
                } else if matches!(stored, Stored::Ms) {
                    // +MS has an effect, so it is read: a carrier or a
                    // rate this modem has not got is an error, and a
                    // subparameter left out stays as it is.
                    let current = self.profile.modulation();
                    match ModulationSelection::parse(value, Some(current)) {
                        Some(selection) => selection.to_string(),
                        None => return Next::Fail,
                    }
                } else if matches!(stored, Stored::Es) {
                    // +ES and +DS have an effect: a mode this modem has
                    // not got is an error.
                    let current = self.profile.error_control_selection();
                    match ErrorControlSelection::parse(value, Some(current)) {
                        Some(selection) => selection.to_string(),
                        None => return Next::Fail,
                    }
                } else if matches!(stored, Stored::Ds) {
                    let current = self.profile.compression_selection();
                    match CompressionSelection::parse(value, Some(current)) {
                        Some(selection) => selection.to_string(),
                        None => return Next::Fail,
                    }
                } else if matches!(stored, Stored::Icf) {
                    // +ICF has an effect too: a format this modem has
                    // not got is an error (V.250 clause 6.2.11).
                    let current = CharacterFraming::parse(self.profile.icf.as_bytes(), None);
                    match CharacterFraming::parse(value, current) {
                        Some(framing) => framing.to_string(),
                        None => return Next::Fail,
                    }
                } else {
                    return Next::Fail;
                };
                *self.stored_mut(stored) = value;
                Next::Continue
            }
            Form::Read => {
                let line = format!("{label}{}", self.stored_mut(stored));
                self.info(&[&line], out);
                Next::Continue
            }
            Form::Test => {
                let line = format!("{label}{range}");
                self.info(&[&line], out);
                Next::Continue
            }
            Form::Action => Next::Fail,
        }
    }

    fn stored_mut(&mut self, stored: Stored) -> &mut String {
        match stored {
            Stored::Ms => &mut self.profile.ms,
            Stored::Es => &mut self.profile.es,
            Stored::Ds => &mut self.profile.ds,
            Stored::Icf => &mut self.profile.icf,
        }
    }
}

/// The number after a basic-syntax command: missing means zero (V.250
/// clause 5.3.1), and one above `max` is an error.
fn parameter(cursor: &mut Cursor<'_>, max: u32) -> Option<u32> {
    let value = cursor.number().unwrap_or(0);
    (value <= max).then_some(value)
}
