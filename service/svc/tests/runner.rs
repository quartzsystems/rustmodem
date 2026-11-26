// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The whole runner at once: the pool on its task, a thread for each
//! port, the control server on the runtime, and with SIP the socket's
//! task and the media threads. Two things are asked of it here that no
//! part can answer for alone: that a control client that does not read
//! holds up nobody, the pool least of all, and that the stop signal ends
//! every task and thread in the time a stop is given, with calls up and
//! lines registered.

mod support;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use config::{ConfigFile, LineConfig};
use ctlproto::{
    Call, ClientFrame, Event, EventSelection, Hello, LineState, LogLevel, PROTOCOL, ServerFrame,
    ServiceMode, decode_server, encode,
};
use ctlsrv::memory::{self, BlockingReader, BlockingWriter, MemoryConnector};
use ctlsrv::{Hub, Limits, PoolLink, Server, ServerParts};
use media::ThreadedRunner;
use msg::Message;
use pool::{
    CallRouter, EventSink, InlineRunner, MediaRunner, MemoryProfiles, NullSwitch, Pool, PoolParts,
    SerialFactory, engine_for,
};
use serial::{MemoryHost, SerialBackend, memory_pair};
use sippeer::{Agent, AgentConfig, MediaKind, PeerEvent, loopback};
use support::{runtime, sip_socket};
use svc::bridge::BridgedPorts;
use svc::pooltask::{LINGER, PoolClock, PoolTask, system_unix_clock};
use svc::shutdown::Shutdown;
use telephony::{RouterParts, Settings, SipRouter, TelephonyHandle, Unobserved, system_random};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// How long a stop may take. The service tells the service control
/// manager to allow thirty seconds; a stop that is well takes the three
/// the router is given to hear its answers, and little more.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

type Hosts = Arc<Mutex<BTreeMap<u16, MemoryHost>>>;

struct Ports(Hosts);

impl SerialFactory for Ports {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        let (backend, host) = memory_pair();
        self.0.lock().unwrap().insert(line.id, host);
        Ok(Box::new(backend))
    }
}

struct HubSink(Hub);

impl EventSink for HubSink {
    fn publish(&mut self, event: Event) {
        self.0.publish(&event);
    }
}

/// Waits until `check` is true, or fails the test.
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A service on the runner, with its control server on the in-memory
/// transport.
struct Service {
    link: PoolLink,
    hub: Hub,
    connector: MemoryConnector,
    hosts: BTreeMap<u16, MemoryHost>,
    logs: Arc<Mutex<Vec<String>>>,
    shutdown: Shutdown,
    pool: JoinHandle<()>,
    control: JoinHandle<std::io::Result<()>>,
    /// Where the SIP listener is, if there is one.
    sip: Option<SocketAddrV4>,
    directory: PathBuf,
}

/// The SIP side of a service: where its listener is, and the router and
/// the media runner on it.
struct Sip {
    listen: SocketAddrV4,
    router: Box<dyn CallRouter>,
    media: Box<dyn MediaRunner>,
    media_events: svc::mediawake::MediaEvents,
}

impl Service {
    /// A service with the lines of `text`, which is a configuration with
    /// `LISTEN` where the SIP listener's address goes, if it has SIP.
    fn start(text: &str) -> Service {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "rustmodem-runner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("rustmodem.toml");

        let wake = Arc::new(Notify::new());
        // With SIP the listener is bound first: the configuration says
        // where it is.
        let socket = text.contains("LISTEN").then(|| {
            let wake = Arc::clone(&wake);
            sip_socket(
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
                Arc::new(move || wake.notify_one()),
            )
            .unwrap()
        });
        let text = match &socket {
            Some(socket) => text.replace("LISTEN", &socket.local().to_string()),
            None => text.to_string(),
        };
        let file = ConfigFile::parse(&text).unwrap_or_else(|problems| panic!("{problems:?}"));
        file.save(&config_path).unwrap();
        let config = file.config().clone();
        let listen = socket.as_ref().map(|socket| socket.local());
        let sip = socket.map(|socket| {
            let (media_waker, media_events) = svc::mediawake::channel();
            Sip {
                listen: socket.local(),
                router: Box::new(SipRouter::new(RouterParts {
                    settings: Settings::new(&config, "9.9.9-test"),
                    transport: Box::new(socket),
                    random: system_random(),
                    observer: Box::new(Unobserved),
                    handle: TelephonyHandle::new(),
                    version: "9.9.9-test".to_string(),
                })),
                media: Box::new(ThreadedRunner::new(config.lines.len(), media_waker).unwrap()),
                media_events,
            }
        });

        let hub = Hub::new();
        let hosts = Hosts::default();
        let (router, media, media_events): (Box<dyn CallRouter>, Box<dyn MediaRunner>, _) =
            match sip {
                Some(sip) => {
                    assert_eq!(sip.listen, config.sip.listen);
                    (sip.router, sip.media, Some(sip.media_events))
                }
                None => (
                    Box::new(NullSwitch::new()),
                    Box::new(InlineRunner::new()),
                    None,
                ),
            };
        let mut pool = Pool::new(PoolParts {
            router,
            media,
            serial: Box::new(BridgedPorts::new(
                Box::new(Ports(Arc::clone(&hosts))),
                Arc::clone(&wake),
            )),
            engines: Box::new(engine_for),
            sink: Box::new(HubSink(hub.clone())),
            profiles: Box::new(MemoryProfiles::new()),
            call_log: None,
        });
        pool.apply_config(&config);

        let (link, requests) = ctlsrv::pool_link();
        let (shutdown, stopping) = svc::shutdown::channel();
        let control_stop = stopping.receiver();
        let pool = runtime().spawn(
            PoolTask {
                pool,
                requests,
                wake,
                media_events,
                clock: PoolClock::Running(system_unix_clock()),
                linger: LINGER,
            }
            .run(stopping),
        );

        let logs: Arc<Mutex<Vec<String>>> = Arc::default();
        let log_to = Arc::clone(&logs);
        let lines = config.lines.len();
        let server = Server::new(ServerParts {
            pool: link.clone(),
            config: file,
            config_path,
            version: "9.9.9-test".to_string(),
            pipe: "test-pipe".to_string(),
            hub: hub.clone(),
            log: Arc::new(move |_: LogLevel, text| log_to.lock().unwrap().push(text)),
            limits: Limits::default(),
            mode: ServiceMode::Console,
            ports: Arc::new(Vec::new()),
            clock: ctlsrv::system_clock(),
            apply_service: ctlsrv::restart_to_apply(),
            cpu: ctlsrv::no_cpu_meter(),
            capture_dir: None,
        });
        let (mut listener, connector) = memory::listener();
        let control =
            runtime().spawn(async move { server.serve(&mut listener, control_stop).await });

        // The hosts open their ports and tell their modems to say nothing
        // they are not asked for.
        eventually("every port to be opened", || {
            hosts.lock().unwrap().len() == lines
        });
        let hosts: BTreeMap<u16, MemoryHost> = hosts.lock().unwrap().clone();
        for host in hosts.values() {
            host.open();
        }
        let service = Service {
            link,
            hub,
            connector,
            hosts,
            logs,
            shutdown,
            pool,
            control,
            sip: listen,
            directory,
        };
        eventually("every line to be idle", || {
            service
                .states()
                .values()
                .all(|state| *state == LineState::Idle)
        });
        service
    }

    fn states(&self) -> BTreeMap<u16, LineState> {
        self.link
            .blocking_lines()
            .unwrap()
            .iter()
            .map(|line| (line.id, line.state))
            .collect()
    }

    fn logged(&self, start: &str) -> bool {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with(start))
    }

    /// A client that has said hello, reading and writing as a program
    /// does.
    fn connect(&self, capacity: usize) -> (BufReader<BlockingReader>, BlockingWriter) {
        let client = self.connector.connect_with_capacity(capacity).unwrap();
        let (mut reader, mut writer) = memory::blocking(client, runtime().handle());
        reader.set_timeout(Some(Duration::from_secs(10)));
        let mut reader = BufReader::new(reader);
        let hello = encode(&ClientFrame::Hello {
            hello: Hello {
                protocol: PROTOCOL,
                version: "test-client".to_string(),
            },
        });
        writer.write_all(format!("{hello}\n").as_bytes()).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(matches!(
            decode_server(line.trim_end()).unwrap(),
            ServerFrame::Hello { .. }
        ));
        (reader, writer)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown.trigger();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Subscribes a client to every event and reads the answer.
fn subscribe(reader: &mut BufReader<BlockingReader>, writer: &mut BlockingWriter) {
    let request = encode(&ClientFrame::Request(
        Call::Subscribe(EventSelection::default()).request(1),
    ));
    writer.write_all(format!("{request}\n").as_bytes()).unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(matches!(
        decode_server(line.trim_end()).unwrap(),
        ServerFrame::Result { id: 1, .. }
    ));
}

const FOUR_LINES: &str = r#"
[[line]]
id = 1
serial = { backend = "com0com", port = "P1" }

[[line]]
id = 2
serial = { backend = "com0com", port = "P2" }

[[line]]
id = 3
serial = { backend = "com0com", port = "P3" }

[[line]]
id = 4
serial = { backend = "com0com", port = "P4" }
"#;

#[test]
fn a_slow_control_client_cannot_delay_the_pool_task() {
    let service = Service::start(FOUR_LINES);
    // A client on a connection so small that the service can hardly write
    // to it, subscribed to everything and reading nothing.
    let (mut slow_reader, mut slow_writer) = service.connect(200);
    subscribe(&mut slow_reader, &mut slow_writer);
    assert_eq!(service.hub.client_count(), 1);

    // The pool publishes far more than the client's queue holds: every
    // change of the bindings is a state event for each line. Each change
    // is asked of the pool's task and answered by it, and none of them
    // waits for the client.
    const ROUNDS: u32 = 3000;
    let mut longest = Duration::ZERO;
    let started = Instant::now();
    for round in 0..ROUNDS {
        let bindings = (1..=4)
            .map(|line| (format!("P{line}"), format!("COM{round}")))
            .collect();
        let asked = Instant::now();
        service
            .link
            .blocking_with(move |pool| pool.set_bindings(bindings))
            .unwrap();
        longest = longest.max(asked.elapsed());
    }
    assert!(
        longest < Duration::from_millis(250),
        "the pool's task took {longest:?} over one request"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{ROUNDS} requests took {:?}",
        started.elapsed()
    );

    // The lines are served meanwhile as they always are.
    let asked = Instant::now();
    let host = &service.hosts[&1];
    host.write(b"ATE0\r");
    let mut said = Vec::new();
    eventually("line 1 to answer its host", || {
        said.extend(host.read());
        said.ends_with(b"\r\nOK\r\n")
    });
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "{:?}",
        asked.elapsed()
    );

    // The client that fell behind was cut off, not waited for.
    eventually("the slow client to be disconnected", || {
        service.logged("RMD2006")
    });
    eventually("the slow client to be forgotten", || {
        service.hub.client_count() == 0
    });
    // What it reads now is what was written before it was cut off, and
    // then the end.
    slow_reader
        .get_mut()
        .set_timeout(Some(Duration::from_secs(10)));
    let mut rest = Vec::new();
    slow_reader.read_to_end(&mut rest).unwrap();
}

/// The far end of the calls: a peer that is the trunk, the registrar and
/// a caller, stepped on a thread of its own, with what it saw.
struct FarEnd {
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FarEnd {
    /// Starts the peer, which calls `number` at `service` when told to.
    fn start(mut agent: Agent, call: Arc<Mutex<Option<(String, SocketAddrV4)>>>) -> FarEnd {
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (saw, stopping) = (Arc::clone(&seen), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                if let Some((uri, to)) = call.lock().unwrap().take() {
                    agent.invite(&uri, to);
                }
                let busy = agent.step().unwrap_or(false);
                for event in agent.events() {
                    let said = match event {
                        PeerEvent::Answered { .. } => "answered".to_string(),
                        PeerEvent::HungUp { .. } => "hung up".to_string(),
                        PeerEvent::Registered { request, status } => format!(
                            "registered {status} for {}",
                            request.headers.expires().unwrap_or(u32::MAX)
                        ),
                        _ => continue,
                    };
                    saw.lock().unwrap().push(said);
                }
                if !busy {
                    std::thread::sleep(Duration::from_micros(300));
                }
            }
        });
        FarEnd {
            seen,
            stop,
            thread: Some(thread),
        }
    }

    fn saw(&self, what: impl Fn(&str) -> bool) -> bool {
        self.seen.lock().unwrap().iter().any(|said| what(said))
    }
}

impl Drop for FarEnd {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn the_stop_signal_ends_every_task_and_thread_with_calls_up_and_lines_registered() {
    let agent = Agent::bind(AgentConfig {
        media: MediaKind::Pattern,
        seed: 0x5709,
        ..AgentConfig::new("pbx", loopback())
    })
    .unwrap();
    let peer = agent.sip_address();
    // A stretch of ports the other test programs do not use.
    let first = 52_000 + (std::process::id() % 50) as u16 * 20;
    let text = format!(
        r#"
[sip]
enabled = true
listen = "LISTEN"
trusted_peers = ["127.0.0.1"]
outbound_peer = "{peer}"

[media]
rtp_ports = "{first}-{}"

[[line]]
id = 1
serial = {{ backend = "com0com", port = "P1" }}
engine = "echo"
auto_answer_rings = 1

[[line]]
id = 2
serial = {{ backend = "com0com", port = "P2" }}
register = {{ server = "{peer}", user = "202", password = "secret" }}

[[route]]
called = "5550101"
target = "line:1"
"#,
        first + 19
    );
    let service = Service::start(&text);
    let listen = service.sip.expect("the service has SIP");
    let call = Arc::new(Mutex::new(None));
    let far_end = FarEnd::start(agent, Arc::clone(&call));

    // A line registered, a call up, and a control client watching.
    eventually("line 2 to register", || {
        far_end.saw(|said| said.starts_with("registered 200 for ") && !said.ends_with(" 0"))
    });
    *call.lock().unwrap() = Some((format!("sip:5550101@{listen}"), listen));
    eventually("the call to be answered", || {
        far_end.saw(|said| said == "answered")
    });
    eventually("line 1 to be online", || {
        service.states()[&1] == LineState::Online
    });
    let (mut reader, mut writer) = service.connect(memory::DEFAULT_CAPACITY);
    subscribe(&mut reader, &mut writer);
    assert_eq!(service.hub.client_count(), 1);

    // The stop signal.
    let stopping_at = Instant::now();
    service.shutdown.trigger();

    // The pool's task ends: by then every call is hung up, every port is
    // released and every registration given up, and the media threads
    // have ended, for the runner that owns them is dropped with the pool
    // and waits for each.
    let pool = runtime()
        .block_on(async { tokio::time::timeout(STOP_TIMEOUT, wait_for(&service.pool)).await });
    assert!(pool.is_ok(), "the pool's task did not end");
    let control = runtime()
        .block_on(async { tokio::time::timeout(STOP_TIMEOUT, wait_for(&service.control)).await });
    assert!(control.is_ok(), "the control server did not stop accepting");
    assert!(
        stopping_at.elapsed() < STOP_TIMEOUT,
        "the stop took {:?}",
        stopping_at.elapsed()
    );

    // The far end was told: a BYE for the call, and the registration
    // removed.
    assert!(
        far_end.saw(|said| said == "hung up"),
        "the call got its BYE"
    );
    assert!(
        far_end.saw(|said| said == "registered 200 for 0"),
        "the registration was removed"
    );
    // The ports' threads have ended and let their ports go.
    for (line, host) in &service.hosts {
        assert!(!host.backend_alive(), "port {line} was released");
    }
    // The client's tasks have ended and its connection is closed: it
    // reads what it was sent and then the end.
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).unwrap();
    eventually("the client to be forgotten", || {
        service.hub.client_count() == 0
    });
    // The task of the SIP socket has ended and closed it: its address can
    // be bound again.
    eventually("the SIP socket to be closed", || {
        std::net::UdpSocket::bind(listen).is_ok()
    });
}

/// Waits for a task to finish without taking its handle.
async fn wait_for<T>(task: &JoinHandle<T>) {
    while !task.is_finished() {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
