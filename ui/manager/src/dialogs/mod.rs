// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The dialogs of the manager.
//!
//! Each is a template in dialog units, built in memory, and a handler
//! that fills it in and reads it back. A dialog that needs the service
//! asks through [`crate::Manager::call`], which waits for the answer
//! while the window behind goes on taking events; a dialog never keeps a
//! request of its own going.

pub mod about;
pub mod capture;
pub mod connect;
pub mod console;
pub mod groups;
pub mod history;
pub mod line;
pub mod peers;
pub mod ports;
pub mod routes;
pub mod sheet;
pub mod telephony;
pub mod trace;

use ctlproto::ErrorInfo;
use msg::message;
use win32::Hwnd;

use crate::window::Manager;

/// The timer a dialog starts when a test wants to be told that it is up.
/// It fires once the dialog has been shown and has painted.
const HOOK_TIMER: usize = 0x7001;

/// How long after it is made a dialog is taken to be up, in milliseconds.
const HOOK_DELAY: u32 = 250;

/// For a dialog that has just been made: asks to be told when it is up,
/// if a test wants to know.
pub(crate) fn watch(manager: &Manager, dialog: Hwnd) {
    if manager.hooks().dialog.is_some() {
        dialog.set_timer(HOOK_TIMER, HOOK_DELAY);
    }
}

/// For a dialog's timer events. If the timer is the one [`watch`]
/// started, tells the test that the dialog `name` is up, in the window
/// `dialog`, and returns `true`. The test closes the dialog when it has
/// seen enough.
pub(crate) fn watched(manager: &Manager, dialog: Hwnd, timer: usize, name: &str) -> bool {
    if timer != HOOK_TIMER {
        return false;
    }
    dialog.kill_timer(HOOK_TIMER);
    if let Some(hook) = manager.hooks().dialog.clone() {
        hook(name, dialog);
    }
    true
}

/// The message for a field that must not be empty.
pub(crate) fn must_not_be_empty(field: &str) -> ErrorInfo {
    ErrorInfo::from(message("RMD4009", &[field]))
}

/// Reads a whole number from `low` to `high` out of a field. Anything
/// else is message `RMD4008` naming the field.
pub(crate) fn number_in(field: &str, text: &str, low: u32, high: u32) -> Result<u32, ErrorInfo> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|number| (low..=high).contains(number))
        .ok_or_else(|| {
            ErrorInfo::from(message(
                "RMD4008",
                &[field, &low.to_string(), &high.to_string()],
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_whole_and_in_its_range() {
        assert_eq!(number_in("Line id", " 12 ", 1, 999), Ok(12));
        assert_eq!(number_in("Line id", "1", 1, 999), Ok(1));
        assert_eq!(number_in("Line id", "999", 1, 999), Ok(999));
        for text in [
            "",
            "0",
            "1000",
            "-3",
            "1.5",
            "seven",
            "99999999999999999999",
        ] {
            let error = number_in("Line id", text, 1, 999).unwrap_err();
            assert_eq!(error.msgid, "RMD4008", "{text:?}");
            assert_eq!(error.text, "Line id must be a whole number from 1 to 999.");
        }
    }

    #[test]
    fn an_empty_field_is_named() {
        let error = must_not_be_empty("Pipe name");
        assert_eq!(error.msgid, "RMD4009");
        assert_eq!(error.text, "Pipe name must not be empty.");
    }
}
