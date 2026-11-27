// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Ports and drivers: the serial ports of the service's computer.
//!
//! RustModem ports are made and removed here. Both are done by the setup
//! program, run as an administrator: `crate::setup`.

use std::cell::RefCell;
use std::rc::Rc;

use ctlproto::{Call, PortInfo, PortKind, PortList};
use model::EMPTY;
use win32::metrics::scale;
use win32::{Align, Column, DialogTemplate, Event, Handler, Hwnd, ListView, Reply};

use super::{watch, watched};
use crate::setup::{self, Outcome};
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// The list of ports.
    pub const LIST: u16 = 300;
    /// Refresh.
    pub const REFRESH: u16 = 301;
    /// Add port.
    pub const ADD: u16 = 302;
    /// Remove port.
    pub const REMOVE: u16 = 303;
}

use ids::{ADD as ID_ADD, LIST as ID_LIST, REFRESH as ID_REFRESH, REMOVE as ID_REMOVE};

/// The columns: their titles and their widths at 96 DPI.
const COLUMNS: [(&str, i32); 8] = [
    ("Port", 70),
    ("Kind", 90),
    ("Pair", 60),
    ("Host port", 65),
    ("Modem", 185),
    ("Driver", 70),
    ("State", 70),
    ("Line", 40),
];

fn template() -> DialogTemplate {
    DialogTemplate::new("Ports and drivers", 480, 210)
        .list_view(ID_LIST, 7, 7, 466, 175)
        .button(ID_REFRESH, 7, 189, 50, 14, "&Refresh")
        .button(ID_ADD, 61, 189, 60, 14, "&Add port")
        .button(ID_REMOVE, 125, 189, 60, 14, "Re&move port")
        .close_button()
}

/// What a port is doing. Of a serial port that is not a line's nothing is
/// known: something else may have it open.
pub fn state(port: &PortInfo) -> &'static str {
    match port.kind {
        _ if port.line.is_some() => "In use",
        // Windows dials through the modem: a port without one takes a
        // line and no call from an application.
        PortKind::Vcom if port.modem.is_none() => "No modem",
        PortKind::Vcom | PortKind::Com0com => "Free",
        PortKind::Serial | PortKind::Other => EMPTY,
    }
}

/// What a row shows of a port.
pub fn row(port: &PortInfo) -> [String; 8] {
    let or_empty = |text: &Option<String>| text.clone().unwrap_or_else(|| EMPTY.to_string());
    [
        port.name.clone(),
        match port.kind {
            PortKind::Com0com => "com0com",
            PortKind::Vcom => "RustModem",
            PortKind::Serial => "Serial",
            PortKind::Other => "Other",
        }
        .to_string(),
        or_empty(&port.pair),
        or_empty(&port.host_port),
        or_empty(&port.modem),
        or_empty(&port.driver_version),
        state(port).to_string(),
        port.line
            .map_or_else(|| EMPTY.to_string(), |line| line.to_string()),
    ]
}

/// The ports in the order the list shows them: RustModem's own first, and
/// the rest after them, each part in the order the service gave.
pub fn ordered(mut ports: Vec<PortInfo>) -> Vec<PortInfo> {
    // The sort keeps the order of those it does not tell apart.
    ports.sort_by_key(|port| port.kind != PortKind::Vcom);
    ports
}

/// Whether a port can be removed here: a RustModem port that no line
/// uses. A line's port goes with the line, under Remove line.
pub fn removable(port: &PortInfo) -> bool {
    port.kind == PortKind::Vcom && port.line.is_none()
}

struct Ports {
    manager: Rc<Manager>,
    ports: RefCell<Vec<PortInfo>>,
}

impl Ports {
    fn show(&self, dialog: Hwnd) {
        let list = ListView::of(dialog.item(ID_LIST));
        let count = self.ports.borrow().len();
        list.set_row_count(count);
        if count > 0 {
            list.redraw_rows(0, count - 1);
        }
        self.show_buttons(dialog);
    }

    /// The port the list has selected.
    fn selected(&self, dialog: Hwnd) -> Option<PortInfo> {
        let row = ListView::of(dialog.item(ID_LIST)).selected()?;
        self.ports.try_borrow().ok()?.get(row).cloned()
    }

    /// Add port is for a computer whose ports the setup program makes;
    /// Remove port is besides for a port that can be removed.
    fn show_buttons(&self, dialog: Hwnd) {
        let can = self.manager.ports_can_change();
        dialog.item(ID_ADD).set_enabled(can);
        dialog
            .item(ID_REMOVE)
            .set_enabled(can && self.selected(dialog).is_some_and(|port| removable(&port)));
    }

    /// Asks the service for the ports again.
    fn refresh(&self, dialog: Hwnd) {
        match self.manager.call::<PortList>(dialog, Call::PortsList) {
            Ok(list) => {
                *self.ports.borrow_mut() = ordered(list.ports);
                self.show(dialog);
            }
            Err(error) => self.manager.show_error(dialog, &error),
        }
    }

    /// Runs a port command of the setup program, and shows the ports as
    /// they are after it.
    fn port_command(&self, dialog: Hwnd, words: Vec<String>) {
        match self.manager.port_command(dialog, words) {
            Outcome::Done => self.refresh(dialog),
            Outcome::Cancelled => {}
            Outcome::Failed(error) => {
                self.manager.show_error(dialog, &error);
                // Something may have been done all the same.
                self.refresh(dialog);
            }
        }
        // A test is told that the dialog is the one that is up again.
        watch(&self.manager, dialog);
    }
}

impl Handler for Ports {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let dpi = dialog.dpi();
                let list = ListView::of(dialog.item(ID_LIST));
                let columns: Vec<Column> = COLUMNS
                    .iter()
                    .enumerate()
                    .map(|(index, (title, width))| {
                        // The line is a number.
                        let align = if index == COLUMNS.len() - 1 {
                            Align::Right
                        } else {
                            Align::Left
                        };
                        Column::new(title, scale(*width, dpi), align)
                    })
                    .collect();
                list.set_columns(&columns);
                self.show(dialog);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "ports") => Reply::Handled,
            Event::Command { id: ID_REFRESH, .. } => {
                self.refresh(dialog);
                Reply::Handled
            }
            Event::Command { id: ID_ADD, .. } => {
                self.port_command(dialog, setup::add_words());
                Reply::Handled
            }
            Event::Command { id: ID_REMOVE, .. } => {
                if let Some(port) = self.selected(dialog).filter(removable) {
                    self.port_command(dialog, setup::remove_words(&port.name));
                }
                Reply::Handled
            }
            Event::ListSelection { id: ID_LIST } => {
                self.show_buttons(dialog);
                Reply::Handled
            }
            Event::ListCell {
                id: ID_LIST,
                row: index,
                column,
                cell,
            } => {
                if let Ok(ports) = self.ports.try_borrow()
                    && let Some(port) = ports.get(index)
                    && let Some(text) = row(port).get(column)
                {
                    cell.text = text.clone();
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Shows the ports the service lists.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    // Asked for before the dialog opens, so that it opens filled in.
    let ports = match manager.call::<PortList>(owner, Call::PortsList) {
        Ok(list) => list.ports,
        Err(error) => {
            manager.show_error(owner, &error);
            return;
        }
    };
    let handler = Rc::new(Ports {
        manager: Rc::clone(manager),
        ports: RefCell::new(ordered(ports)),
    });
    template().show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_the_port_and_what_is_known_of_it() {
        let pair = PortInfo {
            name: "CNCB0".to_string(),
            kind: PortKind::Com0com,
            pair: Some("CNCA0".to_string()),
            host_port: Some("COM11".to_string()),
            modem: Some("Standard 56000 bps Modem".to_string()),
            driver_version: None,
            line: Some(1),
        };
        assert_eq!(
            row(&pair),
            [
                "CNCB0",
                "com0com",
                "CNCA0",
                "COM11",
                "Standard 56000 bps Modem",
                EMPTY,
                "In use",
                "1"
            ]
        );
        let bare = PortInfo {
            name: "COM1".to_string(),
            kind: PortKind::Serial,
            pair: None,
            host_port: None,
            modem: None,
            driver_version: None,
            line: None,
        };
        assert_eq!(
            row(&bare),
            ["COM1", "Serial", EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY]
        );
        let other = PortInfo {
            name: "LPT9".to_string(),
            kind: PortKind::Other,
            ..bare
        };
        assert_eq!(row(&other)[1], "Other");
    }

    #[test]
    fn a_rustmodem_port_shows_its_modem_its_driver_and_its_state() {
        let port = PortInfo {
            name: "COM10".to_string(),
            kind: PortKind::Vcom,
            pair: None,
            host_port: None,
            modem: Some("RustModem Softmodem".to_string()),
            driver_version: Some("0.2.0.0".to_string()),
            line: None,
        };
        assert_eq!(
            row(&port),
            [
                "COM10",
                "RustModem",
                EMPTY,
                EMPTY,
                "RustModem Softmodem",
                "0.2.0.0",
                "Free",
                EMPTY
            ]
        );
        assert!(removable(&port));
        let used = PortInfo {
            line: Some(4),
            ..port.clone()
        };
        assert_eq!(state(&used), "In use");
        // A line's port goes with the line.
        assert!(!removable(&used));
        let bare = PortInfo {
            modem: None,
            driver_version: None,
            ..port
        };
        assert_eq!(state(&bare), "No modem");
        assert!(removable(&bare));
    }

    #[test]
    fn rustmodem_ports_are_listed_first() {
        let port = |name: &str, kind| PortInfo {
            name: name.to_string(),
            kind,
            pair: None,
            host_port: None,
            modem: None,
            driver_version: None,
            line: None,
        };
        let listed = ordered(vec![
            port("CNCB0", PortKind::Com0com),
            port("COM1", PortKind::Serial),
            port("COM10", PortKind::Vcom),
            port("COM11", PortKind::Com0com),
            port("COM12", PortKind::Vcom),
        ]);
        let names: Vec<&str> = listed.iter().map(|port| port.name.as_str()).collect();
        assert_eq!(names, ["COM10", "COM12", "CNCB0", "COM1", "COM11"]);
    }

    #[test]
    fn only_a_rustmodem_port_is_removed_here() {
        let pair = PortInfo {
            name: "CNCB0".to_string(),
            kind: PortKind::Com0com,
            pair: Some("CNCA0".to_string()),
            host_port: Some("COM11".to_string()),
            modem: None,
            driver_version: None,
            line: None,
        };
        assert_eq!(state(&pair), "Free");
        assert!(!removable(&pair));
    }

    #[test]
    fn there_is_a_title_for_every_cell() {
        assert_eq!(COLUMNS.len(), 8);
        assert_eq!(COLUMNS[0].0, "Port");
        assert_eq!(COLUMNS[5].0, "Driver");
        assert_eq!(COLUMNS[6].0, "State");
        assert_eq!(COLUMNS[7].0, "Line");
    }
}
