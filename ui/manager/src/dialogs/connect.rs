// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Connect to service: the name of the control pipe.

use std::cell::RefCell;
use std::rc::Rc;

use win32::{DialogTemplate, Event, Handler, Hwnd, Reply, id};

use super::{must_not_be_empty, watch, watched};
use crate::window::Manager;

const ID_COMPUTER: u16 = 300;
const ID_PIPE: u16 = 301;

/// What the computer field shows while the manager only manages the
/// computer it runs on.
const THIS_COMPUTER: &str = "(this computer)";

/// How a pipe is named in full on this computer.
const PIPE_PREFIX: &str = r"\\.\pipe\";

fn template() -> DialogTemplate {
    DialogTemplate::new("Connect to service", 230, 74)
        .label(7, 9, 60, 8, "&Computer:")
        .edit(ID_COMPUTER, 70, 7, 153, 14)
        .label(7, 27, 60, 8, "&Pipe name:")
        .edit(ID_PIPE, 70, 25, 153, 14)
        .ok_cancel()
}

/// The name of the pipe as the control API takes it, from what was
/// typed: without spaces around it, and without the prefix of a full
/// pipe path, which is how a name copied from elsewhere may come.
fn pipe_name(typed: &str) -> &str {
    let typed = typed.trim();
    match typed.get(..PIPE_PREFIX.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(PIPE_PREFIX) => &typed[PIPE_PREFIX.len()..],
        _ => typed,
    }
}

struct Connect {
    manager: Rc<Manager>,
    /// The pipe the manager is using now.
    pipe: String,
    chosen: RefCell<Option<String>>,
}

impl Handler for Connect {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let computer = dialog.item(ID_COMPUTER);
                computer.set_text(THIS_COMPUTER);
                // BLOCKED(phase9): managing the service of another
                // computer comes with remote management.
                computer.set_enabled(false);
                dialog.item(ID_PIPE).set_text(&self.pipe);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "connect") => Reply::Handled,
            Event::Command { id: id::OK, .. } => {
                let typed = dialog.item(ID_PIPE).text();
                let name = pipe_name(&typed);
                if name.is_empty() {
                    self.manager
                        .show_error(dialog, &must_not_be_empty("Pipe name"));
                    dialog.item(ID_PIPE).focus();
                } else {
                    *self.chosen.borrow_mut() = Some(name.to_string());
                    dialog.end_dialog(id::OK);
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Asks which pipe to connect to. Returns its name, or `None` if the
/// user thought better of it.
pub fn show(manager: &Rc<Manager>, owner: Hwnd, pipe: &str) -> Option<String> {
    let handler = Rc::new(Connect {
        manager: Rc::clone(manager),
        pipe: pipe.to_string(),
        chosen: RefCell::new(None),
    });
    let ended = template().show_modal(Some(owner), handler.clone());
    let chosen = handler.chosen.borrow_mut().take();
    chosen.filter(|_| ended == id::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_is_named_by_its_name_alone() {
        assert_eq!(pipe_name("rustmodem-control"), "rustmodem-control");
        assert_eq!(pipe_name("  rustmodem-demo \t"), "rustmodem-demo");
        assert_eq!(pipe_name(r"\\.\pipe\rustmodem-demo"), "rustmodem-demo");
        assert_eq!(pipe_name(r"\\.\PIPE\Mine"), "Mine");
        assert_eq!(pipe_name(r"\\.\pipe\"), "");
        assert_eq!(pipe_name("   "), "");
        // Only the whole prefix is one.
        assert_eq!(pipe_name(r"\\.\pip"), r"\\.\pip");
    }

    #[test]
    fn the_dialog_has_its_fields_in_tab_order() {
        // Each label, which has no id of its own, is before its field.
        assert_eq!(
            template().ids()[..],
            [0xFFFF, ID_COMPUTER, 0xFFFF, ID_PIPE, id::OK, id::CANCEL]
        );
    }
}
