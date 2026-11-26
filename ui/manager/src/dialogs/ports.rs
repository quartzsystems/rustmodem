// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Ports and drivers: the serial ports of the service's computer.

use std::cell::RefCell;
use std::rc::Rc;

use ctlproto::{Call, PortInfo, PortKind, PortList};
use model::EMPTY;
use win32::metrics::scale;
use win32::{Align, Column, DialogTemplate, Event, Handler, Hwnd, ListView, Reply};

use super::{watch, watched};
use crate::window::Manager;

const ID_LIST: u16 = 300;
const ID_REFRESH: u16 = 301;

/// The columns: their titles and their widths at 96 DPI.
const COLUMNS: [(&str, i32); 6] = [
    ("Port", 80),
    ("Kind", 80),
    ("Pair", 80),
    ("Host port", 80),
    ("Modem", 230),
    ("Line", 50),
];

fn template() -> DialogTemplate {
    DialogTemplate::new("Ports and drivers", 420, 210)
        .list_view(ID_LIST, 7, 7, 406, 175)
        .button(ID_REFRESH, 7, 189, 50, 14, "&Refresh")
        .close_button()
}

/// What a row shows of a port.
pub fn row(port: &PortInfo) -> [String; 6] {
    let or_empty = |text: &Option<String>| text.clone().unwrap_or_else(|| EMPTY.to_string());
    [
        port.name.clone(),
        match port.kind {
            PortKind::Com0com => "com0com",
            PortKind::Serial => "Serial",
            PortKind::Other => "Other",
        }
        .to_string(),
        or_empty(&port.pair),
        or_empty(&port.host_port),
        or_empty(&port.modem),
        port.line
            .map_or_else(|| EMPTY.to_string(), |line| line.to_string()),
    ]
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
                match self.manager.call::<PortList>(dialog, Call::PortsList) {
                    Ok(list) => {
                        *self.ports.borrow_mut() = list.ports;
                        self.show(dialog);
                    }
                    Err(error) => self.manager.show_error(dialog, &error),
                }
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
        ports: RefCell::new(ports),
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
                "1"
            ]
        );
        let bare = PortInfo {
            name: "COM1".to_string(),
            kind: PortKind::Serial,
            pair: None,
            host_port: None,
            modem: None,
            line: None,
        };
        assert_eq!(row(&bare), ["COM1", "Serial", EMPTY, EMPTY, EMPTY, EMPTY]);
        let other = PortInfo {
            name: "LPT9".to_string(),
            kind: PortKind::Other,
            ..bare
        };
        assert_eq!(row(&other)[1], "Other");
    }

    #[test]
    fn there_is_a_title_for_every_cell() {
        assert_eq!(COLUMNS.len(), 6);
        assert_eq!(COLUMNS[0].0, "Port");
        assert_eq!(COLUMNS[5].0, "Line");
    }
}
