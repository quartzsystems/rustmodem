// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The main window, opened for real: without a service, and against the
//! demo service.

#![cfg(windows)]

mod common;

use std::time::Duration;

use common::{Bench, DemoService, pipe_name};
use ctlproto::LineState;
use manager::ids;
use model::{Column, Command, ConnectionState, Filter, MenuEntry};
use win32::controls::Combo;
use win32::{ListView, Menu, RichEdit};

/// Long enough for the demo service to start and the manager, which
/// tries every three seconds, to find it.
const CONNECT: Duration = Duration::from_secs(30);

fn menu(bench: &Bench) -> Menu {
    Menu::of_window(bench.opened.window).expect("the window has a menu bar")
}

fn grid(bench: &Bench) -> ListView {
    ListView::of(bench.opened.window.item(ids::LIST))
}

fn connected(bench: &Bench) -> bool {
    bench.opened.manager.with_model(|model| model.connected())
}

#[test]
fn the_menu_bar_is_the_one_the_model_lists() {
    let bench = Bench::open(&pipe_name("menu"));
    let shown = menu(&bench).commands();

    // Every command of every menu, in order, under its menu's text.
    let mut listed = Vec::new();
    for (title, entries) in model::menu_bar() {
        for entry in entries {
            match entry {
                MenuEntry::Item(command) => listed.push((command, title)),
                MenuEntry::Separator => {}
                MenuEntry::Submenu(text, commands) => {
                    listed.extend(commands.into_iter().map(|command| (command, text)));
                }
            }
        }
    }
    assert_eq!(shown.len(), listed.len());
    for ((id, text, parent), (command, title)) in shown.iter().zip(&listed) {
        assert_eq!(Command::from_id(*id), Some(*command), "{text}");
        assert_eq!(text, command.text());
        assert_eq!(parent, title, "{text}");
    }
    // Every command is in the menus exactly once.
    for command in Command::ALL {
        let times = listed
            .iter()
            .filter(|(listed, _)| *listed == command)
            .count();
        assert_eq!(times, 1, "{command:?}");
    }
}

#[test]
fn every_menu_item_is_enabled_exactly_when_the_model_says_so() {
    let bench = Bench::open(&pipe_name("enabled"));
    // No service listens on the pipe: the first attempt fails.
    assert!(bench.run_until(CONNECT, |bench| {
        bench.opened.manager.with_model(|model| {
            matches!(model.connection(), ConnectionState::Disconnected(Some(_)))
        })
    }));
    let menu = menu(&bench);
    for command in Command::ALL {
        let available = bench
            .opened
            .manager
            .with_model(|model| model.availability(command));
        assert_eq!(
            menu.enabled(command.id()),
            available.is_enabled(),
            "{command:?}: {available:?}"
        );
        // What is not built yet is never enabled, and says why.
        if command.blocked().is_some() {
            assert!(!menu.enabled(command.id()), "{command:?}");
            let help = bench.opened.manager.with_model(|model| model.help(command));
            assert!(help.starts_with("Not available yet"), "{command:?}: {help}");
        }
    }
    // The line commands need the service.
    for command in [Command::Answer, Command::HangUp, Command::NewLine] {
        assert!(!menu.enabled(command.id()), "{command:?}");
    }
    assert!(menu.enabled(Command::Exit.id()));
    assert!(menu.enabled(Command::Disconnect.id()));
}

#[test]
fn without_a_service_the_window_stays_up_and_says_why() {
    let name = pipe_name("alone");
    let bench = Bench::open(&name);
    assert!(bench.run_until(CONNECT, |bench| {
        bench.opened.manager.with_model(|model| {
            matches!(model.connection(), ConnectionState::Disconnected(Some(_)))
        })
    }));
    let window = bench.opened.window;
    assert_eq!(window.text(), "RustModem Manager");
    bench.opened.manager.with_model(|model| {
        let ConnectionState::Disconnected(Some(reason)) = model.connection() else {
            panic!("the manager is {:?}", model.connection());
        };
        assert_eq!(reason.msgid, "RMD2008");
        assert!(reason.text.contains(&name), "{}", reason.text);
        assert_eq!(model.status_message(), reason.to_string());
    });
    // The grid is there, empty, and takes no part.
    assert!(grid(&bench).hwnd().exists());
    assert!(!grid(&bench).hwnd().enabled());
    assert_eq!(grid(&bench).row_count(), 0);
    assert_eq!(grid(&bench).column_count(), Column::ALL.len());

    // The manager goes on trying, and stops when told to.
    bench.choose(Command::Disconnect);
    bench.opened.manager.with_model(|model| {
        assert_eq!(*model.connection(), ConnectionState::Disconnected(None));
    });
    assert!(!menu(&bench).enabled(Command::Disconnect.id()));
    bench.run_for(Duration::from_millis(3500));
    bench.opened.manager.with_model(|model| {
        assert_eq!(*model.connection(), ConnectionState::Disconnected(None));
    });
}

#[test]
fn the_view_menu_shows_and_hides_the_bars_and_sets_the_filter() {
    let bench = Bench::open(&pipe_name("view"));
    let window = bench.opened.window;
    let menu = menu(&bench);
    let toolbar = window.item(ids::TOOLBAR);
    let status = window.item(ids::STATUS);
    let filter = Combo::of(window.item(ids::FILTER));
    bench.pump();

    assert!(menu.checked(Command::ViewToolbar.id()));
    assert!(menu.checked(Command::ViewStatusBar.id()));
    assert!(toolbar.visible() && status.visible() && filter.hwnd().visible());
    let grid_before = grid(&bench).hwnd().screen_rect();

    bench.choose(Command::ViewToolbar);
    assert!(!menu.checked(Command::ViewToolbar.id()));
    assert!(!toolbar.visible() && !filter.hwnd().visible());
    bench.choose(Command::ViewStatusBar);
    assert!(!menu.checked(Command::ViewStatusBar.id()));
    assert!(!status.visible());
    // The grid has the room the bars gave up.
    let grid_without = grid(&bench).hwnd().screen_rect();
    assert!(grid_without.y < grid_before.y);
    assert!(grid_without.height > grid_before.height);

    bench.choose(Command::ViewToolbar);
    bench.choose(Command::ViewStatusBar);
    assert!(toolbar.visible() && status.visible() && filter.hwnd().visible());
    assert_eq!(grid(&bench).hwnd().screen_rect(), grid_before);

    // The filter is one thing, shown in two places.
    assert!(menu.checked(Command::ShowAll.id()));
    assert_eq!(filter.selected(), Some(Filter::All.index()));
    bench.choose(Command::ShowIdle);
    assert!(menu.checked(Command::ShowIdle.id()));
    assert!(!menu.checked(Command::ShowAll.id()));
    assert_eq!(filter.selected(), Some(Filter::Idle.index()));
    bench
        .opened
        .manager
        .with_model(|model| assert_eq!(model.lines.filter(), Filter::Idle));
}

#[test]
fn against_the_demo_service_the_window_shows_the_lines_and_carries_commands() {
    let name = pipe_name("demo");
    let _service = DemoService::start(&name, "frozen", 8);
    let bench = Bench::open(&name);
    assert!(
        bench.run_until(CONNECT, |bench| {
            connected(bench)
                && bench.opened.manager.with_model(|model| {
                    model.lines.len() == 8 && model.stats().is_some() && !model.log.is_empty()
                })
        }),
        "the manager did not connect to the demo service"
    );
    let window = bench.opened.window;
    // The answers to everything asked at the start are in.
    assert!(bench.run_until(CONNECT, |bench| {
        bench.opened.manager.requests_waiting() == 0
    }));
    // The title is the name of the program and nothing else.
    assert_eq!(window.text(), "RustModem Manager");

    // The grid shows what the model holds.
    let list = grid(&bench);
    assert!(list.hwnd().enabled());
    assert_eq!(list.row_count(), 8);
    for row in 0..8 {
        for column in Column::ALL {
            let expected = bench
                .opened
                .manager
                .with_model(|model| model.lines.cell(row, column, common::FROZEN_NOW_MS));
            assert_eq!(
                list.cell_text(row, column.index()),
                expected,
                "row {row}, {column:?}"
            );
        }
    }
    assert_eq!(list.cell_text(0, Column::Number.index()), "1");
    assert_eq!(list.cell_text(0, Column::State.index()), "Online");
    assert_eq!(list.cell_text(0, Column::Duration.index()), "3:11");
    assert_eq!(list.cell_text(7, Column::State.index()), "Port error");
    assert_eq!(list.cell_text(4, Column::Sip.index()), "\u{2014}");

    // The event log shows what the service logged.
    let log = RichEdit::of(window.item(ids::LOG));
    let lines = bench.opened.manager.with_model(|model| model.log.len());
    assert!(lines > 10, "{lines} lines");
    assert!(log.text().contains("RMD1005"), "{}", log.text());

    // With no line selected the line commands cannot be chosen.
    let menu = menu(&bench);
    assert!(!menu.enabled(Command::HangUp.id()));
    assert!(menu.enabled(Command::NewLine.id()));

    // Selecting a row selects its line, and the commands follow it.
    list.select(Some(0));
    bench.pump();
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.selected().map(|line| line.id), Some(1));
    });
    assert!(menu.enabled(Command::HangUp.id()));
    assert!(!menu.enabled(Command::Answer.id()));
    assert!(!menu.enabled(Command::Enable.id()));

    // Hang up: the service does it, and the grid follows.
    bench.choose(Command::HangUp);
    assert!(
        bench.run_until(CONNECT, |bench| {
            bench.opened.manager.with_model(|model| {
                model
                    .lines
                    .line(1)
                    .is_some_and(|line| line.state == LineState::Idle)
            })
        }),
        "line 1 did not go idle"
    );
    assert!(bench.run_until(CONNECT, |_| {
        list.cell_text(0, Column::State.index()).starts_with("Idle")
    }));
    // The selection is still on line 1, which now has no call.
    assert_eq!(list.selected(), Some(0));
    assert!(!menu.enabled(Command::HangUp.id()));

    // Disable and enable.
    bench.choose(Command::Disable);
    assert!(bench.run_until(CONNECT, |_| {
        list.cell_text(0, Column::State.index()) == "Disabled"
    }));
    assert!(menu.enabled(Command::Enable.id()));
    assert!(!menu.enabled(Command::Disable.id()));

    // The filter takes the row away and the selection with it; the line
    // is still selected when the filter lets it back in.
    bench.choose(Command::ShowOnCall);
    assert!(list.row_count() < 8);
    assert_eq!(list.selected(), None);
    bench.choose(Command::ShowAll);
    assert_eq!(list.row_count(), 8);
    assert_eq!(list.selected(), Some(0));

    // Sorting by a column turns the rows round on the second click.
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.sort().column, Column::Number);
        assert!(!model.lines.sort().descending);
    });
}
