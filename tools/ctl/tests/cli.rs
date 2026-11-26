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
