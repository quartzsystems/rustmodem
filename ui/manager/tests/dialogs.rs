// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The dialogs and the actions, against the demo service: each dialog
//! opens, the line dialogs add and change lines, a refusal keeps the
//! dialog open, and the exports write their files. Ports are made and
//! removed by something that stands in for the setup program, which would
//! ask for an administrator.

#![cfg(windows)]

mod common;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use common::{Bench, DemoService, close, frame, pipe_name};
use manager::setup::Outcome;
use manager::{
    Hooks, ID_REMOVE_LINE_ONLY, ID_REMOVE_WITH_PORT, has_handler, ids, line_ids, ports_ids,
    settings_ids,
};
use model::{Command, ConnectionState, MenuEntry};
use win32::controls::Combo;
use win32::{Hwnd, ListView, Menu, TaskDialog, code, id};

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
    /// The port commands the setup program would have been run with.
    setup: RefCell<Vec<Vec<String>>>,
    /// How the setup program ends; done, if nothing says otherwise.
    setup_outcome: RefCell<Option<Outcome>>,
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
/// It has no setup program, as a manager that was not installed has
/// none.
fn connected(test: &str, lines: u32) -> (DemoService, Bench, Rc<Seen>) {
    connected_as(test, lines, false)
}

/// The same, with something standing in for the setup program: it makes
/// and removes nothing, and says what it is told to say.
fn connected_with_setup(test: &str, lines: u32) -> (DemoService, Bench, Rc<Seen>) {
    connected_as(test, lines, true)
}

fn connected_as(test: &str, lines: u32, setup: bool) -> (DemoService, Bench, Rc<Seen>) {
    let name = pipe_name(test);
    let service = DemoService::start(&name, "frozen", lines);
    let seen = Rc::new(Seen::default());
    seen.answer.set(id::CLOSE);
    let mut hooks = hooks(&seen);
    if setup {
        let seen = Rc::clone(&seen);
        hooks.setup = Some(Rc::new(move |words: &[String]| {
            seen.setup.borrow_mut().push(words.to_vec());
            seen.setup_outcome.borrow().clone().unwrap_or(Outcome::Done)
        }));
    }
    let bench = Bench::open_with(&name, hooks);
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

/// What the line dialog's first combo box offers.
const CREATE: &str = "RustModem: create a new port";
const EXISTING: &str = "RustModem: use an existing port";
const PAIRS: &str = "com0com: use a port of a pair";

/// What the line dialog's first combo box has chosen.
fn source(dialog: Hwnd) -> String {
    let sources = Combo::of(dialog.item(line_ids::BACKEND));
    sources
        .selected()
        .map(|index| sources.entry(index))
        .unwrap_or_default()
}

/// Does `act` with a line dialog once it offers the ports of `wanted`.
/// The dialog is told of the choice as its combo box would tell it, and
/// says that it is up again when it has listed the ports.
fn with_source(wanted: &'static str, act: impl Fn(&str, Hwnd) + 'static) -> WithDialog {
    Box::new(move |name, dialog| {
        if source(dialog) != wanted {
            let sources = Combo::of(dialog.item(line_ids::BACKEND));
            let index = (0..sources.count())
                .find(|index| sources.entry(*index) == wanted)
                .unwrap_or_else(|| panic!("{wanted} is not offered"));
            sources.select(Some(index));
            dialog.command_code(line_ids::BACKEND, code::COMBO_SELECTION);
            return;
        }
        act(name, dialog)
    })
}

/// The row of a port in the list of Ports and drivers.
fn port_row(list: ListView, port: &str) -> usize {
    (0..list.row_count())
        .find(|row| list.cell_text(*row, 0) == port)
        .unwrap_or_else(|| panic!("{port} is not listed"))
}

/// What the list of Ports and drivers shows of a port.
fn port_cells(list: ListView, port: &str) -> Vec<String> {
    let row = port_row(list, port);
    (0..list.column_count())
        .map(|column| list.cell_text(row, column))
        .collect()
}

/// Adds line 5 on the demo machine's first RustModem port.
fn add_line_on_com40(bench: &Bench, seen: &Seen) {
    *seen.with_dialog.borrow_mut() = Some(with_source(EXISTING, |_, dialog| {
        let ports = Combo::of(dialog.item(line_ids::PORT));
        assert_eq!(ports.entry(0), "COM40 (RustModem Softmodem)");
        ports.set_text(&ports.entry(0));
        dialog.command(id::OK);
    }));
    bench.choose(Command::NewLine);
    *seen.with_dialog.borrow_mut() = None;
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
    bench.opened.manager.with_model(|model| {
        let line = model.lines.line(5).expect("line 5 was added");
        assert_eq!(
            (line.backend.as_str(), line.port.as_str()),
            ("vcom", "COM40")
        );
    });
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
        (Command::TrunkPeers, "trunk-peers"),
        (Command::HuntGroups, "hunt-groups"),
        (Command::Routes, "routes"),
        (Command::CallCapture, "capture"),
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
    *seen.with_dialog.borrow_mut() = Some(with_source(PAIRS, |name, dialog| {
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
        // The line is on a pair, and no port is made for a line there is.
        assert_eq!(source(dialog), PAIRS);
        assert_eq!(Combo::of(dialog.item(line_ids::BACKEND)).count(), 2);
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
    *seen.with_dialog.borrow_mut() = Some(with_source(PAIRS, |_, dialog| {
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
    *seen.with_dialog.borrow_mut() = Some(with_source(PAIRS, |_, dialog| {
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
        // A pair is not the manager's to remove.
        assert!(tasks[0].links.is_empty());
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
fn a_line_takes_a_rustmodem_port_there_is() {
    let (_service, bench, seen) = connected("vcom-line", 4);

    // With no setup program no port can be made: the ports there are
    // come first, and then the pairs.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, dialog| {
        assert_eq!(name, "new-line");
        let sources = Combo::of(dialog.item(line_ids::BACKEND));
        assert_eq!(sources.count(), 2);
        assert_eq!(source(dialog), EXISTING);
        let ports = Combo::of(dialog.item(line_ids::PORT));
        assert_eq!(ports.count(), 2);
        assert_eq!(ports.entry(0), "COM40 (RustModem Softmodem)");
        assert_eq!(ports.entry(1), "COM41 (RustModem Softmodem #2)");
        assert!(dialog.item(line_ids::PORT).enabled());
        ports.set_text(&ports.entry(1));
        dialog.command(id::OK);
    }));
    bench.choose(Command::NewLine);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
    bench.opened.manager.with_model(|model| {
        let line = model.lines.line(5).expect("line 5 was added");
        assert_eq!(line.backend, "vcom");
        assert_eq!(line.port, "COM41");
        // One port with one name: the binding is the modem Windows has
        // on it.
        assert_eq!(line.binding.as_deref(), Some("RustModem Softmodem #2"));
    });

    // Edit line shows it as it is, and changing nothing sends nothing.
    select_line(&bench, 5);
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, dialog| {
        assert_eq!(name, "edit-line");
        assert_eq!(source(dialog), EXISTING);
        assert_eq!(Combo::of(dialog.item(line_ids::PORT)).text(), "COM41");
        dialog.command(id::OK);
    }));
    bench.choose(Command::EditLine);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());

    // Ports and drivers lists the ports with their modems, the driver
    // and what each is doing. Nothing is made or removed here.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, dialog| {
        assert_eq!(name, "ports");
        let list = ListView::of(dialog.item(ports_ids::LIST));
        assert_eq!(
            port_cells(list, "COM41"),
            [
                "COM41",
                "RustModem",
                model::EMPTY,
                model::EMPTY,
                "RustModem Softmodem #2",
                "0.2.0.0",
                "In use",
                "5"
            ]
        );
        assert_eq!(port_cells(list, "COM40")[6], "Free");
        assert_eq!(port_cells(list, "CNCB0")[1], "com0com");
        list.select(Some(port_row(list, "COM40")));
        assert!(!dialog.item(ports_ids::ADD).enabled());
        assert!(!dialog.item(ports_ids::REMOVE).enabled());
        close(name, dialog);
    }));
    bench.choose(Command::PortsAndDrivers);
    *seen.with_dialog.borrow_mut() = None;

    // Remove line cannot remove the port either, and does not offer to.
    seen.answer.set(id::YES);
    bench.choose(Command::RemoveLine);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert!(tasks[0].content.contains("COM41"));
        assert!(tasks[0].links.is_empty());
    }
    assert!(bench.run_until(CONNECT, |bench| line_count(bench) == 4));
}

#[test]
fn ports_are_made_and_removed_by_the_setup_program() {
    let (_service, bench, seen) = connected_with_setup("ports", 4);

    // Remove port is for a RustModem port that no line uses.
    let step = Rc::new(Cell::new(0));
    *seen.with_dialog.borrow_mut() = Some(Box::new({
        let (seen, step) = (Rc::clone(&seen), Rc::clone(&step));
        move |name, dialog| {
            assert_eq!(name, "ports");
            let list = ListView::of(dialog.item(ports_ids::LIST));
            step.set(step.get() + 1);
            match step.get() {
                1 => {
                    assert!(dialog.item(ports_ids::ADD).enabled());
                    assert!(!dialog.item(ports_ids::REMOVE).enabled());
                    list.select(Some(port_row(list, "CNCB5")));
                    assert!(!dialog.item(ports_ids::REMOVE).enabled());
                    list.select(Some(port_row(list, "COM41")));
                    assert!(dialog.item(ports_ids::REMOVE).enabled());
                    dialog.command(ports_ids::REMOVE);
                }
                2 => {
                    assert_eq!(*seen.setup.borrow(), [["port", "remove", "COM41"]]);
                    dialog.command(ports_ids::ADD);
                }
                _ => {
                    assert_eq!(seen.setup.borrow().len(), 2);
                    assert_eq!(seen.setup.borrow()[1], ["port", "add"]);
                    close(name, dialog);
                }
            }
        }
    }));
    bench.choose(Command::PortsAndDrivers);
    assert_eq!(step.get(), 3);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());

    // What the setup program says when it fails is what is shown.
    let refused = ctlproto::ErrorInfo::from(msg::message("RMD9011", &[]));
    *seen.setup_outcome.borrow_mut() = Some(Outcome::Failed(refused.clone()));
    seen.setup.borrow_mut().clear();
    step.set(0);
    *seen.with_dialog.borrow_mut() = Some(Box::new({
        let step = Rc::clone(&step);
        move |name, dialog| {
            step.set(step.get() + 1);
            if step.get() == 1 {
                dialog.command(ports_ids::ADD);
            } else {
                close(name, dialog);
            }
        }
    }));
    bench.choose(Command::PortsAndDrivers);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "RMD9011");
        assert_eq!(tasks[0].instruction, refused.text);
    }
    seen.tasks.borrow_mut().clear();

    // The person says no to Windows: nothing is said.
    *seen.setup_outcome.borrow_mut() = Some(Outcome::Cancelled);
    step.set(0);
    bench.choose(Command::PortsAndDrivers);
    assert_eq!(seen.setup.borrow().len(), 2);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
}

#[test]
fn a_new_line_has_a_port_made_for_it() {
    let (_service, bench, seen) = connected_with_setup("create", 4);

    // Where ports can be made, making one is what a new line starts
    // with, and there is no port to choose.
    // What is typed is looked at before a port is made for it.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, dialog| {
        assert_eq!(name, "new-line");
        let sources = Combo::of(dialog.item(line_ids::BACKEND));
        assert_eq!(
            (0..sources.count())
                .map(|index| sources.entry(index))
                .collect::<Vec<_>>(),
            [CREATE, EXISTING, PAIRS]
        );
        assert_eq!(source(dialog), CREATE);
        assert!(!dialog.item(line_ids::PORT).enabled());
        assert_eq!(Combo::of(dialog.item(line_ids::PORT)).count(), 0);
        dialog.item(line_ids::RINGS).set_text("999");
        dialog.command(id::OK);
    }));
    bench.choose(Command::NewLine);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "RMD4008");
    }
    assert!(seen.setup.borrow().is_empty());
    seen.tasks.borrow_mut().clear();

    // The person says no to Windows: the dialog stays, and nothing is
    // said. It is closed here by the Cancel that comes after the OK.
    *seen.setup_outcome.borrow_mut() = Some(Outcome::Cancelled);
    *seen.with_dialog.borrow_mut() = Some(Box::new(|_, dialog| {
        dialog.command(id::OK);
        dialog.command(id::CANCEL);
    }));
    bench.choose(Command::NewLine);
    assert_eq!(*seen.setup.borrow(), [["port", "add"]]);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());
    assert_eq!(line_count(&bench), 4);

    // The setup program says it made a port, and the demo service has
    // none that it did not have: no line is added on a port that is
    // not there.
    *seen.setup_outcome.borrow_mut() = None;
    *seen.with_dialog.borrow_mut() = Some(Box::new(|_, dialog| {
        dialog.command(id::OK);
    }));
    bench.choose(Command::NewLine);
    assert_eq!(seen.setup.borrow().len(), 2);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "RMD9014");
    }
    assert_eq!(line_count(&bench), 4);
    seen.tasks.borrow_mut().clear();

    // A port there is can be chosen all the same.
    seen.setup.borrow_mut().clear();
    add_line_on_com40(&bench, &seen);
    assert!(seen.setup.borrow().is_empty());
}

#[test]
fn removing_a_line_offers_to_remove_its_port() {
    let (_service, bench, seen) = connected_with_setup("remove-port", 4);
    add_line_on_com40(&bench, &seen);
    select_line(&bench, 5);

    // Cancel: nothing happens.
    seen.answer.set(id::CANCEL);
    bench.choose(Command::RemoveLine);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].title, "Remove line");
        assert_eq!(tasks[0].instruction, "Remove line 5?");
        assert_eq!(tasks[0].default, Some(id::CANCEL));
        let links: Vec<u16> = tasks[0].links.iter().map(|(id, _)| *id).collect();
        assert_eq!(links, [ID_REMOVE_WITH_PORT, ID_REMOVE_LINE_ONLY]);
        assert!(
            tasks[0]
                .links
                .iter()
                .all(|(_, text)| text.contains("COM40"))
        );
    }
    bench.run_for(Duration::from_millis(300));
    assert_eq!(line_count(&bench), 5);
    assert!(seen.setup.borrow().is_empty());
    seen.tasks.borrow_mut().clear();

    // The line only: the port stays.
    seen.answer.set(ID_REMOVE_LINE_ONLY);
    bench.choose(Command::RemoveLine);
    assert!(bench.run_until(CONNECT, |bench| line_count(bench) == 4));
    assert!(seen.setup.borrow().is_empty());
    seen.tasks.borrow_mut().clear();

    // The line and its port: the line goes first, and then the port.
    add_line_on_com40(&bench, &seen);
    select_line(&bench, 5);
    seen.answer.set(ID_REMOVE_WITH_PORT);
    bench.choose(Command::RemoveLine);
    assert_eq!(line_count(&bench), 4);
    assert_eq!(*seen.setup.borrow(), [["port", "remove", "COM40"]]);
    assert_eq!(seen.tasks.borrow().len(), 1);
    seen.tasks.borrow_mut().clear();

    // A port that could not be removed is said, and the line is gone
    // all the same.
    add_line_on_com40(&bench, &seen);
    select_line(&bench, 5);
    let refused = ctlproto::ErrorInfo::from(msg::message("RMD9009", &["COM40", "it is open"]));
    *seen.setup_outcome.borrow_mut() = Some(Outcome::Failed(refused));
    bench.choose(Command::RemoveLine);
    assert_eq!(line_count(&bench), 4);
    {
        let tasks = seen.tasks.borrow();
        assert_eq!(tasks.len(), 2, "{tasks:?}");
        assert_eq!(tasks[1].title, "RMD9009");
    }
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

    // A change to the service's own settings is sent and is in effect
    // at once: nothing is offered.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, page| {
        Combo::of(page.item(settings_ids::LEVEL)).select(Some(3));
        frame(name, page).command(id::OK);
    }));
    bench.choose(Command::Settings);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.tasks.borrow());

    // A change to the control pipe takes effect when the service next
    // starts, and the manager offers to restart it. Later: nothing more.
    seen.answer.set(902);
    *seen.with_dialog.borrow_mut() = Some(Box::new(|name, page| {
        let sheet = page.parent().expect("a page is in its sheet");
        if name == "settings" {
            // The pipe is on the second page.
            sheet.select_page(1);
        } else {
            assert_eq!(name, "settings-control");
            page.item(settings_ids::PIPE).set_text("rustmodem-other");
            sheet.command(id::OK);
        }
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
        let sheet = page.parent().expect("a page is in its sheet");
        if name == "settings" {
            assert_eq!(Combo::of(page.item(settings_ids::LEVEL)).text(), "Debug");
            sheet.select_page(1);
        } else {
            assert_eq!(page.item(settings_ids::PIPE).text(), "rustmodem-other");
            sheet.command(id::CANCEL);
        }
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
