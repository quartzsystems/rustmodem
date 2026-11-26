// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The dialogs and the actions, against the demo service: each dialog
//! opens, the line dialogs add and change lines, a refusal keeps the
//! dialog open, and the exports write their files.

#![cfg(windows)]

mod common;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use common::{Bench, DemoService, close, frame, pipe_name};
use manager::{Hooks, has_handler, ids, line_ids, settings_ids};
use model::{Command, ConnectionState, MenuEntry};
use win32::controls::Combo;
use win32::{Hwnd, ListView, Menu, TaskDialog, id};

const CONNECT: Duration = Duration::from_secs(30);

/// What a test does with a dialog that has come up: its name and its
/// window.
type WithDialog = Box<dyn Fn(&str, Hwnd)>;

/// What the hooks saw and what they are to do.
#[derive(Default)]
struct Seen {
    /// The names of the dialogs that came up, in order.
    dialogs: RefCell<Vec<String>>,
    /// The task dialogs that would have been shown.
    tasks: RefCell<Vec<TaskDialog>>,
    /// The button a task dialog is answered with.
    answer: Cell<u16>,
    /// What to do with a dialog that comes up; without it the dialog is
    /// closed.
    with_dialog: RefCell<Option<WithDialog>>,
    /// The dialog that is up.
    current: Cell<Option<Hwnd>>,
    /// The file the file dialogs answer with.
    file: RefCell<Option<PathBuf>>,
}

fn hooks(seen: &Rc<Seen>) -> Hooks {
    let dialog = {
        let seen = Rc::clone(seen);
        move |name: &str, window: Hwnd| {
            seen.dialogs.borrow_mut().push(name.to_string());
            seen.current.set(Some(frame(name, window)));
            match seen.with_dialog.borrow().as_ref() {
                Some(act) => act(name, window),
                None => close(name, window),
            }
        }
    };
    let task = {
        let seen = Rc::clone(seen);
        move |dialog: &TaskDialog| {
            seen.tasks.borrow_mut().push(dialog.clone());
            // A dialog that was refused something is still up: close it
            // once the refusal has been taken in.
            if let Some(window) = seen.current.get()
                && window.exists()
                && dialog.title.starts_with("RMD")
            {
                window.command(id::CANCEL);
            }
            seen.answer.get()
        }
    };
    let file = {
        let seen = Rc::clone(seen);
        move |_title: &str| seen.file.borrow().clone()
    };
    Hooks {
        dialog: Some(Rc::new(dialog)),
        task: Some(Rc::new(task)),
        file: Some(Rc::new(file)),
        ..Hooks::default()
    }
}

/// A manager connected to the frozen demo service, with the hooks in.
fn connected(test: &str, lines: u32) -> (DemoService, Bench, Rc<Seen>) {
    let name = pipe_name(test);
    let service = DemoService::start(&name, "frozen", lines);
    let seen = Rc::new(Seen::default());
    seen.answer.set(id::CLOSE);
    let bench = Bench::open_with(&name, hooks(&seen));
    assert!(
        bench.run_until(CONNECT, |bench| {
            bench
                .opened
                .manager
                .with_model(|model| model.connected() && model.lines.len() == lines as usize)
                && bench.opened.manager.requests_waiting() == 0
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
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.selected().map(|line| line.id), Some(id));
    });
}

fn line_count(bench: &Bench) -> usize {
    bench.opened.manager.with_model(|model| model.lines.len())
}

#[test]
fn every_command_that_is_built_has_something_that_carries_it_out() {
    for command in Command::ALL {
        assert_eq!(
            has_handler(command),
            command.blocked().is_none(),
            "{command:?}"
        );
    }
}

#[test]
fn no_menu_item_is_enabled_without_a_handler() {
    let (_service, bench, _seen) = connected("walk", 8);
    let menu = Menu::of_window(bench.opened.window).expect("the window has a menu bar");

    // The menu tree, walked: every item of every menu and submenu.
    let walk = |when: &str| {
        let mut walked = 0;
        for (title, entries) in model::menu_bar() {
            let commands: Vec<Command> = entries
                .into_iter()
                .flat_map(|entry| match entry {
                    MenuEntry::Item(command) => vec![command],
                    MenuEntry::Separator => Vec::new(),
                    MenuEntry::Submenu(_, commands) => commands,
                })
                .collect();
            for command in commands {
                walked += 1;
                if menu.enabled(command.id()) {
                    assert!(
                        has_handler(command),
                        "{when}: {command:?} in {title} is enabled and has no handler"
                    );
                } else {
                    // A disabled item says why in the status bar.
                    let help = bench.opened.manager.with_model(|model| model.help(command));
                    assert!(!help.is_empty(), "{when}: {command:?}");
                    assert_ne!(help, command.description(), "{when}: {command:?}");
                }
            }
        }
        assert_eq!(walked, Command::ALL.len());
    };

    walk("connected, no line selected");
    // A line with a call, a line that rings, an idle line, a disabled
    // line and a line whose port will not open.
    for line in [1, 4, 5, 7, 8] {
        select_line(&bench, line);
        walk(&format!("connected, line {line} selected"));
    }
    bench.choose(Command::Disconnect);
    bench.opened.manager.with_model(|model| {
        assert_eq!(*model.connection(), ConnectionState::Disconnected(None));
    });
    walk("disconnected");
}

#[test]
fn every_dialog_opens() {
    let (_service, bench, seen) = connected("dialogs", 8);
    select_line(&bench, 5);
    for (command, name) in [
        (Command::Connect, "connect"),
        (Command::NewLine, "new-line"),
        (Command::EditLine, "edit-line"),
        (Command::CallHistory, "history"),
        (Command::Settings, "settings"),
        (Command::PortsAndDrivers, "ports"),
        (Command::About, "about"),
    ] {
        bench.choose(command);
        // A modal dialog has come and gone by the time the command
        // returns.
        assert_eq!(
            seen.dialogs.borrow().last().map(String::as_str),
            Some(name),
            "{command:?}"
        );
        assert!(seen.current.get().is_some_and(|window| !window.exists()));
    }

    // The AT console stays open beside the window until it is closed.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|_, _| {}));
    bench.choose(Command::AtConsole);
    let console = bench
        .opened
        .manager
        .console_window()
        .expect("the console is open");
    assert!(bench.run_until(CONNECT, |_| {
        seen.dialogs.borrow().last().map(String::as_str) == Some("console")
    }));
    // Asked for again, it is the same one.
    bench.choose(Command::AtConsole);
    assert_eq!(bench.opened.manager.console_window(), Some(console));
    console.end_dialog(id::CANCEL);
    bench.pump();
    assert!(!console.exists());
    assert_eq!(bench.opened.manager.console_window(), None);

    // Nothing was said that was not a dialog of the manager's own.
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
    assert_eq!(line_count(&bench), 8);
}

#[test]
fn a_line_is_added_changed_and_removed() {
    let (_service, bench, seen) = connected("lines", 4);

    // New line: the id is the lowest free one; a port is typed.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, dialog| {
        assert_eq!(name, "new-line");
        assert_eq!(dialog.item(line_ids::NUMBER).text(), "5");
        // The ports offered are the free ends of the spare pairs.
        let ports = Combo::of(dialog.item(line_ids::PORT));
        assert!(ports.count() > 0);
        assert!(ports.entry(0).starts_with("CNCB4"), "{}", ports.entry(0));
        ports.set_text(&ports.entry(0));
        dialog.item(line_ids::NAME).set_text("Back office");
        dialog.command(id::OK);
    }));
    bench.choose(Command::NewLine);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
    assert_eq!(line_count(&bench), 5);
    bench.opened.manager.with_model(|model| {
        let line = model.lines.line(5).expect("line 5 was added");
        assert_eq!(line.port, "CNCB4");
        assert_eq!(line.name.as_deref(), Some("Back office"));
        assert!(line.enabled);
    });
    let list = ListView::of(bench.opened.window.item(ids::LIST));
    assert_eq!(list.row_count(), 5);

    // Edit line: the id cannot be changed; the name and the rings are.
    select_line(&bench, 5);
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, dialog| {
        assert_eq!(name, "edit-line");
        assert_eq!(dialog.item(line_ids::NUMBER).text(), "5");
        assert!(!dialog.item(line_ids::NUMBER).enabled());
        assert_eq!(dialog.item(line_ids::NAME).text(), "Back office");
        assert_eq!(Combo::of(dialog.item(line_ids::PORT)).text(), "CNCB4");
        dialog.item(line_ids::NAME).set_text("Front desk");
        dialog.item(line_ids::RINGS).set_text("2");
        dialog.command(id::OK);
    }));
    bench.choose(Command::EditLine);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
    bench.opened.manager.with_model(|model| {
        let line = model.lines.line(5).expect("line 5 is there");
        assert_eq!(line.name.as_deref(), Some("Front desk"));
        assert_eq!(line.auto_answer_rings, 2);
        assert_eq!(line.port, "CNCB4");
    });

    // A port that a line already has: the service refuses, the message
    // is shown, and the dialog stays open with what was typed.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|_, dialog| {
        Combo::of(dialog.item(line_ids::PORT)).set_text("CNCB0");
        dialog.item(line_ids::NAME).set_text("Twice");
        dialog.command(id::OK);
    }));
    bench.choose(Command::NewLine);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "RMD0017");
        assert!(
            tasks[0].instruction.contains("CNCB0"),
            "{}",
            tasks[0].instruction
        );
        assert!(tasks[0].expanded.contains("Cause: "));
        assert!(tasks[0].expanded.contains("Recovery: "));
    }
    assert_eq!(line_count(&bench), 5);
    seen.tasks.borrow_mut().clear();

    // What is typed is checked before it is sent.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|_, dialog| {
        dialog.item(line_ids::RINGS).set_text("999");
        Combo::of(dialog.item(line_ids::PORT)).set_text("CNCB6");
        dialog.command(id::OK);
    }));
    bench.choose(Command::NewLine);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "RMD4008");
        assert_eq!(
            tasks[0].instruction,
            "Auto-answer rings must be a whole number from 0 to 255."
        );
    }
    assert_eq!(line_count(&bench), 5);
    seen.tasks.borrow_mut().clear();
    *seen.with_dialog.borrow_mut() = None;

    // Remove line asks first, naming the line. No: nothing happens.
    select_line(&bench, 5);
    seen.answer.set(id::NO);
    bench.choose(Command::RemoveLine);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Remove line");
        assert_eq!(tasks[0].instruction, "Remove line 5 (Front desk)?");
        assert!(tasks[0].content.contains("CNCB4"));
        assert_eq!(tasks[0].default, Some(id::NO));
    }
    bench.run_for(Duration::from_millis(300));
    assert_eq!(line_count(&bench), 5);

    // Yes: the line goes.
    seen.answer.set(id::YES);
    bench.choose(Command::RemoveLine);
    assert!(bench.run_until(CONNECT, |bench| line_count(bench) == 4));
    assert!(bench.run_until(CONNECT, |_| list.row_count() == 4));
    bench.opened.manager.with_model(|model| {
        assert!(model.lines.line(5).is_none());
        assert_eq!(model.lines.selected(), None);
    });

    // A line in a call cannot be removed: the model says no before
    // anything is asked.
    seen.tasks.borrow_mut().clear();
    select_line(&bench, 1);
    let menu = Menu::of_window(bench.opened.window).expect("the window has a menu bar");
    assert!(!menu.enabled(Command::RemoveLine.id()));
    bench.choose(Command::RemoveLine);
    assert!(seen.tasks.borrow().is_empty());
    assert_eq!(line_count(&bench), 4);
}

#[test]
fn settings_are_sent_and_a_restart_is_offered() {
    let (_service, bench, seen) = connected("settings", 2);

    // OK with nothing changed sends nothing and offers nothing.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, page| {
        assert_eq!(name, "settings");
        let level = Combo::of(page.item(settings_ids::LEVEL));
        assert_eq!(level.count(), 4);
        assert_eq!(level.text(), "Info");
        frame(name, page).command(id::OK);
    }));
    bench.choose(Command::Settings);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());

    // A change is sent; it takes effect when the service next starts,
    // and the manager offers to restart it. Later: nothing more.
    seen.answer.set(902);
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, page| {
        Combo::of(page.item(settings_ids::LEVEL)).select(Some(3));
        frame(name, page).command(id::OK);
    }));
    bench.choose(Command::Settings);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "RMD4011");
        assert_eq!(tasks[0].links.len(), 2);
        assert!(tasks[0].links[0].1.starts_with("Restart service now"));
        assert!(tasks[0].links[1].1.starts_with("Later"));
        assert!(tasks[0].expanded.contains("rustmodem-demo.toml"));
    }
    seen.tasks.borrow_mut().clear();

    // The sheet opens again with what was set.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, page| {
        assert_eq!(Combo::of(page.item(settings_ids::LEVEL)).text(), "Debug");
        frame(name, page).command(id::CANCEL);
    }));
    bench.choose(Command::Settings);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
}

#[test]
fn the_exports_write_their_files() {
    let (_service, bench, seen) = connected("exports", 8);
    let folder = std::env::temp_dir().join(pipe_name("exports"));
    std::fs::create_dir_all(&folder).expect("the folder can be made");

    let statistics = folder.join("statistics.csv");
    *seen.file.borrow_mut() = Some(statistics.clone());
    bench.choose(Command::ExportStatistics);
    let text = std::fs::read_to_string(&statistics).expect("the statistics were written");
    assert!(text.starts_with('\u{FEFF}'), "{text}");
    assert!(text.contains("Calls answered,7"), "{text}");
    assert!(text.contains("2400 V.22bis"), "{text}");

    let log = folder.join("events.txt");
    *seen.file.borrow_mut() = Some(log.clone());
    bench.choose(Command::ExportLog);
    let text = std::fs::read_to_string(&log).expect("the log was written");
    let lines = bench.opened.manager.with_model(|model| model.log.len());
    assert_eq!(text.lines().count(), lines);
    assert!(text.starts_with("2027-01-15 "), "{text}");
    assert!(text.contains("RMD1005"), "{text}");

    // Thinking better of it writes nothing and says nothing.
    *seen.file.borrow_mut() = None;
    bench.choose(Command::ExportStatistics);
    assert!(seen.tasks.borrow().is_empty());

    // A file that cannot be written is a message.
    *seen.file.borrow_mut() = Some(folder.join("no such folder").join("statistics.csv"));
    bench.choose(Command::ExportStatistics);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "RMD4002");
        assert!(tasks[0].instruction.contains("no such folder"));
    }
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn connect_to_service_connects_to_the_pipe_that_is_named() {
    let name = pipe_name("first");
    let other = pipe_name("second");
    let _first = DemoService::start(&name, "frozen", 2);
    let _second = DemoService::start(&other, "frozen", 3);
    let seen = Rc::new(Seen::default());
    let bench = Bench::open_with(&name, hooks(&seen));
    assert!(bench.run_until(CONNECT, |bench| line_count(bench) == 2));
    assert_eq!(bench.opened.manager.pipe(), name);

    // Cancel: nothing changes.
    bench.choose(Command::Connect);
    assert_eq!(seen.dialogs.borrow().as_slice(), ["connect"]);
    assert_eq!(bench.opened.manager.pipe(), name);

    // The name of the other pipe, as a full path: the manager goes
    // there.
    let typed = format!(r"\\.\pipe\{other}");
    *seen.with_dialog.borrow_mut() = Some(Box::new(move |_, dialog| {
        // The pipe field is the second edit control: the computer field
        // is there and takes no part.
        let computer = dialog.item(300);
        assert!(!computer.enabled());
        assert_eq!(computer.text(), "(this computer)");
        dialog.item(301).set_text(&typed);
        dialog.command(id::OK);
    }));
    bench.choose(Command::Connect);
    assert_eq!(bench.opened.manager.pipe(), other);
    assert!(bench.run_until(CONNECT, |bench| line_count(bench) == 3));
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
}
