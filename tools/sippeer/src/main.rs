// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `rustmodem-sippeer`: the test peer as a program, for trying rustmodem
//! or a gateway by hand. It runs one script and exits with 0 when every
//! step of it could be done and every `expect` and `check` held.
//!
//! It is a tool for whoever develops rustmodem and is not shipped, so what
//! it says is plain text and not messages of the catalogue.

#![forbid(unsafe_code)]

use std::io::Read;
use std::process::ExitCode;

const USAGE: &str = "\
Usage:
  rustmodem-sippeer <script>       runs the script in a file
  rustmodem-sippeer -              runs the script on standard input
  rustmodem-sippeer --help
  rustmodem-sippeer --version

A script is one step a line; lines that begin with # are skipped.

";

/// The exit code for a script that stopped.
const EXIT_FAILED: u8 = 1;
/// The exit code for a command line that was not understood.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let script = match arguments.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            print!("{USAGE}{}", sippeer::script::HELP);
            return ExitCode::SUCCESS;
        }
        [flag] if flag == "--version" => {
            println!("rustmodem-sippeer {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        [stdin] if stdin == "-" => {
            let mut script = String::new();
            std::io::stdin()
                .read_to_string(&mut script)
                .map(|_| script)
                .map_err(|error| format!("standard input: {error}"))
        }
        [path] if !path.starts_with('-') => {
            std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let script = match script {
        Ok(script) => script,
        Err(problem) => {
            eprintln!("{problem}");
            return ExitCode::from(EXIT_FAILED);
        }
    };
    let stdout = std::io::stdout();
    match sippeer::script::run(&script, &mut stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(stopped) => {
            eprintln!("{stopped}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}
