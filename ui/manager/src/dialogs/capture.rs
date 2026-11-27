// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Call capture: which lines have their calls captured.
//!
//! A line with its box checked has every RTP packet of its calls written
//! to a file in the service's capture directory; `docs/CAPTURE.md` says
//! what is in one and how to check it. Checking and clearing a box takes
//! effect at once. The setting is the service's and is not kept: it is
//! off for every line when the service starts.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use ctlproto::{Call, CaptureSwitch, LineInfo};
use model::EMPTY;
use win32::metrics::scale;
use win32::{Align, CheckList, Column, DialogTemplate, Event, Handler, Hwnd, Reply};

use super::{watch, watched};
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// The list of lines.
    pub const LIST: u16 = 300;
    /// Open folder.
    pub const FOLDER: u16 = 301;
    /// Where the captures go.
    pub const WHERE: u16 = 302;
}

use ids::{FOLDER as ID_FOLDER, LIST as ID_LIST, WHERE as ID_WHERE};

/// The columns: their titles and their widths at 96 DPI.
const COLUMNS: [(&str, i32); 4] = [("Line", 70), ("Name", 150), ("Port", 90), ("SIP", 90)];

fn template() -> DialogTemplate {
    DialogTemplate::new("Call capture", 290, 214)
        .label(
            7,
            7,
            276,
            16,
            "Every RTP packet of the calls of a checked line is written to a file, one \
             for each call. Capture is off for every line when the service starts.",
        )
        .check_list(ID_LIST, 7, 28, 276, 138)
        .label(7, 172, 30, 8, "Folder:")
        .value(ID_WHERE, 38, 172, 245, 8, "")
        .button(ID_FOLDER, 7, 193, 60, 14, "&Open folder")
        .close_button()
}

/// What a row shows of a line.
pub fn row(line: &LineInfo) -> [String; 4] {
    [
        line.id.to_string(),
        line.name.clone().unwrap_or_else(|| EMPTY.to_string()),
        line.port.clone(),
        match line.sip.mode {
            ctlproto::SipMode::None => EMPTY.to_string(),
            ctlproto::SipMode::Trunk => "trunk".to_string(),
            ctlproto::SipMode::Registered => line
                .sip
                .extension
                .clone()
                .unwrap_or_else(|| EMPTY.to_string()),
        },
    ]
}

struct Capture {
    manager: Rc<Manager>,
    /// The lines, in the order of the rows.
    lines: Vec<LineInfo>,
    folder: Option<PathBuf>,
    /// A box is being set by the dialog itself: the service is not told.
    setting: Cell<bool>,
}

impl Capture {
    fn list(dialog: Hwnd) -> CheckList {
        CheckList::of(dialog.item(ID_LIST))
    }

    /// Sets a box without the service being told of it.
    fn set(&self, dialog: Hwnd, row: usize, checked: bool) {
        self.setting.set(true);
        Self::list(dialog).set_checked(row, checked);
        self.setting.set(false);
    }

    /// The user checked or cleared a box: the service is told, and the
    /// box shows what the service then says.
    fn switch(&self, dialog: Hwnd, row: usize, on: bool) {
        let Some(line) = self.lines.get(row) else {
            return;
        };
        let call = Call::LinesCapture(CaptureSwitch { id: line.id, on });
        match self.manager.call::<LineInfo>(dialog, call) {
            Ok(line) => {
                if line.capture != on {
                    self.set(dialog, row, line.capture);
                }
                self.manager.line_changed(line);
            }
            Err(error) => {
                // The box goes back to what it was.
                self.set(dialog, row, !on);
                self.manager.show_error(dialog, &error);
            }
        }
        // The service has answered: a test is told that the list shows it.
        watch(&self.manager, dialog);
    }
}

impl Handler for Capture {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let dpi = dialog.dpi();
                let list = Self::list(dialog);
                let columns: Vec<Column> = COLUMNS
                    .iter()
                    .map(|(title, width)| Column::new(title, scale(*width, dpi), Align::Left))
                    .collect();
                list.set_columns(&columns);
                self.setting.set(true);
                for line in &self.lines {
                    list.add_row(&row(line), line.capture);
                }
                self.setting.set(false);
                match &self.folder {
                    Some(folder) => dialog
                        .item(ID_WHERE)
                        .set_text(&folder.display().to_string()),
                    None => {
                        // A service with no SIP captures nothing.
                        dialog
                            .item(ID_WHERE)
                            .set_text("The service has no SIP: there are no calls to capture.");
                        dialog.item(ID_FOLDER).set_enabled(false);
                    }
                }
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "capture") => Reply::Handled,
            Event::ListCheck {
                id: ID_LIST,
                row,
                checked,
            } => {
                if !self.setting.get() {
                    self.switch(dialog, row, checked);
                }
                Reply::Handled
            }
            Event::Command { id: ID_FOLDER, .. } => {
                if let Some(folder) = &self.folder {
                    self.manager.open_folder(dialog, folder);
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Shows the lines with which of them are captured.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    let (lines, folder) = manager.with_model(|model| {
        (
            model.lines.lines().cloned().collect::<Vec<_>>(),
            model
                .service()
                .and_then(|service| service.capture_dir.clone())
                .map(PathBuf::from),
        )
    });
    let handler = Rc::new(Capture {
        manager: Rc::clone(manager),
        lines,
        folder,
        setting: Cell::new(false),
    });
    template().show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use ctlproto::{Counters, LineSip, LineState, SipMode};

    use super::*;

    #[test]
    fn a_row_is_the_line_its_name_its_port_and_how_it_is_reached() {
        let mut line = LineInfo {
            id: 3,
            name: Some("Front desk".to_string()),
            backend: "com0com".to_string(),
            port: "CNCB2".to_string(),
            enabled: true,
            state: LineState::Idle,
            auto_answer_rings: 0,
            unknown_command: "ok".to_string(),
            counters: Counters::default(),
            call: None,
            error: None,
            binding: None,
            engine: "null".to_string(),
            hunt_groups: Vec::new(),
            sip: LineSip {
                mode: SipMode::Registered,
                extension: Some("203".to_string()),
                ..LineSip::default()
            },
            capture: false,
            modem: ctlproto::LineModem::default(),
        };
        assert_eq!(row(&line), ["3", "Front desk", "CNCB2", "203"]);
        line.name = None;
        line.sip = LineSip {
            mode: SipMode::Trunk,
            ..LineSip::default()
        };
        assert_eq!(row(&line), ["3", EMPTY, "CNCB2", "trunk"]);
        line.sip = LineSip::default();
        assert_eq!(row(&line)[3], EMPTY);
        assert_eq!(COLUMNS.len(), row(&line).len());
    }
}
