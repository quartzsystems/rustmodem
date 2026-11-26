// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The command line.

use msg::{Message, message};

/// What `--help` prints.
pub const USAGE: &str = "\
rustmodem-demosvc, a stand-in for the rustmodem service

Runs a real pool and the real control API on in-memory serial ports, so
that the manager and rustmodemctl can be tried and tested with no com0com
and no elevated prompt. It is not part of the product and is never
installed as a service. Ctrl+C stops it.

Usage:
  rustmodem-demosvc [--lines N] [--script NAME] [--pipe NAME]
  rustmodem-demosvc --help
  rustmodem-demosvc --version

Options:
  --lines N      how many lines, 1 to 64; default 4
  --script NAME  what happens on them; default idle
                   idle    nothing: every line waits for a call
                   busy    the lines call each other, talk and hang up
                   frozen  a fixed set of states and counters on a clock
                           that stands still, for screenshots
  --pipe NAME    the control pipe; default rustmodem-demo

Connect with:  rustmodem-gui --demo
               rustmodemctl --pipe rustmodem-demo lines";

/// The default control pipe name.
pub const DEFAULT_PIPE: &str = "rustmodem-demo";

/// The most lines the demo service runs.
pub const MAX_LINES: u16 = 64;

/// What happens on the lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Nothing happens.
    Idle,
    /// Scripted hosts dial each other, exchange data and hang up.
    Busy,
    /// A fixed, deterministic set of states and counters with a fixed
    /// clock.
    Frozen,
}

impl Script {
    /// The script as the command line spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Script::Idle => "idle",
            Script::Busy => "busy",
            Script::Frozen => "frozen",
        }
    }
}

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// How many lines.
    pub lines: u16,
    /// What happens on them.
    pub script: Script,
    /// The control pipe name.
    pub pipe: String,
}

/// What the program was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Run the demo service.
    Run(Options),
    /// Print the usage.
    Help,
    /// Print the version.
    Version,
}

/// Reads the arguments after the program name. An argument that is not
/// understood is message `RMD0009`.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Mode, Message> {
    let refuse = |argument: &str| message("RMD0009", &[argument]);
    let mut options = Options {
        lines: 4,
        script: Script::Idle,
        pipe: DEFAULT_PIPE.to_string(),
    };
    let mut mode = None;
    let mut args = args;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| refuse(name));
        match arg.as_str() {
            "--lines" => {
                let count = value("--lines")?;
                options.lines = count
                    .parse::<u16>()
                    .ok()
                    .filter(|lines| (1..=MAX_LINES).contains(lines))
                    .ok_or_else(|| refuse(&count))?;
            }
            "--script" => {
                let name = value("--script")?;
                options.script = match name.as_str() {
                    "idle" => Script::Idle,
                    "busy" => Script::Busy,
                    "frozen" => Script::Frozen,
                    _ => return Err(refuse(&name)),
                };
            }
            "--pipe" => {
                let name = value("--pipe")?;
                // What the configuration accepts for control.pipe.
                let valid = !name.is_empty()
                    && name.len() <= 64
                    && name
                        .chars()
                        .all(|c| c.is_ascii_graphic() && c != '\\' && c != '/');
                if !valid {
                    return Err(refuse(&name));
                }
                options.pipe = name;
            }
            "--help" | "-h" | "/?" => mode = Some(Mode::Help),
            "--version" | "-V" => mode = mode.or(Some(Mode::Version)),
            other => return Err(refuse(other)),
        }
    }
    Ok(mode.unwrap_or(Mode::Run(options)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Mode, Message> {
        parse(words.iter().map(|w| w.to_string()))
    }

    #[test]
    fn the_defaults_are_four_idle_lines_on_the_demo_pipe() {
        assert_eq!(
            parse_words(&[]).unwrap(),
            Mode::Run(Options {
                lines: 4,
                script: Script::Idle,
                pipe: "rustmodem-demo".to_string(),
            })
        );
    }

    #[test]
    fn every_option_is_understood() {
        assert_eq!(
            parse_words(&["--script", "busy", "--lines", "12", "--pipe", "other"]).unwrap(),
            Mode::Run(Options {
                lines: 12,
                script: Script::Busy,
                pipe: "other".to_string(),
            })
        );
        for script in [Script::Idle, Script::Busy, Script::Frozen] {
            let Mode::Run(options) = parse_words(&["--script", script.as_str()]).unwrap() else {
                panic!("expected to run");
            };
            assert_eq!(options.script, script);
        }
        assert_eq!(parse_words(&["--help"]).unwrap(), Mode::Help);
        assert_eq!(parse_words(&["--lines", "2", "-h"]).unwrap(), Mode::Help);
        assert_eq!(parse_words(&["--version"]).unwrap(), Mode::Version);
        assert!(parse_words(&["--lines", "64"]).is_ok());
    }

    #[test]
    fn what_is_not_understood_is_message_0009() {
        for words in [
            &["--lines"][..],
            &["--lines", "0"],
            &["--lines", "65"],
            &["--lines", "many"],
            &["--script"],
            &["--script", "chaos"],
            &["--pipe"],
            &["--pipe", "has space"],
            &["--pipe", r"a\b"],
            &["--console"],
            &["extra"],
        ] {
            let problem = parse_words(words).unwrap_err();
            assert_eq!(problem.id, "RMD0009", "{words:?}");
        }
    }
}
