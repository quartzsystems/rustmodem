// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! SIP trace: the SIP messages the service sends and receives, as they
//! pass.
//!
//! A window beside the main window. While it is open the manager asks the
//! service for `sip.trace` events, which the service sends to nobody who
//! has not asked for them by name; when it closes the manager asks for no
//! more. The model keeps the messages. Pause makes it drop what arrives,
//! so that the window stands still and can be read.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use model::trace_entry;
use win32::{
    Colour, DialogTemplate, Event, Font, Handler, Hwnd, ModelessDialog, Reply, RichEdit, id,
};

use super::{watch, watched};
use crate::actions;
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// The messages.
    pub const OUTPUT: u16 = 300;
    /// Pause, or Resume.
    pub const PAUSE: u16 = 301;
    /// Clear.
    pub const CLEAR: u16 = 302;
    /// Save.
    pub const SAVE: u16 = 303;
}

use ids::{CLEAR as ID_CLEAR, OUTPUT as ID_OUTPUT, PAUSE as ID_PAUSE, SAVE as ID_SAVE};

/// The timer that brings the window up to date with the model.
const POLL: usize = 1;

/// How often, in milliseconds.
const POLL_MS: u32 = 200;

/// The most lines the window holds: a message is a dozen or two.
const LINES: usize = 20_000;

/// The colour of the line that says a message was sent: the blue of an
/// idle line in the grid.
const SENT: Colour = Colour::rgb(0, 84, 200);

/// The colour of the line that says a message was received: the green
/// of a line that is online.
const RECEIVED: Colour = Colour::rgb(0, 128, 0);

fn template() -> DialogTemplate {
    DialogTemplate::new("SIP trace", 420, 260)
        .rich_edit(ID_OUTPUT, 7, 7, 406, 225)
        .button(ID_PAUSE, 7, 239, 50, 14, "&Pause")
        .button(ID_CLEAR, 61, 239, 50, 14, "&Clear")
        .button(ID_SAVE, 115, 239, 50, 14, "&Save\u{2026}")
        .close_button()
}

struct Trace {
    manager: Rc<Manager>,
    /// How many of the messages the model has taken the window shows.
    shown: Cell<u64>,
    font: RefCell<Option<Font>>,
}

impl Trace {
    fn output(dialog: Hwnd) -> RichEdit {
        RichEdit::of(dialog.item(ID_OUTPUT))
    }

    /// Adds the messages the model has and the window does not.
    fn poll(&self, dialog: Hwnd) {
        let clock = self.manager.clock();
        let shown = self.shown.get();
        let new = self.manager.with_trace(|trace| {
            let taken = trace.taken();
            // What came since the last look, as far as the buffer still
            // has it.
            let missing = (taken.saturating_sub(shown) as usize).min(trace.len());
            let lines: Vec<(String, Option<Colour>)> = trace
                .messages()
                .skip(trace.len() - missing)
                .flat_map(|message| {
                    let colour = match message.direction {
                        ctlproto::TraceDirection::Sent => SENT,
                        ctlproto::TraceDirection::Received => RECEIVED,
                    };
                    // The first line says when, which way and with whom,
                    // and is in the colour of the way.
                    trace_entry(message, &clock)
                        .lines()
                        .enumerate()
                        .map(|(index, line)| (line.to_string(), (index == 0).then_some(colour)))
                        .collect::<Vec<_>>()
                })
                .collect();
            (taken, lines)
        });
        let Some((taken, lines)) = new else {
            return;
        };
        if taken < shown {
            // The buffer was emptied by something else: so is the window.
            Self::output(dialog).clear();
        }
        if !lines.is_empty() {
            Self::output(dialog).append(&lines, LINES);
        }
        self.shown.set(taken);
        let any = self
            .manager
            .with_trace(|trace| !trace.is_empty())
            .unwrap_or(false);
        dialog.item(ID_SAVE).set_enabled(any);
        dialog.item(ID_CLEAR).set_enabled(any);
    }

    fn pause(&self, dialog: Hwnd) {
        let paused = self.manager.with_trace(|trace| {
            trace.set_paused(!trace.paused());
            trace.paused()
        });
        if let Some(paused) = paused {
            dialog
                .item(ID_PAUSE)
                .set_text(if paused { "&Resume" } else { "&Pause" });
        }
    }

    fn clear(&self, dialog: Hwnd) {
        self.manager.with_trace(|trace| trace.clear());
        Self::output(dialog).clear();
        self.shown.set(0);
        self.poll(dialog);
    }

    fn save(&self, dialog: Hwnd) {
        let clock = self.manager.clock();
        let Some(text) = self.manager.with_trace(|trace| trace.text(&clock)) else {
            return;
        };
        actions::export(
            &self.manager,
            dialog,
            "Save SIP trace",
            "rustmodem-sip-trace.txt",
            "txt",
            "Text files",
            // A text file of Windows has its line ends.
            &text.replace('\n', "\r\n"),
        );
    }
}

impl Handler for Trace {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                // The messages are in the log's fixed-pitch face.
                let font = Font::log(dialog.dpi());
                dialog.item(ID_OUTPUT).set_font(&font);
                *self.font.borrow_mut() = Some(font);
                // A trace that was left paused starts running again.
                self.manager.with_trace(|trace| trace.set_paused(false));
                self.manager.set_tracing(true);
                self.poll(dialog);
                dialog.set_timer(POLL, POLL_MS);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id: POLL } => {
                self.poll(dialog);
                Reply::Handled
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "sip-trace") => {
                Reply::Handled
            }
            Event::Command { id: ID_PAUSE, .. } => {
                self.pause(dialog);
                Reply::Handled
            }
            Event::Command { id: ID_CLEAR, .. } => {
                self.clear(dialog);
                Reply::Handled
            }
            Event::Command { id: ID_SAVE, .. } => {
                self.save(dialog);
                Reply::Handled
            }
            // Close is the dialog's Cancel.
            Event::Command { id: id::CANCEL, .. } => Reply::Default,
            Event::Destroy => {
                dialog.kill_timer(POLL);
                self.manager.set_tracing(false);
                Reply::Default
            }
            _ => Reply::Default,
        }
    }
}

/// Opens the SIP trace as a window beside the main window.
pub fn open(manager: &Rc<Manager>, owner: Hwnd) -> Option<ModelessDialog> {
    let handler = Rc::new(Trace {
        manager: Rc::clone(manager),
        shown: Cell::new(0),
        font: RefCell::new(None),
    });
    let dialog = template().create_modeless(Some(owner), handler)?;
    dialog.hwnd().set_visible(true);
    Some(dialog)
}
