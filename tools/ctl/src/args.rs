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
  rustmodemctl [OPTIONS] answer|hangup|reset|enable|disable <id>
  rustmodemctl [OPTIONS] calls [--last N]
  rustmodemctl [OPTIONS] reload
  rustmodemctl [OPTIONS] stats
  rustmodemctl [OPTIONS] ports
  rustmodemctl [OPTIONS] log [--last N]
  rustmodemctl [OPTIONS] watch          prints events until interrupted

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
    let command = match name {
        Some("--help") => Command::Help,
        Some("--version") => Command::Version,
        Some("info") => alone(&words, Command::Info)?,
        Some("lines") => alone(&words, Command::Lines)?,
        Some("line") => Command::Line(id(&words)?),
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
        ] {
            let problem = parse_words(words).unwrap_err();
            assert_eq!(problem.id, "RMD0009", "{words:?}");
        }
    }
}
