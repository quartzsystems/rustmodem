// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Changes to the SIP side of a configuration file: the registration and
//! engine of a line, `[sip]` and `[media]`, hunt groups and routes. Each
//! change is made in place, keeps the comments around it, and is refused
//! whole when its result would not be valid.

use config::{
    ConfigFile, EngineKind, HuntGroup, HuntGroupEdit, LineEdit, MediaEdit, RegisterConfig, Route,
    RouteEdit, SettingsEdit, SipEdit, Strategy, Target,
};

const FILE: &str = r#"# The modem pool.
[sip]
enabled = true            # on
listen = "0.0.0.0:5060"
trusted_peers = ["10.10.20.11"]

[[line]]
id = 1
serial = { backend = "com0com", port = "P1" }

[[line]]
id = 2
serial = { backend = "com0com", port = "P2" }
register = { server = "pbx", user = "202", password = "secret" }  # the PBX

# The dial-up number.
[[hunt_group]]
name = "dialup"
pilot = "5551000"
lines = [1, 2]

# First the exact number,
[[route]]
called = "5551234"
target = "line:1"

# then everything else in 555.
[[route]]
called = "555*"
target = "group:dialup"
"#;

fn file() -> ConfigFile {
    ConfigFile::parse(FILE).unwrap()
}

fn ids(problems: &[msg::Message]) -> Vec<&str> {
    problems.iter().map(|m| m.id.as_str()).collect()
}

fn routes(file: &ConfigFile) -> Vec<(String, String)> {
    file.config()
        .routes
        .iter()
        .map(|route| (route.called.clone(), route.target.to_string()))
        .collect()
}

fn pairs(routes: &[(&str, &str)]) -> Vec<(String, String)> {
    routes
        .iter()
        .map(|(called, target)| (called.to_string(), target.to_string()))
        .collect()
}

/// What the file says is what a later load reads.
fn reread(file: &ConfigFile) {
    let again = ConfigFile::parse(&file.to_toml()).unwrap();
    assert_eq!(again.config(), file.config());
}

#[test]
fn a_line_gains_and_loses_its_engine_and_registration() {
    let mut file = file();
    let register = RegisterConfig {
        server: "pbx:5070".to_string(),
        user: "201".to_string(),
        password: "pw".to_string(),
        auth_user: "line201".to_string(),
        expires_s: 600,
    };
    file.edit_line(
        1,
        &LineEdit {
            engine: Some(EngineKind::Echo),
            register: Some(Some(register.clone())),
            ..LineEdit::default()
        },
    )
    .unwrap();
    let line = file.config().line(1).unwrap();
    assert_eq!(line.engine, EngineKind::Echo);
    assert_eq!(line.register.as_ref(), Some(&register));
    assert!(file.to_toml().contains(r#"engine = "echo""#));
    reread(&file);

    // A registration that is replaced keeps the comment after it.
    let other = RegisterConfig {
        password: "new".to_string(),
        ..file.config().line(2).unwrap().register.clone().unwrap()
    };
    file.edit_line(
        2,
        &LineEdit {
            register: Some(Some(other)),
            ..LineEdit::default()
        },
    )
    .unwrap();
    assert_eq!(
        file.config()
            .line(2)
            .unwrap()
            .register
            .as_ref()
            .unwrap()
            .password,
        "new"
    );
    reread(&file);

    file.edit_line(
        2,
        &LineEdit {
            register: Some(None),
            ..LineEdit::default()
        },
    )
    .unwrap();
    assert_eq!(file.config().line(2).unwrap().register, None);
    assert!(!file.to_toml().contains("register = { server = \"pbx\","));
    assert!(file.to_toml().starts_with("# The modem pool.\n"));
    reread(&file);
}

#[test]
fn the_sip_and_media_settings_change_in_place() {
    let mut file = file();
    file.edit_settings(&SettingsEdit {
        sip: SipEdit {
            enabled: Some(false),
            trusted_peers: Some(vec!["10.0.0.0/8".to_string(), "192.168.1.5".to_string()]),
            outbound_peer: Some("10.10.20.11:5080".to_string()),
            ring_timeout_s: Some(30),
            ..SipEdit::default()
        },
        media: MediaEdit {
            codecs: Some(vec!["pcma".to_string()]),
            ptime_ms: Some(10),
            rtp_ports: Some("30000-30099".to_string()),
            rtp_timeout_s: Some(5),
            capture_dir: Some(r"D:\captures".to_string()),
        },
        ..SettingsEdit::default()
    })
    .unwrap();
    let config = file.config();
    assert!(!config.sip.enabled);
    assert_eq!(config.sip.trusted_peers.len(), 2);
    assert_eq!(config.sip.outbound_peer, "10.10.20.11:5080");
    assert_eq!(config.sip.ring_timeout_s, 30);
    assert_eq!(config.sip.listen.to_string(), "0.0.0.0:5060");
    assert_eq!(config.media.ptime_ms, 10);
    assert_eq!(config.media.rtp_ports.to_string(), "30000-30099");
    assert_eq!(config.media.capture_dir, r"D:\captures");
    let text = file.to_toml();
    // The comment after the key that changed is still after it, and the
    // table the file did not have is there now.
    assert!(text.contains("enabled = false            # on\n"), "{text}");
    assert!(text.contains("[media]\n"), "{text}");
    reread(&file);
}

#[test]
fn a_setting_that_is_not_valid_is_refused_and_nothing_changes() {
    for edit in [
        SettingsEdit {
            sip: SipEdit {
                listen: Some("not an address".to_string()),
                ..SipEdit::default()
            },
            ..SettingsEdit::default()
        },
        SettingsEdit {
            sip: SipEdit {
                trusted_peers: Some(vec!["10.0.0.0/33".to_string()]),
                ..SipEdit::default()
            },
            ..SettingsEdit::default()
        },
        SettingsEdit {
            media: MediaEdit {
                codecs: Some(vec!["g729".to_string()]),
                ..MediaEdit::default()
            },
            ..SettingsEdit::default()
        },
        SettingsEdit {
            media: MediaEdit {
                ptime_ms: Some(25),
                ..MediaEdit::default()
            },
            ..SettingsEdit::default()
        },
        SettingsEdit {
            media: MediaEdit {
                rtp_ports: Some("100-200".to_string()),
                ..MediaEdit::default()
            },
            ..SettingsEdit::default()
        },
    ] {
        let mut file = file();
        let problems = file.edit_settings(&edit).unwrap_err();
        assert_eq!(ids(&problems), ["RMD0014"], "{edit:?}");
        assert_eq!(file.to_toml(), FILE);
    }
}

#[test]
fn hunt_groups_are_added_changed_and_removed() {
    let mut file = file();
    let night = HuntGroup {
        name: "night".to_string(),
        pilot: "5552000".to_string(),
        lines: vec![2],
        strategy: Strategy::RoundRobin,
    };
    file.add_hunt_group(&night).unwrap();
    assert_eq!(file.config().hunt_groups.len(), 2);
    assert_eq!(file.config().hunt_group("night"), Some(&night));
    reread(&file);

    // A name or a pilot that is in use, and a line that does not exist.
    assert_eq!(ids(&file.add_hunt_group(&night).unwrap_err()), ["RMD0033"]);
    let clash = HuntGroup {
        name: "other".to_string(),
        pilot: "5551000".to_string(),
        ..night.clone()
    };
    assert_eq!(ids(&file.add_hunt_group(&clash).unwrap_err()), ["RMD0029"]);
    let empty = HuntGroup {
        name: "other".to_string(),
        pilot: "5553000".to_string(),
        lines: vec![9],
        ..night.clone()
    };
    assert_eq!(ids(&file.add_hunt_group(&empty).unwrap_err()), ["RMD0027"]);

    file.edit_hunt_group(
        "night",
        &HuntGroupEdit {
            pilot: Some("5552001".to_string()),
            lines: Some(vec![2, 1]),
            strategy: Some("least-recent".to_string()),
            ..HuntGroupEdit::default()
        },
    )
    .unwrap();
    let group = file.config().hunt_group("night").unwrap();
    assert_eq!(group.pilot, "5552001");
    assert_eq!(group.lines, [2, 1]);
    assert_eq!(group.strategy, Strategy::LeastRecent);
    reread(&file);

    let unknown = file
        .edit_hunt_group("nowhere", &HuntGroupEdit::default())
        .unwrap_err();
    assert_eq!(ids(&unknown), ["RMD0032"]);
    let taken = file
        .edit_hunt_group(
            "night",
            &HuntGroupEdit {
                name: Some("dialup".to_string()),
                ..HuntGroupEdit::default()
            },
        )
        .unwrap_err();
    assert_eq!(ids(&taken), ["RMD0033"]);
    let strategy = file
        .edit_hunt_group(
            "night",
            &HuntGroupEdit {
                strategy: Some("random".to_string()),
                ..HuntGroupEdit::default()
            },
        )
        .unwrap_err();
    assert_eq!(ids(&strategy), ["RMD0014"]);

    file.remove_hunt_group("night").unwrap();
    assert_eq!(file.config().hunt_groups.len(), 1);
    assert_eq!(
        ids(&file.remove_hunt_group("night").unwrap_err()),
        ["RMD0032"]
    );
    // A group a route sends calls to stays.
    let before = file.to_toml();
    assert_eq!(
        ids(&file.remove_hunt_group("dialup").unwrap_err()),
        ["RMD0030"]
    );
    assert_eq!(file.to_toml(), before);
    assert!(before.contains("# The dial-up number.\n[[hunt_group]]"));
}

#[test]
fn the_routes_to_a_group_follow_its_new_name() {
    let mut file = file();
    file.edit_hunt_group(
        "dialup",
        &HuntGroupEdit {
            name: Some("modems".to_string()),
            ..HuntGroupEdit::default()
        },
    )
    .unwrap();
    assert!(file.config().hunt_group("modems").is_some());
    assert_eq!(
        routes(&file),
        pairs(&[("5551234", "line:1"), ("555*", "group:modems")])
    );
    assert_eq!(file.config().hunt_groups_of(1), ["modems"]);
    reread(&file);
}

#[test]
fn routes_are_added_changed_moved_and_removed_in_order() {
    let mut file = file();
    file.add_route(
        &Route {
            called: "900*".to_string(),
            target: Target::Line(2),
        },
        None,
    )
    .unwrap();
    assert_eq!(
        routes(&file),
        pairs(&[
            ("5551234", "line:1"),
            ("555*", "group:dialup"),
            ("900*", "line:2")
        ])
    );
    reread(&file);

    file.add_route(
        &Route {
            called: "5559999".to_string(),
            target: Target::Group("dialup".to_string()),
        },
        Some(0),
    )
    .unwrap();
    assert_eq!(
        routes(&file),
        pairs(&[
            ("5559999", "group:dialup"),
            ("5551234", "line:1"),
            ("555*", "group:dialup"),
            ("900*", "line:2"),
        ])
    );
    // The file has them in the order they are tried.
    let text = file.to_toml();
    let at = |called: &str| text.find(&format!("called = \"{called}\"")).unwrap();
    assert!(at("5559999") < at("5551234"));
    assert!(at("5551234") < at("555*"));
    assert!(at("555*") < at("900*"));
    reread(&file);

    file.move_route(3, 1).unwrap();
    assert_eq!(
        routes(&file),
        pairs(&[
            ("5559999", "group:dialup"),
            ("900*", "line:2"),
            ("5551234", "line:1"),
            ("555*", "group:dialup"),
        ])
    );
    reread(&file);
    file.move_route(0, 3).unwrap();
    assert_eq!(routes(&file)[3], pairs(&[("5559999", "group:dialup")])[0]);
    file.move_route(2, 2).unwrap();
    reread(&file);

    file.edit_route(
        0,
        &RouteEdit {
            called: Some("901*".to_string()),
            target: Some("line:1".to_string()),
        },
    )
    .unwrap();
    assert_eq!(routes(&file)[0], pairs(&[("901*", "line:1")])[0]);
    reread(&file);

    file.remove_route(0).unwrap();
    file.remove_route(2).unwrap();
    assert_eq!(
        routes(&file),
        pairs(&[("5551234", "line:1"), ("555*", "group:dialup")])
    );
    reread(&file);
}

#[test]
fn a_route_that_is_not_there_or_not_valid_is_refused() {
    let mut file = file();
    let nowhere = Route {
        called: "1*".to_string(),
        target: Target::Group("nobody".to_string()),
    };
    assert_eq!(
        ids(&file.add_route(&nowhere, None).unwrap_err()),
        ["RMD0030"]
    );
    let fine = Route {
        called: "1*".to_string(),
        target: Target::Line(1),
    };
    let past = file.add_route(&fine, Some(3)).unwrap_err();
    assert_eq!(ids(&past), ["RMD0034"]);
    assert!(past[0].text.contains('3') && past[0].text.contains('2'));
    assert_eq!(ids(&file.remove_route(2).unwrap_err()), ["RMD0034"]);
    assert_eq!(ids(&file.move_route(0, 2).unwrap_err()), ["RMD0034"]);
    assert_eq!(ids(&file.move_route(2, 0).unwrap_err()), ["RMD0034"]);
    assert_eq!(
        ids(&file.edit_route(2, &RouteEdit::default()).unwrap_err()),
        ["RMD0034"]
    );
    let target = file
        .edit_route(
            0,
            &RouteEdit {
                target: Some("trunk:1".to_string()),
                ..RouteEdit::default()
            },
        )
        .unwrap_err();
    assert_eq!(ids(&target), ["RMD0014"]);
    assert_eq!(file.to_toml(), FILE);
}
