// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The transparency test of the phase: two services, each a whole pool
//! with the SIP router and the threaded media runner, call each other over
//! SIP and RTP on the loopback interface, and a megabyte goes each way
//! through their null engines. One octet altered, dropped, doubled or
//! slipped anywhere on the path shows as wrong data or a lost carrier.

mod support;

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use config::{Config, ConfigFile, LineConfig};
use ctlproto::{EndReason, Event, LineState};
use media::ThreadedRunner;
use msg::Message;
use pool::{EventSink, MemoryProfiles, Now, Pool, PoolParts, SerialFactory, engine_for};
use serial::{MemoryHost, SerialBackend, memory_pair};
use support::sip_socket;
use svc::sipsock::SipSocket;
use telephony::{RouterParts, Settings, SipRouter, TelephonyHandle, Unobserved, system_random};

type Hosts = Arc<Mutex<BTreeMap<u16, MemoryHost>>>;

struct Ports(Hosts);

impl SerialFactory for Ports {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        let (backend, host) = memory_pair();
        self.0.lock().unwrap().insert(line.id, host);
        Ok(Box::new(backend))
    }
}

struct Sink(Arc<Mutex<Vec<Event>>>);

impl EventSink for Sink {
    fn publish(&mut self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

/// One service: a pool with one line on a memory port, SIP on a loopback
/// socket, and the media threads.
struct Service {
    pool: Pool,
    hosts: Hosts,
    events: Arc<Mutex<Vec<Event>>>,
    sip: SocketAddrV4,
    /// Everything the line's host has been sent.
    seen: Vec<u8>,
}

impl Service {
    /// `engine` is the line's engine; `outbound` is where the line's
    /// calls go.
    fn new(transport: SipSocket, engine: &str, outbound: Option<SocketAddrV4>) -> Service {
        let sip = transport.local();
        // A stretch of ports the two services and the other test programs
        // do not fight over.
        let first = 36_000 + (std::process::id() % 100) as u16 * 40;
        let text = format!(
            r#"[sip]
enabled = true
listen = "{sip}"
trusted_peers = ["127.0.0.1"]
outbound_peer = "{}"

[media]
rtp_ports = "{first}-{}"
rtp_timeout_s = 5

[[line]]
id = 1
serial = {{ backend = "com0com", port = "P1" }}
engine = "{engine}"

[[hunt_group]]
name = "in"
pilot = "5551000"
lines = [1]
"#,
            outbound.map(|peer| peer.to_string()).unwrap_or_default(),
            first + 39,
        );
        let config: Config = ConfigFile::parse(&text).unwrap().config().clone();
        let router = SipRouter::new(RouterParts {
            settings: Settings::new(&config, "0.0.3"),
            transport: Box::new(transport),
            random: system_random(),
            observer: Box::new(Unobserved),
            handle: TelephonyHandle::new(),
            version: "0.0.3".to_string(),
        });
        let hosts = Hosts::default();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut pool = Pool::new(PoolParts {
            router: Box::new(router),
            media: Box::new(ThreadedRunner::new(1, Arc::new(|| {})).unwrap()),
            serial: Box::new(Ports(Arc::clone(&hosts))),
            engines: Box::new(engine_for),
            sink: Box::new(Sink(Arc::clone(&events))),
            profiles: Box::new(MemoryProfiles::new()),
            call_log: None,
        });
        pool.apply_config(&config);
        Service {
            pool,
            hosts,
            events,
            sip,
            seen: Vec::new(),
        }
    }

    fn host(&self) -> MemoryHost {
        self.hosts.lock().unwrap()[&1].clone()
    }

    fn state(&self) -> LineState {
        self.pool.line(1).unwrap().state
    }

    /// What the host has been sent since the last look, as text.
    fn heard(&mut self) -> String {
        String::from_utf8_lossy(&std::mem::take(&mut self.seen)).into_owned()
    }
}

/// Two services and the wall clock.
struct Pair {
    a: Service,
    b: Service,
    started: Instant,
}

impl Pair {
    /// `a` sends the calls its line dials to `b`. Neither has a host yet.
    fn new(engine_a: &str, engine_b: &str) -> Pair {
        let loopback = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let transport_a = sip_socket(loopback, Arc::new(|| {})).unwrap();
        let transport_b = sip_socket(loopback, Arc::new(|| {})).unwrap();
        let b_at = transport_b.local();
        let mut pair = Pair {
            a: Service::new(transport_a, engine_a, Some(b_at)),
            b: Service::new(transport_b, engine_b, None),
            started: Instant::now(),
        };
        pair.turn();
        pair.a.host().open();
        pair.b.host().open();
        pair.until("both lines are idle", |pair| {
            pair.a.state() == LineState::Idle && pair.b.state() == LineState::Idle
        });
        pair
    }

    /// One turn of both pools, at the time it is.
    fn turn(&mut self) {
        let mono_ms = self.started.elapsed().as_millis() as u64;
        let now = Now {
            mono_ms,
            unix_ms: 1_800_000_000_000 + mono_ms,
        };
        for service in [&mut self.a, &mut self.b] {
            service.pool.service(now);
            let host = service.hosts.lock().unwrap().get(&1).cloned();
            if let Some(host) = host {
                service.seen.extend(host.read());
            }
        }
    }

    /// Runs both pools until `done`, or fails saying what was waited for.
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Pair) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            self.turn();
            if done(self) {
                return;
            }
            if Instant::now() >= deadline {
                // Say what the media was doing, twice, a second apart.
                let first = (self.a.pool.media_stats(1), self.b.pool.media_stats(1));
                for _ in 0..1000 {
                    self.turn();
                    std::thread::sleep(Duration::from_millis(1));
                }
                let second = (self.a.pool.media_stats(1), self.b.pool.media_stats(1));
                panic!(
                    "waited too long until {what}: a is {:?} with {} bytes, b is {:?} with {} \
                     bytes\nmedia then: {first:?}\nmedia a second later: {second:?}",
                    self.a.state(),
                    self.a.seen.len(),
                    self.b.state(),
                    self.b.seen.len(),
                );
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    /// a's host dials b's pilot number, b's host answers, and both are
    /// told the call is connected.
    fn connect(&mut self) {
        self.a.host().write(b"ATE0\r");
        self.b.host().write(b"ATE0\r");
        self.until("echo is off", |pair| {
            pair.a.seen.ends_with(b"OK\r\n") && pair.b.seen.ends_with(b"OK\r\n")
        });
        self.a.heard();
        self.b.heard();

        self.a.host().write(b"ATD5551000\r");
        self.until("b rings", |pair| pair.b.seen.ends_with(b"RING\r\n"));
        assert_eq!(self.b.state(), LineState::Ringing);
        self.b.host().write(b"ATA\r");
        self.until("both are connected", |pair| {
            pair.a.seen.ends_with(b"CONNECT\r\n") && pair.b.seen.ends_with(b"CONNECT\r\n")
        });
        assert_eq!(self.a.heard(), "\r\nCONNECT\r\n");
        assert_eq!(self.b.heard(), "\r\nRING\r\n\r\nCONNECT\r\n");
        assert_eq!(self.a.state(), LineState::Online);
        assert_eq!(self.b.state(), LineState::Online);
    }
}

/// Data that repeats only after a very long time.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect()
}

/// Exit criterion 4 of the phase: a megabyte each way through two null
/// engine lines over SIP and RTP on loopback arrives intact.
#[test]
fn a_megabyte_each_way_between_two_null_engine_lines_over_sip_and_rtp() {
    const SIZE: usize = 1024 * 1024;
    let mut pair = Pair::new("null", "null");
    pair.connect();

    let from_a = noise(0xA11CE, SIZE);
    let from_b = noise(0xB0B, SIZE);
    pair.a.host().write(&from_a);
    pair.b.host().write(&from_b);
    pair.until("a megabyte has gone each way", |pair| {
        pair.a.seen.len() >= SIZE && pair.b.seen.len() >= SIZE
    });
    // A little longer: nothing more is to come.
    for _ in 0..200 {
        pair.turn();
    }
    assert!(
        pair.b.seen == from_a,
        "what a's host sent is what b's host got"
    );
    assert!(
        pair.a.seen == from_b,
        "what b's host sent is what a's host got"
    );
    assert_eq!(
        pair.a.state(),
        LineState::Online,
        "the carrier was never lost"
    );
    assert_eq!(pair.b.state(), LineState::Online);
    pair.a.seen.clear();
    pair.b.seen.clear();

    // a's host hangs up: b is told the carrier is gone.
    pair.a.host().set_dtr(false);
    pair.until("b's call is cleared", |pair| {
        pair.b.seen.ends_with(b"NO CARRIER\r\n")
    });
    pair.until("both lines are free", |pair| {
        pair.a.state() == LineState::NotReady && pair.b.state() == LineState::Idle
    });

    // The call records: every byte is counted on both sides.
    let ended = |service: &Service| {
        service
            .events
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                Event::CallEnded(record) => Some(record.clone()),
                _ => None,
            })
            .expect("the call ended")
    };
    let (a, b) = (ended(&pair.a), ended(&pair.b));
    assert_eq!((a.reason, b.reason), (EndReason::Dtr, EndReason::Remote));
    assert_eq!((a.bytes_tx, a.bytes_rx), (SIZE as u64, SIZE as u64));
    assert_eq!((b.bytes_tx, b.bytes_rx), (SIZE as u64, SIZE as u64));
    assert_eq!(a.number, "5551000");
    assert_eq!(
        b.number, "1",
        "the caller is the line id a trunk line calls as"
    );
}

/// A line with the echo engine sends the caller's own stream back: the
/// null engine at the other end reads back the data it sent.
#[test]
fn a_call_to_an_echo_line_comes_back_over_rtp() {
    let mut pair = Pair::new("null", "echo");
    pair.connect();

    let data = noise(0xEC40, 64 * 1024);
    pair.a.host().write(&data);
    let probe = data.clone();
    let mut quiet_since = Instant::now();
    let mut last_len = 0;
    pair.until("the data has come back", |pair| {
        if pair.a.seen.len() != last_len {
            last_len = pair.a.seen.len();
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() > Duration::from_secs(3) && last_len < probe.len() {
            let at = pair
                .a
                .seen
                .iter()
                .zip(&probe)
                .position(|(got, sent)| got != sent)
                .unwrap_or(last_len);
            let line = pair.a.pool.line(1).unwrap();
            panic!(
                "stuck at {last_len} of {}: first difference at {at}; call {:?}; \
                 sent around there {:02x?}; got {:02x?}",
                probe.len(),
                line.call.map(|c| (c.bytes_tx, c.bytes_rx)),
                &probe[at.saturating_sub(4)..(at + 170).min(probe.len())],
                &pair.a.seen[at.saturating_sub(4)..(at + 12).min(last_len)],
            );
        }
        pair.a.seen.len() >= data.len()
    });
    assert!(pair.a.seen == data);
    assert!(
        pair.b.seen.is_empty(),
        "the echo line's host is sent nothing"
    );

    // The far end, b, hangs up this time.
    pair.a.seen.clear();
    pair.b.host().set_dtr(false);
    pair.until("a's call is cleared", |pair| {
        pair.a.seen.ends_with(b"NO CARRIER\r\n")
    });
}

/// With the media gone and no BYE, the RTP timeout clears the call.
#[test]
fn a_call_whose_media_stops_is_cleared_by_the_timeout() {
    let mut pair = Pair::new("null", "null");
    pair.connect();
    // b vanishes: its pool is no longer served, and its media threads end
    // with it.
    let Pair { mut a, b, started } = pair;
    drop(b);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !a.seen.ends_with(b"NO CARRIER\r\n") {
        assert!(Instant::now() < deadline, "the call was never cleared");
        let mono_ms = started.elapsed().as_millis() as u64;
        a.pool.service(Now {
            mono_ms,
            unix_ms: mono_ms,
        });
        a.seen.extend(a.host().read());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(a.state(), LineState::Idle);
    let _ = a.sip;
}
