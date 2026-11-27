// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The smoke test: a minute of everything, with the demo service busy.
//!
//! Calls come and go on every line while each dialog is opened and
//! closed, each filter chosen and each column sorted by, over and over.
//! At the end the manager has not panicked, holds no more GDI and USER
//! objects than it did after the first round, and its model is what the
//! service says its lines are.

#![cfg(windows)]

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{Bench, DemoService, close, fresh_lines, pipe_name};
use ctlproto::{LineInfo, LineState};
use manager::{Hooks, ids};
use model::{Column, Command, Filter};
use win32::{Hwnd, ListView, gui_handles, id};

/// How long the test runs, once it has connected.
const RUN: Duration = Duration::from_secs(60);

/// How many more GDI or USER objects the manager may hold at the end
/// than after its first round. Windows keeps some for itself: a cached
/// theme part here, a font there.
const HANDLE_MARGIN: u32 = 48;

/// How many lines the demo service runs.
const LINES: usize = 12;

const CONNECT: Duration = Duration::from_secs(30);

/// The dialogs, by the commands that open them and the names they are
/// known to the test by.
const DIALOGS: [(Command, &str); 13] = [
    (Command::Connect, "connect"),
    (Command::NewLine, "new-line"),
    (Command::EditLine, "edit-line"),
    (Command::CallHistory, "history"),
    (Command::Settings, "settings"),
    (Command::PortsAndDrivers, "ports"),
    (Command::About, "about"),
    (Command::TrunkPeers, "trunk-peers"),
    (Command::HuntGroups, "hunt-groups"),
    (Command::Routes, "routes"),
    (Command::CallCapture, "capture"),
    (Command::RateCap, "modem"),
    (Command::LoopbackTest, "selftest"),
];

/// What of a line stays put between one moment and the next while calls
/// are running: who it is and what it is doing, not how many bytes it
/// has counted.
fn essence(line: &LineInfo) -> (u16, String, bool, LineState, Option<u64>) {
    (
        line.id,
        line.port.clone(),
        line.enabled,
        line.state,
        line.call.as_ref().map(|call| call.id),
    )
}

/// One round: every dialog, every filter, every column.
fn round(bench: &Bench, list: ListView, opened: &RefCell<Vec<String>>) {
    for (command, name) in DIALOGS {
        if command == Command::EditLine {
            // Edit line is for the selected line.
            list.select(Some(0));
            bench.pump();
        }
        let before = opened.borrow().len();
        bench.choose(command);
        // A modal dialog has come and gone when the command returns.
        let seen = opened.borrow();
        assert_eq!(seen.len(), before + 1, "{command:?} opened no dialog");
        assert_eq!(seen[before], name, "{command:?}");
    }

    // The AT console stays open until it is closed.
    bench.choose(Command::AtConsole);
    let console = bench
        .opened
        .manager
        .console_window()
        .expect("the console is open");
    bench.run_for(Duration::from_millis(350));
    assert_eq!(opened.borrow().last().map(String::as_str), Some("console"));
    console.end_dialog(id::CANCEL);
    bench.pump();
    assert!(!console.exists());

    // So does the SIP trace.
    bench.choose(Command::SipTrace);
    let trace = bench
        .opened
        .manager
        .trace_window()
        .expect("the trace is open");
    bench.run_for(Duration::from_millis(350));
    assert_eq!(
        opened.borrow().last().map(String::as_str),
        Some("sip-trace")
    );
    trace.end_dialog(id::CANCEL);
    bench.pump();
    assert!(!trace.exists());

    // And the diagnostics of a line, whose scope is painted while they
    // are up.
    list.select(Some(0));
    bench.pump();
    bench.choose(Command::LineDiagnostics);
    let diagnostics = bench
        .opened
        .manager
        .diagnostics_window()
        .expect("the diagnostics are open");
    bench.run_for(Duration::from_millis(350));
    assert_eq!(
        opened.borrow().last().map(String::as_str),
        Some("diagnostics")
    );
    diagnostics.end_dialog(id::CANCEL);
    bench.pump();
    assert!(!diagnostics.exists());

    // Remove line asks first; the answer is no.
    bench.choose(Command::RemoveLine);
    // The exports ask where to; the answer is nowhere.
    bench.choose(Command::ExportStatistics);
    bench.choose(Command::ExportLog);

    for command in [
        Command::ShowOnCall,
        Command::ShowIdle,
        Command::ShowNotReady,
        Command::ShowAll,
    ] {
        bench.choose(command);
        let rows = bench
            .opened
            .manager
            .with_model(|model| model.lines.row_count());
        assert_eq!(list.row_count(), rows, "{command:?}");
    }
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.filter(), Filter::All);
        assert_eq!(model.lines.row_count(), LINES);
    });

    for column in Column::ALL {
        // Twice: one way, and then the other way round.
        for _ in 0..2 {
            let expected = bench
                .opened
                .manager
                .with_model(|model| model.lines.sort_after_click(column));
            list.click_column(column.index());
            bench.pump();
            bench
                .opened
                .manager
                .with_model(|model| assert_eq!(model.lines.sort(), expected));
            assert_eq!(list.row_count(), LINES);
        }
    }
    list.click_column(Column::Number.index());
    bench.pump();
    bench.choose(Command::Refresh);
    bench.run_for(Duration::from_millis(200));
}

#[test]
fn a_minute_of_everything_leaks_nothing_and_ends_with_the_model_right() {
    let name = pipe_name("smoke");
    let _service = DemoService::start(&name, "busy", LINES as u32);

    let opened = Rc::new(RefCell::new(Vec::new()));
    let asked = Rc::new(Cell::new(0u32));
    let messages = Rc::new(RefCell::new(Vec::new()));
    let hooks = Hooks {
        dialog: Some(Rc::new({
            let opened = Rc::clone(&opened);
            move |name: &str, window: Hwnd| {
                opened.borrow_mut().push(name.to_string());
                // The console and the diagnostics are closed by the
                // round, once they have been seen to stay open.
                if name != "console" && name != "diagnostics" {
                    close(name, window);
                }
            }
        })),
        // Every question is answered no, and every message closed.
        task: Some(Rc::new({
            let asked = Rc::clone(&asked);
            let messages = Rc::clone(&messages);
            move |dialog: &win32::TaskDialog| {
                if dialog.title.starts_with("RMD") {
                    messages
                        .borrow_mut()
                        .push(format!("{}: {}", dialog.title, dialog.instruction));
                } else {
                    asked.set(asked.get() + 1);
                }
                if dialog.buttons.no { id::NO } else { id::CLOSE }
            }
        })),
        file: Some(Rc::new(|_: &str| None)),
        serial_ports: Some(Rc::new(|| vec!["COM11".to_string(), "COM12".to_string()])),
        ..Hooks::default()
    };
    let bench = Bench::open_with(&name, hooks);
    assert!(
        bench.run_until(CONNECT, |bench| {
            bench
                .opened
                .manager
                .with_model(|model| model.connected() && model.lines.len() == LINES)
        }),
        "the manager did not connect to the demo service"
    );
    let list = ListView::of(bench.opened.window.item(ids::LIST));

    // The first round makes what is made once: the fonts of the dialogs,
    // the shield, the pages of the property sheet.
    round(&bench, list, &opened);
    let (gdi_before, user_before) = gui_handles();
    assert!(gdi_before > 0 && user_before > 0);

    let started = Instant::now();
    let mut rounds = 1;
    while started.elapsed() < RUN {
        round(&bench, list, &opened);
        rounds += 1;
    }
    assert!(rounds >= 3, "only {rounds} rounds in a minute");
    // Nothing went wrong that the user would have been told of. Remove
    // line asked at most once a round: it does not ask about a line that
    // is in a call.
    assert!(messages.borrow().is_empty(), "{:?}", messages.borrow());
    assert!(asked.get() <= rounds, "{} questions", asked.get());

    // Still connected, with every line, and calls going on.
    bench.opened.manager.with_model(|model| {
        assert!(model.connected(), "{:?}", model.connection());
        assert_eq!(model.lines.len(), LINES);
        assert!(model.log.len() > 100, "{} log lines", model.log.len());
        assert!(model.stats().is_some_and(|stats| stats.calls_answered > 0));
    });
    assert_eq!(bench.opened.manager.pipe(), name);

    // Nothing was leaked.
    let (gdi_after, user_after) = gui_handles();
    assert!(
        gdi_after <= gdi_before + HANDLE_MARGIN,
        "GDI objects went from {gdi_before} to {gdi_after} in {rounds} rounds"
    );
    assert!(
        user_after <= user_before + HANDLE_MARGIN,
        "USER objects went from {user_before} to {user_after} in {rounds} rounds"
    );

    // The model is what the service says its lines are. Calls go on, so
    // the two are compared at a moment when they agree on who is doing
    // what: there is one within a few tries, or the manager has fallen
    // behind.
    let mut agreed = false;
    for _ in 0..40 {
        let fresh: Vec<_> = fresh_lines(&name).iter().map(essence).collect();
        bench.run_for(Duration::from_millis(100));
        let held: Vec<_> = bench
            .opened
            .manager
            .with_model(|model| model.lines.lines().map(essence).collect());
        if held == fresh {
            agreed = true;
            break;
        }
    }
    assert!(agreed, "the model never matched a fresh lines.list");
    // The grid shows the model.
    assert_eq!(list.row_count(), LINES);
    for row in 0..LINES {
        let expected = bench
            .opened
            .manager
            .with_model(|model| model.lines.cell(row, Column::State, 0));
        assert_eq!(list.cell_text(row, Column::State.index()), expected);
    }
}
