// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the manager shows and does of SIP: the SIP menu's dialogs, call
//! capture, the SIP and Media settings and the registration of a line
//! against the frozen demo service, and the grid, the status bar,
//! Re-register and the trace against one that takes calls over SIP.

#![cfg(windows)]

mod common;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use common::{Bench, DemoService, close, frame, pipe_name};
use ctlproto::{RegistrationState, SipMode};
use manager::{
    Hooks, capture_ids, group_ids, groups_ids, ids, line_ids, peers_ids, route_ids, routes_ids,
    telephony_ids, trace_ids,
};
use model::{Column, Command, EMPTY, Filter};
use win32::controls::{self, Combo};
use win32::{CheckList, Hwnd, ListBox, ListView, RichEdit, TaskDialog, code, id};

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
    /// The button a task dialog is answered with.
    answer: Cell<u16>,
    /// What to do with a dialog that is up; without it the dialog is
    /// closed.
    with_dialog: RefCell<Option<WithDialog>>,
    /// The dialog that is up.
    current: Cell<Option<Hwnd>>,
    /// The folders that would have been opened.
    folders: RefCell<Vec<PathBuf>>,
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
            // Every page of the settings is in the sheet, which is what
            // takes the commands.
            let shown = if name.starts_with("settings") {
                window.parent().expect("a page is in its sheet")
            } else {
                frame(name, window)
            };
            seen.current.set(Some(shown));
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
    let open_folder = {
        let seen = Rc::clone(seen);
        move |folder: &std::path::Path| seen.folders.borrow_mut().push(folder.to_path_buf())
    };
    Hooks {
        dialog: Some(Rc::new(dialog)),
        task: Some(Rc::new(task)),
        file: Some(Rc::new(|_: &str| None)),
        open_folder: Some(Rc::new(open_folder)),
        ..Hooks::default()
    }
}

/// A manager connected to a demo service, with the hooks in.
fn connected(test: &str, script: &str, lines: u32) -> (DemoService, Bench, Rc<Seen>) {
    let name = pipe_name(test);
    let service = DemoService::start(&name, script, lines);
    let seen = Rc::new(Seen::default());
    seen.answer.set(id::CLOSE);
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

fn row_of(list: ListView, row: usize) -> Vec<String> {
    (0..list.column_count())
        .map(|column| list.cell_text(row, column))
        .collect()
}

#[test]
fn the_grid_shows_the_hunt_groups_and_narrows_to_one() {
    let (_service, bench, _seen) = connected("groups-grid", "frozen", 8);
    let window = bench.opened.window;
    assert_eq!(cell(&bench, 1, Column::HuntGroup), "dialup");
    assert_eq!(cell(&bench, 5, Column::HuntGroup), "night");
    // The frozen service has no SIP.
    assert_eq!(cell(&bench, 1, Column::Sip), EMPTY);

    // The filter lists the groups after its own entries.
    let filter = Combo::of(window.item(ids::FILTER));
    let entries: Vec<String> = (0..filter.count())
        .map(|index| filter.entry(index))
        .collect();
    assert_eq!(entries.len(), Filter::ALL.len() + 2);
    assert_eq!(entries[4], "Hunt group: dialup");
    assert_eq!(entries[5], "Hunt group: night");

    let list = ListView::of(window.item(ids::LIST));
    filter.select(Some(5));
    window.command_code(ids::FILTER, code::COMBO_SELECTION);
    bench.pump();
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.group(), Some("night"));
        assert_eq!(model.lines.filter(), Filter::All);
    });
    assert_eq!(list.row_count(), 4);
    assert_eq!(list.cell_text(0, 0), "5");

    // A filter of the View menu shows the lines of every group again.
    bench.choose(Command::ShowAll);
    bench.opened.manager.with_model(|model| {
        assert_eq!(model.lines.group(), None);
    });
    assert_eq!(list.row_count(), 8);
    assert_eq!(filter.selected(), Some(Filter::All.index()));
}

#[test]
fn the_trunk_peers_are_added_removed_and_refused() {
    let (_service, bench, seen) = connected("peers", "frozen", 4);

    seen.act(|name, _, dialog| {
        assert_eq!(name, "trunk-peers");
        let peers = ListBox::of(dialog.item(peers_ids::PEERS));
        assert_eq!(peers.count(), 0);
        assert!(!dialog.item(peers_ids::REMOVE).enabled());
        assert_eq!(dialog.item(peers_ids::OUTBOUND).text(), "");
        dialog.item(peers_ids::PEER).set_text("10.10.20.0/24");
        dialog.command(peers_ids::ADD);
        dialog
            .item(peers_ids::OUTBOUND)
            .set_text("10.10.20.11:5080");
        dialog.command(id::OK);
    });
    bench.choose(Command::TrunkPeers);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
    assert_eq!(seen.times("trunk-peers"), 1);

    // It is what the service has now. One more is added and the first
    // taken away.
    seen.act(|_, _, dialog| {
        let peers = ListBox::of(dialog.item(peers_ids::PEERS));
        assert_eq!(peers.count(), 1);
        assert_eq!(peers.entry(0), "10.10.20.0/24");
        assert_eq!(dialog.item(peers_ids::OUTBOUND).text(), "10.10.20.11:5080");
        // The first is taken away, and then one more is added.
        peers.select(Some(0));
        dialog.command_code(peers_ids::PEERS, code::LIST_SELECTED);
        dialog.command(peers_ids::REMOVE);
        dialog.item(peers_ids::PEER).set_text("192.168.1.5");
        dialog.command(peers_ids::ADD);
        dialog.command(id::OK);
    });
    bench.choose(Command::TrunkPeers);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());

    // An address that is none is the service's to refuse: its message is
    // shown and the dialog stays.
    seen.act(|_, _, dialog| {
        let peers = ListBox::of(dialog.item(peers_ids::PEERS));
        assert_eq!(peers.count(), 1);
        assert_eq!(peers.entry(0), "192.168.1.5");
        dialog.item(peers_ids::PEER).set_text("not an address");
        dialog.command(peers_ids::ADD);
        dialog.command(id::OK);
    });
    bench.choose(Command::TrunkPeers);
    assert_eq!(seen.task_titles(), ["RMD0014"]);
    seen.tasks.borrow_mut().clear();

    // Nothing of it was kept; and an empty address is not added.
    seen.act(|_, _, dialog| {
        assert_eq!(ListBox::of(dialog.item(peers_ids::PEERS)).count(), 1);
        dialog.command(peers_ids::ADD);
    });
    bench.choose(Command::TrunkPeers);
    assert_eq!(seen.task_titles(), ["RMD4009"]);
}

#[test]
fn a_hunt_group_is_added_changed_and_removed() {
    let (_service, bench, seen) = connected("groups", "frozen", 8);
    seen.answer.set(id::YES);

    seen.act(|name, time, dialog| match (name, time) {
        ("hunt-groups", 1) => {
            let list = ListView::of(dialog.item(groups_ids::LIST));
            assert_eq!(list.row_count(), 2);
            assert_eq!(
                row_of(list, 0),
                ["dialup", "5551000", "round-robin", "1, 2, 3, 4"]
            );
            assert!(!dialog.item(groups_ids::EDIT).enabled());
            dialog.command(groups_ids::ADD);
        }
        // The editor for a new group: every line is among the others.
        ("hunt-group", 1) => {
            let others = ListBox::of(dialog.item(group_ids::OTHERS));
            assert_eq!(others.count(), 8);
            assert_eq!(ListBox::of(dialog.item(group_ids::MEMBERS)).count(), 0);
            dialog.item(group_ids::NAME).set_text("fax");
            dialog.item(group_ids::PILOT).set_text("5553000");
            Combo::of(dialog.item(group_ids::STRATEGY)).select(Some(2));
            // Lines 3 and then 1: the order they are hunted in.
            others.select(Some(2));
            dialog.command_code(group_ids::OTHERS, code::LIST_SELECTED);
            dialog.command(group_ids::INCLUDE);
            dialog.command(id::OK);
        }
        ("hunt-groups", 2) => {
            let list = ListView::of(dialog.item(groups_ids::LIST));
            assert_eq!(list.row_count(), 3);
            assert_eq!(row_of(list, 2), ["fax", "5553000", "least-recent", "3"]);
            list.select(Some(2));
            dialog.command(groups_ids::EDIT);
        }
        // The editor for the group there is: line 1 goes in, above 3.
        ("hunt-group", 2) => {
            assert_eq!(dialog.item(group_ids::NAME).text(), "fax");
            let members = ListBox::of(dialog.item(group_ids::MEMBERS));
            assert_eq!(members.count(), 1);
            assert_eq!(members.entry(0), "Line 3");
            let others = ListBox::of(dialog.item(group_ids::OTHERS));
            assert_eq!(others.count(), 7);
            others.select(Some(0));
            dialog.command_code(group_ids::OTHERS, code::LIST_SELECTED);
            dialog.command(group_ids::INCLUDE);
            dialog.command(group_ids::UP);
            dialog.item(group_ids::NAME).set_text("facsimile");
            dialog.command(id::OK);
        }
        ("hunt-groups", 3) => {
            let list = ListView::of(dialog.item(groups_ids::LIST));
            assert_eq!(
                row_of(list, 2),
                ["facsimile", "5553000", "least-recent", "1, 3"]
            );
            list.select(Some(2));
            // Asks first; the answer is yes.
            dialog.command(groups_ids::REMOVE);
        }
        ("hunt-groups", 4) => {
            assert_eq!(ListView::of(dialog.item(groups_ids::LIST)).row_count(), 2);
            close(name, dialog);
        }
        other => panic!("unexpected {other:?}"),
    });
    bench.choose(Command::HuntGroups);
    assert_eq!(seen.times("hunt-groups"), 4);
    assert_eq!(seen.times("hunt-group"), 2);
    assert_eq!(seen.task_titles(), ["Remove hunt group"]);
    // The lines are as they were.
    bench.run_for(Duration::from_millis(300));
    assert_eq!(cell(&bench, 1, Column::HuntGroup), "dialup");
    assert_eq!(cell(&bench, 3, Column::HuntGroup), "dialup");
}

#[test]
fn a_hunt_group_the_service_refuses_keeps_its_editor_open() {
    let (_service, bench, seen) = connected("groups-refused", "frozen", 8);
    seen.act(|name, time, dialog| match (name, time) {
        ("hunt-groups", 1) => dialog.command(groups_ids::ADD),
        // A pilot number another group has.
        ("hunt-group", 1) => {
            dialog.item(group_ids::NAME).set_text("again");
            dialog.item(group_ids::PILOT).set_text("5551000");
            dialog.command(id::OK);
        }
        // The list is up again once the editor was closed, by the hook
        // of the message: nothing was added.
        ("hunt-groups", 2) => {
            assert_eq!(ListView::of(dialog.item(groups_ids::LIST)).row_count(), 2);
            close(name, dialog);
        }
        other => panic!("unexpected {other:?}"),
    });
    bench.choose(Command::HuntGroups);
    assert_eq!(seen.task_titles(), ["RMD0029"]);
    seen.tasks.borrow_mut().clear();

    // And a name is asked for before the service is.
    seen.act(|name, time, dialog| match (name, time) {
        ("hunt-groups", 1) => dialog.command(groups_ids::ADD),
        ("hunt-group", 1) => dialog.command(id::OK),
        ("hunt-groups", 2) => close(name, dialog),
        other => panic!("unexpected {other:?}"),
    });
    bench.choose(Command::HuntGroups);
    assert_eq!(seen.task_titles(), ["RMD4009"]);
}

#[test]
fn routes_are_added_changed_moved_and_removed() {
    let (_service, bench, seen) = connected("routes", "frozen", 8);

    seen.act(|name, time, dialog| match (name, time) {
        ("routes", 1) => {
            let list = ListView::of(dialog.item(routes_ids::LIST));
            assert_eq!(list.row_count(), 2);
            assert_eq!(row_of(list, 0), ["1", "5551234", "Line 1"]);
            assert_eq!(row_of(list, 1), ["2", "555*", "Hunt group dialup"]);
            assert!(!dialog.item(routes_ids::UP).enabled());
            // The second goes above the first.
            list.select(Some(1));
            dialog.command(routes_ids::UP);
        }
        ("routes", 2) => {
            let list = ListView::of(dialog.item(routes_ids::LIST));
            assert_eq!(row_of(list, 0), ["1", "555*", "Hunt group dialup"]);
            assert_eq!(row_of(list, 1), ["2", "5551234", "Line 1"]);
            // It is the one that is selected still, and cannot go higher.
            assert_eq!(list.selected(), Some(0));
            assert!(!dialog.item(routes_ids::UP).enabled());
            assert!(dialog.item(routes_ids::DOWN).enabled());
            dialog.command(routes_ids::ADD);
        }
        // The editor for a new route: every line, then every group.
        ("route", 1) => {
            let target = Combo::of(dialog.item(route_ids::TARGET));
            assert_eq!(target.count(), 10);
            assert_eq!(target.entry(0), "Line 1");
            assert_eq!(target.entry(8), "Hunt group dialup");
            assert_eq!(target.entry(9), "Hunt group night");
            dialog.item(route_ids::CALLED).set_text("900*");
            target.select(Some(9));
            dialog.command(id::OK);
        }
        ("routes", 3) => {
            let list = ListView::of(dialog.item(routes_ids::LIST));
            assert_eq!(list.row_count(), 3);
            assert_eq!(row_of(list, 2), ["3", "900*", "Hunt group night"]);
            assert_eq!(list.selected(), Some(2));
            dialog.command(routes_ids::EDIT);
        }
        ("route", 2) => {
            assert_eq!(dialog.item(route_ids::CALLED).text(), "900*");
            let target = Combo::of(dialog.item(route_ids::TARGET));
            assert_eq!(target.selected(), Some(9));
            target.select(Some(1));
            dialog.command(id::OK);
        }
        ("routes", 4) => {
            let list = ListView::of(dialog.item(routes_ids::LIST));
            assert_eq!(row_of(list, 2), ["3", "900*", "Line 2"]);
            assert_eq!(list.selected(), Some(2));
            dialog.command(routes_ids::REMOVE);
        }
        ("routes", 5) => {
            let list = ListView::of(dialog.item(routes_ids::LIST));
            assert_eq!(list.row_count(), 2);
            close(name, dialog);
        }
        other => panic!("unexpected {other:?}"),
    });
    bench.choose(Command::Routes);
    assert_eq!(seen.times("routes"), 5);
    assert_eq!(seen.times("route"), 2);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());

    // The order was kept by the service.
    seen.act(|name, _, dialog| {
        let list = ListView::of(dialog.item(routes_ids::LIST));
        assert_eq!(row_of(list, 0), ["1", "555*", "Hunt group dialup"]);
        close(name, dialog);
    });
    bench.choose(Command::Routes);
}

#[test]
fn capture_is_turned_on_and_off_line_by_line() {
    let (_service, bench, seen) = connected("capture", "frozen", 8);

    seen.act(|name, time, dialog| {
        assert_eq!(name, "capture");
        let list = CheckList::of(dialog.item(capture_ids::LIST));
        match time {
            1 => {
                assert_eq!(list.row_count(), 8);
                assert_eq!(list.cell_text(2, 0), "3");
                assert_eq!(list.cell_text(2, 2), "CNCB2");
                assert!((0..8).all(|row| !list.checked(row)));
                // The frozen service has no SIP and captures nothing.
                assert!(!dialog.item(capture_ids::FOLDER).enabled());
                assert!(
                    dialog
                        .item(capture_ids::WHERE)
                        .text()
                        .contains("has no SIP")
                );
                // Checking a box tells the service at once.
                list.set_checked(2, true);
            }
            2 => close(name, dialog),
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::CallCapture);
    assert_eq!(seen.times("capture"), 2);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
    bench.opened.manager.with_model(|model| {
        assert!(model.lines.line(3).unwrap().capture);
        assert!(!model.lines.line(2).unwrap().capture);
    });

    // It opens with what the service has, and a box cleared is off.
    seen.act(|name, time, dialog| {
        let list = CheckList::of(dialog.item(capture_ids::LIST));
        match time {
            1 => {
                assert!(list.checked(2));
                assert!(!list.checked(1));
                list.set_checked(2, false);
            }
            2 => close(name, dialog),
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::CallCapture);
    bench.opened.manager.with_model(|model| {
        assert!(!model.lines.line(3).unwrap().capture);
    });
}

#[test]
fn the_sip_and_media_settings_are_sent_and_a_restart_offered_when_one_is_needed() {
    let (_service, bench, seen) = connected("sip-settings", "frozen", 4);
    // Later: the offer to restart is turned down.
    seen.answer.set(902);

    // What the router takes while it runs needs no restart.
    seen.act(|name, _, page| {
        let sheet = page.parent().expect("a page is in its sheet");
        match name {
            "settings" => sheet.select_page(2),
            "settings-sip" => {
                assert!(!controls::checked(page.item(telephony_ids::ENABLED)));
                assert_eq!(page.item(telephony_ids::LISTEN).text(), "0.0.0.0:5060");
                assert_eq!(page.item(telephony_ids::RING).text(), "60");
                page.item(telephony_ids::RING).set_text("30");
                sheet.select_page(3);
            }
            "settings-media" => {
                let codecs = Combo::of(page.item(telephony_ids::CODECS));
                assert_eq!(codecs.selected(), Some(0));
                let ptime = Combo::of(page.item(telephony_ids::PTIME));
                assert_eq!(ptime.selected(), Some(1));
                assert_eq!(page.item(telephony_ids::FIRST_PORT).text(), "20000");
                assert_eq!(page.item(telephony_ids::LAST_PORT).text(), "20999");
                assert_eq!(page.item(telephony_ids::TIMEOUT).text(), "10");
                ptime.select(Some(0));
                page.command_code(telephony_ids::PTIME, code::COMBO_SELECTION);
                page.item(telephony_ids::TIMEOUT).set_text("5");
                sheet.command(id::OK);
            }
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::Settings);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());

    // It was kept; turning SIP on waits for the service to start again.
    seen.act(|name, _, page| {
        let sheet = page.parent().expect("a page is in its sheet");
        match name {
            "settings" => sheet.select_page(3),
            "settings-media" => {
                assert_eq!(
                    Combo::of(page.item(telephony_ids::PTIME)).selected(),
                    Some(0)
                );
                assert_eq!(page.item(telephony_ids::TIMEOUT).text(), "5");
                sheet.select_page(2);
            }
            "settings-sip" => {
                assert_eq!(page.item(telephony_ids::RING).text(), "30");
                controls::set_checked(page.item(telephony_ids::ENABLED), true);
                page.command(telephony_ids::ENABLED);
                sheet.command(id::OK);
            }
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::Settings);
    assert_eq!(seen.task_titles(), ["RMD4011"]);
    seen.tasks.borrow_mut().clear();

    // What is typed is checked before it is sent, and the sheet stays.
    seen.act(|name, _, page| {
        let sheet = page.parent().expect("a page is in its sheet");
        match name {
            "settings" => sheet.select_page(3),
            "settings-media" => {
                page.item(telephony_ids::LAST_PORT).set_text("100");
                sheet.command(id::OK);
            }
            other => panic!("unexpected {other}"),
        }
    });
    bench.choose(Command::Settings);
    assert_eq!(seen.task_titles(), ["RMD4008"]);
}

#[test]
fn a_line_is_given_an_engine_and_a_registration() {
    let (_service, bench, seen) = connected("register", "frozen", 5);
    select_line(&bench, 5);

    seen.act(|name, _, dialog| {
        assert_eq!(name, "edit-line");
        // A line that does not register, in a service with no SIP.
        assert!(controls::checked(dialog.item(line_ids::SIP_TRUNK)));
        assert!(
            dialog
                .item(line_ids::SIP_TRUNK)
                .text()
                .contains("does not speak SIP")
        );
        assert!(!dialog.item(line_ids::SERVER).enabled());
        // The line has the modem, the first of the engines; the echo
        // engine is the last.
        assert_eq!(Combo::of(dialog.item(line_ids::ENGINE)).selected(), Some(0));
        assert!(dialog.item(line_ids::MODEM).enabled());
        Combo::of(dialog.item(line_ids::ENGINE)).select(Some(2));
        controls::set_checked(dialog.item(line_ids::SIP_TRUNK), false);
        controls::set_checked(dialog.item(line_ids::SIP_REGISTER), true);
        dialog.item(line_ids::SERVER).set_text("pbx.example.com");
        dialog.item(line_ids::USER).set_text("205");
        dialog.item(line_ids::PASSWORD).set_text("open sesame");
        dialog.command(id::OK);
    });
    bench.choose(Command::EditLine);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
    bench.opened.manager.with_model(|model| {
        let line = model.lines.line(5).expect("line 5 is there");
        assert_eq!(line.engine, "echo");
        assert_eq!(line.sip.mode, SipMode::Registered);
        assert_eq!(line.sip.extension.as_deref(), Some("205"));
        assert_eq!(line.sip.server.as_deref(), Some("pbx.example.com"));
        assert!(line.sip.has_password);
    });
    assert_eq!(cell(&bench, 5, Column::Sip), "205");

    // The dialog shows the registration and never the password; with the
    // password left empty it stays, and the line goes back to the trunk.
    seen.act(|_, _, dialog| {
        assert!(controls::checked(dialog.item(line_ids::SIP_REGISTER)));
        assert!(dialog.item(line_ids::SERVER).enabled());
        assert_eq!(dialog.item(line_ids::SERVER).text(), "pbx.example.com");
        assert_eq!(dialog.item(line_ids::USER).text(), "205");
        assert_eq!(dialog.item(line_ids::PASSWORD).text(), "");
        assert_eq!(dialog.item(line_ids::EXPIRES).text(), "300");
        // The echo engine, which is the third in the list.
        assert_eq!(Combo::of(dialog.item(line_ids::ENGINE)).selected(), Some(2));
        dialog.item(line_ids::EXPIRES).set_text("600");
        dialog.command(id::OK);
    });
    bench.choose(Command::EditLine);
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());
    bench.opened.manager.with_model(|model| {
        let line = model.lines.line(5).expect("line 5 is there");
        assert_eq!(line.sip.expires_s, Some(600));
        assert!(line.sip.has_password);
    });

    seen.act(|_, _, dialog| {
        controls::set_checked(dialog.item(line_ids::SIP_REGISTER), false);
        controls::set_checked(dialog.item(line_ids::SIP_TRUNK), true);
        dialog.command(line_ids::SIP_TRUNK);
        dialog.command(id::OK);
    });
    bench.choose(Command::EditLine);
    bench.opened.manager.with_model(|model| {
        let line = model.lines.line(5).expect("line 5 is there");
        assert_eq!(line.sip.mode, SipMode::None);
        assert!(!line.sip.has_password);
    });

    // A line that registers needs its password.
    seen.act(|_, _, dialog| {
        controls::set_checked(dialog.item(line_ids::SIP_TRUNK), false);
        controls::set_checked(dialog.item(line_ids::SIP_REGISTER), true);
        dialog.item(line_ids::SERVER).set_text("pbx");
        dialog.item(line_ids::USER).set_text("205");
        dialog.command(id::OK);
    });
    bench.choose(Command::EditLine);
    assert_eq!(seen.task_titles(), ["RMD4009"]);
    assert_eq!(
        seen.tasks.borrow()[0].instruction,
        "Password must not be empty."
    );
}

#[test]
fn against_a_service_with_sip_the_window_shows_registrations_calls_and_the_trace() {
    let (_service, bench, seen) = connected("sip", "sip", 4);
    let window = bench.opened.window;
    let manager = &bench.opened.manager;

    // The last line registers to the peer; the others are on the trunk.
    assert!(
        bench.run_until(CONNECT, |bench| {
            bench.opened.manager.with_model(|model| {
                model.lines.line(4).is_some_and(|line| {
                    line.sip.registration.as_ref().map(|r| r.state)
                        == Some(RegistrationState::Registered)
                }) && model.stats().is_some_and(|stats| stats.sip.registered == 1)
            })
        }),
        "line 4 did not register"
    );
    assert_eq!(cell(&bench, 1, Column::Sip), "trunk");
    assert_eq!(cell(&bench, 4, Column::Sip), "204");
    assert_eq!(cell(&bench, 1, Column::HuntGroup), "dialup");
    manager.with_model(|model| {
        let tip = model::sip_tip(model.lines.line(4).unwrap()).unwrap();
        assert!(tip.starts_with("Extension 204 of 127.0.0.1:"), "{tip}");
        assert!(tip.contains(": registered, "), "{tip}");
        let parts = model.status_parts(0, &model::Utc);
        assert_eq!(parts[1], "SIP: 1 / 1 registered");
    });

    // A call comes over SIP: the line shows what went wrong with its RTP,
    // which is a number, and the status bar how much RTP there is.
    assert!(
        bench.run_until(Duration::from_secs(60), |bench| {
            bench.opened.manager.with_model(|model| {
                model.lines.lines().any(|line| {
                    line.call
                        .as_ref()
                        .and_then(|call| call.media.as_ref())
                        .is_some_and(|media| media.rx_packets > 50)
                }) && model.stats().is_some_and(|stats| stats.rtp_kbps > 50)
            })
        }),
        "no call with RTP came"
    );
    manager.with_model(|model| {
        let line = model
            .lines
            .lines()
            .find(|line| line.call.as_ref().is_some_and(|call| call.media.is_some()))
            .unwrap();
        let row = model.lines.row_of(line.id).unwrap();
        let errors = model.lines.cell(row, Column::Errors, 0);
        assert!(errors.parse::<u64>().is_ok(), "{errors:?}");
        let parts = model.status_parts(0, &model::Utc);
        assert!(parts[3].starts_with("RTP: ") && parts[3].ends_with(" kbit/s"));
        let view = model.stats_view(0, &model::Utc);
        assert_eq!(view.carrier[3].1, "PCMU / 20 ms");
        assert_eq!(view.carrier[2].1, "echo, null");
    });

    // The trace: open, it takes the messages that pass. Re-register, with
    // the line that registers selected, makes two.
    *seen.with_dialog.borrow_mut() = Some(Box::new(|_, _, _| {}));
    bench.choose(Command::SipTrace);
    let trace = manager.trace_window().expect("the trace is open");
    assert!(bench.run_until(CONNECT, |_| seen.times("sip-trace") == 1));
    select_line(&bench, 4);
    manager.with_model(|model| {
        assert!(model.availability(Command::Reregister).is_enabled());
    });
    bench.choose(Command::Reregister);
    let output = RichEdit::of(trace.item(trace_ids::OUTPUT));
    assert!(
        bench.run_until(CONNECT, |bench| {
            let text = output.text();
            bench
                .opened
                .manager
                .with_model(|model| model.trace.len() >= 2)
                && text.contains("REGISTER sip:")
                && text.contains("SIP/2.0 200")
        }),
        "the trace did not show the registration: {}",
        output.text()
    );
    let text = output.text();
    assert!(text.contains(" sent to 127.0.0.1:"), "{text}");
    assert!(text.contains(" received from 127.0.0.1:"), "{text}");
    assert!(seen.tasks.borrow().is_empty(), "{:?}", seen.task_titles());

    // Asked for again, it is the same window. Paused, it stands still;
    // cleared, it is empty.
    bench.choose(Command::SipTrace);
    assert_eq!(manager.trace_window(), Some(trace));
    trace.command(trace_ids::PAUSE);
    bench.run_for(Duration::from_millis(300));
    assert_eq!(trace.item(trace_ids::PAUSE).text(), "&Resume");
    assert!(manager.with_model(|model| model.trace.paused()));
    trace.command(trace_ids::CLEAR);
    bench.run_for(Duration::from_millis(500));
    assert_eq!(output.text().trim(), "");
    assert!(manager.with_model(|model| model.trace.is_empty()));
    assert!(!trace.item(trace_ids::SAVE).enabled());
    bench.choose(Command::Reregister);
    bench.run_for(Duration::from_millis(800));
    assert!(manager.with_model(|model| model.trace.is_empty()));

    trace.end_dialog(id::CANCEL);
    bench.pump();
    assert!(!trace.exists());
    assert_eq!(manager.trace_window(), None);

    // With SIP there is a folder the captures go to.
    seen.act(|name, _, dialog| {
        assert!(dialog.item(capture_ids::FOLDER).enabled());
        assert!(dialog.item(capture_ids::WHERE).text().ends_with("captures"));
        dialog.command(capture_ids::FOLDER);
        dialog.command(id::CANCEL);
        let _ = name;
    });
    bench.choose(Command::CallCapture);
    let folders = seen.folders.borrow();
    assert_eq!(folders.len(), 1);
    assert!(folders[0].ends_with("captures"), "{}", folders[0].display());

    // A line of the trunk does not register.
    select_line(&bench, 1);
    manager.with_model(|model| {
        assert_eq!(
            model.availability(Command::Reregister).reason(),
            Some("Line 1 does not register to a PBX.")
        );
    });
    let _ = window;
}
