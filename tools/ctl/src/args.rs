// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The command line.

use msg::{Message, message};

/// What `--help` prints.
pub const USAGE: &str = "\
rustmodemctl, the rustmodem command-line client

Usage:
  rustmodemctl [OPTIONS] info
  rustmodemctl [OPTIONS] lines
  rustmodemctl [OPTIONS] line <id>
  rustmodemctl [OPTIONS] diag <id>      the modem of the line's call
  rustmodemctl [OPTIONS] selftest <a> <b>
  rustmodemctl [OPTIONS] answer|hangup|reset|enable|disable <id>
  rustmodemctl [OPTIONS] calls [--last N]
  rustmodemctl [OPTIONS] reload
  rustmodemctl [OPTIONS] stats
  rustmodemctl [OPTIONS] ports
  rustmodemctl [OPTIONS] log [--last N]
  rustmodemctl [OPTIONS] watch          prints events until interrupted
  rustmodemctl [OPTIONS] sip            the listener and the registrations
  rustmodemctl [OPTIONS] reregister [<id>]
  rustmodemctl [OPTIONS] groups         the hunt groups
  rustmodemctl [OPTIONS] routes         the routes, in the order tried
  rustmodemctl [OPTIONS] capture on|off <id>
  rustmodemctl [--json] capture check <file>
  rustmodemctl capture wav <file> <out.wav>

selftest joins the modems of two idle lines, the first originating and
the second answering, with no network between them, and has each send the
other 64 KiB. It takes some seconds and exits with 0 only when the modems
connected and every octet arrived.

capture on and off turn the capture of a line's calls on and off. The
other capture commands read a call capture file and ask nothing of the
service. capture check exits with 0 only when the call lost nothing,
resynchronised nothing and slipped nothing; capture wav writes the call as
a two-channel WAV file, received on the left and sent on the right.

Options:
  --pipe NAME    the control pipe; default rustmodem-control
  --json         print the service's answer as JSON, for scripts
  --verbose      with an error, print its cause and recovery too
  --help, --version

Exit code: 0 on success, 1 on an error from the service, 2 when the
service cannot be reached, 3 when the command line is not understood.

The control pipe is open to SYSTEM and Administrators only: run
rustmodemctl from an elevated prompt.";

/// The default control pipe name.
pub const DEFAULT_PIPE: &str = "rustmodem-control";

/// What to ask the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// `info`
    Info,
    /// `lines`
    Lines,
    /// `line <id>`
    Line(u16),
    /// `diag <id>`
    Diag(u16),
    /// `selftest <a> <b>`
    SelfTest(u16, u16),
    /// `answer <id>`
    Answer(u16),
    /// `hangup <id>`
    Hangup(u16),
    /// `reset <id>`
    Reset(u16),
    /// `enable <id>`
    Enable(u16),
    /// `disable <id>`
    Disable(u16),
    /// `calls [--last N]`
    Calls(Option<u32>),
    /// `reload`
    Reload,
    /// `stats`
    Stats,
    /// `ports`
    Ports,
    /// `log [--last N]`
    Log(Option<u32>),
    /// `watch`
    Watch,
    /// `sip`
    Sip,
    /// `reregister [<id>]`
    Reregister(Option<u16>),
    /// `groups`
    Groups,
    /// `routes`
    Routes,
    /// `capture on <id>` and `capture off <id>`
    Capture(u16, bool),
    /// `capture check <file>`; the file is the first of the paths.
    CaptureCheck,
    /// `capture wav <file> <out.wav>`; the files are the paths.
    CaptureWav,
    /// `--help`
    Help,
    /// `--version`
    Version,
}

/// The command line, understood.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The control pipe name.
    pub pipe: String,
    /// Print answers as JSON.
    pub json: bool,
    /// Print the cause and recovery of an error.
    pub verbose: bool,
    /// What to ask the service.
    pub command: Command,
    /// The files a capture command names.
    pub paths: Vec<String>,
}

/// Reads the arguments after the program name. An argument that is not
/// understood is message `RMD0009`.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Options, Message> {
    let refuse = |argument: &str| message("RMD0009", &[argument]);
    let mut pipe = DEFAULT_PIPE.to_string();
    let mut json = false;
    let mut verbose = false;
    let mut last = None;
    let mut words: Vec<String> = Vec::new();

    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pipe" => pipe = args.next().ok_or_else(|| refuse("--pipe"))?,
            "--json" => json = true,
            "--verbose" | "-v" => verbose = true,
            "--last" => {
                let count = args.next().ok_or_else(|| refuse("--last"))?;
                last = Some(count.parse::<u32>().map_err(|_| refuse(&count))?);
            }
            "--help" | "-h" | "/?" => words.insert(0, "--help".to_string()),
            "--version" | "-V" => words.insert(0, "--version".to_string()),
            other if other.starts_with('-') => return Err(refuse(other)),
            _ => words.push(arg),
        }
    }

    let id = |words: &[String]| -> Result<u16, Message> {
        match words {
            [_, id] => id.parse::<u16>().map_err(|_| refuse(id)),
            [name] => Err(refuse(name)),
            [_, _, extra, ..] => Err(refuse(extra)),
            [] => Err(refuse("")),
        }
    };
    let alone = |words: &[String], command: Command| match words.get(1) {
        Some(extra) => Err(refuse(extra)),
        None => Ok(command),
    };
    let name = words.first().map(String::as_str);
    let mut paths = Vec::new();
    let command = match name {
        Some("--help") => Command::Help,
        Some("--version") => Command::Version,
        Some("info") => alone(&words, Command::Info)?,
        Some("lines") => alone(&words, Command::Lines)?,
        Some("line") => Command::Line(id(&words)?),
        Some("diag") => Command::Diag(id(&words)?),
        Some("selftest") => match &words[1..] {
            [a, b] => Command::SelfTest(
                a.parse().map_err(|_| refuse(a))?,
                b.parse().map_err(|_| refuse(b))?,
            ),
            [] | [_] => return Err(refuse("selftest")),
            [_, _, extra, ..] => return Err(refuse(extra)),
        },
        Some("answer") => Command::Answer(id(&words)?),
        Some("hangup") => Command::Hangup(id(&words)?),
        Some("reset") => Command::Reset(id(&words)?),
        Some("enable") => Command::Enable(id(&words)?),
        Some("disable") => Command::Disable(id(&words)?),
        Some("calls") => alone(&words, Command::Calls(last))?,
        Some("reload") => alone(&words, Command::Reload)?,
        Some("stats") => alone(&words, Command::Stats)?,
        Some("ports") => alone(&words, Command::Ports)?,
        Some("log") => alone(&words, Command::Log(last))?,
        Some("watch") => alone(&words, Command::Watch)?,
        Some("sip") => alone(&words, Command::Sip)?,
        Some("reregister") => match &words[1..] {
            [] => Command::Reregister(None),
            _ => Command::Reregister(Some(id(&words)?)),
        },
        Some("groups") => alone(&words, Command::Groups)?,
        Some("routes") => alone(&words, Command::Routes)?,
        Some("capture") => match &words[1..] {
            [switch, ..] if switch == "on" || switch == "off" => {
                Command::Capture(id(&words[1..])?, switch == "on")
            }
            [check, file] if check == "check" => {
                paths.push(file.clone());
                Command::CaptureCheck
            }
            [wav, file, out] if wav == "wav" => {
                paths.extend([file.clone(), out.clone()]);
                Command::CaptureWav
            }
            [] => return Err(refuse("capture")),
            [first, ..] if first != "check" && first != "wav" => return Err(refuse(first)),
            // The right word with the wrong number of files.
            [first] => return Err(refuse(first)),
            [.., last] => return Err(refuse(last)),
        },
        Some(other) => return Err(refuse(other)),
        None => Command::Help,
    };
    // --last belongs to calls and log and to nothing else.
    if last.is_some() && !matches!(command, Command::Calls(_) | Command::Log(_)) {
        return Err(refuse("--last"));
    }
    Ok(Options {
        pipe,
        json,
        verbose,
        command,
        paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Options, Message> {
        parse(words.iter().map(|w| w.to_string()))
    }

    fn command(words: &[&str]) -> Command {
        parse_words(words).unwrap().command
    }

    #[test]
    fn every_command_is_understood() {
        assert_eq!(command(&["info"]), Command::Info);
        assert_eq!(command(&["lines"]), Command::Lines);
        assert_eq!(command(&["line", "3"]), Command::Line(3));
        assert_eq!(command(&["diag", "3"]), Command::Diag(3));
        assert_eq!(command(&["selftest", "1", "2"]), Command::SelfTest(1, 2));
        assert_eq!(command(&["answer", "1"]), Command::Answer(1));
        assert_eq!(command(&["hangup", "2"]), Command::Hangup(2));
        assert_eq!(command(&["reset", "999"]), Command::Reset(999));
        assert_eq!(command(&["enable", "4"]), Command::Enable(4));
        assert_eq!(command(&["disable", "4"]), Command::Disable(4));
        assert_eq!(command(&["calls"]), Command::Calls(None));
        assert_eq!(command(&["calls", "--last", "5"]), Command::Calls(Some(5)));
        assert_eq!(command(&["--last", "5", "calls"]), Command::Calls(Some(5)));
        assert_eq!(command(&["reload"]), Command::Reload);
        assert_eq!(command(&["stats"]), Command::Stats);
        assert_eq!(command(&["ports"]), Command::Ports);
        assert_eq!(command(&["log"]), Command::Log(None));
        assert_eq!(command(&["log", "--last", "50"]), Command::Log(Some(50)));
        assert_eq!(command(&["watch"]), Command::Watch);
        assert_eq!(command(&["sip"]), Command::Sip);
        assert_eq!(command(&["reregister"]), Command::Reregister(None));
        assert_eq!(command(&["reregister", "3"]), Command::Reregister(Some(3)));
        assert_eq!(command(&["groups"]), Command::Groups);
        assert_eq!(command(&["routes"]), Command::Routes);
        assert_eq!(command(&["capture", "on", "3"]), Command::Capture(3, true));
        assert_eq!(
            command(&["capture", "off", "12"]),
            Command::Capture(12, false)
        );
        let check = parse_words(&["capture", "check", "a.rmcap", "--json"]).unwrap();
        assert_eq!((check.command, check.json), (Command::CaptureCheck, true));
        assert_eq!(check.paths, ["a.rmcap"]);
        let wav = parse_words(&["capture", "wav", r"C:\caps\a.rmcap", "a.wav"]).unwrap();
        assert_eq!(wav.command, Command::CaptureWav);
        assert_eq!(wav.paths, [r"C:\caps\a.rmcap", "a.wav"]);
        assert_eq!(command(&["--help"]), Command::Help);
        assert_eq!(command(&["lines", "--help"]), Command::Help);
        assert_eq!(command(&["--version"]), Command::Version);
        assert_eq!(command(&[]), Command::Help);
    }

    #[test]
    fn options_may_come_anywhere() {
        let options = parse_words(&["--pipe", "other", "lines", "--json", "--verbose"]).unwrap();
        assert_eq!(
            options,
            Options {
                pipe: "other".to_string(),
                json: true,
                verbose: true,
                command: Command::Lines,
                paths: Vec::new(),
            }
        );
        let options = parse_words(&["lines"]).unwrap();
        assert_eq!(options.pipe, DEFAULT_PIPE);
        assert!(!options.json && !options.verbose);
    }

    #[test]
    fn what_is_not_understood_is_message_0009() {
        for words in [
            &["frobnicate"][..],
            &["line"],
            &["line", "one"],
            &["line", "1", "2"],
            &["line", "70000"],
            &["diag"],
            &["diag", "1", "2"],
            &["selftest"],
            &["selftest", "1"],
            &["selftest", "1", "two"],
            &["selftest", "1", "2", "3"],
            &["lines", "extra"],
            &["--pipe"],
            &["calls", "--last"],
            &["calls", "--last", "many"],
            &["lines", "--last", "3"],
            &["stats", "--last", "3"],
            &["ports", "extra"],
            &["log", "7"],
            &["--frob", "lines"],
            &["watch", "now"],
            &["capture"],
            &["capture", "check"],
            &["capture", "check", "a", "b"],
            &["capture", "wav", "a"],
            &["capture", "wav", "a", "b", "c"],
            &["capture", "play", "a"],
            &["capture", "on"],
            &["capture", "on", "three"],
            &["capture", "off", "1", "2"],
            &["sip", "status"],
            &["reregister", "all"],
            &["reregister", "1", "2"],
            &["groups", "1"],
            &["routes", "now"],
        ] {
            let problem = parse_words(words).unwrap_err();
            assert_eq!(problem.id, "RMD0009", "{words:?}");
        }
    }
}
