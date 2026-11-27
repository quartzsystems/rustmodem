// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the operator does to the pool, and what the pool does when a port
//! goes wrong: answer, hang up, reset, configuration changes, port
//! failures, shutdown and the command channel.

mod common;

use std::io::ErrorKind;

use common::{Harness, config_with_lines, summarise};
use config::LineConfig;
use ctlproto::{EndReason, LineState};
use engine::NullConfig;
use pool::{PORT_RETRY_MS, command_channel};

const RING: &str = "\r\nRING\r\n";
const CONNECT: &str = "\r\nCONNECT\r\n";
const NO_CARRIER: &str = "\r\nNO CARRIER\r\n";

#[test]
fn lines_are_listed_in_id_order_with_their_configuration() {
    let mut config = config_with_lines(3);
    config.lines.reverse();
    config.lines[0].name = Some("Three".to_string());
    config.lines[0].auto_answer_rings = 2;
    config.lines[0].unknown_command = config::UnknownCommand::Error;
    let h = Harness::with_config(config, NullConfig::default());
    let lines = h.pool.lines();
    assert_eq!(lines.iter().map(|l| l.id).collect::<Vec<_>>(), [1, 2, 3]);
    let three = &lines[2];
    assert_eq!(three.name.as_deref(), Some("Three"));
    assert_eq!(three.backend, "com0com");
    assert_eq!(three.port, "P3");
    assert!(three.enabled);
    assert_eq!(three.state, LineState::NotReady);
    assert_eq!(three.auto_answer_rings, 2);
    assert_eq!(three.unknown_command, "error");
    assert!(three.call.is_none() && three.error.is_none());
    assert_eq!(h.pool.line(3).unwrap(), *three);
    assert_eq!(h.pool.line(4).unwrap_err().id, "RMD1001");
    // Memory ports have nothing to wait on.
    assert!(h.pool.wait_handles().is_empty());
}

#[test]
fn the_operator_answers_a_ringing_line() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.pool.answer(2).unwrap_err().id, "RMD1003");
    assert_eq!(h.pool.answer(9).unwrap_err().id, "RMD1001");

    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    // The caller is not ringing; it cannot be answered.
    let refused = h.pool.answer(1).unwrap_err();
    assert_eq!(refused.id, "RMD1003");
    assert!(refused.text.contains("handshaking"), "{}", refused.text);

    h.pool.answer(2).unwrap();
    // The answer takes effect at once, without waiting for a tick.
    assert_eq!(h.read(2), CONNECT);
    assert_eq!(h.read(1), CONNECT);
    assert_eq!(h.state(2), LineState::Online);
    h.host(1).write(b"data");
    h.advance(100);
    assert_eq!(h.read(2), "data");
}

#[test]
fn the_operator_hangs_up_a_call_or_turns_one_away() {
    let mut h = Harness::new(2);
    assert_eq!(h.pool.hangup(1).unwrap_err().id, "RMD1004");
    assert_eq!(h.pool.hangup(9).unwrap_err().id, "RMD1001");

    // A connected call: both hosts are told.
    h.connect(1, 2);
    h.pool.hangup(1).unwrap();
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.read(2), NO_CARRIER);
    assert_eq!((h.state(1), h.state(2)), (LineState::Idle, LineState::Idle));
    let calls = h.pool.calls(None);
    assert_eq!((calls[0].line, calls[0].reason), (1, EndReason::Operator));
    assert_eq!((calls[1].line, calls[1].reason), (2, EndReason::Remote));

    // A call still being set up, hung up at the caller.
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    h.pool.hangup(1).unwrap();
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.state(2), LineState::Idle);

    // A call ringing, turned away at the line it is offered to.
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    h.pool.hangup(2).unwrap();
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.read(2), "");
    assert!(!h.host(2).signals().ri);
    assert_eq!((h.state(1), h.state(2)), (LineState::Idle, LineState::Idle));
    let calls = h.pool.calls(Some(2));
    assert_eq!((calls[0].line, calls[0].reason), (2, EndReason::Operator));
    assert_eq!((calls[1].line, calls[1].reason), (1, EndReason::Failed));
}

#[test]
fn the_operator_resets_a_line() {
    let mut h = Harness::new(2);
    h.connect(1, 2);
    assert_eq!(h.pool.reset(9).unwrap_err().id, "RMD1001");
    h.take_logs();
    h.pool.reset(1).unwrap();
    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(h.read(2), NO_CARRIER);
    assert_eq!(h.state(1), LineState::Idle);
    // The settings are back at their defaults: echo is on again.
    assert_eq!(h.command(1, "AT"), "AT\r\r\nOK\r\n");
    assert!(
        h.take_logs()
            .iter()
            .any(|log| log.text == "RMD1009: Line 1 was reset.")
    );
    // Resetting an idle line is allowed and says nothing to the host.
    h.pool.reset(2).unwrap();
    assert_eq!(h.read(2), "");
}

#[test]
fn a_disabled_line_cannot_be_answered() {
    let mut config = config_with_lines(1);
    config.lines[0].enabled = false;
    let mut h = Harness::with_config(config, NullConfig::default());
    assert_eq!(h.state(1), LineState::Disabled);
    assert_eq!(h.pool.answer(1).unwrap_err().id, "RMD1002");
    // A disabled line holds no port.
    assert_eq!(h.ports.lock().unwrap().opens, 0);
}

#[test]
fn applying_a_configuration_adds_removes_and_changes_lines() {
    let mut h = Harness::new(2);
    h.connect(1, 2);
    let old_host_2 = h.host(2);
    h.take_events();

    // Remove line 2, which is in a call, and add line 5.
    let mut config = h.config.clone();
    config.lines.retain(|line| line.id != 2);
    config.lines.push(LineConfig::new(5, "P5"));
    h.pool.apply_config(&config);

    assert_eq!(h.read(1), NO_CARRIER);
    assert_eq!(old_host_2.read(), NO_CARRIER.as_bytes());
    assert!(!old_host_2.backend_alive(), "the port was released");
    assert!(!old_host_2.signals().dsr, "every signal went off first");
    assert_eq!(h.pool.line(2).unwrap_err().id, "RMD1001");
    assert_eq!(
        h.pool.lines().iter().map(|l| l.id).collect::<Vec<_>>(),
        [1, 5]
    );
    assert_eq!(h.state(1), LineState::Idle);
    assert_eq!(h.state(5), LineState::NotReady);
    assert_eq!(
        summarise(&h.take_events()),
        [
            "call 2 ended on 2 operator",
            "call 1 ended on 1 remote",
            "line 1 idle",
            "line 5 not_ready",
        ]
    );

    // The new line's port opens on the next turn, and it works.
    h.advance(20);
    h.host(5).open();
    h.advance(20);
    assert_eq!(h.state(5), LineState::Idle);
    h.quiet(5);
    assert_eq!(h.command(1, "ATD5"), "");
    assert_eq!(h.read(5), RING);

    // The same configuration again changes nothing.
    h.take_events();
    h.take_logs();
    h.pool.apply_config(&config);
    assert!(h.take_events().is_empty());
    assert!(h.take_logs().is_empty());
    assert_eq!(h.state(5), LineState::Ringing);
}

#[test]
fn changing_a_lines_port_moves_it() {
    let mut h = Harness::new(1);
    let old_host = h.host(1);
    let mut config = h.config.clone();
    config.lines[0].serial.port = "Q1".to_string();
    h.pool.apply_config(&config);
    assert!(!old_host.backend_alive());
    h.advance(20);
    let new_host = h.host_of_port("Q1");
    assert_eq!(h.pool.line(1).unwrap().port, "Q1");
    assert_eq!(h.state(1), LineState::NotReady);
    new_host.open();
    h.advance(20);
    assert_eq!(h.state(1), LineState::Idle);
    assert_eq!(h.command(1, "AT"), "AT\r\r\nOK\r\n");
}

#[test]
fn disabling_a_line_ends_its_call_and_enabling_it_brings_it_back() {
    let mut h = Harness::new(2);
    h.connect(1, 2);
    let old_host = h.host(1);

    let mut config = h.config.clone();
    config.lines[0].enabled = false;
    h.pool.apply_config(&config);
    assert_eq!(h.state(1), LineState::Disabled);
    assert_eq!(h.read(2), NO_CARRIER);
    assert!(!old_host.backend_alive());
    assert_eq!(h.pool.calls(None)[0].reason, EndReason::Operator);

    // A disabled line cannot be called.
    assert_eq!(h.command(2, "ATD1"), "");
    h.advance(2100);
    assert_eq!(h.read(2), NO_CARRIER);

    config.lines[0].enabled = true;
    h.pool.apply_config(&config);
    h.advance(20);
    assert_eq!(h.state(1), LineState::NotReady);
    h.host(1).open();
    h.advance(20);
    assert_eq!(h.state(1), LineState::Idle);
}

#[test]
fn changing_only_the_at_settings_keeps_the_port_and_the_call() {
    let mut h = Harness::new(2);
    h.connect(1, 2);
    let host = h.host(1);
    let opens = h.ports.lock().unwrap().opens;

    let mut config = h.config.clone();
    config.lines[0].name = Some("Renamed".to_string());
    config.lines[0].unknown_command = config::UnknownCommand::Error;
    config.lines[0].auto_answer_rings = 4;
    h.pool.apply_config(&config);

    assert!(host.backend_alive());
    assert_eq!(h.ports.lock().unwrap().opens, opens);
    assert_eq!(h.state(1), LineState::Online);
    assert_eq!(h.pool.line(1).unwrap().name.as_deref(), Some("Renamed"));

    // The new settings are in force: unknown commands are refused, and a
    // reset takes S0 from the new configuration.
    h.advance(1000);
    host.write(b"+++");
    h.advance(1100);
    assert_eq!(h.read(1), "\r\nOK\r\n");
    assert_eq!(h.command(1, "ATY0"), "\r\nERROR\r\n");
    assert_eq!(h.command(1, "AT&FE0S0?"), "\r\n004\r\n\r\nOK\r\n");
}

#[test]
fn a_port_that_will_not_open_is_reported_once_and_retried() {
    let mut h = Harness::new(1);
    h.ports.lock().unwrap().refuse.insert("BAD".to_string());
    let mut config = h.config.clone();
    config.lines.push(LineConfig::new(2, "BAD"));
    h.take_logs();
    h.pool.apply_config(&config);
    h.advance(2 * PORT_RETRY_MS + 1000);

    let line = h.pool.line(2).unwrap();
    assert_eq!(line.state, LineState::NotReady);
    let error = line.error.expect("the reason the port is closed");
    assert_eq!(error.msgid, "RMD3001");
    assert!(error.text.contains("BAD"));
    let reports = h
        .take_logs()
        .iter()
        .filter(|log| log.text.starts_with("RMD3001"))
        .count();
    assert_eq!(reports, 1, "reported once, not at every retry");

    // The port appears; the next retry opens it.
    h.ports.lock().unwrap().refuse.clear();
    h.advance(PORT_RETRY_MS);
    assert!(h.pool.line(2).unwrap().error.is_none());
    h.host(2).open();
    h.advance(20);
    assert_eq!(h.state(2), LineState::Idle);
    assert!(
        h.take_logs()
            .iter()
            .any(|log| log.text == "RMD3008: Serial port BAD opened.")
    );
}

#[test]
fn a_port_that_fails_in_a_call_ends_the_call_and_is_reopened() {
    let mut h = Harness::new(2);
    h.connect(1, 2);
    let failed_host = h.host(1);
    h.take_logs();
    failed_host.inject_failure(Some(ErrorKind::BrokenPipe));
    h.advance(40);

    assert_eq!(h.read(2), NO_CARRIER);
    assert_eq!(h.state(2), LineState::Idle);
    let line = h.pool.line(1).unwrap();
    assert_eq!(line.state, LineState::NotReady);
    assert_eq!(line.error.unwrap().msgid, "RMD3005");
    assert!(!failed_host.backend_alive(), "the failed port was closed");
    assert!(
        h.take_logs()
            .iter()
            .any(|log| log.level == ctlproto::LogLevel::Error && log.text.starts_with("RMD3005"))
    );
    let calls = h.pool.calls(None);
    assert_eq!((calls[0].line, calls[0].reason), (1, EndReason::Failed));
    assert_eq!((calls[1].line, calls[1].reason), (2, EndReason::Remote));

    // After the retry time the port is opened afresh.
    h.advance(PORT_RETRY_MS);
    assert!(h.pool.line(1).unwrap().error.is_none());
    h.host(1).open();
    h.advance(20);
    assert_eq!(h.state(1), LineState::Idle);
    assert_eq!(
        h.command(1, "AT"),
        "\r\nOK\r\n",
        "the modem kept its settings"
    );
}

#[test]
fn shutdown_hangs_up_every_call_and_releases_every_port() {
    let mut h = Harness::new(3);
    h.connect(1, 2);
    let hosts = [h.host(1), h.host(2), h.host(3)];
    let now = h.now();
    h.pool.shutdown(now);

    assert_eq!(hosts[0].read(), NO_CARRIER.as_bytes());
    assert_eq!(hosts[1].read(), NO_CARRIER.as_bytes());
    assert_eq!(hosts[2].read(), b"");
    for host in &hosts {
        assert!(!host.backend_alive());
        assert_eq!(host.signals(), serial::ModemSignals::default());
    }
    let calls = h.pool.calls(None);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].reason, EndReason::Shutdown);
    assert_eq!(h.logged_calls().len(), 2);
}

#[test]
fn another_thread_reaches_the_pool_through_its_handle() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(1, "ATD2"), "");
    let (handle, inbox) = command_channel();

    let asker = std::thread::spawn(move || {
        let lines = handle.lines().unwrap();
        let line = handle.line(2).unwrap();
        let missing = handle.line(7).unwrap_err();
        handle.answer(2).unwrap();
        let refused = handle.answer(2).unwrap_err();
        handle.hangup(1).unwrap();
        handle.reset(2).unwrap();
        let calls = handle.calls(Some(1)).unwrap();
        handle.apply_config(config_with_lines(1)).unwrap();
        let after = handle.lines().unwrap();
        (lines, line, missing, refused, calls, after)
    });

    // The pool's thread: serve commands between turns until the other
    // thread has finished and dropped its handle.
    while inbox.serve(&mut h.pool) {
        h.advance(20);
        std::thread::yield_now();
    }
    let (lines, line, missing, refused, calls, after) = asker.join().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(line.state, LineState::Ringing);
    assert_eq!(missing.id, "RMD1001");
    assert_eq!(refused.id, "RMD1003");
    assert_eq!(calls.len(), 1);
    assert_eq!(after.len(), 1);
}

#[test]
fn a_handle_whose_pool_has_gone_says_the_service_is_stopping() {
    let (handle, inbox) = command_channel();
    drop(inbox);
    assert_eq!(handle.lines().unwrap_err().id, "RMD2010");
    assert_eq!(handle.answer(1).unwrap_err().id, "RMD2010");
}
