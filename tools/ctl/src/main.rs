// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The rustmodemctl command-line client.
//!
//! A thin client of the control API in `ctlproto`: it opens the service's
//! named pipe, makes one call, and prints the answer as a table, or as JSON
//! with `--json`. `watch` subscribes and prints events until interrupted.
//!
//! Errors print the `RMD` id and text; `--verbose` adds the cause and the
//! recovery. The exit code is 0 on success, 1 on an error from the service,
//! 2 when the service cannot be reached, and 3 when the command line is not
//! understood.

#![forbid(unsafe_code)]

mod args;
mod capture;
mod client;
mod output;
#[cfg(test)]
mod tests;

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use args::{Command, Options};
use client::{Client, Failure};
use ctlproto::{
    Call, CallList, CallsQuery, CaptureSwitch, Done, ErrorInfo, EventSelection, HuntGroupList,
    LineInfo, LineList, LineRef, LogTail, LogTailQuery, PortList, Reloaded, ReregisterQuery,
    RouteList, ServiceInfo, SipStatus, Stats, encode,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The exit code for an error from the service.
const EXIT_API: u8 = 1;

/// The exit code when the service cannot be reached.
const EXIT_CONNECTION: u8 = 2;

/// The exit code for a command line that was not understood.
const EXIT_USAGE: u8 = 3;

fn main() -> ExitCode {
    let options = match args::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("{problem}");
            eprintln!("{}", problem.recovery);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match options.command {
        Command::Help => {
            println!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!("rustmodemctl {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        // The capture commands read a file and need no service.
        Command::CaptureCheck | Command::CaptureWav => {
            let stdout = io::stdout();
            let mut out = stdout.lock();
            let outcome = match (options.command, options.paths.as_slice()) {
                (Command::CaptureCheck, [file]) => capture::run_check(file, options.json, &mut out),
                (_, [file, wav]) => capture::run_wav(file, wav, &mut out).map(|()| true),
                _ => Ok(true),
            };
            return match outcome {
                Ok(true) => ExitCode::SUCCESS,
                // The capture was read, and the call in it was not clean.
                Ok(false) => ExitCode::from(EXIT_API),
                Err(problem) => {
                    let error = ErrorInfo::from(problem);
                    eprint!("{}", output::error(&error, options.verbose));
                    ExitCode::from(EXIT_API)
                }
            };
        }
        _ => {}
    }

    let stdout = io::stdout();
    let outcome = Client::connect(&options.pipe)
        .and_then(|mut client| run(&options, &mut client, &mut stdout.lock(), None));
    ExitCode::from(report(outcome, options.verbose, &mut io::stderr().lock()))
}

/// Prints a failure and gives the exit code for the outcome.
fn report(outcome: Result<(), Failure>, verbose: bool, errors: &mut dyn Write) -> u8 {
    let (error, code) = match outcome {
        Ok(()) => return 0,
        Err(Failure::Api(error)) => (error, EXIT_API),
        Err(Failure::Connection(problem)) => (ErrorInfo::from(problem), EXIT_CONNECTION),
    };
    let _ = errors.write_all(output::error(&error, verbose).as_bytes());
    code
}

/// Carries out a command on a connected client and prints the answer.
/// `watch_limit` ends `watch` after that many events; with `None` it runs
/// until the connection ends.
fn run<R: BufRead, W: Write>(
    options: &Options,
    client: &mut Client<R, W>,
    out: &mut dyn Write,
    watch_limit: Option<usize>,
) -> Result<(), Failure> {
    let json = options.json;
    let line_call = |id: u16, make: fn(LineRef) -> Call| make(LineRef { id });
    match options.command {
        Command::Info => {
            answer::<ServiceInfo, _, _>(client, &Call::ServiceInfo, json, out, |info| {
                output::info(info)
            })
        }
        Command::Lines => answer::<LineList, _, _>(client, &Call::LinesList, json, out, |list| {
            output::lines(&list.lines)
        }),
        Command::Line(id) => {
            let call = line_call(id, Call::LinesGet);
            answer::<LineInfo, _, _>(client, &call, json, out, output::line)
        }
        Command::Answer(id) => acted(client, &line_call(id, Call::LinesAnswer), json, out),
        Command::Hangup(id) => acted(client, &line_call(id, Call::LinesHangup), json, out),
        Command::Reset(id) => acted(client, &line_call(id, Call::LinesReset), json, out),
        Command::Enable(id) => acted(client, &line_call(id, Call::LinesEnable), json, out),
        Command::Disable(id) => acted(client, &line_call(id, Call::LinesDisable), json, out),
        Command::Calls(last) => {
            let call = Call::CallsList(CallsQuery { last });
            answer::<CallList, _, _>(client, &call, json, out, |list| output::calls(&list.calls))
        }
        Command::Reload => answer::<Reloaded, _, _>(client, &Call::ConfigReload, json, out, |r| {
            format!("Configuration reloaded: {} lines.\n", r.lines)
        }),
        Command::Stats => answer::<Stats, _, _>(client, &Call::StatsGet, json, out, output::stats),
        Command::Ports => answer::<PortList, _, _>(client, &Call::PortsList, json, out, |list| {
            output::ports(&list.ports)
        }),
        Command::Log(last) => {
            let call = Call::LogTail(LogTailQuery { last });
            answer::<LogTail, _, _>(client, &call, json, out, |tail| output::log(&tail.lines))
        }
        Command::Watch => watch(client, json, out, watch_limit),
        Command::Sip => answer::<SipStatus, _, _>(client, &Call::SipStatus, json, out, output::sip),
        Command::Reregister(id) => {
            let call = Call::SipReregister(ReregisterQuery { id });
            answer::<Done, _, _>(client, &call, json, out, |_| match id {
                Some(id) => format!("Line {id} is registering again.\n"),
                None => "Every line that registers is registering again.\n".to_string(),
            })
        }
        Command::Groups => {
            answer::<HuntGroupList, _, _>(client, &Call::HuntGroupsList, json, out, |list| {
                output::groups(&list.groups)
            })
        }
        Command::Routes => {
            answer::<RouteList, _, _>(client, &Call::RoutesList, json, out, |list| {
                output::routes(&list.routes)
            })
        }
        Command::Capture(id, on) => {
            let call = Call::LinesCapture(CaptureSwitch { id, on });
            answer::<LineInfo, _, _>(client, &call, json, out, |line| {
                format!(
                    "Line {}: capture {}\n",
                    line.id,
                    if line.capture { "on" } else { "off" }
                )
            })
        }
        // Handled before a connection is made.
        Command::Help | Command::Version | Command::CaptureCheck | Command::CaptureWav => Ok(()),
    }
}

/// Makes a call and prints its result: as JSON, or as `show` lays it out.
fn answer<T, R, W>(
    client: &mut Client<R, W>,
    call: &Call,
    json: bool,
    out: &mut dyn Write,
    show: impl FnOnce(&T) -> String,
) -> Result<(), Failure>
where
    T: DeserializeOwned + Serialize,
    R: BufRead,
    W: Write,
{
    let result: T = client.call(call)?;
    let text = if json {
        // What the service sent, read into its type and written back: the
        // keys are in the documented order whatever order they came in.
        format!("{}\n", encode(&result))
    } else {
        show(&result)
    };
    print(out, &text);
    Ok(())
}

/// Makes a call that acts on a line and prints the line's state afterwards.
fn acted<R: BufRead, W: Write>(
    client: &mut Client<R, W>,
    call: &Call,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    answer::<LineInfo, _, _>(client, call, json, out, output::acted)
}

/// Subscribes to every event and prints each as it comes.
fn watch<R: BufRead, W: Write>(
    client: &mut Client<R, W>,
    json: bool,
    out: &mut dyn Write,
    limit: Option<usize>,
) -> Result<(), Failure> {
    client.call_value(&Call::Subscribe(EventSelection::default()))?;
    let mut seen = 0;
    while limit.is_none_or(|limit| seen < limit) {
        let event = client.next_event()?;
        let text = if json {
            encode(&event)
        } else {
            output::event(&event)
        };
        print(out, &format!("{text}\n"));
        seen += 1;
    }
    Ok(())
}

/// Writes to standard output. A reader that has gone away, as when the
/// output is piped to a program that stops reading, is not an error worth
/// reporting.
fn print(out: &mut dyn Write, text: &str) {
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}
