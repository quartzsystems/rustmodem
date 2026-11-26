// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! New line and Edit line.
//!
//! One dialog for both. For a new line the id can be chosen; for an
//! existing one it is shown and fixed. The service decides whether what
//! was typed is a line: when it refuses, its message is shown and the
//! dialog stays open with everything as it was typed.

use std::rc::Rc;

use ctlproto::{Call, ErrorInfo, LineChange, LineInfo, LineSpec, PortInfo, PortKind, PortList};
use win32::controls::{self, Combo, UpDown};
use win32::{DialogTemplate, Event, Handler, Hwnd, Reply, id};

use super::{must_not_be_empty, number_in, watch, watched};
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// The line id.
    pub const NUMBER: u16 = 300;
    /// The arrows beside the line id.
    pub const NUMBER_ARROWS: u16 = 301;
    /// The name.
    pub const NAME: u16 = 302;
    /// The serial backend.
    pub const BACKEND: u16 = 303;
    /// The port.
    pub const PORT: u16 = 304;
    /// Auto-answer rings.
    pub const RINGS: u16 = 305;
    /// The arrows beside the rings.
    pub const RINGS_ARROWS: u16 = 306;
    /// Enabled.
    pub const ENABLED: u16 = 307;
    /// Answer `OK` to an unknown command.
    pub const ANSWER_OK: u16 = 308;
    /// Answer `ERROR` to an unknown command.
    pub const ANSWER_ERROR: u16 = 309;
}

use ids::{
    ANSWER_ERROR as ID_ANSWER_ERROR, ANSWER_OK as ID_ANSWER_OK, BACKEND as ID_BACKEND,
    ENABLED as ID_ENABLED, NAME as ID_NAME, NUMBER as ID_NUMBER, NUMBER_ARROWS as ID_NUMBER_ARROWS,
    PORT as ID_PORT, RINGS as ID_RINGS, RINGS_ARROWS as ID_RINGS_ARROWS,
};

/// The serial backend of this phase.
const BACKEND: &str = "com0com";

/// The line ids the service takes.
const LINE_IDS: (u32, u32) = (1, 999);

fn template(title: &str) -> DialogTemplate {
    DialogTemplate::new(title, 250, 172)
        .label(7, 9, 80, 8, "Line &id:")
        .number(ID_NUMBER, 90, 7, 40, 14)
        .up_down(ID_NUMBER_ARROWS)
        .label(7, 27, 80, 8, "&Name:")
        .edit(ID_NAME, 90, 25, 153, 14)
        .label(7, 45, 80, 8, "Serial &backend:")
        .combo(ID_BACKEND, 90, 43, 80, 60)
        .label(7, 63, 80, 8, "&Port:")
        .combo_editable(ID_PORT, 90, 61, 153, 90)
        .label(7, 81, 80, 8, "&Auto-answer rings:")
        .number(ID_RINGS, 90, 79, 40, 14)
        .up_down(ID_RINGS_ARROWS)
        .check_box(ID_ENABLED, 90, 98, 153, 10, "&Enabled")
        .group(7, 112, 236, 28, "Unknown AT commands")
        .radio_button(ID_ANSWER_OK, 14, 124, 80, 10, "Answer &OK", true)
        .radio_button(ID_ANSWER_ERROR, 100, 124, 90, 10, "Answer E&RROR", false)
        .ok_cancel()
}

/// What the dialog holds, as it was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    /// The line id.
    pub number: String,
    /// The name.
    pub name: String,
    /// The port, as the combo box shows it: the name, and perhaps its
    /// pair and host port in brackets after it.
    pub port: String,
    /// Auto-answer rings.
    pub rings: String,
    /// Whether the line is in service.
    pub enabled: bool,
    /// Whether an unknown AT command is answered `OK`.
    pub answer_ok: bool,
}

impl Form {
    /// What the dialog starts with for a new line.
    pub fn new_line(id: u16) -> Form {
        Form {
            number: id.to_string(),
            name: String::new(),
            port: String::new(),
            rings: "0".to_string(),
            enabled: true,
            answer_ok: true,
        }
    }

    /// What the dialog starts with for a line there is.
    pub fn of(line: &LineInfo) -> Form {
        Form {
            number: line.id.to_string(),
            name: line.name.clone().unwrap_or_default(),
            port: line.port.clone(),
            rings: line.auto_answer_rings.to_string(),
            enabled: line.enabled,
            answer_ok: line.unknown_command != "error",
        }
    }

    /// The name of the port: what is before the first space.
    fn port_name(&self) -> Result<String, ErrorInfo> {
        match self.port.split_whitespace().next() {
            Some(name) => Ok(name.to_string()),
            None => Err(must_not_be_empty("Port")),
        }
    }

    fn rings(&self) -> Result<u8, ErrorInfo> {
        number_in("Auto-answer rings", &self.rings, 0, 255).map(|rings| rings as u8)
    }

    fn unknown_command(&self) -> String {
        if self.answer_ok { "ok" } else { "error" }.to_string()
    }

    /// The line to add.
    pub fn spec(&self) -> Result<LineSpec, ErrorInfo> {
        let id = number_in("Line id", &self.number, LINE_IDS.0, LINE_IDS.1)? as u16;
        let name = self.name.trim();
        Ok(LineSpec {
            id,
            name: (!name.is_empty()).then(|| name.to_string()),
            backend: Some(BACKEND.to_string()),
            port: self.port_name()?,
            enabled: Some(self.enabled),
            auto_answer_rings: Some(self.rings()?),
            unknown_command: Some(self.unknown_command()),
            // BLOCKED(phase2): the engine and the registration come with
            // the dialog's SIP group in the next work package.
            engine: None,
            register: None,
        })
    }

    /// What to change of a line: only what is not as the line has it.
    /// Changing a port hangs up, so a port that was not touched is not
    /// sent.
    pub fn change(&self, line: &LineInfo) -> Result<LineChange, ErrorInfo> {
        let name = self.name.trim();
        let port = self.port_name()?;
        let rings = self.rings()?;
        let unknown_command = self.unknown_command();
        Ok(LineChange {
            id: line.id,
            // An empty name removes the name.
            name: (name != line.name.as_deref().unwrap_or_default()).then(|| name.to_string()),
            backend: None,
            port: (!port.eq_ignore_ascii_case(&line.port)).then_some(port),
            enabled: (self.enabled != line.enabled).then_some(self.enabled),
            auto_answer_rings: (rings != line.auto_answer_rings).then_some(rings),
            unknown_command: (unknown_command != line.unknown_command).then_some(unknown_command),
            engine: None,
            register: None,
        })
    }
}

/// Whether a change changes anything.
fn changes_something(change: &LineChange) -> bool {
    change.name.is_some()
        || change.backend.is_some()
        || change.port.is_some()
        || change.enabled.is_some()
        || change.auto_answer_rings.is_some()
        || change.unknown_command.is_some()
}

/// Whether a port has the name Windows gives a serial port: `COM` and a
/// number.
fn com_named(name: &str) -> bool {
    name.get(..3)
        .is_some_and(|start| start.eq_ignore_ascii_case("COM"))
        && name.len() > 3
        && name[3..].bytes().all(|byte| byte.is_ascii_digit())
}

/// A port as the combo box shows it: its name, and after it in brackets
/// the other end of its pair and the port a host opens.
fn port_entry(port: &PortInfo) -> String {
    let mut beside: Vec<&str> = Vec::new();
    for other in [port.pair.as_deref(), port.host_port.as_deref()]
        .into_iter()
        .flatten()
    {
        if !beside.contains(&other) {
            beside.push(other);
        }
    }
    if beside.is_empty() {
        port.name.clone()
    } else {
        format!("{} ({})", port.name, beside.join(", "))
    }
}

/// The ports to offer for a line: the ends of com0com pairs that no line
/// uses and whose other end no line uses either.
///
/// Of a pair that is wholly free only the end that kept its driver name
/// is offered when the other has a `COM` name: the `COM` name is there
/// for the host to open.
pub fn port_choices(ports: &[PortInfo]) -> Vec<String> {
    let find = |name: &str| {
        ports
            .iter()
            .find(|port| port.name.eq_ignore_ascii_case(name))
    };
    ports
        .iter()
        .filter(|port| port.kind == PortKind::Com0com && port.line.is_none())
        .filter(|port| {
            let other = port.host_port.as_deref().and_then(find);
            match other {
                // The other end is a line's: this one is its host's.
                Some(other) if other.line.is_some() => false,
                Some(other) => !com_named(&port.name) || com_named(&other.name),
                None => true,
            }
        })
        .map(port_entry)
        .collect()
}

/// Which of the two the dialog is.
enum Kind {
    New,
    Edit(Box<LineInfo>),
}

struct LineDialog {
    manager: Rc<Manager>,
    kind: Kind,
    start: Form,
    ports: Vec<String>,
}

impl LineDialog {
    fn fill(&self, dialog: Hwnd) {
        let number = dialog.item(ID_NUMBER);
        number.set_text(&self.start.number);
        UpDown::of(dialog.item(ID_NUMBER_ARROWS)).attach(
            number,
            LINE_IDS.0 as i32,
            LINE_IDS.1 as i32,
        );
        // The up-down control writes its own idea of the number.
        number.set_text(&self.start.number);
        if matches!(self.kind, Kind::Edit(_)) {
            // A line keeps its id.
            number.set_enabled(false);
            dialog.item(ID_NUMBER_ARROWS).set_enabled(false);
        }
        dialog.item(ID_NAME).set_text(&self.start.name);
        let backend = Combo::of(dialog.item(ID_BACKEND));
        backend.add(BACKEND);
        backend.select(Some(0));
        let port = Combo::of(dialog.item(ID_PORT));
        for entry in &self.ports {
            port.add(entry);
        }
        port.set_text(&self.start.port);
        let rings = dialog.item(ID_RINGS);
        UpDown::of(dialog.item(ID_RINGS_ARROWS)).attach(rings, 0, 255);
        rings.set_text(&self.start.rings);
        controls::set_checked(dialog.item(ID_ENABLED), self.start.enabled);
        controls::set_checked(dialog.item(ID_ANSWER_OK), self.start.answer_ok);
        controls::set_checked(dialog.item(ID_ANSWER_ERROR), !self.start.answer_ok);
    }

    fn read(dialog: Hwnd) -> Form {
        Form {
            number: dialog.item(ID_NUMBER).text(),
            name: dialog.item(ID_NAME).text(),
            port: Combo::of(dialog.item(ID_PORT)).text(),
            rings: dialog.item(ID_RINGS).text(),
            enabled: controls::checked(dialog.item(ID_ENABLED)),
            answer_ok: controls::checked(dialog.item(ID_ANSWER_OK)),
        }
    }

    /// Sends what the dialog holds to the service. `Ok(true)` when the
    /// service took it.
    fn send(&self, dialog: Hwnd) -> Result<(), ErrorInfo> {
        let form = Self::read(dialog);
        let call = match &self.kind {
            Kind::New => Call::LinesAdd(form.spec()?),
            Kind::Edit(line) => {
                let change = form.change(line)?;
                if !changes_something(&change) {
                    return Ok(());
                }
                Call::LinesEdit(change)
            }
        };
        let line: LineInfo = self.manager.call(dialog, call)?;
        self.manager.line_changed(line);
        Ok(())
    }
}

impl Handler for LineDialog {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                self.fill(dialog);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } => {
                let name = match self.kind {
                    Kind::New => "new-line",
                    Kind::Edit(_) => "edit-line",
                };
                if watched(&self.manager, dialog, id, name) {
                    Reply::Handled
                } else {
                    Reply::Default
                }
            }
            Event::Command { id: id::OK, .. } => {
                match self.send(dialog) {
                    Ok(()) => dialog.end_dialog(id::OK),
                    // The dialog stays, with what was typed.
                    Err(error) => self.manager.show_error(dialog, &error),
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// The ports to offer, as the service lists them now. If the service
/// cannot say, none are offered and a port can still be typed.
fn ports(manager: &Manager, owner: Hwnd) -> Vec<String> {
    manager
        .call::<PortList>(owner, Call::PortsList)
        .map(|list| port_choices(&list.ports))
        .unwrap_or_default()
}

/// New line, with the lowest free id filled in.
pub fn new_line(manager: &Rc<Manager>, owner: Hwnd) {
    let id = manager.with_model(|model| model.lines.lowest_free_id());
    let handler = Rc::new(LineDialog {
        manager: Rc::clone(manager),
        kind: Kind::New,
        start: Form::new_line(id),
        ports: ports(manager, owner),
    });
    template("New line").show_modal(Some(owner), handler);
}

/// Edit line, for a line there is.
pub fn edit_line(manager: &Rc<Manager>, owner: Hwnd, line: LineInfo) {
    let handler = Rc::new(LineDialog {
        manager: Rc::clone(manager),
        start: Form::of(&line),
        kind: Kind::Edit(Box::new(line)),
        ports: ports(manager, owner),
    });
    template("Edit line").show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use ctlproto::{Counters, LineState};

    use super::*;

    fn line() -> LineInfo {
        LineInfo {
            id: 3,
            name: Some("Front desk".to_string()),
            backend: "com0com".to_string(),
            port: "CNCB2".to_string(),
            enabled: true,
            state: LineState::Idle,
            auto_answer_rings: 2,
            unknown_command: "ok".to_string(),
            counters: Counters::default(),
            call: None,
            error: None,
            binding: None,
            engine: "null".to_string(),
            hunt_groups: Vec::new(),
            sip: ctlproto::LineSip::default(),
            capture: false,
        }
    }

    fn port(name: &str, pair: Option<&str>, host: Option<&str>, line: Option<u16>) -> PortInfo {
        PortInfo {
            name: name.to_string(),
            kind: if pair.is_some() {
                PortKind::Com0com
            } else {
                PortKind::Serial
            },
            pair: pair.map(str::to_string),
            host_port: host.map(str::to_string),
            modem: None,
            line,
        }
    }

    #[test]
    fn a_new_line_starts_enabled_with_the_id_given() {
        let form = Form::new_line(5);
        assert_eq!(form.number, "5");
        assert!(form.enabled && form.answer_ok);
        // The port is still to be chosen.
        assert_eq!(form.spec().unwrap_err().msgid, "RMD4009");

        let form = Form {
            port: "CNCB4 (CNCA4, COM15)".to_string(),
            name: "  Back office ".to_string(),
            rings: "2".to_string(),
            answer_ok: false,
            ..form
        };
        let spec = form.spec().unwrap();
        assert_eq!(spec.id, 5);
        assert_eq!(spec.name.as_deref(), Some("Back office"));
        assert_eq!(spec.backend.as_deref(), Some("com0com"));
        assert_eq!(spec.port, "CNCB4");
        assert_eq!(spec.enabled, Some(true));
        assert_eq!(spec.auto_answer_rings, Some(2));
        assert_eq!(spec.unknown_command.as_deref(), Some("error"));
    }

    #[test]
    fn a_line_with_no_name_has_none() {
        let form = Form {
            port: "CNCB0".to_string(),
            name: "   ".to_string(),
            ..Form::new_line(1)
        };
        assert_eq!(form.spec().unwrap().name, None);
    }

    #[test]
    fn what_is_not_a_number_is_refused_by_the_name_of_its_field() {
        let good = Form {
            port: "CNCB0".to_string(),
            ..Form::new_line(1)
        };
        for number in ["", "0", "1000", "one"] {
            let form = Form {
                number: number.to_string(),
                ..good.clone()
            };
            let error = form.spec().unwrap_err();
            assert_eq!(error.msgid, "RMD4008", "{number:?}");
            assert!(error.text.starts_with("Line id must be"), "{}", error.text);
        }
        for rings in ["", "256", "-1", "two"] {
            let form = Form {
                rings: rings.to_string(),
                ..good.clone()
            };
            let error = form.spec().unwrap_err();
            assert_eq!(error.msgid, "RMD4008", "{rings:?}");
            assert!(error.text.starts_with("Auto-answer rings must be"));
            assert_eq!(form.change(&line()).unwrap_err().msgid, "RMD4008");
        }
    }

    #[test]
    fn an_edit_sends_only_what_was_changed() {
        let line = line();
        let form = Form::of(&line);
        assert_eq!(form.number, "3");
        assert_eq!(form.name, "Front desk");
        assert_eq!(form.rings, "2");
        let untouched = form.change(&line).unwrap();
        assert!(!changes_something(&untouched));
        assert_eq!(untouched.id, 3);

        // The port as the combo box lists it is still the same port.
        let listed = Form {
            port: "cncb2 (CNCA2, COM13)".to_string(),
            ..form.clone()
        };
        assert!(!changes_something(&listed.change(&line).unwrap()));

        let changed = Form {
            name: String::new(),
            port: "CNCB7".to_string(),
            rings: "0".to_string(),
            enabled: false,
            answer_ok: false,
            ..form
        };
        let change = changed.change(&line).unwrap();
        assert!(changes_something(&change));
        // An empty name takes the name away.
        assert_eq!(change.name.as_deref(), Some(""));
        assert_eq!(change.port.as_deref(), Some("CNCB7"));
        assert_eq!(change.enabled, Some(false));
        assert_eq!(change.auto_answer_rings, Some(0));
        assert_eq!(change.unknown_command.as_deref(), Some("error"));
        assert_eq!(change.backend, None);
    }

    #[test]
    fn the_ports_offered_are_the_free_ends_of_pairs() {
        let ports = [
            port("COM1", None, None, None),
            // A pair in use: neither end is offered.
            port("CNCB0", Some("CNCA0"), Some("COM11"), Some(1)),
            port("COM11", Some("CNCB0"), Some("CNCB0"), None),
            // A free pair: the end with the driver's name.
            port("CNCB1", Some("CNCA1"), Some("COM12"), None),
            port("COM12", Some("CNCB1"), Some("CNCB1"), None),
            // A free pair with both ends under their driver names.
            port("CNCA2", Some("CNCB2"), Some("CNCB2"), None),
            port("CNCB2", Some("CNCA2"), Some("CNCA2"), None),
            // A free pair with two COM names.
            port("COM20", Some("CNCB3"), Some("COM21"), None),
            port("COM21", Some("CNCA3"), Some("COM20"), None),
            // Half a pair: the other end is not there.
            port("CNCB4", None, None, None),
        ];
        let mut half = ports.to_vec();
        half[9].kind = PortKind::Com0com;
        assert_eq!(
            port_choices(&half),
            [
                "CNCB1 (CNCA1, COM12)",
                "CNCA2 (CNCB2)",
                "CNCB2 (CNCA2)",
                "COM20 (CNCB3, COM21)",
                "COM21 (CNCA3, COM20)",
                "CNCB4",
            ]
        );
        assert!(port_choices(&[]).is_empty());
    }

    #[test]
    fn a_com_name_is_com_and_a_number() {
        assert!(com_named("COM1") && com_named("com25"));
        assert!(!com_named("COM") && !com_named("CNCB0") && !com_named("COMX"));
        assert!(!com_named("") && !com_named("CO"));
    }
}
