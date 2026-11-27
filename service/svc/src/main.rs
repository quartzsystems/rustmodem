// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The rustmodem Windows service.
//!
//! `rustmodem-svc.exe` owns the lines. It runs as the Windows service
//! `rustmodem`, or in the foreground with `--console`. Either way it reads
//! the configuration, builds the Tokio runtime, runs the pool on a task of
//! it with a thread for each line's serial port, and serves the control
//! API on a named pipe.
//!
//! This program is where the operating system is: the service control
//! manager, the event log, the console and the registry. The runner it
//! starts, which the demo service starts too, is the library of this
//! crate; `docs/RUNTIME.md` says what runs where. The named pipe is in the
//! `pipe` crate, which the demo service and the manager share. This is one
//! of the crates that may use `unsafe`; every use is a Win32 call and says
//! why it is sound, and the library has none.

#![deny(unsafe_code)]

mod args;
mod logger;
mod ports;
mod profiles;

#[cfg(windows)]
#[allow(unsafe_code)]
mod console;
#[cfg(windows)]
#[allow(unsafe_code)]
mod cpu;
#[cfg(windows)]
#[allow(unsafe_code)]
mod eventlog;
#[cfg(windows)]
#[allow(unsafe_code)]
mod registry;
#[cfg(windows)]
mod runner;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod telephony;

use std::process::ExitCode;

use args::Mode;

/// The name of the Windows service.
pub(crate) const SERVICE_NAME: &str = "rustmodem";

/// The display name of the Windows service.
pub(crate) const SERVICE_DISPLAY_NAME: &str = "RustModem Modem Pool";

/// The version of the service.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The exit code for a command line that was not understood.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let mode = match args::parse(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(problem) => {
            eprintln!("{problem}");
            eprintln!("{}", problem.recovery);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match mode {
        Mode::Help => {
            println!("{}", args::USAGE);
            ExitCode::SUCCESS
        }
        Mode::Version => {
            println!("rustmodem-svc {VERSION}");
            ExitCode::SUCCESS
        }
        other => run(other),
    }
}

#[cfg(windows)]
fn run(mode: Mode) -> ExitCode {
    let outcome = match mode {
        Mode::Console(config) => console::run(config),
        Mode::Service(config) => service::run(config),
        Mode::Install(config) => service::install(config),
        Mode::Uninstall => service::uninstall(),
        Mode::Help | Mode::Version => Ok(()),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(problems) => {
            for problem in &problems {
                eprintln!("{problem}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run(_mode: Mode) -> ExitCode {
    // The service is for 64-bit Windows only; this exists so that the
    // workspace still builds elsewhere.
    eprintln!("{}", serial::unavailable_message("com0com"));
    ExitCode::FAILURE
}
