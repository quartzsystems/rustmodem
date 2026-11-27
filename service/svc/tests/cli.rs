// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `rustmodem-svc.exe` as a program: its arguments, its exit codes, how it
//! reports a configuration it cannot use, and console mode from start to a
//! clean stop.

#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};

/// `CREATE_NEW_PROCESS_GROUP`: the child gets a process group of its own,
/// so that a console control event can be sent to it alone.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// A directory for one test, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rustmodem-svc-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// The service's command, with `%ProgramData%` pointed into the
    /// scratch directory so that nothing is written outside it.
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rustmodem-svc"));
        command.env("ProgramData", self.path("ProgramData"));
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Waits until `check` is true, or fails the test.
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn help_and_version() {
    let scratch = Scratch::new("help");
    let help = scratch.run(&["--help"]);
    assert!(help.status.success());
    let usage = text(&help.stdout);
    for word in ["--console", "--install", "--uninstall", "CONFIG"] {
        assert!(usage.contains(word), "{usage}");
    }
    let version = scratch.run(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        text(&version.stdout).trim(),
        format!("rustmodem-svc {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn an_argument_that_is_not_understood_exits_with_2() {
    let scratch = Scratch::new("usage");
    for args in [
        &["--frobnicate"][..],
        &["--console", "a", "b"],
        &["--uninstall", "x"],
    ] {
        let output = scratch.run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(text(&output.stderr).contains("RMD0009"), "{args:?}");
    }
}

#[test]
fn started_with_no_arguments_outside_the_service_manager_says_so() {
    let scratch = Scratch::new("noargs");
    let output = scratch.run(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("RMD0008"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_missing_configuration_is_reported_and_exits_with_1() {
    let scratch = Scratch::new("missing");
    let config = scratch.path("absent.toml");
    let output = scratch.run(&["--console", config.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let errors = text(&output.stderr);
    assert!(errors.contains("RMD0010"), "{errors}");
    assert!(errors.contains("absent.toml"), "{errors}");

    // It is in the log too: with no usable configuration, the log is in the
    // default log directory.
    let log = scratch.path(r"ProgramData\Quartz Systems\RustModem\logs\rustmodem.log");
    let logged = read(&log);
    assert!(logged.contains("ERROR [SVC] RMD0010"), "{logged}");
}

#[test]
fn the_default_configuration_path_is_under_program_data() {
    let scratch = Scratch::new("default");
    let output = scratch.run(&["--console"]);
    assert_eq!(output.status.code(), Some(1));
    let errors = text(&output.stderr);
    assert!(errors.contains("RMD0010"), "{errors}");
    assert!(
        errors.contains(r"ProgramData\Quartz Systems\RustModem\rustmodem.toml"),
        "{errors}"
    );
}

#[test]
fn every_problem_in_a_configuration_is_reported() {
    let scratch = Scratch::new("invalid");
    let config = scratch.path("rustmodem.toml");
    std::fs::write(
        &config,
        r#"[service]
log_level = "loud"

[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }

[[line]]
id = 1
serial = { backend = "usb", port = "CNCB0" }
"#,
    )
    .unwrap();
    let output = scratch.run(&["--console", config.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let errors = text(&output.stderr);
    for id in ["RMD0014", "RMD0016", "RMD0017", "RMD0018"] {
        assert!(errors.contains(id), "{id} missing from:\n{errors}");
    }
    assert!(errors.contains("at line 2"), "{errors}");
}

/// A valid configuration in the scratch directory: one line on a port that
/// does not exist, the log in the scratch directory at debug level, and a
/// pipe name of its own.
fn write_config(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let config = scratch.path("rustmodem.toml");
    let logs = scratch.path("logs");
    let pipe = format!(
        "rustmodem-cli-{}-{}",
        std::process::id(),
        scratch.0.file_name().unwrap().to_string_lossy()
    );
    std::fs::write(
        &config,
        format!(
            "[service]\nlog_level = \"debug\"\nlog_dir = '{}'\n\n[control]\npipe = \"{pipe}\"\n\n\
             [[line]]\nid = 1\nserial = {{ backend = \"com0com\", port = \"RUSTMODEM_NO_SUCH_PORT\" }}\n",
            logs.display()
        ),
    )
    .unwrap();
    (config, logs)
}

fn start_console(scratch: &Scratch, config: &Path) -> Child {
    scratch
        .command()
        .arg("--console")
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .unwrap()
}

#[test]
fn console_mode_starts_logs_and_stops_cleanly() {
    let scratch = Scratch::new("console");
    let (config, logs) = write_config(&scratch);
    let log = logs.join("rustmodem.log");
    let mut child = start_console(&scratch, &config);

    eventually("the service to start", || {
        read(&log).contains("RMD0001: RustModem service started")
    });
    eventually("the port failure to be logged", || {
        read(&log).contains("RMD3001")
    });
    let started = read(&log);
    assert!(
        started.contains("INFO  [SVC] RMD0021: Configuration loaded from"),
        "{started}"
    );
    assert!(
        started.contains("INFO  [Line01] RMD1010: Line 1 added."),
        "{started}"
    );
    assert!(
        started.contains("ERROR [Line01] RMD3001: Serial port RUSTMODEM_NO_SUCH_PORT"),
        "{started}"
    );
    assert!(started.contains("INFO  [Line01] RMD1005: Line 1 state is now not_ready."));
    assert!(logs.join("calls.jsonl").exists());
    assert!(
        child.try_wait().unwrap().is_none(),
        "the service is still running"
    );

    // Ctrl+Break, as from the keyboard. It can only be sent when this test
    // and the service share a console; where there is none, the service is
    // ended the hard way and the clean stop is left to the manual check.
    // SAFETY: the call takes two plain values, an event number and the id
    // of the process group made for the child.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) }.is_ok();
    if !sent {
        child.kill().unwrap();
        child.wait().unwrap();
        eprintln!("no console: the clean stop was not exercised");
        return;
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let finished = read(&log);
    assert!(
        finished
            .trim_end()
            .ends_with("INFO  [SVC] RMD0002: RustModem service stopped."),
        "{finished}"
    );
    // In console mode the log lines are on standard output as well.
    let printed = text(&output.stdout);
    assert!(
        printed.contains("RMD0001: RustModem service started"),
        "{printed}"
    );
    assert!(
        printed.contains("RMD0002: RustModem service stopped."),
        "{printed}"
    );
}

#[test]
fn a_second_copy_with_the_same_pipe_is_refused() {
    let scratch = Scratch::new("second");
    let (config, logs) = write_config(&scratch);
    let log = logs.join("rustmodem.log");
    let mut first = start_console(&scratch, &config);
    eventually("the first copy to start", || read(&log).contains("RMD0001"));

    let second = scratch.run(&["--console", config.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(1));
    assert!(
        text(&second.stderr).contains("RMD2007"),
        "{}",
        text(&second.stderr)
    );

    first.kill().unwrap();
    first.wait().unwrap();
}

/// The resources `build.rs` links in: looked for in the program file, where
/// the version strings are UTF-16 and the manifest is UTF-8.
#[cfg(windows)]
#[test]
fn the_program_carries_its_version_information_icon_and_manifest() {
    let program = std::fs::read(env!("CARGO_BIN_EXE_rustmodem-svc")).unwrap();
    let has = |needle: &[u8]| program.windows(needle.len()).any(|window| window == needle);
    let utf16 =
        |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let file_version = format!("{}.0", env!("CARGO_PKG_VERSION"));
    for wanted in [
        "VS_VERSION_INFO",
        "Quartz Systems",
        "RustModem modem pool service",
        "rustmodem-svc.exe",
        "Copyright (C) 2026 Quartz Systems",
        file_version.as_str(),
    ] {
        assert!(
            has(&utf16(wanted)),
            "{wanted} is not in the version information"
        );
    }
    for wanted in [
        "name=\"QuartzSystems.rustmodem.Service\"",
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
