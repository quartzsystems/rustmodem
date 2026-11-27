// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Stored profiles as the service handles them: taken from an interpreter,
//! given to another, and checked on the way in.

use at::{Action, AtConfig, AtInterpreter, Input, Output, Profile};

/// Runs a command line and returns the profile it asked to have stored.
fn stored_by(interpreter: &mut AtInterpreter, line: &str) -> Option<Profile> {
    let mut out = Vec::new();
    interpreter.step(Input::DteBytes(format!("{line}\r").as_bytes()), &mut out);
    interpreter.check_invariants().unwrap();
    out.into_iter().find_map(|output| match output {
        Output::Action(Action::StoreProfile(profile)) => Some(profile),
        _ => None,
    })
}

#[test]
fn a_profile_survives_from_one_interpreter_to_the_next() {
    let config = AtConfig {
        auto_answer_rings: 4,
        ..AtConfig::default()
    };
    let mut first = AtInterpreter::new(config);
    assert_eq!(first.stored_profile(), None);
    assert_eq!(stored_by(&mut first, "AT"), None);
    let profile = stored_by(&mut first, "ATE0V0X2L3M2&C0&D1&K0S0=2S10=30+MS=B103,0;&W").unwrap();
    assert_eq!(first.stored_profile(), Some(&profile));
    assert!(!profile.echo && !profile.verbose && !profile.dcd_follows_carrier);
    assert_eq!((profile.x, profile.volume, profile.speaker), (2, 3, 2));
    assert_eq!((profile.dtr_action, profile.flow), (1, 0));
    assert_eq!(profile.ms, "B103,0,0,0,0,0");
    assert_eq!(profile.register(0), Some(2));
    assert_eq!(profile.register(10), Some(30));
    // S0 to S7, S9, S10 and S12, and S36, S46 and S48.
    assert_eq!(profile.registers().count(), 14);
    assert_eq!(profile.register(9), Some(6));
    assert_eq!(profile.check(), Ok(()));

    // What a service restart does: a new interpreter, given the profile.
    let second = AtInterpreter::with_profile(config, Some(profile.clone()));
    second.check_invariants().unwrap();
    assert_eq!(second.stored_profile(), Some(&profile));
    assert_eq!(second.register(0), Some(2), "the stored S0 wins");
    assert_eq!(second.register(10), Some(30));

    // With nothing stored, the configured S0 is the starting one.
    assert_eq!(
        AtInterpreter::with_profile(config, None).register(0),
        Some(4)
    );
    assert_eq!(Profile::factory(4).register(0), Some(4));
}

#[test]
fn the_operator_reset_returns_to_the_stored_profile() {
    let mut interpreter = AtInterpreter::new(AtConfig::default());
    stored_by(&mut interpreter, "ATS0=3S7=25&W").unwrap();
    stored_by(&mut interpreter, "ATS0=0S7=60");
    assert_eq!(interpreter.register(7), Some(60));
    let mut out = Vec::new();
    interpreter.operator_reset(&mut out);
    interpreter.check_invariants().unwrap();
    assert_eq!(interpreter.register(0), Some(3));
    assert_eq!(interpreter.register(7), Some(25));
}

#[test]
fn a_profile_with_a_value_no_command_could_set_is_refused() {
    let good = Profile::factory(0);
    assert_eq!(good.check(), Ok(()));
    type Spoil = fn(&mut Profile);
    let cases: [(&str, Spoil); 11] = [
        ("X", |p| p.x = 5),
        ("L", |p| p.volume = 4),
        ("M", |p| p.speaker = 4),
        ("&D", |p| p.dtr_action = 4),
        ("&K", |p| p.flow = 1),
        ("S3", |p| {
            p.set_register(3, 128);
        }),
        ("S4", |p| {
            p.set_register(4, 200);
        }),
        ("S5", |p| {
            p.set_register(5, 255);
        }),
        ("+MS", |p| p.ms = "V21\r\nOK".to_string()),
        ("+ES", |p| p.es = "x".repeat(65)),
        ("+DS", |p| p.ds = "3;0".to_string()),
    ];
    for (name, spoil) in cases {
        let mut profile = good.clone();
        spoil(&mut profile);
        assert_eq!(profile.check(), Err(name));
        // An interpreter given it starts from its factory settings and has
        // nothing stored, and what must always hold still holds.
        let interpreter = AtInterpreter::with_profile(AtConfig::default(), Some(profile));
        interpreter.check_invariants().unwrap();
        assert_eq!(interpreter.stored_profile(), None, "{name}");
    }
    // There is no register 8 to set.
    assert!(!good.clone().set_register(8, 1));
    assert_eq!(good.register(8), None);
}

#[test]
fn a_ring_count_given_in_a_stored_profile_is_dropped() {
    let mut profile = Profile::factory(0);
    assert!(profile.set_register(1, 9));
    let interpreter = AtInterpreter::with_profile(AtConfig::default(), Some(profile));
    interpreter.check_invariants().unwrap();
    assert_eq!(interpreter.register(1), Some(0));
    assert_eq!(interpreter.mode(), at::Mode::Command);
}
