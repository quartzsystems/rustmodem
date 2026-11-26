// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The command line.

use std::path::PathBuf;

use msg::{Message, message};

/// What `--help` prints.
pub const USAGE: &str = "\
rustmodem-svc, the rustmodem modem pool service

Usage:
  rustmodem-svc                       started by the service control manager
  rustmodem-svc --console [CONFIG]    run in the foreground; Ctrl+C stops it
  rustmodem-svc --install [CONFIG]    register the service 'rustmodem'
  rustmodem-svc --uninstall           remove the service
  rustmodem-svc --help
  rustmodem-svc --version

CONFIG is the path of the configuration file. The default is
%ProgramData%\\Quartz Systems\\rustmodem\\rustmodem.toml.";

/// What the program was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Run as a service, started by the service control manager. The
    /// configuration path is given when the service was installed with one.
    Service(Option<PathBuf>),
    /// Run in the foreground.
    Console(Option<PathBuf>),
    /// Register the service.
    Install(Option<PathBuf>),
    /// Remove the service.
    Uninstall,
    /// Print the usage.
    Help,
    /// Print the version.
    Version,
}

/// Reads the arguments after the program name.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Mode, Message> {
    let args: Vec<String> = args.collect();
    let refuse = |argument: &str| message("RMD0009", &[argument]);
    let Some(first) = args.first() else {
        return Ok(Mode::Service(None));
    };
    let config = args.get(1).map(PathBuf::from);
    // Each mode takes at most the configuration path after it.
    let takes_config = matches!(first.as_str(), "--console" | "--install" | "--service");
    let most = if takes_config { 2 } else { 1 };
    if let Some(extra) = args.get(most) {
        return Err(refuse(extra));
    }
    if let Some(path) = args.get(1).filter(|path| path.starts_with("--")) {
        return Err(refuse(path));
    }
    match first.as_str() {
        "--console" => Ok(Mode::Console(config)),
        "--install" => Ok(Mode::Install(config)),
        // How an installed service is started when it was installed with a
        // configuration path.
        "--service" => Ok(Mode::Service(config)),
        "--uninstall" => Ok(Mode::Uninstall),
        "--help" | "-h" | "/?" => Ok(Mode::Help),
        "--version" | "-V" => Ok(Mode::Version),
        other => Err(refuse(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Mode, Message> {
        parse(words.iter().map(|w| w.to_string()))
    }

    #[test]
    fn no_arguments_is_the_service() {
        assert_eq!(parse_words(&[]), Ok(Mode::Service(None)));
    }

    #[test]
    fn each_mode_is_understood() {
        let path = Some(PathBuf::from(r"C:\x\rustmodem.toml"));
        assert_eq!(parse_words(&["--console"]), Ok(Mode::Console(None)));
        assert_eq!(
            parse_words(&["--console", r"C:\x\rustmodem.toml"]),
            Ok(Mode::Console(path.clone()))
        );
        assert_eq!(parse_words(&["--install"]), Ok(Mode::Install(None)));
        assert_eq!(
            parse_words(&["--install", r"C:\x\rustmodem.toml"]),
            Ok(Mode::Install(path.clone()))
        );
        assert_eq!(
            parse_words(&["--service", r"C:\x\rustmodem.toml"]),
            Ok(Mode::Service(path))
        );
        assert_eq!(parse_words(&["--uninstall"]), Ok(Mode::Uninstall));
        assert_eq!(parse_words(&["--help"]), Ok(Mode::Help));
        assert_eq!(parse_words(&["--version"]), Ok(Mode::Version));
    }

    #[test]
    fn anything_else_is_message_0009() {
        for words in [
            &["--frobnicate"][..],
            &["config.toml"],
            &["--console", "a.toml", "b.toml"],
            &["--uninstall", "a.toml"],
            &["--console", "--install"],
            &["--help", "me"],
        ] {
            let problem = parse_words(words).unwrap_err();
            assert_eq!(problem.id, "RMD0009", "{words:?}");
        }
        let problem = parse_words(&["--frobnicate"]).unwrap_err();
        assert!(problem.text.contains("--frobnicate"));
    }
}
