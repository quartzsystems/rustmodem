// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the manager shows and does of the modems, against the frozen
//! demo service, whose lines are modem lines: the modulations and the
//! settings with them, a line's own, the diagnostics of a line in a call,
//! the loopback self-test, and what the grid and the statistics say.

#![cfg(windows)]

mod common;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use common::{Bench, DemoService, close, pipe_name};
use manager::{Hooks, diag_ids, ids, line_ids, modem_ids, selftest_ids};
use model::{Column, Command, EMPTY};
use win32::controls::{self, Combo, Progress};
use win32::{CheckList, Hwnd, ListBox, ListView, TaskDialog, id};

const CONNECT: Duration = Duration::from_secs(30);

/// What a test does with a dialog that is up: its name, how many times
/// that dialog has said so, from 1, and its window.
type WithDialog = Box<dyn Fn(&str, usize, Hwnd)>;

/// What the hooks saw and what they are to do.
#[derive(Default)]
struct Seen {
    /// How many times each dialog has said that it is up.
    times: RefCell<BTreeMap<String, usize>>,
    /// The task dialogs that would have been shown.
    tasks: RefCell<Vec<TaskDialog>>,
    /// What to do with a dialog that is up; without it the dialog is
    /// closed.
    with_dialog: RefCell<Option<WithDialog>>,
    /// The dialog that is up.
    current: Cell<Option<Hwnd>>,
}

impl Seen {
    fn act(&self, act: impl Fn(&str, usize, Hwnd) + 'static) {
        self.times.borrow_mut().clear();
        *self.with_dialog.borrow_mut() = Some(Box::new(act));
    }

    fn times(&self, name: &str) -> usize {
        self.times.borrow().get(name).copied().unwrap_or(0)
    }

    fn task_titles(&self) -> Vec<String> {
        self.tasks
            .borrow()
            .iter()
            .map(|task| task.title.clone())
            .collect()
    }
}

fn hooks(seen: &Rc<Seen>) -> Hooks {
    let dialog = {
        let seen = Rc::clone(seen);
        move |name: &str, window: Hwnd| {
            let time = {
                let mut times = seen.times.borrow_mut();
                let time = times.entry(name.to_string()).or_insert(0);
                *time += 1;
                *time
            };
            seen.current.set(Some(window));
            match seen.with_dialog.borrow().as_ref() {
                Some(act) => act(name, time, window),
                None => close(name, window),
            }
        }
    };
    let task = {
        let seen = Rc::clone(seen);
        move |dialog: &TaskDialog| {
            seen.tasks.borrow_mut().push(dialog.clone());
            id::CLOSE
        }
    };
    Hooks {
        dialog: Some(Rc::new(dialog)),
        task: Some(Rc::new(task)),
        file: Some(Rc::new(|_: &str| None)),
        ..Hooks::default()
    }
}

/// A manager connected to the frozen demo service, with the hooks in.
fn connected(test: &str, lines: u32) -> (DemoService, Bench, Rc<Seen>) {
    let name = pipe_name(test);
    let service = DemoService::start(&name, "frozen", lines);
    let seen = Rc::new(Seen::default());
    let bench = Bench::open_with(&name, hooks(&seen));
    assert!(
        bench.run_until(CONNECT, |bench| {
            bench.opened.manager.with_model(|model| {
                model.connected() && model.lines.len() == lines as usize && model.stats().is_some()
            }) && bench.opened.manager.requests_waiting() == 0
        }),
        "the manager did not connect to the demo service"
    );
    (service, bench, seen)
}

fn select_line(bench: &Bench, id: u16) {
    let row = bench
        .opened
        .manager
        .with_model(|model| model.lines.row_of(id))
        .expect("the line is in the grid");
    ListView::of(bench.opened.window.item(ids::LIST)).select(Some(row));
    bench.pump();
}

fn cell(bench: &Bench, id: u16, column: Column) -> String {
    bench.opened.manager.with_model(|model| {
        let row = model.lines.row_of(id).expect("the line is in the grid");
        model.lines.cell(row, column, 0)
    })
}

/// The names the modulation list has, as it shows them.
const TITLES: [&str; 5] = ["V.22bis", "V.22", "Bell 212A", "V.21", "Bell 103"];

/// Every modulation but V.22bis, as the service calls them and in its
/// order.
const WITHOUT_BIS: [&str; 4] = ["v22", "bell212a", "v21", "bell103"];

/// The rows of the modulation list: the name and whether it is checked.
fn rows(dialog: Hwnd) -> Vec<(&'static str, bool)> {
    let list = CheckList::of(dialog.item(modem_ids::LIST));
    (0..list.row_count())
        .map(|row| {
            let shown = list.cell_text(row, 0);
            let title = TITLES
                .into_iter()
                .find(|title| *title == shown)
                .unwrap_or("a name the list should not have");
            (title, list.checked(row))
        })
        .collect()
}

#[test]
fn the_grid_and_the_statistics_show_what_the_modems_say() {
    let (_service, bench, _seen) = connected("modem-grid", 8);
    // Line 1 is online: its modem has counted no retrain, which is a
    // number. Line 5 has no call and so nothing to count.
    assert_eq!(cell(&bench, 1, Column::Retrains), "0");
    assert_eq!(cell(&bench, 5, Column::Retrains), EMPTY);
    assert_ne!(cell(&bench, 1, Column::Modulation), EMPTY);
    // The engine is rustmodem's own, with the version of the service.
    let engine = bench.opened.manager.with_model(|model| {
        let clock = model::Utc;
        model
            .stats_view(0, &clock)
            .carrier
            .iter()
            .find(|(label, _)| *label == "Softmodem engine")
            .map(|(_, value)| value.clone())
            .unwrap()
    });
    assert_eq!(engine, format!("RustModem {}", env!("CARGO_PKG_VERSION")));
    // The three commands of the modem can be chosen.
    select_line(&bench, 1);
    bench.opened.manager.with_model(|model| {
        for command in [
            Command::RateCap,
            Command::LineDiagnostics,
            Command::LoopbackTest,
        ] {
            assert!(model.availability(command).is_enabled(), "{command:?}");
        }
    });
}

#[test]
fn the_modulations_are_reordered_and_the_settings_sent() {
    let (_service, bench, seen) = connected("modem-settings", 5);

    // A command to a dialog arrives after the step that sent it: the
    // dialog says when it shows the outcome, which is the next step.
    seen.act(|name, time, dialog| {
        assert_eq!(name, "modem");
        let list = CheckList::of(dialog.item(modem_ids::LIST));
        match time {
            1 => {
                // As the service has them: all five, the fastest first,
                // with V.8, at -12, with error control if the far end has
                // it, compression, and no guard tone.
                assert_eq!(
                    rows(dialog),
                    [
                        ("V.22bis", true),
                        ("V.22", true),
                        ("Bell 212A", true),
                        ("V.21", true),
                        ("Bell 103", true)
                    ]
                );
                assert!(controls::checked(dialog.item(modem_ids::V8)));
                let error_control = Combo::of(dialog.item(modem_ids::ERROR_CONTROL));
                assert_eq!(error_control.count(), 3);
                assert_eq!(error_control.selected(), Some(0));
                assert!(controls::checked(dialog.item(modem_ids::COMPRESSION)));
                let guard_tone = Combo::of(dialog.item(modem_ids::GUARD_TONE));
                assert_eq!(guard_tone.entry(guard_tone.selected().unwrap()), "Off");
                let level = Combo::of(dialog.item(modem_ids::LEVEL));
                assert_eq!(level.count(), 12);
                assert_eq!(level.entry(level.selected().unwrap()), "-12 dBm0");
                assert_eq!(dialog.item(modem_ids::DELAY).text(), "0");
                assert_eq!(dialog.item(modem_ids::FALLBACK).text(), "3000");
                // The first row is selected: it can go down and not up.
                assert!(!dialog.item(modem_ids::UP).enabled());
                assert!(dialog.item(modem_ids::DOWN).enabled());
                dialog.command(modem_ids::DOWN);
            }
            2 => {
                assert_eq!(rows(dialog)[..2], [("V.22", true), ("V.22bis", true)]);
                // The row that moved is still the one selected, now the
                // second: it can go either way.
                assert_eq!(list.selected(), Some(1));
                assert!(dialog.item(modem_ids::UP).enabled());
                assert!(dialog.item(modem_ids::DOWN).enabled());
                // Moving keeps the boxes with their rows.
                list.set_checked(1, false);
                dialog.command(modem_ids::UP);
            }
            3 => {
                assert_eq!(rows(dialog)[..2], [("V.22bis", false), ("V.22", true)]);
                dialog.command(modem_ids::DOWN);
            }
            4 => {
                assert_eq!(
                    rows(dialog),
                    [
                        ("V.22", true),
                        ("V.22bis", false),
                        ("Bell 212A", true),
                        ("V.21", true),
                        ("Bell 103", true)
                    ]
                );
                controls::set_checked(dialog.item(modem_ids::V8), false);
                Combo::of(dialog.item(modem_ids::LEVEL)).select(Some(1));
                dialog.item(modem_ids::DELAY).set_text("1500");
                Combo::of(dialog.item(modem_ids::ERROR_CONTROL)).select(Some(1));
                controls::set_checked(dialog.item(modem_ids::COMPRESSION), false);
                Combo::of(dialog.item(modem_ids::GUARD_TONE)).select(Some(1));
                dialog.command(id::OK);
            }
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::RateCap);
    assert_eq!(seen.times("modem"), 4);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());

    // The service has them, and so do the lines when they are asked.
    assert!(bench.run_until(CONNECT, |bench| {
        bench.opened.manager.requests_waiting() == 0
            && bench
                .opened
                .manager
                .with_model(|model| model.lines.line(5).unwrap().modem.modulations == WITHOUT_BIS)
    }));
    bench.opened.manager.with_model(|model| {
        let modem = &model.lines.line(5).unwrap().modem;
        assert_eq!((modem.v8, modem.tx_level_dbm0), (false, -10));
        assert_eq!(
            (
                modem.error_control.as_str(),
                modem.compression,
                modem.guard_tone.as_str()
            ),
            ("required", false, "1800")
        );
        assert!(modem.own.is_empty());
    });

    // It opens with what the service has now; with nothing checked it
    // says so and stays, and Cancel changes nothing.
    seen.act(|name, time, dialog| {
        assert_eq!(name, "modem");
        match time {
            1 => {
                // The ones that are allowed first, and after them the
                // one that is not.
                assert_eq!(
                    rows(dialog),
                    [
                        ("V.22", true),
                        ("Bell 212A", true),
                        ("V.21", true),
                        ("Bell 103", true),
                        ("V.22bis", false)
                    ]
                );
                assert!(!controls::checked(dialog.item(modem_ids::V8)));
                assert_eq!(dialog.item(modem_ids::DELAY).text(), "1500");
                let error_control = Combo::of(dialog.item(modem_ids::ERROR_CONTROL));
                assert_eq!(error_control.selected(), Some(1));
                assert!(!controls::checked(dialog.item(modem_ids::COMPRESSION)));
                let guard_tone = Combo::of(dialog.item(modem_ids::GUARD_TONE));
                assert_eq!(guard_tone.entry(guard_tone.selected().unwrap()), "1800 Hz");
                let list = CheckList::of(dialog.item(modem_ids::LIST));
                for row in 0..4 {
                    list.set_checked(row, false);
                }
                dialog.command(id::OK);
            }
            // Refused: the dialog is still up.
            2 => close(name, dialog),
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::RateCap);
    assert_eq!(seen.times("modem"), 2);
    assert_eq!(seen.task_titles(), ["RMD4015"]);
    seen.tasks.borrow_mut().clear();
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.line(5).unwrap().modem.modulations, WITHOUT_BIS);
    });
}

#[test]
fn a_line_is_given_modem_settings_of_its_own_and_gives_them_up() {
    let (_service, bench, seen) = connected("modem-line", 5);
    select_line(&bench, 5);

    // Edit line, Settings: the line gets settings of its own. The line's
    // dialog says that it is up again when the settings have closed.
    seen.act(|name, time, dialog| match (name, time) {
        ("edit-line", 1) => {
            assert!(dialog.item(line_ids::MODEM).enabled());
            dialog.command(line_ids::MODEM);
        }
        ("line-modem", 1) => {
            // The line has none of its own: what it shows is the
            // service's, and is not to be changed.
            assert!(!controls::checked(dialog.item(modem_ids::OWN)));
            assert!(!dialog.item(modem_ids::LIST).enabled());
            assert!(!dialog.item(modem_ids::V8).enabled());
            assert!(!dialog.item(modem_ids::UP).enabled());
            controls::set_checked(dialog.item(modem_ids::OWN), true);
            dialog.command(modem_ids::OWN);
        }
        ("line-modem", 2) => {
            assert!(dialog.item(modem_ids::LIST).enabled());
            assert!(dialog.item(modem_ids::DOWN).enabled());
            CheckList::of(dialog.item(modem_ids::LIST)).set_checked(0, false);
            controls::set_checked(dialog.item(modem_ids::V8), false);
            dialog.command(id::OK);
        }
        ("edit-line", 2) => dialog.command(id::OK),
        other => panic!("unexpected {other:?}"),
    });
    bench.choose(Command::EditLine);
    assert_eq!((seen.times("edit-line"), seen.times("line-modem")), (2, 2));
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
    bench.opened.manager.with_model(|model| {
        let modem = &model.lines.line(5).unwrap().modem;
        assert_eq!(modem.modulations, WITHOUT_BIS);
        assert!(!modem.v8);
        assert_eq!(
            modem.own,
            [
                "modulations",
                "v8",
                "tx_level_dbm0",
                "error_control",
                "compression",
                "guard_tone"
            ]
        );
        // The other lines have the service's still.
        assert!(model.lines.line(4).unwrap().modem.own.is_empty());
    });

    // It opens with them; cancelled, the line's dialog sends nothing of
    // them, and they stay.
    seen.act(|name, time, dialog| match (name, time) {
        ("edit-line", 1) => dialog.command(line_ids::MODEM),
        ("line-modem", 1) => {
            assert!(controls::checked(dialog.item(modem_ids::OWN)));
            assert_eq!(
                rows(dialog),
                [
                    ("V.22", true),
                    ("Bell 212A", true),
                    ("V.21", true),
                    ("Bell 103", true),
                    ("V.22bis", false)
                ]
            );
            controls::set_checked(dialog.item(modem_ids::OWN), false);
            close(name, dialog);
        }
        ("edit-line", 2) => dialog.command(id::OK),
        other => panic!("unexpected {other:?}"),
    });
    bench.choose(Command::EditLine);
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.line(5).unwrap().modem.own.len(), 6);
    });

    // The box cleared and OK: the line has the service's again.
    seen.act(|name, time, dialog| match (name, time) {
        ("edit-line", 1) => dialog.command(line_ids::MODEM),
        ("line-modem", 1) => {
            controls::set_checked(dialog.item(modem_ids::OWN), false);
            dialog.command(modem_ids::OWN);
        }
        ("line-modem", 2) => dialog.command(id::OK),
        ("edit-line", 2) => dialog.command(id::OK),
        other => panic!("unexpected {other:?}"),
    });
    bench.choose(Command::EditLine);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
    bench.opened.manager.with_model(|model| {
        let modem = &model.lines.line(5).unwrap().modem;
        assert!(modem.own.is_empty());
        assert_eq!(
            modem.modulations,
            ["v22bis", "v22", "bell212a", "v21", "bell103"]
        );
        assert!(modem.v8);
    });

    // With another engine there are no modem settings to open.
    seen.act(|name, time, dialog| {
        assert_eq!(name, "edit-line");
        match time {
            1 => {
                Combo::of(dialog.item(line_ids::ENGINE)).select(Some(1));
                dialog.command(line_ids::ENGINE);
            }
            2 => {
                assert!(!dialog.item(line_ids::MODEM).enabled());
                close(name, dialog);
            }
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::EditLine);
    assert_eq!(seen.times("edit-line"), 2);
}

#[test]
fn the_diagnostics_show_the_modem_of_the_line_and_follow_the_selection() {
    let (_service, bench, seen) = connected("modem-diag", 8);
    let manager = Rc::clone(&bench.opened.manager);
    // The window stays open: nothing closes it but the test.
    seen.act(|name, _, _| assert_eq!(name, "diagnostics"));

    // Line 1 is in a call, connected.
    select_line(&bench, 1);
    bench.choose(Command::LineDiagnostics);
    let window = manager
        .diagnostics_window()
        .expect("the diagnostics are open");
    let text = |control: u16| window.item(control).text();
    assert!(bench.run_until(CONNECT, |_| text(diag_ids::STATE) == "data"));
    assert!(text(diag_ids::LINE).starts_with('1'));
    assert_eq!(text(diag_ids::RX), "-14.5 dBm0");
    assert_eq!(text(diag_ids::TX), "-12.0 dBm0");
    assert_eq!(text(diag_ids::CARRIER), "Detected");
    assert_eq!(text(diag_ids::FREQUENCY), "+0.4 Hz");
    assert_eq!(text(diag_ids::QUALITY), "90%");
    assert_eq!(
        Progress::of(window.item(diag_ids::QUALITY_BAR)).percent(),
        90
    );
    assert_eq!(text(diag_ids::FRAMING), "0");
    assert_eq!(text(diag_ids::RETRAINS), "0");
    let history = ListBox::of(window.item(diag_ids::HISTORY));
    assert_eq!(history.count(), 2);
    assert!(history.entry(0).ends_with("training"));
    assert!(history.entry(1).ends_with("data"));
    // The call is at 1200 bit/s with LAPM and V.42bis: the receiver's
    // figures, the data link, and the constellation where a 300 bit/s
    // call has its scope.
    assert_eq!(text(diag_ids::SNR), "31.5 dB");
    assert_eq!(text(diag_ids::EQUALIZER), "-31.5 dB");
    assert_eq!(text(diag_ids::TIMING), "-12 ppm");
    assert_eq!(text(diag_ids::LINK), "LAPM");
    assert!(text(diag_ids::FRAMES).ends_with(" in"));
    assert!(text(diag_ids::LINK_ERRORS).ends_with(" sent again"));
    assert_eq!(text(diag_ids::WINDOW), "1 of 15");
    assert!(
        text(diag_ids::COMPRESSION).starts_with("2.5:1 out"),
        "{}",
        text(diag_ids::COMPRESSION)
    );
    assert_eq!(
        text(diag_ids::SCOPE_KIND),
        "Constellation: four points, 1200 bit/s"
    );

    // The command again with another line selected: the same window
    // shows that line. Line 5 has no call, and so nothing to show.
    select_line(&bench, 5);
    bench.choose(Command::LineDiagnostics);
    assert_eq!(manager.diagnostics_window(), Some(window));
    assert!(bench.run_until(CONNECT, |_| {
        text(diag_ids::LINE).starts_with('5') && text(diag_ids::STATE) == EMPTY
    }));
    assert_eq!(text(diag_ids::RX), EMPTY);
    assert_eq!(text(diag_ids::QUALITY), EMPTY);
    assert_eq!(history.count(), 0);
    assert_eq!(text(diag_ids::SCOPE_KIND), "Scope: nothing to show");

    // And back to the line that has a call.
    select_line(&bench, 2);
    bench.choose(Command::LineDiagnostics);
    assert!(bench.run_until(CONNECT, |_| {
        text(diag_ids::LINE).starts_with('2') && text(diag_ids::STATE) == "data"
    }));

    // Closed, the service is asked for no more.
    window.end_dialog(id::CANCEL);
    bench.pump();
    assert_eq!(manager.diagnostics_window(), None);
    assert!(bench.run_until(CONNECT, |bench| {
        bench.opened.manager.requests_waiting() == 0
    }));
    assert_eq!(seen.times("diagnostics"), 1);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
}

#[test]
fn the_self_test_runs_two_lines_and_shows_what_their_modems_did() {
    let (_service, bench, seen) = connected("modem-selftest", 5);
    // In the frozen history lines 1 to 4 are in calls. The selected line
    // is the last of five, so the dialog has it call the first, which is
    // busy: the service refuses the test.
    select_line(&bench, 5);
    seen.act(|name, time, dialog| {
        assert_eq!(name, "selftest");
        let from = Combo::of(dialog.item(selftest_ids::FROM));
        let to = Combo::of(dialog.item(selftest_ids::TO));
        match time {
            1 => {
                // Every line is a modem line; the selected one calls,
                // and being the last it calls the first.
                assert_eq!(from.count(), 5);
                assert_eq!(from.selected(), Some(4));
                assert_eq!(to.selected(), Some(0));
                assert_eq!(dialog.item(selftest_ids::RESULT).text(), EMPTY);
                dialog.command(selftest_ids::RUN);
            }
            2 => {
                // Line 1 is in a call: the service refused, and said so.
                assert_eq!(dialog.item(selftest_ids::RESULT).text(), "Not run");
                close(name, dialog);
            }
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::LoopbackTest);
    assert_eq!(seen.times("selftest"), 2);
    assert_eq!(seen.task_titles(), ["RMD7002"]);
    seen.tasks.borrow_mut().clear();
}

#[test]
fn the_self_test_passes_between_two_idle_lines() {
    // The frozen history has something for each of its first eight
    // lines to be; the two after them are idle.
    let (_service, bench, seen) = connected("modem-selftest-pass", 10);
    select_line(&bench, 9);
    seen.act(|name, time, dialog| {
        assert_eq!(name, "selftest");
        match time {
            1 => {
                let from = Combo::of(dialog.item(selftest_ids::FROM));
                let to = Combo::of(dialog.item(selftest_ids::TO));
                assert_eq!(from.entry(from.selected().unwrap()), "9");
                assert_eq!(to.entry(to.selected().unwrap()), "10");
                dialog.command(selftest_ids::RUN);
            }
            2 => {
                let text = |control: u16| dialog.item(control).text();
                assert_eq!(text(selftest_ids::RESULT), "Passed");
                assert!(
                    text(selftest_ids::CONNECT)
                        .starts_with("2400 bit/s, V.22bis, LAPM, V.42bis, after "),
                    "{}",
                    text(selftest_ids::CONNECT)
                );
                assert_eq!(
                    text(selftest_ids::FORWARD),
                    "65536 of 65536 octets arrived whole"
                );
                assert_eq!(
                    text(selftest_ids::BACK),
                    "65536 of 65536 octets arrived whole"
                );
                assert!(
                    text(selftest_ids::TIME).starts_with("4 min of call in "),
                    "{}",
                    text(selftest_ids::TIME)
                );
                close(name, dialog);
            }
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::LoopbackTest);
    assert_eq!(seen.times("selftest"), 2);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
    // The lines are idle again.
    assert!(bench.run_until(CONNECT, |bench| {
        bench.opened.manager.with_model(|model| {
            [9, 10]
                .iter()
                .all(|id| model.lines.line(*id).unwrap().state == ctlproto::LineState::Idle)
        })
    }));
}
