// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The Hayes-compatible AT command interpreter, a pure state machine.
//!
//! [`AtInterpreter`] does no I/O, starts no threads and reads no clock.
//! Everything that happens to a line is given to [`AtInterpreter::step`] as
//! an [`Input`], and everything the interpreter wants done comes back as
//! [`Output`]s. Time passes only through [`Input::Elapsed`].
//!
//! The command set is written from the phase 0 handoff and ITU-T V.250;
//! comments cite the clause where behaviour comes from the Recommendation.
//! `docs/AT.md` describes every command, register and result code as built.

#![forbid(unsafe_code)]

mod command;
mod interp;
mod profile;

pub use engine::ConnectInfo;
pub use interp::AtInterpreter;
pub use profile::{Carrier, CharacterFormat, CharacterFraming, ModulationSelection, Profile};

/// The V.24 circuits a modem drives toward its host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct ModemSignals {
    /// Data carrier detect (circuit 109).
    pub dcd: bool,
    /// Data set ready (circuit 107).
    pub dsr: bool,
    /// Clear to send (circuit 106).
    pub cts: bool,
    /// Ring indicator (circuit 125).
    pub ri: bool,
}

/// Something that happened on the line side of the modem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineEvent {
    /// A ring-on interval began on an incoming call.
    Ring,
    /// The ring-on interval ended, or the incoming call went away.
    RingEnd,
    /// The modems connected.
    Connected(ConnectInfo),
    /// The carrier was lost, or the far end cleared the call.
    CarrierLost,
    /// The number dialled is busy.
    Busy,
    /// The number dialled did not answer.
    NoAnswer,
    /// There is no dial tone.
    NoDialtone,
}

/// One thing given to the interpreter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input<'a> {
    /// Bytes the host wrote to the COM port.
    DteBytes(&'a [u8]),
    /// Bytes received from the far end.
    LineBytes(&'a [u8]),
    /// Milliseconds since the previous `Elapsed`.
    Elapsed(u32),
    /// The host changed DTR (circuit 108/2).
    Dtr(bool),
    /// Something happened on the line.
    Event(LineEvent),
}

/// Why the interpreter asked for a hang-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HangupCause {
    /// An `H` command.
    Command,
    /// DTR dropped with `&D2` or `&D3` in effect.
    Dtr,
    /// The host sent a character while the call was being set up.
    Abort,
    /// No connection within S7 seconds.
    Timeout,
    /// A `Z` command.
    Reset,
    /// The operator hung the line up or reset it through the control API.
    Operator,
}

/// Something the interpreter wants done on the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Go off hook and dial this string.
    Dial(String),
    /// Go off hook and answer.
    Answer,
    /// Go on hook.
    Hangup(HangupCause),
    /// Keep this as the line's stored profile: the host gave `&W`. The
    /// interpreter already has it; this is so that it outlasts the
    /// interpreter.
    StoreProfile(Profile),
}

/// What the interpreter did, for a log. Nothing depends on these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trace {
    /// A command line was executed; the text is the line without its
    /// terminator.
    Command(String),
    /// A result code was issued (or would have been, with `Q1`).
    Result(String),
}

/// One thing the interpreter produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// Bytes for the host: echo, result codes, received data.
    DteBytes(Vec<u8>),
    /// Data to send to the far end.
    LineBytes(Vec<u8>),
    /// Something to do on the line.
    Action(Action),
    /// The signals toward the host as they should now be.
    Signals(ModemSignals),
    /// A note for the log.
    Trace(Trace),
}

/// The interpreter's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Accepting commands; no call.
    Command,
    /// An incoming call is ringing; still accepting commands.
    Ringing,
    /// A call is being set up; any byte from the host aborts it.
    Handshaking,
    /// Connected; bytes from the host are data.
    OnlineData,
    /// Connected, but accepting commands.
    OnlineCommand,
}

/// The result codes (V.250 clause 5.7.1, table 1, and the handoff).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResultCode {
    /// 0: the command line was executed.
    Ok,
    /// 1: a connection was established.
    Connect,
    /// 2: an incoming call is ringing.
    Ring,
    /// 3: the connection ended or could not be made.
    NoCarrier,
    /// 4: the command line was not accepted.
    Error,
    /// 6: no dial tone.
    NoDialtone,
    /// 7: the number is busy.
    Busy,
    /// 8: no answer.
    NoAnswer,
}

impl ResultCode {
    /// The numeric form, used with `V0`.
    pub fn number(self) -> u8 {
        match self {
            ResultCode::Ok => 0,
            ResultCode::Connect => 1,
            ResultCode::Ring => 2,
            ResultCode::NoCarrier => 3,
            ResultCode::Error => 4,
            ResultCode::NoDialtone => 6,
            ResultCode::Busy => 7,
            ResultCode::NoAnswer => 8,
        }
    }

    /// The verbose form, used with `V1`.
    pub fn text(self) -> &'static str {
        match self {
            ResultCode::Ok => "OK",
            ResultCode::Connect => "CONNECT",
            ResultCode::Ring => "RING",
            ResultCode::NoCarrier => "NO CARRIER",
            ResultCode::Error => "ERROR",
            ResultCode::NoDialtone => "NO DIALTONE",
            ResultCode::Busy => "BUSY",
            ResultCode::NoAnswer => "NO ANSWER",
        }
    }
}

/// The numeric result code for a connection at `rate` bit/s when the rate
/// is reported (`X1` and above). These follow the common Hayes-compatible
/// convention; a rate with no conventional code reports plain `1`.
pub fn connect_number(rate: u32) -> u8 {
    match rate {
        1200 => 5,
        2400 => 10,
        4800 => 11,
        9600 => 12,
        14400 => 13,
        19200 => 14,
        _ => 1,
    }
}

/// What the interpreter answers to a command it does not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum UnknownCommand {
    /// Skip the command and carry on.
    #[default]
    Ok,
    /// Stop the command line with `ERROR`.
    Error,
}

/// The per-line settings the interpreter is created with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtConfig {
    /// What to answer to an unknown command.
    pub unknown_command: UnknownCommand,
    /// The factory value of S0: rings before auto-answer, 0 for off. A
    /// stored profile has its own S0, which wins.
    pub auto_answer_rings: u8,
    /// The name of the line's modem engine, which `I4` reports: `NULL`,
    /// `ECHO`.
    pub engine: &'static str,
}

impl Default for AtConfig {
    fn default() -> Self {
        AtConfig {
            unknown_command: UnknownCommand::default(),
            auto_answer_rings: 0,
            engine: "NULL",
        }
    }
}

/// The longest command line body accepted, in characters after the `AT`
/// prefix.
pub const MAX_COMMAND_LINE: usize = 255;

/// How long after the last ring S1 returns to zero, in milliseconds.
pub const RING_COUNT_RESET_MS: u32 = 8000;

/// The most received data held while in online command mode, in bytes.
/// Data beyond this is dropped until the host returns to data mode.
pub const MAX_HELD_DATA: usize = 65536;
