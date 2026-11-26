// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The engine a line's `engine` setting gives it.

mod common;

use common::{Harness, SharedProfiles, config_with_lines, summarise};
use config::EngineKind;
use ctlproto::LineState;
use pool::{InlineRunner, NullSwitch, TICK_MS, engine_for};

/// Lines 1 and 2, the first with the echo engine, on the null switch.
fn echo_and_null() -> Harness {
    let mut config = config_with_lines(2);
    config.lines[0].engine = EngineKind::Echo;
    let mut harness = Harness::with_parts(
        config,
        SharedProfiles::default(),
        Box::new(NullSwitch::new()),
        Box::new(InlineRunner::new()),
        Box::new(engine_for),
    );
    for id in 1..=2 {
        harness.host(id).open();
    }
    harness.advance(TICK_MS);
    harness.take_events();
    harness
}

#[test]
fn i4_names_the_engine_of_the_line() {
    let mut h = echo_and_null();
    h.quiet(1);
    h.quiet(2);
    assert_eq!(
        h.command(1, "ATI4"),
        "\r\nECHO MODEM,V.250,+MS,+ES,+DS\r\n\r\nOK\r\n"
    );
    assert_eq!(
        h.command(2, "ATI4"),
        "\r\nNULL MODEM,V.250,+MS,+ES,+DS\r\n\r\nOK\r\n"
    );
}

#[test]
fn a_call_to_a_line_with_the_echo_engine_comes_back() {
    let mut h = echo_and_null();
    h.quiet(1);
    h.quiet(2);
    // The null line calls the echo line.
    assert_eq!(h.command(2, "ATD1"), "");
    assert_eq!(h.read(1), "\r\nRING\r\n");
    assert_eq!(h.command(1, "ATA"), "\r\nCONNECT\r\n");
    assert_eq!(h.read(2), "\r\nCONNECT\r\n");
    assert_eq!(h.state(1), LineState::Online);

    // The echo engine reports its own carrier.
    let line = h.pool.line(1).unwrap();
    let connect = line.call.unwrap().connect.unwrap();
    assert_eq!(
        (connect.rate, connect.carrier.as_deref()),
        (0, Some("ECHO"))
    );

    // What the null line sends comes back to it, octet for octet: the
    // echo engine returns the very stream the null engine framed.
    let data = common::noise(7, 20_000);
    h.host(2).write(&data);
    let mut back = Vec::new();
    for _ in 0..400 {
        h.advance(TICK_MS);
        back.extend(h.host(2).read());
        if back.len() >= data.len() {
            break;
        }
    }
    assert!(
        back == data,
        "{} of {} bytes came back",
        back.len(),
        data.len()
    );

    // What the echo line's host types goes nowhere, and it is sent
    // nothing.
    h.host(1).write(b"is anybody there");
    h.advance(10 * TICK_MS);
    assert_eq!(h.read(1), "");
    assert_eq!(h.read(2), "");
}

#[test]
fn changing_a_lines_engine_ends_its_call_and_takes_effect() {
    let mut h = echo_and_null();
    h.connect(2, 1);
    h.take_events();

    let mut config = h.config.clone();
    config.lines[0].engine = EngineKind::Null;
    h.pool.apply_config(&config);
    h.advance(2 * TICK_MS);
    assert_eq!(h.read(1), "\r\nNO CARRIER\r\n");
    assert_eq!(h.read(2), "\r\nNO CARRIER\r\n");
    let events = summarise(&h.take_events());
    assert!(
        events.contains(&"call 2 ended on 1 operator".to_string()),
        "{events:?}"
    );
    assert_eq!(
        h.command(1, "ATI4"),
        "\r\nNULL MODEM,V.250,+MS,+ES,+DS\r\n\r\nOK\r\n"
    );

    // Two null engines now: data goes from one host to the other.
    assert_eq!(h.command(2, "ATD1"), "");
    assert_eq!(h.read(1), "\r\nRING\r\n");
    assert_eq!(h.command(1, "ATA"), "\r\nCONNECT\r\n");
    assert_eq!(h.read(2), "\r\nCONNECT\r\n");
    h.host(2).write(b"across");
    h.advance(10 * TICK_MS);
    assert_eq!(h.read(1), "across");

    // A change that leaves the engine alone leaves the call alone.
    config.lines[0].name = Some("renamed".to_string());
    h.pool.apply_config(&config);
    h.advance(2 * TICK_MS);
    assert_eq!(h.state(1), LineState::Online);
    assert_eq!(h.read(2), "");
}
