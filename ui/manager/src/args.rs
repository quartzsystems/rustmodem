// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The manager's command line.
//!
//! ```text
//! rustmodem-gui [--pipe NAME] [--demo]
//! ```

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
}

impl Default for Args {
    fn default() -> Self {
        Args {
            pipe: DEFAULT_PIPE.to_string(),
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
                other => return Err(message("RMD0009", &[other])),
            }
        }
        Ok(args)
    }

    /// The arguments that say the same again: what the manager gives the
    /// copy of itself it starts with administrator rights.
    pub fn arguments(&self) -> Vec<String> {
        if self.pipe == DEFAULT_PIPE {
            Vec::new()
        } else {
            vec!["--pipe".to_string(), self.pipe.clone()]
        }
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
    fn what_is_not_understood_is_refused_by_name() {
        for (words, named) in [
            (&["--pipe"][..], "--pipe"),
            (&["--pipe", ""][..], "--pipe"),
            (&["--pipe", "--demo"][..], "--pipe"),
            (&["--verbose"][..], "--verbose"),
            (&["start"][..], "start"),
        ] {
            let problem = parse(words).unwrap_err();
            assert_eq!(problem.id, "RMD0009", "{words:?}");
            assert!(problem.text.contains(named), "{words:?}: {}", problem.text);
        }
    }

    #[test]
    fn the_arguments_given_back_parse_to_the_same() {
        for words in [&[][..], &["--demo"][..], &["--pipe", "mine"][..]] {
            let args = parse(words).unwrap();
            assert_eq!(Args::parse(args.arguments()).unwrap(), args);
        }
        assert!(Args::default().arguments().is_empty());
    }
}
