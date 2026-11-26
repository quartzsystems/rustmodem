// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The lines, the line state machine and the null switch.
//!
//! A [`Pool`] owns the lines. Each line joins one AT interpreter, one modem
//! engine and one serial port. The pool is driven from one thread: every
//! call to [`Pool::service`] reads the serial ports, steps the
//! interpreters, carries out what they ask for, ticks the engines of lines
//! in a call, moves data and applies the signals.
//!
//! The pool reads no clock and opens no files. The caller gives it the time
//! ([`Now`]), the serial ports ([`SerialFactory`]), somewhere to send
//! events ([`EventSink`]), somewhere to keep the lines' stored profiles
//! ([`ProfileStore`]) and somewhere to append call records, so the whole
//! of it runs in a test with memory ports and a clock the test moves.

#![forbid(unsafe_code)]

mod handle;
mod line;
mod pool;
mod router;
mod stats;

use std::collections::BTreeMap;

use config::LineConfig;
use ctlproto::Event;
use engine::ModemEngine;
use msg::Message;
use serial::SerialBackend;

pub use at::Profile;
pub use handle::{Command, PoolHandle, PoolInbox, command_channel};
pub use pool::{Pool, PoolParts};
pub use router::{
    CallRouter, LineDirectory, LineId, NullSwitch, RING_OFF_MS, RING_ON_MS, RouterEvent,
    UNOBTAINABLE_DELAY_MS,
};

/// How often the engines are ticked, in milliseconds.
/// BLOCKED(phase2): ticks move from this timer to RTP packet arrival on
/// media threads.
pub const TICK_MS: u64 = 20;

/// How long the pool waits before trying again to open a serial port that
/// would not open, in milliseconds.
pub const PORT_RETRY_MS: u64 = 5000;

/// How many ended calls the pool remembers.
pub const CALL_HISTORY: usize = 1000;

/// The time, as the caller of the pool tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Now {
    /// Milliseconds on a clock that only goes forward. Timers use this.
    pub mono_ms: u64,
    /// Milliseconds since the Unix epoch. Records and log lines use this.
    pub unix_ms: u64,
}

/// Where the pool sends what happens: state changes, calls and log lines.
/// `publish` must not block; the pool calls it from its one thread.
pub trait EventSink: Send {
    /// Takes one event.
    fn publish(&mut self, event: Event);
}

/// Opens the serial port of a line.
pub trait SerialFactory: Send {
    /// Opens the port the line's configuration names.
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message>;
}

/// Keeps the lines' stored profiles: what each host's `AT&W` saved. A line
/// starts from its stored profile, so one kept where it outlasts the
/// service is how a line's settings survive a restart.
pub trait ProfileStore: Send {
    /// The stored profile of a line, if it has one.
    fn load(&mut self, line: LineId) -> Option<Profile>;

    /// Keeps a line's stored profile, in place of any it had.
    fn store(&mut self, line: LineId, profile: &Profile) -> Result<(), Message>;

    /// Forgets a line's stored profile: the line has been removed.
    fn remove(&mut self, line: LineId) -> Result<(), Message>;
}

/// Stored profiles kept in memory, for as long as the pool lives.
#[derive(Debug, Default)]
pub struct MemoryProfiles(BTreeMap<LineId, Profile>);

impl MemoryProfiles {
    /// No profiles.
    pub fn new() -> MemoryProfiles {
        MemoryProfiles::default()
    }
}

impl ProfileStore for MemoryProfiles {
    fn load(&mut self, line: LineId) -> Option<Profile> {
        self.0.get(&line).cloned()
    }

    fn store(&mut self, line: LineId, profile: &Profile) -> Result<(), Message> {
        self.0.insert(line, profile.clone());
        Ok(())
    }

    fn remove(&mut self, line: LineId) -> Result<(), Message> {
        self.0.remove(&line);
        Ok(())
    }
}

/// Makes the modem engine of a line.
pub type EngineFactory = Box<dyn FnMut(LineId) -> Box<dyn ModemEngine> + Send>;
