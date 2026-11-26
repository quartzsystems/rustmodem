// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A whole service against the test peer on real sockets, with the call
//! captured: the peer calls a line over SIP, sends RTP by a clock of its
//! own, with and without faults, and checks what comes back; the capture
//! of the call is then read and checked as `rustmodemctl capture check`
//! does.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use config::{Config, ConfigFile, LineConfig};
use ctlproto::{Event, LineState};
use media::{CaptureEvent, CaptureWriter, ThreadedRunner};
use msg::Message;
use pool::{EventSink, MemoryProfiles, Now, Pool, PoolParts, SerialFactory, engine_for};
use rtp::capture::{Reader, Report, check, write_wav};
use serial::{MemoryHost, SerialBackend, memory_pair};
use sippeer::{Agent, AgentConfig, Impairments, MediaKind, PeerEvent, loopback};
use telephony::{
    RouterParts, Settings, SipRouter, TelephonyHandle, UdpTransport, Unobserved, system_random,
};

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

static TESTS: AtomicU32 = AtomicU32::new(0);

/// A service with one line, its calls captured, and a peer to call it.
struct Bench {
    pool: Pool,
    hosts: Hosts,
    sip: SocketAddrV4,
    agent: Agent,
    started: Instant,
    /// Everything the line's host has been sent.
    seen: Vec<u8>,
    directory: PathBuf,
    /// The capture files the writer says it began.
    captures: Arc<Mutex<Vec<PathBuf>>>,
}

impl Bench {
    fn new(engine: &str, ptime: u32, agent: AgentConfig) -> Bench {
        let test = TESTS.fetch_add(1, Ordering::Relaxed);
        let transport =
            UdpTransport::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), Arc::new(|| {})).unwrap();
        let sip = transport.local();
        // A stretch of ports the tests of this program, and the other
        // test programs, do not fight over.
        let first = 41_000 + (std::process::id() % 100) as u16 * 40 + test as u16 * 8;
        let text = format!(
            r#"[sip]
enabled = true
listen = "{sip}"
trusted_peers = ["127.0.0.1"]

[media]
rtp_ports = "{first}-{}"
ptime_ms = {ptime}

[[line]]
id = 1
serial = {{ backend = "com0com", port = "P1" }}
engine = "{engine}"

[[hunt_group]]
name = "in"
pilot = "5551000"
lines = [1]
"#,
            first + 7,
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
        let directory =
            std::env::temp_dir().join(format!("rustmodem-peer-{}-{test}", std::process::id()));
        let captures = Arc::new(Mutex::new(Vec::new()));
        let began = Arc::clone(&captures);
        let writer = CaptureWriter::new(
            directory.clone(),
            Box::new(move |event| match event {
                CaptureEvent::Started { path, .. } => began.lock().unwrap().push(path),
                CaptureEvent::Failed { path, error, .. } => {
                    panic!("the capture {} failed: {error}", path.display())
                }
            }),
        )
        .unwrap();
        let hosts = Hosts::default();
        let mut pool = Pool::new(PoolParts {
            router: Box::new(router),
            media: Box::new(ThreadedRunner::with_capture(1, Arc::new(|| {}), writer).unwrap()),
            serial: Box::new(Ports(Arc::clone(&hosts))),
            engines: Box::new(engine_for),
            sink: Box::new(Sink),
            profiles: Box::new(MemoryProfiles::new()),
            call_log: None,
        });
        pool.apply_config(&config);
        assert!(pool.set_capture(1, true).is_ok());
        let mut bench = Bench {
            pool,
            hosts,
            sip,
            agent: Agent::bind(agent).unwrap(),
            started: Instant::now(),
            seen: Vec::new(),
            directory,
            captures,
        };
        bench.turn();
        bench.host().open();
        bench.until("the line is idle", |bench| {
            bench.pool.line(1).unwrap().state == LineState::Idle
        });
        bench.host().write(b"ATE0\r");
        bench.until("echo is off", |bench| bench.seen.ends_with(b"OK\r\n"));
        bench.seen.clear();
        bench
    }

    fn host(&self) -> MemoryHost {
        self.hosts.lock().unwrap()[&1].clone()
    }

    /// One turn of the pool and of the peer.
    fn turn(&mut self) {
        let mono_ms = self.started.elapsed().as_millis() as u64;
        self.pool.service(Now {
            mono_ms,
            unix_ms: 1_800_000_000_000 + mono_ms,
        });
        let host = self.hosts.lock().unwrap().get(&1).cloned();
        if let Some(host) = host {
            self.seen.extend(host.read());
        }
        self.agent.step().unwrap();
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Bench) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.turn();
            if done(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "waited too long until {what}; the peer's media: {:?}; the line's: {:?}",
                self.agent.report(),
                self.pool.media_stats(1),
            );
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    fn run(&mut self, time: Duration) {
        let until = Instant::now() + time;
        self.until("the time has passed", |_| Instant::now() >= until);
    }

    /// The peer calls the pilot number, the host answers, and the call is
    /// through at both ends.
    fn connect(&mut self) {
        let uri = format!("sip:5551000@{}", self.sip);
        self.agent.invite(&uri, self.sip);
        self.until("the line rings", |bench| bench.seen.ends_with(b"RING\r\n"));
        self.host().write(b"ATA\r");
        let mut answered = false;
        self.until("the call is answered", |bench| {
            answered |= bench
                .agent
                .events()
                .iter()
                .any(|event| matches!(event, PeerEvent::Answered { .. }));
            answered && bench.seen.ends_with(b"CONNECT\r\n")
        });
        self.seen.clear();
    }

    /// The peer hangs up; the service is shut and the capture of the call
    /// is checked. Gives the report and the frames of its WAV file.
    fn finish(mut self) -> (Report, u64) {
        self.agent.hang_up();
        self.until("the call is cleared", |bench| {
            bench.seen.ends_with(b"NO CARRIER\r\n")
        });
        let Bench {
            pool,
            directory,
            captures,
            ..
        } = self;
        // The writer ends, with everything written, when the runner does.
        drop(pool);
        let captures = captures.lock().unwrap().clone();
        assert_eq!(captures.len(), 1, "one call, one capture");
        let open = || Reader::new(BufReader::new(File::open(&captures[0]).unwrap())).unwrap();
        let mut reader = open();
        assert_eq!(reader.header().line, 1);
        assert!(!reader.header().call_id.is_empty());
        let report = check(&mut reader).unwrap();
        let mut wav = Vec::new();
        let frames = write_wav(&mut open(), &mut wav).unwrap();
        assert_eq!(
            wav.len() as u64,
            58 + frames * 80 * 2,
            "a WAV of two channels"
        );
        let _ = std::fs::remove_dir_all(&directory);
        (report, frames)
    }
}

fn peer(media: MediaKind, ptime_ms: u32, impairments: Impairments) -> AgentConfig {
    AgentConfig {
        media,
        ptime_ms,
        impairments,
        seed: 0x5EED + u64::from(ptime_ms),
        ..AgentConfig::new("caller", loopback())
    }
}

/// A clean call to an echo line: every frame the peer sent comes back as
/// it was sent, and the capture says the call was sample-exact.
fn clean_echo(ptime: u32) {
    let mut bench = Bench::new(
        "echo",
        ptime,
        peer(MediaKind::Pattern, ptime, Impairments::default()),
    );
    bench.connect();
    bench.run(Duration::from_secs(3));
    let seen = bench.agent.report().unwrap();
    assert_eq!(bench.agent.negotiated().unwrap().ptime_ms, ptime);
    assert!(seen.echoed_frames > 200, "{seen:?}");
    assert_eq!(seen.mismatched_frames, 0, "{seen:?}");
    assert_eq!(
        (seen.received.lost, seen.received.resync),
        (0, 0),
        "{seen:?}"
    );

    let (report, frames) = bench.finish();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!((report.lost, report.resync, report.slips), (0, 0, 0));
    assert!(report.received > 3000 / u64::from(ptime) - 10, "{report:?}");
    assert!(frames >= 290, "{frames} frames in the WAV");
}

#[test]
fn a_clean_call_to_an_echo_line_is_sample_exact_at_20_ms() {
    clean_echo(20);
}

#[test]
fn a_clean_call_to_an_echo_line_is_sample_exact_at_10_ms() {
    clean_echo(10);
}

/// Loss, duplicates, reordering, jitter and a clock that runs fast: the
/// echo that does come back is still the peer's own, the capture counts
/// the faults, and the line never slips: it sends a sample for every
/// sample it takes, the fill frames with them.
#[test]
fn a_call_with_faults_never_slips() {
    let faults = Impairments {
        loss: 30,
        duplicate: 30,
        reorder: 30,
        jitter_ms: 4,
        clock_ppm: 300,
    };
    let mut bench = Bench::new("echo", 20, peer(MediaKind::Pattern, 20, faults));
    bench.connect();
    bench.run(Duration::from_secs(6));
    let seen = bench.agent.report().unwrap();
    assert!(
        seen.lost > 0 && seen.duplicated > 0 && seen.reordered > 0,
        "{seen:?}"
    );
    assert!(seen.echoed_frames > 400, "{seen:?}");
    assert_eq!(seen.mismatched_frames, 0, "{seen:?}");

    let (report, _) = bench.finish();
    assert!(report.lost > 0, "{report:?}");
    assert!(report.duplicate > 0, "{report:?}");
    assert!(report.late > 0, "{report:?}");
    assert!(!report.is_clean());
    assert_eq!(report.slips, 0, "{report:?}");
    assert_eq!(report.resync, 0, "{report:?}");
    assert_eq!(report.dropped_records, 0, "{report:?}");
}

/// The peer's null engine is the far end of a line with the null engine:
/// data goes both ways between the peer and the line's host.
#[test]
fn the_peer_carries_data_as_a_null_engine_far_end() {
    let mut bench = Bench::new(
        "null",
        20,
        peer(MediaKind::Null, 20, Impairments::default()),
    );
    bench.connect();

    let down: Vec<u8> = (0..20_000u32).map(|n| (n * 7 + n / 251) as u8).collect();
    let up: Vec<u8> = (0..20_000u32).map(|n| (n * 13 + n / 127) as u8).collect();
    bench.host().write(&up);
    let mut sent = 0;
    let mut got = Vec::new();
    bench.until("the data has crossed both ways", |bench| {
        let engine = bench.agent.engine().expect("the call has an engine");
        sent += engine.dte_write(&down[sent..]);
        let mut buffer = [0u8; 2048];
        let len = engine.dte_read(&mut buffer);
        got.extend_from_slice(&buffer[..len]);
        got.len() >= up.len() && bench.seen.len() >= down.len()
    });
    assert!(got == up, "what the peer was sent is what the host wrote");
    assert!(
        bench.seen == down,
        "what the host was sent is what the peer wrote"
    );

    bench.seen.clear();
    let (report, _) = bench.finish();
    assert!(report.is_clean(), "{report:?}");
}
