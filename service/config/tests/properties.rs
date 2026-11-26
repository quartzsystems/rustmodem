// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Property tests: any valid configuration round-trips through write and
//! read unchanged, and comments survive an edit to an unrelated key.

use std::collections::BTreeSet;

use config::{
    Backend, Config, ConfigFile, ControlConfig, LineConfig, LineEdit, LogLevel, SerialConfig,
    ServiceConfig, UnknownCommand,
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

/// The parts of a line other than its id and port, which must be unique.
fn line_body() -> impl Strategy<Value = (Option<String>, String, bool, u8, UnknownCommand)> {
    (
        prop::option::of(free_text()),
        "[A-Za-z][A-Za-z0-9.-]{0,8}",
        any::<bool>(),
        any::<u8>(),
        unknown_command(),
    )
}

fn valid_config() -> impl Strategy<Value = Config> {
    (
        log_level(),
        free_text(),
        "[!-.0-\\[\\]-~]{1,20}",
        prop::collection::btree_set(1u16..=999, 0..8),
    )
        .prop_flat_map(|(level, log_dir, pipe, ids)| {
            let ids: Vec<u16> = ids.into_iter().collect();
            let bodies = prop::collection::vec(line_body(), ids.len());
            (Just(level), Just(log_dir), Just(pipe), Just(ids), bodies)
        })
        .prop_map(|(log_level, log_dir, pipe, ids, bodies)| {
            let lines = ids
                .into_iter()
                .zip(bodies)
                .map(
                    |(id, (name, port, enabled, auto_answer_rings, unknown_command))| LineConfig {
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
                    },
                )
                .collect();
            Config {
                service: ServiceConfig { log_level, log_dir },
                control: ControlConfig { pipe },
                lines,
            }
        })
        .prop_shuffle_lines()
}

/// Lines need not be in id order in a file; shuffle them.
trait ShuffleLines: Strategy<Value = Config> + Sized {
    fn prop_shuffle_lines(self) -> BoxedStrategy<Config>
    where
        Self: 'static,
    {
        self.prop_flat_map(|config| {
            let Config {
                service,
                control,
                lines,
            } = config;
            Just(lines).prop_shuffle().prop_map(move |lines| Config {
                service: service.clone(),
                control: control.clone(),
                lines,
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
