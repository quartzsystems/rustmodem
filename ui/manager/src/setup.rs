// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The setup program, as the manager uses it: to make a RustModem port and
//! to remove one.
//!
//! A port is two devices, and making a device takes an administrator. The
//! manager does not become one for it: it runs `rustmodem-setup.exe`,
//! which is installed beside it, as an administrator, and Windows asks the
//! person whether to allow that. The setup program has no window then, so
//! it writes what it would have said to a file the manager names, four
//! lines for each message: its id, its text, its cause and its recovery.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ctlproto::ErrorInfo;
use msg::message;

/// The setup program's file name.
pub const PROGRAM: &str = "rustmodem-setup.exe";

/// What Windows says when the person answers no to running a program as
/// an administrator: `ERROR_CANCELLED`.
const CANCELLED: i32 = 1223;

/// How a port command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It was done.
    Done,
    /// The person did not allow it. Nothing was done and there is nothing
    /// to say.
    Cancelled,
    /// It was not done, and this is why.
    Failed(ErrorInfo),
}

/// Where the setup program is, if it is there: beside the manager.
pub fn program() -> Option<PathBuf> {
    let beside = std::env::current_exe().ok()?.parent()?.join(PROGRAM);
    beside.is_file().then_some(beside)
}

/// The words of `port add` for one port.
pub fn add_words() -> Vec<String> {
    vec!["port".to_string(), "add".to_string()]
}

/// The words of `port remove` for a port.
pub fn remove_words(port: &str) -> Vec<String> {
    vec!["port".to_string(), "remove".to_string(), port.to_string()]
}

/// The messages in what the setup program wrote.
pub fn messages(log: &str) -> Vec<ErrorInfo> {
    let lines: Vec<&str> = log.lines().collect();
    lines
        .as_chunks::<4>()
        .0
        .iter()
        .map(|[msgid, text, cause, recovery]| ErrorInfo {
            msgid: msgid.to_string(),
            text: text.to_string(),
            cause: cause.to_string(),
            recovery: recovery.to_string(),
        })
        .collect()
}

/// What a run of the setup program came to: `ran` is its exit code, or
/// why it did not run, and `log` what it wrote.
pub fn outcome(ran: std::io::Result<u32>, log: &str) -> Outcome {
    let could_not = |detail: &str| Outcome::Failed(message("RMD9013", &[PROGRAM, detail]).into());
    match ran {
        Ok(0) => Outcome::Done,
        Ok(code) => match messages(log).pop() {
            // The last thing it said is what stopped it.
            Some(said) => Outcome::Failed(said),
            None => could_not(&format!(
                "it ended with exit code {code} and gave no reason"
            )),
        },
        Err(error) if error.raw_os_error() == Some(CANCELLED) => Outcome::Cancelled,
        Err(error) => could_not(&error.to_string()),
    }
}

/// Runs the setup program as an administrator with the words of a port
/// command, and waits for it. For a thread that may wait.
pub fn run(program: &Path, words: &[String]) -> Outcome {
    static RUNS: AtomicU32 = AtomicU32::new(0);
    let log = std::env::temp_dir().join(format!(
        "rustmodem-port-{}-{}.log",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&log);
    let mut arguments = vec!["--log".to_string(), log.display().to_string()];
    arguments.extend(words.iter().cloned());
    let ran = win32::run_elevated(None, program, &arguments);
    let said = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    outcome(ran, &said)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFUSED: &str = "RMD9004\nRustModem port COM10 was created, with its modem.\ncause\n\
                           recovery\nRMD9011\nThere is no free COM number.\nwhy\nwhat to do\n";

    #[test]
    fn the_words_are_the_setup_programs_commands() {
        assert_eq!(add_words(), ["port", "add"]);
        assert_eq!(remove_words("COM12"), ["port", "remove", "COM12"]);
    }

    #[test]
    fn what_the_setup_program_wrote_is_read_four_lines_at_a_time() {
        let said = messages(REFUSED);
        assert_eq!(said.len(), 2);
        assert_eq!(said[0].msgid, "RMD9004");
        assert_eq!(said[1].text, "There is no free COM number.");
        assert_eq!(said[1].cause, "why");
        assert_eq!(said[1].recovery, "what to do");
        // A message that was cut short is not one.
        assert_eq!(messages("RMD9004\ntext\n").len(), 0);
    }

    #[test]
    fn a_run_is_done_cancelled_or_failed_with_what_was_said() {
        assert_eq!(outcome(Ok(0), REFUSED), Outcome::Done);
        let Outcome::Failed(said) = outcome(Ok(1), REFUSED) else {
            panic!("it failed");
        };
        assert_eq!(said.msgid, "RMD9011");
        // Nothing written: the exit code is all there is.
        let Outcome::Failed(said) = outcome(Ok(3), "") else {
            panic!("it failed");
        };
        assert_eq!(said.msgid, "RMD9013");
        assert!(said.text.contains("exit code 3"), "{}", said.text);
        assert_eq!(
            outcome(Err(std::io::Error::from_raw_os_error(CANCELLED)), ""),
            Outcome::Cancelled
        );
        let Outcome::Failed(said) = outcome(Err(std::io::Error::from_raw_os_error(2)), "") else {
            panic!("it failed");
        };
        assert_eq!(said.msgid, "RMD9013");
        assert!(said.text.contains(PROGRAM), "{}", said.text);
    }
}
