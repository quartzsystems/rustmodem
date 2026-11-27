// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Replays every host command log in `tests/hostlogs/`: the lines of the
//! service's own log that say what a host sent a line and what the line
//! answered. `tests/hostlogs/README.md` has the format and how a log is
//! added.
//!
//! Each command of a log is given to a new interpreter, which must issue
//! the result codes the log has, and none of them may be `ERROR`: a host
//! whose initialisation is answered `ERROR` takes the modem for broken.

mod common;

use std::collections::VecDeque;
use std::path::Path;

use at::{AtInterpreter, ConnectInfo, Input, LineEvent, Output, Trace};

/// A line of a log that says something: what the host sent, or a result
/// code the line issued.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Said {
    Host(String),
    Modem(String),
}

/// Reads a line of the service's log: the line of the service it is of,
/// and what was said. `None` for a line that is neither a command nor a
/// result code.
fn said(line: &str) -> Option<(&str, Said)> {
    let (_, rest) = line.split_once('[')?;
    let (source, rest) = rest.split_once("] ")?;
    if let Some(command) = rest.strip_prefix("> ") {
        Some((source, Said::Host(command.to_string())))
    } else {
        let code = rest.strip_prefix("< ")?;
        Some((source, Said::Modem(code.to_string())))
    }
}

/// What a call connected with, as the reports before its `CONNECT` and
/// the `CONNECT` itself say it.
fn connected(reports: &[String], connect: &str) -> ConnectInfo {
    let after = |prefix: &str| {
        reports
            .iter()
            .find_map(|report| report.strip_prefix(prefix))
            .map(str::trim)
    };
    let rate = connect
        .strip_prefix("CONNECT")
        .and_then(|rate| rate.trim().parse().ok())
        .unwrap_or(0);
    // The carrier as the modem names it in a report, back to its name.
    let carrier = after("+MCR:").map(|carrier| {
        match carrier {
            "V22B" => "V.22bis",
            "V22" => "V.22",
            "B212A" => "BELL212A",
            "V21" => "V.21",
            "B103" => "BELL103",
            other => other,
        }
        .to_string()
    });
    let protocol = after("+ER:")
        .filter(|protocol| *protocol != "NONE")
        .map(str::to_string);
    let compression = after("+DR:")
        .filter(|compression| *compression != "NONE")
        .map(|compression| match compression {
            "V42B" => "V.42bis".to_string(),
            "V42B RD" => "V.42bis RD".to_string(),
            "V42B TD" => "V.42bis TD".to_string(),
            other => other.to_string(),
        });
    ConnectInfo {
        rate,
        carrier,
        protocol,
        compression,
    }
}

/// The interpreter of a replay, with the result codes it has issued and
/// that the log has not yet accounted for.
struct Replay {
    interpreter: AtInterpreter,
    issued: VecDeque<String>,
}

impl Replay {
    fn feed(&mut self, input: Input<'_>) {
        let mut out = Vec::new();
        self.interpreter.step(input, &mut out);
        for output in out {
            if let Output::Trace(Trace::Result(code)) = output {
                self.issued.push_back(code);
            }
        }
    }
}

/// Replays a log. Gives how many commands it had.
fn replay(text: &str) -> Result<usize, String> {
    let mut lines = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .peekable();
    let mut config = at::AtConfig::default();
    if let Some(words) = lines.peek().and_then(|line| line.strip_prefix("# config")) {
        config = common::config(words.split_whitespace())?;
    }
    let mut replay = Replay {
        interpreter: AtInterpreter::new(config),
        issued: VecDeque::new(),
    };
    replay.feed(Input::Dtr(true));
    let said: Vec<(usize, &str, Said)> = lines
        .enumerate()
        .filter(|(_, line)| !line.starts_with('#'))
        .filter_map(|(index, line)| said(line).map(|(source, said)| (index + 1, source, said)))
        .collect();
    let mut commands = 0;
    let mut of_line = None;
    for (at, (number, source, what)) in said.iter().enumerate() {
        let number = *number;
        if *of_line.get_or_insert(*source) != *source {
            return Err(format!(
                "line {number}: the log has more than one line of the service in it"
            ));
        }
        match what {
            Said::Host(command) => {
                if let Some(code) = replay.issued.front() {
                    return Err(format!(
                        "line {number}: the modem said `{code}` before this, and the log has \
                         not got it"
                    ));
                }
                commands += 1;
                let mut bytes = command.clone().into_bytes();
                bytes.push(b'\r');
                replay.feed(Input::DteBytes(&bytes));
            }
            Said::Modem(code) => {
                if code == "ERROR" {
                    return Err(format!("line {number}: the host was answered ERROR"));
                }
                if replay.issued.is_empty() {
                    // Nothing the host sent accounts for it: it is what
                    // happened on the line.
                    if code == "RING" {
                        replay.feed(Input::Event(LineEvent::Ring));
                    } else if code == "NO CARRIER" {
                        replay.feed(Input::Event(LineEvent::CarrierLost));
                    } else if code.starts_with("CONNECT") || code.starts_with('+') {
                        // The reports of a connection and its CONNECT
                        // come together, from one event.
                        let rest: Vec<String> = said[at..]
                            .iter()
                            .map_while(|(_, _, said)| match said {
                                Said::Modem(code) => Some(code.clone()),
                                Said::Host(_) => None,
                            })
                            .collect();
                        let Some(end) = rest.iter().position(|code| code.starts_with("CONNECT"))
                        else {
                            return Err(format!(
                                "line {number}: `{code}` with no CONNECT after it"
                            ));
                        };
                        let info = connected(&rest[..end], &rest[end]);
                        replay.feed(Input::Event(LineEvent::Connected(info)));
                    }
                }
                match replay.issued.pop_front() {
                    Some(issued) if issued == *code => {}
                    Some(issued) => {
                        return Err(format!(
                            "line {number}: the log has `{code}` and the modem says `{issued}`"
                        ));
                    }
                    None => {
                        return Err(format!(
                            "line {number}: the log has `{code}` and the modem says nothing"
                        ));
                    }
                }
            }
        }
    }
    match replay.issued.front() {
        Some(code) => Err(format!(
            "at the end: the modem said `{code}`, and the log has not got it"
        )),
        None => Ok(commands),
    }
}

#[test]
fn every_host_log_is_answered_as_it_was_and_never_with_error() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hostlogs");
    let mut paths: Vec<_> = std::fs::read_dir(&directory)
        .expect("the host logs directory")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "txt"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "there is no host log");
    let mut failures = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).expect("a readable log");
        match replay(&text) {
            Ok(0) => failures.push(format!("{name}: no command in it")),
            Ok(_) => {}
            Err(problem) => failures.push(format!("{name}: {problem}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} host logs failed:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}

#[test]
fn a_line_of_the_log_is_read_whatever_is_before_its_source() {
    assert_eq!(
        said("2027-01-15T08:00:02.120Z DEBUG [Line01] > ATE0V1"),
        Some(("Line01", Said::Host("ATE0V1".to_string())))
    );
    assert_eq!(
        said("08:24:50 DEBUG [Line03] < NO CARRIER"),
        Some(("Line03", Said::Modem("NO CARRIER".to_string())))
    );
    assert_eq!(
        said("2027-01-15T08:00:02.142Z INFO  [Line01] RMD1005: Line 1 state is now idle."),
        None
    );
    assert_eq!(said("nothing of the kind"), None);
}

#[test]
fn a_log_that_the_modem_would_not_have_made_is_noticed() {
    let log = |lines: &[&str]| -> String {
        lines
            .iter()
            .map(|line| format!("2027-01-15T08:00:00.000Z DEBUG [Line01] {line}\n"))
            .collect()
    };
    assert_eq!(replay(&log(&["> AT", "< OK", "> ATE0", "< OK"])), Ok(2));
    // An answer of ERROR, whether or not the modem would give it.
    let error = replay(&log(&["> AT+MS=V34", "< ERROR"])).unwrap_err();
    assert!(error.contains("answered ERROR"), "{error}");
    // An answer the modem does not give.
    let other = replay(&log(&["> AT+MS=V34", "< OK"])).unwrap_err();
    assert!(other.contains("the modem says `ERROR`"), "{other}");
    // A command whose answer the log has lost.
    let lost = replay(&log(&["> AT", "> AT", "< OK"])).unwrap_err();
    assert!(lost.contains("has not got it"), "{lost}");
    // A call: a ring, the answer, the reports and the far end clearing.
    let call = log(&[
        "> AT+MR=1;+ER=1",
        "< OK",
        "< RING",
        "> ATA",
        "< +MCR: V22B",
        "< +MRR: 2400",
        "< +ER: LAPM",
        "< CONNECT 2400",
        "< NO CARRIER",
    ]);
    assert_eq!(replay(&call), Ok(2));
    // The configuration of the line is the first line's to say.
    let skipped = "# config unknown_command=ok\n".to_string() + &log(&["> AT#CID=1", "< OK"]);
    assert_eq!(replay(&skipped), Ok(1));
    let refused = "# config unknown_command=error\n".to_string() + &log(&["> AT#CID=1", "< OK"]);
    assert!(replay(&refused).is_err());
    // Two lines of the service in one file.
    let two = "t DEBUG [Line01] > AT\nt DEBUG [Line01] < OK\nt DEBUG [Line02] > AT\n";
    assert!(replay(two).unwrap_err().contains("more than one line"));
}
