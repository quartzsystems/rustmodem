// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A pool on memory ports with a clock the test moves.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::{Arc, Mutex};

use config::{Config, LineConfig};
use ctlproto::{CallRecord, Event, LineState, LogEvent};
use engine::{NullConfig, NullEngine};
use msg::{Message, message};
use pool::{
    CallRouter, EngineFactory, EventSink, InlineRunner, LineId, MediaRunner, Now, NullSwitch, Pool,
    PoolParts, Profile, ProfileStore, SerialFactory, TICK_MS,
};
use serial::{MemoryHost, SerialBackend, memory_pair};

/// The wall clock at the start of every test, in Unix milliseconds.
pub const EPOCH_MS: u64 = 1_800_000_000_000;

/// What the factory and the test share: the host end of every port that
/// has been opened, and the ports that refuse to open.
#[derive(Default)]
pub struct Ports {
    pub hosts: BTreeMap<String, MemoryHost>,
    pub refuse: BTreeSet<String>,
    pub opens: usize,
}

struct Factory(Arc<Mutex<Ports>>);

impl SerialFactory for Factory {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        let mut ports = self.0.lock().unwrap();
        let port = &line.serial.port;
        if ports.refuse.contains(port) {
            return Err(message("RMD3001", &[port, "refused by the test"]));
        }
        ports.opens += 1;
        let (backend, host) = memory_pair();
        ports.hosts.insert(port.clone(), host);
        Ok(Box::new(backend))
    }
}

struct Sink(Arc<Mutex<Vec<Event>>>);

impl EventSink for Sink {
    fn publish(&mut self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

/// Stored profiles the test can read back, keep from one pool to the next,
/// and make fail.
#[derive(Clone, Default)]
pub struct SharedProfiles {
    pub stored: Arc<Mutex<BTreeMap<LineId, Profile>>>,
    pub fail: Arc<Mutex<bool>>,
}

impl SharedProfiles {
    fn check(&self) -> Result<(), Message> {
        if *self.fail.lock().unwrap() {
            Err(message(
                "RMD0025",
                &["profiles.toml", "refused by the test"],
            ))
        } else {
            Ok(())
        }
    }
}

impl ProfileStore for SharedProfiles {
    fn load(&mut self, line: LineId) -> Option<Profile> {
        self.stored.lock().unwrap().get(&line).cloned()
    }

    fn store(&mut self, line: LineId, profile: &Profile) -> Result<(), Message> {
        self.check()?;
        self.stored.lock().unwrap().insert(line, profile.clone());
        Ok(())
    }

    fn remove(&mut self, line: LineId) -> Result<(), Message> {
        self.check()?;
        self.stored.lock().unwrap().remove(&line);
        Ok(())
    }
}

/// A call log the test can read back.
#[derive(Clone, Default)]
pub struct SharedLog(pub Arc<Mutex<Vec<u8>>>);

impl Write for SharedLog {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Harness {
    pub pool: Pool,
    pub ports: Arc<Mutex<Ports>>,
    pub call_log: SharedLog,
    pub profiles: SharedProfiles,
    events: Arc<Mutex<Vec<Event>>>,
    pub config: Config,
    pub mono_ms: u64,
}

/// A configuration with lines 1 to `count` on ports `P1` to `P<count>`.
pub fn config_with_lines(count: u16) -> Config {
    Config {
        lines: (1..=count)
            .map(|id| LineConfig {
                engine: config::EngineKind::Null,
                ..LineConfig::new(id, &format!("P{id}"))
            })
            .collect(),
        ..Config::default()
    }
}

impl Harness {
    /// A pool with lines 1 to `count`, each port opened by its host, so
    /// that every line is idle.
    pub fn new(count: u16) -> Harness {
        let mut harness = Harness::with_config(config_with_lines(count), NullConfig::default());
        for id in 1..=count {
            harness.host(id).open();
        }
        harness.advance(TICK_MS);
        harness.take_events();
        harness
    }

    /// A pool with the given configuration. No host has opened its port.
    pub fn with_config(config: Config, null: NullConfig) -> Harness {
        Harness::with_profiles(config, null, SharedProfiles::default())
    }

    /// A pool with the given configuration and stored profiles. No host
    /// has opened its port.
    pub fn with_profiles(config: Config, null: NullConfig, profiles: SharedProfiles) -> Harness {
        Harness::with_parts(
            config,
            profiles,
            Box::new(NullSwitch::new()),
            Box::new(InlineRunner::new()),
            Box::new(move |_, _| Box::new(NullEngine::new(null))),
        )
    }

    /// A pool with the given configuration, router, media runner and
    /// engines. No host has opened its port.
    pub fn with_parts(
        config: Config,
        profiles: SharedProfiles,
        router: Box<dyn CallRouter>,
        media: Box<dyn MediaRunner>,
        engines: EngineFactory,
    ) -> Harness {
        let ports = Arc::new(Mutex::new(Ports::default()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let call_log = SharedLog::default();
        let pool = Pool::new(PoolParts {
            router,
            media,
            serial: Box::new(Factory(Arc::clone(&ports))),
            engines,
            sink: Box::new(Sink(Arc::clone(&events))),
            profiles: Box::new(profiles.clone()),
            call_log: Some(Box::new(call_log.clone())),
        });
        let mut harness = Harness {
            pool,
            ports,
            call_log,
            profiles,
            events,
            config,
            mono_ms: 0,
        };
        let config = harness.config.clone();
        harness.pool.apply_config(&config);
        harness.service();
        harness
    }

    pub fn now(&self) -> Now {
        Now {
            mono_ms: self.mono_ms,
            unix_ms: EPOCH_MS + self.mono_ms,
        }
    }

    /// One turn of the pool's loop without the clock moving.
    pub fn service(&mut self) {
        let now = self.now();
        self.pool.service(now);
    }

    /// Moves the clock on, a tick at a time, serving the pool at each.
    pub fn advance(&mut self, ms: u64) {
        let end = self.mono_ms + ms;
        while self.mono_ms < end {
            self.mono_ms = (self.mono_ms + TICK_MS).min(end);
            self.service();
        }
    }

    /// The host end of a line's port.
    pub fn host(&self, id: u16) -> MemoryHost {
        let port = self.pool.line(id).unwrap_or_else(|m| panic!("{m}")).port;
        self.host_of_port(&port)
    }

    pub fn host_of_port(&self, port: &str) -> MemoryHost {
        self.ports
            .lock()
            .unwrap()
            .hosts
            .get(port)
            .unwrap_or_else(|| panic!("port {port} has not been opened"))
            .clone()
    }

    /// Everything the line's host has been sent, as text.
    pub fn read(&self, id: u16) -> String {
        String::from_utf8_lossy(&self.host(id).read()).into_owned()
    }

    /// The host types a command line and two ticks pass. Returns what the
    /// host was sent in reply.
    pub fn command(&mut self, id: u16, line: &str) -> String {
        self.host(id).write(format!("{line}\r").as_bytes());
        self.advance(2 * TICK_MS);
        self.read(id)
    }

    /// Turns echo off on a line, so that replies are only result codes.
    pub fn quiet(&mut self, id: u16) {
        assert_eq!(self.command(id, "ATE0"), "ATE0\r\r\nOK\r\n");
    }

    pub fn state(&self, id: u16) -> LineState {
        self.pool.line(id).unwrap_or_else(|m| panic!("{m}")).state
    }

    /// Takes every event published since the last call, log lines left
    /// out.
    pub fn take_events(&mut self) -> Vec<Event> {
        let mut events = self.events.lock().unwrap();
        events
            .drain(..)
            .filter(|event| !matches!(event, Event::Log(_)))
            .collect()
    }

    /// Takes every event published since the last call, log lines only.
    pub fn take_logs(&mut self) -> Vec<LogEvent> {
        let mut events = self.events.lock().unwrap();
        let logs = events
            .iter()
            .filter_map(|event| match event {
                Event::Log(log) => Some(log.clone()),
                _ => None,
            })
            .collect();
        events.retain(|event| !matches!(event, Event::Log(_)));
        logs
    }

    /// The call records written to the call log so far.
    pub fn logged_calls(&self) -> Vec<CallRecord> {
        let bytes = self.call_log.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a call record"))
            .collect()
    }

    /// Line `caller` dials line `callee`, which answers by command. Both
    /// have echo off, and both `CONNECT`s are checked and consumed.
    pub fn connect(&mut self, caller: u16, callee: u16) {
        self.quiet(caller);
        self.quiet(callee);
        assert_eq!(self.command(caller, &format!("ATD{callee}")), "");
        assert_eq!(self.read(callee), "\r\nRING\r\n");
        assert_eq!(self.command(callee, "ATA"), "\r\nCONNECT\r\n");
        assert_eq!(self.read(caller), "\r\nCONNECT\r\n");
        assert_eq!(self.state(caller), LineState::Online);
        assert_eq!(self.state(callee), LineState::Online);
    }
}

/// A short description of each event, for comparing sequences.
pub fn summarise(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            Event::LineState(info) => format!("line {} {}", info.id, info.state.as_str()),
            Event::CallStarted(e) => format!(
                "call {} started on {} {} {}",
                e.call.id,
                e.line,
                e.call.direction.as_str(),
                e.call.number
            ),
            Event::CallConnected(e) => format!("call {} connected on {}", e.call.id, e.line),
            Event::CallEnded(record) => format!(
                "call {} ended on {} {}",
                record.id,
                record.line,
                record.reason.as_str()
            ),
            Event::Log(log) => format!("log {}", log.text),
            Event::SipRegistration(e) => format!(
                "line {} registration {}",
                e.line,
                e.registration.state.as_str()
            ),
            Event::SipTrace(trace) => format!("trace {}", trace.direction.as_str()),
        })
        .collect()
}

/// Bytes that repeat only after a very long time, from a seed.
pub fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect()
}
