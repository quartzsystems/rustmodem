// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Line diagnostics: the modem of a line's call, as it works.
//!
//! A window beside the main window. While it is open the manager asks the
//! service for the `line.diag` events of the line it shows, which the
//! service sends a few times a second while the line's call has a modem;
//! when it closes the manager asks for no more. Choosing the command
//! again with another line selected shows that line in the same window.
//!
//! Everything is a stock control but the scope, for which there is none:
//! it is the demodulator's output over the last moments, painted on a
//! bitmap and copied to the screen in one piece.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ctlproto::{LineDiag, ModemInfo};
use model::EMPTY;
use win32::controls::Progress;
use win32::{
    Canvas, Colour, DialogTemplate, Event, Handler, Hwnd, ListBox, ModelessDialog, Reply, id,
};

use super::{watch, watched};
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// Which line.
    pub const LINE: u16 = 300;
    /// The modem's state.
    pub const STATE: u16 = 301;
    /// The level received.
    pub const RX: u16 = 302;
    /// The level sent.
    pub const TX: u16 = 303;
    /// The carrier.
    pub const CARRIER: u16 = 304;
    /// The frequency error.
    pub const FREQUENCY: u16 = 305;
    /// The quality, as a bar.
    pub const QUALITY_BAR: u16 = 306;
    /// The quality, as a number.
    pub const QUALITY: u16 = 307;
    /// Framing errors.
    pub const FRAMING: u16 = 308;
    /// Retrains.
    pub const RETRAINS: u16 = 309;
    /// The changes of state.
    pub const HISTORY: u16 = 310;
    /// The scope.
    pub const SCOPE: u16 = 311;
    /// What the scope shows.
    pub const SCOPE_KIND: u16 = 312;
}

/// The timer that brings the window up to date with the model.
const POLL: usize = 1;

/// How often, in milliseconds: the service sends four a second.
const POLL_MS: u32 = 100;

/// The colour of the scope's trace: the blue of an idle line in the grid.
const TRACE: Colour = Colour::rgb(0, 84, 200);

fn template() -> DialogTemplate {
    let mut dialog = DialogTemplate::new("Line diagnostics", 300, 232)
        .label(7, 7, 70, 8, "Line:")
        .value(ids::LINE, 80, 7, 213, 8, "");
    let rows = [
        ("State:", ids::STATE),
        ("Receive level:", ids::RX),
        ("Transmit level:", ids::TX),
        ("Carrier:", ids::CARRIER),
        ("Frequency error:", ids::FREQUENCY),
    ];
    let mut y = 21;
    for (label, id) in rows {
        dialog = dialog.label(7, y, 70, 8, label).value(id, 80, y, 70, 8, "");
        y += 12;
    }
    dialog
        .label(7, y, 70, 8, "Quality:")
        .progress(ids::QUALITY_BAR, 80, y, 40, 8)
        .value(ids::QUALITY, 124, y, 26, 8, "")
        .label(7, y + 12, 70, 8, "Framing errors:")
        .value(ids::FRAMING, 80, y + 12, 70, 8, "")
        .label(7, y + 24, 70, 8, "Retrains:")
        .value(ids::RETRAINS, 80, y + 24, 70, 8, "")
        .label(158, 21, 135, 8, "&History:")
        .list_box(ids::HISTORY, 158, 31, 135, 86)
        .value(ids::SCOPE_KIND, 7, 123, 286, 8, "")
        .canvas(ids::SCOPE, 7, 134, 286, 70)
        .close_button()
}

/// A level in tenths of a dBm0 as the window shows it.
fn level(tenths: Option<i32>) -> String {
    match tenths {
        Some(tenths) => format!("{:.1} dBm0", f64::from(tenths) / 10.0),
        None => EMPTY.to_string(),
    }
}

/// What the window says of a line's diagnostics: the values, in the order
/// of [`VALUES`].
pub fn values(modem: Option<&ModemInfo>) -> [String; 7] {
    let Some(modem) = modem else {
        return std::array::from_fn(|_| EMPTY.to_string());
    };
    [
        modem.state.clone(),
        level(modem.rx_level_tenth_dbm0),
        level(modem.tx_level_tenth_dbm0),
        if modem.carrier { "Detected" } else { "None" }.to_string(),
        format!("{:+.1} Hz", f64::from(modem.frequency_error_mhz) / 1000.0),
        modem.framing_errors.to_string(),
        modem.retrains.to_string(),
    ]
}

/// The controls [`values`] fills, in its order.
pub const VALUES: [u16; 7] = [
    ids::STATE,
    ids::RX,
    ids::TX,
    ids::CARRIER,
    ids::FREQUENCY,
    ids::FRAMING,
    ids::RETRAINS,
];

/// The changes of state as the list shows them, the newest last.
pub fn history(diag: &LineDiag) -> Vec<String> {
    diag.history
        .iter()
        .map(|change| {
            format!(
                "{:>3}.{:03} s  {}",
                change.at_ms / 1000,
                change.at_ms % 1000,
                change.state
            )
        })
        .collect()
}

/// What the line over the scope says.
pub fn scope_title(diag: &LineDiag) -> String {
    match (diag.scope_kind.as_str(), diag.scope.is_empty()) {
        ("discriminator", false) => {
            "Demodulator output: mark at the top, space at the bottom".to_string()
        }
        _ => "Scope: nothing to show".to_string(),
    }
}

/// The points of the scope's trace on a canvas of a size: the samples
/// from left to right, 127 at the top and -127 at the bottom, with a
/// margin of two pixels.
pub fn scope_points(scope: &[i8], width: i32, height: i32) -> Vec<(i32, i32)> {
    const MARGIN: i32 = 2;
    if scope.len() < 2 || width <= 2 * MARGIN || height <= 2 * MARGIN {
        return Vec::new();
    }
    let (across, down) = (width - 1 - 2 * MARGIN, height - 1 - 2 * MARGIN);
    let last = scope.len() as i32 - 1;
    scope
        .iter()
        .enumerate()
        .map(|(n, sample)| {
            let x = MARGIN + n as i32 * across / last;
            let y = MARGIN + (127 - i32::from(*sample).clamp(-127, 127)) * down / 254;
            (x, y)
        })
        .collect()
}

struct Diagnostics {
    manager: Rc<Manager>,
    /// The line the window shows.
    line: Cell<u16>,
    /// What the window last showed.
    shown: RefCell<Option<LineDiag>>,
}

impl Diagnostics {
    /// The line's diagnostics as the model has them, if they are this
    /// line's.
    fn latest(&self) -> Option<LineDiag> {
        let line = self.line.get();
        self.manager
            .with_diag(|diag| diag.filter(|diag| diag.line == line).cloned())
            .flatten()
    }

    /// Says which line the window shows.
    fn title(&self, dialog: Hwnd) {
        let line = self.line.get();
        let name = self
            .manager
            .with_model(|model| model.lines.line(line).and_then(|line| line.name.clone()));
        dialog.item(ids::LINE).set_text(&match name {
            Some(name) => format!("{line} ({name})"),
            None => line.to_string(),
        });
    }

    /// Shows what the service last said, if it is new.
    fn poll(&self, dialog: Hwnd) {
        let latest = self.latest().unwrap_or(LineDiag {
            line: self.line.get(),
            scope_kind: "none".to_string(),
            ..LineDiag::default()
        });
        if self.shown.borrow().as_ref() == Some(&latest) {
            return;
        }
        for (control, text) in VALUES.iter().zip(values(latest.modem.as_ref())) {
            let control = dialog.item(*control);
            // A value that is set again flickers.
            if control.text() != text {
                control.set_text(&text);
            }
        }
        let quality = latest.modem.as_ref().map(|modem| modem.quality_percent);
        Progress::of(dialog.item(ids::QUALITY_BAR)).set_percent(quality.unwrap_or(0));
        dialog.item(ids::QUALITY).set_text(&match quality {
            Some(percent) => format!("{percent}%"),
            None => EMPTY.to_string(),
        });
        let lines = history(&latest);
        let unchanged = self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|shown| shown.history == latest.history);
        if !unchanged {
            let last = lines.len().checked_sub(1);
            ListBox::of(dialog.item(ids::HISTORY)).set_entries(&lines, last);
        }
        dialog.item(ids::SCOPE_KIND).set_text(&scope_title(&latest));
        *self.shown.borrow_mut() = Some(latest);
        dialog.item(ids::SCOPE).redraw();
    }

    fn paint(&self, canvas: &mut Canvas) {
        canvas.fill(Colour::window());
        let (width, height) = (canvas.width(), canvas.height());
        // Where neither tone is the stronger.
        canvas.line((0, height / 2), (width, height / 2), Colour::gray_text());
        if let Some(shown) = self.shown.borrow().as_ref() {
            canvas.polyline(&scope_points(&shown.scope, width, height), TRACE);
        }
    }

    /// Shows another line in the window.
    fn retarget(&self, dialog: Hwnd, line: u16) {
        self.line.set(line);
        *self.shown.borrow_mut() = None;
        self.manager.set_diagnostics(Some(line));
        self.title(dialog);
        self.poll(dialog);
    }
}

impl Handler for Diagnostics {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                self.manager.set_diagnostics(Some(self.line.get()));
                self.title(dialog);
                self.poll(dialog);
                dialog.set_timer(POLL, POLL_MS);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id: POLL } => {
                // The command was chosen again with another line selected.
                if let Some(line) = self.manager.diagnostics_line()
                    && line != self.line.get()
                {
                    self.retarget(dialog, line);
                }
                self.poll(dialog);
                Reply::Handled
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "diagnostics") => {
                Reply::Handled
            }
            Event::Draw {
                id: ids::SCOPE,
                canvas,
            } => {
                self.paint(canvas);
                Reply::Handled
            }
            // Close is the dialog's Cancel.
            Event::Command { id: id::CANCEL, .. } => Reply::Default,
            Event::Destroy => {
                dialog.kill_timer(POLL);
                self.manager.set_diagnostics(None);
                Reply::Default
            }
            _ => Reply::Default,
        }
    }
}

/// Opens the diagnostics of a line as a window beside the main window.
pub fn open(manager: &Rc<Manager>, owner: Hwnd, line: u16) -> Option<ModelessDialog> {
    let handler = Rc::new(Diagnostics {
        manager: Rc::clone(manager),
        line: Cell::new(line),
        shown: RefCell::new(None),
    });
    let dialog = template().create_modeless(Some(owner), handler)?;
    dialog.hwnd().set_visible(true);
    Some(dialog)
}

#[cfg(test)]
mod tests {
    use ctlproto::ModemStateChange;

    use super::*;

    fn modem() -> ModemInfo {
        ModemInfo {
            state: "data".to_string(),
            rx_level_tenth_dbm0: Some(-143),
            tx_level_tenth_dbm0: Some(-120),
            carrier: true,
            frequency_error_mhz: -1250,
            quality_percent: 71,
            framing_errors: 2,
            retrains: 0,
        }
    }

    #[test]
    fn the_values_are_laid_out_for_people() {
        assert_eq!(
            values(Some(&modem())),
            [
                "data",
                "-14.3 dBm0",
                "-12.0 dBm0",
                "Detected",
                "-1.2 Hz",
                "2",
                "0"
            ]
        );
        let silent = ModemInfo {
            rx_level_tenth_dbm0: None,
            carrier: false,
            frequency_error_mhz: 0,
            ..modem()
        };
        let shown = values(Some(&silent));
        assert_eq!((shown[1].as_str(), shown[3].as_str()), (EMPTY, "None"));
        assert_eq!(shown[4], "+0.0 Hz");
        // A line with no modem to show has a dash for everything.
        assert!(values(None).iter().all(|value| value == EMPTY));
        assert_eq!(VALUES.len(), values(None).len());
    }

    #[test]
    fn the_history_and_the_scope_say_what_they_are() {
        let diag = LineDiag {
            line: 1,
            modem: Some(modem()),
            history: vec![
                ModemStateChange {
                    state: "answer-tone".to_string(),
                    at_ms: 2150,
                },
                ModemStateChange {
                    state: "data".to_string(),
                    at_ms: 15_490,
                },
            ],
            scope_kind: "discriminator".to_string(),
            scope: vec![127, -127],
        };
        assert_eq!(
            history(&diag),
            ["  2.150 s  answer-tone", " 15.490 s  data"]
        );
        assert_eq!(
            scope_title(&diag),
            "Demodulator output: mark at the top, space at the bottom"
        );
        let none = LineDiag {
            line: 1,
            scope_kind: "none".to_string(),
            ..LineDiag::default()
        };
        assert_eq!(scope_title(&none), "Scope: nothing to show");
        assert!(history(&none).is_empty());
    }

    #[test]
    fn the_trace_runs_from_edge_to_edge_with_mark_at_the_top() {
        let points = scope_points(&[127, 0, -127, -128], 105, 45);
        assert_eq!(points.len(), 4);
        // From the left margin to the right, the top margin to the bottom.
        assert_eq!(points[0], (2, 2));
        assert_eq!(points[1], (2 + 100 / 3, 2 + 20));
        assert_eq!(points[2], (2 + 200 / 3, 42));
        // What is past the scale is at its end.
        assert_eq!(points[3], (102, 42));
        // One sample is no line, and a canvas with no room has none.
        assert!(scope_points(&[5], 100, 40).is_empty());
        assert!(scope_points(&[5, 6], 4, 40).is_empty());
        assert!(scope_points(&[], 100, 40).is_empty());
    }

    #[test]
    fn the_template_has_the_scope_and_every_value() {
        let ids = template().ids();
        for id in VALUES {
            assert!(ids.contains(&id));
        }
        for id in [
            ids::QUALITY_BAR,
            ids::HISTORY,
            ids::SCOPE,
            win32::id::CANCEL,
        ] {
            assert!(ids.contains(&id));
        }
    }
}
