// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The pool's statistics and the port bindings it is told about.

mod common;

use common::{Harness, config_with_lines, summarise};
use config::LineConfig;
use ctlproto::{Event, Stats};
use pool::{TICK_MS, UNOBTAINABLE_DELAY_MS, command_channel};

const RING: &str = "\r\nRING\r\n";

#[test]
fn a_pool_that_has_done_nothing_has_empty_statistics() {
    let h = Harness::new(2);
    assert_eq!(h.pool.stats(), Stats::default());
}

#[test]
fn lines_added_before_the_loop_starts_are_logged_with_the_time_given() {
    let mut h = Harness::new(0);
    h.take_logs();
    h.pool.set_now(pool::Now {
        mono_ms: 5,
        unix_ms: common::EPOCH_MS + 12_345,
    });
    h.pool.apply_config(&config_with_lines(1));
    let logs = h.take_logs();
    assert!(!logs.is_empty());
    assert!(
        logs.iter()
            .all(|log| log.time_ms == common::EPOCH_MS + 12_345)
    );
}

#[test]
fn a_connected_call_is_counted_once_from_the_line_that_was_called() {
    let mut h = Harness::new(3);
    h.connect(1, 2);
    let stats = h.pool.stats();
    assert_eq!(stats.calls_answered, 1);
    assert_eq!(stats.calls_connected, 1);
    assert_eq!(stats.lines_in_use, 2);
    assert_eq!(stats.peak_lines_in_use, 2);
    assert_eq!(stats.connects.len(), 1);
    let group = &stats.connects[0];
    assert_eq!(
        (group.rate, group.carrier.as_deref(), group.count),
        (0, None, 1)
    );
    // Nothing has ended yet.
    assert_eq!((stats.avg_call_ms, stats.longest_call_ms), (0, 0));

    h.host(1).write(b"hello");
    h.advance(200);
    assert_eq!(h.read(2), "hello");
    h.host(2).write(b"hi");
    h.advance(200);
    assert_eq!(h.read(1), "hi");
    let stats = h.pool.stats();
    // Each data byte leaves one host and reaches the other.
    assert_eq!((stats.bytes_out, stats.bytes_in), (7, 7));

    h.advance(10_000);
    h.pool.hangup(1).unwrap();
    let stats = h.pool.stats();
    assert_eq!(stats.lines_in_use, 0);
    assert_eq!(stats.peak_lines_in_use, 2);
    assert!(stats.longest_call_ms >= 10_000, "{}", stats.longest_call_ms);
    assert_eq!(stats.avg_call_ms, stats.longest_call_ms);
    assert_eq!(stats.calls_failed, 0);

    // A second, shorter call brings the mean down and leaves the longest.
    h.read(1);
    h.read(2);
    assert_eq!(h.command(1, "ATD3"), "");
    assert_eq!(h.read(3), RING);
    h.pool.answer(3).unwrap();
    h.advance(1000);
    h.pool.hangup(3).unwrap();
    let again = h.pool.stats();
    assert_eq!(again.calls_connected, 2);
    assert_eq!(again.connects[0].count, 2);
    assert_eq!(again.longest_call_ms, stats.longest_call_ms);
    assert!(again.avg_call_ms < stats.avg_call_ms);
}

#[test]
fn calls_that_are_turned_away_or_given_up_are_counted() {
    let mut h = Harness::new(3);
    for line in 1..=3 {
        h.quiet(line);
    }

    // The caller gives up while line 2 rings.
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    h.command(1, "");
    let stats = h.pool.stats();
    assert_eq!(stats.calls_abandoned, 1);
    assert_eq!((stats.calls_answered, stats.calls_rejected), (0, 0));

    // Line 2 is busy with line 1 when line 3 calls it.
    h.read(1);
    h.read(2);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    h.pool.answer(2).unwrap();
    h.command(3, "ATD2");
    assert_eq!(h.pool.stats().calls_rejected, 1);

    // A number that is not a line: nothing is ready to take the call.
    h.command(3, "ATD77");
    h.advance(UNOBTAINABLE_DELAY_MS + TICK_MS);
    let stats = h.pool.stats();
    assert_eq!(stats.calls_rejected, 2);
    assert_eq!(stats.calls_abandoned, 1);
    assert_eq!(stats.calls_connected, 1);
    assert_eq!(stats.peak_lines_in_use, 3);
}

#[test]
fn at_command_lines_are_counted_for_the_line_and_for_the_pool() {
    let mut h = Harness::new(2);
    h.command(1, "AT");
    h.command(1, "ATE0");
    h.command(2, "ATI0");
    // An empty line is not a command line.
    h.command(2, "");
    assert_eq!(h.pool.line(1).unwrap().counters.at_commands, 2);
    assert_eq!(h.pool.line(2).unwrap().counters.at_commands, 1);
    assert_eq!(h.pool.stats().at_commands, 3);
}

#[test]
fn a_line_shows_the_binding_of_its_port_and_says_when_it_changes() {
    let mut h = Harness::new(2);
    assert_eq!(h.pool.line(1).unwrap().binding, None);

    // Port names do not distinguish case.
    h.pool.set_bindings(vec![
        (
            "p1".to_string(),
            "Standard 56000 bps Modem on COM4".to_string(),
        ),
        ("P9".to_string(), "COM9".to_string()),
    ]);
    assert_eq!(
        h.pool.line(1).unwrap().binding.as_deref(),
        Some("Standard 56000 bps Modem on COM4")
    );
    assert_eq!(h.pool.line(2).unwrap().binding, None);
    let events = h.take_events();
    assert_eq!(summarise(&events), ["line 1 idle"]);
    let Event::LineState(info) = &events[0] else {
        unreachable!();
    };
    assert!(info.binding.is_some());
    // The line did not change state, so nothing is logged.
    assert!(h.take_logs().is_empty());

    // Told the same again, the pool has nothing to say.
    h.pool.set_bindings(vec![
        (
            "P1".to_string(),
            "Standard 56000 bps Modem on COM4".to_string(),
        ),
        ("P9".to_string(), "COM9".to_string()),
    ]);
    assert!(h.take_events().is_empty());

    // A line added on a port the pool was told about has its binding from
    // the start, and a line moved to another port takes that port's.
    let mut config = config_with_lines(2);
    config.lines.push(LineConfig::new(9, "P9"));
    config.lines[0].serial.port = "P5".to_string();
    h.pool.apply_config(&config);
    assert_eq!(h.pool.line(9).unwrap().binding.as_deref(), Some("COM9"));
    assert_eq!(h.pool.line(1).unwrap().binding, None);

    h.pool.set_bindings(Vec::new());
    assert_eq!(h.pool.line(9).unwrap().binding, None);
}

#[test]
fn statistics_and_bindings_are_reached_through_the_command_channel() {
    let mut h = Harness::new(2);
    h.connect(1, 2);
    let (handle, inbox) = command_channel();
    let asking = std::thread::spawn(move || {
        handle
            .set_bindings(vec![("P2".to_string(), "COM7".to_string())])
            .unwrap();
        (handle.stats().unwrap(), handle.line(2).unwrap())
    });
    while !asking.is_finished() {
        // The handle goes away with the thread, which is not an error.
        inbox.serve(&mut h.pool);
        std::thread::yield_now();
    }
    let (stats, line) = asking.join().unwrap();
    assert_eq!(stats.calls_connected, 1);
    assert_eq!(line.binding.as_deref(), Some("COM7"));
}
