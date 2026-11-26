// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Stored profiles through the pool: `AT&W` reaches the store, a line
//! starts from what was stored, and a line that is removed is forgotten.

mod common;

use common::{Harness, SharedProfiles, config_with_lines};
use engine::NullConfig;
use pool::{MemoryProfiles, ProfileStore, TICK_MS};

const RING: &str = "\r\nRING\r\n";

#[test]
fn a_stored_profile_outlasts_the_pool() {
    let mut h = Harness::new(2);
    assert_eq!(
        h.command(1, "ATE0S0=2S7=25&W"),
        "ATE0S0=2S7=25&W\r\r\nOK\r\n"
    );
    let stored = h.profiles.stored.lock().unwrap().clone();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[&1].register(0), Some(2));
    assert_eq!(stored[&1].register(7), Some(25));
    assert!(!stored[&1].echo);
    let logs = h.take_logs();
    assert!(
        logs.iter()
            .any(|log| log.source == "Line01" && log.text == "RMD1014: Line 1 stored its profile."),
        "{logs:?}"
    );

    // The service stops and starts again: a new pool with the same store.
    let profiles = h.profiles.clone();
    drop(h);
    let mut config = config_with_lines(2);
    // The configured S0 gives way to the stored one, and stands for the
    // line that has nothing stored.
    config.lines[0].auto_answer_rings = 9;
    config.lines[1].auto_answer_rings = 4;
    let mut h = Harness::with_profiles(config, NullConfig::default(), profiles);
    h.host(1).open();
    h.host(2).open();
    h.advance(TICK_MS);
    // Echo is off on line 1, as stored, and S0 and S7 are what was stored.
    assert_eq!(h.command(1, "ATS0?S7?"), "\r\n002\r\n\r\n025\r\n\r\nOK\r\n");
    assert_eq!(h.command(2, "ATS0?"), "ATS0?\r\r\n004\r\n\r\nOK\r\n");
}

#[test]
fn the_operator_reset_returns_a_line_to_its_stored_profile() {
    let mut h = Harness::new(1);
    h.command(1, "ATE0S0=3&W");
    h.command(1, "ATS0=0E1");
    assert_eq!(h.command(1, "ATS0?"), "ATS0?\r\r\n000\r\n\r\nOK\r\n");
    h.pool.reset(1).unwrap();
    assert_eq!(h.command(1, "ATS0?"), "\r\n003\r\n\r\nOK\r\n");
    // &F is the way back to the factory settings.
    assert_eq!(h.command(1, "AT&FS0?"), "\r\n000\r\n\r\nOK\r\n");
}

#[test]
fn a_line_that_auto_answers_from_its_stored_profile_needs_no_command() {
    let mut h = Harness::new(2);
    h.command(2, "ATE0S0=1&W");
    let profiles = h.profiles.clone();
    drop(h);
    let mut h = Harness::with_profiles(config_with_lines(2), NullConfig::default(), profiles);
    h.host(1).open();
    h.host(2).open();
    h.advance(TICK_MS);
    h.quiet(1);
    assert_eq!(h.command(1, "ATD2"), "\r\nCONNECT\r\n");
    assert_eq!(h.read(2), format!("{RING}\r\nCONNECT\r\n"));
    assert_eq!(h.pool.stats().calls_answered, 1);
}

#[test]
fn a_profile_that_cannot_be_kept_is_logged_and_still_in_effect() {
    let mut h = Harness::new(1);
    *h.profiles.fail.lock().unwrap() = true;
    h.take_logs();
    // The host is told OK: the modem has the profile, whatever becomes of
    // the file.
    assert_eq!(h.command(1, "ATE0S0=6&W"), "ATE0S0=6&W\r\r\nOK\r\n");
    let logs = h.take_logs();
    assert!(
        logs.iter()
            .any(|log| log.text.starts_with("RMD0025: Stored profiles file")),
        "{logs:?}"
    );
    assert!(!logs.iter().any(|log| log.text.starts_with("RMD1014")));
    assert!(h.profiles.stored.lock().unwrap().is_empty());
    h.command(1, "ATS0=0");
    assert_eq!(h.command(1, "ATZ"), "\r\nOK\r\n");
    assert_eq!(h.command(1, "ATS0?"), "\r\n006\r\n\r\nOK\r\n");
}

#[test]
fn a_line_that_is_removed_is_forgotten() {
    let mut h = Harness::new(2);
    h.command(1, "ATS0=2&W");
    h.command(2, "ATS0=3&W");
    assert_eq!(h.profiles.stored.lock().unwrap().len(), 2);

    h.pool.apply_config(&config_with_lines(1));
    let stored = h.profiles.stored.lock().unwrap().clone();
    assert_eq!(stored.keys().copied().collect::<Vec<_>>(), [1]);

    // A line added later under the same id is another line.
    h.pool.apply_config(&config_with_lines(2));
    h.host(2).open();
    h.advance(TICK_MS);
    assert_eq!(h.command(2, "ATS0?"), "ATS0?\r\r\n000\r\n\r\nOK\r\n");

    // A line that is disabled, or moved to another port, keeps its profile.
    let mut config = config_with_lines(2);
    config.lines[0].enabled = false;
    h.pool.apply_config(&config);
    config.lines[0].enabled = true;
    config.lines[0].serial.port = "P7".to_string();
    h.pool.apply_config(&config);
    h.advance(TICK_MS);
    h.host(1).open();
    h.advance(TICK_MS);
    assert_eq!(h.command(1, "ATS0?"), "ATS0?\r\r\n002\r\n\r\nOK\r\n");
}

#[test]
fn profiles_in_memory_are_kept_stored_and_removed() {
    let mut profiles = MemoryProfiles::new();
    assert!(profiles.load(1).is_none());
    let profile = pool::Profile::factory(7);
    profiles.store(1, &profile).unwrap();
    assert_eq!(profiles.load(1), Some(profile));
    assert!(profiles.load(2).is_none());
    profiles.remove(1).unwrap();
    profiles.remove(1).unwrap();
    assert!(profiles.load(1).is_none());
    let _ = SharedProfiles::default();
}
