// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Property tests of the escape sequence and of the interpreter as a whole.
//!
//! The escape sequence is: at least the guard time (S12) of silence from the
//! host, exactly three escape characters (S2) each less than the guard time
//! after the one before, then the guard time of silence again. Time comes
//! only from `Elapsed` inputs.

use at::{
    Action, AtConfig, AtInterpreter, ConnectInfo, Input, LineEvent, Mode, Output, UnknownCommand,
};
use proptest::prelude::*;

/// An interpreter in online data mode with the given guard time register,
/// its clock at the moment of connection.
fn online(s12: u8) -> AtInterpreter {
    let mut interpreter = AtInterpreter::new(AtConfig::default());
    let mut out = Vec::new();
    interpreter.step(Input::Dtr(true), &mut out);
    let setup = format!("ATE0S12={s12}D5\r");
    interpreter.step(Input::DteBytes(setup.as_bytes()), &mut out);
    interpreter.step(
        Input::Event(LineEvent::Connected(ConnectInfo::unknown())),
        &mut out,
    );
    assert_eq!(interpreter.mode(), Mode::OnlineData);
    interpreter
}

/// Lets `total` milliseconds pass in steps of at most `step`.
fn pass(interpreter: &mut AtInterpreter, total: u32, step: u32, out: &mut Vec<Output>) {
    let mut left = total;
    while left > 0 {
        let now = left.min(step.max(1));
        interpreter.step(Input::Elapsed(now), out);
        left -= now;
    }
}

fn said_ok(out: &[Output]) -> bool {
    out.iter()
        .any(|o| matches!(o, Output::DteBytes(bytes) if bytes == b"\r\nOK\r\n"))
}

/// A time near one of the guard boundaries, or anywhere.
fn around(guard: u32) -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(guard.saturating_sub(1)),
        Just(guard),
        Just(guard + 1),
        0..guard.max(1),
        guard..guard * 3 + 2,
        Just(0),
    ]
}

#[derive(Clone, Debug)]
enum Op {
    Byte(u8),
    Wait(u32),
}

fn ops(guard: u32) -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        4 => Just(Op::Byte(b'+')),
        1 => Just(Op::Byte(b'x')),
        3 => around(guard).prop_map(Op::Wait),
    ];
    prop::collection::vec(op, 0..40)
}

/// The escape rule stated over a timeline, with no state machine: given the
/// time of every byte the host has sent (the connection counts as one) and
/// the time now, has an escape sequence completed?
fn reference_detects(bytes: &[(u32, u8)], now: u32, guard: u32) -> bool {
    let n = bytes.len();
    if n < 4 {
        return false;
    }
    let last_three_are_escapes = bytes[n - 3..].iter().all(|(_, b)| *b == b'+');
    let (before, first, second, third) = (
        bytes[n - 4].0,
        bytes[n - 3].0,
        bytes[n - 2].0,
        bytes[n - 1].0,
    );
    // "Exactly three": the byte before them, if it was an escape character,
    // must not have been part of the same run. It is part of the same run
    // unless the guard time separates it, which the first condition demands
    // anyway.
    last_three_are_escapes
        && first - before >= guard
        && second - first < guard
        && third - second < guard
        && now - third >= guard
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Three escape characters with every timing around the guard
    /// boundaries: detected exactly when all four timing conditions hold.
    #[test]
    fn three_escape_characters_and_the_four_timings(
        s12 in 1u8..=100,
        timings in (0u32..4, 0u32..4, 0u32..4, 0u32..4),
        offsets in (-2i32..=2, -2i32..=2, -2i32..=2, -2i32..=2),
        step in 1u32..2500,
    ) {
        let guard = u32::from(s12) * 20;
        // Each timing is a small offset from 0, the guard time, or twice it.
        let time = |choice: u32, offset: i32| -> u32 {
            let base = match choice { 0 => 0, 1 | 2 => guard, _ => guard * 2 };
            base.saturating_add_signed(offset)
        };
        let before = time(timings.0, offsets.0);
        let gap1 = time(timings.1, offsets.1);
        let gap2 = time(timings.2, offsets.2);
        let after = time(timings.3, offsets.3);

        let mut interpreter = online(s12);
        let mut out = Vec::new();
        pass(&mut interpreter, before, step, &mut out);
        interpreter.step(Input::DteBytes(b"+"), &mut out);
        pass(&mut interpreter, gap1, step, &mut out);
        interpreter.step(Input::DteBytes(b"+"), &mut out);
        pass(&mut interpreter, gap2, step, &mut out);
        interpreter.step(Input::DteBytes(b"+"), &mut out);
        // Nothing is decided until the closing silence has passed.
        prop_assert!(!said_ok(&out));
        pass(&mut interpreter, after, step, &mut out);

        let expected = before >= guard && gap1 < guard && gap2 < guard && after >= guard;
        prop_assert_eq!(said_ok(&out), expected);
        let mode = if expected { Mode::OnlineCommand } else { Mode::OnlineData };
        prop_assert_eq!(interpreter.mode(), mode);
        // The characters went to the line as data when they arrived.
        let sent: Vec<u8> = out.iter().filter_map(|o| match o {
            Output::LineBytes(bytes) => Some(bytes.clone()),
            _ => None,
        }).flatten().collect();
        prop_assert_eq!(sent, b"+++".to_vec());
        prop_assert!(interpreter.check_invariants().is_ok());
    }

    /// Any mixture of escape characters, other data and pauses: the
    /// interpreter detects an escape exactly when the rule, stated over the
    /// timeline, says one has completed.
    #[test]
    fn any_timeline_agrees_with_the_rule(
        (s12, script) in (1u8..=50).prop_flat_map(|s12| (Just(s12), ops(u32::from(s12) * 20))),
    ) {
        let guard = u32::from(s12) * 20;
        let mut interpreter = online(s12);
        let mut now = 0u32;
        // The connection starts the first silence.
        let mut bytes: Vec<(u32, u8)> = vec![(0, 0)];
        for op in script {
            let mut out = Vec::new();
            match op {
                Op::Byte(byte) => {
                    interpreter.step(Input::DteBytes(&[byte]), &mut out);
                    bytes.push((now, byte));
                    prop_assert!(!said_ok(&out));
                }
                Op::Wait(ms) => {
                    interpreter.step(Input::Elapsed(ms), &mut out);
                    now += ms;
                    let expected = reference_detects(&bytes, now, guard);
                    prop_assert_eq!(said_ok(&out), expected, "at {} ms, bytes {:?}", now, bytes);
                    if expected {
                        prop_assert_eq!(interpreter.mode(), Mode::OnlineCommand);
                        return Ok(());
                    }
                }
            }
            prop_assert_eq!(interpreter.mode(), Mode::OnlineData);
            prop_assert!(interpreter.check_invariants().is_ok());
        }
    }

    /// The escape characters arriving in one write, or split any way, give
    /// the same result.
    #[test]
    fn how_the_bytes_are_split_does_not_matter(split in 0usize..=3) {
        let mut interpreter = online(50);
        let mut out = Vec::new();
        pass(&mut interpreter, 1000, 1000, &mut out);
        let data = b"+++";
        interpreter.step(Input::DteBytes(&data[..split]), &mut out);
        interpreter.step(Input::DteBytes(&data[split..]), &mut out);
        pass(&mut interpreter, 1000, 20, &mut out);
        prop_assert!(said_ok(&out));
    }
}

#[test]
fn a_guard_time_of_zero_needs_no_silence() {
    let mut interpreter = online(0);
    let mut out = Vec::new();
    interpreter.step(Input::DteBytes(b"data+++"), &mut out);
    assert!(!said_ok(&out));
    interpreter.step(Input::Elapsed(0), &mut out);
    assert!(said_ok(&out));
    assert_eq!(interpreter.mode(), Mode::OnlineCommand);
}

#[test]
fn an_escape_character_above_127_disables_the_escape() {
    let mut interpreter = AtInterpreter::new(AtConfig::default());
    let mut out = Vec::new();
    interpreter.step(Input::DteBytes(b"ATE0S2=200D5\r"), &mut out);
    interpreter.step(
        Input::Event(LineEvent::Connected(ConnectInfo::unknown())),
        &mut out,
    );
    out.clear();
    for data in [&b"+++"[..], &[200, 200, 200][..]] {
        interpreter.step(Input::Elapsed(2000), &mut out);
        interpreter.step(Input::DteBytes(data), &mut out);
        interpreter.step(Input::Elapsed(2000), &mut out);
    }
    assert!(!said_ok(&out));
    assert_eq!(interpreter.mode(), Mode::OnlineData);
}

// --------------------------------------------------------------------------
// The interpreter as a whole: any interleaving of inputs leaves it in a
// valid state. The fuzz target `at_input` explores the same space further.
// --------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Step {
    Dte(Vec<u8>),
    Line(Vec<u8>),
    Elapsed(u32),
    Dtr(bool),
    Event(LineEvent),
    OperatorAnswer,
    OperatorHangup,
    OperatorReset,
}

fn command_text() -> impl Strategy<Value = Vec<u8>> {
    let piece = prop::sample::select(vec![
        "AT",
        "A/",
        "\r",
        "D5",
        "D5;",
        "A",
        "H",
        "O",
        "Z",
        "E0",
        "E1",
        "Q1",
        "Q0",
        "V0",
        "V1",
        "X0",
        "X4",
        "&F",
        "&C0",
        "&D0",
        "&D1",
        "&D2",
        "&D3",
        "&V",
        "S0=1",
        "S2=255",
        "S3=65",
        "S5=43",
        "S7=0",
        "S7=1",
        "S12=0",
        "S12=1",
        "+MS=V34",
        "+ES?",
        "+++",
        "\x08",
        " ",
        ";",
        "S",
        "=",
        "?",
        "999999999999",
        "\\N3",
        "#CID=1",
        "I4",
    ]);
    prop::collection::vec(piece, 0..8).prop_map(|pieces| pieces.concat().into_bytes())
}

fn any_step() -> impl Strategy<Value = Step> {
    let event = prop_oneof![
        Just(LineEvent::Ring),
        Just(LineEvent::RingEnd),
        (0u32..40000).prop_map(|rate| LineEvent::Connected(ConnectInfo::with_rate(rate))),
        Just(LineEvent::CarrierLost),
        Just(LineEvent::Busy),
        Just(LineEvent::NoAnswer),
        Just(LineEvent::NoDialtone),
    ];
    prop_oneof![
        6 => command_text().prop_map(Step::Dte),
        2 => prop::collection::vec(any::<u8>(), 0..24).prop_map(Step::Dte),
        2 => prop::collection::vec(any::<u8>(), 0..24).prop_map(Step::Line),
        3 => prop_oneof![0u32..50, 0u32..3000, Just(u32::MAX)].prop_map(Step::Elapsed),
        1 => any::<bool>().prop_map(Step::Dtr),
        4 => event.prop_map(Step::Event),
        1 => Just(Step::OperatorAnswer),
        1 => Just(Step::OperatorHangup),
        1 => Just(Step::OperatorReset),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 3000,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn any_interleaving_of_inputs_keeps_the_state_valid(
        refuse_unknown in any::<bool>(),
        s0 in 0u8..3,
        steps in prop::collection::vec(any_step(), 0..60),
    ) {
        let config = AtConfig {
            unknown_command: if refuse_unknown { UnknownCommand::Error } else { UnknownCommand::Ok },
            auto_answer_rings: s0,
        };
        let mut interpreter = AtInterpreter::new(config);
        prop_assert!(interpreter.check_invariants().is_ok());
        for step in steps {
            let before = interpreter.mode();
            let mut out = Vec::new();
            match &step {
                Step::Dte(bytes) => interpreter.step(Input::DteBytes(bytes), &mut out),
                Step::Line(bytes) => interpreter.step(Input::LineBytes(bytes), &mut out),
                Step::Elapsed(ms) => interpreter.step(Input::Elapsed(*ms), &mut out),
                Step::Dtr(level) => interpreter.step(Input::Dtr(*level), &mut out),
                Step::Event(event) => interpreter.step(Input::Event(event.clone()), &mut out),
                Step::OperatorAnswer => { interpreter.operator_answer(&mut out); }
                Step::OperatorHangup => { interpreter.operator_hangup(&mut out); }
                Step::OperatorReset => interpreter.operator_reset(&mut out),
            }
            if let Err(problem) = interpreter.check_invariants() {
                prop_assert!(false, "{} after {:?}", problem, step);
            }
            for output in &out {
                match output {
                    Output::DteBytes(bytes) => prop_assert!(!bytes.is_empty()),
                    Output::LineBytes(bytes) => {
                        // Data goes to the line only from online data mode,
                        // or from a write that began in a command mode and
                        // returned to data with O.
                        prop_assert!(!bytes.is_empty());
                        prop_assert!(matches!(step, Step::Dte(_)));
                        prop_assert!(matches!(before, Mode::OnlineData | Mode::OnlineCommand));
                    }
                    Output::Action(Action::Dial(number)) => {
                        prop_assert!(number.bytes().all(|b| b.is_ascii_digit() || b"*#+,W".contains(&b)));
                    }
                    Output::Action(_) | Output::Trace(_) => {}
                    Output::Signals(signals) => {
                        prop_assert_eq!(*signals, interpreter.signals());
                        prop_assert!(signals.dsr);
                    }
                }
            }
        }
    }
}
