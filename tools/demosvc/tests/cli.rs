// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `rustmodem-demosvc.exe` as a program: its arguments, and each of its
//! scripts seen through the control API on its pipe.

#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ctlproto::{
    Call, CallList, CallsQuery, CaptureSwitch, ClientFrame, ControlChange, EndReason, ErrorInfo,
    Event, EventSelection, Hello, HuntGroupList, LineChange, LineInfo, LineList, LineRef, LineSpec,
    LineState, ListenerState, LogLevel, LogTail, LogTailQuery, PROTOCOL, PortList,
    RegistrationState, ReregisterQuery, RouteList, ServerFrame, ServiceInfo, ServiceMode, Settings,
    SettingsChange, SettingsResult, SipMode, SipStatus, Stats, Subscription, TraceDirection,
    decode_server, encode,
};

/// How long a test waits for the demo service before it fails.
const PATIENCE: Duration = Duration::from_secs(60);

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustmodem-demosvc"))
        .args(args)
        .output()
        .unwrap()
}

/// Waits until `check` is true, or fails the test.
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A demo service running as a child process on a pipe of its own, ended
/// when the test ends.
struct Demo {
    child: Child,
    pipe: String,
}

impl Demo {
    fn start(script: &str, lines: u16) -> Demo {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let pipe = format!(
            "rustmodem-demo-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let child = Command::new(env!("CARGO_BIN_EXE_rustmodem-demosvc"))
            .args(["--script", script, "--lines", &lines.to_string()])
            .args(["--pipe", &pipe])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Demo { child, pipe }
    }

    /// A client that has said hello. The pipe is there a moment after the
    /// program starts.
    fn connect(&self) -> Client {
        let path = format!(r"\\.\pipe\{}", self.pipe);
        let deadline = Instant::now() + PATIENCE;
        let file = loop {
            match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => break file,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "the demo service never opened its pipe: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut client = Client {
            reader: BufReader::new(file.try_clone().unwrap()),
            writer: file,
            next_id: 1,
        };
        client.send(&encode(&ClientFrame::Hello {
            hello: Hello {
                protocol: PROTOCOL,
                version: "demo-test".to_string(),
            },
        }));
        assert!(matches!(client.read(), ServerFrame::Hello { .. }));
        client
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ =
            std::fs::remove_dir_all(std::env::temp_dir().join("rustmodem-demo").join(&self.pipe));
    }
}

/// A client that asks and waits for the answer, as `rustmodem-ctl` does.
struct Client {
    reader: BufReader<File>,
    writer: File,
    next_id: u64,
}

impl Client {
    fn send(&mut self, line: &str) {
        self.writer
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
    }

    fn read(&mut self) -> ServerFrame {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        decode_server(line.trim_end()).unwrap_or_else(|m| panic!("{m}: {line}"))
    }

    fn call(&mut self, call: Call) -> Result<serde_json::Value, ErrorInfo> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&encode(&ClientFrame::Request(call.request(id))));
        match self.read() {
            ServerFrame::Result { result, .. } => Ok(result),
            ServerFrame::Error { error, .. } => Err(error),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn ok<T: serde::de::DeserializeOwned>(&mut self, call: Call) -> T {
        let method = call.method();
        let result = self
            .call(call)
            .unwrap_or_else(|error| panic!("{method} failed: {error}"));
        serde_json::from_value(result).unwrap_or_else(|e| panic!("{method} result: {e}"))
    }

    fn lines(&mut self) -> Vec<LineInfo> {
        self.ok::<LineList>(Call::LinesList).lines
    }

    fn states(&mut self) -> Vec<LineState> {
        self.lines().iter().map(|line| line.state).collect()
    }
}

#[test]
fn help_version_and_arguments_that_are_not_understood() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    let usage = text(&help.stdout);
    for word in [
        "--lines", "--script", "idle", "busy", "frozen", "sip", "--pipe",
    ] {
        assert!(usage.contains(word), "{usage}");
    }
    let version = run(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        text(&version.stdout).trim(),
        format!("rustmodem-demosvc {}", env!("CARGO_PKG_VERSION"))
    );
    let refused = run(&["--script", "chaos"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        text(&refused.stderr).starts_with("RMD0009: Command-line argument chaos"),
        "{}",
        text(&refused.stderr)
    );
}

#[test]
fn the_idle_script_is_a_service_with_every_line_waiting() {
    let demo = Demo::start("idle", 3);
    let mut client = demo.connect();

    let info: ServiceInfo = client.ok(Call::ServiceInfo);
    assert_eq!(info.mode, ServiceMode::Demo);
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(info.pipe, demo.pipe);
    assert_eq!(info.lines, 3);
    assert!(info.config_path.ends_with("rustmodem-demo.toml"));

    eventually("every line to be idle", || {
        client.states() == [LineState::Idle; 3]
    });
    let lines = client.lines();
    assert_eq!(lines[0].port, "CNCB0");
    assert_eq!(
        lines[0].binding.as_deref(),
        Some("Standard 56000 bps Modem on COM11")
    );
    assert_eq!(
        lines[2].binding.as_deref(),
        Some("Standard 56000 bps Modem on COM13")
    );

    // The inventory has the lines' ports, spare pairs and a real port.
    let ports = client.ok::<PortList>(Call::PortsList).ports;
    let used: Vec<(&str, u16)> = ports
        .iter()
        .filter_map(|port| Some((port.name.as_str(), port.line?)))
        .collect();
    assert_eq!(used, [("CNCB0", 1), ("CNCB1", 2), ("CNCB2", 3)]);
    assert!(ports.iter().any(|port| port.name == "COM1"));
    assert!(
        ports
            .iter()
            .any(|port| port.name == "CNCB3" && port.line.is_none())
    );

    // Nothing happens by itself.
    std::thread::sleep(Duration::from_millis(500));
    let stats: Stats = client.ok(Call::StatsGet);
    assert_eq!(stats.calls_answered + stats.calls_rejected, 0);
    assert_eq!(stats.lines_in_use, 0);

    // It is a real service: a line can be added, on a spare port, and a
    // program opens that port too; lines can be taken out of service and
    // put back; settings are written to its own file.
    let added: LineInfo = client.ok(Call::LinesAdd(LineSpec {
        id: 9,
        name: Some("Added".to_string()),
        backend: None,
        port: "CNCB3".to_string(),
        enabled: None,
        auto_answer_rings: None,
        unknown_command: None,
        engine: None,
        register: None,
        modem: None,
    }));
    assert_eq!(added.binding.as_deref(), Some("COM14"));
    eventually("the new line to be idle", || {
        client.states() == [LineState::Idle; 4]
    });
    let disabled: LineInfo = client.ok(Call::LinesDisable(LineRef { id: 1 }));
    assert_eq!(disabled.state, LineState::Disabled);
    client.ok::<LineInfo>(Call::LinesEnable(LineRef { id: 1 }));
    let moved: LineInfo = client.ok(Call::LinesEdit(LineChange {
        id: 2,
        name: None,
        backend: None,
        port: Some("CNCB5".to_string()),
        enabled: None,
        auto_answer_rings: None,
        unknown_command: None,
        engine: None,
        register: None,
        modem: None,
    }));
    assert_eq!(moved.binding.as_deref(), Some("COM16"));
    eventually("every line to be idle again", || {
        client.states() == [LineState::Idle; 4]
    });
    let set: SettingsResult = client.ok(Call::ConfigSet(SettingsChange {
        service: None,
        control: Some(ControlChange {
            pipe: Some("another-pipe".to_string()),
        }),
        ..SettingsChange::default()
    }));
    assert!(set.restart_required);
    assert!(
        std::fs::read_to_string(&info.config_path)
            .unwrap()
            .contains("another-pipe")
    );

    // The log has the service's own lines, the lines' and the control
    // server's.
    let log = client
        .ok::<LogTail>(Call::LogTail(LogTailQuery::default()))
        .lines;
    for source in ["SVC", "Line01", "CTL"] {
        assert!(log.iter().any(|line| line.source == source), "{source}");
    }
    assert!(log.iter().any(|line| line.text.starts_with("RMD0001:")));
}

#[test]
fn the_frozen_script_is_the_same_at_every_run() {
    // Everything a screenshot shows, as two separate runs report it.
    let snapshot = |demo: &Demo| {
        let mut client = demo.connect();
        let mut info: ServiceInfo = client.ok(Call::ServiceInfo);
        // The two runs are on different pipes.
        info.pipe.clear();
        info.config_path.clear();
        let lines = client.lines();
        let stats: Stats = client.ok(Call::StatsGet);
        let calls: CallList = client.ok(Call::CallsList(CallsQuery::default()));
        let log: LogTail = client.ok(Call::LogTail(LogTailQuery::default()));
        (
            encode(&info),
            encode(&LineList { lines }),
            encode(&stats),
            encode(&calls),
            encode(&log).replace(&demo.pipe, "PIPE"),
        )
    };
    let first = Demo::start("frozen", 8);
    let second = Demo::start("frozen", 8);
    let one = snapshot(&first);
    assert_eq!(one, snapshot(&second));
    // And at a later moment of the same run: the clock stands still.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(one, snapshot(&first));

    let mut client = first.connect();
    let info: ServiceInfo = client.ok(Call::ServiceInfo);
    assert_eq!(info.mode, ServiceMode::Demo);
    assert_eq!(info.started_ms, 1_800_000_000_000);
    assert_eq!(info.uptime_ms, 1_497_170);

    use LineState::{Disabled, Handshaking, Idle, NotReady, Online, Ringing};
    assert_eq!(
        client.states(),
        [
            Online,
            Online,
            Handshaking,
            Ringing,
            Idle,
            NotReady,
            Disabled,
            NotReady
        ]
    );
    let lines = client.lines();
    // Line 6 has no program on its port; line 8 has a port that will not
    // open, which is a different thing.
    assert!(lines[5].error.is_none());
    assert_eq!(lines[7].error.as_ref().unwrap().msgid, "RMD3001");
    let call = lines[0].call.as_ref().unwrap();
    assert_eq!(call.number, "2");
    assert_eq!(call.connect.as_ref().unwrap().rate, 1200);
    assert_eq!(
        (call.bytes_tx, call.bytes_rx),
        (5_200, 18_400),
        "the bytes of the call that is up"
    );
    // The call has been connected for three minutes and some seconds.
    let connected_for = info.started_ms + info.uptime_ms - call.connected_ms.unwrap();
    assert_eq!(connected_for / 60_000, 3);
    assert_eq!(
        lines[1].call.as_ref().unwrap().connect,
        call.connect,
        "both ends negotiated the same"
    );

    let stats: Stats = client.ok(Call::StatsGet);
    assert_eq!(stats.calls_answered, 7);
    assert_eq!(stats.calls_connected, 6);
    assert_eq!(stats.calls_failed, 1);
    assert_eq!(stats.calls_rejected, 2);
    assert_eq!(stats.calls_abandoned, 1);
    assert_eq!((stats.lines_in_use, stats.peak_lines_in_use), (4, 4));
    assert_eq!(stats.longest_call_ms / 60_000, 12);
    let connects: Vec<(u32, u64)> = stats.connects.iter().map(|c| (c.rate, c.count)).collect();
    assert_eq!(connects, [(2400, 3), (1200, 2), (300, 1)]);

    let calls = client
        .ok::<CallList>(Call::CallsList(CallsQuery::default()))
        .calls;
    assert!(calls.iter().any(|call| call.reason == EndReason::Abandoned));
    assert!(calls.iter().any(|call| call.reason == EndReason::Busy));
    let log = client
        .ok::<LogTail>(Call::LogTail(LogTailQuery::default()))
        .lines;
    assert!(log.iter().all(|line| line.source != "CTL"));
    assert!(log.iter().any(|line| line.level == LogLevel::Error));

    // What the operator asks for is still done.
    let answered: LineInfo = client.ok(Call::LinesAnswer(LineRef { id: 4 }));
    assert_eq!(answered.state, Handshaking);
    let hung_up: LineInfo = client.ok(Call::LinesHangup(LineRef { id: 1 }));
    assert_eq!(hung_up.state, Idle);
}

#[test]
fn the_frozen_script_does_what_fewer_lines_allow() {
    for (lines, expected) in [
        (1, vec![LineState::Idle]),
        (2, vec![LineState::Online, LineState::Online]),
        (
            3,
            vec![LineState::Online, LineState::Online, LineState::Idle],
        ),
    ] {
        let demo = Demo::start("frozen", lines);
        let mut client = demo.connect();
        assert_eq!(client.states(), expected, "{lines} lines");
    }
}

#[test]
fn the_busy_script_makes_calls_that_connect_and_carry_data() {
    let demo = Demo::start("busy", 4);
    let mut client = demo.connect();
    let mut seen_online = false;
    // The first host dials at once; its call rings, is answered, connects
    // and carries data within a few seconds. Calls end too, after talking
    // for up to a minute, which the smoke test of the manager sits through
    // and this test does not.
    eventually("a call to connect and carry data", || {
        seen_online |= client.states().contains(&LineState::Online);
        let stats: Stats = client.ok(Call::StatsGet);
        seen_online && stats.calls_connected >= 1 && stats.bytes_in > 0
    });
    let stats: Stats = client.ok(Call::StatsGet);
    assert!(stats.peak_lines_in_use >= 2);
    assert!(stats.at_commands >= 4, "every host turned echo off");
    let connect = &stats.connects[0];
    assert!(connect.rate > 0 && connect.carrier.is_some());
    // Every line is still in service: no port failed.
    assert!(client.lines().iter().all(|line| line.error.is_none()));
}

#[test]
fn a_second_copy_on_the_same_pipe_is_refused() {
    let demo = Demo::start("idle", 1);
    let _client = demo.connect();
    let second = run(&["--pipe", &demo.pipe]);
    assert_eq!(second.status.code(), Some(1));
    assert!(
        text(&second.stderr).starts_with("RMD2007: Control pipe"),
        "{}",
        text(&second.stderr)
    );
}

/// The `sip` script is the service with SIP on: calls come over SIP and
/// RTP from test peers, a line registers, and everything the control API
/// has for SIP and media says so.
#[test]
fn the_sip_script_takes_calls_over_sip_and_shows_them() {
    let demo = Demo::start("sip", 4);
    let mut client = demo.connect();

    // The configuration the script made for itself.
    let settings: Settings = client.ok(Call::ConfigGet);
    assert!(settings.sip.enabled);
    assert!(settings.sip.listen.starts_with("127.0.0.1:"));
    assert_eq!(settings.sip.trusted_peers, ["127.0.0.1"]);
    let groups: HuntGroupList = client.ok(Call::HuntGroupsList);
    assert_eq!(groups.groups.len(), 1);
    assert_eq!(groups.groups[0].lines, [1, 2, 3, 4]);
    let routes: RouteList = client.ok(Call::RoutesList);
    assert_eq!(routes.routes[0].target, "group:dialup");

    // The last line registers to the peer; the others are on the trunk.
    eventually("line 4 to be registered", || {
        let status: SipStatus = client.ok(Call::SipStatus);
        status.listener == ListenerState::Listening
            && status.listen == settings.sip.listen
            && status.registrations.len() == 1
            && status.registrations[0].line == 4
            && status.registrations[0].registration.state == RegistrationState::Registered
    });
    let lines = client.lines();
    assert_eq!(lines[0].sip.mode, SipMode::Trunk);
    assert_eq!(lines[0].engine, "echo");
    assert_eq!(lines[1].engine, "null");
    assert_eq!(lines[0].hunt_groups, ["dialup"]);
    let registered = &lines[3].sip;
    assert_eq!(registered.mode, SipMode::Registered);
    assert_eq!(registered.extension.as_deref(), Some("204"));
    assert!(registered.has_password);
    let registration = registered.registration.as_ref().unwrap();
    assert_eq!(registration.state, RegistrationState::Registered);
    assert!(registration.expires_in_s.is_some_and(|left| left <= 300));

    // A call comes, is answered by the line itself, and its RTP shows.
    let captured: LineInfo = client.ok(Call::LinesCapture(CaptureSwitch { id: 1, on: true }));
    assert!(captured.capture);
    let mut media = None;
    eventually("a call with RTP on a line", || {
        media = client
            .lines()
            .into_iter()
            .filter(|line| line.state == LineState::Online)
            .filter_map(|line| line.call?.media)
            .find(|media| media.rx_packets > 100);
        media.is_some()
    });
    let media = media.unwrap();
    assert_eq!(media.codec, "PCMU");
    assert_eq!(media.ptime_ms, 20);
    assert!(media.remote.as_deref().unwrap().starts_with("127.0.0.1:"));
    assert!(media.tx_packets > 90, "{media:?}");
    assert_eq!((media.slips, media.resync), (0, 0), "{media:?}");

    eventually("the statistics to show the call", || {
        let stats: Stats = client.ok(Call::StatsGet);
        stats.rtp_kbps > 100
            && stats.codec == "PCMU"
            && stats.ptime_ms == 20
            && stats.sip.listener == ListenerState::Listening
            && (stats.sip.registered, stats.sip.registering) == (1, 1)
    });

    // The trace has to be asked for by name; a registration made again
    // is a message each way, and an event when its state changes.
    let all: Subscription = client.ok(Call::Subscribe(EventSelection::default()));
    assert!(!all.events.iter().any(|name| name == "sip.trace"));
    client.ok::<Subscription>(Call::Unsubscribe(EventSelection::default()));
    let named: Subscription = client.ok(Call::Subscribe(EventSelection {
        events: Some(vec!["sip.trace".to_string()]),
        line: None,
    }));
    assert_eq!(named.events, ["sip.trace"]);
    let id = client.next_id;
    client.next_id += 1;
    client.send(&encode(&ClientFrame::Request(
        Call::SipReregister(ReregisterQuery { id: Some(4) }).request(id),
    )));
    let (mut answered, mut sent, mut received) = (false, false, false);
    let deadline = Instant::now() + PATIENCE;
    while !(answered && sent && received) {
        assert!(Instant::now() < deadline, "no trace of the registration");
        match client.read() {
            ServerFrame::Result { id: answer, .. } if answer == id => answered = true,
            ServerFrame::Event(Event::SipTrace(trace)) => {
                assert!(trace.peer.starts_with("127.0.0.1:"));
                if trace.text.starts_with("REGISTER ") {
                    assert_eq!(trace.direction, TraceDirection::Sent);
                    sent = true;
                }
                if sent && trace.text.starts_with("SIP/2.0 200") && trace.text.contains("REGISTER")
                {
                    assert_eq!(trace.direction, TraceDirection::Received);
                    received = true;
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
