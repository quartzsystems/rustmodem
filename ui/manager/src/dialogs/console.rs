// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The AT console: a terminal on a COM port of this computer.
//!
//! It opens the port itself, with the Win32 serial API, and does not go
//! through the service: it is what a host would be, for talking to a line
//! by hand from the host's end of its pair. What is typed is sent with a
//! carriage return; what arrives is shown as it comes. DTR and RTS are
//! the user's to set, and DCD, DSR, CTS and RI show what the modem
//! drives.
//!
//! A thread reads the port and hands what it reads to the window. The
//! port is closed by telling the thread to stop and waiting for it, which
//! takes no longer than one read's timeout.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use ctlproto::ErrorInfo;
use msg::message;
use win32::controls::{self, Combo};
use win32::{
    DialogTemplate, Event, Font, Handler, Hwnd, ModelessDialog, Reply, RichEdit, SerialPort,
    UiReceiver, UiSender, channel, id, serial_ports,
};

use super::{watch, watched};
use crate::window::Manager;

const ID_PORT: u16 = 300;
const ID_OPEN: u16 = 301;
const ID_DTR: u16 = 302;
const ID_RTS: u16 = 303;
const ID_DCD: u16 = 304;
const ID_DSR: u16 = 305;
const ID_CTS: u16 = 306;
const ID_RI: u16 = 307;
const ID_OUTPUT: u16 = 308;
const ID_INPUT: u16 = 309;
const ID_SEND: u16 = 310;

/// The timer that reads the signals and shows a line that has stopped
/// short of its end.
const POLL: usize = 1;

/// How often it fires, in milliseconds.
const POLL_MS: u32 = 250;

/// How many lines the terminal keeps.
const LINES: usize = 2000;

/// The longest a line is let grow before it is shown as it is: a port
/// that sends no line ends does not grow one without end.
const LONGEST_LINE: usize = 512;

fn template() -> DialogTemplate {
    DialogTemplate::new("AT console", 300, 214)
        .label(7, 9, 20, 8, "&Port:")
        .combo(ID_PORT, 30, 7, 70, 120)
        .button(ID_OPEN, 104, 7, 50, 14, "&Open")
        .check_box(ID_DTR, 166, 9, 30, 10, "&DTR")
        .check_box(ID_RTS, 200, 9, 30, 10, "&RTS")
        .check_box(ID_DCD, 7, 26, 32, 10, "DCD")
        .check_box(ID_DSR, 43, 26, 32, 10, "DSR")
        .check_box(ID_CTS, 79, 26, 32, 10, "CTS")
        .check_box(ID_RI, 115, 26, 28, 10, "RI")
        .rich_edit(ID_OUTPUT, 7, 40, 286, 130)
        .edit(ID_INPUT, 7, 175, 232, 14)
        .default_button(ID_SEND, 243, 175, 50, 14, "&Send")
        .button(id::CANCEL, 243, 193, 50, 14, "Close")
}

/// Turns the bytes a port delivers into lines of text.
///
/// A line ends at a line feed; carriage returns are dropped, so that the
/// `<CR><LF>` a modem puts round its answers is one line end and not
/// two. A byte that is not printable ASCII is shown as a dot.
#[derive(Debug, Default)]
pub struct Lines {
    partial: String,
}

impl Lines {
    /// Takes bytes in. Returns the lines they completed.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut complete = Vec::new();
        for byte in bytes {
            match byte {
                b'\n' => complete.push(std::mem::take(&mut self.partial)),
                b'\r' => {}
                0x20..=0x7E => self.partial.push(char::from(*byte)),
                b'\t' => self.partial.push(' '),
                _ => self.partial.push('.'),
            }
            if self.partial.len() >= LONGEST_LINE {
                complete.push(std::mem::take(&mut self.partial));
            }
        }
        complete
    }

    /// Whether a line has been started and not ended.
    pub fn has_partial(&self) -> bool {
        !self.partial.is_empty()
    }

    /// Gives up the line that has been started, as it is: what a modem
    /// sends without a line end, such as an echo, is still to be seen.
    pub fn flush(&mut self) -> Option<String> {
        (!self.partial.is_empty()).then(|| std::mem::take(&mut self.partial))
    }
}

/// What the reading thread tells the window. The number is the opening
/// it belongs to: what a port that has since been closed still says is
/// not shown.
enum Received {
    Data(u64, Vec<u8>),
    Failed(u64, String),
}

/// A port that is open.
struct Open {
    name: String,
    port: Arc<SerialPort>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

impl Open {
    fn close(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

struct Console {
    manager: Rc<Manager>,
    open: RefCell<Option<Open>>,
    /// How many times a port has been opened.
    openings: Cell<u64>,
    sender: RefCell<Option<UiSender<Received>>>,
    receiver: RefCell<Option<UiReceiver<Received>>>,
    lines: RefCell<Lines>,
    /// The partial line was there at the last poll too.
    lingering: Cell<bool>,
    font: RefCell<Option<Font>>,
}

impl Console {
    fn output(dialog: Hwnd) -> RichEdit {
        RichEdit::of(dialog.item(ID_OUTPUT))
    }

    fn append(dialog: Hwnd, lines: Vec<String>) {
        let lines: Vec<(String, Option<win32::Colour>)> =
            lines.into_iter().map(|line| (line, None)).collect();
        Self::output(dialog).append(&lines, LINES);
    }

    /// Shows the controls as they are with a port open or with none.
    fn show_state(&self, dialog: Hwnd) {
        let open = self.open.borrow().is_some();
        dialog
            .item(ID_OPEN)
            .set_text(if open { "Cl&ose" } else { "&Open" });
        dialog.item(ID_PORT).set_enabled(!open);
        for control in [ID_DTR, ID_RTS, ID_INPUT, ID_SEND] {
            dialog.item(control).set_enabled(open);
        }
        if !open {
            for signal in [ID_DCD, ID_DSR, ID_CTS, ID_RI] {
                controls::set_checked(dialog.item(signal), false);
            }
        }
    }

    fn open_port(&self, dialog: Hwnd) {
        let name = Combo::of(dialog.item(ID_PORT)).text();
        if name.is_empty() {
            self.manager
                .show_error(dialog, &super::must_not_be_empty("Port"));
            return;
        }
        let port = match SerialPort::open(&name) {
            Ok(port) => Arc::new(port),
            Err(e) => {
                let error = ErrorInfo::from(message("RMD4006", &[&name, &e.to_string()]));
                self.manager.show_error(dialog, &error);
                return;
            }
        };
        let Some(sender) = self.sender.borrow().clone() else {
            return;
        };
        let opening = self.openings.get() + 1;
        self.openings.set(opening);
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let port = Arc::clone(&port);
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("AT console reader".to_string())
                .spawn(move || {
                    let mut buffer = [0u8; 1024];
                    while !stop.load(Ordering::SeqCst) {
                        match port.read(&mut buffer) {
                            // Nothing arrived within the timeout.
                            Ok(0) => {}
                            Ok(read) => {
                                sender.send(Received::Data(opening, buffer[..read].to_vec()));
                            }
                            Err(e) => {
                                sender.send(Received::Failed(opening, e.to_string()));
                                break;
                            }
                        }
                    }
                })
                .ok()
        };
        // A port is opened with both signals on.
        controls::set_checked(dialog.item(ID_DTR), true);
        controls::set_checked(dialog.item(ID_RTS), true);
        *self.lines.borrow_mut() = Lines::default();
        *self.open.borrow_mut() = Some(Open {
            name,
            port,
            stop,
            reader,
        });
        self.show_state(dialog);
        dialog.item(ID_INPUT).focus();
    }

    fn close_port(&self, dialog: Hwnd) {
        let open = self.open.borrow_mut().take();
        if let Some(open) = open {
            open.close();
        }
        if let Some(rest) = self.lines.borrow_mut().flush() {
            Self::append(dialog, vec![rest]);
        }
        if dialog.exists() {
            self.show_state(dialog);
        }
    }

    /// The port stopped working: says so and closes it.
    fn failed(&self, dialog: Hwnd, reason: &str) {
        let name = self
            .open
            .borrow()
            .as_ref()
            .map(|open| open.name.clone())
            .unwrap_or_default();
        self.close_port(dialog);
        let error = ErrorInfo::from(message("RMD4007", &[&name, reason]));
        self.manager.show_error(dialog, &error);
    }

    fn send_line(&self, dialog: Hwnd) {
        let input = dialog.item(ID_INPUT);
        let mut line = input.text().into_bytes();
        line.push(b'\r');
        let written = match self.open.borrow().as_ref() {
            Some(open) => open.port.write(&line),
            None => return,
        };
        match written {
            Ok(_) => {
                input.set_text("");
                input.focus();
            }
            Err(e) => self.failed(dialog, &e.to_string()),
        }
    }

    fn set_signal(&self, dialog: Hwnd, control: u16) {
        let on = controls::checked(dialog.item(control));
        let set = match self.open.borrow().as_ref() {
            Some(open) if control == ID_DTR => open.port.set_dtr(on),
            Some(open) => open.port.set_rts(on),
            None => return,
        };
        if let Err(e) = set {
            self.failed(dialog, &e.to_string());
        }
    }

    fn poll(&self, dialog: Hwnd) {
        let status = match self.open.borrow().as_ref() {
            Some(open) => open.port.status(),
            None => return,
        };
        match status {
            Ok(status) => {
                for (control, on) in [
                    (ID_DCD, status.dcd),
                    (ID_DSR, status.dsr),
                    (ID_CTS, status.cts),
                    (ID_RI, status.ri),
                ] {
                    let control = dialog.item(control);
                    if controls::checked(control) != on {
                        controls::set_checked(control, on);
                    }
                }
            }
            Err(e) => {
                self.failed(dialog, &e.to_string());
                return;
            }
        }
        // A line that has waited a whole poll for its end is shown.
        let partial = self.lines.borrow().has_partial();
        if partial && self.lingering.get() {
            if let Some(rest) = self.lines.borrow_mut().flush() {
                Self::append(dialog, vec![rest]);
            }
            self.lingering.set(false);
        } else {
            self.lingering.set(partial);
        }
    }

    fn receive(&self, dialog: Hwnd) {
        let items = match self.receiver.borrow().as_ref() {
            Some(receiver) => receiver.drain(),
            None => return,
        };
        let current = self.openings.get();
        let is_open = self.open.borrow().is_some();
        for item in items {
            match item {
                Received::Data(opening, bytes) if opening == current && is_open => {
                    let complete = self.lines.borrow_mut().push(&bytes);
                    self.lingering.set(false);
                    Self::append(dialog, complete);
                }
                Received::Failed(opening, reason) if opening == current && is_open => {
                    self.failed(dialog, &reason);
                    return;
                }
                // From a port that has been closed since.
                Received::Data(..) | Received::Failed(..) => {}
            }
        }
    }
}

impl Handler for Console {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let (sender, receiver) = channel::<Received>(dialog);
                *self.sender.borrow_mut() = Some(sender);
                *self.receiver.borrow_mut() = Some(receiver);
                let ports = Combo::of(dialog.item(ID_PORT));
                for name in serial_ports() {
                    ports.add(&name);
                }
                if ports.count() > 0 {
                    ports.select(Some(0));
                }
                // The terminal is in the log's fixed-pitch face.
                let font = Font::log(dialog.dpi());
                dialog.item(ID_OUTPUT).set_font(&font);
                *self.font.borrow_mut() = Some(font);
                // The signals are shown, not set.
                for signal in [ID_DCD, ID_DSR, ID_CTS, ID_RI] {
                    dialog.item(signal).set_enabled(false);
                }
                self.show_state(dialog);
                dialog.set_timer(POLL, POLL_MS);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id: POLL } => {
                self.poll(dialog);
                Reply::Handled
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "console") => Reply::Handled,
            Event::Wake => {
                self.receive(dialog);
                Reply::Handled
            }
            Event::Command { id: ID_OPEN, .. } => {
                if self.open.borrow().is_some() {
                    self.close_port(dialog);
                } else {
                    self.open_port(dialog);
                }
                Reply::Handled
            }
            Event::Command { id: ID_SEND, .. } => {
                self.send_line(dialog);
                Reply::Handled
            }
            Event::Command {
                id: control @ (ID_DTR | ID_RTS),
                ..
            } => {
                self.set_signal(dialog, control);
                Reply::Handled
            }
            Event::Destroy => {
                dialog.kill_timer(POLL);
                let open = self.open.borrow_mut().take();
                if let Some(open) = open {
                    open.close();
                }
                Reply::Default
            }
            _ => Reply::Default,
        }
    }
}

/// Opens the AT console as a window beside the main window.
pub fn open(manager: &Rc<Manager>, owner: Hwnd) -> Option<ModelessDialog> {
    let handler = Rc::new(Console {
        manager: Rc::clone(manager),
        open: RefCell::new(None),
        openings: Cell::new(0),
        sender: RefCell::new(None),
        receiver: RefCell::new(None),
        lines: RefCell::new(Lines::default()),
        lingering: Cell::new(false),
        font: RefCell::new(None),
    });
    let dialog = template().create_modeless(Some(owner), handler)?;
    dialog.hwnd().set_visible(true);
    Some(dialog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_ends_at_a_line_feed_and_carriage_returns_are_dropped() {
        let mut lines = Lines::default();
        assert_eq!(lines.push(b"AT\r\r\nOK\r\n"), ["AT", "OK"]);
        assert!(!lines.has_partial());
        // A blank line is a line.
        assert_eq!(lines.push(b"\r\nRING\r\n"), ["", "RING"]);
    }

    #[test]
    fn a_line_can_come_in_pieces() {
        let mut lines = Lines::default();
        assert!(lines.push(b"CONN").is_empty());
        assert!(lines.has_partial());
        assert!(lines.push(b"ECT 24").is_empty());
        assert_eq!(lines.push(b"00\r\nNO"), ["CONNECT 2400"]);
        assert_eq!(lines.flush().as_deref(), Some("NO"));
        assert!(!lines.has_partial());
        assert_eq!(lines.flush(), None);
    }

    #[test]
    fn what_cannot_be_printed_is_a_dot() {
        let mut lines = Lines::default();
        assert_eq!(
            lines.push(&[b'a', 0x00, 0x07, 0xFF, b'\t', b'b', b'\n']),
            ["a... b"]
        );
    }

    #[test]
    fn a_line_without_end_does_not_grow_without_end() {
        let mut lines = Lines::default();
        let complete = lines.push(&vec![b'x'; LONGEST_LINE * 2 + 5]);
        assert_eq!(complete.len(), 2);
        assert!(complete.iter().all(|line| line.len() == LONGEST_LINE));
        assert_eq!(lines.flush().map(|rest| rest.len()), Some(5));
    }
}
