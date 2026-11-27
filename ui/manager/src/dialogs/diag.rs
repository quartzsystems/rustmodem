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
//! Everything is a stock control but the scope, for which there is none.
//! At 300 bit/s it is the demodulator's output over the last moments; at
//! 1200 and 2400 bit/s it is the constellation, the last received points
//! over the ideal ones. Either is painted on a bitmap and copied to the
//! screen in one piece.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ctlproto::{LineDiag, LinkInfo, ModemInfo};
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
    /// The signal-to-noise ratio.
    pub const SNR: u16 = 313;
    /// The equaliser's error.
    pub const EQUALIZER: u16 = 314;
    /// The timing offset.
    pub const TIMING: u16 = 315;
    /// The data link: what it is.
    pub const LINK: u16 = 316;
    /// Its frames.
    pub const FRAMES: u16 = 317;
    /// Its errors.
    pub const LINK_ERRORS: u16 = 318;
    /// Its window.
    pub const WINDOW: u16 = 319;
    /// Its compression.
    pub const COMPRESSION: u16 = 320;
}

/// The timer that brings the window up to date with the model.
const POLL: usize = 1;

/// How often, in milliseconds: the service sends four a second.
const POLL_MS: u32 = 100;

/// The colour of the scope's trace: the blue of an idle line in the grid.
const TRACE: Colour = Colour::rgb(0, 84, 200);

fn template() -> DialogTemplate {
    let mut dialog = DialogTemplate::new("Line diagnostics", 300, 322)
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
        .label(7, y + 36, 70, 8, "Signal to noise:")
        .value(ids::SNR, 80, y + 36, 70, 8, "")
        .label(7, y + 48, 70, 8, "Equaliser error:")
        .value(ids::EQUALIZER, 80, y + 48, 70, 8, "")
        .label(7, y + 60, 70, 8, "Timing offset:")
        .value(ids::TIMING, 80, y + 60, 70, 8, "")
        .label(158, 21, 135, 8, "&History:")
        .list_box(ids::HISTORY, 158, 31, 135, 62)
        .label(158, 99, 50, 8, "Link:")
        .value(ids::LINK, 210, 99, 83, 8, "")
        .label(158, 111, 50, 8, "Frames:")
        .value(ids::FRAMES, 210, 111, 83, 8, "")
        .label(158, 123, 50, 8, "Errors:")
        .value(ids::LINK_ERRORS, 210, 123, 83, 8, "")
        .label(158, 135, 50, 8, "Window:")
        .value(ids::WINDOW, 210, 135, 83, 8, "")
        .label(158, 147, 50, 8, "Compression:")
        .value(ids::COMPRESSION, 210, 147, 83, 8, "")
        .value(ids::SCOPE_KIND, 7, 163, 286, 8, "")
        .canvas(ids::SCOPE, 7, 174, 286, 120)
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
pub fn values(modem: Option<&ModemInfo>) -> [String; 15] {
    let Some(modem) = modem else {
        return std::array::from_fn(|_| EMPTY.to_string());
    };
    let decibels = |tenths: Option<i32>| match tenths {
        Some(tenths) => format!("{:.1} dB", f64::from(tenths) / 10.0),
        None => EMPTY.to_string(),
    };
    let [link, frames, errors, window, compression] = link_values(modem.link.as_deref());
    [
        modem.state.clone(),
        level(modem.rx_level_tenth_dbm0),
        level(modem.tx_level_tenth_dbm0),
        if modem.carrier { "Detected" } else { "None" }.to_string(),
        format!("{:+.1} Hz", f64::from(modem.frequency_error_mhz) / 1000.0),
        modem.framing_errors.to_string(),
        modem.retrains.to_string(),
        decibels(modem.snr_tenth_db),
        decibels(modem.equalizer_error_tenth_db),
        match modem.timing_offset_ppm {
            Some(ppm) => format!("{ppm:+} ppm"),
            None => EMPTY.to_string(),
        },
        link,
        frames,
        errors,
        window,
        compression,
    ]
}

/// The controls [`values`] fills, in its order.
pub const VALUES: [u16; 15] = [
    ids::STATE,
    ids::RX,
    ids::TX,
    ids::CARRIER,
    ids::FREQUENCY,
    ids::FRAMING,
    ids::RETRAINS,
    ids::SNR,
    ids::EQUALIZER,
    ids::TIMING,
    ids::LINK,
    ids::FRAMES,
    ids::LINK_ERRORS,
    ids::WINDOW,
    ids::COMPRESSION,
];

/// How many octets went into a compressor for each that came out, as
/// the window shows it; nothing for a compressor that has had none.
fn ratio(plain: u32, coded: u32) -> Option<String> {
    (plain > 0 && coded > 0).then(|| format!("{:.1}:1", f64::from(plain) / f64::from(coded)))
}

/// What the window says of a call's data link: what it is, its frames,
/// its errors, its window and its compression.
pub fn link_values(link: Option<&LinkInfo>) -> [String; 5] {
    let Some(link) = link else {
        return std::array::from_fn(|_| EMPTY.to_string());
    };
    let what = match (link.stage.as_str(), link.end.as_deref()) {
        ("detection", _) => "Detecting error control".to_string(),
        ("setup", _) => "Setting up LAPM".to_string(),
        ("lapm", _) => "LAPM".to_string(),
        ("async", _) => "No error control".to_string(),
        ("down", Some(end)) => format!("Ended: {end}"),
        (other, _) => other.to_string(),
    };
    // Without LAPM there are no frames, and nothing of them to say.
    let framed = link.window_size > 0 || link.frames_tx > 0 || link.frames_rx > 0;
    if !framed {
        let rest: [String; 4] = std::array::from_fn(|_| EMPTY.to_string());
        let [a, b, c, d] = rest;
        return [what, a, b, c, d];
    }
    let busy = match (link.own_busy, link.peer_busy) {
        (true, true) => ", both busy",
        (true, false) => ", host busy",
        (false, true) => ", far end busy",
        (false, false) => "",
    };
    let direction = |mode: &str, plain: u32, coded: u32| match mode {
        "none" => None,
        _ => Some(ratio(plain, coded).unwrap_or_else(|| mode.to_string())),
    };
    let sent = direction(&link.compression_tx, link.tx_octets_in, link.tx_octets_out);
    let received = direction(&link.compression_rx, link.rx_octets_out, link.rx_octets_in);
    let compression = match (sent, received) {
        (None, None) => "None".to_string(),
        (Some(sent), None) => format!("{sent} out"),
        (None, Some(received)) => format!("{received} in"),
        (Some(sent), Some(received)) => format!("{sent} out, {received} in"),
    };
    [
        what,
        format!("{} out, {} in", link.frames_tx, link.frames_rx),
        format!(
            "{} bad, {} sent again",
            link.frame_errors, link.retransmissions
        ),
        format!("{} of {}{busy}", link.window, link.window_size),
        compression,
    ]
}

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
    if !diag.constellation.is_empty() {
        return match diag.constellation_bits {
            4 => "Constellation: sixteen points, 2400 bit/s".to_string(),
            _ => "Constellation: four points, 1200 bit/s".to_string(),
        };
    }
    match (diag.scope_kind.as_str(), diag.scope.is_empty()) {
        ("discriminator", false) => {
            "Demodulator output: mark at the top, space at the bottom".to_string()
        }
        _ => "Scope: nothing to show".to_string(),
    }
}

/// How far from the centre the constellation is drawn, in units of its
/// grid: the outer points are at 3, and what is received is around them.
const CONSTELLATION_REACH: i32 = 5;

/// The ideal points of a constellation, in units of its grid (Figure
/// 2/V.22 bis): the sixteen at 1 and 3 each way for four bits a symbol,
/// and for two the four of them that 1200 bit/s uses.
pub fn ideal_points(bits: u32) -> Vec<(i32, i32)> {
    match bits {
        4 => {
            let mut points = Vec::with_capacity(16);
            for x in [-3, -1, 1, 3] {
                for y in [-3, -1, 1, 3] {
                    points.push((x, y));
                }
            }
            points
        }
        2 => vec![(3, 1), (-1, 3), (-3, -1), (1, -3)],
        _ => Vec::new(),
    }
}

/// The square the constellation is drawn in on a canvas of a size: its
/// left, its top and its side. It is as tall as the canvas, less a
/// margin, and in the middle of it.
pub fn constellation_square(width: i32, height: i32) -> (i32, i32, i32) {
    const MARGIN: i32 = 2;
    let side = (width.min(height) - 2 * MARGIN).max(0);
    ((width - side) / 2, (height - side) / 2, side)
}

/// Where a point of the constellation is in a square: `x` and `y` are in
/// parts of a unit of the grid, `scale` of them to the unit, with the
/// real part to the right and the imaginary part up.
pub fn constellation_place(
    (x, y): (i32, i32),
    scale: i32,
    (left, top, side): (i32, i32, i32),
) -> (i32, i32) {
    let reach = CONSTELLATION_REACH * scale.max(1);
    let half = side / 2;
    let across = |value: i32| value.clamp(-reach, reach) * half / reach;
    (left + half + across(x), top + half - across(y))
}

/// The received points of a constellation as the service sends them, two
/// numbers for each.
pub fn received_points(constellation: &[i8]) -> Vec<(i32, i32)> {
    constellation
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[re, im]| (i32::from(*re), i32::from(*im)))
        .collect()
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
        let shown = self.shown.borrow();
        if let Some(diag) = shown.as_ref().filter(|diag| !diag.constellation.is_empty()) {
            // The axes, the ideal points as small crosses, and over them
            // what was received.
            let square = constellation_square(width, height);
            let (left, top, side) = square;
            let middle = (left + side / 2, top + side / 2);
            let grey = Colour::gray_text();
            canvas.line((left, middle.1), (left + side, middle.1), grey);
            canvas.line((middle.0, top), (middle.0, top + side), grey);
            for ideal in ideal_points(diag.constellation_bits) {
                let (x, y) = constellation_place(ideal, 1, square);
                canvas.line((x - 3, y), (x + 4, y), grey);
                canvas.line((x, y - 3), (x, y + 4), grey);
            }
            for point in received_points(&diag.constellation) {
                let (x, y) = constellation_place(point, diag.constellation_unit, square);
                canvas.rect(x - 1, y - 1, x + 2, y + 2, TRACE);
            }
            return;
        }
        // Where neither tone is the stronger.
        canvas.line((0, height / 2), (width, height / 2), Colour::gray_text());
        if let Some(shown) = shown.as_ref() {
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
            ..ModemInfo::default()
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
                "0",
                EMPTY,
                EMPTY,
                EMPTY,
                EMPTY,
                EMPTY,
                EMPTY,
                EMPTY,
                EMPTY
            ]
        );
        // A modem at 2400 bit/s with LAPM and V.42bis.
        let fast = ModemInfo {
            snr_tenth_db: Some(312),
            equalizer_error_tenth_db: Some(-305),
            timing_offset_ppm: Some(-12),
            link: Some(Box::new(LinkInfo {
                stage: "lapm".to_string(),
                frames_tx: 162,
                frames_rx: 161,
                retransmissions: 3,
                frame_errors: 2,
                window: 1,
                window_size: 15,
                compression_tx: "compressed".to_string(),
                compression_rx: "transparent".to_string(),
                tx_octets_in: 20_000,
                tx_octets_out: 8000,
                rx_octets_in: 10_100,
                rx_octets_out: 10_000,
                ..LinkInfo::default()
            })),
            ..modem()
        };
        assert_eq!(
            values(Some(&fast))[7..],
            [
                "31.2 dB",
                "-30.5 dB",
                "-12 ppm",
                "LAPM",
                "162 out, 161 in",
                "2 bad, 3 sent again",
                "1 of 15",
                "2.5:1 out, 1.0:1 in"
            ]
        );
        // The link as it comes up, without error control, and ended.
        let link = |stage: &str, end: Option<&str>| LinkInfo {
            stage: stage.to_string(),
            end: end.map(str::to_string),
            compression_tx: "none".to_string(),
            compression_rx: "none".to_string(),
            ..LinkInfo::default()
        };
        let shown = |link: &LinkInfo| link_values(Some(link))[0].clone();
        assert_eq!(shown(&link("detection", None)), "Detecting error control");
        assert_eq!(shown(&link("setup", None)), "Setting up LAPM");
        assert_eq!(shown(&link("down", Some("required"))), "Ended: required");
        assert_eq!(
            link_values(Some(&link("async", None))),
            ["No error control", EMPTY, EMPTY, EMPTY, EMPTY]
        );
        // LAPM with no compression, and a host that is not reading.
        let busy = LinkInfo {
            window_size: 15,
            own_busy: true,
            ..link("lapm", None)
        };
        assert_eq!(
            link_values(Some(&busy))[3..],
            ["0 of 15, host busy", "None"]
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
            ..LineDiag::default()
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
        let constellation = LineDiag {
            constellation_bits: 4,
            constellation_unit: 30,
            constellation: vec![30, 30, -90, 90],
            ..none.clone()
        };
        assert_eq!(
            scope_title(&constellation),
            "Constellation: sixteen points, 2400 bit/s"
        );
        let four = LineDiag {
            constellation_bits: 2,
            ..constellation
        };
        assert_eq!(scope_title(&four), "Constellation: four points, 1200 bit/s");
    }

    #[test]
    fn the_constellation_is_a_square_in_the_middle_with_the_real_part_to_the_right() {
        // A canvas wider than it is tall: the square is as tall as it,
        // less the margins, and in the middle.
        let square = constellation_square(404, 104);
        assert_eq!(square, (152, 2, 100));
        // The centre, a unit to the right, and three units up: five units
        // are half the square.
        assert_eq!(constellation_place((0, 0), 30, square), (202, 52));
        assert_eq!(constellation_place((30, 0), 30, square), (212, 52));
        assert_eq!(constellation_place((0, 90), 30, square), (202, 22));
        // What is past the edge is at it.
        assert_eq!(constellation_place((127, -127), 20, square), (252, 102));
        // The ideal points are in units.
        assert_eq!(constellation_place((3, 1), 1, square), (232, 42));
        assert_eq!(ideal_points(4).len(), 16);
        assert!(ideal_points(4).contains(&(-3, 1)));
        assert_eq!(ideal_points(2), [(3, 1), (-1, 3), (-3, -1), (1, -3)]);
        assert!(ideal_points(0).is_empty());
        // The points come two numbers each; an odd one over is dropped.
        assert_eq!(received_points(&[1, 2, -3, 4, 5]), [(1, 2), (-3, 4)]);
        assert_eq!(constellation_square(3, 3), (1, 1, 0));
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
