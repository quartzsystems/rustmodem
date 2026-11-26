// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The `frozen` script: a fixed set of states and counters on a clock that
//! stands still, for screenshots.
//!
//! The states are not painted on. Before the service starts serving, the
//! script plays the hosts through a short history on a clock it moves
//! itself, single-threaded, so that every run arrives at the same lines,
//! calls, counters and log, to the millisecond. Then the clock stops.
//!
//! With eight lines, which is what the screenshots use, it ends with:
//!
//! | Line | State |
//! |---|---|
//! | 1 | online, having dialled line 2 |
//! | 2 | online, called by line 1 |
//! | 3 | handshaking: it has dialled line 4 |
//! | 4 | ringing |
//! | 5 | idle |
//! | 6 | not ready: no program has its port open |
//! | 7 | disabled |
//! | 8 | a port that will not open |
//!
//! and with a history behind it of calls that connected at each rate of
//! the demo engine, one that failed to connect, one the caller abandoned,
//! one to a busy line and one to a number that is not a line. With fewer
//! lines it does what the lines there are allow.

use std::sync::atomic::{AtomicU64, Ordering};

use config::{Config, HuntGroup, Route, Strategy, Target};
use pool::{Now, Pool, TICK_MS, UNOBTAINABLE_DELAY_MS};
use serial::MemoryHost;

use crate::hosts::{Hosts, port_of_line};

/// When the frozen service says it started: 2027-01-15 08:00:00 UTC.
pub const START_MS: u64 = 1_800_000_000_000;

/// The line that is left with its port unopened by any program.
pub const NOT_READY_LINE: u16 = 6;

/// The line that is disabled in the configuration.
pub const DISABLED_LINE: u16 = 7;

/// The line whose port will not open.
pub const BROKEN_LINE: u16 = 8;

/// Gives the frozen configuration its hunt groups and routes, so that
/// the window has some to show: the first four lines are a group, the
/// lines after them another, a number goes to line 1 alone and the rest
/// of its exchange to the first group. The service has no SIP and no
/// call comes by them. With fewer lines there is what the lines allow.
pub fn arrange(config: &mut Config) {
    let ids: Vec<u16> = config.lines.iter().map(|line| line.id).collect();
    let (first, rest) = ids.split_at(ids.len().min(4));
    if first.is_empty() {
        return;
    }
    config.hunt_groups.push(HuntGroup {
        name: "dialup".to_string(),
        pilot: "5551000".to_string(),
        lines: first.to_vec(),
        strategy: Strategy::RoundRobin,
    });
    if !rest.is_empty() {
        config.hunt_groups.push(HuntGroup {
            name: "night".to_string(),
            pilot: "5552000".to_string(),
            lines: rest.to_vec(),
            strategy: Strategy::FirstIdle,
        });
    }
    config.routes.push(Route {
        called: "5551234".to_string(),
        target: Target::Line(first[0]),
    });
    config.routes.push(Route {
        called: "555*".to_string(),
        target: Target::Group("dialup".to_string()),
    });
}

/// How long a host waits for its modem to act on a command line.
const COMMAND_MS: u64 = 2 * TICK_MS;

/// The pool, the hosts and the clock while the history is played.
struct Stage<'a> {
    pool: &'a mut Pool,
    hosts: &'a Hosts,
    lines: u16,
    mono_ms: u64,
    /// The clock the control server reads.
    clock: &'a AtomicU64,
}

impl Stage<'_> {
    fn now(&self) -> Now {
        Now {
            mono_ms: self.mono_ms,
            unix_ms: START_MS + self.mono_ms,
        }
    }

    /// Moves the clock on, a tick at a time, serving the pool at each.
    fn advance(&mut self, ms: u64) {
        let end = self.mono_ms + ms;
        while self.mono_ms < end {
            self.mono_ms = (self.mono_ms + TICK_MS).min(end);
            self.pool.service(self.now());
            // Nobody reads what the modems say.
            if self.mono_ms % 1000 < TICK_MS {
                self.drain();
            }
        }
        self.drain();
        self.clock.store(START_MS + self.mono_ms, Ordering::Relaxed);
    }

    fn drain(&self) {
        for line in 1..=self.lines {
            if let Some(host) = self.hosts.get(&port_of_line(line)) {
                host.read();
            }
        }
    }

    fn host(&self, line: u16) -> Option<MemoryHost> {
        self.hosts.get(&port_of_line(line))
    }

    /// The host of a line types a command line.
    fn command(&mut self, line: u16, text: &str) {
        if let Some(host) = self.host(line) {
            host.write(format!("{text}\r").as_bytes());
        }
        self.advance(COMMAND_MS);
    }

    /// The host of a line sends data on its call, a little at a time as a
    /// program would.
    fn send(&mut self, line: u16, bytes: usize) {
        let Some(host) = self.host(line) else {
            return;
        };
        let mut sent = 0;
        while sent < bytes {
            let chunk = (bytes - sent).min(1000);
            // Printable, and no three plus signs in a row.
            let data: Vec<u8> = (sent..sent + chunk)
                .map(|i| b'a' + (i % 26) as u8)
                .collect();
            host.write(&data);
            sent += chunk;
            self.advance(250);
        }
    }

    /// `from` dials `to`, which answers after the first ring. Both wait
    /// for their modems to connect.
    fn connect(&mut self, from: u16, to: u16) {
        self.command(from, &format!("ATD{to}"));
        self.advance(2_500);
        self.command(to, "ATA");
        self.advance(2_000);
    }

    /// The host of a line ends its call by dropping DTR, as a program that
    /// closes the port does, and then opens the port again.
    fn hang_up(&mut self, line: u16) {
        if let Some(host) = self.host(line) {
            host.set_dtr(false);
            self.advance(COMMAND_MS);
            host.set_dtr(true);
            self.advance(COMMAND_MS);
        }
    }

    /// A whole call: connect, talk for `talk_ms` with `bytes` each way,
    /// and hang up.
    fn call(&mut self, from: u16, to: u16, talk_ms: u64, bytes: usize) {
        self.connect(from, to);
        let before = self.mono_ms;
        self.send(from, bytes);
        self.send(to, bytes / 3);
        let talked = self.mono_ms - before;
        self.advance(talk_ms.saturating_sub(talked));
        self.hang_up(from);
    }
}

/// Plays the history on a pool that has its lines and has done nothing
/// yet, and returns the time at which the clock stops. `clock` is left at
/// that time.
pub fn play(pool: &mut Pool, hosts: &Hosts, lines: u16, clock: &AtomicU64) -> Now {
    let mut stage = Stage {
        pool,
        hosts,
        lines,
        mono_ms: 0,
        clock,
    };
    // The pool opens the ports; programs open the ones that will be in
    // service and turn echo off.
    stage.advance(TICK_MS);
    for line in 1..=lines {
        if line != NOT_READY_LINE
            && let Some(host) = stage.host(line)
        {
            host.open();
        }
    }
    stage.advance(TICK_MS);
    for line in 1..=lines {
        if line != NOT_READY_LINE {
            stage.command(line, "ATE0");
        }
    }

    // A history, so that the statistics have something in every row.
    if lines >= 2 {
        stage.call(1, 2, 95_000, 3_000);
    }
    if lines >= 4 {
        stage.call(3, 4, 40_000, 1_200);
    }
    if lines >= 5 {
        stage.call(5, 2, 12 * 60_000 + 7_000, 48_000);
        stage.call(4, 5, 61_000, 900);
        stage.call(2, 1, 5 * 60_000 + 42_000, 21_000);
    }
    if lines >= 2 {
        // Rung and never answered: the caller gives up.
        stage.command(1, "ATD2");
        stage.advance(9_000);
        stage.command(1, "");
        stage.advance(500);
    }
    if lines >= 4 {
        // Answered, and the caller gone before the modems connected.
        stage.command(3, "ATD4");
        stage.advance(2_500);
        stage.command(4, "ATA");
        stage.advance(400);
        stage.command(3, "");
        stage.advance(500);
    }

    // What is left standing when the clock stops.
    if lines >= 2 {
        stage.connect(1, 2);
        stage.send(1, 5_200);
        stage.send(2, 18_400);
    }
    if lines >= 3 {
        // Line 2 is in a call: busy. And a number that is not a line.
        stage.command(3, "ATD2");
        stage.advance(500);
        stage.command(3, "ATD5550123");
        stage.advance(UNOBTAINABLE_DELAY_MS + 500);
    }
    if lines >= 2 {
        stage.advance(175_000);
    }
    if lines >= 4 {
        stage.command(3, "ATD4");
        stage.advance(6_500);
    }
    stage.now()
}
