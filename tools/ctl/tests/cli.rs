// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `rustmodemctl.exe` as a program: its arguments and its exit codes.
//!
//! The commands themselves are tested against the control server inside the
//! crate. The service's pipe is open to Administrators only, so the program
//! as built can be run here only up to the point where it connects.

use std::process::{Command, Output};

fn ctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustmodemctl"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn help_and_version() {
    for args in [&["--help"][..], &[], &["lines", "--help"]] {
        let output = ctl(args);
        assert!(output.status.success(), "{args:?}");
        let usage = text(&output.stdout);
        for word in [
            "info",
            "lines",
            "answer|hangup|reset|enable|disable",
            "calls",
            "watch",
        ] {
            assert!(usage.contains(word), "{usage}");
        }
        assert!(usage.contains("--pipe") && usage.contains("--json"));
        assert!(usage.contains("--verbose"));
    }
    let version = ctl(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        text(&version.stdout).trim(),
        format!("rustmodemctl {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_command_line_that_is_not_understood_exits_with_3() {
    for args in [&["frobnicate"][..], &["line"], &["line", "x"], &["--pipe"]] {
        let output = ctl(args);
        assert_eq!(output.status.code(), Some(3), "{args:?}");
        assert!(text(&output.stderr).starts_with("RMD0009: "), "{args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[cfg(windows)]
#[test]
fn a_service_that_is_not_there_exits_with_2() {
    let pipe = format!("rustmodem-absent-{}", std::process::id());
    let output = ctl(&["--pipe", &pipe, "lines"]);
    assert_eq!(output.status.code(), Some(2));
    let errors = text(&output.stderr);
    assert!(
        errors.starts_with(&format!(
            "RMD2008: Control pipe {pipe} could not be opened:"
        )),
        "{errors}"
    );
    assert_eq!(errors.lines().count(), 1);
    assert!(output.stdout.is_empty());

    let output = ctl(&["--pipe", &pipe, "--verbose", "watch"]);
    assert_eq!(output.status.code(), Some(2));
    let errors = text(&output.stderr);
    assert!(errors.contains("\nCause: "), "{errors}");
    assert!(errors.contains("\nRecovery: "), "{errors}");
    assert!(errors.contains("elevated prompt"), "{errors}");
}

/// The resources `build.rs` links in: looked for in the program file, where
/// the version strings are UTF-16 and the manifest is UTF-8.
#[cfg(windows)]
#[test]
fn the_program_carries_its_version_information_icon_and_manifest() {
    let program = std::fs::read(env!("CARGO_BIN_EXE_rustmodemctl")).unwrap();
    let has = |needle: &[u8]| program.windows(needle.len()).any(|window| window == needle);
    let utf16 =
        |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let file_version = format!("{}.0", env!("CARGO_PKG_VERSION"));
    for wanted in [
        "VS_VERSION_INFO",
        "Quartz Systems",
        "rustmodem command-line client",
        "rustmodemctl.exe",
        "Copyright (C) 2026 Quartz Systems",
        file_version.as_str(),
    ] {
        assert!(
            has(&utf16(wanted)),
            "{wanted} is not in the version information"
        );
    }
    for wanted in [
        "name=\"QuartzSystems.rustmodem.Ctl\"",
        "level=\"asInvoker\"",
        "{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}",
        "longPathAware",
    ] {
        assert!(has(wanted.as_bytes()), "{wanted} is not in the manifest");
    }
    // A program with no windows asks for no common controls.
    assert!(!has(b"Microsoft.Windows.Common-Controls"));
    // The largest image of the icon is a PNG.
    assert!(has(b"\x89PNG\r\n\x1a\n"));
}
