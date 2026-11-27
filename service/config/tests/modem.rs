// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The `[modem]` table, a line's `modem` table, and the `modem` engine.

use config::{Config, ConfigFile, EngineKind, LineConfig, LineModem, ModemConfig, ModulationName};
use proptest::prelude::*;

const LINE: &str = r#"
[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }
"#;

fn parse(text: &str) -> Config {
    ConfigFile::parse(text)
        .unwrap_or_else(|problems| panic!("{problems:?}"))
        .config()
        .clone()
}

fn problems(text: &str) -> Vec<String> {
    let Err(problems) = ConfigFile::parse(text) else {
        panic!("the file is to be refused: {text}");
    };
    problems
        .iter()
        .map(|message| format!("{}: {}", message.id, message.text))
        .collect()
}

#[test]
fn a_file_with_no_modem_table_has_the_defaults() {
    let config = parse(LINE);
    assert_eq!(
        config.modem,
        ModemConfig {
            modulations: vec![ModulationName::V21, ModulationName::Bell103],
            v8: true,
            tx_level_dbm0: -12,
            answer_delay_ms: 0,
            fallback_ms: 3000,
        }
    );
    // A line that names no engine has the modem, and no settings of its
    // own.
    assert_eq!(config.lines[0].engine, EngineKind::Modem);
    assert_eq!(config.lines[0].modem, LineModem::default());
    assert!(config.lines[0].modem.is_empty());
    assert_eq!(config.modem.for_line(&config.lines[0].modem), config.modem);
    // And written out again, the file says nothing of either.
    let text = ConfigFile::from_config(&config).unwrap().to_toml();
    assert!(
        !text.contains("[modem]") && !text.contains("modem =") && !text.contains("engine"),
        "{text}"
    );
}

#[test]
fn the_modem_table_is_read() {
    let text = format!(
        r#"
[modem]
modulations = ["bell103", "v21"]
v8 = false
tx_level_dbm0 = -15
answer_delay_ms = 1800
fallback_ms = 2500
{LINE}"#
    );
    let config = parse(&text);
    assert_eq!(
        config.modem,
        ModemConfig {
            modulations: vec![ModulationName::Bell103, ModulationName::V21],
            v8: false,
            tx_level_dbm0: -15,
            answer_delay_ms: 1800,
            fallback_ms: 2500,
        }
    );
}

#[test]
fn a_line_has_its_own_settings_over_the_tables() {
    let text = r#"
[modem]
modulations = ["v21"]
tx_level_dbm0 = -10
answer_delay_ms = 2000

[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }
modem = { modulations = ["bell103"], v8 = false }

[[line]]
id = 2
serial = { backend = "com0com", port = "CNCB1" }
engine = "modem"
modem = { tx_level_dbm0 = -20 }

[[line]]
id = 3
serial = { backend = "com0com", port = "CNCB2" }
engine = "null"
"#;
    let config = parse(text);
    let of = |index: usize| config.modem.for_line(&config.lines[index].modem);
    assert_eq!(
        of(0),
        ModemConfig {
            modulations: vec![ModulationName::Bell103],
            v8: false,
            tx_level_dbm0: -10,
            answer_delay_ms: 2000,
            fallback_ms: 3000,
        }
    );
    assert_eq!(of(1).modulations, [ModulationName::V21]);
    assert!(of(1).v8);
    assert_eq!(of(1).tx_level_dbm0, -20);
    assert_eq!(of(2), config.modem);
    assert_eq!(
        config.lines.iter().map(|l| l.engine).collect::<Vec<_>>(),
        [EngineKind::Modem, EngineKind::Modem, EngineKind::Null]
    );
    // It survives being written and read.
    let text = ConfigFile::from_config(&config).unwrap().to_toml();
    assert_eq!(parse(&text), config);
    // The default engine is not written, and the others are.
    assert_eq!(text.matches("engine").count(), 1, "{text}");
}

#[test]
fn what_is_wrong_is_reported_with_its_key_and_line() {
    let cases = [
        (
            "[modem]\nmodulations = [\"v22\"]",
            "modem.modulations",
            "v22",
        ),
        (
            "[modem]\nmodulations = [\"v21\", \"v21\"]",
            "modem.modulations",
            "v21",
        ),
        ("[modem]\nmodulations = []", "modem.modulations", "[]"),
        ("[modem]\nmodulations = \"v21\"", "modem.modulations", ""),
        ("[modem]\nv8 = 1", "modem.v8", ""),
        ("[modem]\ntx_level_dbm0 = -8", "modem.tx_level_dbm0", "-8"),
        ("[modem]\ntx_level_dbm0 = -21", "modem.tx_level_dbm0", "-21"),
        (
            "[modem]\nanswer_delay_ms = 10001",
            "modem.answer_delay_ms",
            "10001",
        ),
        ("[modem]\nfallback_ms = 100", "modem.fallback_ms", "100"),
    ];
    for (table, key, value) in cases {
        let found = problems(&format!("{table}\n{LINE}"));
        assert_eq!(found.len(), 1, "{table}: {found:?}");
        assert!(found[0].contains(key), "{table}: {found:?}");
        assert!(found[0].contains(value), "{table}: {found:?}");
        // On line 2 of the file, where the key is.
        assert!(found[0].contains("line 2"), "{table}: {found:?}");
    }
    let line = |modem: &str| {
        format!("[[line]]\nid = 1\nserial = {{ backend = \"com0com\", port = \"A\" }}\n{modem}\n")
    };
    for (modem, key) in [
        (
            "modem = { modulations = [\"x\"] }",
            "line.modem.modulations",
        ),
        ("modem = { v8 = \"yes\" }", "line.modem.v8"),
        ("modem = { tx_level_dbm0 = 0 }", "line.modem.tx_level_dbm0"),
        ("modem = 3", "line.modem"),
        ("engine = \"v34\"", "line.engine"),
    ] {
        let found = problems(&line(modem));
        assert_eq!(found.len(), 1, "{modem}: {found:?}");
        assert!(found[0].contains(key), "{modem}: {found:?}");
    }
    // Everything wrong is reported at once.
    let found = problems(&format!(
        "[modem]\nv8 = 2\ntx_level_dbm0 = 5\nfallback_ms = 1\n{LINE}"
    ));
    assert_eq!(found.len(), 3, "{found:?}");
}

#[test]
fn the_names_are_what_the_file_spells() {
    assert_eq!(EngineKind::default(), EngineKind::Modem);
    assert_eq!(
        EngineKind::ALL.map(EngineKind::as_str),
        ["modem", "null", "echo"]
    );
    for engine in EngineKind::ALL {
        assert_eq!(EngineKind::parse(engine.as_str()), Some(engine));
    }
    assert_eq!(
        ModulationName::ALL.map(ModulationName::as_str),
        ["v21", "bell103"]
    );
    for modulation in ModulationName::ALL {
        assert_eq!(ModulationName::parse(modulation.as_str()), Some(modulation));
        assert_eq!(modulation.to_string(), modulation.as_str());
    }
    assert_eq!(ModulationName::parse("V21"), None);
}

fn modulations() -> impl Strategy<Value = Vec<ModulationName>> {
    prop_oneof![
        Just(vec![ModulationName::V21]),
        Just(vec![ModulationName::Bell103]),
        Just(vec![ModulationName::V21, ModulationName::Bell103]),
        Just(vec![ModulationName::Bell103, ModulationName::V21]),
    ]
}

fn line_modem() -> impl Strategy<Value = LineModem> {
    (
        proptest::option::of(modulations()),
        proptest::option::of(any::<bool>()),
        proptest::option::of(-20i8..=-9),
    )
        .prop_map(|(modulations, v8, tx_level_dbm0)| LineModem {
            modulations,
            v8,
            tx_level_dbm0,
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_modem_setting_survives_the_file(
        modulations in modulations(),
        v8 in any::<bool>(),
        tx_level_dbm0 in -20i8..=-9,
        answer_delay_ms in 0u32..=10_000,
        fallback_ms in 500u32..=20_000,
        lines in proptest::collection::vec((line_modem(), 0usize..3), 1..5),
    ) {
        let mut config = Config {
            modem: ModemConfig { modulations, v8, tx_level_dbm0, answer_delay_ms, fallback_ms },
            ..Config::default()
        };
        for (n, (modem, engine)) in lines.into_iter().enumerate() {
            let mut line = LineConfig::new(n as u16 + 1, &format!("PORT{n}"));
            line.modem = modem;
            line.engine = EngineKind::ALL[engine];
            config.lines.push(line);
        }
        let text = ConfigFile::from_config(&config).unwrap().to_toml();
        let back = ConfigFile::parse(&text).unwrap();
        prop_assert_eq!(back.config(), &config);
        prop_assert!(back.warnings().is_empty());
    }
}
