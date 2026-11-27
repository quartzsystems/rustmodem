// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The command line of `rustmodem-setup`.

use std::path::PathBuf;

/// What the program prints for `--help` and for arguments it does not
/// understand.
pub const USAGE: &str = "\
rustmodem-setup [install] [options]     install, or upgrade what is installed
rustmodem-setup repair [options]        put back anything missing or altered
rustmodem-setup uninstall [--purge] [options]
                                        remove RustModem; --purge also removes
                                        its configuration and logs
rustmodem-setup port add [--com N] [--count K]
                                        make K ports (one unless said), the
                                        first COMN or the next free number
rustmodem-setup port remove COMn        remove a port and its modem
rustmodem-setup port list               list the RustModem ports
rustmodem-setup plan [install|uninstall] [--purge]
                                        print what would be done, and do nothing
rustmodem-setup pack <directory> <file> make a setup program from this one and
                                        the files of a directory
rustmodem-setup --version

Options:
  /quiet, --quiet               ask nothing and show nothing
  --trust-lab-certificate       a lab build trusts its certificate without
                                asking, which /quiet needs
  --log <file>                  write this run's log there

Exit codes: 0 done, 1 usage, 2 the setup program is damaged, 3 needs an
administrator, 4 cancelled, 5 failed and undone, 6 Windows not supported,
7 not installed.";

/// What the program is asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Install, or upgrade.
    Install,
    Repair,
    Uninstall {
        purge: bool,
    },
    PortAdd {
        com: Option<u32>,
        count: u32,
    },
    PortRemove {
        com: String,
    },
    PortList,
    /// Print a plan.
    Plan {
        uninstall: bool,
        purge: bool,
    },
    Pack {
        directory: PathBuf,
        output: PathBuf,
    },
    Version,
    Help,
}

impl Command {
    /// Whether the command changes the machine, and so needs an
    /// administrator.
    pub fn needs_administrator(&self) -> bool {
        matches!(
            self,
            Command::Install
                | Command::Repair
                | Command::Uninstall { .. }
                | Command::PortAdd { .. }
                | Command::PortRemove { .. }
        )
    }
}

/// A command and its options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cli {
    pub command: Command,
    pub quiet: bool,
    pub trust_lab_certificate: bool,
    pub log: Option<PathBuf>,
}

/// Reads the arguments, without the program's name. The error is what
/// was not understood.
pub fn parse(arguments: &[String]) -> Result<Cli, String> {
    let mut quiet = false;
    let mut trust = false;
    let mut purge = false;
    let mut log = None;
    let mut com = None;
    let mut count = None;
    let mut words: Vec<&str> = Vec::new();

    let mut rest = arguments.iter().map(String::as_str);
    while let Some(argument) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .ok_or_else(|| format!("{name} needs a value after it"))
        };
        match argument.to_ascii_lowercase().as_str() {
            "/quiet" | "--quiet" | "/q" => quiet = true,
            "--trust-lab-certificate" => trust = true,
            "--purge" => purge = true,
            "--log" => log = Some(PathBuf::from(value("--log")?)),
            "--com" => {
                let text = value("--com")?;
                let number = text
                    .strip_prefix("COM")
                    .or_else(|| text.strip_prefix("com"))
                    .unwrap_or(text);
                com = Some(
                    number
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=crate::ports::HIGHEST).contains(n))
                        .ok_or_else(|| format!("{text} is not a COM number"))?,
                );
            }
            "--count" => {
                let text = value("--count")?;
                count = Some(
                    text.parse::<u32>()
                        .ok()
                        .filter(|n| (1..=crate::ports::HIGHEST).contains(n))
                        .ok_or_else(|| format!("{text} is not a number of ports"))?,
                );
            }
            "--help" | "-h" | "/?" => words.insert(0, "help"),
            "--version" => words.insert(0, "version"),
            option if option.starts_with('-') => {
                return Err(format!("{argument} is not an option"));
            }
            _ => words.push(argument),
        }
    }

    let lower: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    let lower: Vec<&str> = lower.iter().map(String::as_str).collect();
    let command = match lower.as_slice() {
        ["help", ..] => Command::Help,
        ["version", ..] => Command::Version,
        [] | ["install"] | ["upgrade"] => Command::Install,
        ["repair"] => Command::Repair,
        ["uninstall"] => Command::Uninstall { purge },
        ["port", "add"] => Command::PortAdd {
            com,
            count: count.unwrap_or(1),
        },
        ["port", "remove", name] => {
            let number = crate::ports::com_number(name)
                .ok_or_else(|| format!("{} is not a COM name", words[2]))?;
            Command::PortRemove {
                com: format!("COM{number}"),
            }
        }
        ["port", "list"] => Command::PortList,
        ["plan"] | ["plan", "install"] => Command::Plan {
            uninstall: false,
            purge: false,
        },
        ["plan", "uninstall"] => Command::Plan {
            uninstall: true,
            purge,
        },
        ["pack", _, _] => Command::Pack {
            directory: PathBuf::from(words[1]),
            output: PathBuf::from(words[2]),
        },
        _ => return Err(format!("{} is not a command", words.join(" "))),
    };
    // An option that belongs to another command is a mistake, not
    // something to pass over.
    let is_port_add = matches!(command, Command::PortAdd { .. });
    if (com.is_some() || count.is_some()) && !is_port_add {
        return Err("--com and --count belong to port add".to_string());
    }
    let takes_purge = matches!(
        command,
        Command::Uninstall { .. }
            | Command::Plan {
                uninstall: true,
                ..
            }
    );
    if purge && !takes_purge {
        return Err("--purge belongs to uninstall".to_string());
    }
    Ok(Cli {
        command,
        quiet,
        trust_lab_certificate: trust,
        log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(line: &str) -> Result<Cli, String> {
        let arguments: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        parse(&arguments)
    }

    fn command(line: &str) -> Command {
        cli(line).unwrap_or_else(|e| panic!("{line}: {e}")).command
    }

    #[test]
    fn each_command_is_read() {
        assert_eq!(command(""), Command::Install);
        assert_eq!(command("install"), Command::Install);
        assert_eq!(command("upgrade"), Command::Install);
        assert_eq!(command("repair"), Command::Repair);
        assert_eq!(command("uninstall"), Command::Uninstall { purge: false });
        assert_eq!(
            command("uninstall --purge"),
            Command::Uninstall { purge: true }
        );
        assert_eq!(command("port list"), Command::PortList);
        assert_eq!(
            command("port add"),
            Command::PortAdd {
                com: None,
                count: 1
            }
        );
        assert_eq!(
            command("port add --com 12 --count 4"),
            Command::PortAdd {
                com: Some(12),
                count: 4
            }
        );
        assert_eq!(
            command("port add --com COM7"),
            Command::PortAdd {
                com: Some(7),
                count: 1
            }
        );
        assert_eq!(
            command("port remove com12"),
            Command::PortRemove {
                com: "COM12".to_string()
            }
        );
        assert_eq!(
            command("plan"),
            Command::Plan {
                uninstall: false,
                purge: false
            }
        );
        assert_eq!(
            command("plan uninstall --purge"),
            Command::Plan {
                uninstall: true,
                purge: true
            }
        );
        assert_eq!(
            command("pack stage out.exe"),
            Command::Pack {
                directory: PathBuf::from("stage"),
                output: PathBuf::from("out.exe")
            }
        );
        assert_eq!(command("--version"), Command::Version);
        assert_eq!(command("--help"), Command::Help);
        assert_eq!(command("/?"), Command::Help);
    }

    #[test]
    fn the_options_are_read_wherever_they_are() {
        let cli = cli("/quiet --log C:\\setup.log install --trust-lab-certificate").unwrap();
        assert_eq!(cli.command, Command::Install);
        assert!(cli.quiet && cli.trust_lab_certificate);
        assert_eq!(cli.log, Some(PathBuf::from("C:\\setup.log")));
        let cli = self::cli("uninstall /QUIET").unwrap();
        assert!(cli.quiet && !cli.trust_lab_certificate);
        assert_eq!(cli.log, None);
    }

    #[test]
    fn what_is_not_understood_is_said() {
        for line in [
            "instal",
            "port",
            "port add COM3",
            "port remove",
            "port remove 12",
            "port remove COM0",
            "port remove COM999",
            "port add --com 0",
            "port add --com 257",
            "port add --com",
            "port add --count 0",
            "port add --count many",
            "--log",
            "--frobnicate",
            "install --purge",
            "install --com 3",
            "port list --count 2",
            "pack only-one",
        ] {
            assert!(cli(line).is_err(), "{line} was accepted");
        }
    }

    #[test]
    fn only_what_changes_the_machine_needs_an_administrator() {
        for line in [
            "install",
            "repair",
            "uninstall",
            "port add",
            "port remove COM3",
        ] {
            assert!(command(line).needs_administrator(), "{line}");
        }
        for line in [
            "port list",
            "plan",
            "plan uninstall",
            "--version",
            "--help",
            "pack a b",
        ] {
            assert!(!command(line).needs_administrator(), "{line}");
        }
    }
}
