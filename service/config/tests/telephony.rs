// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The sections that came with SIP: `[sip]`, `[media]`, a line's `engine`
//! and `register`, `[[hunt_group]]` and `[[route]]`.

use std::net::Ipv4Addr;

use config::{
    Cidr, CodecName, Config, ConfigFile, EngineKind, HuntGroup, MediaConfig, PortRange,
    RegisterConfig, Route, SipConfig, Strategy, Target, pattern_covers, pattern_matches,
};

/// The example of the phase 2 handoff.
const EXAMPLE: &str = r#"[sip]
enabled = true
listen = "0.0.0.0:5060"            # UDP
address = ""                        # address to advertise
user_agent = ""                     # empty = "rustmodem/<version>"
trusted_peers = ["10.10.20.11"]     # addresses or CIDR blocks
outbound_peer = ""                  # host[:port]
ring_timeout_s = 60

[media]
codecs = ["pcmu", "pcma"]           # preference order
ptime_ms = 20                       # 10, 20 or 30
rtp_ports = "20000-20999"           # even ports are used
rtp_timeout_s = 10
capture_dir = ""                    # empty = <log_dir>\captures

[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }
engine = "null"                     # "null" or "echo" in this phase

[[line]]
id = 2
serial = { backend = "com0com", port = "CNCB1" }
register = { server = "pbx.example.lan", user = "7002", password = "secret", auth_user = "", expires_s = 300 }

[[hunt_group]]
name = "dialin"
pilot = "5551000"
lines = [1, 2]
strategy = "first-idle"             # "first-idle", "round-robin", "least-recent"

[[route]]
called = "5552*"                    # exact digits, or a prefix ending in *
target = "line:2"                   # "line:<id>" or "group:<name>"
"#;

fn ids(problems: &[msg::Message]) -> Vec<&str> {
    problems.iter().map(|m| m.id.as_str()).collect()
}

/// The problems of a configuration: two lines, then `rest`.
fn problems(rest: &str) -> Vec<msg::Message> {
    let text = format!(
        "[[line]]\nid = 1\nserial = {{ backend = \"com0com\", port = \"CNCB0\" }}\n\
         [[line]]\nid = 2\nserial = {{ backend = \"com0com\", port = \"CNCB1\" }}\n{rest}"
    );
    match ConfigFile::parse(&text) {
        Ok(_) => Vec::new(),
        Err(problems) => problems,
    }
}

#[test]
fn the_handoff_example_parses() {
    let file = ConfigFile::parse(EXAMPLE).unwrap();
    assert!(file.warnings().is_empty());
    let config = file.config();

    assert_eq!(
        config.sip,
        SipConfig {
            enabled: true,
            listen: "0.0.0.0:5060".parse().unwrap(),
            address: String::new(),
            user_agent: String::new(),
            trusted_peers: vec![Cidr {
                address: Ipv4Addr::new(10, 10, 20, 11),
                prefix: 32
            }],
            outbound_peer: String::new(),
            ring_timeout_s: 60,
        }
    );
    assert_eq!(
        config.media,
        MediaConfig {
            codecs: vec![CodecName::Pcmu, CodecName::Pcma],
            ptime_ms: 20,
            rtp_ports: PortRange {
                first: 20000,
                last: 20999
            },
            rtp_timeout_s: 10,
            capture_dir: String::new(),
        }
    );
    assert_eq!(config.lines[0].engine, EngineKind::Null);
    assert_eq!(config.lines[0].register, None);
    assert_eq!(
        config.lines[1].register,
        Some(RegisterConfig {
            server: "pbx.example.lan".to_string(),
            user: "7002".to_string(),
            password: "secret".to_string(),
            auth_user: String::new(),
            expires_s: 300,
        })
    );
    assert_eq!(
        config.hunt_groups,
        [HuntGroup {
            name: "dialin".to_string(),
            pilot: "5551000".to_string(),
            lines: vec![1, 2],
            strategy: Strategy::FirstIdle,
        }]
    );
    assert_eq!(
        config.routes,
        [Route {
            called: "5552*".to_string(),
            target: Target::Line(2),
        }]
    );
    assert_eq!(config.hunt_group("dialin").unwrap().pilot, "5551000");
    assert!(config.hunt_group("other").is_none());
    assert_eq!(config.hunt_groups_of(1), ["dialin"]);
    assert!(config.hunt_groups_of(3).is_empty());

    // Untouched, it is written back byte for byte.
    assert_eq!(file.to_toml(), EXAMPLE);
}

#[test]
fn a_file_with_none_of_the_new_sections_is_as_it_was() {
    let file = ConfigFile::parse(
        "[[line]]\nid = 1\nserial = { backend = \"com0com\", port = \"CNCB0\" }\n",
    )
    .unwrap();
    let config = file.config();
    assert_eq!(config.sip, SipConfig::default());
    assert!(!config.sip.enabled);
    assert_eq!(config.media, MediaConfig::default());
    assert!(config.hunt_groups.is_empty());
    assert!(config.routes.is_empty());
    assert_eq!(config.lines[0].engine, EngineKind::Null);
    assert_eq!(config.lines[0].register, None);
    // And a configuration with none of them is rendered with none of them.
    let rendered = ConfigFile::from_config(config).unwrap().to_toml();
    for section in [
        "[sip]",
        "[media]",
        "hunt_group",
        "route",
        "engine",
        "register",
    ] {
        assert!(!rendered.contains(section), "{section} in:\n{rendered}");
    }
}

#[test]
fn the_defaults_of_each_section() {
    let file = ConfigFile::parse(
        "[sip]\n[media]\n\
         [[line]]\nid = 1\nserial = { backend = \"com0com\", port = \"CNCB0\" }\n\
         register = { server = \"pbx\", user = \"7001\" }\n\
         [[hunt_group]]\nname = \"g\"\npilot = \"100\"\n",
    )
    .unwrap();
    let config = file.config();
    // A [sip] table does not turn SIP on by being there.
    assert_eq!(config.sip, SipConfig::default());
    assert_eq!(config.sip.listen, "0.0.0.0:5060".parse().unwrap());
    assert_eq!(config.sip.ring_timeout_s, 60);
    assert_eq!(config.media.codecs, [CodecName::Pcmu, CodecName::Pcma]);
    assert_eq!(config.media.ptime_ms, 20);
    assert_eq!(config.media.rtp_ports.to_string(), "20000-20999");
    assert_eq!(config.media.rtp_timeout_s, 10);
    let register = config.lines[0].register.as_ref().unwrap();
    assert_eq!(register.password, "");
    assert_eq!(register.auth_user, "");
    assert_eq!(register.expires_s, 300);
    assert_eq!(config.hunt_groups[0].strategy, Strategy::FirstIdle);
    assert!(config.hunt_groups[0].lines.is_empty());
}

#[test]
fn every_value_that_is_allowed() {
    let file = ConfigFile::parse(
        r#"[sip]
enabled = true
listen = "10.10.20.5:5070"
address = "modems.example.lan"
user_agent = "Quartz rustmodem (test)"
trusted_peers = ["10.10.20.0/24", "192.0.2.7", "0.0.0.0/0"]
outbound_peer = "gw.example.lan:5062"
ring_timeout_s = 5

[media]
codecs = ["pcma"]
ptime_ms = 30
rtp_ports = "1024-1025"
rtp_timeout_s = 3600
capture_dir = 'D:\captures'

[[line]]
id = 7
serial = { backend = "com0com", port = "CNCB7" }
engine = "echo"

[line.register]
server = "10.10.20.1:5080"
user = "+1-555.7007_a"
password = "p\"w"
auth_user = "auth@realm"
expires_s = 86400

[[hunt_group]]
name = "A.b-c_1"
pilot = "+15551000#"
lines = [7]
strategy = "least-recent"

[[hunt_group]]
name = "rr"
pilot = "200"
lines = []
strategy = "round-robin"

[[route]]
called = "*"
target = "group:rr"

[[route]]
called = "+1555*"
target = "line:7"
"#,
    )
    .unwrap();
    let config = file.config();
    assert_eq!(config.sip.trusted_peers.len(), 3);
    assert_eq!(config.sip.trusted_peers[0].to_string(), "10.10.20.0/24");
    assert_eq!(config.sip.outbound_peer, "gw.example.lan:5062");
    assert_eq!(config.media.codecs, [CodecName::Pcma]);
    assert_eq!(config.media.capture_dir, r"D:\captures");
    assert_eq!(config.lines[0].engine, EngineKind::Echo);
    let register = config.lines[0].register.as_ref().unwrap();
    assert_eq!(register.password, "p\"w");
    assert_eq!(register.expires_s, 86_400);
    assert_eq!(config.hunt_groups[0].strategy, Strategy::LeastRecent);
    assert_eq!(config.routes[0].target, Target::Group("rr".to_string()));
    // The second route can never match: the first takes everything.
    assert_eq!(ids(file.warnings()), ["RMD0031"]);
}

#[test]
fn every_problem_is_reported_at_once_with_key_and_line() {
    let found = problems(
        r#"[sip]
enabled = "yes"
listen = "5060"
address = "not a host"
user_agent = "tab	here"
trusted_peers = ["10.10.20.11", "10.10.20.0/33", "pbx.example.lan"]
outbound_peer = "gw:0"
ring_timeout_s = 4

[media]
codecs = ["pcmu", "g729", "pcmu"]
ptime_ms = 40
rtp_ports = "20001-20001"
rtp_timeout_s = 0
capture_dir = 5

[[line]]
id = 3
serial = { backend = "com0com", port = "CNCB2" }
engine = "v90"
register = { server = "pbx:port", user = "70 02", auth_user = "a b", expires_s = 10 }

[[line]]
id = 4
serial = { backend = "com0com", port = "CNCB3" }
register = { password = "secret" }

[[hunt_group]]
name = "has space"
pilot = "12a"
lines = [1, 1, 9, 1000]
strategy = "random"

[[hunt_group]]
pilot = "100"

[[route]]
called = "55*5"
target = "trunk:1"

[[route]]
target = "line:1"
"#,
    );
    let texts: Vec<String> = found.iter().map(|m| m.to_string()).collect();
    let expect = [
        "RMD0013: Key sip.enabled at line 8 has the wrong type; boolean expected.",
        "RMD0014: Key sip.listen at line 9 has value 5060, which is not allowed.",
        "RMD0014: Key sip.address at line 10 has value not a host, which is not allowed.",
        "RMD0014: Key sip.user_agent at line 11 has value tab\there, which is not allowed.",
        "RMD0014: Key sip.trusted_peers at line 12 has value 10.10.20.0/33, which is not allowed.",
        "RMD0014: Key sip.trusted_peers at line 12 has value pbx.example.lan, which is not allowed.",
        "RMD0014: Key sip.outbound_peer at line 13 has value gw:0, which is not allowed.",
        "RMD0014: Key sip.ring_timeout_s at line 14 has value 4, which is not allowed.",
        "RMD0014: Key media.codecs at line 17 has value g729, which is not allowed.",
        "RMD0014: Key media.codecs at line 17 has value pcmu, which is not allowed.",
        "RMD0014: Key media.ptime_ms at line 18 has value 40, which is not allowed.",
        "RMD0014: Key media.rtp_ports at line 19 has value 20001-20001, which is not allowed.",
        "RMD0014: Key media.rtp_timeout_s at line 20 has value 0, which is not allowed.",
        "RMD0013: Key media.capture_dir at line 21 has the wrong type; string expected.",
        "RMD0014: Key line.engine at line 26 has value v90, which is not allowed.",
        "RMD0014: Key line.register.server at line 27 has value pbx:port, which is not allowed.",
        "RMD0014: Key line.register.user at line 27 has value 70 02, which is not allowed.",
        "RMD0014: Key line.register.auth_user at line 27 has value a b, which is not allowed.",
        "RMD0014: Key line.register.expires_s at line 27 has value 10, which is not allowed.",
        "RMD0015: Required key line.register.server is missing from the table at line 29.",
        "RMD0015: Required key line.register.user is missing from the table at line 29.",
        "RMD0014: Key hunt_group.name at line 35 has value has space, which is not allowed.",
        "RMD0014: Key hunt_group.pilot at line 36 has value 12a, which is not allowed.",
        "RMD0014: Key hunt_group.lines at line 37 has value 1, which is not allowed.",
        "RMD0027: Hunt group  at line 37 names line 9, which does not exist.",
        "RMD0014: Key hunt_group.lines at line 37 has value 1000, which is not allowed.",
        "RMD0014: Key hunt_group.strategy at line 38 has value random, which is not allowed.",
        "RMD0015: Required key hunt_group.name is missing from the table at line 40.",
        "RMD0014: Key route.called at line 44 has value 55*5, which is not allowed.",
        "RMD0014: Key route.target at line 45 has value trunk:1, which is not allowed.",
        "RMD0015: Required key route.called is missing from the table at line 47.",
    ];
    assert_eq!(texts, expect, "{texts:#?}");
    // The password of a table that is refused is not in any message.
    assert!(found.iter().all(|m| {
        !m.text.contains("secret") && !m.cause.contains("secret") && !m.recovery.contains("secret")
    }));
}

#[test]
fn wrong_shapes_are_type_errors() {
    for (rest, key) in [
        ("sip = 1\n", "sip"),
        ("media = \"x\"\n", "media"),
        ("hunt_group = { name = \"g\" }\n", "hunt_group"),
        ("route = [1]\n", "route"),
        ("[sip]\ntrusted_peers = \"10.0.0.1\"\n", "sip.trusted_peers"),
        ("[sip]\ntrusted_peers = [1]\n", "sip.trusted_peers"),
        ("[media]\ncodecs = [\"pcmu\", 8]\n", "media.codecs"),
        ("[media]\nptime_ms = \"20\"\n", "media.ptime_ms"),
        ("[media]\nrtp_ports = 20000\n", "media.rtp_ports"),
        (
            "[[hunt_group]]\nname = \"g\"\npilot = \"1\"\nlines = \"1\"\n",
            "hunt_group.lines",
        ),
        (
            "[[hunt_group]]\nname = \"g\"\npilot = \"1\"\nlines = [\"1\"]\n",
            "hunt_group.lines",
        ),
        (
            "[[hunt_group]]\nname = \"g\"\npilot = 100\n",
            "hunt_group.pilot",
        ),
        (
            "[[route]]\ncalled = 555\ntarget = \"line:1\"\n",
            "route.called",
        ),
    ] {
        // A key of the root has to come before the first table.
        let found = if rest.starts_with('[') {
            problems(rest)
        } else {
            ConfigFile::parse(rest).err().unwrap_or_default()
        };
        assert_eq!(ids(&found), ["RMD0013"], "{rest}");
        assert!(
            found[0].text.contains(&format!("Key {key} ")),
            "{}",
            found[0].text
        );
    }
    assert_eq!(ids(&problems("[media]\ncodecs = []\n")), ["RMD0014"]);
}

#[test]
fn a_line_with_a_register_table_that_is_not_a_table() {
    let found = ConfigFile::parse(
        "[[line]]\nid = 1\nserial = { backend = \"com0com\", port = \"CNCB0\" }\nregister = true\n",
    )
    .unwrap_err();
    assert_eq!(ids(&found), ["RMD0013"]);
}

#[test]
fn names_and_pilots_are_unique_and_members_and_targets_exist() {
    let found = problems(
        r#"[[hunt_group]]
name = "a"
pilot = "100"
lines = [1, 2]

[[hunt_group]]
name = "a"
pilot = "200"

[[hunt_group]]
name = "b"
pilot = "100"
lines = [3]

[[route]]
called = "300"
target = "line:3"

[[route]]
called = "400"
target = "group:c"

[[route]]
called = "500"
target = "group:a"
"#,
    );
    let texts: Vec<String> = found.iter().map(|m| m.to_string()).collect();
    assert_eq!(
        texts,
        [
            "RMD0028: Hunt group name a at line 13 is already used at line 8.",
            "RMD0029: Pilot number 100 at line 18 is already used at line 9.",
            "RMD0027: Hunt group b at line 19 names line 3, which does not exist.",
            "RMD0030: Route 300 at line 23 has target line:3, which does not exist.",
            "RMD0030: Route 400 at line 27 has target group:c, which does not exist.",
        ]
    );
}

#[test]
fn a_route_that_an_earlier_one_shadows_is_a_warning_not_an_error() {
    let text = r#"[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }

[[route]]
called = "555*"
target = "line:1"

[[route]]
called = "5551000"
target = "line:1"

[[route]]
called = "5559*"
target = "line:1"

[[route]]
called = "556*"
target = "line:1"

[[route]]
called = "5561000"
target = "line:1"

[[route]]
called = "5561000"
target = "line:1"

[[route]]
called = "55*"
target = "line:1"
"#;
    let file = ConfigFile::parse(text).unwrap();
    assert_eq!(file.config().routes.len(), 7);
    let warnings: Vec<String> = file.warnings().iter().map(|m| m.to_string()).collect();
    assert_eq!(
        warnings,
        [
            "RMD0031: Route 5551000 at line 10 never gets a call: route 555* at line 6 takes them first.",
            "RMD0031: Route 5559* at line 14 never gets a call: route 555* at line 6 takes them first.",
            "RMD0031: Route 5561000 at line 22 never gets a call: route 556* at line 18 takes them first.",
            "RMD0031: Route 5561000 at line 26 never gets a call: route 556* at line 18 takes them first.",
        ]
    );
    assert!(
        file.warnings()
            .iter()
            .all(|m| m.severity == msg::Severity::Warning)
    );
}

#[test]
fn patterns() {
    for (pattern, number, matches) in [
        ("5551000", "5551000", true),
        ("5551000", "555100", false),
        ("5551000", "55510001", false),
        ("5552*", "5552", true),
        ("5552*", "55529999", true),
        ("5552*", "5553000", false),
        ("*", "", true),
        ("*", "anything", true),
        ("", "", true),
    ] {
        assert_eq!(
            pattern_matches(pattern, number),
            matches,
            "{pattern} {number}"
        );
    }
    for (earlier, later, covers) in [
        ("555*", "5551000", true),
        ("555*", "5559*", true),
        ("555*", "555*", true),
        ("5559*", "555*", false),
        ("5551000", "5551000", true),
        ("5551000", "555*", false),
        ("5551000", "5551001", false),
        ("*", "1", true),
        ("*", "1*", true),
    ] {
        assert_eq!(pattern_covers(earlier, later), covers, "{earlier} {later}");
    }
}

#[test]
fn addresses_and_blocks() {
    let peer = |text: &str| Cidr::parse(text).unwrap();
    let ip = |text: &str| text.parse::<Ipv4Addr>().unwrap();
    assert!(peer("10.10.20.11").contains(ip("10.10.20.11")));
    assert!(!peer("10.10.20.11").contains(ip("10.10.20.12")));
    assert!(peer("10.10.20.0/24").contains(ip("10.10.20.255")));
    assert!(!peer("10.10.20.0/24").contains(ip("10.10.21.0")));
    // The host bits of a block are not looked at.
    assert!(peer("10.10.20.99/24").contains(ip("10.10.20.1")));
    assert!(peer("0.0.0.0/0").contains(ip("203.0.113.9")));
    assert!(peer("10.10.20.10/31").contains(ip("10.10.20.11")));
    assert!(!peer("10.10.20.10/31").contains(ip("10.10.20.12")));
    assert_eq!(peer("10.10.20.11/32").to_string(), "10.10.20.11");
    for bad in [
        "",
        "10.10.20",
        "10.10.20.0/",
        "10.10.20.0/33",
        "10.10.20.0/-1",
        "host/24",
        "::1",
    ] {
        assert_eq!(Cidr::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn port_ranges() {
    let range = PortRange::parse("20000-20005").unwrap();
    assert_eq!(
        range.even_ports().collect::<Vec<_>>(),
        [20000, 20002, 20004]
    );
    assert_eq!(
        PortRange::parse("20001-20002")
            .unwrap()
            .even_ports()
            .count(),
        1
    );
    assert_eq!(
        PortRange::parse("65534-65535")
            .unwrap()
            .even_ports()
            .count(),
        1
    );
    for bad in [
        "",
        "20000",
        "20000-",
        "-20000",
        "20002-20000",
        "20001-20001",
        "1000-2000",
        "20000-70000",
        "a-b",
        "20000 - 20010",
    ] {
        assert_eq!(PortRange::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn targets_and_names() {
    assert_eq!(Target::parse("line:12"), Some(Target::Line(12)));
    assert_eq!(
        Target::parse("group:dialin"),
        Some(Target::Group("dialin".to_string()))
    );
    assert_eq!(Target::Line(12).to_string(), "line:12");
    assert_eq!(
        Target::Group("dialin".to_string()).to_string(),
        "group:dialin"
    );
    for bad in [
        "", "line", "line:", "line:x", "line:-1", "group:", "trunk:1", "12",
    ] {
        assert_eq!(Target::parse(bad), None, "{bad:?}");
    }
    for strategy in Strategy::ALL {
        assert_eq!(Strategy::parse(strategy.as_str()), Some(strategy));
    }
    for engine in EngineKind::ALL {
        assert_eq!(EngineKind::parse(&engine.to_string()), Some(engine));
    }
    for codec in CodecName::ALL {
        assert_eq!(CodecName::parse(&codec.to_string()), Some(codec));
    }
    assert_eq!(Strategy::parse("first idle"), None);
    assert_eq!(CodecName::parse("PCMU"), None);
}

#[test]
fn the_password_is_not_printed() {
    let file = ConfigFile::parse(EXAMPLE).unwrap();
    let printed = format!("{:?}", file.config());
    assert!(printed.contains("7002"));
    assert!(printed.contains("has_password: true"));
    assert!(!printed.contains("secret"), "{printed}");
    // Written to the file it is, where it has to be.
    assert!(file.to_toml().contains("password = \"secret\""));
    let _: &Config = file.config();
}
