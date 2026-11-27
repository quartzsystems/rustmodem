// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! New line and Edit line.
//!
//! One dialog for both. For a new line the id can be chosen; for an
//! existing one it is shown and fixed. The service decides whether what
//! was typed is a line: when it refuses, its message is shown and the
//! dialog stays open with everything as it was typed.

use std::cell::RefCell;
use std::rc::Rc;

use ctlproto::{
    Call, ErrorInfo, LineChange, LineInfo, LineModem, LineModemSpec, LineSpec, PortInfo, PortKind,
    PortList, RegisterChange, RegisterSpec, SipMode,
};
use win32::controls::{self, Combo, UpDown};
use win32::{DialogTemplate, Event, Handler, Hwnd, Reply, id};

use super::{modem, must_not_be_empty, number_in, watch, watched};
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
    /// The modem engine.
    pub const ENGINE: u16 = 310;
    /// The modem's settings.
    pub const MODEM: u16 = 322;
    /// SIP: the line is reached by the trunk, or with no SIP not at all.
    pub const SIP_TRUNK: u16 = 311;
    /// SIP: the line registers to a PBX.
    pub const SIP_REGISTER: u16 = 312;
    /// The registrar.
    pub const SERVER: u16 = 313;
    /// The extension.
    pub const USER: u16 = 314;
    /// The password.
    pub const PASSWORD: u16 = 315;
    /// The user name to authenticate with.
    pub const AUTH_USER: u16 = 316;
    /// The registration time to ask for.
    pub const EXPIRES: u16 = 317;
    /// The arrows beside the registration time.
    pub const EXPIRES_ARROWS: u16 = 318;
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

/// The engines, in the order the combo box lists them: what the
/// service calls each, and what the user is shown.
pub const ENGINES: [(&str, &str); 3] = [
    ("modem", "Modem: V.21 and Bell 103, with V.8"),
    ("null", "Null: data straight into the samples"),
    ("echo", "Echo: sends back what it receives"),
];

/// The registration times the service takes, in seconds.
const EXPIRES: (u32, u32) = (60, 86_400);

/// The registration time a line asks for when none is given.
const DEFAULT_EXPIRES: u32 = 300;

fn template(title: &str) -> DialogTemplate {
    DialogTemplate::new(title, 250, 336)
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
        .label(7, 149, 80, 8, "&Modem engine:")
        .combo(ids::ENGINE, 90, 147, 153, 60)
        .button(ids::MODEM, 90, 165, 80, 14, "Modem settin&gs\u{2026}")
        .group(7, 184, 236, 120, "SIP")
        .radio_button(ids::SIP_TRUNK, 14, 196, 222, 10, "&Trunk", true)
        .radio_button(
            ids::SIP_REGISTER,
            14,
            209,
            222,
            10,
            "Register to a PB&X",
            false,
        )
        .label(26, 225, 62, 8, "&Server:")
        .edit(ids::SERVER, 90, 223, 146, 14)
        .label(26, 241, 62, 8, "E&xtension:")
        .edit(ids::USER, 90, 239, 80, 14)
        .label(26, 257, 62, 8, "Pass&word:")
        .password(ids::PASSWORD, 90, 255, 146, 14)
        .label(26, 273, 62, 8, "Authenticate a&s:")
        .edit(ids::AUTH_USER, 90, 271, 80, 14)
        .label(26, 289, 62, 8, "Expir&y (seconds):")
        .number(ids::EXPIRES, 90, 287, 40, 14)
        .up_down(ids::EXPIRES_ARROWS)
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
    /// The modem engine, as the service calls it.
    pub engine: String,
    /// The line's own modem settings, if the Settings dialog changed
    /// them: all of them, and none for a line that is to have the
    /// service's.
    pub modem: Option<LineModemSpec>,
    /// Whether the line registers to a PBX.
    pub register: bool,
    /// The registrar.
    pub server: String,
    /// The extension.
    pub user: String,
    /// The password. On Edit an empty one means the password stays.
    pub password: String,
    /// The user name to authenticate with; empty for the extension.
    pub auth_user: String,
    /// The registration time to ask for, in seconds.
    pub expires: String,
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
            engine: ENGINES[0].0.to_string(),
            modem: None,
            register: false,
            server: String::new(),
            user: String::new(),
            password: String::new(),
            auth_user: String::new(),
            expires: DEFAULT_EXPIRES.to_string(),
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
            engine: line.engine.clone(),
            modem: None,
            register: line.sip.mode == SipMode::Registered,
            server: line.sip.server.clone().unwrap_or_default(),
            user: line.sip.extension.clone().unwrap_or_default(),
            // The service never says what the password is.
            password: String::new(),
            auth_user: line.sip.auth_user.clone().unwrap_or_default(),
            expires: line.sip.expires_s.unwrap_or(DEFAULT_EXPIRES).to_string(),
        }
    }

    fn expires(&self) -> Result<u32, ErrorInfo> {
        number_in("Expiry", &self.expires, EXPIRES.0, EXPIRES.1)
    }

    /// The server and the extension, neither of them empty.
    fn registrar(&self) -> Result<(String, String), ErrorInfo> {
        let (server, user) = (self.server.trim(), self.user.trim());
        if server.is_empty() {
            return Err(must_not_be_empty("Server"));
        }
        if user.is_empty() {
            return Err(must_not_be_empty("Extension"));
        }
        Ok((server.to_string(), user.to_string()))
    }

    /// The registration of a line to add, if it registers.
    fn register_spec(&self) -> Result<Option<RegisterSpec>, ErrorInfo> {
        if !self.register {
            return Ok(None);
        }
        let (server, user) = self.registrar()?;
        if self.password.is_empty() {
            return Err(must_not_be_empty("Password"));
        }
        let expires = self.expires()?;
        let auth_user = self.auth_user.trim();
        Ok(Some(RegisterSpec {
            server,
            user,
            password: self.password.clone(),
            auth_user: (!auth_user.is_empty()).then(|| auth_user.to_string()),
            expires_s: (expires != DEFAULT_EXPIRES).then_some(expires),
        }))
    }

    /// What to change of a line's registration: only what is not as the
    /// line has it. A password left empty stays as it is; a line that
    /// had no registration needs one.
    fn register_change(&self, line: &LineInfo) -> Result<Option<RegisterChange>, ErrorInfo> {
        let registered = line.sip.mode == SipMode::Registered;
        if !self.register {
            // An empty server takes the registration away.
            return Ok(registered.then(|| RegisterChange {
                server: Some(String::new()),
                ..RegisterChange::default()
            }));
        }
        let (server, user) = self.registrar()?;
        if !registered && self.password.is_empty() {
            return Err(must_not_be_empty("Password"));
        }
        let expires = self.expires()?;
        let auth_user = self.auth_user.trim();
        let before = &line.sip;
        let change = RegisterChange {
            server: (before.server.as_deref() != Some(server.as_str())).then_some(server),
            user: (before.extension.as_deref() != Some(user.as_str())).then_some(user),
            password: (!self.password.is_empty()).then(|| self.password.clone()),
            auth_user: (before.auth_user.as_deref().unwrap_or_default() != auth_user)
                .then(|| auth_user.to_string()),
            expires_s: (before.expires_s.unwrap_or(DEFAULT_EXPIRES) != expires).then_some(expires),
        };
        Ok((change != RegisterChange::default()).then_some(change))
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
            engine: Some(self.engine.clone()),
            register: self.register_spec()?,
            // A line with none of its own says nothing of them.
            modem: self
                .modem
                .clone()
                .filter(|modem| *modem != LineModemSpec::default()),
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
            engine: (self.engine != line.engine).then(|| self.engine.clone()),
            register: self.register_change(line)?,
            modem: self.modem.clone(),
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
        || change.engine.is_some()
        || change.register.is_some()
        || change.modem.is_some()
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
    /// The service speaks SIP: a line that does not register is reached
    /// by the trunk. Without SIP it is not reached at all.
    sip: bool,
    /// The modem settings the Settings dialog starts with: the line's
    /// as its calls use them, and then what that dialog was last closed
    /// with.
    modem: RefCell<LineModem>,
    /// What the Settings dialog was last closed with, to be sent with
    /// the line.
    modem_change: RefCell<Option<LineModemSpec>>,
}

impl LineDialog {
    /// The engine the combo box has chosen, as the service calls it.
    fn engine(dialog: Hwnd) -> &'static str {
        Combo::of(dialog.item(ids::ENGINE))
            .selected()
            .and_then(|index| ENGINES.get(index))
            .map_or(ENGINES[0].0, |(name, _)| *name)
    }

    /// The modem's settings are for a line whose engine is the modem.
    fn show_modem(dialog: Hwnd) {
        dialog
            .item(ids::MODEM)
            .set_enabled(Self::engine(dialog) == model::MODEM_ENGINE);
    }

    /// The line's own modem settings, in a dialog of their own. What it
    /// is closed with is kept until the line is sent.
    fn modem_settings(&self, dialog: Hwnd) {
        let start = self.modem.borrow().clone();
        if let Some(spec) = modem::line_settings(&self.manager, dialog, &start) {
            *self.modem.borrow_mut() = modem::line_modem_of(&spec, &start);
            *self.modem_change.borrow_mut() = Some(spec);
        }
        // A test is told that the line's dialog is the one that is up
        // again.
        watch(&self.manager, dialog);
    }

    /// The fields of the registration are for a line that registers.
    fn show_register(dialog: Hwnd) {
        let register = controls::checked(dialog.item(ids::SIP_REGISTER));
        for field in [
            ids::SERVER,
            ids::USER,
            ids::PASSWORD,
            ids::AUTH_USER,
            ids::EXPIRES,
            ids::EXPIRES_ARROWS,
        ] {
            dialog.item(field).set_enabled(register);
        }
    }

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

        let engine = Combo::of(dialog.item(ids::ENGINE));
        for (_, text) in ENGINES {
            engine.add(text);
        }
        engine.select(
            ENGINES
                .iter()
                .position(|(name, _)| *name == self.start.engine),
        );
        // A line that does not register is reached by the trunk when the
        // service speaks SIP, and not at all when it does not: the first
        // choice says which.
        dialog.item(ids::SIP_TRUNK).set_text(if self.sip {
            "&Trunk: calls come from the trusted peers"
        } else {
            "&None: the service does not speak SIP"
        });
        controls::set_checked(dialog.item(ids::SIP_TRUNK), !self.start.register);
        controls::set_checked(dialog.item(ids::SIP_REGISTER), self.start.register);
        dialog.item(ids::SERVER).set_text(&self.start.server);
        dialog.item(ids::USER).set_text(&self.start.user);
        dialog.item(ids::AUTH_USER).set_text(&self.start.auth_user);
        let expires = dialog.item(ids::EXPIRES);
        UpDown::of(dialog.item(ids::EXPIRES_ARROWS)).attach(
            expires,
            EXPIRES.0 as i32,
            EXPIRES.1 as i32,
        );
        expires.set_text(&self.start.expires);
        Self::show_register(dialog);
        Self::show_modem(dialog);
    }

    fn read(&self, dialog: Hwnd) -> Form {
        Form {
            modem: self.modem_change.borrow().clone(),
            number: dialog.item(ID_NUMBER).text(),
            name: dialog.item(ID_NAME).text(),
            port: Combo::of(dialog.item(ID_PORT)).text(),
            rings: dialog.item(ID_RINGS).text(),
            enabled: controls::checked(dialog.item(ID_ENABLED)),
            answer_ok: controls::checked(dialog.item(ID_ANSWER_OK)),
            engine: Self::engine(dialog).to_string(),
            register: controls::checked(dialog.item(ids::SIP_REGISTER)),
            server: dialog.item(ids::SERVER).text(),
            user: dialog.item(ids::USER).text(),
            password: dialog.item(ids::PASSWORD).text(),
            auth_user: dialog.item(ids::AUTH_USER).text(),
            expires: dialog.item(ids::EXPIRES).text(),
        }
    }

    /// Sends what the dialog holds to the service. `Ok(true)` when the
    /// service took it.
    fn send(&self, dialog: Hwnd) -> Result<(), ErrorInfo> {
        let form = self.read(dialog);
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
            Event::Command {
                id: ids::SIP_TRUNK | ids::SIP_REGISTER,
                ..
            } => {
                Self::show_register(dialog);
                Reply::Handled
            }
            Event::Command {
                id: ids::ENGINE, ..
            } => {
                Self::show_modem(dialog);
                watch(&self.manager, dialog);
                Reply::Handled
            }
            Event::Command { id: ids::MODEM, .. } => {
                self.modem_settings(dialog);
                Reply::Handled
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

/// Whether the service speaks SIP, as far as the manager knows.
fn sip_on(manager: &Manager) -> bool {
    manager.with_model(|model| {
        model
            .stats()
            .is_some_and(|stats| stats.sip.listener != ctlproto::ListenerState::Off)
    })
}

/// The ports to offer, as the service lists them now. If the service
/// cannot say, none are offered and a port can still be typed.
fn ports(manager: &Manager, owner: Hwnd) -> Vec<String> {
    manager
        .call::<PortList>(owner, Call::PortsList)
        .map(|list| port_choices(&list.ports))
        .unwrap_or_default()
}

/// The modem settings a new line starts with: the service's, as a line
/// that has none of its own shows them, or what a modem has when the
/// service has no such line to show.
fn service_modem(manager: &Manager) -> LineModem {
    manager.with_model(|model| {
        model
            .lines
            .lines()
            .find(|line| line.modem.own.is_empty() && !line.modem.modulations.is_empty())
            .map_or_else(
                || LineModem {
                    modulations: modem::MODULATIONS
                        .iter()
                        .map(|(name, _, _)| name.to_string())
                        .collect(),
                    v8: true,
                    tx_level_dbm0: -12,
                    own: Vec::new(),
                },
                |line| line.modem.clone(),
            )
    })
}

/// New line, with the lowest free id filled in.
pub fn new_line(manager: &Rc<Manager>, owner: Hwnd) {
    let id = manager.with_model(|model| model.lines.lowest_free_id());
    let handler = Rc::new(LineDialog {
        manager: Rc::clone(manager),
        kind: Kind::New,
        start: Form::new_line(id),
        ports: ports(manager, owner),
        sip: sip_on(manager),
        modem: RefCell::new(service_modem(manager)),
        modem_change: RefCell::new(None),
    });
    template("New line").show_modal(Some(owner), handler);
}

/// Edit line, for a line there is.
pub fn edit_line(manager: &Rc<Manager>, owner: Hwnd, line: LineInfo) {
    let handler = Rc::new(LineDialog {
        manager: Rc::clone(manager),
        start: Form::of(&line),
        modem: RefCell::new(line.modem.clone()),
        modem_change: RefCell::new(None),
        kind: Kind::Edit(Box::new(line)),
        ports: ports(manager, owner),
        sip: sip_on(manager),
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
            modem: ctlproto::LineModem::default(),
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

    fn registered() -> LineInfo {
        let mut line = line();
        line.sip = ctlproto::LineSip {
            mode: SipMode::Registered,
            extension: Some("203".to_string()),
            server: Some("pbx.example.com".to_string()),
            auth_user: None,
            expires_s: Some(300),
            registration: None,
            has_password: true,
        };
        line
    }

    #[test]
    fn a_new_line_takes_an_engine_and_a_registration() {
        let form = Form {
            port: "CNCB4".to_string(),
            ..Form::new_line(5)
        };
        let spec = form.spec().unwrap();
        // A new line has the modem, with the service's settings.
        assert_eq!(spec.engine.as_deref(), Some("modem"));
        assert_eq!(spec.register, None);
        assert_eq!(spec.modem, None);

        let form = Form {
            engine: "echo".to_string(),
            register: true,
            server: " pbx.example.com ".to_string(),
            user: "205".to_string(),
            password: "open sesame".to_string(),
            ..form
        };
        let spec = form.spec().unwrap();
        assert_eq!(spec.engine.as_deref(), Some("echo"));
        let register = spec.register.unwrap();
        assert_eq!(register.server, "pbx.example.com");
        assert_eq!(register.user, "205");
        assert_eq!(register.password, "open sesame");
        // What was left as it started is left to the service.
        assert_eq!((register.auth_user, register.expires_s), (None, None));

        let form = Form {
            auth_user: "line205".to_string(),
            expires: "600".to_string(),
            ..form
        };
        let register = form.spec().unwrap().register.unwrap();
        assert_eq!(register.auth_user.as_deref(), Some("line205"));
        assert_eq!(register.expires_s, Some(600));

        // A line that registers needs all three, and a time the service
        // takes.
        for (broken, field) in [
            (
                Form {
                    server: String::new(),
                    ..form.clone()
                },
                "Server",
            ),
            (
                Form {
                    user: "  ".to_string(),
                    ..form.clone()
                },
                "Extension",
            ),
            (
                Form {
                    password: String::new(),
                    ..form.clone()
                },
                "Password",
            ),
        ] {
            assert_eq!(
                broken.spec().unwrap_err().text,
                format!("{field} must not be empty.")
            );
        }
        let short = Form {
            expires: "5".to_string(),
            ..form
        };
        assert_eq!(short.spec().unwrap_err().msgid, "RMD4008");
    }

    #[test]
    fn a_registration_that_was_not_touched_is_not_sent_and_its_password_stays() {
        let line = registered();
        let form = Form::of(&line);
        assert!(form.register);
        assert_eq!(form.server, "pbx.example.com");
        assert_eq!(form.user, "203");
        assert_eq!(form.password, "");
        assert_eq!(form.expires, "300");
        let change = form.change(&line).unwrap();
        assert_eq!(change.register, None);
        assert!(!changes_something(&change));

        // Only what was changed is sent; a password that was typed is.
        let edited = Form {
            user: "204".to_string(),
            password: "another".to_string(),
            expires: "600".to_string(),
            ..form.clone()
        };
        let change = edited.change(&line).unwrap();
        let register = change.register.clone().unwrap();
        assert_eq!(register.server, None);
        assert_eq!(register.user.as_deref(), Some("204"));
        assert_eq!(register.password.as_deref(), Some("another"));
        assert_eq!(register.auth_user, None);
        assert_eq!(register.expires_s, Some(600));
        assert!(changes_something(&change));

        // The user name to authenticate with can be taken away again.
        let mut named = line.clone();
        named.sip.auth_user = Some("line203".to_string());
        let change = form.change(&named).unwrap();
        assert_eq!(
            change.register.unwrap().auth_user.as_deref(),
            Some(""),
            "an empty name means the extension"
        );
    }

    #[test]
    fn a_line_stops_registering_and_another_starts() {
        let line = registered();
        let trunk = Form {
            register: false,
            ..Form::of(&line)
        };
        let register = trunk.change(&line).unwrap().register.unwrap();
        assert_eq!(register.server.as_deref(), Some(""));
        assert_eq!(register.user, None);

        // A line that did not register needs the password with the rest.
        let plain = self::line();
        let starts = Form {
            register: true,
            server: "pbx".to_string(),
            user: "203".to_string(),
            ..Form::of(&plain)
        };
        assert_eq!(
            starts.change(&plain).unwrap_err().text,
            "Password must not be empty."
        );
        let starts = Form {
            password: "pw".to_string(),
            ..starts
        };
        let register = starts.change(&plain).unwrap().register.unwrap();
        assert_eq!(register.server.as_deref(), Some("pbx"));
        assert_eq!(register.user.as_deref(), Some("203"));
        assert_eq!(register.password.as_deref(), Some("pw"));
        // A line that does not register and is left so changes nothing.
        assert_eq!(Form::of(&plain).change(&plain).unwrap().register, None);

        // The engine is sent when it was changed.
        let echo = Form {
            engine: "echo".to_string(),
            ..Form::of(&plain)
        };
        let change = echo.change(&plain).unwrap();
        assert_eq!(change.engine.as_deref(), Some("echo"));
        assert!(changes_something(&change));
        assert_eq!(ENGINES.map(|(name, _)| name), ["modem", "null", "echo"]);

        // The line's own modem settings are sent when the Settings dialog
        // was closed with OK, whole: with none of its own they are sent
        // as none, which is what gives them up.
        assert_eq!(Form::of(&plain).change(&plain).unwrap().modem, None);
        let own = LineModemSpec {
            modulations: Some(vec!["bell103".to_string()]),
            v8: Some(false),
            tx_level_dbm0: Some(-15),
        };
        let with_own = Form {
            modem: Some(own.clone()),
            ..Form::of(&plain)
        };
        let change = with_own.change(&plain).unwrap();
        assert_eq!(change.modem, Some(own.clone()));
        assert!(changes_something(&change));
        let given_up = Form {
            modem: Some(LineModemSpec::default()),
            ..Form::of(&plain)
        };
        let change = given_up.change(&plain).unwrap();
        assert_eq!(change.modem, Some(LineModemSpec::default()));
        assert!(changes_something(&change));
        // A new line says nothing of settings it does not have.
        let new = Form {
            port: "CNCB4".to_string(),
            modem: Some(LineModemSpec::default()),
            ..Form::new_line(5)
        };
        assert_eq!(new.spec().unwrap().modem, None);
        let new = Form {
            modem: Some(own.clone()),
            ..new
        };
        assert_eq!(new.spec().unwrap().modem, Some(own));
    }
}
