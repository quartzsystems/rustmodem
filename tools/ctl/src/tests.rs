// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Every command against the real control server, over its in-memory
//! transport, with a pool on memory ports behind it.

use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use config::{Config, ConfigFile, LineConfig};
use ctlproto::{
    CallList, Event, LineInfo, LineList, LineState, LogTail, PortInfo, PortKind, PortList,
    ServiceInfo, ServiceMode, Stats,
};
use ctlsrv::memory::{self, BlockingReader, BlockingWriter, MemoryConnector};
use ctlsrv::{Hub, Limits, Server, ServerParts};
use engine::{NullConfig, NullEngine};
use msg::Message;
use pool::{
    EventSink, InlineRunner, MemoryProfiles, Now, NullSwitch, Pool, PoolParts, SerialFactory,
};
use serial::{MemoryHost, SerialBackend, memory_pair};

use crate::args::{self, Options};
use crate::client::{Client, Failure};
use crate::{report, run};

type Hosts = Arc<Mutex<BTreeMap<String, MemoryHost>>>;
type TestClient = Client<BufReader<BlockingReader>, BlockingWriter>;

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

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A service with two idle lines.
struct Service {
    hub: Hub,
    connector: MemoryConnector,
    hosts: Hosts,
    directory: PathBuf,
    config_path: PathBuf,
    stop: Arc<AtomicBool>,
    pool_thread: Option<JoinHandle<()>>,
}

/// What the control servers of the tests run on. The commands themselves
/// block, as they do in `rustmodem-ctl`, on the threads of the tests. A
/// server lives on after its service has gone, for as long as it has a
/// client, so the runtime is the whole program's.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    })
}

impl Service {
    fn start() -> Service {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "rustmodem-ctl-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("rustmodem.toml");
        let mut config = Config {
            lines: [(1, "P1"), (2, "P2")]
                .map(|(id, port)| LineConfig {
                    engine: config::EngineKind::Null,
                    ..LineConfig::new(id, port)
                })
                .to_vec(),
            ..Config::default()
        };
        config.lines[0].name = Some("Front desk".to_string());
        let file = ConfigFile::from_config(&config).unwrap();
        file.save(&config_path).unwrap();

        let hub = Hub::new();
        let hosts: Hosts = Arc::default();
        let mut pool = Pool::new(PoolParts {
            router: Box::new(NullSwitch::new()),
            media: Box::new(InlineRunner::new()),
            serial: Box::new(Factory(Arc::clone(&hosts))),
            engines: Box::new(|_, _| Box::new(NullEngine::new(NullConfig::default()))),
            sink: Box::new(HubSink(hub.clone())),
            profiles: Box::new(MemoryProfiles::new()),
            call_log: None,
        });
        pool.apply_config(&config);
        let (handle, mut requests) = ctlsrv::pool_link();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let pool_thread = std::thread::spawn(move || {
            let started = Instant::now();
            while !stopping.load(Ordering::Relaxed) {
                while let Ok(request) = requests.try_recv() {
                    request.run(&mut pool);
                }
                let mono_ms = started.elapsed().as_millis() as u64;
                pool.service(Now {
                    mono_ms,
                    unix_ms: 1_800_000_000_000 + mono_ms,
                });
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let server = Server::new(ServerParts {
            pool: handle,
            config: file,
            config_path: config_path.clone(),
            version: "9.9.9-test".to_string(),
            pipe: "test-pipe".to_string(),
            hub: hub.clone(),
            log: Arc::new(|_, _| {}),
            limits: Limits::default(),
            mode: ServiceMode::Console,
            clock: ctlsrv::system_clock(),
            apply_service: ctlsrv::restart_to_apply(),
            ports: Arc::new(vec![
                PortInfo {
                    name: "P1".to_string(),
                    kind: PortKind::Com0com,
                    pair: Some("A1".to_string()),
                    host_port: Some("COM11".to_string()),
                    modem: Some("Standard 56000 bps Modem".to_string()),
                    driver_version: None,
                    line: None,
                },
                PortInfo {
                    name: "COM3".to_string(),
                    kind: PortKind::Serial,
                    pair: None,
                    host_port: None,
                    modem: None,
                    driver_version: None,
                    line: None,
                },
            ]),
            cpu: ctlsrv::no_cpu_meter(),
            capture_dir: None,
        });
        let binding_server = server.clone();
        let (mut listener, connector) = memory::listener();
        runtime().spawn(async move { server.serve(&mut listener, ctlsrv::never_stops()).await });

        let service = Service {
            hub,
            connector,
            hosts,
            directory,
            config_path,
            stop,
            pool_thread: Some(pool_thread),
        };
        for port in ["P1", "P2"] {
            service.host(port).open();
        }
        runtime()
            .block_on(binding_server.refresh_bindings())
            .unwrap();
        let mut client = service.client();
        eventually("both lines to be idle", || {
            let list: LineList = client.call(&ctlproto::Call::LinesList).unwrap();
            list.lines.iter().all(|l| l.state == LineState::Idle)
        });
        service
    }

    fn host(&self, port: &str) -> MemoryHost {
        let mut found = None;
        eventually("the port to be opened", || {
            found = self.hosts.lock().unwrap().get(port).cloned();
            found.is_some()
        });
        found.unwrap()
    }

    /// A client that has said hello.
    fn client(&self) -> TestClient {
        let (mut reader, writer) =
            memory::blocking(self.connector.connect().unwrap(), runtime().handle());
        // Long enough for the self-test, which is some seconds of work
        // and on a slow machine more.
        reader.set_timeout(Some(Duration::from_secs(120)));
        let mut client = Client::new(BufReader::new(reader), writer);
        let hello = client.hello().unwrap();
        assert_eq!(hello.version, "9.9.9-test");
        client
    }

    /// Runs a command line as `rustmodem-ctl` would and returns the exit
    /// code, what went to standard output and what went to standard error.
    fn ctl(&self, words: &[&str]) -> (u8, String, String) {
        self.ctl_watching(words, None)
    }

    fn ctl_watching(&self, words: &[&str], limit: Option<usize>) -> (u8, String, String) {
        let options: Options = args::parse(words.iter().map(|w| w.to_string())).unwrap();
        let mut client = self.client();
        let (mut out, mut errors) = (Vec::new(), Vec::new());
        let outcome = run(&options, &mut client, &mut out, limit);
        let code = report(outcome, options.verbose, &mut errors);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(errors).unwrap(),
        )
    }

    fn state(&self, id: u16) -> LineState {
        let mut client = self.client();
        let line: LineInfo = client
            .call(&ctlproto::Call::LinesGet(ctlproto::LineRef { id }))
            .unwrap();
        line.state
    }

    /// Does something once a `watch` started after this call has connected
    /// and had time to subscribe, which it does straight after its hello.
    fn once_watched(&self, act: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
        let hub = self.hub.clone();
        eventually("earlier clients to be gone", || hub.client_count() == 0);
        std::thread::spawn(move || {
            eventually("the watcher to connect", || hub.client_count() == 1);
            std::thread::sleep(Duration::from_millis(100));
            act();
        })
    }

    /// Line 1 dials line 2; waits until line 2 is ringing.
    fn ring_line_2(&self) {
        self.host("P1").write(b"ATE0D2\r");
        eventually("line 2 to ring", || self.state(2) == LineState::Ringing);
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.pool_thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn info_prints_the_service() {
    let service = Service::start();
    let (code, out, errors) = service.ctl(&["info"]);
    assert_eq!((code, errors.as_str()), (0, ""));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "Version:        9.9.9-test");
    assert_eq!(lines[1], "Protocol:       1");
    assert_eq!(lines[2], "Pipe:           test-pipe");
    assert_eq!(
        lines[3],
        format!("Configuration:  {}", service.config_path.display())
    );
    assert_eq!(lines[4], "Lines:          2");
    assert!(lines[5].starts_with("Started:        20") && lines[5].ends_with(" UTC"));
    assert!(lines[6].starts_with("Uptime:         0:00:0"));
    assert_eq!(lines[7], "Mode:           console");
    assert!(lines[8].starts_with("Config loaded:  20") && lines[8].ends_with(" UTC"));

    let (code, out, _) = service.ctl(&["info", "--json"]);
    assert_eq!(code, 0);
    let info: ServiceInfo = serde_json::from_str(&out).unwrap();
    assert_eq!(info.lines, 2);
    assert_eq!(out.lines().count(), 1, "JSON is one line");
}

#[test]
fn lines_prints_a_table_or_json() {
    let service = Service::start();
    let (code, out, errors) = service.ctl(&["lines"]);
    assert_eq!((code, errors.as_str()), (0, ""));
    assert_eq!(
        out,
        "ID  NAME        PORT  STATE  IN  OUT  CONNECTS  TX  RX\n \
          1  Front desk  P1    idle    0    0         0   0   0\n \
          2  -           P2    idle    0    0         0   0   0\n"
    );

    let (code, out, _) = service.ctl(&["lines", "--json"]);
    assert_eq!(code, 0);
    let list: LineList = serde_json::from_str(&out).unwrap();
    assert_eq!(list.lines.len(), 2);
    assert_eq!(list.lines[0].name.as_deref(), Some("Front desk"));
}

#[test]
fn line_prints_one_line_and_an_unknown_line_is_an_api_error() {
    let service = Service::start();
    let (code, out, _) = service.ctl(&["line", "1"]);
    assert_eq!(code, 0);
    assert!(
        out.starts_with("Line:               1\nName:               Front desk\n"),
        "{out}"
    );
    assert!(out.contains("Port:               P1\n"));
    assert!(out.contains("State:              idle\n"));
    assert!(out.contains("Unknown commands:   ok\n"));
    assert!(out.contains("AT commands:        0\n"));
    assert!(out.contains("Binding:            Standard 56000 bps Modem on COM11\n"));
    assert!(!out.contains("Call:"));
    let (_, out, _) = service.ctl(&["line", "2"]);
    assert!(!out.contains("Binding:"), "{out}");

    let (code, out, errors) = service.ctl(&["line", "9"]);
    assert_eq!(code, 1, "an error from the service is exit code 1");
    assert_eq!(out, "");
    assert_eq!(errors, "RMD1001: Line 9 does not exist.\n");

    let (code, _, errors) = service.ctl(&["line", "9", "--verbose"]);
    assert_eq!(code, 1);
    let lines: Vec<&str> = errors.lines().collect();
    assert_eq!(lines[0], "RMD1001: Line 9 does not exist.");
    assert!(lines[1].starts_with("Cause: The request names a line id"));
    assert!(lines[2].starts_with("Recovery: List the lines"));
}

#[test]
fn answer_hangup_and_calls_follow_a_call() {
    let service = Service::start();
    let (code, _, errors) = service.ctl(&["answer", "2"]);
    assert_eq!(code, 1);
    assert_eq!(
        errors,
        "RMD1003: Line 2 is not ringing; its state is idle.\n"
    );

    service.ring_line_2();
    let (code, out, errors) = service.ctl(&["answer", "2"]);
    assert_eq!(
        (code, out.as_str(), errors.as_str()),
        (0, "Line 2: online\n", "")
    );
    eventually("line 1 to be online", || {
        service.state(1) == LineState::Online
    });

    // The line shows its call while it is up.
    let (_, out, _) = service.ctl(&["line", "1"]);
    assert!(
        out.contains(
            "Call:               outgoing 2 (connected, unknown), 0 bytes sent, 0 received\n"
        ),
        "{out}"
    );

    let (code, out, _) = service.ctl(&["hangup", "1"]);
    assert_eq!((code, out.as_str()), (0, "Line 1: idle\n"));
    let (code, _, errors) = service.ctl(&["hangup", "1"]);
    assert_eq!(code, 1);
    assert_eq!(errors, "RMD1004: Line 1 has no call to hang up.\n");
    eventually("line 2 to be idle", || service.state(2) == LineState::Idle);

    let (code, out, _) = service.ctl(&["calls"]);
    assert_eq!(code, 0);
    let rows: Vec<&str> = out.lines().collect();
    assert_eq!(rows.len(), 3);
    assert!(
        rows[0].starts_with("CALL  LINE  DIRECTION  NUMBER  STARTED (UTC)"),
        "{out}"
    );
    assert!(
        rows[0].ends_with("LENGTH  RATE     ENDED     TX  RX"),
        "{out}"
    );
    assert!(
        rows[1].starts_with("   1     1  outgoing   2       20"),
        "{out}"
    );
    assert!(
        rows[1].ends_with("0:00:00  unknown  operator   0   0"),
        "{out}"
    );
    assert!(
        rows[2].starts_with("   2     2  incoming   1       20"),
        "{out}"
    );
    assert!(rows[2].ends_with("unknown  remote     0   0"), "{out}");

    let (_, out, _) = service.ctl(&["calls", "--last", "1"]);
    assert_eq!(out.lines().count(), 2);
    let (_, out, _) = service.ctl(&["calls", "--last", "1", "--json"]);
    let list: CallList = serde_json::from_str(&out).unwrap();
    assert_eq!(list.calls.len(), 1);
    assert_eq!(list.calls[0].line, 2);
}

#[test]
fn reset_enable_disable_and_reload() {
    let service = Service::start();
    let (code, out, _) = service.ctl(&["reset", "1"]);
    assert_eq!((code, out.as_str()), (0, "Line 1: idle\n"));

    let (code, out, _) = service.ctl(&["disable", "2"]);
    assert_eq!((code, out.as_str()), (0, "Line 2: disabled\n"));
    assert!(
        std::fs::read_to_string(&service.config_path)
            .unwrap()
            .contains("enabled = false")
    );
    let (_, out, _) = service.ctl(&["lines"]);
    assert!(out.contains(" 2  -           P2    disabled"), "{out}");

    let (code, out, _) = service.ctl(&["enable", "2"]);
    assert_eq!((code, out.as_str()), (0, "Line 2: not_ready\n"));
    let (code, _, errors) = service.ctl(&["enable", "9"]);
    assert_eq!(code, 1);
    assert_eq!(errors, "RMD0019: Line 9 is not in the configuration.\n");

    let text = std::fs::read_to_string(&service.config_path).unwrap();
    let added =
        format!("{text}\n[[line]]\nid = 5\nserial = {{ backend = \"com0com\", port = \"P5\" }}\n");
    std::fs::write(&service.config_path, added).unwrap();
    let (code, out, _) = service.ctl(&["reload"]);
    assert_eq!(
        (code, out.as_str()),
        (0, "Configuration reloaded: 3 lines.\n")
    );
    let (_, out, _) = service.ctl(&["reload", "--json"]);
    assert_eq!(out, "{\"lines\":3}\n");

    std::fs::write(&service.config_path, "[[line]]\nid = 0\n").unwrap();
    let (code, _, errors) = service.ctl(&["reload"]);
    assert_eq!(code, 1);
    assert!(
        errors.starts_with("RMD0014: Key line.id at line 2 has value 0"),
        "{errors}"
    );
}

#[test]
fn watch_prints_each_event_as_it_happens() {
    let service = Service::start();
    let host = service.host("P1");
    let dialler = service.once_watched(move || host.write(b"ATE0D9\r"));
    let (code, out, errors) = service.ctl_watching(&["watch"], Some(3));
    dialler.join().unwrap();
    assert_eq!((code, errors.as_str()), (0, ""));
    let lines: Vec<&str> = out
        .lines()
        .filter(|line| !line.starts_with("log "))
        .collect();
    assert!(
        lines.contains(&"line 1 call 1 started: outgoing 9"),
        "{out}"
    );

    // As JSON, each event is one line, the frame the service sent.
    let host = service.host("P2");
    let dialler = service.once_watched(move || host.set_dtr(false));
    let (code, out, _) = service.ctl_watching(&["watch", "--json"], Some(1));
    dialler.join().unwrap();
    assert_eq!(code, 0);
    let event: Event = serde_json::from_str(out.trim()).unwrap();
    assert!(matches!(
        event,
        Event::LineState(_) | Event::Log(_) | Event::CallEnded(_)
    ));
}

#[test]
fn every_kind_of_event_has_a_line() {
    use ctlproto::{
        CallEvent, CallInfo, CallRecord, ConnectInfo, Direction, EndReason, LogEvent, LogLevel,
    };
    let call = CallInfo {
        id: 7,
        direction: Direction::Outgoing,
        number: "2".to_string(),
        started_ms: 0,
        connected_ms: Some(1),
        connect: Some(ConnectInfo {
            rate: 2400,
            ..ConnectInfo::default()
        }),
        bytes_tx: 0,
        bytes_rx: 0,
        media: None,
        modem: None,
    };
    let service = Service::start();
    let mut client = service.client();
    let line: LineInfo = client
        .call(&ctlproto::Call::LinesGet(ctlproto::LineRef { id: 1 }))
        .unwrap();
    let cases = [
        (Event::LineState(Box::new(line)), "line 1 is idle"),
        (
            Event::CallStarted(CallEvent {
                line: 1,
                call: call.clone(),
            }),
            "line 1 call 7 started: outgoing 2",
        ),
        (
            Event::CallConnected(CallEvent { line: 1, call }),
            "line 1 call 7 connected: 2400 bit/s",
        ),
        (
            Event::CallEnded(CallRecord {
                id: 7,
                line: 1,
                direction: Direction::Outgoing,
                number: "2".to_string(),
                started_ms: 0,
                connected_ms: None,
                ended_ms: 5,
                reason: EndReason::NoAnswer,
                connect: None,
                bytes_tx: 12,
                bytes_rx: 34,
                media: None,
                modem: None,
            }),
            "line 1 call 7 ended: no_answer, 12 bytes sent, 34 received",
        ),
        (
            Event::Log(LogEvent {
                time_ms: 0,
                level: LogLevel::Warning,
                source: "CTL".to_string(),
                text: "something".to_string(),
            }),
            "log WARN [CTL] something",
        ),
    ];
    for (event, text) in cases {
        assert_eq!(crate::output::event(&event), text);
    }
}

#[test]
fn a_service_that_cannot_be_reached_is_exit_code_2() {
    let Err(failure) = Client::connect("rustmodem-no-such-pipe-for-the-test") else {
        panic!("there is no such pipe");
    };
    let Failure::Connection(problem) = &failure else {
        panic!("expected a connection failure, got {failure:?}");
    };
    assert_eq!(problem.id, "RMD2008");
    let mut errors = Vec::new();
    assert_eq!(report(Err(failure), false, &mut errors), 2);
    let text = String::from_utf8(errors).unwrap();
    assert!(
        text.starts_with(
            "RMD2008: Control pipe rustmodem-no-such-pipe-for-the-test could not be opened:"
        ),
        "{text}"
    );
}

#[test]
fn a_service_that_goes_away_is_exit_code_2() {
    let service = Service::start();
    let mut client = service.client();
    // The server's side of the connection is closed under the client.
    let connector = service.connector.clone();
    drop(service);
    drop(connector);
    let options = args::parse(["lines".to_string()].into_iter()).unwrap();
    let mut out = Vec::new();
    // The pool has stopped: the first answer is the service saying so.
    let outcome = run(&options, &mut client, &mut out, None);
    assert!(matches!(&outcome, Err(Failure::Api(error)) if error.msgid == "RMD2010"));
    assert_eq!(report(outcome, false, &mut Vec::new()), 1);

    // A connection that ends in the middle of things is a connection error.
    let mut cut: Client<&[u8], Vec<u8>> = Client::new(&b""[..], Vec::new());
    let outcome = run(&options, &mut cut, &mut out, None);
    let Err(Failure::Connection(problem)) = &outcome else {
        panic!("expected a connection failure");
    };
    assert_eq!(problem.id, "RMD2009");
    assert_eq!(report(outcome, false, &mut Vec::new()), 2);
}

#[test]
fn a_service_of_another_protocol_is_reported_from_its_own_error() {
    let refusal = ctlproto::encode(&ctlproto::ServerFrame::Error {
        id: None,
        error: msg::message("RMD2001", &["1", "2"]).into(),
    });
    let script = format!("{refusal}\n");
    let mut client: Client<&[u8], Vec<u8>> = Client::new(script.as_bytes(), Vec::new());
    let Err(Failure::Api(error)) = client.hello() else {
        panic!("expected the service's error");
    };
    assert_eq!(error.msgid, "RMD2001");

    // Something that is not the control protocol at all.
    let mut client: Client<&[u8], Vec<u8>> = Client::new(&b"220 smtp ready\n"[..], Vec::new());
    let Err(Failure::Connection(problem)) = client.hello() else {
        panic!("expected a connection failure");
    };
    assert_eq!(problem.id, "RMD2013");
}

#[test]
fn stats_prints_the_statistics_and_what_calls_negotiated() {
    let service = Service::start();
    let (code, out, errors) = service.ctl(&["stats"]);
    assert_eq!((code, errors.as_str()), (0, ""));
    assert_eq!(
        out,
        "Calls answered:     0\n\
         Connected:          0\n\
         Failed to connect:  0\n\
         Rejected:           0\n\
         Abandoned:          0\n\
         Average call:       0:00:00\n\
         Longest call:       0:00:00\n\
         Lines in use:       0 (peak 0)\n\
         Bytes in:           0\n\
         Bytes out:          0\n\
         AT commands:        0\n\
         Retrains:           0\n"
    );

    service.ring_line_2();
    let (code, _, _) = service.ctl(&["answer", "2"]);
    assert_eq!(code, 0);
    eventually("line 1 to be online", || {
        service.state(1) == LineState::Online
    });
    let (code, out, _) = service.ctl(&["stats"]);
    assert_eq!(code, 0);
    assert!(out.contains("Calls answered:     1\n"), "{out}");
    assert!(out.contains("Connected:          1\n"), "{out}");
    assert!(out.contains("Lines in use:       2 (peak 2)\n"), "{out}");
    assert!(out.contains("AT commands:        1\n"), "{out}");
    assert!(
        out.ends_with("\nNEGOTIATED  WHAT     CALLS\nconnect     unknown      1\n"),
        "{out}"
    );

    let (code, out, _) = service.ctl(&["stats", "--json"]);
    assert_eq!(code, 0);
    let stats: Stats = serde_json::from_str(&out).unwrap();
    assert_eq!(stats.calls_connected, 1);
    assert_eq!(stats.connects[0].count, 1);
}

#[test]
fn ports_prints_the_inventory_with_the_lines_that_use_it() {
    let service = Service::start();
    let (code, out, errors) = service.ctl(&["ports"]);
    assert_eq!((code, errors.as_str()), (0, ""));
    assert_eq!(
        out,
        "PORT  KIND     PAIR  HOST PORT  MODEM                     DRIVER  LINE\n\
         COM3  serial   -     -          -                         -          -\n\
         P1    com0com  A1    COM11      Standard 56000 bps Modem  -          1\n"
    );
    let (code, out, _) = service.ctl(&["ports", "--json"]);
    assert_eq!(code, 0);
    let list: PortList = serde_json::from_str(&out).unwrap();
    assert_eq!(list.ports.len(), 2);
    assert_eq!(list.ports[1].line, Some(1));
}

#[test]
fn log_prints_the_last_lines_the_service_logged() {
    let service = Service::start();
    service.host("P1").write(b"ATE0\r");
    eventually("the command to be logged", || {
        let (_, out, _) = service.ctl(&["log"]);
        out.contains("DEBUG [Line01] < OK\n")
    });
    let (code, out, errors) = service.ctl(&["log"]);
    assert_eq!((code, errors.as_str()), (0, ""));
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.len() > 4, "{out}");
    // Each line is the time in UTC to the millisecond, the level, the
    // source and the text.
    for line in &lines {
        let bytes = line.as_bytes();
        assert_eq!(
            (bytes[10], bytes[19], bytes[23]),
            (b' ', b'.', b' '),
            "{line}"
        );
    }
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(" INFO  [Line01] RMD1005: Line 1 state is now idle.")),
        "{out}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(" DEBUG [Line01] > ATE0")),
        "{out}"
    );

    let (code, out, _) = service.ctl(&["log", "--last", "2"]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().collect::<Vec<_>>(), lines[lines.len() - 2..]);
    let (_, out, _) = service.ctl(&["log", "--last", "2", "--json"]);
    let tail: LogTail = serde_json::from_str(&out).unwrap();
    assert_eq!(tail.lines.len(), 2);
    assert_eq!(tail.lines[1].text, "< OK");
}

#[test]
fn sip_groups_routes_capture_and_reregister_against_a_service_with_no_sip() {
    let service = Service::start();
    let (code, out, errors) = service.ctl(&["sip"]);
    assert_eq!(
        (code, out.as_str(), errors.as_str()),
        (0, "SIP is off.\n", "")
    );
    let (code, out, _) = service.ctl(&["sip", "--json"]);
    assert_eq!(code, 0);
    let status: ctlproto::SipStatus = serde_json::from_str(&out).unwrap();
    assert_eq!(status, ctlproto::SipStatus::default());

    let (code, out, _) = service.ctl(&["groups"]);
    assert_eq!((code, out.as_str()), (0, "GROUP  PILOT  STRATEGY  LINES\n"));
    let (code, out, _) = service.ctl(&["routes"]);
    assert_eq!((code, out.as_str()), (0, "#  CALLED  TARGET\n"));

    let (code, out, _) = service.ctl(&["capture", "on", "2"]);
    assert_eq!((code, out.as_str()), (0, "Line 2: capture on\n"));
    let (_, out, _) = service.ctl(&["line", "2"]);
    assert!(out.contains("Capture:            on\n"), "{out}");
    assert!(out.contains("Engine:             null\n"), "{out}");
    assert!(out.contains("SIP:                none\n"), "{out}");
    let (code, out, _) = service.ctl(&["capture", "off", "2"]);
    assert_eq!((code, out.as_str()), (0, "Line 2: capture off\n"));
    let (code, _, errors) = service.ctl(&["capture", "on", "9"]);
    assert_eq!(code, 1);
    assert!(errors.starts_with("RMD1001: "), "{errors}");

    let (code, out, _) = service.ctl(&["reregister"]);
    assert_eq!(
        (code, out.as_str()),
        (0, "Every line that registers is registering again.\n")
    );
    let (code, out, _) = service.ctl(&["reregister", "1"]);
    assert_eq!((code, out.as_str()), (0, "Line 1 is registering again.\n"));
    let (code, _, errors) = service.ctl(&["reregister", "9"]);
    assert_eq!(code, 1);
    assert!(errors.starts_with("RMD1001: "), "{errors}");
}

#[test]
fn the_sip_side_of_a_service_is_laid_out_for_people() {
    use crate::output;
    use ctlproto::{
        HuntGroupInfo, ListenerState, Registration, RegistrationState, RouteInfo, SipRegistration,
        SipStatus, SipSummary,
    };
    let failed = ctlproto::ErrorInfo::from(msg::message("RMD5004", &["3", "pbx"]));
    let status = SipStatus {
        listener: ListenerState::Listening,
        listen: "0.0.0.0:5060".to_string(),
        trusted_peers: vec!["10.10.20.0/24".to_string(), "10.10.30.5".to_string()],
        outbound_peer: String::new(),
        registrations: vec![
            SipRegistration {
                line: 2,
                server: "pbx".to_string(),
                user: "202".to_string(),
                registration: Registration {
                    state: RegistrationState::Registered,
                    expires_in_s: Some(120),
                    error: None,
                },
            },
            SipRegistration {
                line: 3,
                server: "pbx".to_string(),
                user: "203".to_string(),
                registration: Registration {
                    state: RegistrationState::Failed,
                    expires_in_s: None,
                    error: Some(failed),
                },
            },
        ],
        dropped_untrusted: 4,
        calls: 1,
    };
    assert_eq!(
        output::sip(&status),
        "Listener:          listening on 0.0.0.0:5060\n\
         Trusted peers:     10.10.20.0/24, 10.10.30.5\n\
         Outbound peer:     none\n\
         Calls:             1\n\
         Requests dropped:  4\n\
         \n\
         LINE  EXTENSION  REGISTRAR  REGISTRATION\n   \
            2  202        pbx        registered for 120 s more\n   \
            3  203        pbx        failed, RMD5004: Registrar pbx did not answer the registration of line 3.\n"
    );

    assert_eq!(
        output::groups(&[HuntGroupInfo {
            name: "dialup".to_string(),
            pilot: "5551000".to_string(),
            lines: vec![1, 2, 3],
            strategy: "round-robin".to_string(),
        }]),
        "GROUP   PILOT    STRATEGY     LINES\ndialup  5551000  round-robin  1 2 3\n"
    );
    assert_eq!(
        output::routes(&[
            RouteInfo {
                index: 0,
                called: "5551234".to_string(),
                target: "line:3".to_string(),
            },
            RouteInfo {
                index: 1,
                called: "555*".to_string(),
                target: "group:dialup".to_string(),
            },
        ]),
        "#  CALLED   TARGET\n0  5551234  line:3\n1  555*     group:dialup\n"
    );

    let stats = Stats {
        rtp_kbps: 138,
        jitter_ms_avg: 0.25,
        jitter_ms_max: 1.875,
        packet_loss_percent: 0.125,
        codec: "PCMU".to_string(),
        ptime_ms: 20,
        cpu_percent: 1.5,
        sip: SipSummary {
            listener: ListenerState::Listening,
            registered: 1,
            registering: 2,
            dropped_untrusted: 4,
        },
        ..Stats::default()
    };
    let text = output::stats(&stats);
    assert!(
        text.ends_with(
            "SIP:          listening, 1 of 2 lines registered, 4 requests dropped\n\
             RTP:          138 kbit/s, PCMU at 20 ms\n\
             Jitter:       0.2 ms average, 1.9 ms at most\n\
             Packet loss:  0.12 %\n\
             CPU:          1.5 %\n"
        ),
        "{text}"
    );
}

/// A change to a line that changes nothing yet.
fn line_change(id: u16) -> ctlproto::LineChange {
    ctlproto::LineChange {
        id,
        name: None,
        backend: None,
        port: None,
        enabled: None,
        auto_answer_rings: None,
        unknown_command: None,
        engine: None,
        register: None,
        modem: None,
    }
}

#[test]
fn diag_and_selftest_against_lines_with_no_modem() {
    let service = Service::start();
    // A line with no call has no modem to show, and that is no error.
    let (code, out, errors) = service.ctl(&["diag", "1"]);
    assert_eq!(
        (code, out.as_str(), errors.as_str()),
        (
            0,
            "Line 1 has no modem to show: it has no call, or its engine is not the modem.\n",
            ""
        )
    );
    let (code, out, _) = service.ctl(&["diag", "1", "--json"]);
    assert_eq!(code, 0);
    let diag: ctlproto::LineDiag = serde_json::from_str(&out).unwrap();
    assert_eq!((diag.line, diag.modem), (1, None));
    let (code, _, errors) = service.ctl(&["diag", "9"]);
    assert_eq!(code, 1);
    assert!(errors.starts_with("RMD1001: "), "{errors}");

    // The lines here have the null engine, which a self-test cannot use.
    let (code, out, errors) = service.ctl(&["selftest", "1", "2"]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        errors,
        "RMD7003: Line 1 cannot be in a self-test: its engine is null, not the modem.\n"
    );
    let (code, _, errors) = service.ctl(&["selftest", "2", "2"]);
    assert_eq!(code, 1);
    assert!(errors.starts_with("RMD7004: "), "{errors}");
    // A line with the null engine shows no modem settings.
    let (_, out, _) = service.ctl(&["line", "1"]);
    assert!(!out.contains("Modem:"), "{out}");
}

#[test]
fn selftest_runs_two_modems_and_its_exit_code_says_whether_they_passed() {
    let service = Service::start();
    let mut client = service.client();
    for id in [1, 2] {
        let mut change = line_change(id);
        change.engine = Some("modem".to_string());
        client
            .call::<LineInfo>(&ctlproto::Call::LinesEdit(change))
            .unwrap();
    }
    let (_, out, _) = service.ctl(&["line", "1"]);
    assert!(out.contains("Engine:             modem\n"), "{out}");
    assert!(
        out.contains(
            "Modem:              v22bis then v22 then bell212a then v21 then bell103, V.8 on, \
             -12 dBm0, error control auto, compression on, guard tone off\n"
        ),
        "{out}"
    );

    let (code, out, errors) = service.ctl(&["selftest", "1", "2"]);
    assert_eq!((code, errors.as_str()), (0, ""), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "Self-test:  line 1 calls line 2");
    assert!(
        lines[1].starts_with("Connect:    2400 bit/s V.22bis, LAPM, V.42bis after "),
        "{out}"
    );
    assert_eq!(
        lines[2],
        "Data:       65536 of 65536 octets whole from 1 to 2, 65536 of 65536 from 2 to 1"
    );
    assert!(lines[3].starts_with("Time:       0:04:"), "{out}");
    assert_eq!(lines[4], "Result:     passed");
    assert_eq!(lines.len(), 5);
    // The lines are free again.
    assert_eq!(service.state(1), LineState::Idle);

    // Modems with no modulation in common: the test is run, the result
    // is printed, and the command fails with the reason.
    let mut bell = line_change(2);
    bell.modem = Some(ctlproto::LineModemSpec {
        modulations: Some(vec!["bell103".to_string()]),
        v8: Some(false),
        tx_level_dbm0: Some(-10),
        ..ctlproto::LineModemSpec::default()
    });
    client
        .call::<LineInfo>(&ctlproto::Call::LinesEdit(bell))
        .unwrap();
    let mut v21 = line_change(1);
    v21.modem = Some(ctlproto::LineModemSpec {
        modulations: Some(vec!["v21".to_string()]),
        ..ctlproto::LineModemSpec::default()
    });
    client
        .call::<LineInfo>(&ctlproto::Call::LinesEdit(v21))
        .unwrap();
    let (_, out, _) = service.ctl(&["line", "2"]);
    assert!(
        out.contains(
            "Modem:              bell103 (line), V.8 off (line), -10 dBm0 (line), error control \
             auto, compression on, guard tone off\n"
        ),
        "{out}"
    );
    let (code, out, errors) = service.ctl(&["selftest", "1", "2"]);
    assert_eq!(code, 1);
    assert!(out.contains("Connect:    none\n"), "{out}");
    assert!(out.contains("Result:     FAILED\n"), "{out}");
    assert!(out.contains("Reason:     RMD7005: "), "{out}");
    assert!(
        errors.starts_with("RMD7005: The self-test of lines 1 and 2 failed"),
        "{errors}"
    );
    let (code, out, _) = service.ctl(&["selftest", "1", "2", "--json"]);
    assert_eq!(code, 1);
    let result: ctlproto::SelfTestResult = serde_json::from_str(&out).unwrap();
    assert!(!result.passed && result.error.is_some());
}

#[test]
fn the_modem_of_a_call_is_laid_out_for_people() {
    use crate::output;
    use ctlproto::{LineDiag, ModemInfo, ModemStateChange};
    let modem = ModemInfo {
        state: "data".to_string(),
        rx_level_tenth_dbm0: Some(-143),
        tx_level_tenth_dbm0: Some(-120),
        carrier: true,
        frequency_error_mhz: -1250,
        quality_percent: 71,
        framing_errors: 2,
        retrains: 0,
        ..ModemInfo::default()
    };
    let diag = LineDiag {
        line: 3,
        modem: Some(modem.clone()),
        history: vec![
            ModemStateChange {
                state: "answer-tone".to_string(),
                at_ms: 2150,
            },
            ModemStateChange {
                state: "data".to_string(),
                at_ms: 5490,
            },
        ],
        scope_kind: "discriminator".to_string(),
        scope: vec![-127, -64, 0, 64, 127],
        ..LineDiag::default()
    };
    assert_eq!(
        output::diag(&diag),
        "Line:             3\n\
         State:            data\n\
         Receive level:    -14.3 dBm0\n\
         Transmit level:   -12.0 dBm0\n\
         Carrier:          detected\n\
         Frequency error:  -1.2 Hz\n\
         Quality:          71%\n\
         Framing errors:   2\n\
         Retrains:         0\n\
         History:          answer-tone at 2.150 s, data at 5.490 s\n\
         Scope:            ▁▃▅▇█ (discriminator)\n"
    );
    // A modem that hears and sends nothing, and has nothing to show.
    let silent = LineDiag {
        modem: Some(ModemInfo {
            state: "answer-silence".to_string(),
            rx_level_tenth_dbm0: None,
            tx_level_tenth_dbm0: None,
            carrier: false,
            ..modem.clone()
        }),
        history: Vec::new(),
        scope: Vec::new(),
        ..diag.clone()
    };
    // A modem at 2400 bit/s with its data link, and its constellation
    // where the scope was: three points, of which one is 0.3 of a unit
    // out.
    let fast = LineDiag {
        modem: Some(ModemInfo {
            snr_tenth_db: Some(312),
            equalizer_error_tenth_db: Some(-305),
            timing_offset_ppm: Some(-12),
            link: Some(Box::new(ctlproto::LinkInfo {
                stage: "lapm".to_string(),
                frames_tx: 162,
                frames_rx: 161,
                retransmissions: 3,
                frame_errors: 2,
                window: 1,
                window_size: 15,
                compression_tx: "compressed".to_string(),
                compression_rx: "transparent".to_string(),
                tx_octets_in: 20_000,
                tx_octets_out: 8000,
                rx_octets_in: 10_100,
                rx_octets_out: 10_000,
                ..ctlproto::LinkInfo::default()
            })),
            ..modem.clone()
        }),
        history: Vec::new(),
        scope_kind: "none".to_string(),
        scope: Vec::new(),
        constellation_bits: 4,
        constellation_unit: 30,
        constellation: vec![30, 30, -90, 90, 99, -30],
        ..diag.clone()
    };
    let text = output::diag(&fast);
    assert!(
        text.ends_with(
            "Signal to noise:  31.2 dB\n\
             Equaliser error:  -30.5 dB\n\
             Timing offset:    -12 ppm\n\
             Link:             lapm, frames 162 out and 161 in, 2 bad, 3 sent again, window 1 of \
             15, compression out compressed 2.5:1, in transparent 1.0:1\n\
             Constellation:    3 points of sixteen, the farthest 0.30 of a unit from its place\n"
        ),
        "{text}"
    );
    let text = output::diag(&silent);
    assert!(text.contains("Receive level:    none\n"), "{text}");
    assert!(text.contains("Carrier:          absent\n"), "{text}");
    assert!(!text.contains("History:") && !text.contains("Scope:"));
    // As `watch` prints it, and with the call on `line`.
    assert_eq!(
        output::event(&Event::LineDiag(diag)),
        "line 3 modem: data, receiving -14.3 dBm0, sending -12.0 dBm0, carrier detected, \
         -1.2 Hz, quality 71%, 2 framing errors, 0 retrains"
    );
    assert_eq!(
        output::event(&Event::LineDiag(LineDiag {
            line: 3,
            ..LineDiag::default()
        })),
        "line 3 modem: none"
    );
}
