// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Writes the modem INF, for the build of the driver package:
//!
//!   cargo run -p modeminf -- <file>

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: modeminf <file>");
        return ExitCode::from(2);
    };
    match std::fs::write(&path, modeminf::generate()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{path}: {error}");
            ExitCode::FAILURE
        }
    }
}
