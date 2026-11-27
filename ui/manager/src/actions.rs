// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the manager does outside the service's control API: writing
//! exports to files, and starting, stopping, installing and removing the
//! Windows service.
//!
//! The service commands need administrator rights. A manager that does
//! not have them starts itself again with them, giving the command as an
//! argument, and closes; the new copy carries the command out.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use ctlproto::{ErrorInfo, Settings};
use model::Command;
use msg::message;
use win32::{
    FileFilter, Hwnd, ServiceAction, TaskButtons, TaskDialog, TaskIcon, id, is_elevated,
    service_control,
};

use crate::message as messages;
use crate::window::Manager;

/// The name of the Windows service.
pub const SERVICE: &str = "rustmodem";

/// The service's program, which is installed beside the manager.
const SERVICE_PROGRAM: &str = "rustmodem-svc.exe";

/// The ids of the command links that follow a change of settings.
const ID_RESTART_NOW: u16 = 901;
const ID_RESTART_LATER: u16 = 902;

/// The mark that says a text file is UTF-8, for the programs that would
/// otherwise guess: a spreadsheet opening a CSV file.
const BYTE_ORDER_MARK: &str = "\u{FEFF}";

/// The process creation flag that gives a console program no window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// --------------------------------------------------------------------------
// Exports
// --------------------------------------------------------------------------

/// Writes `text` to the file the user chooses. A file that cannot be
/// written is message `RMD4002`.
pub(crate) fn export(
    manager: &Manager,
    owner: Hwnd,
    title: &str,
    file_name: &str,
    extension: &str,
    kind: &str,
    text: &str,
) {
    let filters = [
        FileFilter::new(kind, &format!("*.{extension}")),
        FileFilter::new("All files", "*.*"),
    ];
    let Some(path) = manager.save_path(owner, title, file_name, extension, &filters) else {
        return;
    };
    if let Err(e) = std::fs::write(&path, text) {
        let error = ErrorInfo::from(message(
            "RMD4002",
            &[&path.display().to_string(), &e.to_string()],
        ));
        manager.show_error(owner, &error);
    }
}

fn csv(text: &str) -> String {
    format!("{BYTE_ORDER_MARK}{text}")
}

/// Export statistics: the statistics and the connect rates, as CSV.
pub fn export_statistics(manager: &Manager, owner: Hwnd) {
    let clock = manager.clock();
    let text = manager.with_model(|model| model.stats_view(clock.now_ms(), &clock).csv());
    export(
        manager,
        owner,
        "Export statistics",
        "rustmodem-statistics.csv",
        "csv",
        "CSV files",
        &csv(&text),
    );
}

/// Export event log: what the log pane holds, as text.
pub fn export_log(manager: &Manager, owner: Hwnd) {
    let clock = manager.clock();
    let text = manager.with_model(|model| model.log.text(&clock));
    export(
        manager,
        owner,
        "Export event log",
        "rustmodem-events.txt",
        "txt",
        "Text files",
        &text,
    );
}

/// Export the call history, as CSV.
pub fn export_calls(manager: &Manager, owner: Hwnd) {
    let clock = manager.clock();
    let text = manager.with_model(|model| model.calls.csv(&clock));
    export(
        manager,
        owner,
        "Export call history",
        "rustmodem-calls.csv",
        "csv",
        "CSV files",
        &csv(&text),
    );
}

// --------------------------------------------------------------------------
// The Windows service
// --------------------------------------------------------------------------

/// Where the service's program is: beside the manager.
fn service_program() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|program| program.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join(SERVICE_PROGRAM)
}

/// What became of a run of the service's program: what it printed, and
/// whether it ended without an error.
type ProgramRun = std::io::Result<(bool, String)>;

/// Runs the service's program with one argument and waits for it.
fn run_service_program(program: &Path, argument: &str) -> ProgramRun {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new(program)
        .arg(argument)
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    let mut said = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let errors = String::from_utf8_lossy(&output.stderr);
    let errors = errors.trim();
    if !errors.is_empty() {
        if !said.is_empty() {
            said.push('\n');
        }
        said.push_str(errors);
    }
    Ok((output.status.success(), said))
}

/// The message for what a run of the service's program came to.
fn program_report(program: &Path, run: ProgramRun) -> ErrorInfo {
    let name = program.display().to_string();
    let report = match run {
        Ok((true, said)) if said.is_empty() => message("RMD4013", &["Done."]),
        Ok((true, said)) => message("RMD4013", &[&said]),
        Ok((false, said)) if said.is_empty() => message("RMD4012", &[&name, "no reason given"]),
        Ok((false, said)) => message("RMD4012", &[&name, &said]),
        Err(e) => message("RMD4005", &[&name, &e.to_string()]),
    };
    ErrorInfo::from(report)
}

/// What a command does to the service, as the messages say it.
fn done_word(command: Command) -> &'static str {
    match command {
        Command::ServiceStart => "started",
        Command::ServiceStop => "stopped",
        Command::ServiceRestart => "restarted",
        Command::ServiceInstall => "installed",
        _ => "removed",
    }
}

/// The message for what became of starting, stopping or restarting the
/// service.
fn control_report(command: Command, outcome: Option<std::io::Result<()>>) -> ErrorInfo {
    let word = done_word(command);
    let report = match outcome {
        Some(Ok(())) => message("RMD4010", &[SERVICE, word]),
        Some(Err(e)) => message("RMD4003", &[SERVICE, word, &e.to_string()]),
        None => message("RMD4003", &[SERVICE, word, "the request did not finish"]),
    };
    ErrorInfo::from(report)
}

/// Asks before the service is installed.
fn confirm_install(manager: &Manager, window: Hwnd, program: &Path) -> bool {
    let dialog = TaskDialog {
        title: "Install service".to_string(),
        instruction: "Install RustModem as a Windows service?".to_string(),
        content: format!(
            "This runs {} --install. The service is registered to start with Windows and reads \
             its configuration from its default place.",
            program.display()
        ),
        expanded: String::new(),
        icon: if is_elevated() {
            TaskIcon::None
        } else {
            TaskIcon::Shield
        },
        buttons: TaskButtons::YES_NO,
        links: Vec::new(),
        default: Some(id::YES),
    };
    manager.task(Some(window), &dialog) == id::YES
}

/// Carries out a service command: start, stop, restart, install or
/// uninstall.
///
/// `from_start` says the command came on the command line, from a
/// manager that started this one to carry it out: the user has already
/// been asked whatever there was to ask, and this manager does not start
/// another.
pub fn service_command(manager: &Rc<Manager>, window: Hwnd, command: Command, from_start: bool) {
    let program = service_program();
    if command == Command::ServiceInstall
        && !from_start
        && !confirm_install(manager, window, &program)
    {
        return;
    }
    if !is_elevated() && !from_start {
        manager.restart_elevated(window, Some(command));
        return;
    }
    let report = match command {
        Command::ServiceStart | Command::ServiceStop | Command::ServiceRestart => {
            let action = match command {
                Command::ServiceStart => ServiceAction::Start,
                Command::ServiceStop => ServiceAction::Stop,
                _ => ServiceAction::Restart,
            };
            // The service control manager takes its time; the window
            // goes on in the meantime.
            let outcome = manager.wait_for(window, move || service_control(SERVICE, action));
            control_report(command, outcome)
        }
        Command::ServiceInstall | Command::ServiceUninstall => {
            let argument = if command == Command::ServiceInstall {
                "--install"
            } else {
                "--uninstall"
            };
            let run = {
                let program = program.clone();
                manager.wait_for(window, move || run_service_program(&program, argument))
            };
            match run {
                Some(run) => program_report(&program, run),
                None => program_report(
                    &program,
                    Err(std::io::Error::other("the program did not finish")),
                ),
            }
        }
        _ => return,
    };
    manager.show_error(window, &report);
}

/// After a change of settings that needs the service started again:
/// offers to do it now.
pub fn offer_restart(manager: &Rc<Manager>, window: Hwnd, settings: &Settings) {
    let info = ErrorInfo::from(message("RMD4011", &[&settings.path]));
    let dialog = TaskDialog {
        buttons: TaskButtons::default(),
        links: vec![
            (
                ID_RESTART_NOW,
                "Restart service now\nEvery call is hung up.".to_string(),
            ),
            (
                ID_RESTART_LATER,
                "Later\nThe service goes on with the settings it started with.".to_string(),
            ),
        ],
        default: Some(ID_RESTART_LATER),
        ..messages::dialog(&info)
    };
    if manager.task(Some(window), &dialog) == ID_RESTART_NOW {
        service_command(manager, window, Command::ServiceRestart, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_program_is_beside_the_manager() {
        let program = service_program();
        assert_eq!(
            program.file_name().and_then(|name| name.to_str()),
            Some("rustmodem-svc.exe")
        );
        let here = std::env::current_exe().unwrap();
        assert_eq!(program.parent(), here.parent());
    }

    #[test]
    fn what_the_program_says_is_what_the_user_is_told() {
        let program = Path::new(r"C:\rustmodem\rustmodem-svc.exe");
        let fine = program_report(
            program,
            Ok((true, "RMD0004: Service rustmodem installed.".to_string())),
        );
        assert_eq!(fine.msgid, "RMD4013");
        assert_eq!(fine.text, "RMD0004: Service rustmodem installed.");
        assert_eq!(
            program_report(program, Ok((true, String::new()))).text,
            "Done."
        );

        let failed = program_report(
            program,
            Ok((
                false,
                "RMD0006: Service rustmodem could not be installed: no".to_string(),
            )),
        );
        assert_eq!(failed.msgid, "RMD4012");
        assert!(failed.text.contains("RMD0006"), "{}", failed.text);
        assert!(
            failed.cause.contains("rustmodem-svc.exe"),
            "{}",
            failed.cause
        );

        let silent = program_report(program, Ok((false, String::new())));
        assert_eq!(silent.msgid, "RMD4012");

        let missing = program_report(
            program,
            Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        );
        assert_eq!(missing.msgid, "RMD4005");
        assert!(
            missing.text.contains("rustmodem-svc.exe"),
            "{}",
            missing.text
        );
    }

    #[test]
    fn a_program_that_is_not_there_cannot_be_run() {
        let nowhere = Path::new(r"C:\no such folder\rustmodem-svc.exe");
        assert!(run_service_program(nowhere, "--install").is_err());
    }

    #[test]
    fn the_service_is_reported_by_name_and_by_what_was_done() {
        let started = control_report(Command::ServiceStart, Some(Ok(())));
        assert_eq!(started.msgid, "RMD4010");
        assert_eq!(started.text, "Service rustmodem was started.");
        let refused = control_report(
            Command::ServiceStop,
            Some(Err(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            ))),
        );
        assert_eq!(refused.msgid, "RMD4003");
        assert!(
            refused
                .text
                .starts_with("Service rustmodem could not be stopped: "),
            "{}",
            refused.text
        );
        let lost = control_report(Command::ServiceRestart, None);
        assert_eq!(lost.msgid, "RMD4003");
        assert!(lost.text.contains("restarted"), "{}", lost.text);
    }

    #[test]
    fn a_csv_file_says_that_it_is_utf_8() {
        let text = csv("Statistic,Value\r\n");
        assert!(text.starts_with('\u{FEFF}'));
        assert!(text.ends_with("Statistic,Value\r\n"));
    }
}
