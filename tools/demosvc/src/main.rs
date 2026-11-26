// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The rustmodem demo service.
//!
//! `rustmodem-demosvc.exe` is a stand-in for the service, for development,
//! tests and screenshots. It runs a real pool and the real control API,
//! with every line on an in-memory serial port and scripted hosts on the
//! other ends, so that the manager can be built and tried with no com0com
//! and no elevated prompt: its pipe admits the user who started it.
//!
//! It is not shipped in the product and is not installed as a service.
//! `service.info` reports its mode as `demo`.

#![forbid(unsafe_code)]

mod args;

#[cfg(windows)]
mod busy;
#[cfg(windows)]
mod demo;
#[cfg(windows)]
mod engine;
#[cfg(windows)]
mod frozen;
#[cfg(windows)]
mod hosts;

use std::process::ExitCode;

use args::Mode;

/// The version of the demo service.
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
            println!("rustmodem-demosvc {VERSION}");
            ExitCode::SUCCESS
        }
        Mode::Run(options) => run(&options),
    }
}

#[cfg(windows)]
fn run(options: &args::Options) -> ExitCode {
    match demo::run(options) {
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
fn run(_options: &args::Options) -> ExitCode {
    // The control pipe is a Windows named pipe; this exists so that the
    // workspace still builds elsewhere.
    eprintln!("rustmodem-demosvc runs on Windows only.");
    ExitCode::FAILURE
}
