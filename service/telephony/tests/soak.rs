// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The soak test of the phase: a whole service with 24 lines on memory
//! ports and SIP on the loopback interface takes 24 calls at once from 24
//! test peers and holds them, for a minute, or for as many minutes as
//! `RM_SOAK_MINUTES` says.
//!
//! Half the lines run the echo engine: their peers send a pattern and
//! check that it comes back as it went. The other half run the null
//! engine, and so do their peers: data goes both ways between each peer
//! and the host on the line's port, and every octet is checked as it
//! arrives. At the end nothing may have been lost, resynchronised or
//! slipped on any call, the data must be whole, and every call must end
//! cleanly when its peer hangs up.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use config::{Config, ConfigFile, LineConfig};
use ctlproto::{EndReason, Event, LineState};
use media::ThreadedRunner;
use msg::Message;
use pool::{EventSink, MemoryProfiles, Now, Pool, PoolParts, SerialFactory, engine_for};
use serial::{MemoryHost, SerialBackend, memory_pair};
use sippeer::{Agent, AgentConfig, MediaKind, MediaReport, PeerEvent, loopback};
use telephony::{
    RouterParts, Settings, SipRouter, TelephonyHandle, UdpTransport, Unobserved, system_random,
};

/// How many lines, and calls.
const LINES: u16 = 24;

/// Data octets a second, each way, on a call that carries data. The null
/// engine carries up to a sample's worth for every sample; this leaves it
/// room.
const RATE: u64 = 2000;

/// How long the calls are held.
fn soak() -> Duration {
    let minutes = std::env::var("RM_SOAK_MINUTES")
        .ok()
        .and_then(|minutes| minutes.parse::<u64>().ok());
    Duration::from_secs(minutes.map_or(60, |minutes| minutes * 60))
}

/// Whether a line runs the echo engine: the odd ones do.
fn echoes(line: u16) -> bool {
    line % 2 == 1
}

/// Octet `index` of a stream of data that does not repeat for a long
/// time: the stream of `seed`.
fn octet(seed: u64, index: u64) -> u8 {
    let mixed = (index.wrapping_mul(0x9E37_79B9) ^ seed.wrapping_mul(0x85EB_CA6B)) >> 7;
    (mixed ^ index) as u8
}

/// One direction of data on a call, as its two ends see it.
#[derive(Default)]
struct Stream {
    sent: AtomicU64,
    received: AtomicU64,
    /// An octet arrived that is not the one that was sent in its place.
    wrong: AtomicBool,
}

impl Stream {
    /// The next octets to send, up to what `elapsed` allows at [`RATE`].
    fn due(&self, seed: u64, elapsed: Duration) -> Vec<u8> {
        let sent = self.sent.load(Ordering::Relaxed);
        let target = elapsed.as_millis() as u64 * RATE / 1000;
        (sent..target.min(sent + 512))
            .map(|index| octet(seed, index))
            .collect()
    }

    /// Checks octets that arrived against the ones that were sent.
    fn check(&self, seed: u64, arrived: &[u8]) {
        let at = self.received.load(Ordering::Relaxed);
        for (offset, got) in arrived.iter().enumerate() {
            if *got != octet(seed, at + offset as u64) {
                self.wrong.store(true, Ordering::Relaxed);
            }
        }
        self.received
            .store(at + arrived.len() as u64, Ordering::Relaxed);
    }
}

/// What the two threads of the test share.
#[derive(Default)]
struct Shared {
    /// From the host of each line to its peer, and back, by line.
    up: BTreeMap<u16, Stream>,
    down: BTreeMap<u16, Stream>,
    /// No more data is sent: what is on its way arrives.
    quiet: AtomicBool,
    /// The peers hang up.
    hang_up: AtomicBool,
}

fn up_seed(line: u16) -> u64 {
    u64::from(line)
}

fn down_seed(line: u16) -> u64 {
    1000 + u64::from(line)
}

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

/// The configuration: SIP on `listen`, a line for each call that answers
/// by itself, and a number for each line.
fn configuration(listen: SocketAddrV4) -> Config {
    // A stretch of ports the other test programs do not use.
    let first = 45_000 + (std::process::id() % 50) as u16 * 100;
    let mut text = format!(
        "[sip]\nenabled = true\nlisten = \"{listen}\"\ntrusted_peers = [\"127.0.0.1\"]\n\n\
         [media]\nrtp_ports = \"{first}-{}\"\n\n",
        first + 99
    );
    for line in 1..=LINES {
        let engine = if echoes(line) { "echo" } else { "null" };
        text.push_str(&format!(
            "[[line]]\nid = {line}\nserial = {{ backend = \"com0com\", port = \"P{line}\" }}\n\
             engine = \"{engine}\"\nauto_answer_rings = 1\n\n"
        ));
    }
    for line in 1..=LINES {
        text.push_str(&format!(
            "[[route]]\ncalled = \"55501{line:02}\"\ntarget = \"line:{line}\"\n\n"
        ));
    }
    ConfigFile::parse(&text).unwrap().config().clone()
}

/// What a peer has to say of its call at the end.
struct Outcome {
    line: u16,
    answered: bool,
    /// The far end hung up, or the call failed, before the peer did.
    ended_early: bool,
    media: Option<MediaReport>,
}

/// The far end: a peer for each line, each calling its line's number and
/// holding the call until told to hang up.
fn far_end(service: SocketAddrV4, shared: &Shared) -> Vec<Outcome> {
    struct Caller {
        line: u16,
        agent: Agent,
        answered: Option<Instant>,
        ended_early: bool,
    }
    let mut callers: Vec<Caller> = (1..=LINES)
        .map(|line| {
            let mut agent = Agent::bind(AgentConfig {
                media: if echoes(line) {
                    MediaKind::Pattern
                } else {
                    MediaKind::Null
                },
                seed: 0x50AC + u64::from(line),
                ..AgentConfig::new(&format!("caller{line}"), loopback())
            })
            .unwrap();
            agent.invite(&format!("sip:55501{line:02}@{service}"), service);
            Caller {
                line,
                agent,
                answered: None,
                ended_early: false,
            }
        })
        .collect();

    let mut hung_up_at = None;
    let mut buffer = [0u8; 2048];
    loop {
        let quiet = shared.quiet.load(Ordering::Relaxed);
        let mut busy = false;
        for caller in &mut callers {
            busy |= caller.agent.step().unwrap();
            for event in caller.agent.events() {
                match event {
                    PeerEvent::Answered { .. } => caller.answered = Some(Instant::now()),
                    PeerEvent::Refused { .. }
                    | PeerEvent::HungUp { .. }
                    | PeerEvent::TimedOut { .. } => caller.ended_early |= hung_up_at.is_none(),
                    _ => {}
                }
            }
            // The data of a call that carries some.
            let (Some(answered), Some(engine)) = (caller.answered, caller.agent.engine()) else {
                continue;
            };
            let (up, down) = (&shared.up[&caller.line], &shared.down[&caller.line]);
            if !quiet {
                let due = down.due(down_seed(caller.line), answered.elapsed());
                let taken = engine.dte_write(&due);
                down.sent.fetch_add(taken as u64, Ordering::Relaxed);
            }
            let count = engine.dte_read(&mut buffer);
            if count > 0 {
                up.check(up_seed(caller.line), &buffer[..count]);
            }
        }
        if shared.hang_up.load(Ordering::Relaxed) && hung_up_at.is_none() {
            for caller in &mut callers {
                caller.agent.hang_up();
            }
            hung_up_at = Some(Instant::now());
        }
        // The BYEs go and are answered.
        if hung_up_at.is_some_and(|at| at.elapsed() > Duration::from_secs(1)) {
            break;
        }
        if !busy {
            std::thread::sleep(Duration::from_micros(300));
        }
    }
    callers
        .into_iter()
        .map(|caller| Outcome {
            line: caller.line,
            answered: caller.answered.is_some(),
            ended_early: caller.ended_early,
            media: caller.agent.report(),
        })
        .collect()
}

#[test]
fn twenty_four_calls_at_once_lose_nothing_and_carry_their_data_whole() {
    let held = soak();
    let transport =
        UdpTransport::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), Arc::new(|| {})).unwrap();
    let service = transport.local();
    let config = configuration(service);
    let router = SipRouter::new(RouterParts {
        settings: Settings::new(&config, "0.0.3"),
        transport: Box::new(transport),
        random: system_random(),
        observer: Box::new(Unobserved),
        handle: TelephonyHandle::new(),
        version: "0.0.3".to_string(),
    });
    let hosts = Hosts::default();
    let mut pool = Pool::new(PoolParts {
        router: Box::new(router),
        media: Box::new(ThreadedRunner::new(usize::from(LINES), Arc::new(|| {})).unwrap()),
        serial: Box::new(Ports(Arc::clone(&hosts))),
        engines: Box::new(engine_for),
        sink: Box::new(Sink),
        profiles: Box::new(MemoryProfiles::new()),
        call_log: None,
    });
    pool.apply_config(&config);

    let started = Instant::now();
    let now = || {
        let mono_ms = started.elapsed().as_millis() as u64;
        Now {
            mono_ms,
            unix_ms: 1_800_000_000_000 + mono_ms,
        }
    };
    let states = |pool: &Pool| -> Vec<LineState> {
        (1..=LINES)
            .map(|line| pool.line(line).unwrap().state)
            .collect()
    };

    // The hosts open their ports and tell their modems to say nothing:
    // what a host is sent during a call is then the data and only that.
    pool.service(now());
    let hosts: BTreeMap<u16, MemoryHost> = hosts.lock().unwrap().clone();
    for host in hosts.values() {
        host.open();
        host.write(b"ATE0Q1\r");
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while states(&pool).iter().any(|state| *state != LineState::Idle) {
        assert!(
            Instant::now() < deadline,
            "the lines are {:?}",
            states(&pool)
        );
        pool.service(now());
        std::thread::sleep(Duration::from_millis(1));
    }
    for host in hosts.values() {
        host.read();
    }

    let mut shared = Shared::default();
    for line in (1..=LINES).filter(|line| !echoes(*line)) {
        shared.up.insert(line, Stream::default());
        shared.down.insert(line, Stream::default());
    }
    let shared = Arc::new(shared);
    let far = {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("far end".to_string())
            .spawn(move || far_end(service, &shared))
            .unwrap()
    };

    // The calls come, are answered by the lines themselves, and are held.
    let mut online: BTreeMap<u16, Instant> = BTreeMap::new();
    let mut all_online = None;
    let mut quiet_at = None;
    let mut worst: BTreeMap<u16, pool::MediaStats> = BTreeMap::new();
    let end = loop {
        pool.service(now());
        for line in 1..=LINES {
            if !online.contains_key(&line) && pool.line(line).unwrap().state == LineState::Online {
                online.insert(line, Instant::now());
            }
        }
        if all_online.is_none() && online.len() == usize::from(LINES) {
            all_online = Some(Instant::now());
        }
        assert!(
            all_online.is_some() || started.elapsed() < Duration::from_secs(40),
            "only {} of {LINES} calls were answered: {:?}",
            online.len(),
            states(&pool)
        );
        // The hosts of the lines that carry data send theirs and check
        // what they are sent.
        for (line, up) in &shared.up {
            let (Some(since), Some(host)) = (online.get(line), hosts.get(line)) else {
                continue;
            };
            if quiet_at.is_none() {
                let due = up.due(up_seed(*line), since.elapsed());
                host.write(&due);
                up.sent.fetch_add(due.len() as u64, Ordering::Relaxed);
            }
            let arrived = host.read();
            if !arrived.is_empty() {
                shared.down[line].check(down_seed(*line), &arrived);
            }
        }
        if let Some(since) = all_online
            && quiet_at.is_none()
            && since.elapsed() >= held
        {
            shared.quiet.store(true, Ordering::Relaxed);
            quiet_at = Some(Instant::now());
        }
        // What was sent last has arrived two seconds on: the calls are
        // looked at as they stand, and the peers hang up.
        if quiet_at.is_some_and(|at: Instant| at.elapsed() > Duration::from_secs(2)) {
            for line in 1..=LINES {
                worst.insert(line, pool.media_stats(line).expect("the call has media"));
            }
            shared.hang_up.store(true, Ordering::Relaxed);
            break Instant::now();
        }
        std::thread::sleep(Duration::from_micros(300));
    };

    // Every call is torn down by its BYE.
    while states(&pool).iter().any(|state| *state != LineState::Idle) {
        assert!(
            end.elapsed() < Duration::from_secs(20),
            "calls were left: {:?}",
            states(&pool)
        );
        pool.service(now());
        std::thread::sleep(Duration::from_millis(1));
    }
    let outcomes = far.join().unwrap();
    let records = pool.calls(None);
    assert_eq!(records.len(), usize::from(LINES));
    for record in &records {
        assert_eq!(record.reason, EndReason::Remote, "line {}", record.line);
        assert!(record.connected_ms.is_some(), "line {}", record.line);
        let media = record.media.as_ref().expect("the record has its media");
        assert_eq!(
            (media.lost, media.resync, media.slips),
            (0, 0, 0),
            "line {}: {media:?}",
            record.line
        );
    }

    // The lines: nothing lost, nothing resynchronised, nothing slipped,
    // and no tick that took as long as a packet.
    let seconds = held.as_secs();
    let mut slowest = (0u32, 0u32, 0u32);
    for (line, stats) in &worst {
        assert_eq!(
            (stats.lost, stats.resync, stats.slips),
            (0, 0, 0),
            "line {line}: {stats:?}"
        );
        assert!(
            stats.rx_packets >= seconds * 50 * 9 / 10,
            "line {line}: {stats:?}"
        );
        assert!(
            stats.tick_p99_us < stats.ptime_ms * 1000,
            "line {line}: {stats:?}"
        );
        slowest = (
            slowest.0.max(stats.tick_p50_us),
            slowest.1.max(stats.tick_p99_us),
            slowest.2.max(stats.tick_max_us),
        );
    }
    println!(
        "soak: {LINES} calls for {seconds} s; of the lines' tick times the greatest median was \
         {} us, the greatest 99th percentile {} us and the longest tick {} us",
        slowest.0, slowest.1, slowest.2
    );

    // The peers: every call answered and held, every packet back, and on
    // the lines that echo every frame as it was sent.
    for outcome in &outcomes {
        let line = outcome.line;
        assert!(outcome.answered, "the call to line {line} was not answered");
        assert!(!outcome.ended_early, "the call to line {line} ended early");
        let media = outcome.media.expect("the peer had media");
        assert_eq!(
            (media.received.lost, media.received.resync),
            (0, 0),
            "the peer of line {line}: {media:?}"
        );
        if echoes(line) {
            assert_eq!(media.mismatched_frames, 0, "line {line}: {media:?}");
            assert!(
                media.echoed_frames >= seconds * 100 * 9 / 10,
                "line {line}: {media:?}"
            );
        }
    }

    // The data: each end was sent exactly what the other sent it.
    for (line, up) in &shared.up {
        let down = &shared.down[line];
        for (stream, way) in [(up, "host to peer"), (down, "peer to host")] {
            let (sent, received) = (
                stream.sent.load(Ordering::Relaxed),
                stream.received.load(Ordering::Relaxed),
            );
            assert!(
                !stream.wrong.load(Ordering::Relaxed),
                "line {line}, {way}: an octet arrived that was not the one sent"
            );
            assert_eq!(sent, received, "line {line}, {way}");
            assert!(
                sent >= seconds * RATE * 8 / 10,
                "line {line}, {way}: {sent}"
            );
        }
    }
}
