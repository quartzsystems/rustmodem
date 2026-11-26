// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The Windows Event Log: where the service reports that it started, that
//! it stopped, and anything that stopped it.
//!
//! Events are written to the Application log under the source `rustmodem`.
//! BLOCKED(phase5): the installer registers a message file for the source.
//! Until then the Event Viewer shows each event's text after a note that
//! the description for the event id cannot be found.

use msg::{Message, Severity};
use windows::Win32::System::EventLog::{
    DeregisterEventSource, EVENTLOG_ERROR_TYPE, EVENTLOG_INFORMATION_TYPE, EVENTLOG_WARNING_TYPE,
    RegisterEventSourceW, ReportEventW,
};
use windows::core::{HSTRING, PCWSTR};

use crate::SERVICE_NAME;

/// Writes an `RMD` message to the Application event log. The event id is
/// the number of the message id. Failure to write is ignored: there is
/// nowhere left to report it.
pub fn report(message: &Message) {
    let kind = match message.severity {
        Severity::Info => EVENTLOG_INFORMATION_TYPE,
        Severity::Warning => EVENTLOG_WARNING_TYPE,
        Severity::Error => EVENTLOG_ERROR_TYPE,
    };
    let event_id: u32 = message
        .id
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0);
    let source = HSTRING::from(SERVICE_NAME);
    let text = HSTRING::from(format!(
        "{message}\r\n\r\nCause: {}\r\nRecovery: {}",
        message.cause, message.recovery
    ));
    // SAFETY: `source` and `text` are NUL-terminated wide strings that live
    // until after the calls; the handle returned by RegisterEventSourceW is
    // used only here and is closed by DeregisterEventSource.
    unsafe {
        let Ok(log) = RegisterEventSourceW(PCWSTR::null(), &source) else {
            return;
        };
        let strings = [PCWSTR(text.as_ptr())];
        let _ = ReportEventW(log, kind, 0, event_id, None, 0, Some(&strings), None);
        let _ = DeregisterEventSource(log);
    }
}
