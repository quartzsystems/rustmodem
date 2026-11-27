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
    Call, CallEvent, CallList, CallRecord, CallsQuery, CaptureSwitch, ControlChange, Direction,
    Done, EVENT_NAMES, EndReason, Event, EventSelection, HuntGroupChange, HuntGroupInfo,
    HuntGroupList, HuntGroupRef, HuntGroupSpec, LineChange, LineDiag, LineInfo, LineList, LineRef,
    LineSpec, LineState, ListenerState, LogEvent, LogLevel, LogTail, LogTailQuery, MediaChange,
    PROTOCOL, PortKind, PortList, RegisterChange, RegisterSpec, Reloaded, ReregisterQuery,
    RouteChange, RouteList, RouteMove, RouteRef, RouteSpec, SelfTest, ServerFrame, ServiceChange,
    ServiceInfo, ServiceMode, Settings, SettingsChange, SettingsResult, SipChange, SipMode,
    SipStatus, Stats, Subscription,
};
use ctlsrv::{LOG_RING, Limits};

fn line(id: u16) -> LineRef {
    LineRef { id }
}

fn selection(events: &[&str]) -> EventSelection {
    EventSelection {
        events: Some(events.iter().map(|e| e.to_string()).collect()),
        line: None,
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
        engine: None,
        register: None,
        modem: None,
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
        engine: None,
        register: None,
        modem: None,
    }
}

/// Line 1 dials line 2 and the test waits until line 2 is ringing.
async fn ring_line_2(fixture: &Fixture, client: &mut TestClient) {
    fixture.host("P1").await.write(b"ATE0D2\r");
    client.wait_for_state(2, LineState::Ringing).await;
}

/// Exit criterion 3 of phase 0: every method in docs/CONTROL_API.md has a
/// round-trip test. The methods are taken from the list the documentation
/// is generated from, so a method added there without a case here fails.
#[tokio::test(start_paused = true)]
async fn every_method_round_trips() {
    let fixture = Fixture::new(2).await;
    let mut client = fixture.connect().await;

    for method in Call::METHODS {
        match method {
            "service.info" => {
                let info: ServiceInfo = client.ok(Call::ServiceInfo).await;
                assert_eq!(info.version, VERSION);
                assert_eq!(info.protocol, PROTOCOL);
                assert_eq!(info.pipe, PIPE);
                assert_eq!(info.config_path, fixture.config_path.display().to_string());
                assert_eq!(info.lines, 2);
                assert!(info.started_ms > 1_600_000_000_000);
                assert_eq!(info.config_loaded_ms, info.started_ms);
                assert_eq!(info.mode, ServiceMode::Console);
                let later: ServiceInfo = client.ok(Call::ServiceInfo).await;
                assert!(later.uptime_ms >= info.uptime_ms);
            }
            "lines.list" => {
                let list: LineList = client.ok(Call::LinesList).await;
                assert_eq!(list.lines.iter().map(|l| l.id).collect::<Vec<_>>(), [1, 2]);
                assert!(list.lines.iter().all(|l| l.state == LineState::Idle));
            }
            "lines.get" => {
                let info: LineInfo = client.ok(Call::LinesGet(line(2))).await;
                assert_eq!((info.id, info.port.as_str()), (2, "P2"));
                assert_eq!(info.backend, "com0com");
                assert_eq!(client.err(Call::LinesGet(line(9))).await, "RMD1001");
            }
            "lines.answer" => {
                assert_eq!(client.err(Call::LinesAnswer(line(2))).await, "RMD1003");
                assert_eq!(client.err(Call::LinesAnswer(line(9))).await, "RMD1001");
                ring_line_2(&fixture, &mut client).await;
                let info: LineInfo = client.ok(Call::LinesAnswer(line(2))).await;
                assert_eq!(info.state, LineState::Online);
                assert!(info.call.is_some());
                client.wait_for_state(1, LineState::Online).await;
            }
            "lines.hangup" => {
                // The call the answer case set up is still connected.
                let info: LineInfo = client.ok(Call::LinesHangup(line(1))).await;
                assert_eq!(info.state, LineState::Idle);
                assert!(info.call.is_none());
                client.wait_for_state(2, LineState::Idle).await;
                assert_eq!(client.err(Call::LinesHangup(line(1))).await, "RMD1004");
                assert_eq!(client.err(Call::LinesHangup(line(9))).await, "RMD1001");
            }
            "lines.reset" => {
                ring_line_2(&fixture, &mut client).await;
                let info: LineInfo = client.ok(Call::LinesReset(line(1))).await;
                assert_eq!(info.state, LineState::Idle);
                client.wait_for_state(2, LineState::Idle).await;
                assert_eq!(client.err(Call::LinesReset(line(9))).await, "RMD1001");
            }
            "lines.enable" => {
                client.ok::<LineInfo>(Call::LinesDisable(line(2))).await;
                let info: LineInfo = client.ok(Call::LinesEnable(line(2))).await;
                assert!(info.enabled);
                assert_ne!(info.state, LineState::Disabled);
                assert!(fixture.config_text().contains("enabled = true"));
                assert_eq!(client.err(Call::LinesEnable(line(9))).await, "RMD0019");
                // The port was released and is opened again by the pool's
                // thread, a moment after the answer; its host has to open
                // the new port, not the one that was released.
                eventually("the port to be opened again", || {
                    fixture.hosts.lock().unwrap()["P2"].backend_alive()
                })
                .await;
                fixture.host("P2").await.open();
                client.wait_for_state(2, LineState::Idle).await;
            }
            "lines.disable" => {
                let old_host = fixture.host("P2").await;
                let info: LineInfo = client.ok(Call::LinesDisable(line(2))).await;
                assert!(!info.enabled);
                assert_eq!(info.state, LineState::Disabled);
                assert!(fixture.config_text().contains("enabled = false"));
                assert!(!old_host.backend_alive(), "the port was released");
                assert_eq!(client.err(Call::LinesDisable(line(9))).await, "RMD0019");
                client.ok::<LineInfo>(Call::LinesEnable(line(2))).await;
                eventually("the port to be opened again", || {
                    fixture.hosts.lock().unwrap()["P2"].backend_alive()
                })
                .await;
                fixture.host("P2").await.open();
                client.wait_for_state(2, LineState::Idle).await;
            }
            "lines.add" => {
                let mut new = spec(3, "P3");
                new.name = Some("Three".to_string());
                new.backend = Some("com0com".to_string());
                new.enabled = Some(true);
                new.auto_answer_rings = Some(2);
                new.unknown_command = Some("error".to_string());
                let info: LineInfo = client.ok(Call::LinesAdd(new.clone())).await;
                assert_eq!(info.id, 3);
                assert_eq!(info.name.as_deref(), Some("Three"));
                assert_eq!(info.auto_answer_rings, 2);
                assert_eq!(info.unknown_command, "error");
                assert_eq!(client.lines().await.len(), 3);
                let text = fixture.config_text();
                assert!(
                    text.contains("id = 3") && text.contains("port = \"P3\""),
                    "{text}"
                );

                assert_eq!(client.err(Call::LinesAdd(new)).await, "RMD0020");
                assert_eq!(client.err(Call::LinesAdd(spec(4, "p3"))).await, "RMD0017");
                assert_eq!(client.err(Call::LinesAdd(spec(0, "P4"))).await, "RMD0014");
                assert_eq!(
                    client.err(Call::LinesAdd(spec(4, "bad port"))).await,
                    "RMD0014"
                );
                let mut bad = spec(4, "P4");
                bad.backend = Some("usb".to_string());
                assert_eq!(client.err(Call::LinesAdd(bad)).await, "RMD2003");
                let mut bad = spec(4, "P4");
                bad.unknown_command = Some("maybe".to_string());
                assert_eq!(client.err(Call::LinesAdd(bad)).await, "RMD2003");
                // None of the refused lines was added, in the service or in
                // the file.
                assert_eq!(client.lines().await.len(), 3);
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
                let info: LineInfo = client.ok(Call::LinesEdit(edit)).await;
                assert_eq!(info.name, None);
                assert_eq!(info.port, "P9");
                assert!(!info.enabled);
                assert_eq!(info.auto_answer_rings, 5);
                assert_eq!(info.unknown_command, "ok");
                assert!(fixture.config_text().contains("port = \"P9\""));

                // Only the keys given change.
                let mut rename = change(3);
                rename.name = Some("Renamed".to_string());
                let info: LineInfo = client.ok(Call::LinesEdit(rename)).await;
                assert_eq!(info.name.as_deref(), Some("Renamed"));
                assert_eq!(info.port, "P9");

                assert_eq!(client.err(Call::LinesEdit(change(9))).await, "RMD0019");
                let mut clash = change(3);
                clash.port = Some("P1".to_string());
                assert_eq!(client.err(Call::LinesEdit(clash)).await, "RMD0017");
                let mut bad = change(3);
                bad.backend = Some("usb".to_string());
                assert_eq!(client.err(Call::LinesEdit(bad)).await, "RMD2003");
                assert_eq!(client.line(3).await.port, "P9");
            }
            "lines.remove" => {
                let done: Done = client.ok(Call::LinesRemove(line(3))).await;
                assert_eq!(done, Done {});
                assert_eq!(client.err(Call::LinesGet(line(3))).await, "RMD1001");
                assert_eq!(client.err(Call::LinesRemove(line(3))).await, "RMD0019");
                assert!(!fixture.config_text().contains("id = 3"));
                assert_eq!(client.lines().await.len(), 2);
            }
            "lines.diag" => {
                // A line with no call has no modem to show.
                let diag: LineDiag = client.ok(Call::LinesDiag(line(1))).await;
                assert_eq!((diag.line, &diag.modem), (1, &None));
                assert_eq!(diag.scope_kind, "none");
                assert!(diag.history.is_empty() && diag.scope.is_empty());
                assert_eq!(client.err(Call::LinesDiag(line(9))).await, "RMD1001");
            }
            "selftest.run" => {
                // The lines here have the null engine: `tests/modem.rs`
                // runs the test itself.
                for ((a, b), refused) in [
                    ((1, 1), "RMD7004"),
                    ((1, 9), "RMD1001"),
                    ((1, 2), "RMD7003"),
                ] {
                    assert_eq!(
                        client.err(Call::SelfTestRun(SelfTest { a, b })).await,
                        refused
                    );
                }
            }
            "lines.capture" => {
                let on: LineInfo = client
                    .ok(Call::LinesCapture(CaptureSwitch { id: 2, on: true }))
                    .await;
                assert!(on.capture);
                assert!(client.line(2).await.capture && !client.line(1).await.capture);
                let off: LineInfo = client
                    .ok(Call::LinesCapture(CaptureSwitch { id: 2, on: false }))
                    .await;
                assert!(!off.capture);
                assert_eq!(
                    client
                        .err(Call::LinesCapture(CaptureSwitch { id: 9, on: true }))
                        .await,
                    "RMD1001"
                );
                // It is not a setting of the configuration.
                assert!(!fixture.config_text().contains("capture"));
            }
            "sip.status" => {
                // This service has no SIP.
                let status: SipStatus = client.ok(Call::SipStatus).await;
                assert_eq!(status, SipStatus::default());
                assert_eq!(status.listener, ListenerState::Off);
            }
            "sip.reregister" => {
                let all = Call::SipReregister(ReregisterQuery { id: None });
                assert_eq!(client.ok::<Done>(all).await, Done {});
                let one = Call::SipReregister(ReregisterQuery { id: Some(1) });
                assert_eq!(client.ok::<Done>(one).await, Done {});
                assert_eq!(
                    client
                        .err(Call::SipReregister(ReregisterQuery { id: Some(9) }))
                        .await,
                    "RMD1001"
                );
            }
            "huntgroups.list" => {
                let list: HuntGroupList = client.ok(Call::HuntGroupsList).await;
                assert!(list.groups.is_empty());
            }
            "huntgroups.add" => {
                let group = |name: &str, pilot: &str, lines: &[u16], strategy: Option<&str>| {
                    Call::HuntGroupsAdd(HuntGroupSpec {
                        name: name.to_string(),
                        pilot: pilot.to_string(),
                        lines: lines.to_vec(),
                        strategy: strategy.map(str::to_string),
                    })
                };
                let added: HuntGroupInfo =
                    client.ok(group("dialup", "5551000", &[1, 2], None)).await;
                assert_eq!(added.strategy, "first-idle");
                assert_eq!(added.lines, [1, 2]);
                let list: HuntGroupList = client.ok(Call::HuntGroupsList).await;
                assert_eq!(list.groups, [added]);
                assert!(fixture.config_text().contains("[[hunt_group]]"));
                // The lines show the groups they are in.
                assert_eq!(client.line(1).await.hunt_groups, ["dialup"]);
                assert_eq!(client.line(2).await.hunt_groups, ["dialup"]);

                assert_eq!(
                    client.err(group("dialup", "1", &[1], None)).await,
                    "RMD0033"
                );
                assert_eq!(
                    client.err(group("b", "5551000", &[1], None)).await,
                    "RMD0029"
                );
                assert_eq!(
                    client.err(group("b", "5552000", &[9], None)).await,
                    "RMD0027"
                );
                assert_eq!(
                    client
                        .err(group("b", "5552000", &[1], Some("random")))
                        .await,
                    "RMD2003"
                );
                let second: HuntGroupInfo = client
                    .ok(group("night", "5552000", &[2], Some("round-robin")))
                    .await;
                assert_eq!(second.strategy, "round-robin");
                assert_eq!(client.line(2).await.hunt_groups, ["dialup", "night"]);
            }
            "huntgroups.edit" => {
                let edited: HuntGroupInfo = client
                    .ok(Call::HuntGroupsEdit(HuntGroupChange {
                        name: "night".to_string(),
                        new_name: Some("late".to_string()),
                        pilot: Some("5552001".to_string()),
                        lines: Some(vec![1]),
                        strategy: Some("least-recent".to_string()),
                    }))
                    .await;
                assert_eq!(edited.name, "late");
                assert_eq!(edited.pilot, "5552001");
                assert_eq!(edited.lines, [1]);
                assert_eq!(edited.strategy, "least-recent");
                assert_eq!(client.line(1).await.hunt_groups, ["dialup", "late"]);
                assert_eq!(client.line(2).await.hunt_groups, ["dialup"]);

                let nothing = |name: &str| HuntGroupChange {
                    name: name.to_string(),
                    new_name: None,
                    pilot: None,
                    lines: None,
                    strategy: None,
                };
                // Only the keys given change.
                let same: HuntGroupInfo = client.ok(Call::HuntGroupsEdit(nothing("late"))).await;
                assert_eq!(same, edited);
                assert_eq!(
                    client.err(Call::HuntGroupsEdit(nothing("night"))).await,
                    "RMD0032"
                );
                let mut taken = nothing("late");
                taken.new_name = Some("dialup".to_string());
                assert_eq!(client.err(Call::HuntGroupsEdit(taken)).await, "RMD0033");
                let mut bad = nothing("late");
                bad.strategy = Some("random".to_string());
                assert_eq!(client.err(Call::HuntGroupsEdit(bad)).await, "RMD0014");
            }
            "huntgroups.remove" => {
                let late = || {
                    Call::HuntGroupsRemove(HuntGroupRef {
                        name: "late".to_string(),
                    })
                };
                assert_eq!(client.ok::<Done>(late()).await, Done {});
                assert_eq!(client.err(late()).await, "RMD0032");
                let list: HuntGroupList = client.ok(Call::HuntGroupsList).await;
                assert_eq!(list.groups.len(), 1);
                assert_eq!(client.line(1).await.hunt_groups, ["dialup"]);
            }
            "routes.list" => {
                let list: RouteList = client.ok(Call::RoutesList).await;
                assert!(list.routes.is_empty());
            }
            "routes.add" => {
                let route = |called: &str, target: &str, index: Option<u32>| {
                    Call::RoutesAdd(RouteSpec {
                        called: called.to_string(),
                        target: target.to_string(),
                        index,
                    })
                };
                let list: RouteList = client.ok(route("555*", "group:dialup", None)).await;
                assert_eq!(list.routes.len(), 1);
                assert_eq!(list.routes[0].index, 0);
                let list: RouteList = client.ok(route("5551234", "line:2", Some(0))).await;
                let shown: Vec<(u32, &str, &str)> = list
                    .routes
                    .iter()
                    .map(|r| (r.index, r.called.as_str(), r.target.as_str()))
                    .collect();
                assert_eq!(
                    shown,
                    [(0, "5551234", "line:2"), (1, "555*", "group:dialup")]
                );
                assert_eq!(client.ok::<RouteList>(Call::RoutesList).await, list);
                assert!(fixture.config_text().contains("[[route]]"));

                assert_eq!(
                    client.err(route("9*", "group:nobody", None)).await,
                    "RMD0030"
                );
                assert_eq!(client.err(route("9*", "line:9", None)).await, "RMD0030");
                assert_eq!(client.err(route("9*", "trunk", None)).await, "RMD2003");
                assert_eq!(client.err(route("9*", "line:1", Some(3))).await, "RMD0034");
                // A group a route sends calls to cannot be removed.
                assert_eq!(
                    client
                        .err(Call::HuntGroupsRemove(HuntGroupRef {
                            name: "dialup".to_string()
                        }))
                        .await,
                    "RMD0030"
                );
            }
            "routes.edit" => {
                let list: RouteList = client
                    .ok(Call::RoutesEdit(RouteChange {
                        index: 0,
                        called: Some("5559999".to_string()),
                        target: Some("line:1".to_string()),
                    }))
                    .await;
                assert_eq!(list.routes[0].called, "5559999");
                assert_eq!(list.routes[0].target, "line:1");
                assert_eq!(list.routes[1].called, "555*");
                // Only the keys given change.
                let same: RouteList = client
                    .ok(Call::RoutesEdit(RouteChange {
                        index: 0,
                        called: None,
                        target: None,
                    }))
                    .await;
                assert_eq!(same, list);
                assert_eq!(
                    client
                        .err(Call::RoutesEdit(RouteChange {
                            index: 2,
                            called: None,
                            target: None,
                        }))
                        .await,
                    "RMD0034"
                );
                assert_eq!(
                    client
                        .err(Call::RoutesEdit(RouteChange {
                            index: 0,
                            called: None,
                            target: Some("nowhere".to_string()),
                        }))
                        .await,
                    "RMD0014"
                );
            }
            "routes.remove" => {
                // Left for after the move: the list is two routes here.
                let list: RouteList = client
                    .ok(Call::RoutesAdd(RouteSpec {
                        called: "900*".to_string(),
                        target: "line:2".to_string(),
                        index: None,
                    }))
                    .await;
                assert_eq!(list.routes.len(), 3);
                let list: RouteList = client.ok(Call::RoutesRemove(RouteRef { index: 2 })).await;
                assert_eq!(list.routes.len(), 2);
                assert_eq!(
                    client.err(Call::RoutesRemove(RouteRef { index: 2 })).await,
                    "RMD0034"
                );
            }
            "routes.move" => {
                let list: RouteList = client
                    .ok(Call::RoutesMove(RouteMove { index: 1, to: 0 }))
                    .await;
                let called: Vec<&str> = list.routes.iter().map(|r| r.called.as_str()).collect();
                assert_eq!(called, ["555*", "5559999"]);
                assert_eq!(client.ok::<RouteList>(Call::RoutesList).await, list);
                // The file has them in the order they are tried.
                let text = fixture.config_text();
                assert!(text.find("\"555*\"").unwrap() < text.find("\"5559999\"").unwrap());
                assert_eq!(
                    client
                        .err(Call::RoutesMove(RouteMove { index: 0, to: 2 }))
                        .await,
                    "RMD0034"
                );
                // What the groups and routes tests made is taken away
                // again, so that the tests after them find two bare lines.
                client
                    .ok::<RouteList>(Call::RoutesRemove(RouteRef { index: 0 }))
                    .await;
                client
                    .ok::<RouteList>(Call::RoutesRemove(RouteRef { index: 0 }))
                    .await;
                client
                    .ok::<Done>(Call::HuntGroupsRemove(HuntGroupRef {
                        name: "dialup".to_string(),
                    }))
                    .await;
                assert!(client.line(1).await.hunt_groups.is_empty());
            }
            "calls.list" => {
                let all: CallList = client.ok(Call::CallsList(CallsQuery { last: None })).await;
                // The answered call and the reset one, each seen from both
                // lines.
                assert_eq!(all.calls.len(), 4);
                let first = &all.calls[0];
                assert_eq!((first.line, first.direction), (1, Direction::Outgoing));
                assert_eq!(first.reason, EndReason::Operator);
                assert!(first.connected_ms.is_some());
                let last: CallList = client
                    .ok(Call::CallsList(CallsQuery { last: Some(1) }))
                    .await;
                assert_eq!(last.calls, all.calls[3..]);
            }
            "config.reload" => {
                let before = fixture.config_text();
                let added = format!(
                    "{before}\n# added by hand\n[[line]]\nid = 7\nserial = {{ backend = \"com0com\", port = \"P7\" }}\n"
                );
                std::fs::write(&fixture.config_path, &added).unwrap();
                std::thread::sleep(Duration::from_millis(5));
                let reloaded: Reloaded = client.ok(Call::ConfigReload).await;
                assert_eq!(reloaded.lines, 3);
                let info: ServiceInfo = client.ok(Call::ServiceInfo).await;
                assert!(info.config_loaded_ms > info.started_ms);
                assert_eq!(client.line(7).await.port, "P7");

                // A file that is not valid changes nothing.
                std::fs::write(&fixture.config_path, "[[line]]\nid = \n").unwrap();
                assert_eq!(client.err(Call::ConfigReload).await, "RMD0012");
                std::fs::write(&fixture.config_path, "[[line]]\nid = 1\n").unwrap();
                assert_eq!(client.err(Call::ConfigReload).await, "RMD0015");
                std::fs::remove_file(&fixture.config_path).unwrap();
                assert_eq!(client.err(Call::ConfigReload).await, "RMD0010");
                assert_eq!(client.lines().await.len(), 3);

                // A later change through the API keeps what was added by
                // hand, comment included.
                std::fs::write(&fixture.config_path, &added).unwrap();
                client.ok::<Reloaded>(Call::ConfigReload).await;
                client.ok::<Done>(Call::LinesRemove(line(7))).await;
                let text = fixture.config_text();
                assert!(!text.contains("id = 7"), "{text}");
                assert_eq!(client.lines().await.len(), 2);
            }
            "config.get" => {
                let settings: Settings = client.ok(Call::ConfigGet).await;
                assert_eq!(settings.service.log_level, LogLevel::Info);
                assert_eq!(settings.service.log_dir, "");
                assert_eq!(settings.control.pipe, "rustmodem-control");
                assert_eq!(settings.path, fixture.config_path.display().to_string());
                // The defaults of a file that says nothing of SIP.
                assert!(!settings.sip.enabled);
                assert_eq!(settings.sip.listen, "0.0.0.0:5060");
                assert!(settings.sip.trusted_peers.is_empty());
                assert_eq!(settings.sip.ring_timeout_s, 60);
                assert_eq!(settings.media.codecs, ["pcmu", "pcma"]);
                assert_eq!(settings.media.ptime_ms, 20);
                assert_eq!(settings.media.rtp_ports, "20000-20999");
            }
            "config.set" => {
                let before: Settings = client.ok(Call::ConfigGet).await;
                let set = |log_level, log_dir: Option<&str>, pipe: Option<&str>| {
                    Call::ConfigSet(SettingsChange {
                        service: Some(ServiceChange {
                            log_level,
                            log_dir: log_dir.map(str::to_string),
                        }),
                        control: pipe.map(|pipe| ControlChange {
                            pipe: Some(pipe.to_string()),
                        }),
                        ..SettingsChange::default()
                    })
                };
                let result: SettingsResult = client
                    .ok(set(Some(LogLevel::Debug), Some("logs-here"), Some("other")))
                    .await;
                assert!(result.restart_required);
                assert_eq!(result.config.service.log_level, LogLevel::Debug);
                assert_eq!(result.config.service.log_dir, "logs-here");
                assert_eq!(result.config.control.pipe, "other");
                assert_eq!(result.config.path, before.path);
                let text = fixture.config_text();
                assert!(text.contains("log_level = \"debug\""), "{text}");
                assert!(text.contains("pipe = \"other\""), "{text}");
                assert_eq!(client.ok::<Settings>(Call::ConfigGet).await, result.config);
                // The service goes on as it started: the pipe it reports is
                // the one it listens on.
                assert_eq!(client.ok::<ServiceInfo>(Call::ServiceInfo).await.pipe, PIPE);

                // Only the keys given change.
                let result: SettingsResult =
                    client.ok(set(Some(LogLevel::Warning), None, None)).await;
                assert_eq!(result.config.service.log_dir, "logs-here");
                assert_eq!(result.config.control.pipe, "other");

                // A value that is not allowed changes nothing.
                assert_eq!(
                    client
                        .err(set(Some(LogLevel::Error), None, Some("not a pipe")))
                        .await,
                    "RMD0014"
                );
                assert_eq!(client.ok::<Settings>(Call::ConfigGet).await, result.config);
                let refused = client.exchange(
                    r#"{"id":900,"method":"config.set","params":{"service":{"log_level":"loud"}}}"#,
                ).await;
                assert!(
                    matches!(&refused, ServerFrame::Error { error, .. } if error.msgid == "RMD2003"),
                    "{refused:?}"
                );

                // Back as the service started, nothing needs a restart;
                // and a change that names nothing is no change.
                let result: SettingsResult = client
                    .ok(set(
                        Some(LogLevel::Info),
                        Some(""),
                        Some("rustmodem-control"),
                    ))
                    .await;
                assert!(!result.restart_required);
                assert_eq!(result.config, before);
                let result: SettingsResult =
                    client.ok(Call::ConfigSet(SettingsChange::default())).await;
                assert!(!result.restart_required);

                // What the router takes while it runs needs no restart.
                let sip = |sip: SipChange| {
                    Call::ConfigSet(SettingsChange {
                        sip: Some(sip),
                        ..SettingsChange::default()
                    })
                };
                let media = |media: MediaChange| {
                    Call::ConfigSet(SettingsChange {
                        media: Some(media),
                        ..SettingsChange::default()
                    })
                };
                let result: SettingsResult = client
                    .ok(sip(SipChange {
                        trusted_peers: Some(vec!["10.10.20.0/24".to_string()]),
                        outbound_peer: Some("10.10.20.11".to_string()),
                        ring_timeout_s: Some(30),
                        ..SipChange::default()
                    }))
                    .await;
                assert!(!result.restart_required);
                assert_eq!(result.config.sip.trusted_peers, ["10.10.20.0/24"]);
                assert_eq!(result.config.sip.outbound_peer, "10.10.20.11");
                assert_eq!(result.config.sip.ring_timeout_s, 30);
                let result: SettingsResult = client
                    .ok(media(MediaChange {
                        codecs: Some(vec!["pcma".to_string()]),
                        ptime_ms: Some(10),
                        rtp_timeout_s: Some(5),
                        ..MediaChange::default()
                    }))
                    .await;
                assert!(!result.restart_required);
                assert_eq!(result.config.media.codecs, ["pcma"]);
                assert_eq!(result.config.media.ptime_ms, 10);
                assert!(fixture.config_text().contains("[media]"));

                // Turning SIP on, the listener's address, the RTP ports and
                // the capture directory wait for the service to start
                // again; put back, nothing waits.
                for (change, back) in [
                    (
                        sip(SipChange {
                            enabled: Some(true),
                            ..SipChange::default()
                        }),
                        sip(SipChange {
                            enabled: Some(false),
                            ..SipChange::default()
                        }),
                    ),
                    (
                        sip(SipChange {
                            listen: Some("127.0.0.1:5070".to_string()),
                            ..SipChange::default()
                        }),
                        sip(SipChange {
                            listen: Some("0.0.0.0:5060".to_string()),
                            ..SipChange::default()
                        }),
                    ),
                    (
                        media(MediaChange {
                            rtp_ports: Some("30000-30099".to_string()),
                            ..MediaChange::default()
                        }),
                        media(MediaChange {
                            rtp_ports: Some("20000-20999".to_string()),
                            ..MediaChange::default()
                        }),
                    ),
                    (
                        media(MediaChange {
                            capture_dir: Some("captures-here".to_string()),
                            ..MediaChange::default()
                        }),
                        media(MediaChange {
                            capture_dir: Some(String::new()),
                            ..MediaChange::default()
                        }),
                    ),
                ] {
                    assert!(client.ok::<SettingsResult>(change).await.restart_required);
                    assert!(!client.ok::<SettingsResult>(back).await.restart_required);
                }
                assert_eq!(
                    client
                        .err(sip(SipChange {
                            listen: Some("nowhere".to_string()),
                            ..SipChange::default()
                        }))
                        .await,
                    "RMD0014"
                );
                assert_eq!(
                    client
                        .err(media(MediaChange {
                            ptime_ms: Some(25),
                            ..MediaChange::default()
                        }))
                        .await,
                    "RMD0014"
                );
                // The lines were not touched.
                assert_eq!(client.lines().await.len(), 2);
            }
            "stats.get" => {
                // So far: one call answered and hung up, and one the caller
                // was reset out of while line 2 rang.
                let stats: Stats = client.ok(Call::StatsGet).await;
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
                let all: LogTail = client.ok(Call::LogTail(LogTailQuery { last: None })).await;
                assert!(all.lines.len() > 10 && all.lines.len() <= LOG_RING);
                assert!(all.lines.is_sorted_by_key(|line| line.time_ms));
                assert!(
                    all.lines
                        .iter()
                        .any(|line| line.source == "Line02" && line.text.starts_with("RMD1005"))
                );
                let last: LogTail = client
                    .ok(Call::LogTail(LogTailQuery { last: Some(3) }))
                    .await;
                assert_eq!(last.lines, all.lines[all.lines.len() - 3..]);
                let none: LogTail = client
                    .ok(Call::LogTail(LogTailQuery { last: Some(0) }))
                    .await;
                assert!(none.lines.is_empty());
            }
            "ports.list" => {
                let list: PortList = client.ok(Call::PortsList).await;
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
                let sub: Subscription =
                    client.ok(Call::Subscribe(selection(&["line.state"]))).await;
                assert_eq!(sub.events, ["line.state"]);
                fixture.host("P2").await.set_dtr(false);
                let Event::LineState(info) = client.next_event().await else {
                    panic!("expected a line.state event");
                };
                assert_eq!((info.id, info.state), (2, LineState::NotReady));
                fixture.host("P2").await.set_dtr(true);
                let Event::LineState(info) = client.next_event().await else {
                    panic!("expected a line.state event");
                };
                assert_eq!((info.id, info.state), (2, LineState::Idle));

                // With no names, every event but the one that has to be
                // asked for by name.
                let sub: Subscription = client.ok(Call::Subscribe(EventSelection::default())).await;
                assert_eq!(sub.events, EVENT_NAMES[..6]);
                let sub: Subscription = client.ok(Call::Subscribe(selection(&["sip.trace"]))).await;
                assert_eq!(sub.events, EVENT_NAMES[..7]);
                // The diagnostics of a line are asked for with the line,
                // and given up again.
                let diag = EventSelection {
                    events: Some(vec!["line.diag".to_string()]),
                    line: Some(1),
                };
                let sub: Subscription = client.ok(Call::Subscribe(diag.clone())).await;
                assert_eq!(sub.events, EVENT_NAMES);
                assert_eq!(sub.line, Some(1));
                let sub: Subscription = client.ok(Call::Unsubscribe(diag)).await;
                assert_eq!((sub.events.len(), sub.line), (7, None));
                assert_eq!(
                    client.err(Call::Subscribe(selection(&["line.gone"]))).await,
                    "RMD2003"
                );
            }
            "unsubscribe" => {
                let sub: Subscription = client.ok(Call::Unsubscribe(selection(&["log"]))).await;
                assert_eq!(
                    sub.events,
                    [
                        "line.state",
                        "call.started",
                        "call.connected",
                        "call.ended",
                        "sip.registration",
                        "sip.trace"
                    ]
                );
                assert_eq!(
                    client.err(Call::Unsubscribe(selection(&["nope"]))).await,
                    "RMD2003"
                );
                let sub: Subscription = client
                    .ok(Call::Unsubscribe(EventSelection::default()))
                    .await;
                assert!(sub.events.is_empty());

                // Nothing arrives any more.
                client.events.clear();
                fixture.host("P2").await.set_dtr(false);
                client.wait_for_state(2, LineState::NotReady).await;
                fixture.host("P2").await.set_dtr(true);
                client.wait_for_state(2, LineState::Idle).await;
                assert!(client.events.is_empty(), "{:?}", client.events);
            }
            other => panic!("method {other} has no round-trip test"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_client_of_another_protocol_is_told_so_and_disconnected() {
    let fixture = Fixture::new(0).await;
    let mut client = fixture.connect_silent();
    let Some(ServerFrame::Error { id, error }) = client.hello(PROTOCOL + 1).await else {
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
        client.read_frame().await,
        None,
        "the service closed the connection"
    );
}

#[tokio::test(start_paused = true)]
async fn a_client_that_does_not_say_hello_first_is_disconnected() {
    let fixture = Fixture::new(0).await;
    for first in [
        r#"{"id":1,"method":"lines.list"}"#,
        "not json at all",
        r#"{"hello":"there"}"#,
    ] {
        let mut client = fixture.connect_silent();
        client.send_line(first).await;
        let Some(ServerFrame::Error { id, error }) = client.read_frame().await else {
            panic!("expected an error for {first}");
        };
        assert_eq!(id, None);
        assert_eq!(error.msgid, "RMD2005");
        assert_eq!(client.read_frame().await, None);
    }
    // A client that connects and leaves without a word is no trouble.
    drop(fixture.connect_silent());
    let mut client = fixture.connect().await;
    client.ok::<LineList>(Call::LinesList).await;
}

#[tokio::test(start_paused = true)]
async fn a_bad_request_gets_an_error_and_the_connection_stays_up() {
    let fixture = Fixture::new(1).await;
    let mut client = fixture.connect().await;

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
        let ServerFrame::Error { id, error } = client.exchange(request).await else {
            panic!("expected an error for {request}");
        };
        assert_eq!(id, want_id, "{request}");
        assert_eq!(error.msgid, want_error, "{request}");
    }
    // Blank lines and a carriage return before the line feed are tolerated.
    client.send_line("").await;
    client.send_line("   ").await;
    let frame = client
        .exchange("{\"id\":11,\"method\":\"lines.list\"}\r")
        .await;
    assert!(matches!(frame, ServerFrame::Result { id: 11, .. }));
    // Bytes that are not UTF-8.
    use tokio::io::AsyncWriteExt;
    client.writer.write_all(&[0xff, 0xfe, b'\n']).await.unwrap();
    let ServerFrame::Error { id: None, error } = client.next_reply().await else {
        panic!("expected an error");
    };
    assert_eq!(error.msgid, "RMD2004");
    assert_eq!(client.line(1).await.id, 1);
}

#[tokio::test(start_paused = true)]
async fn a_message_that_is_too_long_is_refused_and_the_client_disconnected() {
    let fixture = Fixture::new(0).await;
    let mut client = fixture.connect().await;
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
    let sender = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let _ = writer.write_all(long.as_bytes()).await;
        let _ = writer.write_all(b"\n").await;
    });
    let Some(ServerFrame::Error { id: None, error }) = client.read_frame().await else {
        panic!("expected an error");
    };
    assert_eq!(error.msgid, "RMD2014");
    assert_eq!(client.read_frame().await, None);
    sender.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_call_is_reported_event_by_event() {
    let fixture = Fixture::new(2).await;
    let mut client = fixture.connect().await;
    client
        .ok::<Subscription>(Call::Subscribe(selection(&[
            "line.state",
            "call.started",
            "call.connected",
            "call.ended",
        ])))
        .await;

    fixture.host("P1").await.write(b"ATE0D2\r");
    let Event::CallStarted(CallEvent { line: 1, call }) = client.next_event().await else {
        panic!("expected call.started on line 1");
    };
    assert_eq!(
        (call.direction, call.number.as_str()),
        (Direction::Outgoing, "2")
    );
    let Event::CallStarted(CallEvent { line: 2, call }) = client.next_event().await else {
        panic!("expected call.started on line 2");
    };
    assert_eq!(
        (call.direction, call.number.as_str()),
        (Direction::Incoming, "1")
    );
    for (id, state) in [(1, LineState::Handshaking), (2, LineState::Ringing)] {
        let Event::LineState(info) = client.next_event().await else {
            panic!("expected line.state");
        };
        assert_eq!((info.id, info.state), (id, state));
    }

    client.ok::<LineInfo>(Call::LinesAnswer(line(2))).await;
    for id in [1, 2] {
        let Event::CallConnected(CallEvent { line, call }) = client.next_event().await else {
            panic!("expected call.connected");
        };
        assert_eq!(line, id);
        assert_eq!(call.connect.unwrap().rate, 0);
    }
    for id in [1, 2] {
        let Event::LineState(info) = client.next_event().await else {
            panic!("expected line.state");
        };
        assert_eq!((info.id, info.state), (id, LineState::Online));
    }

    client.ok::<LineInfo>(Call::LinesHangup(line(2))).await;
    let Event::CallEnded(record) = client.next_event().await else {
        panic!("expected call.ended");
    };
    assert_eq!((record.line, record.reason), (2, EndReason::Operator));
    let Event::CallEnded(record) = client.next_event().await else {
        panic!("expected call.ended");
    };
    assert_eq!((record.line, record.reason), (1, EndReason::Remote));
}

#[tokio::test(start_paused = true)]
async fn log_events_reach_a_client_that_asks_for_them() {
    let fixture = Fixture::new(1).await;
    let mut client = fixture.connect().await;
    client
        .ok::<Subscription>(Call::Subscribe(selection(&["log"])))
        .await;
    fixture.host("P1").await.write(b"ATE0\r");
    let Event::Log(log) = client
        .wait_for_event(|e| matches!(e, Event::Log(l) if l.text == "< OK"))
        .await
    else {
        unreachable!();
    };
    assert_eq!(log.level, LogLevel::Debug);
    assert_eq!(log.source, "Line01");
}

#[tokio::test(start_paused = true)]
async fn each_client_has_its_own_subscription() {
    let fixture = Fixture::new(1).await;
    let mut watcher = fixture.connect().await;
    let mut other = fixture.connect().await;
    watcher
        .ok::<Subscription>(Call::Subscribe(selection(&["line.state"])))
        .await;

    fixture.host("P1").await.set_dtr(false);
    assert!(matches!(watcher.next_event().await, Event::LineState(_)));
    other.wait_for_state(1, LineState::NotReady).await;
    assert!(other.events.is_empty());
    assert_eq!(fixture.hub.client_count(), 2);

    drop(watcher);
    drop(other);
    eventually("both clients to be forgotten", || {
        fixture.hub.client_count() == 0
    })
    .await;
    assert!(fixture.logged("RMD2011: Control client"));
    eventually("the disconnects to be logged", || {
        fixture.logged("RMD2012: Control client")
    })
    .await;
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
        media: None,
    })
}

/// A client on a pipe so small that the service can hardly write to it,
/// subscribed to everything and not reading.
async fn slow_client(fixture: &Fixture) -> TestClient {
    let mut client = TestClient::new(fixture.connector.connect_with_capacity(200).unwrap());
    assert!(matches!(
        client.hello(PROTOCOL).await,
        Some(ServerFrame::Hello { .. })
    ));
    client
        .ok::<Subscription>(Call::Subscribe(EventSelection::default()))
        .await;
    client
}

#[tokio::test(start_paused = true)]
async fn a_slow_subscriber_loses_old_log_events_but_no_state_events() {
    let limits = Limits {
        max_queued: 100,
        max_logs: 10,
    };
    let fixture = Fixture::with_limits(0, limits).await;
    let mut client = slow_client(&fixture).await;

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
    client.ok::<LineList>(Call::LinesList).await;
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

#[tokio::test(start_paused = true)]
async fn a_subscriber_that_falls_too_far_behind_is_disconnected() {
    let limits = Limits {
        max_queued: 100,
        max_logs: 10,
    };
    let fixture = Fixture::with_limits(0, limits).await;
    let mut client = slow_client(&fixture).await;
    let mut bystander = fixture.connect().await;
    bystander
        .ok::<Subscription>(Call::Subscribe(selection(&["call.ended"])))
        .await;

    let started = Instant::now();
    for state in 1..=500 {
        fixture.hub.publish(&state_event(state));
        // The bystander keeps up, so it is not affected.
        if state % 50 == 0 {
            bystander.collect_events(state as usize).await;
        }
    }
    assert!(started.elapsed() < Duration::from_secs(5));

    // The slow client is cut off rather than sent a state with holes in it.
    // The connection is closed under the writer, so the last line may be
    // cut short; whole lines are whole events.
    let mut delivered = 0;
    loop {
        use tokio::io::AsyncBufReadExt;
        let mut text = String::new();
        let read = client.reader.read_line(&mut text).await.unwrap();
        if read == 0 || !text.ends_with('\n') {
            break;
        }
        let frame = ctlproto::decode_server(text.trim_end()).unwrap();
        assert!(matches!(frame, ServerFrame::Event(Event::CallEnded(_))));
        delivered += 1;
    }
    assert!(delivered < 500);
    eventually("the disconnect to be logged", || fixture.logged("RMD2006")).await;
    eventually("the slow client to be forgotten", || {
        fixture.hub.client_count() == 1
    })
    .await;

    assert_eq!(bystander.events.len(), 500);
    bystander.ok::<LineList>(Call::LinesList).await;
}

#[tokio::test(start_paused = true)]
async fn a_change_that_cannot_be_saved_changes_nothing() {
    let fixture = Fixture::new(1).await;
    let mut client = fixture.connect().await;
    // The directory of the configuration file goes away.
    std::fs::remove_dir_all(&fixture.directory).unwrap();
    assert_eq!(client.err(Call::LinesAdd(spec(2, "P2"))).await, "RMD0011");
    assert_eq!(client.err(Call::LinesDisable(line(1))).await, "RMD0011");
    let lines = client.lines().await;
    assert_eq!(lines.len(), 1);
    assert!(lines[0].enabled);
}

#[tokio::test(start_paused = true)]
async fn requests_are_refused_once_the_pool_has_stopped() {
    let fixture = Fixture::new(1).await;
    let mut client = fixture.connect().await;
    let server = fixture.server.clone();
    let connector = fixture.connector.clone();
    // Dropping the fixture stops the pool's thread; the server lives on in
    // its own threads for as long as there is a connector.
    drop(fixture);
    assert_eq!(client.err(Call::LinesList).await, "RMD2010");
    assert_eq!(client.err(Call::LinesAnswer(line(1))).await, "RMD2010");
    drop(server);
    drop(connector);
}

#[tokio::test(start_paused = true)]
async fn the_log_tail_holds_the_last_two_thousand_lines() {
    let fixture = Fixture::new(0).await;
    let mut client = fixture.connect().await;
    let before = client
        .ok::<LogTail>(Call::LogTail(LogTailQuery::default()))
        .await
        .lines
        .len();
    assert!(before < 100, "{before} lines from an empty service");
    // Nobody is subscribed: the lines are remembered all the same.
    for number in 1..=(LOG_RING as u64 + 500) {
        fixture.hub.publish(&log_event(number));
    }
    let tail: LogTail = client.ok(Call::LogTail(LogTailQuery::default())).await;
    assert_eq!(tail.lines.len(), LOG_RING);
    assert_eq!(tail.lines[0].time_ms, 501);
    assert_eq!(tail.lines[LOG_RING - 1].text, "log line 2500");
    let last: LogTail = client
        .ok(Call::LogTail(LogTailQuery { last: Some(2) }))
        .await;
    let numbers: Vec<u64> = last.lines.iter().map(|line| line.time_ms).collect();
    assert_eq!(numbers, [2499, 2500]);
    // Asking for more than there is gives what there is.
    let all: LogTail = client
        .ok(Call::LogTail(LogTailQuery { last: Some(99_999) }))
        .await;
    assert_eq!(all.lines.len(), LOG_RING);
}

#[tokio::test(start_paused = true)]
async fn lines_show_what_their_ports_are_bound_to() {
    let fixture = Fixture::new(3).await;
    let mut client = fixture.connect().await;
    assert_eq!(client.line(1).await.binding, None);

    client
        .ok::<Subscription>(Call::Subscribe(selection(&["line.state"])))
        .await;
    fixture.server.refresh_bindings().await.unwrap();
    // The lines whose binding became known say so; line 3 is on a port the
    // inventory does not have.
    let mut told = Vec::new();
    for _ in 0..2 {
        let Event::LineState(info) = client.next_event().await else {
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
    assert_eq!(client.line(3).await.binding, None);

    // A line that moves to a port takes the binding of that port, and
    // reloading the configuration looks the ports up again.
    let mut moved = change(3);
    moved.port = Some("P2x".to_string());
    assert_eq!(
        client.ok::<LineInfo>(Call::LinesEdit(moved)).await.binding,
        None
    );
    client.ok::<Reloaded>(Call::ConfigReload).await;
    assert_eq!(client.line(2).await.binding.as_deref(), Some("COM12"));
}

#[tokio::test(start_paused = true)]
async fn a_service_that_can_apply_its_settings_needs_no_restart_for_them() {
    let fixture = Fixture::new(1).await;
    let mut client = fixture.connect().await;
    let level = |level| {
        Call::ConfigSet(SettingsChange {
            service: Some(ServiceChange {
                log_level: Some(level),
                log_dir: None,
            }),
            control: None,
            ..SettingsChange::default()
        })
    };

    // A service that takes its settings up only when it starts.
    let result: SettingsResult = client.ok(level(LogLevel::Debug)).await;
    assert!(result.restart_required);
    assert_eq!(result.config.service.log_level, LogLevel::Debug);

    // One that puts them into effect: the change that was waiting is in
    // effect with the next one, and nothing waits for a restart.
    fixture
        .live
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let result: SettingsResult = client.ok(level(LogLevel::Warning)).await;
    assert!(!result.restart_required);
    assert_eq!(result.config.service.log_level, LogLevel::Warning);
    let text = fixture.config_text();
    assert!(text.contains("log_level = \"warning\""), "{text}");

    // The control pipe is made when the service starts, whatever the
    // service can do while it runs.
    let result: SettingsResult = client
        .ok(Call::ConfigSet(SettingsChange {
            service: None,
            control: Some(ControlChange {
                pipe: Some("other".to_string()),
            }),
            ..SettingsChange::default()
        }))
        .await;
    assert!(result.restart_required);
    // And back: nothing is waiting any more.
    let result: SettingsResult = client
        .ok(Call::ConfigSet(SettingsChange {
            service: Some(ServiceChange {
                log_level: Some(LogLevel::Error),
                log_dir: None,
            }),
            control: Some(ControlChange {
                pipe: Some("rustmodem-control".to_string()),
            }),
            ..SettingsChange::default()
        }))
        .await;
    assert!(!result.restart_required);
}

/// A line's engine and its registration to a PBX are set when it is
/// added and changed key by key afterwards. The password goes to the
/// configuration file and is never reported.
#[tokio::test(start_paused = true)]
async fn a_line_takes_an_engine_and_a_registration_and_never_shows_the_password() {
    let fixture = Fixture::new(1).await;
    let mut client = fixture.connect().await;

    let mut new = spec(2, "P2");
    new.engine = Some("echo".to_string());
    new.register = Some(RegisterSpec {
        server: "pbx.example.com".to_string(),
        user: "202".to_string(),
        password: "open sesame".to_string(),
        auth_user: None,
        expires_s: None,
    });
    let reply = client
        .exchange(&ctlproto::encode(&ctlproto::ClientFrame::Request(
            Call::LinesAdd(new).request(77),
        )))
        .await;
    let text = format!("{reply:?}");
    assert!(!text.contains("open sesame"), "{text}");
    let added = client.line(2).await;
    assert_eq!(added.engine, "echo");
    assert_eq!(added.sip.mode, SipMode::Registered);
    assert_eq!(added.sip.extension.as_deref(), Some("202"));
    assert_eq!(added.sip.server.as_deref(), Some("pbx.example.com"));
    assert_eq!(added.sip.auth_user, None);
    assert_eq!(added.sip.expires_s, Some(300));
    assert!(added.sip.has_password);
    // This service has no SIP, so nothing registers.
    assert_eq!(added.sip.registration, None);
    assert!(fixture.config_text().contains("open sesame"));
    // A line with no registration, in a service with no SIP.
    assert_eq!(client.line(1).await.sip.mode, SipMode::None);
    assert_eq!(client.line(1).await.engine, "null");

    // Only the keys given change: the password stays.
    let mut edit = change(2);
    edit.engine = Some("null".to_string());
    edit.register = Some(RegisterChange {
        user: Some("203".to_string()),
        auth_user: Some("line203".to_string()),
        expires_s: Some(600),
        ..RegisterChange::default()
    });
    let edited: LineInfo = client.ok(Call::LinesEdit(edit)).await;
    assert_eq!(edited.engine, "null");
    assert_eq!(edited.sip.extension.as_deref(), Some("203"));
    assert_eq!(edited.sip.auth_user.as_deref(), Some("line203"));
    assert_eq!(edited.sip.expires_s, Some(600));
    assert!(edited.sip.has_password);
    assert!(fixture.config_text().contains("open sesame"));

    let mut password = change(2);
    password.register = Some(RegisterChange {
        password: Some("another".to_string()),
        ..RegisterChange::default()
    });
    client.ok::<LineInfo>(Call::LinesEdit(password)).await;
    let text = fixture.config_text();
    assert!(
        text.contains("another") && !text.contains("open sesame"),
        "{text}"
    );

    // An empty server takes the registration away.
    let mut remove = change(2);
    remove.register = Some(RegisterChange {
        server: Some(String::new()),
        ..RegisterChange::default()
    });
    let removed: LineInfo = client.ok(Call::LinesEdit(remove)).await;
    assert_eq!(removed.sip.mode, SipMode::None);
    assert!(!removed.sip.has_password);
    assert!(!fixture.config_text().contains("register"));

    // A line that has none needs the server, the user and the password
    // together.
    let mut partial = change(2);
    partial.register = Some(RegisterChange {
        server: Some("pbx".to_string()),
        user: Some("202".to_string()),
        ..RegisterChange::default()
    });
    assert_eq!(client.err(Call::LinesEdit(partial)).await, "RMD0035");
    let mut whole = change(1);
    whole.register = Some(RegisterChange {
        server: Some("pbx".to_string()),
        user: Some("201".to_string()),
        password: Some("pw".to_string()),
        ..RegisterChange::default()
    });
    let registered: LineInfo = client.ok(Call::LinesEdit(whole)).await;
    assert_eq!(registered.sip.mode, SipMode::Registered);

    let mut bad = change(1);
    bad.engine = Some("v90".to_string());
    assert_eq!(client.err(Call::LinesEdit(bad)).await, "RMD2003");
    let mut bad = spec(3, "P3");
    bad.engine = Some("v90".to_string());
    assert_eq!(client.err(Call::LinesAdd(bad)).await, "RMD2003");
}
