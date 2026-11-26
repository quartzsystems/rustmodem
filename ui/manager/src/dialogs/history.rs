// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Call history: the calls that have ended, newest first.

use std::rc::Rc;

use model::CALL_COLUMNS;
use win32::metrics::scale;
use win32::{Align, Column, DialogTemplate, Event, Handler, Hwnd, ListView, Reply};

use super::{watch, watched};
use crate::actions;
use crate::window::Manager;

const ID_LIST: u16 = 300;
const ID_EXPORT: u16 = 301;

/// The timer that brings the list up to date while the dialog is open.
const REFRESH: usize = 1;

fn template() -> DialogTemplate {
    DialogTemplate::new("Call history", 480, 230)
        .list_view(ID_LIST, 7, 7, 466, 195)
        .button(ID_EXPORT, 7, 209, 60, 14, "&Export\u{2026}")
        .close_button()
}

struct History {
    manager: Rc<Manager>,
}

impl History {
    /// Brings the list up to date with the model, which goes on taking
    /// the calls that end while the dialog is open.
    fn show(&self, dialog: Hwnd) {
        let list = ListView::of(dialog.item(ID_LIST));
        let count = self.manager.with_model(|model| model.calls.len());
        if list.row_count() != count {
            list.set_row_count(count);
            // A new call is the first row, and every other moved down.
            list.redraw_visible();
        }
        dialog.item(ID_EXPORT).set_enabled(count > 0);
    }
}

impl Handler for History {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let dpi = dialog.dpi();
                let list = ListView::of(dialog.item(ID_LIST));
                let columns: Vec<Column> = CALL_COLUMNS
                    .iter()
                    .map(|(title, width, right)| {
                        let align = if *right { Align::Right } else { Align::Left };
                        Column::new(title, scale(*width, dpi), align)
                    })
                    .collect();
                list.set_columns(&columns);
                self.show(dialog);
                dialog.set_timer(REFRESH, 1000);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id: REFRESH } => {
                self.show(dialog);
                Reply::Handled
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "history") => Reply::Handled,
            Event::Destroy => {
                dialog.kill_timer(REFRESH);
                Reply::Default
            }
            Event::Command { id: ID_EXPORT, .. } => {
                actions::export_calls(&self.manager, dialog);
                Reply::Handled
            }
            Event::ListCell {
                id: ID_LIST,
                row,
                column,
                cell,
            } => {
                let clock = self.manager.clock();
                cell.text = self
                    .manager
                    .with_model(|model| model.calls.cell(row, column, &clock));
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Shows the ended calls.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    let handler = Rc::new(History {
        manager: Rc::clone(manager),
    });
    template().show_modal(Some(owner), handler);
}
