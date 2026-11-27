// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A hundred lines on memory ports, on the service's own runner: the
//! pool's task on the runtime, and a thread for every port.
//!
//! When the pool waited on its ports' handles itself, one wait took 64 of
//! them and the ports past the 63rd were served at the pool's tick. Here
//! every line is served as the first is.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use config::{Config, LineConfig};
use ctlproto::{EndReason, Event, LineState};
use ctlsrv::PoolLink;
use engine::{NullConfig, NullEngine};
use msg::Message;
use pool::{EventSink, InlineRunner, MemoryProfiles, NullSwitch, Pool, PoolParts, SerialFactory};
use serial::{MemoryHost, SerialBackend, memory_pair};
use svc::bridge::BridgedPorts;
use svc::pooltask::{LINGER, PoolClock, PoolTask, system_unix_clock};
use tokio::sync::Notify;

/// How many lines.
const LINES: u16 = 100;

type Hosts = Arc<Mutex<BTreeMap<u16, MemoryHost>>>;

struct Ports(Hosts);

impl SerialFactory for Ports {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        let (backend, host) = memory_pair();
        self.0.lock().unwrap().insert(line.id, host);
        Ok(Box::new(backend))
    }
}

struct Sink;

impl EventSink for Sink {
    fn publish(&mut self, _event: Event) {}
}

/// Waits until `check` is true, or fails the test.
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The states of the lines `from..=to`.
fn states(pool: &PoolLink, from: u16, to: u16) -> Vec<LineState> {
    pool.blocking_lines()
        .unwrap()
        .iter()
        .filter(|line| (from..=to).contains(&line.id))
        .map(|line| line.state)
        .collect()
}

/// Octet `index` of the data line `line`'s host sends.
fn octet(line: u16, index: usize) -> u8 {
    (index as u32)
        .wrapping_mul(31)
        .wrapping_add(u32::from(line) * 7) as u8
}

#[test]
fn a_hundred_lines_on_memory_ports_are_served_as_the_first_is() {
    let runtime = svc::runtime::build().unwrap();
    let (shutdown, stopping) = svc::shutdown::channel();
    let config = Config {
        lines: (1..=LINES)
            .map(|id| LineConfig {
                engine: config::EngineKind::Null,
                ..LineConfig::new(id, &format!("P{id}"))
            })
            .collect(),
        ..Config::default()
    };
    let hosts = Hosts::default();
    let wake = Arc::new(Notify::new());
    let mut pool = Pool::new(PoolParts {
        router: Box::new(NullSwitch::new()),
        media: Box::new(InlineRunner::new()),
        serial: Box::new(BridgedPorts::new(
            Box::new(Ports(Arc::clone(&hosts))),
            Arc::clone(&wake),
        )),
        engines: Box::new(|_, _| Box::new(NullEngine::new(NullConfig::default()))),
        sink: Box::new(Sink),
        profiles: Box::new(MemoryProfiles::new()),
        call_log: None,
    });
    pool.apply_config(&config);
    let (link, requests) = ctlsrv::pool_link();
    let task = runtime.spawn(
        PoolTask {
            pool,
            requests,
            wake,
            media_events: None,
            clock: PoolClock::Running(system_unix_clock()),
            linger: LINGER,
        }
        .run(stopping),
    );

    // Every port is opened, by the pool and then by its host, and every
    // line is idle.
    eventually("every port to be opened", || {
        hosts.lock().unwrap().len() == usize::from(LINES)
    });
    let hosts: BTreeMap<u16, MemoryHost> = hosts.lock().unwrap().clone();
    for host in hosts.values() {
        host.open();
    }
    eventually("every line to be idle", || {
        states(&link, 1, LINES)
            .iter()
            .all(|state| *state == LineState::Idle)
    });

    // Every line answers its host, the hundredth as the first.
    let mut heard: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
    for host in hosts.values() {
        host.write(b"ATE0Q0\r");
    }
    eventually("every line to say OK", || {
        for (line, host) in &hosts {
            heard.entry(*line).or_default().extend(host.read());
        }
        heard.len() == usize::from(LINES)
            && heard.values().all(|said| said.ends_with(b"\r\nOK\r\n"))
    });

    // Fifty calls at once: each line of the first half dials its opposite
    // number in the second, which rings and answers.
    let half = LINES / 2;
    for line in 1..=half {
        hosts[&line].write(format!("ATD{}\r", line + half).as_bytes());
    }
    eventually("the second half to ring", || {
        states(&link, half + 1, LINES)
            .iter()
            .all(|state| *state == LineState::Ringing)
    });
    for line in half + 1..=LINES {
        hosts[&line].write(b"ATA\r");
    }
    eventually("every line to be online", || {
        states(&link, 1, LINES)
            .iter()
            .all(|state| *state == LineState::Online)
    });
    for host in hosts.values() {
        assert!(host.signals().dcd);
    }
    // What was said on the way to the call is read and done with.
    std::thread::sleep(Duration::from_millis(100));
    for host in hosts.values() {
        host.read();
    }

    // Data both ways on every call, each octet where it was sent.
    const OCTETS: usize = 400;
    for (line, host) in &hosts {
        let data: Vec<u8> = (0..OCTETS).map(|index| octet(*line, index)).collect();
        host.write(&data);
    }
    let mut arrived: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
    eventually("the data to arrive", || {
        for (line, host) in &hosts {
            arrived.entry(*line).or_default().extend(host.read());
        }
        arrived.len() == usize::from(LINES) && arrived.values().all(|data| data.len() >= OCTETS)
    });
    for (line, data) in &arrived {
        let far = if *line <= half {
            line + half
        } else {
            line - half
        };
        let sent: Vec<u8> = (0..OCTETS).map(|index| octet(far, index)).collect();
        assert_eq!(data, &sent, "line {line} hears line {far}");
    }

    // The first half is hung up by the operator; the second hears it.
    for line in 1..=half {
        link.blocking_with(move |pool| pool.hangup(line))
            .unwrap()
            .unwrap();
    }
    eventually("every line to be idle again", || {
        states(&link, 1, LINES)
            .iter()
            .all(|state| *state == LineState::Idle)
    });
    for line in half + 1..=LINES {
        let said = String::from_utf8(hosts[&line].read()).unwrap();
        assert!(
            said.ends_with("\r\nNO CARRIER\r\n"),
            "line {line}: {said:?}"
        );
    }
    let records = link.blocking_with(|pool| pool.calls(None)).unwrap();
    assert_eq!(records.len(), usize::from(LINES));
    for record in &records {
        assert!(record.connected_ms.is_some(), "line {}", record.line);
        let expected = if record.line <= half {
            EndReason::Operator
        } else {
            EndReason::Remote
        };
        assert_eq!(record.reason, expected, "line {}", record.line);
    }

    // The service stops: the task ends and every port is released.
    let stopping_at = Instant::now();
    shutdown.trigger();
    runtime.block_on(task).unwrap();
    assert!(stopping_at.elapsed() < Duration::from_secs(10));
    for (line, host) in &hosts {
        assert!(!host.backend_alive(), "port {line} was released");
    }
}
