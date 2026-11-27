// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A whole service against the test peer on real sockets, with the call
//! captured: the peer calls a line over SIP, sends RTP by a clock of its
//! own, with and without faults, and checks what comes back; the capture
//! of the call is then read and checked as `rustmodemctl capture check`
//! does.

mod support;

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
use support::sip_socket;
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
    /// The name the call's capture is kept under when `RM_CORPUS` names a
    /// directory to keep captures in.
    keep: Option<&'static str>,
}

impl Bench {
    fn new(engine: &str, ptime: u32, agent: AgentConfig) -> Bench {
        let test = TESTS.fetch_add(1, Ordering::Relaxed);
        let transport =
            sip_socket(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), Arc::new(|| {})).unwrap();
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
            media: Box::new(ThreadedRunner::with_capture(1, Arc::new(|| true), writer).unwrap()),
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
            keep: None,
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
        while Instant::now() < until {
            self.turn();
            std::thread::sleep(Duration::from_micros(200));
        }
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
            keep,
            ..
        } = self;
        // The record of the call has its RTP as it was at the end, and the
        // statistics have counted it.
        let records = pool.calls(None);
        let media = records
            .last()
            .and_then(|record| record.media.clone())
            .expect("the call that ended had media");
        assert_eq!(media.codec, "PCMU");
        assert!(
            media.rx_packets > 100 && media.tx_packets > 100,
            "{media:?}"
        );
        assert_eq!((media.slips, media.resync), (0, 0), "{media:?}");
        assert!(media.remote.is_some());
        let stats = pool.stats();
        assert_eq!(stats.codec, "PCMU");
        assert_eq!(stats.sip.listener, ctlproto::ListenerState::Listening);
        assert_eq!(stats.packet_loss_percent > 0.0, media.lost > 0, "{stats:?}");
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
        // The captures of the corpus (`modem/engine/tests/corpus`) are
        // made by running these tests with `RM_CORPUS` set to a directory.
        if let (Some(name), Some(corpus)) = (keep, std::env::var_os("RM_CORPUS")) {
            let kept = PathBuf::from(corpus).join(format!("{name}.rmcap"));
            std::fs::copy(&captures[0], &kept).unwrap();
        }
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
    // Three seconds, or as many minutes as `RM_CAPTURE_MINUTES` says: the
    // ten-minute call of the phase's exit criteria is this test with 10.
    let length = std::env::var("RM_CAPTURE_MINUTES")
        .ok()
        .and_then(|minutes| minutes.parse::<u64>().ok())
        .map_or(Duration::from_secs(3), |minutes| {
            Duration::from_secs(minutes * 60)
        });
    bench.run(length);
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
    let packets = length.as_millis() as u64 / u64::from(ptime);
    assert!(report.received > packets - 10, "{report:?}");
    assert!(
        frames >= length.as_millis() as u64 / 10 - 10,
        "{frames} frames in the WAV"
    );
    println!(
        "capture check at {ptime} ms for {} s: {} received, {} sent, lost {}, resync {}, slips {},          largest gap {} us",
        length.as_secs(),
        report.received,
        report.sent,
        report.lost,
        report.resync,
        report.slips,
        report.largest_gap_us
    );
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

/// The 300 bit/s modem as a peer: both modulations, with V.8.
fn both_with_v8() -> MediaKind {
    MediaKind::modem(
        &[engine::Modulation::V21, engine::Modulation::Bell103],
        true,
    )
}

/// How many octets of `sent` are in `got` in their order: the length of
/// the longest sequence that is in both.
fn in_order(sent: &[u8], got: &[u8]) -> usize {
    let mut row = vec![0usize; got.len() + 1];
    for octet in sent {
        let mut diagonal = 0;
        for (n, other) in got.iter().enumerate() {
            let above = row[n + 1];
            row[n + 1] = if octet == other {
                diagonal + 1
            } else {
                above.max(row[n])
            };
            diagonal = above;
        }
    }
    row[got.len()]
}

/// The peer's modem calls a line with the modem engine: they connect at
/// 300 bit/s and data goes both ways for `seconds`. Gives what the host
/// and the peer each sent and got, and the bench to finish.
struct ModemCall {
    bench: Bench,
    /// What the line's host wrote and the peer's modem read.
    up: (Vec<u8>, Vec<u8>),
    /// What the peer's modem wrote and the line's host read.
    down: (Vec<u8>, Vec<u8>),
}

fn modem_call(impairments: Impairments, octets: usize) -> ModemCall {
    modem_call_of(both_with_v8(), impairments, octets, "V.21")
}

/// The same with the peer's modem as `media` says, connecting at 300
/// bit/s with `carrier`.
fn modem_call_of(
    media: MediaKind,
    impairments: Impairments,
    octets: usize,
    carrier: &str,
) -> ModemCall {
    modem_call_at(media, impairments, octets, (300, carrier), 30)
}

/// The same connecting at a rate with a carrier, and carrying so many
/// octets a second.
fn modem_call_at(
    media: MediaKind,
    impairments: Impairments,
    octets: usize,
    (rate, carrier): (u32, &str),
    octets_a_second: u64,
) -> ModemCall {
    let mut bench = Bench::new("modem", 20, peer(media, 20, impairments));
    let uri = format!("sip:5551000@{}", bench.sip);
    bench.agent.invite(&uri, bench.sip);
    bench.until("the line rings", |bench| bench.seen.ends_with(b"RING\r\n"));
    bench.host().write(b"ATA\r");
    let mut peer_connected = None;
    bench.until("both modems are connected", |bench| {
        if let Some(engine) = bench.agent.engine() {
            while let Some(event) = engine.poll_event() {
                if let engine::ModemEvent::Connect(info) = event {
                    peer_connected = Some((info.rate, info.carrier));
                }
            }
        }
        peer_connected.is_some()
            && bench
                .seen
                .ends_with(format!("CONNECT {rate}\r\n").as_bytes())
    });
    assert_eq!(peer_connected, Some((rate, Some(carrier.to_string()))));
    let connect = bench.pool.line(1).unwrap().call.unwrap().connect.unwrap();
    assert_eq!(
        (connect.rate, connect.carrier.as_deref()),
        (rate, Some(carrier))
    );
    bench.seen.clear();

    let down: Vec<u8> = (0..octets as u32)
        .map(|n| (n * 7 + n / 251) as u8)
        .collect();
    let up: Vec<u8> = (0..octets as u32)
        .map(|n| (n * 13 + n / 127) as u8)
        .collect();
    bench.host().write(&up);
    let mut sent = 0;
    let mut got = Vec::new();
    // So many octets a second, and some seconds for the last of them.
    let length = Duration::from_millis(octets as u64 * 1000 / octets_a_second + 3000);
    let until = Instant::now() + length;
    while Instant::now() < until {
        bench.turn();
        let engine = bench.agent.engine().expect("the call has an engine");
        sent += engine.dte_write(&down[sent..]);
        let mut buffer = [0u8; 256];
        let len = engine.dte_read(&mut buffer);
        got.extend_from_slice(&buffer[..len]);
        std::thread::sleep(Duration::from_micros(200));
    }
    assert_eq!(sent, down.len(), "the peer's modem took all there was");
    // The call is still up: neither modem lost the other's carrier.
    assert_eq!(bench.pool.line(1).unwrap().state, LineState::Online);
    let seen = std::mem::take(&mut bench.seen);
    ModemCall {
        bench,
        up: (up, got),
        down: (down, seen),
    }
}

/// Over a clean path the peer's modem and the line's connect and every
/// character arrives, both ways.
#[test]
fn a_modem_peer_and_a_modem_line_carry_data_over_a_clean_path() {
    let call = modem_call(Impairments::default(), 300);
    assert!(call.up.0 == call.up.1, "what the host wrote the peer read");
    assert!(
        call.down.0 == call.down.1,
        "what the peer wrote the host read"
    );
    let (report, _) = call.bench.finish();
    assert!(report.is_clean(), "{report:?}");
}

/// The peer's clock is fast by 300 parts in a million, three times what a
/// telephone network allows a clock: the line's modem takes its time from
/// what arrives, and every character still arrives.
#[test]
fn a_modem_line_carries_data_from_a_peer_whose_clock_is_off() {
    let impairments = Impairments {
        clock_ppm: 300,
        ..Impairments::default()
    };
    let call = modem_call(impairments, 300);
    assert!(call.up.0 == call.up.1, "what the host wrote the peer read");
    assert!(
        call.down.0 == call.down.1,
        "what the peer wrote the host read"
    );
    let (report, _) = call.bench.finish();
    assert_eq!((report.slips, report.resync), (0, 0), "{report:?}");
}

/// One packet in a hundred of the peer's is lost. The modems connect all
/// the same and hold their carriers; a lost packet is 20 ms of the line,
/// six bits, and costs the characters it falls in and nothing after them.
#[test]
fn a_modem_line_holds_its_call_when_the_peer_loses_packets() {
    let impairments = Impairments {
        loss: 10,
        ..Impairments::default()
    };
    let call = modem_call(impairments, 300);
    let lost = call.bench.agent.report().unwrap().lost as usize;
    assert!(lost > 0, "the peer lost none of its packets");
    // A lost packet may damage the character it falls in and the one
    // after, in what the host is sent; what the peer is sent is late by
    // as much, which to its modem is a gap of the same length.
    for (which, (sent, got)) in [("up", &call.up), ("down", &call.down)] {
        let kept = in_order(sent, got);
        assert!(
            kept + 6 * lost >= sent.len() && kept * 10 >= sent.len() * 8,
            "{which}: {kept} of {} arrived in order with {lost} packets lost",
            sent.len()
        );
        assert!(got.len() <= sent.len() + 6 * lost, "{which}: {}", got.len());
    }
    let (report, _) = call.bench.finish();
    assert!(report.lost > 0, "{report:?}");
}

/// The peer's modem and the line's as they come: V.8 chooses V.22bis, the
/// link is LAPM with V.42bis, and every octet arrives, both ways.
#[test]
fn a_modem_peer_and_a_modem_line_connect_at_2400_with_error_control() {
    let media = MediaKind::modem(&engine::Modulation::ALL, true);
    let call = modem_call_at(media, Impairments::default(), 4000, (2400, "V.22bis"), 200);
    assert!(call.up.0 == call.up.1, "what the host wrote the peer read");
    assert!(
        call.down.0 == call.down.1,
        "what the peer wrote the host read"
    );
    let info = call.bench.pool.line(1).unwrap().call.unwrap();
    let connect = info.connect.unwrap();
    assert_eq!(connect.protocol.as_deref(), Some("LAPM"));
    assert_eq!(connect.compression.as_deref(), Some("V.42bis"));
    let (report, _) = call.bench.finish();
    assert!(report.is_clean(), "{report:?}");
}

/// One packet in a hundred of the peer's is lost, on a call at 2400 bit/s
/// with LAPM. A lost packet is 20 ms of the line and the frame it falls
/// in; LAPM has it sent again, and what each host is given is what the
/// other sent, with nothing missing and nothing wrong. The call holds.
#[test]
fn a_call_with_error_control_loses_no_data_when_the_peer_loses_packets() {
    let impairments = Impairments {
        loss: 10,
        ..Impairments::default()
    };
    let media = MediaKind::modem(&engine::Modulation::ALL, true);
    // A minute of it, at a third of what a clean line carries.
    let call = modem_call_at(media, impairments, 6000, (2400, "V.22bis"), 100);
    let lost = call.bench.agent.report().unwrap().lost;
    assert!(lost > 10, "the peer lost {lost} of its packets");
    assert!(call.up.0 == call.up.1, "what the host wrote the peer read");
    assert!(
        call.down.0 == call.down.1,
        "what the peer wrote the host read"
    );
    let stats = call.bench.pool.stats();
    assert_eq!(stats.retrains, 0, "{stats:?}");
    let (report, _) = call.bench.finish();
    assert!(report.lost > 10, "{report:?}");
}

/// A short call from the peer's modem, as `media` has it, that the line
/// answers: the data is whole both ways. Its capture is one of the corpus
/// when `RM_CORPUS` says where to keep it.
fn corpus_call(name: &'static str, media: MediaKind, carrier: &str) {
    let mut call = modem_call_of(media, Impairments::default(), 60, carrier);
    assert!(call.up.0 == call.up.1, "what the host wrote the peer read");
    assert!(
        call.down.0 == call.down.1,
        "what the peer wrote the host read"
    );
    call.bench.keep = Some(name);
    let (report, _) = call.bench.finish();
    assert!(report.is_clean(), "{report:?}");
}

/// Both modems have V.8, and it chooses V.21.
#[test]
fn a_modem_line_answers_a_caller_with_v8() {
    corpus_call("answer-v8-v21", both_with_v8(), "V.21");
}

/// The caller has V.21 and no V.8: the line's V.8 comes to nothing and it
/// offers V.21, which the caller takes.
#[test]
fn a_modem_line_answers_a_v21_caller_without_v8() {
    let media = MediaKind::modem(&[engine::Modulation::V21], false);
    corpus_call("answer-v21", media, "V.21");
}

/// The caller has Bell 103 and nothing else: the line falls back to it.
#[test]
fn a_modem_line_answers_a_bell_103_caller() {
    let media = MediaKind::modem(&[engine::Modulation::Bell103], false);
    corpus_call("answer-bell103", media, "BELL103");
}
