// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the control API has for the modems: a line's settings and those
//! of `[modem]`, a line's diagnostics and who is sent them, and the
//! loopback self-test.

mod common;

use common::{Fixture, TestClient};
use ctlproto::{
    Call, Event, EventSelection, LineChange, LineDiag, LineInfo, LineModemSpec, LineRef, LineSpec,
    LineState, ModemChange, ModemInfo, SelfTest, SelfTestResult, Settings, SettingsChange,
    SettingsResult, Subscription,
};

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

fn diagnostics_of(line: u16) -> EventSelection {
    EventSelection {
        events: Some(vec!["line.diag".to_string()]),
        line: Some(line),
    }
}

/// The diagnostics events a client has been sent and not looked at yet,
/// as the line and whether there was a modem.
fn diags(client: &mut TestClient) -> Vec<(u16, bool)> {
    client
        .events
        .drain(..)
        .filter_map(|event| match event {
            Event::LineDiag(diag) => Some((diag.line, diag.modem.is_some())),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_line_takes_modem_settings_of_its_own_and_gives_them_up() {
    let fixture = Fixture::new(1).await;
    let mut client = fixture.connect().await;
    // A line that says nothing has the settings of `[modem]`.
    let line = client.line(1).await;
    assert_eq!(line.modem.modulations, ["v21", "bell103"]);
    assert_eq!((line.modem.v8, line.modem.tx_level_dbm0), (true, -12));
    assert!(line.modem.own.is_empty());

    let added: LineInfo = client
        .ok(Call::LinesAdd(LineSpec {
            id: 2,
            name: None,
            backend: None,
            port: "P2".to_string(),
            enabled: None,
            auto_answer_rings: None,
            unknown_command: None,
            engine: None,
            register: None,
            modem: Some(LineModemSpec {
                modulations: Some(vec!["bell103".to_string()]),
                v8: Some(false),
                tx_level_dbm0: Some(-15),
            }),
        }))
        .await;
    // A line that names no engine has the modem.
    assert_eq!(added.engine, "modem");
    assert_eq!(added.modem.modulations, ["bell103"]);
    assert_eq!((added.modem.v8, added.modem.tx_level_dbm0), (false, -15));
    assert_eq!(added.modem.own, ["modulations", "v8", "tx_level_dbm0"]);
    assert!(
        fixture
            .config_text()
            .contains(r#"modem = { modulations = ["bell103"], v8 = false, tx_level_dbm0 = -15 }"#),
        "{}",
        fixture.config_text()
    );

    // The settings are given whole: what is left out is `[modem]`'s again.
    let mut level_only = change(2);
    level_only.modem = Some(LineModemSpec {
        tx_level_dbm0: Some(-9),
        ..LineModemSpec::default()
    });
    let edited: LineInfo = client.ok(Call::LinesEdit(level_only)).await;
    assert_eq!(edited.modem.modulations, ["v21", "bell103"]);
    assert_eq!((edited.modem.v8, edited.modem.tx_level_dbm0), (true, -9));
    assert_eq!(edited.modem.own, ["tx_level_dbm0"]);
    assert!(
        fixture
            .config_text()
            .contains("modem = { tx_level_dbm0 = -9 }")
    );
    // An edit that says nothing of the modem leaves it alone.
    let mut renamed = change(2);
    renamed.name = Some("Second".to_string());
    let edited: LineInfo = client.ok(Call::LinesEdit(renamed)).await;
    assert_eq!(edited.modem.own, ["tx_level_dbm0"]);
    // And an empty one leaves the line with none of its own.
    let mut none = change(2);
    none.modem = Some(LineModemSpec::default());
    let edited: LineInfo = client.ok(Call::LinesEdit(none)).await;
    assert!(edited.modem.own.is_empty());
    assert_eq!(edited.modem.tx_level_dbm0, -12);
    assert!(!fixture.config_text().contains("modem ="));

    // What the configuration would refuse is refused, and nothing is
    // changed.
    for bad in [
        LineModemSpec {
            modulations: Some(vec!["v22".to_string()]),
            ..LineModemSpec::default()
        },
        LineModemSpec {
            modulations: Some(Vec::new()),
            ..LineModemSpec::default()
        },
        LineModemSpec {
            tx_level_dbm0: Some(-8),
            ..LineModemSpec::default()
        },
        LineModemSpec {
            tx_level_dbm0: Some(1000),
            ..LineModemSpec::default()
        },
    ] {
        let mut edit = change(2);
        edit.modem = Some(bad.clone());
        assert_eq!(
            client.err(Call::LinesEdit(edit)).await,
            "RMD0014",
            "{bad:?}"
        );
    }
    assert!(client.line(2).await.modem.own.is_empty());
    let mut engine = change(2);
    engine.engine = Some("v34".to_string());
    assert_eq!(client.err(Call::LinesEdit(engine)).await, "RMD2003");
}

#[tokio::test(start_paused = true)]
async fn the_modem_table_is_reported_and_changed_and_the_lines_follow() {
    let fixture = Fixture::new(2).await;
    let mut client = fixture.connect().await;
    let settings: Settings = client.ok(Call::ConfigGet).await;
    assert_eq!(settings.modem.modulations, ["v21", "bell103"]);
    assert_eq!(
        (
            settings.modem.v8,
            settings.modem.tx_level_dbm0,
            settings.modem.answer_delay_ms,
            settings.modem.fallback_ms
        ),
        (true, -12, 0, 3000)
    );

    let set = |modem: ModemChange| {
        Call::ConfigSet(SettingsChange {
            modem: Some(modem),
            ..SettingsChange::default()
        })
    };
    let result: SettingsResult = client
        .ok(set(ModemChange {
            modulations: Some(vec!["bell103".to_string(), "v21".to_string()]),
            v8: Some(false),
            tx_level_dbm0: Some(-10),
            answer_delay_ms: Some(500),
            fallback_ms: Some(2000),
        }))
        .await;
    // It is in force for the calls that follow, with no restart.
    assert!(!result.restart_required);
    assert_eq!(result.config.modem.modulations, ["bell103", "v21"]);
    assert_eq!(
        (
            result.config.modem.v8,
            result.config.modem.tx_level_dbm0,
            result.config.modem.answer_delay_ms,
            result.config.modem.fallback_ms
        ),
        (false, -10, 500, 2000)
    );
    let text = fixture.config_text();
    assert!(text.contains("[modem]\n"), "{text}");
    assert!(text.contains("tx_level_dbm0 = -10\n"), "{text}");
    // The pool has it: a line with no settings of its own shows the new.
    let line = client.line(1).await;
    assert_eq!(line.modem.modulations, ["bell103", "v21"]);
    assert_eq!((line.modem.v8, line.modem.tx_level_dbm0), (false, -10));

    // One key changes one key.
    let result: SettingsResult = client
        .ok(set(ModemChange {
            v8: Some(true),
            ..ModemChange::default()
        }))
        .await;
    assert!(result.config.modem.v8);
    assert_eq!(result.config.modem.tx_level_dbm0, -10);

    for bad in [
        ModemChange {
            tx_level_dbm0: Some(0),
            ..ModemChange::default()
        },
        ModemChange {
            modulations: Some(vec!["v21".to_string(), "v21".to_string()]),
            ..ModemChange::default()
        },
        ModemChange {
            answer_delay_ms: Some(60_000),
            ..ModemChange::default()
        },
        ModemChange {
            fallback_ms: Some(1),
            ..ModemChange::default()
        },
    ] {
        assert_eq!(client.err(set(bad.clone())).await, "RMD0014", "{bad:?}");
    }
    let settings: Settings = client.ok(Call::ConfigGet).await;
    assert_eq!(settings.modem, result.config.modem);
}

#[tokio::test(start_paused = true)]
async fn a_lines_diagnostics_go_to_the_clients_that_asked_for_that_line() {
    let fixture = Fixture::new(2).await;
    let mut first = fixture.connect().await;
    let mut second = fixture.connect().await;
    let mut everything = fixture.connect().await;

    // A line with no call has nothing to show.
    let diag: LineDiag = first.ok(Call::LinesDiag(LineRef { id: 1 })).await;
    assert_eq!(
        diag,
        LineDiag {
            line: 1,
            scope_kind: "none".to_string(),
            ..LineDiag::default()
        }
    );
    assert_eq!(
        first.err(Call::LinesDiag(LineRef { id: 9 })).await,
        "RMD1001"
    );

    // Every event is every event but the ones that are asked for by name.
    let all: Subscription = everything
        .ok(Call::Subscribe(EventSelection::default()))
        .await;
    assert!(!all.events.iter().any(|name| name == "line.diag"));
    assert_eq!(all.line, None);

    // The diagnostics are one line's: which has to be said, the line has
    // to be there, and a line is said for nothing else.
    let named = EventSelection {
        events: Some(vec!["line.diag".to_string()]),
        line: None,
    };
    assert_eq!(first.err(Call::Subscribe(named)).await, "RMD2003");
    assert_eq!(
        first.err(Call::Subscribe(diagnostics_of(9))).await,
        "RMD1001"
    );
    let stray = EventSelection {
        events: Some(vec!["log".to_string()]),
        line: Some(1),
    };
    assert_eq!(first.err(Call::Subscribe(stray)).await, "RMD2003");

    let sub: Subscription = first.ok(Call::Subscribe(diagnostics_of(1))).await;
    assert_eq!(
        (sub.events.as_slice(), sub.line),
        (&["line.diag".to_string()][..], Some(1))
    );
    let sub: Subscription = second.ok(Call::Subscribe(diagnostics_of(2))).await;
    assert_eq!(sub.line, Some(2));
    // The pool says a line that has just been watched at once.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    first.line(1).await;
    second.line(1).await;
    assert_eq!(diags(&mut first), [(1, false)]);
    assert_eq!(diags(&mut second), [(2, false)]);

    // What is published of a line goes to whoever watches that line.
    let of = |line: u16| {
        Event::LineDiag(LineDiag {
            line,
            modem: Some(ModemInfo {
                state: "data".to_string(),
                ..ModemInfo::default()
            }),
            scope_kind: "none".to_string(),
            ..LineDiag::default()
        })
    };
    fixture.hub.publish(&of(1));
    fixture.hub.publish(&of(2));
    fixture.hub.publish(&of(1));
    first.line(1).await;
    second.line(1).await;
    everything.line(1).await;
    assert_eq!(diags(&mut first), [(1, true), (1, true)]);
    assert_eq!(diags(&mut second), [(2, true)]);
    assert!(diags(&mut everything).is_empty());

    // Another line named: the client is sent that one's from now on.
    let sub: Subscription = first.ok(Call::Subscribe(diagnostics_of(2))).await;
    assert_eq!(sub.line, Some(2));
    fixture.hub.publish(&of(1));
    fixture.hub.publish(&of(2));
    first.line(1).await;
    assert_eq!(diags(&mut first).last(), Some(&(2, true)));
    assert!(!diags(&mut first).contains(&(1, true)));

    // Unsubscribed, it is sent none.
    let sub: Subscription = first
        .ok(Call::Unsubscribe(EventSelection {
            events: Some(vec!["line.diag".to_string()]),
            line: None,
        }))
        .await;
    assert!(sub.events.is_empty());
    assert_eq!(sub.line, None);
    first.events.clear();
    fixture.hub.publish(&of(2));
    first.line(1).await;
    assert!(diags(&mut first).is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_self_test_runs_two_modems_and_holds_their_lines_meanwhile() {
    let fixture = Fixture::new(3).await;
    let mut client = fixture.connect().await;
    let mut watcher = fixture.connect().await;
    let test = |a, b| Call::SelfTestRun(SelfTest { a, b });

    // The lines of the fixture have the null engine.
    assert_eq!(client.err(test(1, 2)).await, "RMD7003");
    assert_eq!(client.err(test(1, 1)).await, "RMD7004");
    assert_eq!(client.err(test(1, 9)).await, "RMD1001");
    for id in [1, 2] {
        let mut edit = change(id);
        edit.engine = Some("modem".to_string());
        client.ok::<LineInfo>(Call::LinesEdit(edit)).await;
    }

    // While the test runs both lines are seen to be busy, and when it is
    // over they are idle again.
    watcher
        .ok::<Subscription>(Call::Subscribe(EventSelection {
            events: Some(vec!["line.state".to_string()]),
            line: None,
        }))
        .await;
    let result: SelfTestResult = client.ok(test(1, 2)).await;
    assert!(result.passed, "{result:?}");
    assert_eq!((result.a, result.b, &result.error), (1, 2, &None));
    let connect = result.connect.clone().unwrap();
    assert_eq!(
        (connect.rate, connect.carrier.as_deref()),
        (300, Some("V.21"))
    );
    assert_eq!(
        (result.octets, result.octets_a_to_b, result.octets_b_to_a),
        (65_536, 65_536, 65_536)
    );
    // 64 KiB at thirty characters a second is more than half an hour of
    // the call.
    assert!(result.call_ms > 2_100_000, "{result:?}");
    assert!(result.connect_ms.unwrap() < 10_000);

    watcher.line(1).await;
    let seen: Vec<(u16, LineState)> = watcher
        .events
        .drain(..)
        .filter_map(|event| match event {
            Event::LineState(line) => Some((line.id, line.state)),
            _ => None,
        })
        .collect();
    assert_eq!(
        seen,
        [
            (1, LineState::Handshaking),
            (2, LineState::Handshaking),
            (1, LineState::Idle),
            (2, LineState::Idle)
        ]
    );

    // Modems with no modulation in common: the test runs and fails, which
    // is a result and not an error, and is logged.
    let mut bell = change(2);
    bell.modem = Some(LineModemSpec {
        modulations: Some(vec!["bell103".to_string()]),
        v8: Some(false),
        tx_level_dbm0: None,
    });
    client.ok::<LineInfo>(Call::LinesEdit(bell)).await;
    let mut v21 = change(1);
    v21.modem = Some(LineModemSpec {
        modulations: Some(vec!["v21".to_string()]),
        ..LineModemSpec::default()
    });
    client.ok::<LineInfo>(Call::LinesEdit(v21)).await;
    let result: SelfTestResult = client.ok(test(1, 2)).await;
    assert!(!result.passed);
    assert_eq!(result.connect, None);
    assert_eq!(result.error.as_ref().unwrap().msgid, "RMD7005");
    assert!(fixture.logged("RMD7005: The self-test of lines 1 and 2 failed"));
    // The lines were let go all the same.
    assert_eq!(client.line(1).await.state, LineState::Idle);
    assert_eq!(client.line(2).await.state, LineState::Idle);
}
