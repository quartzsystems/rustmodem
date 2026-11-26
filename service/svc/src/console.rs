// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Console mode: the service in the foreground, logging to standard
//! output, stopped by Ctrl+C.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use msg::{Message, message};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::core::BOOL;

use crate::runner::{self, RunMode};
use pipe::Event;

/// Set by the console control handler to ask the service to stop.
static STOP: OnceLock<Arc<Event>> = OnceLock::new();

/// Set by the main thread when the service has stopped.
static DONE: OnceLock<Arc<Event>> = OnceLock::new();

/// How long the handler waits for the service to stop. Windows ends the
/// process when the handler for a console close returns, so the handler
/// must not return before the ports are released.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// Called by Windows on a thread of its own for Ctrl+C, Ctrl+Break, and the
/// console closing.
unsafe extern "system" fn on_console_control(_kind: u32) -> BOOL {
    if let Some(stop) = STOP.get() {
        stop.set();
    }
    if let Some(done) = DONE.get() {
        done.wait(STOP_WAIT);
    }
    // Handled: Windows should not also end the process itself.
    true.into()
}

/// Runs the service in the foreground until Ctrl+C.
pub fn run(config: Option<PathBuf>) -> Result<(), Vec<Message>> {
    let event = || {
        Event::new()
            .map(Arc::new)
            .map_err(|e| vec![message("RMD0023", &[&e.to_string()])])
    };
    let stop = event()?;
    let done = event()?;
    // These are set once per process; console mode runs once.
    let _ = STOP.set(Arc::clone(&stop));
    let _ = DONE.set(Arc::clone(&done));
    // SAFETY: the handler is a function that lives for the whole program
    // and touches only the two events above, which are never freed.
    unsafe { SetConsoleCtrlHandler(Some(on_console_control), true) }
        .map_err(|e| vec![message("RMD0023", &[&e.message()])])?;

    let outcome = runner::start(config, RunMode::Console, Arc::clone(&stop)).map(|running| {
        stop.wait_forever();
        running.stop();
    });
    done.set();
    outcome
}
