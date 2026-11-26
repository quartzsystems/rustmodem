// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control API over the in-memory transport: a round trip for every
//! method, the hello exchange, malformed requests, events, and clients that
//! do not keep up.

mod common;

use std::time::{Duration, Instant};

use common::{Fixture, PIPE, TestClient, VERSION, eventually};
use ctlproto::{
    Call, CallEvent, CallList, CallRecord, CallsQuery, ControlChange, Direction, Done, EVENT_NAMES,
    EndReason, Event, EventSelection, LineChange, LineInfo, LineList, LineRef, LineSpec, LineState,
    LogEvent, LogLevel, LogTail, LogTailQuery, PROTOCOL, PortKind, PortList, Reloaded, ServerFrame,
    ServiceChange, ServiceInfo, ServiceMode, Settings, SettingsChange, SettingsResult, Stats,
    Subscription,
};
use ctlsrv::{LOG_RING, Limits};

fn line(id: u16) -> LineRef {
    LineRef { id }
}

fn selection(events: &[&str]) -> EventSelection {
    EventSelection {
        events: Some(events.iter().map(|e| e.to_string()).collect()),
    }
}

fn spec(id: u16, port: &str) -> LineSpec {
    LineSpec {
        id,
        name: None,
        backend: None,
        port: port.to_string(),
        enabled: None,
        auto_answer_rings: None,
        unknown_command: None,
    }
}

fn change(id: u16) -> LineChange {
    LineChange {
        id,
        name: None,
        backend: None,
        port: None,
        enabled: None,
        auto_answer_rings: None,
        unknown_command: None,
    }
}

/// Line 1 dials line 2 and the test waits until line 2 is ringing.
fn ring_line_2(fixture: &Fixture, client: &mut TestClient) {
    fixture.host("P1").write(b"ATE0D2\r");
    client.wait_for_state(2, LineState::Ringing);
}

/// Exit criterion 3 of phase 0: every method in docs/CONTROL_API.md has a
/// round-trip test. The methods are taken from the list the documentation
/// is generated from, so a method added there without a case here fails.
#[test]
fn every_method_round_trips() {
    let fixture = Fixture::new(2);
    let mut client = fixture.connect();

    for method in Call::METHODS {
        match method {
            "service.info" => {
                let info: ServiceInfo = client.ok(Call::ServiceInfo);
                assert_eq!(info.version, VERSION);
                assert_eq!(info.protocol, PROTOCOL);
                assert_eq!(info.pipe, PIPE);
                assert_eq!(info.config_path, fixture.config_path.display().to_string());
                assert_eq!(info.lines, 2);
                assert!(info.started_ms > 1_600_000_000_000);
                assert_eq!(info.config_loaded_ms, info.started_ms);
                assert_eq!(info.mode, ServiceMode::Console);
                let later: ServiceInfo = client.ok(Call::ServiceInfo);
                assert!(later.uptime_ms >= info.uptime_ms);
            }
            "lines.list" => {
                let list: LineList = client.ok(Call::LinesList);
                assert_eq!(list.lines.iter().map(|l| l.id).collect::<Vec<_>>(), [1, 2]);
                assert!(list.lines.iter().all(|l| l.state == LineState::Idle));
            }
            "lines.get" => {
                let info: LineInfo = client.ok(Call::LinesGet(line(2)));
                assert_eq!((info.id, info.port.as_str()), (2, "P2"));
                assert_eq!(info.backend, "com0com");
                assert_eq!(client.err(Call::LinesGet(line(9))), "RMD1001");
            }
            "lines.answer" => {
                assert_eq!(client.err(Call::LinesAnswer(line(2))), "RMD1003");
                assert_eq!(client.err(Call::LinesAnswer(line(9))), "RMD1001");
                ring_line_2(&fixture, &mut client);
                let info: LineInfo = client.ok(Call::LinesAnswer(line(2)));
                assert_eq!(info.state, LineState::Online);
                assert!(info.call.is_some());
                client.wait_for_state(1, LineState::Online);
            }
            "lines.hangup" => {
                // The call the answer case set up is still connected.
                let info: LineInfo = client.ok(Call::LinesHangup(line(1)));
                assert_eq!(info.state, LineState::Idle);
                assert!(info.call.is_none());
                client.wait_for_state(2, LineState::Idle);
                assert_eq!(client.err(Call::LinesHangup(line(1))), "RMD1004");
                assert_eq!(client.err(Call::LinesHangup(line(9))), "RMD1001");
            }
            "lines.reset" => {
                ring_line_2(&fixture, &mut client);
                let info: LineInfo = client.ok(Call::LinesReset(line(1)));
                assert_eq!(info.state, LineState::Idle);
                client.wait_for_state(2, LineState::Idle);
                assert_eq!(client.err(Call::LinesReset(line(9))), "RMD1001");
            }
            "lines.enable" => {
                client.ok::<LineInfo>(Call::LinesDisable(line(2)));
                let info: LineInfo = client.ok(Call::LinesEnable(line(2)));
                assert!(info.enabled);
                assert_ne!(info.state, LineState::Disabled);
                assert!(fixture.config_text().contains("enabled = true"));
                assert_eq!(client.err(Call::LinesEnable(line(9))), "RMD0019");
                // The port was released and is opened again by the pool's
                // thread, a moment after the answer; its host has to open
                // the new port, not the one that was released.
                eventually("the port to be opened again", || {
                    fixture.hosts.lock().unwrap()["P2"].backend_alive()
                });
                fixture.host("P2").open();
                client.wait_for_state(2, LineState::Idle);
            }
            "lines.disable" => {
                let old_host = fixture.host("P2");
                let info: LineInfo = client.ok(Call::LinesDisable(line(2)));
                assert!(!info.enabled);
                assert_eq!(info.state, LineState::Disabled);
                assert!(fixture.config_text().contains("enabled = false"));
                assert!(!old_host.backend_alive(), "the port was released");
                assert_eq!(client.err(Call::LinesDisable(line(9))), "RMD0019");
                client.ok::<LineInfo>(Call::LinesEnable(line(2)));
                eventually("the port to be opened again", || {
                    fixture.hosts.lock().unwrap()["P2"].backend_alive()
                });
                fixture.host("P2").open();
                client.wait_for_state(2, LineState::Idle);
            }
            "lines.add" => {
                let mut new = spec(3, "P3");
                new.name = Some("Three".to_string());
                new.backend = Some("com0com".to_string());
                new.enabled = Some(true);
                new.auto_answer_rings = Some(2);
                new.unknown_command = Some("error".to_string());
                let info: LineInfo = client.ok(Call::LinesAdd(new.clone()));
                assert_eq!(info.id, 3);
                assert_eq!(info.name.as_deref(), Some("Three"));
                assert_eq!(info.auto_answer_rings, 2);
                assert_eq!(info.unknown_command, "error");
                assert_eq!(client.lines().len(), 3);
                let text = fixture.config_text();
                assert!(
                    text.contains("id = 3") && text.contains("port = \"P3\""),
                    "{text}"
                );

                assert_eq!(client.err(Call::LinesAdd(new)), "RMD0020");
                assert_eq!(client.err(Call::LinesAdd(spec(4, "p3"))), "RMD0017");
                assert_eq!(client.err(Call::LinesAdd(spec(0, "P4"))), "RMD0014");
                assert_eq!(client.err(Call::LinesAdd(spec(4, "bad port"))), "RMD0014");
                let mut bad = spec(4, "P4");
                bad.backend = Some("usb".to_string());
                assert_eq!(client.err(Call::LinesAdd(bad)), "RMD2003");
                let mut bad = spec(4, "P4");
                bad.unknown_command = Some("maybe".to_string());
                assert_eq!(client.err(Call::LinesAdd(bad)), "RMD2003");
                // None of the refused lines was added, in the service or in
                // the file.
                assert_eq!(client.lines().len(), 3);
                assert!(!fixture.config_text().contains("id = 4"));
            }
            "lines.edit" => {
                let mut edit = change(3);
                edit.name = Some(String::new());
                edit.port = Some("P9".to_string());
                edit.enabled = Some(false);
                edit.auto_answer_rings = Some(5);
                edit.unknown_command = Some("ok".to_string());
                edit.backend = Some("com0com".to_string());
                let info: LineInfo = client.ok(Call::LinesEdit(edit));
                assert_eq!(info.name, None);
                assert_eq!(info.port, "P9");
                assert!(!info.enabled);
                assert_eq!(info.auto_answer_rings, 5);
                assert_eq!(info.unknown_command, "ok");
                assert!(fixture.config_text().contains("port = \"P9\""));

                // Only the keys given change.
                let mut rename = change(3);
                rename.name = Some("Renamed".to_string());
                let info: LineInfo = client.ok(Call::LinesEdit(rename));
                assert_eq!(info.name.as_deref(), Some("Renamed"));
                assert_eq!(info.port, "P9");

                assert_eq!(client.err(Call::LinesEdit(change(9))), "RMD0019");
                let mut clash = change(3);
                clash.port = Some("P1".to_string());
                assert_eq!(client.err(Call::LinesEdit(clash)), "RMD0017");
                let mut bad = change(3);
                bad.backend = Some("usb".to_string());
                assert_eq!(client.err(Call::LinesEdit(bad)), "RMD2003");
                assert_eq!(client.line(3).port, "P9");
            }
            "lines.remove" => {
                let done: Done = client.ok(Call::LinesRemove(line(3)));
                assert_eq!(done, Done {});
                assert_eq!(client.err(Call::LinesGet(line(3))), "RMD1001");
                assert_eq!(client.err(Call::LinesRemove(line(3))), "RMD0019");
                assert!(!fixture.config_text().contains("id = 3"));
                assert_eq!(client.lines().len(), 2);
            }
            "calls.list" => {
                let all: CallList = client.ok(Call::CallsList(CallsQuery { last: None }));
                // The answered call and the reset one, each seen from both
                // lines.
                assert_eq!(all.calls.len(), 4);
                let first = &all.calls[0];
                assert_eq!((first.line, first.direction), (1, Direction::Outgoing));
                assert_eq!(first.reason, EndReason::Operator);
                assert!(first.connected_ms.is_some());
                let last: CallList = client.ok(Call::CallsList(CallsQuery { last: Some(1) }));
                assert_eq!(last.calls, all.calls[3..]);
            }
            "config.reload" => {
                let before = fixture.config_text();
                let added = format!(
                    "{before}\n# added by hand\n[[line]]\nid = 7\nserial = {{ backend = \"com0com\", port = \"P7\" }}\n"
                );
                std::fs::write(&fixture.config_path, &added).unwrap();
                std::thread::sleep(Duration::from_millis(5));
                let reloaded: Reloaded = client.ok(Call::ConfigReload);
                assert_eq!(reloaded.lines, 3);
                let info: ServiceInfo = client.ok(Call::ServiceInfo);
                assert!(info.config_loaded_ms > info.started_ms);
                assert_eq!(client.line(7).port, "P7");

                // A file that is not valid changes nothing.
                std::fs::write(&fixture.config_path, "[[line]]\nid = \n").unwrap();
                assert_eq!(client.err(Call::ConfigReload), "RMD0012");
                std::fs::write(&fixture.config_path, "[[line]]\nid = 1\n").unwrap();
                assert_eq!(client.err(Call::ConfigReload), "RMD0015");
                std::fs::remove_file(&fixture.config_path).unwrap();
                assert_eq!(client.err(Call::ConfigReload), "RMD0010");
                assert_eq!(client.lines().len(), 3);

                // A later change through the API keeps what was added by
                // hand, comment included.
                std::fs::write(&fixture.config_path, &added).unwrap();
                client.ok::<Reloaded>(Call::ConfigReload);
                client.ok::<Done>(Call::LinesRemove(line(7)));
                let text = fixture.config_text();
                assert!(!text.contains("id = 7"), "{text}");
                assert_eq!(client.lines().len(), 2);
            }
            "config.get" => {
                let settings: Settings = client.ok(Call::ConfigGet);
                assert_eq!(settings.service.log_level, LogLevel::Info);
                assert_eq!(settings.service.log_dir, "");
                assert_eq!(settings.control.pipe, "rustmodem-control");
                assert_eq!(settings.path, fixture.config_path.display().to_string());
            }
            "config.set" => {
                let before: Settings = client.ok(Call::ConfigGet);
                let set = |log_level, log_dir: Option<&str>, pipe: Option<&str>| {
                    Call::ConfigSet(SettingsChange {
                        service: Some(ServiceChange {
                            log_level,
                            log_dir: log_dir.map(str::to_string),
                        }),
                        control: pipe.map(|pipe| ControlChange {
                            pipe: Some(pipe.to_string()),
                        }),
                    })
                };
                let result: SettingsResult =
                    client.ok(set(Some(LogLevel::Debug), Some("logs-here"), Some("other")));
                assert!(result.restart_required);
                assert_eq!(result.config.service.log_level, LogLevel::Debug);
                assert_eq!(result.config.service.log_dir, "logs-here");
                assert_eq!(result.config.control.pipe, "other");
                assert_eq!(result.config.path, before.path);
                let text = fixture.config_text();
                assert!(text.contains("log_level = \"debug\""), "{text}");
                assert!(text.contains("pipe = \"other\""), "{text}");
                assert_eq!(client.ok::<Settings>(Call::ConfigGet), result.config);
                // The service goes on as it started: the pipe it reports is
                // the one it listens on.
                assert_eq!(client.ok::<ServiceInfo>(Call::ServiceInfo).pipe, PIPE);

                // Only the keys given change.
                let result: SettingsResult = client.ok(set(Some(LogLevel::Warning), None, None));
                assert_eq!(result.config.service.log_dir, "logs-here");
                assert_eq!(result.config.control.pipe, "other");

                // A value that is not allowed changes nothing.
                assert_eq!(
                    client.err(set(Some(LogLevel::Error), None, Some("not a pipe"))),
                    "RMD0014"
                );
                assert_eq!(client.ok::<Settings>(Call::ConfigGet), result.config);
                let refused = client.exchange(
                    r#"{"id":900,"method":"config.set","params":{"service":{"log_level":"loud"}}}"#,
                );
                assert!(
                    matches!(&refused, ServerFrame::Error { error, .. } if error.msgid == "RMD2003"),
                    "{refused:?}"
                );

                // Back as the service started, nothing needs a restart;
                // and a change that names nothing is no change.
                let result: SettingsResult = client.ok(set(
                    Some(LogLevel::Info),
                    Some(""),
                    Some("rustmodem-control"),
                ));
                assert!(!result.restart_required);
                assert_eq!(result.config, before);
                let result: SettingsResult = client.ok(Call::ConfigSet(SettingsChange::default()));
                assert!(!result.restart_required);
                // The lines were not touched.
                assert_eq!(client.lines().len(), 2);
            }
            "stats.get" => {
                // So far: one call answered and hung up, and one the caller
                // was reset out of while line 2 rang.
                let stats: Stats = client.ok(Call::StatsGet);
                assert_eq!(stats.calls_answered, 1);
                assert_eq!(stats.calls_connected, 1);
                assert_eq!(stats.calls_abandoned, 1);
                assert_eq!((stats.calls_failed, stats.calls_rejected), (0, 0));
                assert_eq!((stats.lines_in_use, stats.peak_lines_in_use), (0, 2));
                assert_eq!(stats.at_commands, 2);
                assert_eq!(stats.connects.len(), 1);
                assert_eq!((stats.connects[0].rate, stats.connects[0].count), (0, 1));
                assert_eq!(stats.retrains, 0);
                assert!(stats.protocols.is_empty() && stats.compressions.is_empty());
            }
            "log.tail" => {
                let all: LogTail = client.ok(Call::LogTail(LogTailQuery { last: None }));
                assert!(all.lines.len() > 10 && all.lines.len() <= LOG_RING);
                assert!(all.lines.is_sorted_by_key(|line| line.time_ms));
                assert!(
                    all.lines
                        .iter()
                        .any(|line| line.source == "Line02" && line.text.starts_with("RMD1005"))
                );
                let last: LogTail = client.ok(Call::LogTail(LogTailQuery { last: Some(3) }));
                assert_eq!(last.lines, all.lines[all.lines.len() - 3..]);
                let none: LogTail = client.ok(Call::LogTail(LogTailQuery { last: Some(0) }));
                assert!(none.lines.is_empty());
            }
            "ports.list" => {
                let list: PortList = client.ok(Call::PortsList);
                let names: Vec<&str> = list.ports.iter().map(|p| p.name.as_str()).collect();
                assert_eq!(names, ["COM3", "COM10", "p1", "P2"]);
                let lines: Vec<Option<u16>> = list.ports.iter().map(|p| p.line).collect();
                assert_eq!(lines, [None, None, Some(1), Some(2)]);
                let first = &list.ports[2];
                assert_eq!(first.kind, PortKind::Com0com);
                assert_eq!(first.pair.as_deref(), Some("A1"));
                assert_eq!(first.host_port.as_deref(), Some("COM11"));
                assert_eq!(first.modem.as_deref(), Some("Standard 56000 bps Modem"));
                assert_eq!(list.ports[0].kind, PortKind::Serial);
            }
            "subscribe" => {
                let sub: Subscription = client.ok(Call::Subscribe(selection(&["line.state"])));
                assert_eq!(sub.events, ["line.state"]);
                fixture.host("P2").set_dtr(false);
                let Event::LineState(info) = client.next_event() else {
                    panic!("expected a line.state event");
                };
                assert_eq!((info.id, info.state), (2, LineState::NotReady));
                fixture.host("P2").set_dtr(true);
                let Event::LineState(info) = client.next_event() else {
                    panic!("expected a line.state event");
                };
                assert_eq!((info.id, info.state), (2, LineState::Idle));

                // With no names, every event.
                let sub: Subscription = client.ok(Call::Subscribe(EventSelection::default()));
                assert_eq!(sub.events, EVENT_NAMES);
                assert_eq!(
                    client.err(Call::Subscribe(selection(&["line.gone"]))),
                    "RMD2003"
                );
            }
            "unsubscribe" => {
                let sub: Subscription = client.ok(Call::Unsubscribe(selection(&["log"])));
                assert_eq!(
                    sub.events,
                    ["line.state", "call.started", "call.connected", "call.ended"]
                );
                assert_eq!(
                    client.err(Call::Unsubscribe(selection(&["nope"]))),
                    "RMD2003"
                );
                let sub: Subscription = client.ok(Call::Unsubscribe(EventSelection::default()));
                assert!(sub.events.is_empty());

                // Nothing arrives any more.
                client.events.clear();
                fixture.host("P2").set_dtr(false);
                client.wait_for_state(2, LineState::NotReady);
                fixture.host("P2").set_dtr(true);
                client.wait_for_state(2, LineState::Idle);
                assert!(client.events.is_empty(), "{:?}", client.events);
            }
            other => panic!("method {other} has no round-trip test"),
        }
    }
}

#[test]
fn a_client_of_another_protocol_is_told_so_and_disconnected() {
    let fixture = Fixture::new(0);
    let mut client = fixture.connect_silent();
    let Some(ServerFrame::Error { id, error }) = client.hello(PROTOCOL + 1) else {
        panic!("expected an error");
    };
    assert_eq!(id, None);
    assert_eq!(error.msgid, "RMD2001");
    assert!(error.text.contains(&(PROTOCOL + 1).to_string()));
    assert!(
        error
            .text
            .contains(&format!("supports protocol {PROTOCOL}"))
    );
    assert_eq!(
        client.read_frame(),
        None,
        "the service closed the connection"
    );
}

#[test]
fn a_client_that_does_not_say_hello_first_is_disconnected() {
    let fixture = Fixture::new(0);
    for first in [
        r#"{"id":1,"method":"lines.list"}"#,
        "not json at all",
        r#"{"hello":"there"}"#,
    ] {
        let mut client = fixture.connect_silent();
        client.send_line(first);
        let Some(ServerFrame::Error { id, error }) = client.read_frame() else {
            panic!("expected an error for {first}");
        };
        assert_eq!(id, None);
        assert_eq!(error.msgid, "RMD2005");
        assert_eq!(client.read_frame(), None);
    }
    // A client that connects and leaves without a word is no trouble.
    drop(fixture.connect_silent());
    let mut client = fixture.connect();
    client.ok::<LineList>(Call::LinesList);
}

#[test]
fn a_bad_request_gets_an_error_and_the_connection_stays_up() {
    let fixture = Fixture::new(1);
    let mut client = fixture.connect();

    let cases = [
        ("this is not json", None, "RMD2004"),
        (r#"{"method":"lines.list"}"#, None, "RMD2004"),
        (r#"{"id":"seven","method":"lines.list"}"#, None, "RMD2004"),
        (r#"{"id":5}"#, Some(5), "RMD2004"),
        (r#"{"id":6,"method":"lines.explode"}"#, Some(6), "RMD2002"),
        (r#"{"id":7,"method":"lines.get"}"#, Some(7), "RMD2003"),
        (
            r#"{"id":8,"method":"lines.get","params":{"id":"x"}}"#,
            Some(8),
            "RMD2003",
        ),
        (
            r#"{"id":9,"method":"lines.get","params":{"id":1,"extra":2}}"#,
            Some(9),
            "RMD2003",
        ),
        (
            r#"{"id":10,"method":"lines.get","params":[1]}"#,
            Some(10),
            "RMD2003",
        ),
        (
            r#"{"hello":{"protocol":1,"version":"again"}}"#,
            None,
            "RMD2004",
        ),
    ];
    for (request, want_id, want_error) in cases {
        let ServerFrame::Error { id, error } = client.exchange(request) else {
            panic!("expected an error for {request}");
        };
        assert_eq!(id, want_id, "{request}");
        assert_eq!(error.msgid, want_error, "{request}");
    }
    // Blank lines and a carriage return before the line feed are tolerated.
    client.send_line("");
    client.send_line("   ");
    let frame = client.exchange("{\"id\":11,\"method\":\"lines.list\"}\r");
    assert!(matches!(frame, ServerFrame::Result { id: 11, .. }));
    // Bytes that are not UTF-8.
    use std::io::Write;
    client.writer.write_all(&[0xff, 0xfe, b'\n']).unwrap();
    let ServerFrame::Error { id: None, error } = client.next_reply() else {
        panic!("expected an error");
    };
    assert_eq!(error.msgid, "RMD2004");
    assert_eq!(client.line(1).id, 1);
}

#[test]
fn a_message_that_is_too_long_is_refused_and_the_client_disconnected() {
    let fixture = Fixture::new(0);
    let mut client = fixture.connect();
    let long = format!(
        r#"{{"id":1,"method":"lines.list","params":{{"pad":"{}"}}}}"#,
        "x".repeat(ctlproto::MAX_MESSAGE_BYTES)
    );
    // The service stops reading part of the way through, so the write may
    // not finish; either way the answer is the error.
    let mut writer = std::mem::replace(
        &mut client.writer,
        fixture.connector.connect().unwrap().writer,
    );
    let sender = std::thread::spawn(move || {
        use std::io::Write;
        let _ = writer.write_all(long.as_bytes());
        let _ = writer.write_all(b"\n");
    });
    let Some(ServerFrame::Error { id: None, error }) = client.read_frame() else {
        panic!("expected an error");
    };
    assert_eq!(error.msgid, "RMD2014");
    assert_eq!(client.read_frame(), None);
    sender.join().unwrap();
}

#[test]
fn a_call_is_reported_event_by_event() {
    let fixture = Fixture::new(2);
    let mut client = fixture.connect();
    client.ok::<Subscription>(Call::Subscribe(selection(&[
        "line.state",
        "call.started",
        "call.connected",
        "call.ended",
    ])));

    fixture.host("P1").write(b"ATE0D2\r");
    let Event::CallStarted(CallEvent { line: 1, call }) = client.next_event() else {
        panic!("expected call.started on line 1");
    };
    assert_eq!(
        (call.direction, call.number.as_str()),
        (Direction::Outgoing, "2")
    );
    let Event::CallStarted(CallEvent { line: 2, call }) = client.next_event() else {
        panic!("expected call.started on line 2");
    };
    assert_eq!(
        (call.direction, call.number.as_str()),
        (Direction::Incoming, "1")
    );
    for (id, state) in [(1, LineState::Handshaking), (2, LineState::Ringing)] {
        let Event::LineState(info) = client.next_event() else {
            panic!("expected line.state");
        };
        assert_eq!((info.id, info.state), (id, state));
    }

    client.ok::<LineInfo>(Call::LinesAnswer(line(2)));
    for id in [1, 2] {
        let Event::CallConnected(CallEvent { line, call }) = client.next_event() else {
            panic!("expected call.connected");
        };
        assert_eq!(line, id);
        assert_eq!(call.connect.unwrap().rate, 0);
    }
    for id in [1, 2] {
        let Event::LineState(info) = client.next_event() else {
            panic!("expected line.state");
        };
        assert_eq!((info.id, info.state), (id, LineState::Online));
    }

    client.ok::<LineInfo>(Call::LinesHangup(line(2)));
    let Event::CallEnded(record) = client.next_event() else {
        panic!("expected call.ended");
    };
    assert_eq!((record.line, record.reason), (2, EndReason::Operator));
    let Event::CallEnded(record) = client.next_event() else {
        panic!("expected call.ended");
    };
    assert_eq!((record.line, record.reason), (1, EndReason::Remote));
}

#[test]
fn log_events_reach_a_client_that_asks_for_them() {
    let fixture = Fixture::new(1);
    let mut client = fixture.connect();
    client.ok::<Subscription>(Call::Subscribe(selection(&["log"])));
    fixture.host("P1").write(b"ATE0\r");
    let Event::Log(log) = client.wait_for_event(|e| matches!(e, Event::Log(l) if l.text == "< OK"))
    else {
        unreachable!();
    };
    assert_eq!(log.level, LogLevel::Debug);
    assert_eq!(log.source, "Line01");
}

#[test]
fn each_client_has_its_own_subscription() {
    let fixture = Fixture::new(1);
    let mut watcher = fixture.connect();
    let mut other = fixture.connect();
    watcher.ok::<Subscription>(Call::Subscribe(selection(&["line.state"])));

    fixture.host("P1").set_dtr(false);
    assert!(matches!(watcher.next_event(), Event::LineState(_)));
    other.wait_for_state(1, LineState::NotReady);
    assert!(other.events.is_empty());
    assert_eq!(fixture.hub.client_count(), 2);

    drop(watcher);
    drop(other);
    eventually("both clients to be forgotten", || {
        fixture.hub.client_count() == 0
    });
    assert!(fixture.logged("RMD2011: Control client"));
    eventually("the disconnects to be logged", || {
        fixture.logged("RMD2012: Control client")
    });
}

fn log_event(number: u64) -> Event {
    Event::Log(LogEvent {
        time_ms: number,
        level: LogLevel::Info,
        source: "TEST".to_string(),
        text: format!("log line {number}"),
    })
}

fn state_event(number: u64) -> Event {
    Event::CallEnded(CallRecord {
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
    })
}

/// A client on a pipe so small that the service can hardly write to it,
/// subscribed to everything and not reading.
fn slow_client(fixture: &Fixture) -> TestClient {
    let mut client = TestClient::new(fixture.connector.connect_with_capacity(200).unwrap());
    assert!(matches!(
        client.hello(PROTOCOL),
        Some(ServerFrame::Hello { .. })
    ));
    client.ok::<Subscription>(Call::Subscribe(EventSelection::default()));
    client
}

#[test]
fn a_slow_subscriber_loses_old_log_events_but_no_state_events() {
    let limits = Limits {
        max_queued: 100,
        max_logs: 10,
    };
    let fixture = Fixture::with_limits(0, limits);
    let mut client = slow_client(&fixture);

    // Far more than the client's queue holds, while it reads nothing.
    // Publishing must not wait for it: this is the pool's thread in the
    // service.
    let started = Instant::now();
    let mut number = 0;
    for state in 1..=50 {
        for _ in 0..200 {
            number += 1;
            fixture.hub.publish(&log_event(number));
        }
        fixture.hub.publish(&state_event(state));
    }
    let last_log = number;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "publishing took {:?}; it must not block on a slow client",
        started.elapsed()
    );

    // Now the client reads. A call marks the end: its answer is queued
    // behind everything published before it.
    let mut states = Vec::new();
    let mut logs = Vec::new();
    client.ok::<LineList>(Call::LinesList);
    for event in client.events.drain(..) {
        match event {
            Event::CallEnded(record) => states.push(record.id),
            Event::Log(log) => logs.push(log.time_ms),
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert_eq!(
        states,
        (1..=50).collect::<Vec<u64>>(),
        "every state event, in order"
    );
    // Of ten thousand log lines only a queue's worth arrived, plus the few
    // that were already on their way when the queue filled.
    assert!(
        logs.len() <= limits.max_logs + 8,
        "{} log events arrived",
        logs.len()
    );
    assert!(logs.len() >= limits.max_logs);
    assert!(logs.is_sorted());
    assert_eq!(
        logs.last(),
        Some(&last_log),
        "the newest log lines are the ones kept"
    );
    assert!(!fixture.logged("RMD2006"));
}

#[test]
fn a_subscriber_that_falls_too_far_behind_is_disconnected() {
    let limits = Limits {
        max_queued: 100,
        max_logs: 10,
    };
    let fixture = Fixture::with_limits(0, limits);
    let mut client = slow_client(&fixture);
    let mut bystander = fixture.connect();
    bystander.ok::<Subscription>(Call::Subscribe(selection(&["call.ended"])));

    let started = Instant::now();
    for state in 1..=500 {
        fixture.hub.publish(&state_event(state));
        // The bystander keeps up, so it is not affected.
        if state % 50 == 0 {
            bystander.collect_events(state as usize);
        }
    }
    assert!(started.elapsed() < Duration::from_secs(5));

    // The slow client is cut off rather than sent a state with holes in it.
    // The connection is closed under the writer, so the last line may be
    // cut short; whole lines are whole events.
    let mut delivered = 0;
    loop {
        use std::io::BufRead;
        let mut text = String::new();
        let read = client.reader.read_line(&mut text).unwrap();
        if read == 0 || !text.ends_with('\n') {
            break;
        }
        let frame = ctlproto::decode_server(text.trim_end()).unwrap();
        assert!(matches!(frame, ServerFrame::Event(Event::CallEnded(_))));
        delivered += 1;
    }
    assert!(delivered < 500);
    eventually("the disconnect to be logged", || fixture.logged("RMD2006"));
    eventually("the slow client to be forgotten", || {
        fixture.hub.client_count() == 1
    });

    assert_eq!(bystander.events.len(), 500);
    bystander.ok::<LineList>(Call::LinesList);
}

#[test]
fn a_change_that_cannot_be_saved_changes_nothing() {
    let fixture = Fixture::new(1);
    let mut client = fixture.connect();
    // The directory of the configuration file goes away.
    std::fs::remove_dir_all(&fixture.directory).unwrap();
    assert_eq!(client.err(Call::LinesAdd(spec(2, "P2"))), "RMD0011");
    assert_eq!(client.err(Call::LinesDisable(line(1))), "RMD0011");
    let lines = client.lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].enabled);
}

#[test]
fn requests_are_refused_once_the_pool_has_stopped() {
    let fixture = Fixture::new(1);
    let mut client = fixture.connect();
    let server = fixture.server.clone();
    let connector = fixture.connector.clone();
    // Dropping the fixture stops the pool's thread; the server lives on in
    // its own threads for as long as there is a connector.
    drop(fixture);
    assert_eq!(client.err(Call::LinesList), "RMD2010");
    assert_eq!(client.err(Call::LinesAnswer(line(1))), "RMD2010");
    drop(server);
    drop(connector);
}

#[test]
fn the_log_tail_holds_the_last_two_thousand_lines() {
    let fixture = Fixture::new(0);
    let mut client = fixture.connect();
    let before = client
        .ok::<LogTail>(Call::LogTail(LogTailQuery::default()))
        .lines
        .len();
    assert!(before < 100, "{before} lines from an empty service");
    // Nobody is subscribed: the lines are remembered all the same.
    for number in 1..=(LOG_RING as u64 + 500) {
        fixture.hub.publish(&log_event(number));
    }
    let tail: LogTail = client.ok(Call::LogTail(LogTailQuery::default()));
    assert_eq!(tail.lines.len(), LOG_RING);
    assert_eq!(tail.lines[0].time_ms, 501);
    assert_eq!(tail.lines[LOG_RING - 1].text, "log line 2500");
    let last: LogTail = client.ok(Call::LogTail(LogTailQuery { last: Some(2) }));
    let numbers: Vec<u64> = last.lines.iter().map(|line| line.time_ms).collect();
    assert_eq!(numbers, [2499, 2500]);
    // Asking for more than there is gives what there is.
    let all: LogTail = client.ok(Call::LogTail(LogTailQuery { last: Some(99_999) }));
    assert_eq!(all.lines.len(), LOG_RING);
}

#[test]
fn lines_show_what_their_ports_are_bound_to() {
    let fixture = Fixture::new(3);
    let mut client = fixture.connect();
    assert_eq!(client.line(1).binding, None);

    client.ok::<Subscription>(Call::Subscribe(selection(&["line.state"])));
    fixture.server.refresh_bindings().unwrap();
    // The lines whose binding became known say so; line 3 is on a port the
    // inventory does not have.
    let mut told = Vec::new();
    for _ in 0..2 {
        let Event::LineState(info) = client.next_event() else {
            panic!("expected line.state");
        };
        told.push((info.id, info.binding.clone()));
    }
    assert_eq!(
        told,
        [
            (1, Some("Standard 56000 bps Modem on COM11".to_string())),
            (2, Some("COM12".to_string())),
        ]
    );
    assert_eq!(client.line(3).binding, None);

    // A line that moves to a port takes the binding of that port, and
    // reloading the configuration looks the ports up again.
    let mut moved = change(3);
    moved.port = Some("P2x".to_string());
    assert_eq!(client.ok::<LineInfo>(Call::LinesEdit(moved)).binding, None);
    client.ok::<Reloaded>(Call::ConfigReload);
    assert_eq!(client.line(2).binding.as_deref(), Some("COM12"));
}

#[test]
fn a_service_that_can_apply_its_settings_needs_no_restart_for_them() {
    let fixture = Fixture::new(1);
    let mut client = fixture.connect();
    let level = |level| {
        Call::ConfigSet(SettingsChange {
            service: Some(ServiceChange {
                log_level: Some(level),
                log_dir: None,
            }),
            control: None,
        })
    };

    // A service that takes its settings up only when it starts.
    let result: SettingsResult = client.ok(level(LogLevel::Debug));
    assert!(result.restart_required);
    assert_eq!(result.config.service.log_level, LogLevel::Debug);

    // One that puts them into effect: the change that was waiting is in
    // effect with the next one, and nothing waits for a restart.
    fixture
        .live
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let result: SettingsResult = client.ok(level(LogLevel::Warning));
    assert!(!result.restart_required);
    assert_eq!(result.config.service.log_level, LogLevel::Warning);
    let text = fixture.config_text();
    assert!(text.contains("log_level = \"warning\""), "{text}");

    // The control pipe is made when the service starts, whatever the
    // service can do while it runs.
    let result: SettingsResult = client.ok(Call::ConfigSet(SettingsChange {
        service: None,
        control: Some(ControlChange {
            pipe: Some("other".to_string()),
        }),
    }));
    assert!(result.restart_required);
    // And back: nothing is waiting any more.
    let result: SettingsResult = client.ok(Call::ConfigSet(SettingsChange {
        service: Some(ServiceChange {
            log_level: Some(LogLevel::Error),
            log_dir: None,
        }),
        control: Some(ControlChange {
            pipe: Some("rustmodem-control".to_string()),
        }),
    }));
    assert!(!result.restart_required);
}
