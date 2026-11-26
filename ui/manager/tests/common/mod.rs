// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the tests of the manager's window share: the demo service as a
//! child process, and a manager opened against a pipe.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use manager::{Clock, Hooks, Opened, Options};
use win32::App;

/// The moment the frozen demo service stands still at, in Unix
/// milliseconds: 24 minutes 57.170 seconds after 2027-01-15 08:00:00 UTC.
pub const FROZEN_NOW_MS: u64 = 1_800_001_497_170;

/// The size the tests open the window at, at 96 DPI.
pub const SIZE: (i32, i32) = (1180, 740);

/// A name for a pipe that no other test and no other run uses.
pub fn pipe_name(test: &str) -> String {
    format!("rustmodem-test-{test}-{}", std::process::id())
}

/// The name of the demo service's program.
const DEMO_SERVICE: &str = "rustmodem-demosvc.exe";

/// Where Cargo put the demo service: in the directory of the profile this
/// test program was built with, which is one of the directories above the
/// test program. Which one depends on the version of Cargo.
fn demo_service_path() -> Option<PathBuf> {
    let test = std::env::current_exe().expect("the test program has a path");
    test.ancestors()
        .skip(1)
        .map(|directory| directory.join(DEMO_SERVICE))
        .find(|program| program.exists())
}

/// The demo service, built if this run of Cargo did not build it: testing
/// this package alone does not build the programs of another.
fn demo_service_program() -> PathBuf {
    if let Some(path) = demo_service_path() {
        return path;
    }
    let test = std::env::current_exe().expect("the test program has a path");
    let mut build = Command::new(env!("CARGO"));
    build.args(["build", "-p", "demosvc"]);
    if test.components().any(|part| part.as_os_str() == "release") {
        build.arg("--release");
    }
    let status = build.status().expect("cargo can be run");
    assert!(status.success(), "the demo service could not be built");
    demo_service_path().unwrap_or_else(|| panic!("{DEMO_SERVICE} is not above {}", test.display()))
}

/// The demo service as a child process. It is ended when this is dropped.
pub struct DemoService {
    child: Child,
}

impl DemoService {
    /// Starts `rustmodem-demosvc --script <script> --lines <lines>` on a
    /// pipe.
    pub fn start(pipe: &str, script: &str, lines: u32) -> DemoService {
        let child = Command::new(demo_service_program())
            .args(["--script", script, "--lines", &lines.to_string()])
            .args(["--pipe", pipe])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the demo service starts");
        DemoService { child }
    }

    /// Ends the service at once, as a crash would.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DemoService {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A manager with its window open, and the loop that runs it.
pub struct Bench {
    pub app: App,
    pub opened: Opened,
}

impl Bench {
    /// Opens the manager against a pipe, with its clock stopped at the
    /// frozen demo service's moment, nothing remembered from other runs,
    /// and nothing written for later ones.
    pub fn open(pipe: &str) -> Bench {
        Bench::open_with(pipe, Hooks::default())
    }

    /// As [`Bench::open`], with something in place of the dialogs that
    /// would wait for a person.
    pub fn open_with(pipe: &str, hooks: Hooks) -> Bench {
        let app = App::new();
        let opened = manager::open(
            app,
            Options {
                pipe: pipe.to_string(),
                action: None,
                clock: Clock::fixed(FROZEN_NOW_MS),
                settings_key: None,
                size: Some(SIZE),
                machine: "TESTBENCH".to_string(),
                hooks,
            },
        )
        .expect("the window opens");
        Bench { app, opened }
    }

    /// Handles the messages that are waiting.
    pub fn pump(&self) {
        self.app
            .pump(self.opened.window, Some(&self.opened.accelerators));
    }

    /// Runs the window for a while.
    pub fn run_for(&self, time: Duration) {
        let until = Instant::now() + time;
        while Instant::now() < until {
            self.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Runs the window until something is so, or `limit` has passed.
    /// Returns whether it came to be so.
    pub fn run_until(&self, limit: Duration, so: impl Fn(&Bench) -> bool) -> bool {
        let until = Instant::now() + limit;
        loop {
            self.pump();
            if so(self) {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Chooses a command as its menu item would, and handles it.
    pub fn choose(&self, command: model::Command) {
        self.opened.window.command(command.id());
        self.pump();
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.opened.window.destroy();
        self.pump();
    }
}
