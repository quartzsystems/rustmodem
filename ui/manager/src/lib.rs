// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The rustmodem Manager: the operator's window.
//!
//! One window that connects to the service over the control pipe, shows
//! every line live, and carries the operator's actions. It holds no state
//! of its own: what it shows is what the service says, kept in the
//! [`model`] crate's `Model`, and what it remembers between runs is how
//! its window was arranged.
//!
//! - [`args`] is the command line.
//! - [`requests`] matches the service's answers to what was asked, and
//!   gives up on a request that gets none.
//! - `link` is the threads that own the pipe.
//! - `view` is the controls of the main window, and `window` is what
//!   happens in it.
//! - `dialogs` is the dialogs, and `actions` is what the manager does
//!   outside the control API: exports, and the Windows service.
//!
//! Everything from Win32 comes through the safe wrappers of the `win32`
//! crate; there is no `unsafe` here.

#![forbid(unsafe_code)]

pub mod args;
pub mod requests;

#[cfg(windows)]
mod actions;
#[cfg(windows)]
mod clock;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod link;
#[cfg(windows)]
mod message;
#[cfg(windows)]
mod settings;
#[cfg(windows)]
mod view;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use clock::Clock;
#[cfg(windows)]
pub use dialogs::line::ids as line_ids;
#[cfg(windows)]
pub use dialogs::sheet::ids as settings_ids;
#[cfg(windows)]
pub use message::{denied_dialog, dialog as message_dialog};
#[cfg(windows)]
pub use view::ids;
#[cfg(windows)]
pub use window::{
    DialogHook, FileHook, Hooks, Manager, Opened, Options, PortsHook, TaskHook, TaskWatchHook,
    has_handler, open,
};

/// Runs the manager: reads the command line, opens the window, and stays
/// until the window is closed. Returns the exit code of the program.
#[cfg(windows)]
pub fn main() -> i32 {
    let app = win32::App::new();
    let args = match args::Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(problem) => {
            message::show(None, &ctlproto::ErrorInfo::from(problem));
            return 2;
        }
    };
    let Some(opened) = open(app, Options::from_args(&args)) else {
        return 1;
    };
    app.run(opened.window, Some(&opened.accelerators))
}
