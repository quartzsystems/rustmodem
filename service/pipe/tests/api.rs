// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

#![cfg(windows)]

//! The control API over a real named pipe: the same round trips the server
//! is tested with over its in-memory transport, the hello exchange, and
//! what the pipe itself has to get right.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use config::{Config, ConfigFile, LineConfig};
use ctlproto::{
    Call, CallRecord, ClientFrame, Direction, EndReason, Event as ControlEvent, Hello, LineState,
    LogLevel, PROTOCOL, PortInfo, PortKind, ServerFrame, ServiceMode, decode_server, doc, encode,
};
use ctlsrv::{Hub, Limits, Listener, Server, ServerParts};
use engine::{NullConfig, NullEngine};
use msg::Message;
use pipe::{Event, PipeClient as OverlappedClient, PipeListener, PipeSecurity};
use pool::{
    EventSink, InlineRunner, MemoryProfiles, Now, NullSwitch, Pool, PoolParts, SerialFactory,
    command_channel,
};
use serial::{MemoryHost, SerialBackend, memory_pair};

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
    fn publish(&mut self, event: ControlEvent) {
        self.0.publish(&event);
    }
}

/// A pipe name no other test or process is using.
fn unique_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "rustmodem-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A service on memory ports with its control API on a real pipe.
struct Fixture {
    pipe: String,
    hub: Hub,
    logs: Arc<Mutex<Vec<String>>>,
    directory: PathBuf,
    stop: Arc<Event>,
    stop_pool: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Fixture {
    fn new(lines: u16, limits: Limits) -> Fixture {
        let pipe = unique_name();
        let directory = std::env::temp_dir().join(&pipe);
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
        let stop_pool = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop_pool);
        let pool_thread = std::thread::spawn(move || {
            let started = Instant::now();
            while !stopping.load(Ordering::Relaxed) && inbox.serve(&mut pool) {
                let mono_ms = started.elapsed().as_millis() as u64;
                pool.service(Now {
                    mono_ms,
                    unix_ms: 1_800_000_000_000 + mono_ms,
                });
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let logs: Arc<Mutex<Vec<String>>> = Arc::default();
        let log_to = Arc::clone(&logs);
        let server = Server::new(ServerParts {
            pool: handle,
            config: file,
            config_path,
            version: "9.9.9-test".to_string(),
            pipe: pipe.clone(),
            hub: hub.clone(),
            log: Arc::new(move |_: LogLevel, text| log_to.lock().unwrap().push(text)),
            limits,
            mode: ServiceMode::Console,
            clock: ctlsrv::system_clock(),
            apply_service: ctlsrv::restart_to_apply(),
            ports: Arc::new(vec![PortInfo {
                name: "P1".to_string(),
                kind: PortKind::Com0com,
                pair: Some("A1".to_string()),
                host_port: Some("COM11".to_string()),
                modem: None,
                line: None,
            }]),
        });

        let stop = Arc::new(Event::new().unwrap());
        let mut listener =
            PipeListener::bind(&pipe, PipeSecurity::Creator, Arc::clone(&stop)).unwrap();
        assert_eq!(listener.name(), pipe);
        let accept_thread = std::thread::spawn(move || {
            while let Some(connection) = listener.accept().unwrap() {
                let server = server.clone();
                std::thread::spawn(move || server.handle(connection));
            }
        });

        // The hosts open their ports, so the lines are idle.
        for id in 1..=lines {
            let port = format!("P{id}");
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(host) = hosts.lock().unwrap().get(&port) {
                    host.open();
                    break;
                }
                assert!(Instant::now() < deadline, "port {port} was never opened");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        Fixture {
            pipe,
            hub,
            logs,
            directory,
            stop,
            stop_pool,
            threads: vec![pool_thread, accept_thread],
        }
    }

    fn connect(&self) -> PipeClient {
        let mut client = PipeClient::open(&self.pipe).unwrap();
        let hello = client.hello(PROTOCOL);
        assert!(matches!(hello, Some(ServerFrame::Hello { hello }) if hello.protocol == PROTOCOL));
        client
    }

    fn logged(&self, start: &str) -> bool {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with(start))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.set();
        self.stop_pool.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// A client on the pipe, as `rustmodemctl` is: the pipe opened as a file.
struct PipeClient {
    reader: BufReader<File>,
    writer: File,
}

impl PipeClient {
    fn open(pipe: &str) -> std::io::Result<PipeClient> {
        let path = format!(r"\\.\pipe\{pipe}");
        let deadline = Instant::now() + Duration::from_secs(10);
        let file = loop {
            match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => break file,
                // Every instance is taken for a moment between one client
                // connecting and the next instance being made.
                Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(e),
            }
        };
        Ok(PipeClient {
            reader: BufReader::new(file.try_clone()?),
            writer: file,
        })
    }

    fn send(&mut self, line: &str) {
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
    }

    fn read(&mut self) -> Option<ServerFrame> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) if !line.ends_with('\n') => None,
            Ok(_) => Some(decode_server(line.trim_end()).unwrap()),
            // The service closing its end is the end of the stream: the
            // pipe is broken (109), or was disconnected (233).
            Err(e) if matches!(e.raw_os_error(), Some(109 | 233)) => None,
            Err(e) => panic!("reading the pipe: {e}"),
        }
    }

    fn hello(&mut self, protocol: u32) -> Option<ServerFrame> {
        self.send(&encode(&ClientFrame::Hello {
            hello: Hello {
                protocol,
                version: "pipe-test".to_string(),
            },
        }));
        self.read()
    }

    /// Sends a request and returns its answer; events on the way are
    /// collected.
    fn request(&mut self, line: &str, events: &mut Vec<ControlEvent>) -> ServerFrame {
        self.send(line);
        loop {
            match self.read().expect("an answer") {
                ServerFrame::Event(event) => events.push(event),
                other => return other,
            }
        }
    }
}

/// Every method in docs/CONTROL_API.md, with the example the documentation
/// gives for it, over a real pipe.
#[test]
fn every_method_round_trips_over_a_real_pipe() {
    let fixture = Fixture::new(2, Limits::default());
    let mut client = fixture.connect();
    let mut events = Vec::new();

    // What each documented example gets on a fresh service with two idle
    // lines, when the examples are sent in the documented order.
    let expected: BTreeMap<&str, Result<(), &str>> = BTreeMap::from([
        ("service.info", Ok(())),
        ("lines.list", Ok(())),
        ("lines.get", Ok(())),
        ("lines.answer", Err("RMD1003")),
        ("lines.hangup", Err("RMD1004")),
        ("lines.reset", Ok(())),
        ("lines.enable", Ok(())),
        ("lines.disable", Ok(())),
        ("lines.add", Ok(())),
        ("lines.edit", Ok(())),
        ("lines.remove", Ok(())),
        ("calls.list", Ok(())),
        ("config.reload", Ok(())),
        ("config.get", Ok(())),
        ("config.set", Ok(())),
        ("stats.get", Ok(())),
        ("log.tail", Ok(())),
        ("ports.list", Ok(())),
        ("subscribe", Ok(())),
        ("unsubscribe", Ok(())),
    ]);
    assert_eq!(expected.len(), Call::METHODS.len());

    for (index, method) in doc::METHODS.iter().enumerate() {
        let id = index as u64 + 100;
        let request = format!(
            r#"{{"id":{id},"method":"{}","params":{}}}"#,
            method.name, method.example
        );
        let answer = client.request(&request, &mut events);
        match (answer, expected[method.name]) {
            (ServerFrame::Result { id: got, result }, Ok(())) => {
                assert_eq!(got, id, "{}", method.name);
                assert!(result.is_object(), "{}", method.name);
            }
            (ServerFrame::Error { id: got, error }, Err(want)) => {
                assert_eq!(got, Some(id), "{}", method.name);
                assert_eq!(error.msgid, want, "{}", method.name);
            }
            (answer, want) => panic!("{}: got {answer:?}, expected {want:?}", method.name),
        }
    }

    // And the results mean something: line 1 was disabled by its example.
    let answer = client.request(
        r#"{"id":1,"method":"lines.get","params":{"id":1}}"#,
        &mut events,
    );
    let ServerFrame::Result { result, .. } = answer else {
        panic!("lines.get failed");
    };
    assert_eq!(result["state"], "disabled");
    assert_eq!(result["enabled"], false);
    // config.reload looked the ports up, so the line shows its binding.
    assert_eq!(result["binding"], "COM11");
    assert_eq!(result["counters"]["at_commands"], 0);

    // The additions of phase 1 carry what they should over the pipe.
    let mut ask = |method: &str| {
        let request = format!(r#"{{"id":2,"method":"{method}"}}"#);
        match client.request(&request, &mut events) {
            ServerFrame::Result { result, .. } => result,
            other => panic!("{method}: {other:?}"),
        }
    };
    let info = ask("service.info");
    assert_eq!(info["mode"], "console");
    assert!(info["config_loaded_ms"].as_u64().unwrap() >= info["started_ms"].as_u64().unwrap());
    // config.set's example asked for the debug level, which is not what
    // the service started with.
    let settings = ask("config.get");
    assert_eq!(settings["service"]["log_level"], "debug");
    assert_eq!(settings["control"]["pipe"], "rustmodem-control");
    assert!(
        settings["path"]
            .as_str()
            .unwrap()
            .ends_with("rustmodem.toml")
    );
    let stats = ask("stats.get");
    assert_eq!(stats["calls_answered"], 0);
    assert!(stats["connects"].as_array().unwrap().is_empty());
    let tail = ask("log.tail");
    assert!(!tail["lines"].as_array().unwrap().is_empty());
    let ports = ask("ports.list");
    assert_eq!(ports["ports"][0]["name"], "P1");
    assert_eq!(ports["ports"][0]["line"], 1);
    assert_eq!(ports["ports"][0]["kind"], "com0com");
}

#[test]
fn events_arrive_over_a_real_pipe() {
    let fixture = Fixture::new(1, Limits::default());
    let mut client = fixture.connect();
    let mut events = Vec::new();
    let subscribed = r#"{"id":1,"method":"subscribe","params":{"events":["line.state"]}}"#;
    assert!(matches!(
        client.request(subscribed, &mut events),
        ServerFrame::Result { id: 1, .. }
    ));
    let disable = r#"{"id":2,"method":"lines.disable","params":{"id":1}}"#;
    assert!(matches!(
        client.request(disable, &mut events),
        ServerFrame::Result { id: 2, .. }
    ));
    // The event may come before the answer or after it.
    while !events
        .iter()
        .any(|e| matches!(e, ControlEvent::LineState(l) if l.state == LineState::Disabled))
    {
        match client.read().expect("the line.state event") {
            ServerFrame::Event(event) => events.push(event),
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn a_client_of_another_protocol_is_refused_over_a_real_pipe() {
    let fixture = Fixture::new(0, Limits::default());
    let mut client = PipeClient::open(&fixture.pipe).unwrap();
    let Some(ServerFrame::Error { id: None, error }) = client.hello(PROTOCOL + 5) else {
        panic!("expected an error");
    };
    assert_eq!(error.msgid, "RMD2001");
    assert_eq!(client.read(), None, "the service closed the pipe");

    // The pipe goes on serving other clients.
    let mut events = Vec::new();
    let mut other = fixture.connect();
    assert!(matches!(
        other.request(r#"{"id":1,"method":"lines.list"}"#, &mut events),
        ServerFrame::Result { id: 1, .. }
    ));
}

#[test]
fn several_clients_are_served_at_once() {
    let fixture = Fixture::new(1, Limits::default());
    let mut clients: Vec<PipeClient> = (0..5).map(|_| fixture.connect()).collect();
    let mut events = Vec::new();
    for round in 0..3 {
        for (number, client) in clients.iter_mut().enumerate() {
            let id = round * 10 + number as u64;
            let request = format!(r#"{{"id":{id},"method":"service.info"}}"#);
            let ServerFrame::Result { id: got, result } = client.request(&request, &mut events)
            else {
                panic!("service.info failed");
            };
            assert_eq!(got, id);
            assert_eq!(result["pipe"], fixture.pipe.as_str());
        }
    }
    assert_eq!(fixture.hub.client_count(), 5);
    drop(clients);
    let deadline = Instant::now() + Duration::from_secs(10);
    while fixture.hub.client_count() != 0 {
        assert!(
            Instant::now() < deadline,
            "the clients were never forgotten"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_client_that_stops_reading_is_disconnected_and_holds_nobody_up() {
    let limits = Limits {
        max_queued: 50,
        max_logs: 10,
    };
    let fixture = Fixture::new(0, limits);
    let mut client = fixture.connect();
    let mut events = Vec::new();
    assert!(matches!(
        client.request(r#"{"id":1,"method":"subscribe"}"#, &mut events),
        ServerFrame::Result { id: 1, .. }
    ));

    // Far more state events than the pipe's buffers and the client's queue
    // hold, while the client reads nothing. Publishing is what the pool's
    // thread does; it must not wait.
    let started = Instant::now();
    for number in 0..20_000 {
        fixture.hub.publish(&ControlEvent::CallEnded(CallRecord {
            id: number,
            line: 1,
            direction: Direction::Outgoing,
            number: "2".to_string(),
            started_ms: 0,
            connected_ms: None,
            ended_ms: 0,
            reason: EndReason::Busy,
            connect: None,
            bytes_tx: 0,
            bytes_rx: 0,
        }));
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "publishing took {:?}",
        started.elapsed()
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.logged("RMD2006") || fixture.hub.client_count() != 0 {
        assert!(
            Instant::now() < deadline,
            "the slow client was never disconnected"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // What the client reads now ends, well short of everything published.
    let mut delivered = 0;
    while client.read().is_some() {
        delivered += 1;
    }
    assert!(delivered < 20_000);
}

#[test]
fn a_pipe_name_already_in_use_is_message_2007() {
    let fixture = Fixture::new(0, Limits::default());
    let stop = Arc::new(Event::new().unwrap());
    let Err(problem) = PipeListener::bind(&fixture.pipe, PipeSecurity::Creator, stop) else {
        panic!("a second listener on the same pipe must be refused");
    };
    assert_eq!(problem.id, "RMD2007");
    assert!(problem.text.contains(&fixture.pipe));
}

#[test]
fn the_stop_signal_ends_a_wait_for_a_client() {
    let stop = Arc::new(Event::new().unwrap());
    let mut listener =
        PipeListener::bind(&unique_name(), PipeSecurity::Creator, Arc::clone(&stop)).unwrap();
    let waiting = std::thread::spawn(move || listener.accept().map(|c| c.is_none()));
    std::thread::sleep(Duration::from_millis(100));
    assert!(!waiting.is_finished(), "accept waits for a client");
    stop.set();
    assert!(
        waiting.join().unwrap().unwrap(),
        "accept reports the shutdown"
    );
}

#[test]
fn the_service_pipe_admits_only_system_and_administrators() {
    let name = unique_name();
    let stop = Arc::new(Event::new().unwrap());
    let mut listener =
        PipeListener::bind(&name, PipeSecurity::AdministratorsOnly, Arc::clone(&stop)).unwrap();
    let accepting = std::thread::spawn(move || listener.accept().map(|c| c.is_some()));

    // Whether this test may open the pipe depends on whether it runs
    // elevated. Either it is let in, or it is refused for lack of access;
    // anything else is wrong.
    match PipeClient::open(&name) {
        Ok(_client) => assert!(accepting.join().unwrap().unwrap()),
        Err(error) => {
            assert_eq!(error.kind(), ErrorKind::PermissionDenied, "{error}");
            stop.set();
            assert!(!accepting.join().unwrap().unwrap());
        }
    }
}

/// The client the manager uses: it reads on one thread while it writes on
/// another, which a pipe opened as an ordinary file cannot do.
#[test]
fn a_client_reads_on_one_thread_while_it_writes_on_another() {
    let fixture = Fixture::new(1, Limits::default());
    let halves = OverlappedClient::connect(&fixture.pipe, Duration::from_secs(5))
        .unwrap()
        .split();
    let mut writer = halves.writer;
    let (frames, received) = std::sync::mpsc::channel();
    let reading = std::thread::spawn(move || {
        let mut reader = BufReader::new(halves.reader);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if frames
                        .send(decode_server(line.trim_end()).unwrap())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    let mut send = |line: &str| writer.write_all(format!("{line}\n").as_bytes()).unwrap();
    let next = || received.recv_timeout(Duration::from_secs(10)).unwrap();

    send(&encode(&ClientFrame::Hello {
        hello: Hello {
            protocol: PROTOCOL,
            version: "overlapped".to_string(),
        },
    }));
    assert!(matches!(next(), ServerFrame::Hello { .. }));

    // The reader is now waiting with nothing to read. A write must go
    // through all the same; with an ordinary file handle it would wait for
    // the read to end, which would be never.
    std::thread::sleep(Duration::from_millis(100));
    send(r#"{"id":1,"method":"subscribe"}"#);
    assert!(matches!(next(), ServerFrame::Result { id: 1, .. }));
    for id in 2..50 {
        send(&format!(
            r#"{{"id":{id},"method":"lines.disable","params":{{"id":1}}}}"#
        ));
        send(&format!(
            r#"{{"id":{id},"method":"lines.enable","params":{{"id":1}}}}"#
        ));
    }
    let mut answers = 0;
    let mut events = 0;
    while answers < 96 {
        match next() {
            ServerFrame::Result { .. } => answers += 1,
            ServerFrame::Event(_) => events += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(events > 0, "events arrive between the answers");

    // Closing from another thread ends the read that is waiting.
    halves.closer.close();
    reading.join().unwrap();
    assert!(
        writer.write_all(b"x\n").is_err(),
        "a closed client writes nothing"
    );
}

#[test]
fn a_client_is_told_why_it_cannot_connect() {
    let missing = OverlappedClient::connect(&unique_name(), Duration::ZERO)
        .err()
        .expect("nothing listens on that name");
    assert_eq!(missing.kind(), ErrorKind::NotFound, "{missing}");

    // The service's own pipe: this test is let in when it runs elevated,
    // and is refused for lack of access when it does not.
    let name = unique_name();
    let stop = Arc::new(Event::new().unwrap());
    let mut listener =
        PipeListener::bind(&name, PipeSecurity::AdministratorsOnly, Arc::clone(&stop)).unwrap();
    let accepting = std::thread::spawn(move || listener.accept().map(|c| c.is_some()));
    match OverlappedClient::connect(&name, Duration::from_secs(1)) {
        Ok(_client) => assert!(accepting.join().unwrap().unwrap()),
        Err(error) => {
            assert_eq!(error.kind(), ErrorKind::PermissionDenied, "{error}");
            stop.set();
            assert!(!accepting.join().unwrap().unwrap());
        }
    }
}

#[test]
fn a_client_sees_the_end_of_the_stream_when_the_service_hangs_up() {
    let fixture = Fixture::new(0, Limits::default());
    let halves = OverlappedClient::connect(&fixture.pipe, Duration::from_secs(5))
        .unwrap()
        .split();
    let mut writer = halves.writer;
    let mut reader = BufReader::new(halves.reader);
    // Not a hello: the service says so and closes the connection.
    writer
        .write_all(b"{\"id\":1,\"method\":\"lines.list\"}\n")
        .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let ServerFrame::Error { error, .. } = decode_server(line.trim_end()).unwrap() else {
        panic!("expected an error, got {line}");
    };
    assert_eq!(error.msgid, "RMD2005");
    line.clear();
    assert_eq!(
        reader.read_line(&mut line).unwrap(),
        0,
        "the end of the stream"
    );
}
