// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `rustmodem-replay`: gives the frames a captured call received to the
//! modem engine and says what the engine does with them.
//!
//! It is a tool for whoever develops rustmodem and is not shipped, so what
//! it says is plain text and not messages of the catalogue.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::process::ExitCode;

use engine::{ErrorControl, Law};
use replay::{Options, Role, compare, modulation, render, render_with_frames, replay, write_wav};
use rtp::capture::Reader;

const USAGE: &str = "\
Usage:
  rustmodem-replay <file.rmcap> [options]
  rustmodem-replay --help
  rustmodem-replay --version

Gives the frames the captured call received to a new modem engine, one at
a time as the call's media thread did, and prints what the engine does:
its states, its events, the data it receives.

Options:
  --answer              the engine answered the call (the default)
  --originate           the engine made the call
  --law ulaw|alaw       the law of the call; the capture's by default
  --modulations <list>  the modulations the modem may use, the first by
                        choice: v22bis, v22, bell212a, v21 and bell103,
                        with commas between; all of them by default
  --no-v8               the modem does not use V.8
  --error-control auto|required|off
                        the modem's error control; auto by default
  --no-compression      the modem does not ask for V.42bis
  --frames              prints every frame of LAPM the data link sends and
                        receives, each where it comes
  --expect <file>       compares what the engine does with the file, which
                        has what this program prints; lines that begin
                        with # are skipped. Exits with 1 if they differ
  --tx-out <file.wav>   writes what the engine sends as sound

A replay is exact only until the engine's transmission changes. The far
end of the captured call answered what the engine of that day sent; once
the engine of today sends something else, the frames that follow are the
answer to a call that is no longer being made. The line `sent-as-captured`
says for how many frames the engine sent what the capture has. The data
the host sent is not in a capture, so the engine sends none; in a call
with error control the far end then acknowledges frames that were never
sent, and the link ends there.

";

/// The exit code for a replay that could not be done, or that differs
/// from what was expected.
const EXIT_FAILED: u8 = 1;
/// The exit code for a command line that was not understood.
const EXIT_USAGE: u8 = 2;

/// What the command line asks for.
struct Asked {
    capture: String,
    options: Options,
    expect: Option<String>,
    tx_out: Option<String>,
    frames: bool,
}

/// Reads the command line; `None` when it is not understood.
fn asked(arguments: &[String]) -> Option<Asked> {
    let mut capture = None;
    let mut options = Options::default();
    let mut expect = None;
    let mut tx_out = None;
    let mut frames = false;
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--answer" => options.role = Role::Answer,
            "--originate" => options.role = Role::Originate,
            "--no-v8" => options.modem.v8 = false,
            "--no-compression" => options.modem.compression = false,
            "--frames" => frames = true,
            "--error-control" => {
                options.modem.error_control = match rest.next()?.as_str() {
                    "auto" => ErrorControl::Auto,
                    "required" => ErrorControl::Required,
                    "off" => ErrorControl::Off,
                    _ => return None,
                };
            }
            "--law" => {
                options.law = Some(match rest.next()?.as_str() {
                    "ulaw" => Law::Ulaw,
                    "alaw" => Law::Alaw,
                    _ => return None,
                });
            }
            "--modulations" => {
                let mut chosen = Vec::new();
                for name in rest.next()?.split(',') {
                    let named = modulation(name)?;
                    if chosen.contains(&named) {
                        return None;
                    }
                    chosen.push(named);
                }
                options.modem.modulations = engine::modulations(&chosen);
            }
            "--expect" => expect = Some(rest.next()?.clone()),
            "--tx-out" => tx_out = Some(rest.next()?.clone()),
            path if !path.starts_with('-') && capture.is_none() => capture = Some(path.to_string()),
            _ => return None,
        }
    }
    Some(Asked {
        capture: capture?,
        options,
        expect,
        tx_out,
        frames,
    })
}

/// Does what was asked. `Ok(false)` when the replay differs from what was
/// expected of it.
fn run(asked: &Asked) -> Result<bool, String> {
    let path = &asked.capture;
    let file = File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let mut reader =
        Reader::new(BufReader::new(file)).map_err(|error| format!("{path}: {error}"))?;
    let outcome =
        replay(&mut reader, &asked.options).map_err(|error| format!("{path}: {error}"))?;
    // An expectation has no frames in it, whatever is printed.
    let rendered = render(&outcome);
    if asked.frames {
        print!("{}", render_with_frames(&outcome));
    } else {
        print!("{rendered}");
    }
    if let Some(path) = &asked.tx_out {
        let file = File::create(path).map_err(|error| format!("{path}: {error}"))?;
        write_wav(&mut BufWriter::new(file), &outcome.sent, outcome.alaw)
            .map_err(|error| format!("{path}: {error}"))?;
    }
    if let Some(path) = &asked.expect {
        let expected = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
        if let Err(difference) = compare(&rendered, &expected) {
            eprintln!("{path}: {difference}");
            return Ok(false);
        }
    }
    Ok(true)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [flag] if flag == "--help" || flag == "-h" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        [flag] if flag == "--version" => {
            println!("rustmodem-replay {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    let Some(asked) = asked(&arguments) else {
        eprint!("{USAGE}");
        return ExitCode::from(EXIT_USAGE);
    };
    match run(&asked) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(EXIT_FAILED),
        Err(problem) => {
            eprintln!("{problem}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}
