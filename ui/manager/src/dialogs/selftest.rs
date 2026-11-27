// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Loopback self-test: the modems of two lines, joined.
//!
//! The service joins the modem of one line to the modem of another with
//! nothing between them, has the first call the second, and once they are
//! connected has each send the other 64 KiB. It says whether the modems
//! of this service work with the settings of those two lines; the network
//! and the hosts have no part in it. The lines must be idle and are busy
//! while the test runs, which is some seconds.

use std::rc::Rc;

use ctlproto::{Call, LineInfo, SelfTest, SelfTestResult};
use model::{EMPTY, MODEM_ENGINE};
use win32::controls::Combo;
use win32::{DialogTemplate, Event, Handler, Hwnd, Reply};

use super::{watch, watched};
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// The line that originates.
    pub const FROM: u16 = 300;
    /// The line that answers.
    pub const TO: u16 = 301;
    /// Run.
    pub const RUN: u16 = 302;
    /// What the modems connected with.
    pub const CONNECT: u16 = 303;
    /// What crossed from the first to the second.
    pub const FORWARD: u16 = 304;
    /// What crossed from the second to the first.
    pub const BACK: u16 = 305;
    /// How long it took.
    pub const TIME: u16 = 306;
    /// Passed or failed.
    pub const RESULT: u16 = 307;
}

/// The values of a result, in the order of [`VALUES`].
pub const VALUES: [u16; 5] = [
    ids::CONNECT,
    ids::FORWARD,
    ids::BACK,
    ids::TIME,
    ids::RESULT,
];

fn template() -> DialogTemplate {
    DialogTemplate::new("Loopback self-test", 270, 176)
        .label(
            7,
            7,
            256,
            24,
            "The modems of two idle lines are joined with nothing between them. The \
             first calls the second, and each sends the other 64 KiB. Both lines are \
             busy while the test runs, which is some seconds.",
        )
        .label(7, 39, 80, 8, "&Originating line:")
        .combo(ids::FROM, 90, 37, 120, 100)
        .label(7, 57, 80, 8, "&Answering line:")
        .combo(ids::TO, 90, 55, 120, 100)
        .default_button(ids::RUN, 213, 37, 50, 14, "&Run")
        .group(7, 76, 256, 72, "Result")
        .label(14, 88, 70, 8, "Connected:")
        .value(ids::CONNECT, 90, 88, 166, 8, "")
        .label(14, 100, 70, 8, "First to second:")
        .value(ids::FORWARD, 90, 100, 166, 8, "")
        .label(14, 112, 70, 8, "Second to first:")
        .value(ids::BACK, 90, 112, 166, 8, "")
        .label(14, 124, 70, 8, "Time:")
        .value(ids::TIME, 90, 124, 166, 8, "")
        .label(14, 136, 70, 8, "Result:")
        .value(ids::RESULT, 90, 136, 166, 8, "")
        .button(win32::id::CANCEL, 213, 155, 50, 14, "Close")
}

/// A line as the lists show it.
pub fn entry(line: &LineInfo) -> String {
    match &line.name {
        Some(name) => format!("{}  {name}", line.id),
        None => line.id.to_string(),
    }
}

/// The lines a test can use, in the order of their ids: those whose
/// engine is the modem.
pub fn candidates(lines: &[LineInfo]) -> Vec<&LineInfo> {
    lines
        .iter()
        .filter(|line| line.engine == MODEM_ENGINE)
        .collect()
}

/// The two lines the dialog starts with: the selected line and the one
/// after it, or the first two. Indexes into [`candidates`].
pub fn first_choice(candidates: &[&LineInfo], selected: Option<u16>) -> (usize, usize) {
    let from = selected
        .and_then(|id| candidates.iter().position(|line| line.id == id))
        .unwrap_or(0);
    let to = if from + 1 < candidates.len() {
        from + 1
    } else {
        0
    };
    (from, to)
}

/// A time in milliseconds as seconds and tenths.
fn seconds(ms: u32) -> String {
    format!("{}.{} s", ms / 1000, ms % 1000 / 100)
}

/// What the dialog says of a result, in the order of [`VALUES`].
pub fn values(result: &SelfTestResult) -> [String; 5] {
    let crossed = |good: u32| format!("{good} of {} octets arrived whole", result.octets);
    [
        match (&result.connect, result.connect_ms) {
            (Some(connect), Some(ms)) => format!(
                "{} bit/s{} after {} of the call",
                connect.rate,
                match &connect.carrier {
                    Some(carrier) => format!(", {carrier},"),
                    None => String::new(),
                },
                seconds(ms)
            ),
            _ => "The modems did not connect".to_string(),
        },
        crossed(result.octets_a_to_b),
        crossed(result.octets_b_to_a),
        format!(
            "{} min of call in {}",
            result.call_ms / 60_000,
            seconds(result.elapsed_ms)
        ),
        if result.passed { "Passed" } else { "Failed" }.to_string(),
    ]
}

struct SelfTestDialog {
    manager: Rc<Manager>,
    /// The lines of the lists, in their order.
    lines: Vec<LineInfo>,
    start: (usize, usize),
}

impl SelfTestDialog {
    fn chosen(&self, dialog: Hwnd, list: u16) -> Option<u16> {
        let index = Combo::of(dialog.item(list)).selected()?;
        self.lines.get(index).map(|line| line.id)
    }

    fn run(&self, dialog: Hwnd) {
        let (Some(a), Some(b)) = (self.chosen(dialog, ids::FROM), self.chosen(dialog, ids::TO))
        else {
            return;
        };
        for control in VALUES {
            dialog.item(control).set_text(EMPTY);
        }
        dialog.item(ids::RESULT).set_text("Running\u{2026}");
        let answer = self
            .manager
            .call::<SelfTestResult>(dialog, Call::SelfTestRun(SelfTest { a, b }));
        match answer {
            Ok(result) => {
                for (control, text) in VALUES.iter().zip(values(&result)) {
                    dialog.item(*control).set_text(&text);
                }
                // A test that failed says why as every failure does.
                if let Some(error) = &result.error {
                    self.manager.show_error(dialog, error);
                }
            }
            Err(error) => {
                dialog.item(ids::RESULT).set_text("Not run");
                self.manager.show_error(dialog, &error);
            }
        }
        // The service has answered: a test is told that the result shows.
        watch(&self.manager, dialog);
    }
}

impl Handler for SelfTestDialog {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                for (list, chosen) in [(ids::FROM, self.start.0), (ids::TO, self.start.1)] {
                    let combo = Combo::of(dialog.item(list));
                    for line in &self.lines {
                        combo.add(&entry(line));
                    }
                    combo.select(Some(chosen));
                }
                for control in VALUES {
                    dialog.item(control).set_text(EMPTY);
                }
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "selftest") => Reply::Handled,
            Event::Command { id: ids::RUN, .. } => {
                self.run(dialog);
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Shows the self-test, with the selected line as the one that
/// originates.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    let (lines, selected) = manager.with_model(|model| {
        let all: Vec<LineInfo> = model.lines.lines().cloned().collect();
        let mut lines: Vec<LineInfo> = candidates(&all).into_iter().cloned().collect();
        lines.sort_by_key(|line| line.id);
        (lines, model.lines.selected().map(|line| line.id))
    });
    let start = first_choice(&lines.iter().collect::<Vec<_>>(), selected);
    let handler = Rc::new(SelfTestDialog {
        manager: Rc::clone(manager),
        lines,
        start,
    });
    template().show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use ctlproto::{ConnectInfo, Counters, ErrorInfo, LineModem, LineSip, LineState};

    use super::*;

    fn line(id: u16, engine: &str, name: Option<&str>) -> LineInfo {
        LineInfo {
            id,
            name: name.map(str::to_string),
            backend: "com0com".to_string(),
            port: format!("CNCB{id}"),
            enabled: true,
            state: LineState::Idle,
            auto_answer_rings: 0,
            unknown_command: "ok".to_string(),
            counters: Counters::default(),
            call: None,
            error: None,
            binding: None,
            engine: engine.to_string(),
            hunt_groups: Vec::new(),
            sip: LineSip::default(),
            capture: false,
            modem: LineModem::default(),
        }
    }

    #[test]
    fn the_lists_have_the_lines_whose_engine_is_the_modem() {
        let lines = [
            line(1, "modem", Some("Front desk")),
            line(2, "null", None),
            line(3, "modem", None),
            line(4, "modem", None),
        ];
        let offered = candidates(&lines);
        assert_eq!(
            offered.iter().map(|line| line.id).collect::<Vec<_>>(),
            [1, 3, 4]
        );
        assert_eq!(entry(offered[0]), "1  Front desk");
        assert_eq!(entry(offered[1]), "3");
        // The selected line calls the one after it; the last calls the
        // first; a line that is not offered, or none, is the first two.
        assert_eq!(first_choice(&offered, Some(3)), (1, 2));
        assert_eq!(first_choice(&offered, Some(4)), (2, 0));
        assert_eq!(first_choice(&offered, Some(2)), (0, 1));
        assert_eq!(first_choice(&offered, None), (0, 1));
    }

    #[test]
    fn a_result_is_laid_out_for_people() {
        let passed = SelfTestResult {
            a: 1,
            b: 2,
            passed: true,
            connect: Some(ConnectInfo {
                rate: 300,
                carrier: Some("V.21".to_string()),
                ..ConnectInfo::default()
            }),
            connect_ms: Some(5480),
            octets: 65_536,
            octets_a_to_b: 65_536,
            octets_b_to_a: 65_536,
            call_ms: 2_192_000,
            elapsed_ms: 6240,
            error: None,
        };
        assert_eq!(
            values(&passed),
            [
                "300 bit/s, V.21, after 5.4 s of the call",
                "65536 of 65536 octets arrived whole",
                "65536 of 65536 octets arrived whole",
                "36 min of call in 6.2 s",
                "Passed"
            ]
        );
        let failed = SelfTestResult {
            passed: false,
            connect: None,
            connect_ms: None,
            octets_a_to_b: 0,
            octets_b_to_a: 0,
            error: Some(ErrorInfo::from(msg::message(
                "RMD7005",
                &["1", "2", "1", "listening"],
            ))),
            ..passed
        };
        let shown = values(&failed);
        assert_eq!(shown[0], "The modems did not connect");
        assert_eq!(shown[1], "0 of 65536 octets arrived whole");
        assert_eq!(shown[4], "Failed");
        assert_eq!(VALUES.len(), shown.len());
    }

    #[test]
    fn the_template_runs_by_default_and_closes_with_escape() {
        let ids = template().ids();
        for id in VALUES {
            assert!(ids.contains(&id));
        }
        assert!(ids.contains(&ids::RUN) && ids.contains(&win32::id::CANCEL));
    }
}
