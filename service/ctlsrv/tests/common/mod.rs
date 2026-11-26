// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A whole service in one process: a pool on memory ports running on its
//! own thread, a control server over the in-memory transport, and a client.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use config::{Config, ConfigFile, LineConfig};
use ctlproto::{
    Call, ClientFrame, ErrorInfo, Event, Hello, LineInfo, LineRef, LineState, LogLevel, PROTOCOL,
    PortInfo, PortKind, ServerFrame, ServiceMode, decode_server, encode,
};
use ctlsrv::memory::{self, MemoryClient, MemoryConnector, PipeReader, PipeWriter};
use ctlsrv::{Hub, Limits, Server, ServerParts};
use engine::{NullConfig, NullEngine};
use msg::Message;
use pool::{
    EventSink, InlineRunner, MemoryProfiles, Now, NullSwitch, Pool, PoolParts, SerialFactory,
    command_channel,
};
use serde_json::Value;
use serial::{MemoryHost, SerialBackend, memory_pair};

/// The version the test service reports.
pub const VERSION: &str = "9.9.9-test";

/// The pipe name the test service reports.
pub const PIPE: &str = "test-pipe";

/// How long a client waits for the service before the test fails.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// The ports the test service says the machine has, in no particular
/// order: the two a fixture's first lines use, and two nothing uses.
pub fn test_ports() -> Vec<PortInfo> {
    let port =
        |name: &str, kind, pair: Option<&str>, host: Option<&str>, modem: Option<&str>| PortInfo {
            name: name.to_string(),
            kind,
            pair: pair.map(str::to_string),
            host_port: host.map(str::to_string),
            modem: modem.map(str::to_string),
            line: None,
        };
    vec![
        port("P2", PortKind::Com0com, Some("A2"), Some("COM12"), None),
        port("COM10", PortKind::Serial, None, None, None),
        port(
            "p1",
            PortKind::Com0com,
            Some("A1"),
            Some("COM11"),
            Some("Standard 56000 bps Modem"),
        ),
        port("COM3", PortKind::Serial, None, None, None),
    ]
}

type Hosts = Arc<Mutex<BTreeMap<String, MemoryHost>>>;

struct Factory(Hosts);

impl SerialFactory for Factory {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        let (backend, host) = memory_pair();
        self.0
            .lock()
            .unwrap()
            .insert(line.serial.port.clone(), host);
        Ok(Box::new(backend))
    }
}

struct HubSink(Hub);

impl EventSink for HubSink {
    fn publish(&mut self, event: Event) {
        self.0.publish(&event);
    }
}

pub struct Fixture {
    pub server: Server,
    pub hub: Hub,
    pub connector: MemoryConnector,
    pub hosts: Hosts,
    pub logs: Arc<Mutex<Vec<(LogLevel, String)>>>,
    /// Whether the service behind the server can put `[service]` settings
    /// into effect while it runs. It cannot, until a test says it can.
    pub live: Arc<AtomicBool>,
    pub directory: PathBuf,
    pub config_path: PathBuf,
    stop: Arc<AtomicBool>,
    pool_thread: Option<JoinHandle<()>>,
}

/// Waits until `check` is true, or fails the test.
pub fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

impl Fixture {
    /// A service with lines 1 to `lines` on ports `P1` to `P<lines>`, every
    /// port opened by its host, so that every line is idle.
    pub fn new(lines: u16) -> Fixture {
        Fixture::with_limits(lines, Limits::default())
    }

    pub fn with_limits(lines: u16, limits: Limits) -> Fixture {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "rustmodem-ctlsrv-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("rustmodem.toml");

        let config = Config {
            lines: (1..=lines)
                .map(|id| LineConfig::new(id, &format!("P{id}")))
                .collect(),
            ..Config::default()
        };
        let file = ConfigFile::from_config(&config).unwrap();
        file.save(&config_path).unwrap();

        let hub = Hub::new();
        let hosts: Hosts = Arc::default();
        let mut pool = Pool::new(PoolParts {
            router: Box::new(NullSwitch::new()),
            media: Box::new(InlineRunner::new()),
            serial: Box::new(Factory(Arc::clone(&hosts))),
            engines: Box::new(|_| Box::new(NullEngine::new(NullConfig::default()))),
            sink: Box::new(HubSink(hub.clone())),
            profiles: Box::new(MemoryProfiles::new()),
            call_log: None,
        });
        pool.apply_config(&config);

        let (handle, inbox) = command_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let pool_thread = std::thread::spawn(move || {
            let started = Instant::now();
            let now = || {
                let mono_ms = started.elapsed().as_millis() as u64;
                Now {
                    mono_ms,
                    unix_ms: 1_800_000_000_000 + mono_ms,
                }
            };
            while !stopping.load(Ordering::Relaxed) && inbox.serve(&mut pool) {
                pool.service(now());
                std::thread::sleep(Duration::from_millis(1));
            }
            pool.shutdown(now());
        });

        let logs: Arc<Mutex<Vec<(LogLevel, String)>>> = Arc::default();
        let live = Arc::new(AtomicBool::new(false));
        let live_settings = Arc::clone(&live);
        let log_to = Arc::clone(&logs);
        let server = Server::new(ServerParts {
            pool: handle,
            config: file,
            config_path: config_path.clone(),
            version: VERSION.to_string(),
            pipe: PIPE.to_string(),
            hub: hub.clone(),
            log: Arc::new(move |level, text| log_to.lock().unwrap().push((level, text))),
            limits,
            mode: ServiceMode::Console,
            clock: ctlsrv::system_clock(),
            ports: Arc::new(test_ports()),
            apply_service: Arc::new(move |_| Ok(live_settings.load(Ordering::Relaxed))),
        });

        let (listener, connector) = memory::listener();
        let serving = server.clone();
        std::thread::spawn(move || serving.serve(Box::new(listener)));

        let fixture = Fixture {
            server,
            hub,
            connector,
            hosts,
            logs,
            live,
            directory,
            config_path,
            stop,
            pool_thread: Some(pool_thread),
        };
        for id in 1..=lines {
            fixture.host(&format!("P{id}")).open();
        }
        if lines > 0 {
            let mut client = fixture.connect();
            eventually("every line to be idle", || {
                client.lines().iter().all(|l| l.state == LineState::Idle)
            });
        }
        fixture
    }

    /// The host end of a port, once the pool has opened it.
    pub fn host(&self, port: &str) -> MemoryHost {
        let mut found = None;
        eventually(&format!("port {port} to be opened"), || {
            found = self.hosts.lock().unwrap().get(port).cloned();
            found.is_some()
        });
        found.unwrap()
    }

    /// A client that has said hello.
    pub fn connect(&self) -> TestClient {
        let mut client = TestClient::new(self.connector.connect().unwrap());
        let frame = client.hello(PROTOCOL);
        assert_eq!(
            frame,
            Some(ServerFrame::Hello {
                hello: Hello {
                    protocol: PROTOCOL,
                    version: VERSION.to_string()
                }
            })
        );
        client
    }

    /// A client that has said nothing yet.
    pub fn connect_silent(&self) -> TestClient {
        TestClient::new(self.connector.connect().unwrap())
    }

    pub fn config_text(&self) -> String {
        std::fs::read_to_string(&self.config_path).unwrap()
    }

    pub fn logged(&self, start: &str) -> bool {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|(_, text)| text.starts_with(start))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.pool_thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub struct TestClient {
    pub reader: BufReader<PipeReader>,
    pub writer: PipeWriter,
    next_id: u64,
    /// Events that arrived while waiting for an answer.
    pub events: VecDeque<Event>,
}

impl TestClient {
    pub fn new(client: MemoryClient) -> TestClient {
        let MemoryClient { mut reader, writer } = client;
        reader.set_timeout(Some(PATIENCE));
        TestClient {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            events: VecDeque::new(),
        }
    }

    pub fn send_line(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.write_all(b"\n").unwrap();
    }

    /// The next frame from the service; `None` when it has closed the
    /// connection.
    pub fn read_frame(&mut self) -> Option<ServerFrame> {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .expect("the service to answer in time");
        if read == 0 {
            return None;
        }
        Some(decode_server(line.trim_end()).unwrap_or_else(|m| panic!("{m}: {line}")))
    }

    pub fn hello(&mut self, protocol: u32) -> Option<ServerFrame> {
        self.send_line(&encode(&ClientFrame::Hello {
            hello: Hello {
                protocol,
                version: "test-client".to_string(),
            },
        }));
        self.read_frame()
    }

    /// Sends a line and returns the next frame that is not an event.
    pub fn exchange(&mut self, line: &str) -> ServerFrame {
        self.send_line(line);
        self.next_reply()
    }

    pub fn next_reply(&mut self) -> ServerFrame {
        loop {
            match self
                .read_frame()
                .expect("an answer, not a closed connection")
            {
                ServerFrame::Event(event) => self.events.push_back(event),
                other => return other,
            }
        }
    }

    /// Makes a call and returns its result or its error.
    pub fn call(&mut self, call: Call) -> Result<Value, ErrorInfo> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = self.exchange(&encode(&ClientFrame::Request(call.request(id))));
        match frame {
            ServerFrame::Result { id: got, result } => {
                assert_eq!(got, id, "the answer carries the request's id");
                Ok(result)
            }
            ServerFrame::Error { id: got, error } => {
                assert_eq!(got, Some(id), "the error carries the request's id");
                Err(error)
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }

    /// Makes a call that must succeed and reads its result as `T`.
    pub fn ok<T: serde::de::DeserializeOwned>(&mut self, call: Call) -> T {
        let method = call.method();
        let result = self
            .call(call)
            .unwrap_or_else(|error| panic!("{method} failed: {error}"));
        serde_json::from_value(result).unwrap_or_else(|e| panic!("{method} result: {e}"))
    }

    /// Makes a call that must fail and returns the message id.
    pub fn err(&mut self, call: Call) -> String {
        let method = call.method();
        match self.call(call) {
            Ok(result) => panic!("{method} should have failed, got {result}"),
            Err(error) => {
                assert!(!error.text.is_empty() && !error.cause.is_empty());
                assert!(!error.recovery.is_empty());
                error.msgid
            }
        }
    }

    pub fn lines(&mut self) -> Vec<LineInfo> {
        self.ok::<ctlproto::LineList>(Call::LinesList).lines
    }

    pub fn line(&mut self, id: u16) -> LineInfo {
        self.ok(Call::LinesGet(LineRef { id }))
    }

    /// Waits until a line is in a state.
    pub fn wait_for_state(&mut self, id: u16, state: LineState) {
        eventually(&format!("line {id} to be {}", state.as_str()), || {
            self.line(id).state == state
        });
    }

    /// The next event, waiting for it if none has arrived.
    pub fn next_event(&mut self) -> Event {
        if let Some(event) = self.events.pop_front() {
            return event;
        }
        match self
            .read_frame()
            .expect("an event, not a closed connection")
        {
            ServerFrame::Event(event) => event,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    /// Reads until `count` events are held, keeping them all.
    pub fn collect_events(&mut self, count: usize) {
        while self.events.len() < count {
            match self
                .read_frame()
                .expect("an event, not a closed connection")
            {
                ServerFrame::Event(event) => self.events.push_back(event),
                other => panic!("expected an event, got {other:?}"),
            }
        }
    }

    /// The next event that `wanted` accepts; the ones before it are
    /// discarded.
    pub fn wait_for_event(&mut self, mut wanted: impl FnMut(&Event) -> bool) -> Event {
        loop {
            let event = self.next_event();
            if wanted(&event) {
                return event;
            }
        }
    }
}
