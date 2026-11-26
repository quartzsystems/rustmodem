// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The schema, validation and write-back of the configuration file.

use config::{
    Backend, Config, ConfigFile, EngineKind, LineConfig, LineEdit, LogLevel, SerialConfig,
    SettingsEdit, UnknownCommand,
};

/// The example from the phase 0 handoff, with a later-phase section added.
const EXAMPLE: &str = r#"# rustmodem configuration
[service]
log_level = "info"            # error, warning, info, debug
log_dir = ""                  # empty = %ProgramData%\Quartz Systems\rustmodem\logs

[control]
pipe = "rustmodem-control"    # \\.\pipe\<name>

# A later phase's section: unknown to phase 0, kept as it is.
[sip]
registrar = "sip.example.net" # the registrar
ports = [5060, 5061]

[[line]]
id = 1                        # 1..=999, unique
name = "Line 1"               # optional
serial = { backend = "com0com", port = "CNCB0" }
enabled = true
auto_answer_rings = 0         # initial S0; 0 = off
unknown_command = "ok"        # "ok" or "error"

[[line]]
id = 2
serial = { backend = "com0com", port = "CNCB1" }
"#;

fn ids(problems: &[msg::Message]) -> Vec<&str> {
    problems.iter().map(|m| m.id.as_str()).collect()
}

#[test]
fn the_handoff_example_parses() {
    let file = ConfigFile::parse(EXAMPLE).unwrap();
    let config = file.config();
    assert_eq!(config.service.log_level, LogLevel::Info);
    assert_eq!(config.service.log_dir, "");
    assert_eq!(config.control.pipe, "rustmodem-control");
    assert_eq!(config.lines.len(), 2);
    assert_eq!(
        config.lines[0],
        LineConfig {
            id: 1,
            name: Some("Line 1".to_string()),
            serial: SerialConfig {
                backend: Backend::Com0com,
                port: "CNCB0".to_string(),
            },
            enabled: true,
            auto_answer_rings: 0,
            unknown_command: UnknownCommand::Ok,
            engine: EngineKind::Null,
            register: None,
        }
    );
    assert_eq!(config.lines[1], LineConfig::new(2, "CNCB1"));
    assert_eq!(config.line(2).unwrap().serial.port, "CNCB1");
    assert!(config.line(3).is_none());
}

#[test]
fn an_untouched_file_writes_back_byte_for_byte() {
    let file = ConfigFile::parse(EXAMPLE).unwrap();
    assert_eq!(file.to_toml(), EXAMPLE);
}

#[test]
fn an_empty_file_is_the_default_configuration() {
    let file = ConfigFile::parse("").unwrap();
    assert_eq!(*file.config(), Config::default());
    assert_eq!(file.config().control.pipe, "rustmodem-control");
}

#[test]
fn a_syntax_error_reports_its_line() {
    let problems = ConfigFile::parse("[service]\nlog_level = \n").unwrap_err();
    assert_eq!(ids(&problems), ["RMD0012"]);
    assert!(problems[0].text.contains("line 2"), "{}", problems[0].text);
}

#[test]
fn every_problem_is_reported_at_once_with_key_and_line() {
    let text = r#"[service]
log_level = "loud"

[control]
pipe = 7

[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }

[[line]]
id = 1
serial = { backend = "com0com", port = "cncb0" }

[[line]]
id = 1000
serial = { backend = "usb", port = "X" }
auto_answer_rings = 300
unknown_command = "maybe"
enabled = "yes"

[[line]]
name = 5
"#;
    let problems = ConfigFile::parse(text).unwrap_err();
    let texts: Vec<String> = problems.iter().map(|m| m.to_string()).collect();
    let expect = [
        "RMD0014: Key service.log_level at line 2 has value loud, which is not allowed.",
        "RMD0013: Key control.pipe at line 5 has the wrong type; string expected.",
        "RMD0016: Line id 1 at line 12 is already used at line 8.",
        "RMD0017: Serial port cncb0 at line 13 is already used at line 9.",
        "RMD0014: Key line.id at line 16 has value 1000, which is not allowed.",
        "RMD0018: Serial backend usb at line 17 is not known.",
        "RMD0013: Key line.enabled at line 20 has the wrong type; boolean expected.",
        "RMD0014: Key line.auto_answer_rings at line 18 has value 300, which is not allowed.",
        "RMD0014: Key line.unknown_command at line 19 has value maybe, which is not allowed.",
        "RMD0015: Required key line.id is missing from the table at line 22.",
        "RMD0013: Key line.name at line 23 has the wrong type; string expected.",
        "RMD0015: Required key line.serial is missing from the table at line 22.",
    ];
    assert_eq!(texts, expect);
    // The cause of an out-of-range value says what is allowed.
    assert!(problems[4].cause.contains("1 to 999"));
}

#[test]
fn wrong_shapes_are_type_errors() {
    let problems = ConfigFile::parse("service = 3\nline = 4\n").unwrap_err();
    assert_eq!(ids(&problems), ["RMD0013", "RMD0013"]);
    let problems = ConfigFile::parse("[[line]]\nid = 1\nserial = \"CNCB0\"\n").unwrap_err();
    assert_eq!(ids(&problems), ["RMD0013"]);
    let problems =
        ConfigFile::parse("[[line]]\nid = 1\nserial = { backend = \"com0com\" }\n").unwrap_err();
    assert_eq!(ids(&problems), ["RMD0015"]);
    let problems =
        ConfigFile::parse("[[line]]\nid = 1\nserial = { backend = \"com0com\", port = \"a b\" }\n")
            .unwrap_err();
    assert_eq!(ids(&problems), ["RMD0014"]);
    let problems = ConfigFile::parse("[control]\npipe = \"a\\\\b\"\n").unwrap_err();
    assert_eq!(ids(&problems), ["RMD0014"]);
}

#[test]
fn the_serial_table_may_be_a_sub_table() {
    let text = "[[line]]\nid = 4\n[line.serial]\nbackend = \"com0com\"\nport = \"CNCB7\"\n";
    let mut file = ConfigFile::parse(text).unwrap();
    assert_eq!(file.config().lines[0], LineConfig::new(4, "CNCB7"));
    file.edit_line(
        4,
        &LineEdit {
            port: Some("CNCB8".to_string()),
            ..LineEdit::default()
        },
    )
    .unwrap();
    assert_eq!(file.config().lines[0].serial.port, "CNCB8");
    assert!(file.to_toml().contains("[line.serial]"));
}

#[test]
fn adding_a_line_keeps_everything_else() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    let mut line = LineConfig::new(7, "CNCB6");
    line.name = Some("Seven".to_string());
    line.auto_answer_rings = 2;
    line.unknown_command = UnknownCommand::Error;
    line.enabled = false;
    file.add_line(&line).unwrap();

    let text = file.to_toml();
    assert!(text.starts_with(EXAMPLE.trim_end()), "{text}");
    let again = ConfigFile::parse(&text).unwrap();
    assert_eq!(again.config().lines.len(), 3);
    assert_eq!(again.config().lines[2], line);
    assert_eq!(again.config(), file.config());
}

#[test]
fn adding_a_line_to_a_file_with_none_creates_the_array() {
    let mut file = ConfigFile::parse("[service]\nlog_level = \"debug\"\n").unwrap();
    file.add_line(&LineConfig::new(1, "CNCB0")).unwrap();
    let again = ConfigFile::parse(&file.to_toml()).unwrap();
    assert_eq!(again.config().lines, [LineConfig::new(1, "CNCB0")]);
    assert_eq!(again.config().service.log_level, LogLevel::Debug);
}

#[test]
fn adding_refuses_a_used_id_or_port_and_changes_nothing() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    let problems = file.add_line(&LineConfig::new(2, "CNCB9")).unwrap_err();
    assert_eq!(ids(&problems), ["RMD0020"]);
    let problems = file.add_line(&LineConfig::new(3, "cncb1")).unwrap_err();
    assert_eq!(ids(&problems), ["RMD0017"]);
    let problems = file.add_line(&LineConfig::new(0, "CNCB9")).unwrap_err();
    assert_eq!(ids(&problems), ["RMD0014"]);
    assert_eq!(file.to_toml(), EXAMPLE);
}

#[test]
fn editing_a_line_keeps_the_comment_on_the_edited_key() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    file.edit_line(
        1,
        &LineEdit {
            name: Some(Some("Front desk".to_string())),
            port: Some("CNCB4".to_string()),
            enabled: Some(false),
            auto_answer_rings: Some(3),
            unknown_command: Some(UnknownCommand::Error),
            ..LineEdit::default()
        },
    )
    .unwrap();

    let line = &file.config().lines[0];
    assert_eq!(line.name.as_deref(), Some("Front desk"));
    assert_eq!(line.serial.port, "CNCB4");
    assert!(!line.enabled);
    assert_eq!(line.auto_answer_rings, 3);
    assert_eq!(line.unknown_command, UnknownCommand::Error);

    let text = file.to_toml();
    assert!(
        text.contains("name = \"Front desk\"               # optional"),
        "{text}"
    );
    assert!(
        text.contains("auto_answer_rings = 3         # initial S0; 0 = off"),
        "{text}"
    );
    assert!(
        text.contains("unknown_command = \"error\"        # \"ok\" or \"error\""),
        "{text}"
    );
    assert!(
        text.contains("serial = { backend = \"com0com\", port = \"CNCB4\" }"),
        "{text}"
    );
    assert!(text.contains("registrar = \"sip.example.net\" # the registrar"));
}

#[test]
fn editing_can_remove_the_name_and_add_missing_keys() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    file.edit_line(
        1,
        &LineEdit {
            name: Some(None),
            ..LineEdit::default()
        },
    )
    .unwrap();
    assert_eq!(file.config().lines[0].name, None);

    file.edit_line(
        2,
        &LineEdit {
            enabled: Some(false),
            ..LineEdit::default()
        },
    )
    .unwrap();
    assert!(!file.config().lines[1].enabled);
    assert!(file.to_toml().trim_end().ends_with("enabled = false"));
}

#[test]
fn editing_refuses_an_unknown_line_or_an_invalid_result() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    let problems = file.edit_line(9, &LineEdit::default()).unwrap_err();
    assert_eq!(ids(&problems), ["RMD0019"]);
    let problems = file
        .edit_line(
            2,
            &LineEdit {
                port: Some("CNCB0".to_string()),
                ..LineEdit::default()
            },
        )
        .unwrap_err();
    assert_eq!(ids(&problems), ["RMD0017"]);
    assert_eq!(file.to_toml(), EXAMPLE);
}

#[test]
fn removing_a_line_keeps_the_others() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    file.remove_line(1).unwrap();
    assert_eq!(file.config().lines, [LineConfig::new(2, "CNCB1")]);
    let text = file.to_toml();
    assert!(!text.contains("CNCB0"));
    assert!(text.contains("# A later phase's section"));
    assert!(text.contains("ports = [5060, 5061]"));
    assert_eq!(ids(&file.remove_line(1).unwrap_err()), ["RMD0019"]);
}

#[test]
fn save_and_load_round_trip_through_a_file() {
    let directory = std::env::temp_dir().join(format!("rustmodem-config-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("rustmodem.toml");

    let file = ConfigFile::parse(EXAMPLE).unwrap();
    file.save(&path).unwrap();
    let loaded = ConfigFile::load(&path).unwrap();
    assert_eq!(loaded.to_toml(), EXAMPLE);
    assert_eq!(loaded.config(), file.config());

    let missing = ConfigFile::load(&directory.join("absent.toml")).unwrap_err();
    assert_eq!(ids(&missing), ["RMD0010"]);
    let unwritable = file
        .save(&directory.join("no-such-dir").join("x.toml"))
        .unwrap_err();
    assert_eq!(unwritable.id, "RMD0011");

    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn default_paths_are_under_the_data_directory() {
    let data = config::data_dir();
    assert!(
        data.ends_with("Quartz Systems/rustmodem") || data.ends_with(r"Quartz Systems\rustmodem")
    );
    assert_eq!(config::default_path(), data.join("rustmodem.toml"));
    assert_eq!(config::default_log_dir(), data.join("logs"));
}

#[test]
fn editing_the_settings_keeps_comments_and_everything_else() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    file.edit_settings(&SettingsEdit {
        log_level: Some(LogLevel::Debug),
        log_dir: Some(r"D:\logs".to_string()),
        pipe: Some("other-pipe".to_string()),
        ..SettingsEdit::default()
    })
    .unwrap();
    assert_eq!(file.config().service.log_level, LogLevel::Debug);
    assert_eq!(file.config().service.log_dir, r"D:\logs");
    assert_eq!(file.config().control.pipe, "other-pipe");
    let text = file.to_toml();
    assert!(
        text.contains("log_level = \"debug\"            # error, warning, info, debug"),
        "{text}"
    );
    assert!(text.contains("# empty = %ProgramData%"), "{text}");
    assert!(
        text.contains("pipe = \"other-pipe\"    # \\\\.\\pipe\\<name>"),
        "{text}"
    );
    // Nothing else moved: putting the old values back gives the old file.
    file.edit_settings(&SettingsEdit {
        log_level: Some(LogLevel::Info),
        log_dir: Some(String::new()),
        pipe: Some("rustmodem-control".to_string()),
        ..SettingsEdit::default()
    })
    .unwrap();
    assert_eq!(file.to_toml(), EXAMPLE);

    // Only the keys named change.
    file.edit_settings(&SettingsEdit {
        log_level: Some(LogLevel::Error),
        ..SettingsEdit::default()
    })
    .unwrap();
    assert_eq!(file.config().service.log_level, LogLevel::Error);
    assert_eq!(file.config().control.pipe, "rustmodem-control");
    assert_eq!(file.config().lines.len(), 2);

    // An edit that names nothing changes nothing.
    let before = file.to_toml();
    file.edit_settings(&SettingsEdit::default()).unwrap();
    assert_eq!(file.to_toml(), before);
}

#[test]
fn editing_the_settings_adds_the_tables_a_file_does_not_have() {
    let mut file = ConfigFile::parse(
        "# only a line\n[[line]]\nid = 1\nserial = { backend = \"com0com\", port = \"P1\" }\n",
    )
    .unwrap();
    file.edit_settings(&SettingsEdit {
        log_level: Some(LogLevel::Warning),
        log_dir: None,
        pipe: Some("p".to_string()),
        ..SettingsEdit::default()
    })
    .unwrap();
    let text = file.to_toml();
    assert!(text.starts_with("# only a line\n"), "{text}");
    let again = ConfigFile::parse(&text).unwrap();
    assert_eq!(again.config().service.log_level, LogLevel::Warning);
    assert_eq!(again.config().service.log_dir, "");
    assert_eq!(again.config().control.pipe, "p");
    assert_eq!(again.config().lines.len(), 1);
}

#[test]
fn editing_the_settings_refuses_an_invalid_result_and_changes_nothing() {
    let mut file = ConfigFile::parse(EXAMPLE).unwrap();
    let problems = file
        .edit_settings(&SettingsEdit {
            log_level: Some(LogLevel::Debug),
            log_dir: None,
            pipe: Some("not a pipe name".to_string()),
            ..SettingsEdit::default()
        })
        .unwrap_err();
    assert_eq!(ids(&problems), ["RMD0014"]);
    assert!(problems[0].text.contains("control.pipe"));
    assert_eq!(file.to_toml(), EXAMPLE);
    assert_eq!(file.config().service.log_level, LogLevel::Info);
}
