// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `at_input`: arbitrary interleavings of inputs to the AT
//! interpreter never panic and never leave it in an invalid state.
//!
//! The input is read as a small program. The first byte chooses the line's
//! settings; after that each operation is one opcode byte and its operands.

#![no_main]

use at::{
    Action, AtConfig, AtInterpreter, ConnectInfo, Input, LineEvent, Mode, Output, UnknownCommand,
};
use libfuzzer_sys::fuzz_target;

/// Pieces of command lines, so the fuzzer does not have to discover the
/// command set a byte at a time.
const PIECES: &[&[u8]] = &[
    b"AT", b"A/", b"\r", b"D", b"D5;", b"A", b"H", b"O", b"Z", b"E0", b"E1", b"Q1", b"Q0", b"V0",
    b"V1", b"X", b"&F", b"&C", b"&D", b"&K3", b"&V", b"&W", b"S0=", b"S2=", b"S3=", b"S4=",
    b"S5=", b"S7=", b"S12=", b"S1?", b"+MS=", b"+ES?", b"+DS=?", b"+++", b"\x08", b" ", b";",
    b"=", b"?", b"0", b"1", b"2", b"3", b"255", b"256", b"I", b"L", b"M", b"\\N", b"#CID=",
];

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (first, rest) = self.data.split_first()?;
        self.data = rest;
        Some(*first)
    }

    fn chunk(&mut self, most: usize) -> &'a [u8] {
        let wanted = usize::from(self.byte().unwrap_or(0)) % (most + 1);
        let take = wanted.min(self.data.len());
        let (chunk, rest) = self.data.split_at(take);
        self.data = rest;
        chunk
    }
}

fuzz_target!(|data: &[u8]| {
    let mut reader = Reader { data };
    let Some(settings) = reader.byte() else {
        return;
    };
    let config = AtConfig {
        unknown_command: if settings & 1 == 0 {
            UnknownCommand::Ok
        } else {
            UnknownCommand::Error
        },
        auto_answer_rings: (settings >> 1) & 3,
    };
    let mut interpreter = AtInterpreter::new(config);
    let mut out = Vec::new();

    while let Some(opcode) = reader.byte() {
        out.clear();
        let before = interpreter.mode();
        let mut from_host = false;
        match opcode % 12 {
            0..=2 => {
                from_host = true;
                let bytes = reader.chunk(32);
                interpreter.step(Input::DteBytes(bytes), &mut out);
            }
            3 | 4 => {
                from_host = true;
                let piece = PIECES[usize::from(reader.byte().unwrap_or(0)) % PIECES.len()];
                interpreter.step(Input::DteBytes(piece), &mut out);
            }
            5 => {
                let bytes = reader.chunk(32);
                interpreter.step(Input::LineBytes(bytes), &mut out);
            }
            6 => {
                let ms = u32::from(reader.byte().unwrap_or(0));
                interpreter.step(Input::Elapsed(ms), &mut out);
            }
            7 => {
                let ms = u32::from(reader.byte().unwrap_or(0)) * 20;
                interpreter.step(Input::Elapsed(ms), &mut out);
            }
            8 => {
                let high = u32::from(reader.byte().unwrap_or(0));
                let ms = if high == 255 { u32::MAX } else { high * 1000 };
                interpreter.step(Input::Elapsed(ms), &mut out);
            }
            9 => {
                let level = reader.byte().unwrap_or(0) & 1 == 1;
                interpreter.step(Input::Dtr(level), &mut out);
            }
            10 => {
                let event = match reader.byte().unwrap_or(0) % 7 {
                    0 => LineEvent::Ring,
                    1 => LineEvent::RingEnd,
                    2 => {
                        let rate = u32::from(reader.byte().unwrap_or(0)) * 300;
                        LineEvent::Connected(ConnectInfo::with_rate(rate))
                    }
                    3 => LineEvent::CarrierLost,
                    4 => LineEvent::Busy,
                    5 => LineEvent::NoAnswer,
                    _ => LineEvent::NoDialtone,
                };
                interpreter.step(Input::Event(event), &mut out);
            }
            _ => match reader.byte().unwrap_or(0) % 4 {
                0 => {
                    interpreter.operator_answer(&mut out);
                }
                1 => {
                    interpreter.operator_hangup(&mut out);
                }
                2 => interpreter.operator_reset(&mut out),
                _ => {
                    let hold = reader.byte().unwrap_or(0) & 1 == 1;
                    interpreter.set_flow_hold(hold, &mut out);
                }
            },
        }

        if let Err(problem) = interpreter.check_invariants() {
            panic!("invalid state: {problem}");
        }
        for output in &out {
            match output {
                Output::DteBytes(bytes) => assert!(!bytes.is_empty()),
                Output::LineBytes(bytes) => {
                    assert!(!bytes.is_empty());
                    assert!(from_host, "data sent to the line without the host sending any");
                    assert!(
                        matches!(before, Mode::OnlineData | Mode::OnlineCommand),
                        "data sent to the line from {before:?}"
                    );
                }
                Output::Action(Action::Dial(number)) => {
                    assert!(
                        number
                            .bytes()
                            .all(|b| b.is_ascii_digit() || b"*#+,W".contains(&b)),
                        "dial string {number:?} has a character that should have been dropped"
                    );
                }
                Output::Action(_) | Output::Trace(_) => {}
                Output::Signals(signals) => {
                    assert_eq!(*signals, interpreter.signals());
                    assert!(signals.dsr);
                }
            }
        }
    }
});
