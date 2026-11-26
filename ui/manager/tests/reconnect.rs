// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The reconnect test: the demo service is killed and started again
//! under the manager, which shows that it is disconnected and then
//! recovers, with the grid correct.

#![cfg(windows)]

mod common;

use std::time::Duration;

use common::{Bench, DemoService, FROZEN_NOW_MS, fresh_lines, pipe_name};
use manager::ids;
use model::{Column, Command, ConnectionState};
use win32::{ListView, Menu};

/// Long enough for the demo service to start and the manager, which
/// tries every three seconds, to find it.
const CONNECT: Duration = Duration::from_secs(30);

fn connected_with(bench: &Bench, lines: usize) -> bool {
    bench
        .opened
        .manager
        .with_model(|model| model.connected() && model.lines.len() == lines)
        && bench.opened.manager.requests_waiting() == 0
}

/// The grid shows, cell for cell, what the model holds.
fn assert_grid_is_the_model(bench: &Bench, list: ListView) {
    let rows = bench
        .opened
        .manager
        .with_model(|model| model.lines.row_count());
    assert_eq!(list.row_count(), rows);
    for row in 0..rows {
        for column in Column::ALL {
            let expected = bench
                .opened
                .manager
                .with_model(|model| model.lines.cell(row, column, FROZEN_NOW_MS));
            assert_eq!(
                list.cell_text(row, column.index()),
                expected,
                "row {row}, {column:?}"
            );
        }
    }
}

#[test]
fn a_service_that_is_killed_and_started_again_is_lost_and_found() {
    let name = pipe_name("again");
    let mut service = DemoService::start(&name, "frozen", 4);
    let bench = Bench::open(&name);
    let window = bench.opened.window;
    let list = ListView::of(window.item(ids::LIST));
    let menu = Menu::of_window(window).expect("the window has a menu bar");
    assert!(
        bench.run_until(CONNECT, |bench| connected_with(bench, 4)),
        "the manager did not connect to the demo service"
    );
    assert!(bench.run_until(CONNECT, |_| list.row_count() == 4));
    assert_grid_is_the_model(&bench, list);
    assert!(list.hwnd().enabled());
    list.select(Some(2));
    bench.pump();
    assert!(menu.enabled(Command::Reset.id()));

    // The service dies. The window stays up, says why it has no service,
    // and the grid and the line commands take no part.
    service.kill();
    assert!(
        bench.run_until(CONNECT, |bench| {
            bench.opened.manager.with_model(|model| !model.connected())
        }),
        "the manager did not notice that the service was gone"
    );
    bench.opened.manager.with_model(|model| {
        let ConnectionState::Disconnected(Some(reason)) = model.connection() else {
            panic!("the manager is {:?}", model.connection());
        };
        assert!(reason.msgid.starts_with("RMD2"), "{reason}");
        assert_eq!(model.status_message(), reason.to_string());
    });
    assert!(window.exists());
    assert_eq!(window.text(), "RustModem Manager");
    assert!(!list.hwnd().enabled());
    for command in [
        Command::Answer,
        Command::HangUp,
        Command::Reset,
        Command::Enable,
        Command::Disable,
        Command::NewLine,
        Command::EditLine,
        Command::RemoveLine,
        Command::ReloadConfig,
    ] {
        assert!(!menu.enabled(command.id()), "{command:?}");
    }
    // It goes on trying while there is nothing to find.
    bench.run_for(Duration::from_millis(3500));
    bench.opened.manager.with_model(|model| {
        assert!(matches!(
            model.connection(),
            ConnectionState::Disconnected(Some(_))
        ));
    });

    // The service comes back, with other lines. The manager finds it by
    // itself and the grid is the new service's.
    let _again = DemoService::start(&name, "frozen", 6);
    assert!(
        bench.run_until(CONNECT, |bench| connected_with(bench, 6)),
        "the manager did not find the service again"
    );
    assert!(bench.run_until(CONNECT, |_| list.row_count() == 6));
    assert!(list.hwnd().enabled());
    assert_grid_is_the_model(&bench, list);
    assert!(menu.enabled(Command::NewLine.id()));
    assert_eq!(window.text(), "RustModem Manager");
    bench.opened.manager.with_model(|model| {
        let title = model.lines.title();
        assert!(title.contains("6 configured"), "{title}");
    });
    // The model is what a client that has just connected is told.
    let fresh = fresh_lines(&name);
    bench.opened.manager.with_model(|model| {
        let held: Vec<_> = model.lines.lines().cloned().collect();
        assert_eq!(held, fresh);
    });
    // The line that was selected is selected still: it is the same id.
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.selected().map(|line| line.id), Some(3));
    });
    assert_eq!(list.selected(), Some(2));
}
