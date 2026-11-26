// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The manager's command line.
//!
//! ```text
//! rustmodem-gui [--pipe NAME] [--demo] [start|stop|restart|install|uninstall]
//! ```
//!
//! The last argument names a service command. It is how the manager hands
//! a command that needs administrator rights to the copy of itself it
//! starts with them: that copy opens its window and carries the command
//! out.

use model::Command;
use msg::{Message, message};

/// The control pipe of the service, when none is named.
pub const DEFAULT_PIPE: &str = "rustmodem-control";

/// The control pipe of the demo service, which `--demo` names.
pub const DEMO_PIPE: &str = "rustmodem-demo";

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// The name of the control pipe to connect to.
    pub pipe: String,
    /// The service command to carry out once the window is open.
    pub action: Option<Command>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            pipe: DEFAULT_PIPE.to_string(),
            action: None,
        }
    }
}

impl Args {
    /// Reads the arguments after the program name. An argument that is
    /// not understood is message `RMD0009`.
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Args, Message> {
        let mut args = Args::default();
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--pipe" => match arguments.next() {
                    Some(name) if !name.is_empty() && !name.starts_with("--") => args.pipe = name,
                    _ => return Err(message("RMD0009", &["--pipe"])),
                },
                "--demo" => args.pipe = DEMO_PIPE.to_string(),
                other => match Command::from_argument(other) {
                    // One command, and nothing after it.
                    Some(command) if args.action.is_none() => args.action = Some(command),
                    _ => return Err(message("RMD0009", &[other])),
                },
            }
        }
        Ok(args)
    }

    /// The arguments that say the same again: what the manager gives the
    /// copy of itself it starts with administrator rights.
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        if self.pipe != DEFAULT_PIPE {
            arguments.extend(["--pipe".to_string(), self.pipe.clone()]);
        }
        if let Some(argument) = self.action.and_then(Command::argument) {
            arguments.push(argument.to_string());
        }
        arguments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<Args, Message> {
        Args::parse(words.iter().map(|word| word.to_string()))
    }

    #[test]
    fn with_no_arguments_the_pipe_is_the_service_s_own() {
        assert_eq!(parse(&[]).unwrap().pipe, "rustmodem-control");
    }

    #[test]
    fn a_pipe_can_be_named_and_demo_names_the_demo_service_s() {
        assert_eq!(parse(&["--pipe", "other"]).unwrap().pipe, "other");
        assert_eq!(parse(&["--demo"]).unwrap().pipe, "rustmodem-demo");
        // The last one said wins.
        assert_eq!(parse(&["--demo", "--pipe", "mine"]).unwrap().pipe, "mine");
        assert_eq!(
            parse(&["--pipe", "mine", "--demo"]).unwrap().pipe,
            "rustmodem-demo"
        );
    }

    #[test]
    fn a_service_command_can_be_named() {
        assert_eq!(parse(&[]).unwrap().action, None);
        for (word, command) in [
            ("start", Command::ServiceStart),
            ("stop", Command::ServiceStop),
            ("restart", Command::ServiceRestart),
            ("install", Command::ServiceInstall),
            ("uninstall", Command::ServiceUninstall),
        ] {
            let args = parse(&["--pipe", "mine", word]).unwrap();
            assert_eq!(args.action, Some(command), "{word}");
            assert_eq!(args.pipe, "mine");
            assert_eq!(args.arguments(), ["--pipe", "mine", word]);
        }
    }

    #[test]
    fn what_is_not_understood_is_refused_by_name() {
        for (words, named) in [
            (&["--pipe"][..], "--pipe"),
            (&["--pipe", ""][..], "--pipe"),
            (&["--pipe", "--demo"][..], "--pipe"),
            (&["--verbose"][..], "--verbose"),
            (&["reload"][..], "reload"),
            (&["start", "stop"][..], "stop"),
        ] {
            let problem = parse(words).unwrap_err();
            assert_eq!(problem.id, "RMD0009", "{words:?}");
            assert!(problem.text.contains(named), "{words:?}: {}", problem.text);
        }
    }

    #[test]
    fn the_arguments_given_back_parse_to_the_same() {
        for words in [
            &[][..],
            &["--demo"][..],
            &["--pipe", "mine"][..],
            &["restart"][..],
            &["--demo", "install"][..],
        ] {
            let args = parse(words).unwrap();
            assert_eq!(Args::parse(args.arguments()).unwrap(), args);
        }
        assert!(Args::default().arguments().is_empty());
    }
}
