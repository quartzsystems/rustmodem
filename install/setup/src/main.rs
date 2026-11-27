// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `rustmodem-setup.exe`. The library has what it does; this is the
//! program around it: the command line, becoming an administrator, the
//! questions, and saying how it went.

#![forbid(unsafe_code)]

#[cfg(not(windows))]
fn main() {
    eprintln!("rustmodem-setup is a program for Windows");
    std::process::exit(setup::exit::UNSUPPORTED);
}

#[cfg(windows)]
fn main() {
    std::process::exit(program::run());
}

#[cfg(windows)]
mod program {
    use std::path::{Path, PathBuf};

    use msg::{Message, Severity, message};
    use setup::cli::{self, Cli, Command};
    use setup::payload::{self, Damage, Entry};
    use setup::plan::{self, Found, System};
    use setup::ports;
    use setup::windows::{LEAST_BUILD, Machine, SetupApi, windows_build};
    use setup::{PRODUCT, VERSION, exit, files};
    use win32::{TaskButtons, TaskDialog, TaskIcon, task_dialog};

    /// The button of a task dialog that says yes.
    const ID_YES: u16 = 6;

    pub fn run() -> i32 {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let cli = match cli::parse(&arguments) {
            Ok(cli) => cli,
            Err(detail) => {
                say(&message("RMD9512", &[&detail]), true);
                eprintln!("{}", cli::USAGE);
                return exit::USAGE;
            }
        };
        match &cli.command {
            Command::Help => {
                println!("{}", cli::USAGE);
                return exit::OK;
            }
            Command::Version => {
                println!("rustmodem-setup {VERSION}");
                return exit::OK;
            }
            Command::Pack { directory, output } => return pack(directory, output),
            _ => {}
        }
        if cli.command.needs_administrator() && !win32::is_elevated() {
            // Asked for once. The program that is then an administrator
            // does the work, and this one has done its part.
            if cli.quiet || win32::relaunch_elevated(None, &arguments).is_err() {
                say(&message("RMD9504", &[]), cli.quiet);
                return exit::NEEDS_ADMIN;
            }
            return exit::OK;
        }
        match cli.command.clone() {
            Command::Install => install(&cli, false),
            Command::Repair => install(&cli, true),
            Command::Uninstall { purge } => uninstall(&cli, purge),
            Command::Plan { uninstall, purge } => print_plan(uninstall, purge),
            Command::PortList => port_list(),
            Command::PortAdd { com, count } => match ports::add(&mut SetupApi, com, count) {
                Ok(names) => {
                    for name in names {
                        say(&message("RMD9004", &[&name]), true);
                    }
                    exit::OK
                }
                Err(failure) => {
                    say(&failure, true);
                    exit::FAILED
                }
            },
            Command::PortRemove { com } => match ports::remove(&mut SetupApi, &com) {
                Ok(()) => {
                    say(&message("RMD9005", &[&com]), true);
                    exit::OK
                }
                Err(failure) => {
                    say(&failure, true);
                    exit::FAILED
                }
            },
            Command::Help | Command::Version | Command::Pack { .. } => exit::OK,
        }
    }

    /// Says a message: on the console always, and in a dialog unless the
    /// run is quiet. The dialog is the one the manager shows a message
    /// in: the id in the title, the text as the instruction, the cause
    /// and the recovery in the part that expands.
    fn say(message: &Message, quiet: bool) {
        if message.severity == Severity::Info {
            println!("{message}");
        } else {
            eprintln!("{message}");
            eprintln!("Cause: {}", message.cause);
            eprintln!("Recovery: {}", message.recovery);
        }
        if quiet {
            return;
        }
        task_dialog(
            None,
            &TaskDialog {
                title: format!("{PRODUCT} Setup - {}", message.id),
                instruction: message.text.clone(),
                expanded: format!("{}\n\n{}", message.cause, message.recovery),
                icon: match message.severity {
                    Severity::Info => TaskIcon::Information,
                    Severity::Warning => TaskIcon::Warning,
                    Severity::Error => TaskIcon::Error,
                },
                buttons: TaskButtons::OK,
                ..TaskDialog::default()
            },
        );
    }

    /// Asks a question with Yes and No. A quiet run is not asked and the
    /// answer is yes: what a quiet run may not assume is checked before.
    fn ask(quiet: bool, instruction: &str, content: &str, icon: TaskIcon) -> bool {
        if quiet {
            return true;
        }
        let answer = task_dialog(
            None,
            &TaskDialog {
                title: format!("{PRODUCT} Setup"),
                instruction: instruction.to_string(),
                content: content.to_string(),
                icon,
                buttons: TaskButtons::YES_NO,
                ..TaskDialog::default()
            },
        );
        answer == ID_YES
    }

    /// This program's own file, and the files it carries.
    fn own_payload() -> Result<(Vec<u8>, Vec<Entry>), Damage> {
        let image = std::env::current_exe()
            .and_then(std::fs::read)
            .map_err(|_| Damage::Missing)?;
        let entries = payload::unpack(payload::payload_of(&image)?)?;
        Ok((image, entries))
    }

    fn log_path(cli: &Cli, machine_logs: &Path, purge: bool) -> PathBuf {
        match &cli.log {
            Some(path) => path.clone(),
            // A purge removes the logs directory, and the log of the run
            // that did it with it.
            None if purge => std::env::temp_dir().join("rustmodem-setup.log"),
            None => machine_logs.join("setup.log"),
        }
    }

    /// The parts of a version, for telling newer from older.
    fn version_parts(version: &str) -> Vec<u32> {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }

    fn install(cli: &Cli, repair: bool) -> i32 {
        let build = windows_build();
        if build < LEAST_BUILD {
            say(&message("RMD9505", &[&build.to_string()]), cli.quiet);
            return exit::UNSUPPORTED;
        }
        let (image, entries) = match own_payload() {
            Ok(own) => own,
            Err(damage) => {
                say(&message("RMD9506", &[&damage.to_string()]), cli.quiet);
                return exit::DAMAGED;
            }
        };
        let lab = entries
            .iter()
            .any(|entry| entry.name == files::LAB_CERTIFICATE);
        let logs = setup::windows::layout().logs_dir();
        let mut machine = Machine::new(entries, image, Some(&log_path(cli, &logs, false)));
        let found = machine.find();
        machine.log(&format!("rustmodem-setup {VERSION}: found {found:?}"));

        if repair && found.installed.is_none() {
            say(&message("RMD9508", &[]), cli.quiet);
            return exit::NOT_INSTALLED;
        }
        if let Some(installed) = &found.installed
            && version_parts(installed) > version_parts(VERSION)
        {
            say(&message("RMD9510", &[installed, VERSION]), cli.quiet);
            return exit::FAILED;
        }

        // The certificate is said plainly and asked about, by itself.
        if lab && !found.certificate_trusted {
            let agreed = cli.trust_lab_certificate
                || (!cli.quiet
                    && ask(
                        false,
                        "Trust the RustModem lab certificate on this computer?",
                        "This is a lab build of RustModem. Its driver is signed with a certificate made for testing, not one issued by a certificate authority.\n\n\
                         To install the driver, Setup will add that certificate to this computer's Trusted Root Certification Authorities and Trusted Publishers stores. Anything signed with the certificate's private key will then be trusted by this computer.\n\n\
                         Uninstalling RustModem removes the certificate. Do this only on a machine you test on.",
                        TaskIcon::Shield,
                    ));
            if !agreed {
                say(&message("RMD9509", &[]), cli.quiet);
                return exit::CANCELLED;
            }
        }
        let question = match (&found.installed, repair) {
            (Some(_), true) => format!("Repair {PRODUCT} {VERSION}?"),
            (Some(installed), false) => format!("Upgrade {PRODUCT} from {installed} to {VERSION}?"),
            (None, _) => format!("Install {PRODUCT} {VERSION}?"),
        };
        let detail = format!(
            "Setup will install the programs in {}, the driver of the RustModem ports, and the RustModem service, which will run under its own account. It will add Windows Firewall rules for SIP and RTP to the service, and a QoS policy that marks its RTP.\n\nThe configuration, the logs and the ports that are there are kept.",
            machine.layout.install_dir.display()
        );
        if !ask(cli.quiet, &question, &detail, TaskIcon::Shield) {
            say(&message("RMD9514", &[]), true);
            return exit::CANCELLED;
        }

        let mut names: Vec<String> = machine.payload.iter().map(|e| e.name.clone()).collect();
        // This program itself is the one file its payload cannot carry.
        names.push(files::SETUP.to_string());
        let layout = machine.layout.clone();
        let plan = plan::install(&found, &layout, &names, lab);
        match plan::carry_out(&plan, &mut machine) {
            Ok(()) => {
                let done = match (&found.installed, repair) {
                    (Some(_), true) => message("RMD9502", &[VERSION]),
                    (Some(installed), false) if installed != VERSION => {
                        message("RMD9501", &[installed, VERSION])
                    }
                    _ => message("RMD9500", &[VERSION]),
                };
                machine.log(&done.to_string());
                say(&done, cli.quiet);
                exit::OK
            }
            Err(failure) => {
                machine.log(&failure.to_string());
                say(&failure, cli.quiet);
                exit::FAILED
            }
        }
    }

    fn uninstall(cli: &Cli, purge: bool) -> i32 {
        let logs = setup::windows::layout().logs_dir();
        let mut machine = Machine::new(Vec::new(), Vec::new(), Some(&log_path(cli, &logs, purge)));
        let mut found = machine.find();
        if found.installed.is_none() && !found.install_dir && !found.service {
            say(&message("RMD9508", &[]), cli.quiet);
            return exit::NOT_INSTALLED;
        }
        let content = if purge {
            "The ports, the driver, the service, the firewall rules, the QoS policy and the programs will be removed, and the configuration and the logs with them."
        } else {
            "The ports, the driver, the service, the firewall rules, the QoS policy and the programs will be removed. The configuration and the logs are kept."
        };
        if !ask(
            cli.quiet,
            &format!("Remove {PRODUCT}?"),
            content,
            TaskIcon::Warning,
        ) {
            say(&message("RMD9514", &[]), true);
            return exit::CANCELLED;
        }
        machine.log(&format!(
            "rustmodem-setup {VERSION}: uninstall, found {found:?}"
        ));

        // A program cannot remove itself while it runs: its own file is
        // left out, and removed by Windows once this program has ended.
        let own = std::env::current_exe().ok();
        let in_install_dir = own
            .as_ref()
            .is_some_and(|path| path.starts_with(&machine.layout.install_dir));
        if in_install_dir {
            found
                .files
                .retain(|name| !name.eq_ignore_ascii_case(files::SETUP));
        }
        let layout = machine.layout.clone();
        let plan = plan::uninstall(&found, &layout, found.certificate_trusted, purge);
        let failures = plan::carry_out_all(&plan, &mut machine);
        if in_install_dir && let Some(own) = &own {
            setup::windows::remove_after_exit(own, &layout.install_dir);
        }
        for failure in &failures {
            say(failure, true);
        }
        if failures.is_empty() {
            say(&message("RMD9503", &[]), cli.quiet);
            exit::OK
        } else {
            exit::FAILED
        }
    }

    fn print_plan(uninstall: bool, purge: bool) -> i32 {
        let entries = own_payload()
            .map(|(_, entries)| entries)
            .unwrap_or_default();
        let lab = entries
            .iter()
            .any(|entry| entry.name == files::LAB_CERTIFICATE);
        let mut names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        names.push(files::SETUP.to_string());
        let mut machine = Machine::new(Vec::new(), Vec::new(), None);
        let found: Found = machine.find();
        let layout = machine.layout.clone();
        if uninstall {
            for step in plan::uninstall(&found, &layout, found.certificate_trusted, purge) {
                println!("{step:?}");
            }
        } else {
            for planned in plan::install(&found, &layout, &names, lab) {
                match planned.undo {
                    Some(undo) => println!("{:?}    (undone by {undo:?})", planned.step),
                    None => println!("{:?}", planned.step),
                }
            }
        }
        exit::OK
    }

    fn port_list() -> i32 {
        match ports::list(&mut SetupApi) {
            Ok(listed) => {
                // One port a line, its fields between tabs: the manager
                // and rustmodem-ctl read this.
                for port in listed {
                    println!(
                        "{}\t{}\t{}\t{}",
                        port.com,
                        port.modem_name.as_deref().unwrap_or(""),
                        port.driver_version.as_deref().unwrap_or(""),
                        if port.started { "started" } else { "stopped" },
                    );
                }
                exit::OK
            }
            Err(failure) => {
                say(&failure, true);
                exit::FAILED
            }
        }
    }

    /// Makes a setup program: this one, without what it carries, and the
    /// files of a directory.
    fn pack(directory: &Path, output: &Path) -> i32 {
        fn gather(directory: &Path, prefix: &str, entries: &mut Vec<Entry>) -> std::io::Result<()> {
            let mut children: Vec<_> = std::fs::read_dir(directory)?.collect::<Result<_, _>>()?;
            children.sort_by_key(|child| child.file_name());
            for child in children {
                let name = format!("{prefix}{}", child.file_name().to_string_lossy());
                if child.path().is_dir() {
                    gather(&child.path(), &format!("{name}\\"), entries)?;
                } else {
                    entries.push(Entry {
                        name,
                        data: std::fs::read(child.path())?,
                    });
                }
            }
            Ok(())
        }
        let made = (|| -> std::io::Result<usize> {
            let image = std::fs::read(std::env::current_exe()?)?;
            let mut entries = Vec::new();
            gather(directory, "", &mut entries)?;
            let file = payload::assemble(payload::stub_of(&image), &payload::pack(&entries));
            std::fs::write(output, file)?;
            Ok(entries.len())
        })();
        match made {
            Ok(count) => {
                println!("{}: {count} files", output.display());
                exit::OK
            }
            Err(error) => {
                eprintln!("{}: {error}", output.display());
                exit::FAILED
            }
        }
    }
}
