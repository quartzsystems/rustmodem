// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Property tests: any valid configuration round-trips through write and
//! read unchanged, and comments survive an edit to an unrelated key.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddrV4};

use config::{
    Backend, Cidr, CodecName, Config, ConfigFile, ControlConfig, EngineKind, HuntGroup, LineConfig,
    LineEdit, LogLevel, MediaConfig, PortRange, RegisterConfig, Route, SerialConfig, ServiceConfig,
    SipConfig, Strategy as HuntStrategy, Target, UnknownCommand,
};
use proptest::prelude::*;

fn log_level() -> impl Strategy<Value = LogLevel> {
    prop::sample::select(LogLevel::ALL.to_vec())
}

fn unknown_command() -> impl Strategy<Value = UnknownCommand> {
    prop_oneof![Just(UnknownCommand::Ok), Just(UnknownCommand::Error)]
}

/// Any text a string key may hold, including what TOML has to escape.
fn free_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..12).prop_map(|chars| chars.into_iter().collect())
}

fn host() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9.-]{0,10}"
}

fn host_port() -> impl Strategy<Value = String> {
    (host(), prop::option::of(1u16..)).prop_map(|(host, port)| match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn register() -> impl Strategy<Value = RegisterConfig> {
    (
        host_port(),
        "[a-zA-Z0-9_.+-]{1,10}",
        free_text(),
        "[!-~]{0,10}",
        30u32..=86_400,
    )
        .prop_map(
            |(server, user, password, auth_user, expires_s)| RegisterConfig {
                server,
                user,
                password,
                auth_user,
                expires_s,
            },
        )
}

/// The parts of a line other than its id and port, which must be unique.
type LineBody = (
    Option<String>,
    String,
    bool,
    u8,
    UnknownCommand,
    EngineKind,
    Option<RegisterConfig>,
);

fn line_body() -> impl Strategy<Value = LineBody> {
    (
        prop::option::of(free_text()),
        "[A-Za-z][A-Za-z0-9.-]{0,8}",
        any::<bool>(),
        any::<u8>(),
        unknown_command(),
        prop::sample::select(EngineKind::ALL.to_vec()),
        prop::option::of(register()),
    )
}

fn sip() -> impl Strategy<Value = SipConfig> {
    (
        any::<bool>(),
        (any::<[u8; 4]>(), 1u16..),
        prop_oneof![Just(String::new()), host()],
        "[ -~]{0,20}",
        prop::collection::vec((any::<[u8; 4]>(), 0u8..=32), 0..4),
        prop_oneof![Just(String::new()), host_port()],
        5u32..=3600,
    )
        .prop_map(
            |(
                enabled,
                (listen, port),
                address,
                user_agent,
                peers,
                outbound_peer,
                ring_timeout_s,
            )| {
                SipConfig {
                    enabled,
                    listen: SocketAddrV4::new(Ipv4Addr::from(listen), port),
                    address,
                    user_agent,
                    trusted_peers: peers
                        .into_iter()
                        .map(|(address, prefix)| Cidr {
                            address: Ipv4Addr::from(address),
                            prefix,
                        })
                        .collect(),
                    outbound_peer,
                    ring_timeout_s,
                }
            },
        )
}

fn media() -> impl Strategy<Value = MediaConfig> {
    (
        prop::sample::select(vec![
            vec![CodecName::Pcmu],
            vec![CodecName::Pcma],
            vec![CodecName::Pcmu, CodecName::Pcma],
            vec![CodecName::Pcma, CodecName::Pcmu],
        ]),
        prop::sample::select(MediaConfig::PTIMES.to_vec()),
        // Two ports or more, so that one of them is even.
        (1024u16..65_000, 1u16..500),
        1u32..=3600,
        free_text(),
    )
        .prop_map(
            |(codecs, ptime_ms, (first, more), rtp_timeout_s, capture_dir)| MediaConfig {
                codecs,
                ptime_ms,
                rtp_ports: PortRange {
                    first,
                    last: first + more,
                },
                rtp_timeout_s,
                capture_dir,
            },
        )
}

/// Routes to the lines and the groups there are; none when there is
/// nothing to send a call to.
fn routes(ids: &[u16], names: &[String]) -> BoxedStrategy<Vec<Route>> {
    let mut targets: Vec<Target> = ids.iter().map(|id| Target::Line(*id)).collect();
    targets.extend(names.iter().cloned().map(Target::Group));
    if targets.is_empty() {
        return Just(Vec::new()).boxed();
    }
    prop::collection::vec(
        (
            prop_oneof![Just("*".to_string()), "[0-9#+]{1,6}\\*?"],
            prop::sample::select(targets),
        ),
        0..5,
    )
    .prop_map(|list| {
        list.into_iter()
            .map(|(called, target)| Route { called, target })
            .collect()
    })
    .boxed()
}

fn valid_config() -> impl Strategy<Value = Config> {
    (
        (log_level(), free_text(), "[!-.0-\\[\\]-~]{1,20}"),
        prop::collection::btree_set(1u16..=999, 0..8),
        prop::collection::btree_set("[A-Za-z0-9_.-]{1,8}", 0..4),
        sip(),
        media(),
    )
        .prop_flat_map(|(service, ids, names, sip, media)| {
            let ids: Vec<u16> = ids.into_iter().collect();
            let names: Vec<String> = names.into_iter().collect();
            let bodies = prop::collection::vec(line_body(), ids.len());
            let groups = (
                prop::collection::btree_set("[0-9#+]{1,8}", names.len()..=names.len()),
                prop::collection::vec(
                    prop::sample::subsequence(ids.clone(), 0..=ids.len()).prop_shuffle(),
                    names.len(),
                ),
                prop::collection::vec(
                    prop::sample::select(HuntStrategy::ALL.to_vec()),
                    names.len(),
                ),
            );
            let routes = routes(&ids, &names);
            (
                Just(service),
                Just(ids),
                Just(names),
                Just(sip),
                Just(media),
                bodies,
                groups,
                routes,
            )
        })
        .prop_map(
            |(
                (log_level, log_dir, pipe),
                ids,
                names,
                sip,
                media,
                bodies,
                (pilots, members, strategies),
                routes,
            )| {
                let lines = ids
                    .into_iter()
                    .zip(bodies)
                    .map(|(id, body)| {
                        let (
                            name,
                            port,
                            enabled,
                            auto_answer_rings,
                            unknown_command,
                            engine,
                            register,
                        ) = body;
                        LineConfig {
                            id,
                            name,
                            serial: SerialConfig {
                                backend: Backend::Com0com,
                                // The id makes the port unique whatever the base.
                                port: format!("{port}_{id}"),
                            },
                            enabled,
                            auto_answer_rings,
                            unknown_command,
                            engine,
                            register,
                        }
                    })
                    .collect();
                let hunt_groups = names
                    .into_iter()
                    .zip(pilots)
                    .zip(members)
                    .zip(strategies)
                    .map(|(((name, pilot), lines), strategy)| HuntGroup {
                        name,
                        pilot,
                        lines,
                        strategy,
                    })
                    .collect();
                Config {
                    service: ServiceConfig { log_level, log_dir },
                    control: ControlConfig { pipe },
                    lines,
                    sip,
                    media,
                    hunt_groups,
                    routes,
                }
            },
        )
        .prop_shuffle_lines()
}

/// Lines need not be in id order in a file; shuffle them.
trait ShuffleLines: Strategy<Value = Config> + Sized {
    fn prop_shuffle_lines(self) -> BoxedStrategy<Config>
    where
        Self: 'static,
    {
        self.prop_flat_map(|config| {
            Just(config.lines.clone())
                .prop_shuffle()
                .prop_map(move |lines| Config {
                    lines,
                    ..config.clone()
                })
        })
        .boxed()
    }
}

impl<S: Strategy<Value = Config>> ShuffleLines for S {}

/// Adds a full-line comment before every line of `text` and a trailing
/// comment after every line that can take one. Returns the commented text
/// and every comment added.
fn add_comments(text: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut comments = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        let above = format!("# above {number}");
        let after = format!("# after {number}");
        out.push_str(&above);
        out.push('\n');
        out.push_str(line);
        out.push(' ');
        out.push_str(&after);
        out.push('\n');
        comments.push(above);
        comments.push(after);
    }
    (out, comments)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn a_valid_config_round_trips_unchanged(config in valid_config()) {
        let file = ConfigFile::from_config(&config).unwrap();
        prop_assert_eq!(file.config(), &config);

        let text = file.to_toml();
        let again = ConfigFile::parse(&text).unwrap();
        prop_assert_eq!(again.config(), &config);
        // Writing what was read changes nothing either.
        prop_assert_eq!(again.to_toml(), text);
    }

    #[test]
    fn comments_survive_an_edit_to_an_unrelated_key(
        config in valid_config().prop_filter("needs a line", |c| !c.lines.is_empty()),
        pick in any::<prop::sample::Index>(),
        rings in any::<u8>(),
        enabled in any::<bool>(),
        setting in unknown_command(),
        which in 0usize..4,
    ) {
        let plain = ConfigFile::from_config(&config).unwrap().to_toml();
        // A string with a line break is written as a multi-line string, and
        // the comment injector below works a line at a time.
        prop_assume!(!plain.contains("\"\"\"") && !plain.contains("'''"));
        let (commented, comments) = add_comments(&plain);
        let mut file = ConfigFile::parse(&commented).unwrap();
        prop_assert_eq!(file.config(), &config);

        let index = pick.index(config.lines.len());
        let id = config.lines[index].id;
        let mut expected = config.clone();
        let mut edit = LineEdit::default();
        match which {
            0 => {
                edit.auto_answer_rings = Some(rings);
                expected.lines[index].auto_answer_rings = rings;
            }
            1 => {
                edit.enabled = Some(enabled);
                expected.lines[index].enabled = enabled;
            }
            2 => {
                edit.unknown_command = Some(setting);
                expected.lines[index].unknown_command = setting;
            }
            _ => {
                edit.name = Some(Some("edited".to_string()));
                expected.lines[index].name = Some("edited".to_string());
            }
        }
        file.edit_line(id, &edit).unwrap();
        prop_assert_eq!(file.config(), &expected);

        let text = file.to_toml();
        let present: BTreeSet<&str> = text
            .lines()
            .filter_map(|line| line.rfind("# a").map(|at| &line[at..]))
            .collect();
        for comment in &comments {
            prop_assert!(
                present.contains(comment.as_str()),
                "comment {:?} was lost:\n{}",
                comment,
                text
            );
        }
        let again = ConfigFile::parse(&text).unwrap();
        prop_assert_eq!(again.config(), &expected);
    }
}
