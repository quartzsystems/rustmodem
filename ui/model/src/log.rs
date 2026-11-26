// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The event log: the last lines the service logged.
//!
//! The buffer is filled from `log.tail` when the manager connects and
//! from `log` events after that. The two overlap for a moment: a line
//! logged while the tail is being asked for can arrive both in the tail
//! and as an event. A line that arrives as an event and is already at the
//! end of the buffer is taken once.

use std::collections::VecDeque;

use ctlproto::{LogEvent, LogLevel};

use crate::format::TimeZone;

/// How many lines the buffer keeps.
pub const LOG_LIMIT: usize = 5000;

/// How far back from the end an arriving line is looked for, to tell a
/// line that came twice.
const OVERLAP: usize = 64;

/// How much a log line matters, which is what its colour says: each
/// level has a colour of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogClass {
    /// Something failed.
    Error,
    /// Something is wrong but the service went on.
    Warning,
    /// Something happened.
    Info,
    /// Detail: every AT command line and result code.
    Debug,
}

impl LogClass {
    /// The class of a line: its level, whatever its source.
    pub fn of(event: &LogEvent) -> LogClass {
        match event.level {
            LogLevel::Error => LogClass::Error,
            LogLevel::Warning => LogClass::Warning,
            LogLevel::Info => LogClass::Info,
            LogLevel::Debug => LogClass::Debug,
        }
    }
}

/// One line of the event log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    /// The line as the service sent it.
    pub event: LogEvent,
    /// What it is about.
    pub class: LogClass,
}

impl LogLine {
    /// The line as the pane shows it: the time of day, the level, the
    /// source and the text.
    pub fn text(&self, zone: &dyn TimeZone) -> String {
        let level = match self.event.level {
            LogLevel::Error => "ERROR",
            LogLevel::Warning => "WARN ",
            LogLevel::Info => "INFO ",
            LogLevel::Debug => "DEBUG",
        };
        format!(
            "{} {level} [{}] {}",
            zone.wall(self.event.time_ms).time(),
            self.event.source,
            self.event.text
        )
    }
}

/// The last lines the service logged.
#[derive(Clone, Debug, Default)]
pub struct LogBuffer {
    lines: VecDeque<LogLine>,
    /// How many lines have been added since the buffer was last started
    /// over. The pane remembers how many it has shown and asks for the
    /// ones since.
    added: u64,
    /// How many times the buffer has been started over. A pane that has
    /// shown lines of an earlier generation empties itself first.
    generation: u64,
    /// The time of the last line of the tail the buffer was filled from.
    /// An event no later than this may be a line the tail already had.
    tail_end_ms: Option<u64>,
}

impl LogBuffer {
    /// An empty buffer.
    pub fn new() -> LogBuffer {
        LogBuffer::default()
    }

    /// How many lines the buffer holds.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether the buffer holds no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The lines, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = &LogLine> {
        self.lines.iter()
    }

    /// How many lines have been added since the buffer was started over.
    pub fn added(&self) -> u64 {
        self.added
    }

    /// How many times the buffer has been started over: emptied, or
    /// filled from a tail. What a pane showed of an earlier generation is
    /// no longer what the buffer holds.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The lines added since `seen` had been, oldest first; if more were
    /// added than the buffer keeps, the ones it still has.
    pub fn since(&self, seen: u64) -> impl Iterator<Item = &LogLine> {
        let new = self.added.saturating_sub(seen).min(self.lines.len() as u64) as usize;
        self.lines.iter().skip(self.lines.len() - new)
    }

    fn add(&mut self, event: LogEvent) {
        if self.lines.len() == LOG_LIMIT {
            self.lines.pop_front();
        }
        let class = LogClass::of(&event);
        self.lines.push_back(LogLine { event, class });
        self.added += 1;
    }

    /// Empties the buffer: the manager is connecting again, and the pane
    /// starts over.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.tail_end_ms = None;
        self.added = 0;
        self.generation += 1;
    }

    /// Takes in the answer to `log.tail`. Lines that arrived as events
    /// while the tail was on its way are kept after it, unless the tail
    /// has them.
    pub fn set_tail(&mut self, tail: Vec<LogEvent>) {
        let early: Vec<LogEvent> = self.lines.drain(..).map(|line| line.event).collect();
        self.added = 0;
        self.generation += 1;
        self.tail_end_ms = tail.last().map(|line| line.time_ms);
        for event in tail {
            self.add(event);
        }
        for event in early {
            self.push(event);
        }
    }

    /// Takes in a `log` event. Returns whether the line was added: a line
    /// the tail already had is not.
    pub fn push(&mut self, event: LogEvent) -> bool {
        if self.tail_end_ms.is_some_and(|end| event.time_ms <= end)
            && self
                .lines
                .iter()
                .rev()
                .take(OVERLAP)
                .any(|line| line.event == event)
        {
            return false;
        }
        self.add(event);
        true
    }

    /// The whole log as text, a line to a line, for exporting.
    pub fn text(&self, zone: &dyn TimeZone) -> String {
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(&zone.wall(line.event.time_ms).date());
            out.push(' ');
            out.push_str(&line.text(zone));
            out.push_str("\r\n");
        }
        out
    }
}
