// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Calls between lines through the null switch, end to end: memory ports on
//! every line and a clock the test moves.

mod common;

use common::{EPOCH_MS, Harness, config_with_lines, noise, summarise};
use ctlproto::{Direction, EndReason, Event, LineState};
use engine::NullConfig;

const RING: &str = "\r\nRING\r\n";
const CONNECT: &str = "\r\nCONNECT\r\n";
const NO_CARRIER: &str = "\r\nNO CARRIER\r\n";
const OK: &str = "\r\nOK\r\n";

#[test]
fn a_line_answers_at_once_its_host_opens_the_port() {
    let mut h = Harness::with_config(config_with_lines(1), NullConfig::default());
    assert_eq!(h.state(1), LineState::NotReady);
    h.host(1).open();
    h.advance(20);
    assert_eq!(h.state(1), LineState::Idle);
    assert_eq!(h.command(1, "AT"), "AT\r\r\nOK\r\n");
    // DSR and CTS are on, DCD and RI off.
    let signals = h.host(1).signals();
    assert!(signals.dsr && signals.cts && !signals.dcd && !signals.ri);
}

/// Exit criterion 2 of phase 0: a complete call between two lines.
#[test]
fn a_complete_call_with_a_megabyte_each_way() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);

    // ATD2 on line 1, RING on line 2.
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    assert_eq!(h.state(1), LineState::Handshaking);
    assert_eq!(h.state(2), LineState::Ringing);

    // ATA on line 2, CONNECT on both.
    assert_eq!(h.command(2, "ATA"), CONNECT);
    assert_eq!(h.read(1), CONNECT);
    assert_eq!(h.state(1), LineState::Online);
    assert_eq!(h.state(2), LineState::Online);
    assert!(h.host(1).signals().dcd);
    assert!(h.host(2).signals().dcd);

    // 1 MB each way, intact.
    const SIZE: usize = 1024 * 1024;
    let from_1 = noise(1, SIZE);
    let from_2 = noise(2, SIZE);
    h.host(1).write(&from_1);
    h.host(2).write(&from_2);
    let (mut at_1, mut at_2) = (Vec::new(), Vec::new());
    let mut waited_ms = 0;
    while (at_1.len() < SIZE || at_2.len() < SIZE) && waited_ms < 400_000 {
        h.advance(1000);
        waited_ms += 1000;
        at_1.extend(h.host(1).read());
        at_2.extend(h.host(2).read());
    }
    assert_eq!(at_1.len(), SIZE, "line 1 received everything");
    assert_eq!(at_2.len(), SIZE, "line 2 received everything");
    assert!(at_1 == from_2, "line 1 received what line 2 sent");
    assert!(at_2 == from_1, "line 2 received what line 1 sent");
    assert_eq!(h.state(1), LineState::Online);

    // +++ on line 1, OK. The characters themselves went through as data.
    h.advance(1000);
    h.host(1).write(b"+++");
    h.advance(1100);
    assert_eq!(h.read(1), OK);
    assert_eq!(h.read(2), "+++");
    assert_eq!(h.state(1), LineState::Online);

    // ATH on line 1, NO CARRIER on the far side.
    assert_eq!(h.command(1, "ATH"), OK);
    assert_eq!(h.read(2), NO_CARRIER);
    assert_eq!(h.state(1), LineState::Idle);
    assert_eq!(h.state(2), LineState::Idle);
    assert!(!h.host(1).signals().dcd);
    assert!(!h.host(2).signals().dcd);

    // Both ends have a record, with the bytes counted.
    let calls = h.pool.calls(None);
    assert_eq!(calls.len(), 2);
    let (out, inc) = (&calls[0], &calls[1]);
    assert_eq!(
        (out.line, out.direction, out.number.as_str()),
        (1, Direction::Outgoing, "2")
    );
    assert_eq!(
        (inc.line, inc.direction, inc.number.as_str()),
        (2, Direction::Incoming, "1")
    );
    assert_eq!(out.reason, EndReason::Hangup);
    assert_eq!(inc.reason, EndReason::Remote);
    assert_eq!((out.bytes_tx, out.bytes_rx), (SIZE as u64 + 3, SIZE as u64));
    assert_eq!((inc.bytes_tx, inc.bytes_rx), (SIZE as u64, SIZE as u64 + 3));
    assert!(out.connected_ms.is_some() && inc.connected_ms.is_some());
    assert_eq!(out.connect.as_ref().unwrap().rate, 0);
    assert!(out.started_ms >= EPOCH_MS && out.ended_ms > out.started_ms);
    assert_eq!(h.logged_calls(), calls);

    let line = h.pool.line(1).unwrap();
    assert_eq!(line.counters.calls_out, 1);
    assert_eq!(line.counters.connects, 1);
    assert_eq!(line.counters.bytes_tx, SIZE as u64 + 3);
    assert!(line.call.is_none());
}

#[test]
fn an_idle_target_rings_two_seconds_on_and_four_off() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(1, "ATD2"), "");
    // The command took two ticks; the first ring began at the dial.
    assert_eq!(h.read(2), RING);
    assert!(h.host(2).signals().ri);
    h.advance(1900);
    assert!(h.host(2).signals().ri, "still in the ring-on interval");
    h.advance(200);
    assert!(!h.host(2).signals().ri, "the ring-on interval is over");
    h.advance(3700);
    assert_eq!(h.read(2), "", "one RING for each ring-on interval");
    assert!(!h.host(2).signals().ri);
    h.advance(300);
    assert_eq!(h.read(2), RING);
    assert!(h.host(2).signals().ri);
    h.advance(6000);
    assert_eq!(h.read(2), RING);
    assert_eq!(h.read(1), "", "the caller hears nothing while it rings");
    assert_eq!(h.state(1), LineState::Handshaking);
    assert_eq!(h.state(2), LineState::Ringing);
}

#[test]
fn a_target_that_is_ringing_handshaking_or_online_is_busy() {
    let mut h = Harness::new(4);
    for id in 1..=4 {
        h.quiet(id);
    }
    const BUSY: &str = "\r\nBUSY\r\n";

    // Ringing.
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.state(2), LineState::Ringing);
    assert_eq!(h.command(3, "ATD2"), BUSY);
    assert_eq!(h.state(3), LineState::Idle);
    // The caller of that call is handshaking.
    assert_eq!(h.state(1), LineState::Handshaking);
    assert_eq!(h.command(3, "ATD1"), BUSY);

    // Online.
    assert_eq!(h.read(2), RING);
    assert_eq!(h.command(2, "ATA"), CONNECT);
    assert_eq!(h.read(1), CONNECT);
    assert_eq!(h.command(3, "ATD2"), BUSY);
    assert_eq!(h.command(3, "ATD1"), BUSY);

    // A line that dials itself is handshaking by then.
    assert_eq!(h.command(4, "ATD4"), BUSY);

    // None of this disturbed the call.
    assert_eq!(h.read(1), "");
    assert_eq!(h.read(2), "");
    assert_eq!(h.state(1), LineState::Online);

    let busy = h
        .pool
        .calls(None)
        .iter()
        .filter(|c| c.reason == EndReason::Busy)
        .count();
    assert_eq!(busy, 5);
}

#[test]
fn a_target_that_is_not_a_line_disabled_or_not_ready_is_no_carrier_after_two_seconds() {
    let mut config = config_with_lines(4);
    config.lines[2].enabled = false; // line 3
    let mut h = Harness::with_config(config, NullConfig::default());
    h.host(1).open();
    h.host(2).open();
    // Line 4's host never opens its port: not ready.
    h.advance(20);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.state(3), LineState::Disabled);
    assert_eq!(h.state(4), LineState::NotReady);

    for number in ["9", "3", "4", "", "1*2", "99999999"] {
        assert_eq!(h.command(1, &format!("ATD{number}")), "");
        assert_eq!(h.state(1), LineState::Handshaking);
        h.advance(1900);
        assert_eq!(h.read(1), "", "nothing before two seconds for {number:?}");
        h.advance(200);
        assert_eq!(h.read(1), NO_CARRIER, "for {number:?}");
        assert_eq!(h.state(1), LineState::Idle);
    }
    let calls = h.pool.calls(None);
    assert_eq!(calls.len(), 6);
    assert!(calls.iter().all(|c| c.reason == EndReason::Failed));
    // The lines that could not be called were never rung.
    assert_eq!(h.pool.line(4).unwrap().counters.calls_in, 0);
}

#[test]
fn a_target_that_auto_answers_connects_both() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(2, "ATS0=2"), OK);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    h.advance(5900);
    assert_eq!(h.read(1), "");
    assert_eq!(h.state(2), LineState::Ringing);
    h.advance(200);
    // The second ring: RING, then the modem answers by itself.
    assert_eq!(h.read(2), format!("{RING}{CONNECT}"));
    assert_eq!(h.read(1), CONNECT);
    assert_eq!(h.state(1), LineState::Online);
    assert_eq!(h.state(2), LineState::Online);
    assert!(!h.host(2).signals().ri);
}

#[test]
fn the_configured_auto_answer_applies_from_the_start() {
    let mut config = config_with_lines(2);
    config.lines[1].auto_answer_rings = 1;
    let mut h = Harness::with_config(config, NullConfig::default());
    h.host(1).open();
    h.host(2).open();
    h.advance(20);
    h.quiet(1);
    assert_eq!(h.command(1, "ATD2"), CONNECT);
    assert_eq!(h.read(2), format!("{RING}{CONNECT}"));
}

#[test]
fn the_callers_s7_expiring_is_no_answer_and_the_target_stops_ringing() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(1, "ATS7=7"), OK);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    h.advance(6100);
    assert_eq!(h.read(2), RING);
    assert!(h.host(2).signals().ri);
    assert_eq!(h.read(1), "");
    h.advance(1000);
    assert_eq!(h.read(1), "\r\nNO ANSWER\r\n");
    assert_eq!(h.state(1), LineState::Idle);
    assert_eq!(h.state(2), LineState::Idle);
    assert!(!h.host(2).signals().ri, "RI goes off with the call");
    h.advance(20_000);
    assert_eq!(h.read(2), "", "no more rings");

    let calls = h.pool.calls(None);
    assert_eq!(calls.len(), 2);
    assert_eq!((calls[0].line, calls[0].reason), (1, EndReason::NoAnswer));
    assert_eq!((calls[1].line, calls[1].reason), (2, EndReason::Abandoned));
    assert!(calls.iter().all(|c| c.connected_ms.is_none()));
}

#[test]
fn either_side_hanging_up_is_no_carrier_for_the_other() {
    // The caller hangs up.
    let mut h = Harness::new(2);
    h.connect(1, 2);
    h.advance(1000);
    h.host(1).write(b"+++");
    h.advance(1100);
    assert_eq!(h.read(1), OK);
    assert_eq!(h.read(2), "+++");
    assert_eq!(h.command(1, "ATH"), OK);
    assert_eq!(h.read(2), NO_CARRIER);
    assert_eq!((h.state(1), h.state(2)), (LineState::Idle, LineState::Idle));

    // The line that was called hangs up.
    let mut h = Harness::new(2);
    h.connect(1, 2);
    h.advance(1000);
    h.host(2).write(b"+++");
    h.advance(1100);
    assert_eq!(h.read(2), OK);
    assert_eq!(h.read(1), "+++");
    assert_eq!(h.command(2, "ATH"), OK);
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!((h.state(1), h.state(2)), (LineState::Idle, LineState::Idle));
    let calls = h.pool.calls(None);
    assert_eq!((calls[0].line, calls[0].reason), (2, EndReason::Hangup));
    assert_eq!((calls[1].line, calls[1].reason), (1, EndReason::Remote));

    // Both lines can call again afterwards.
    assert_eq!(h.command(2, "ATD1"), "");
    assert_eq!(h.read(1), RING);
    assert_eq!(h.command(1, "ATA"), CONNECT);
    assert_eq!(h.read(2), CONNECT);
}

#[test]
fn either_side_dropping_dtr_is_no_carrier_for_the_other() {
    for dropper in [1, 2] {
        let other = 3 - dropper;
        let mut h = Harness::new(2);
        h.connect(1, 2);
        h.host(dropper).set_dtr(false);
        h.advance(40);
        // &D2, the default: the modem hangs up and says OK.
        assert_eq!(h.read(dropper), OK);
        assert_eq!(h.read(other), NO_CARRIER);
        assert_eq!(h.state(dropper), LineState::NotReady);
        assert_eq!(h.state(other), LineState::Idle);
        let calls = h.pool.calls(None);
        assert_eq!((calls[0].line, calls[0].reason), (dropper, EndReason::Dtr));
        assert_eq!((calls[1].line, calls[1].reason), (other, EndReason::Remote));

        h.host(dropper).set_dtr(true);
        h.advance(20);
        assert_eq!(h.state(dropper), LineState::Idle);
    }
}

#[test]
fn dtr_handling_follows_the_and_d_setting() {
    // &D0: the drop is ignored and the call goes on.
    let mut h = Harness::new(2);
    h.connect(1, 2);
    h.advance(1000);
    h.host(1).write(b"+++");
    h.advance(1100);
    assert_eq!(h.read(1), OK);
    assert_eq!(h.read(2), "+++");
    assert_eq!(h.command(1, "AT&D0"), OK);
    assert_eq!(h.command(1, "ATO"), CONNECT);
    h.host(1).set_dtr(false);
    h.advance(100);
    assert_eq!(h.read(2), "");
    assert_eq!(h.state(1), LineState::Online);
    h.host(1).write(b"still here");
    h.advance(100);
    assert_eq!(h.read(2), "still here");
}

#[test]
fn the_host_closing_its_port_ends_the_call() {
    let mut h = Harness::new(2);
    h.connect(1, 2);
    h.host(2).close();
    h.advance(40);
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.state(2), LineState::NotReady);
    assert_eq!(h.read(2), "", "nothing is left for a host that has gone");
}

#[test]
fn a_target_that_stops_being_ready_while_ringing_fails_the_call() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.state(2), LineState::Ringing);
    h.host(2).set_dtr(false);
    h.advance(40);
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.state(1), LineState::Idle);
    assert_eq!(h.state(2), LineState::NotReady);
    assert!(!h.host(2).signals().ri);
    let calls = h.pool.calls(None);
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|c| c.reason == EndReason::Failed));
}

#[test]
fn a_key_during_call_setup_aborts_it_and_the_target_stops_ringing() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    h.host(1).write(b"x");
    h.advance(40);
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.state(2), LineState::Idle);
    let calls = h.pool.calls(None);
    assert_eq!((calls[0].line, calls[0].reason), (1, EndReason::Aborted));
    assert_eq!((calls[1].line, calls[1].reason), (2, EndReason::Abandoned));
}

#[test]
fn a_line_that_dials_while_it_is_ringing_turns_the_call_down() {
    let mut h = Harness::new(3);
    for id in 1..=3 {
        h.quiet(id);
    }
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    assert_eq!(h.command(2, "ATD3"), "");
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.read(3), RING);
    assert_eq!(h.state(2), LineState::Handshaking);
    assert_eq!(h.state(3), LineState::Ringing);
}

#[test]
fn answering_when_nothing_is_calling_is_no_carrier() {
    let mut h = Harness::new(1);
    h.quiet(1);
    assert_eq!(h.command(1, "ATA"), NO_CARRIER);
    assert_eq!(h.state(1), LineState::Idle);
    assert!(h.pool.calls(None).is_empty());
}

#[test]
fn the_engines_connect_delay_holds_back_connect() {
    let null = NullConfig {
        connect_delay_ticks: 50,
        ..NullConfig::default()
    };
    let mut h = Harness::with_config(config_with_lines(2), null);
    h.host(1).open();
    h.host(2).open();
    h.advance(20);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    assert_eq!(h.command(2, "ATA"), "");
    assert_eq!(h.state(1), LineState::Handshaking);
    assert_eq!(h.state(2), LineState::Handshaking);
    h.advance(900);
    assert_eq!(h.read(1), "");
    h.advance(200);
    assert_eq!(h.read(1), CONNECT);
    assert_eq!(h.read(2), CONNECT);
    h.host(1).write(b"through");
    h.advance(100);
    assert_eq!(h.read(2), "through");
}

#[test]
fn events_are_published_as_things_happen() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(summarise(&h.take_events()), Vec::<String>::new());

    h.command(1, "ATD2");
    assert_eq!(
        summarise(&h.take_events()),
        [
            "call 1 started on 1 outgoing 2",
            "call 2 started on 2 incoming 1",
            "line 1 handshaking",
            "line 2 ringing",
        ]
    );

    h.command(2, "ATA");
    let events = h.take_events();
    assert_eq!(
        summarise(&events),
        [
            "call 1 connected on 1",
            "call 2 connected on 2",
            "line 1 online",
            "line 2 online",
        ]
    );
    let Event::CallConnected(connected) = &events[0] else {
        panic!("expected a connected event");
    };
    assert_eq!(connected.call.connect.as_ref().unwrap().rate, 0);
    assert!(connected.call.connected_ms.is_some());
    // The line's state event carries the call in progress.
    let Event::LineState(info) = &events[2] else {
        panic!("expected a state event");
    };
    assert_eq!(info.call.as_ref().unwrap().id, 1);

    h.host(1).write(b"12345");
    h.advance(1100);
    h.host(1).write(b"+++");
    h.advance(1100);
    h.command(1, "ATH");
    let events = h.take_events();
    assert_eq!(
        summarise(&events),
        [
            "call 1 ended on 1 hangup",
            "call 2 ended on 2 remote",
            "line 1 idle",
            "line 2 idle",
        ]
    );
    let Event::CallEnded(record) = &events[0] else {
        panic!("expected an ended event");
    };
    assert_eq!((record.bytes_tx, record.bytes_rx), (8, 0));
    let Event::LineState(info) = &events[2] else {
        panic!("expected a state event");
    };
    assert_eq!(info.counters.bytes_tx, 8);
    assert!(info.call.is_none());
}

#[test]
fn at_traffic_is_logged_at_debug_and_changes_at_info() {
    let mut h = Harness::new(2);
    h.take_logs();
    h.command(1, "ATE0S0=3");
    h.command(1, "ATD2");
    let logs = h.take_logs();
    let lines: Vec<String> = logs
        .iter()
        .map(|log| format!("{} [{}] {}", log.level.as_str(), log.source, log.text))
        .collect();
    assert_eq!(
        lines,
        [
            "debug [Line01] > ATE0S0=3",
            "debug [Line01] < OK",
            "debug [Line01] > ATD2",
            "info [Line01] RMD1006: Line 1 call started: outgoing, number 2.",
            "info [Line02] RMD1006: Line 2 call started: incoming, number 1.",
            "debug [Line02] < RING",
            "info [Line01] RMD1005: Line 1 state is now handshaking.",
            "info [Line02] RMD1005: Line 2 state is now ringing.",
        ]
    );
    assert!(logs.iter().all(|log| log.time_ms >= EPOCH_MS));
}

#[test]
fn only_the_last_thousand_calls_are_remembered_and_all_are_logged() {
    let mut h = Harness::new(1);
    h.quiet(1);
    for _ in 0..1005 {
        // A line that dials itself is busy: the shortest call there is.
        assert_eq!(h.command(1, "ATD1"), "\r\nBUSY\r\n");
    }
    let calls = h.pool.calls(None);
    assert_eq!(calls.len(), 1000);
    assert_eq!(calls.first().unwrap().id, 6);
    assert_eq!(calls.last().unwrap().id, 1005);
    let last = h.pool.calls(Some(3));
    assert_eq!(
        last.iter().map(|c| c.id).collect::<Vec<_>>(),
        [1003, 1004, 1005]
    );
    assert_eq!(h.pool.calls(Some(5000)).len(), 1000);
    assert!(h.pool.calls(Some(0)).is_empty());

    let logged = h.logged_calls();
    assert_eq!(logged.len(), 1005);
    assert_eq!(logged[0].id, 1);
    assert_eq!(logged[1004], calls[999]);
    assert_eq!(h.pool.line(1).unwrap().counters.calls_out, 1005);
}

#[test]
fn a_host_that_reads_slowly_loses_nothing() {
    let mut h = Harness::new(1);
    h.host(1).set_capacity(Some(7));
    h.host(1).write(b"ATE0I4\r");
    let mut seen = Vec::new();
    for _ in 0..40 {
        h.advance(20);
        seen.extend(h.host(1).read_some(5));
    }
    assert_eq!(
        String::from_utf8(seen).unwrap(),
        "ATE0I4\r\r\nNULL MODEM,V.250,+MS,+ES,+DS\r\n\r\nOK\r\n"
    );
}
