// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Showing an `RMD` message to the user.
//!
//! Every message is shown the same way, in a task dialog: the id in the
//! title, the text as the main instruction, and the cause and the
//! recovery in the part that expands.

use ctlproto::ErrorInfo;
use msg::Severity;
use win32::{Hwnd, TaskButtons, TaskDialog, TaskIcon, id, task_dialog};

/// The id of the command link that starts the manager again with
/// administrator rights.
pub const ID_ELEVATE: u16 = 900;

/// The icon that goes with a message: its severity in the catalogue. A
/// message the catalogue of this build does not have came from a newer
/// service as an error.
fn icon(error: &ErrorInfo) -> TaskIcon {
    let severity = msg::entries()
        .find(|entry| entry.id == error.msgid)
        .map(|entry| entry.severity);
    match severity {
        Some(Severity::Info) => TaskIcon::Information,
        Some(Severity::Warning) => TaskIcon::Warning,
        _ => TaskIcon::Error,
    }
}

/// The cause and the recovery, for the part that expands.
fn second_level(error: &ErrorInfo) -> String {
    let mut text = String::new();
    if !error.cause.is_empty() {
        text.push_str("Cause: ");
        text.push_str(&error.cause);
    }
    if !error.recovery.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("Recovery: ");
        text.push_str(&error.recovery);
    }
    text
}

/// The task dialog for a message.
pub fn dialog(error: &ErrorInfo) -> TaskDialog {
    TaskDialog {
        title: error.msgid.clone(),
        instruction: error.text.clone(),
        content: String::new(),
        expanded: second_level(error),
        icon: icon(error),
        buttons: TaskButtons::CLOSE,
        links: Vec::new(),
        default: Some(id::CLOSE),
    }
}

/// The task dialog for the message that the pipe does not admit this
/// user, with the way out: starting again as an administrator.
pub fn denied_dialog(error: &ErrorInfo) -> TaskDialog {
    TaskDialog {
        buttons: TaskButtons::CANCEL,
        links: vec![(
            ID_ELEVATE,
            "Restart as administrator\nThe manager closes and opens again with administrator \
             rights."
                .to_string(),
        )],
        default: Some(ID_ELEVATE),
        ..dialog(error)
    }
}

/// Shows a message and waits until the user has closed it.
pub fn show(owner: Option<Hwnd>, error: &ErrorInfo) {
    task_dialog(owner, &dialog(error));
}

#[cfg(test)]
mod tests {
    use msg::message;

    use super::*;

    #[test]
    fn a_message_is_its_id_its_text_and_the_rest_to_expand() {
        let error = ErrorInfo::from(message("RMD4001", &["stats.get", "10"]));
        let shown = dialog(&error);
        assert_eq!(shown.title, "RMD4001");
        assert_eq!(
            shown.instruction,
            "The service did not answer stats.get within 10 seconds."
        );
        assert!(shown.content.is_empty());
        assert!(shown.expanded.starts_with("Cause: The request was sent"));
        assert!(shown.expanded.contains("\n\nRecovery: Check that"));
        assert_eq!(shown.icon, TaskIcon::Error);
        assert_eq!(shown.buttons, TaskButtons::CLOSE);
        assert!(shown.links.is_empty());
    }

    #[test]
    fn the_icon_is_the_severity_of_the_message() {
        let info = ErrorInfo::from(message("RMD4010", &["rustmodem", "started"]));
        assert_eq!(dialog(&info).icon, TaskIcon::Information);
        let warning = ErrorInfo::from(message("RMD0024", &["profiles.toml", "no"]));
        assert_eq!(dialog(&warning).icon, TaskIcon::Warning);
        // A message a newer service has and this build does not.
        let unknown = ErrorInfo {
            msgid: "RMD8123".to_string(),
            text: "Something new.".to_string(),
            cause: String::new(),
            recovery: String::new(),
        };
        let shown = dialog(&unknown);
        assert_eq!(shown.icon, TaskIcon::Error);
        assert!(shown.expanded.is_empty());
    }

    #[test]
    fn access_denied_offers_to_start_again_as_an_administrator() {
        let error = ErrorInfo::from(message(
            "RMD2008",
            &["rustmodem-control", "Access is denied. (os error 5)"],
        ));
        let shown = denied_dialog(&error);
        assert_eq!(shown.title, "RMD2008");
        assert!(shown.instruction.contains("Access is denied"));
        assert_eq!(shown.buttons, TaskButtons::CANCEL);
        assert_eq!(shown.links.len(), 1);
        assert_eq!(shown.links[0].0, ID_ELEVATE);
        assert!(shown.links[0].1.starts_with("Restart as administrator\n"));
        assert_eq!(shown.default, Some(ID_ELEVATE));
    }
}
