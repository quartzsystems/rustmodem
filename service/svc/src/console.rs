// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Console mode: the service in the foreground, logging to standard
//! output, stopped by Ctrl+C.

use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use msg::{Message, message};
use svc::shutdown::{self, Shutdown};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::core::BOOL;

use crate::runner::{self, RunMode};

/// What the console control handler asks the service to stop with.
static STOP: OnceLock<Shutdown> = OnceLock::new();

/// Set by the main thread when the service has stopped.
static DONE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// How long the handler waits for the service to stop. Windows ends the
/// process when the handler for a console close returns, so the handler
/// must not return before the ports are released.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// Called by Windows on a thread of its own for Ctrl+C, Ctrl+Break, and the
/// console closing.
unsafe extern "system" fn on_console_control(_kind: u32) -> BOOL {
    if let Some(stop) = STOP.get() {
        stop.trigger();
    }
    let (done, changed) = &DONE;
    let guard = done.lock().unwrap_or_else(|e| e.into_inner());
    let _ = changed.wait_timeout_while(guard, STOP_WAIT, |done| !*done);
    // Handled: Windows should not also end the process itself.
    true.into()
}

/// Runs the service in the foreground until Ctrl+C.
pub fn run(config: Option<PathBuf>) -> Result<(), Vec<Message>> {
    let runtime = svc::runtime::build().map_err(|e| vec![message("RMD0003", &[&e.to_string()])])?;
    let (shutdown, mut stopping) = shutdown::channel();
    // This is set once per process; console mode runs once.
    let _ = STOP.set(shutdown);
    // SAFETY: the handler is a function that lives for the whole program
    // and touches only the two statics above, which are never freed.
    unsafe { SetConsoleCtrlHandler(Some(on_console_control), true) }
        .map_err(|e| vec![message("RMD0023", &[&e.message()])])?;

    let outcome = runner::start(config, RunMode::Console).map(|running| {
        runtime.block_on(stopping.wait());
        running.stop();
    });
    drop(runtime);
    let (done, changed) = &DONE;
    *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
    changed.notify_all();
    outcome
}
