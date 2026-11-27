// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Rate cap and modulations: what the modems offer.
//!
//! The modulations are a list with a box on each: a modem may use the
//! ones that are checked, and prefers them in the order of the list,
//! which Move up and Move down change. An answering modem that cannot
//! agree a modulation by V.8 offers them in that order. With them are
//! whether the modems use V.8, the level they send at, and what a call at
//! 1200 or 2400 bit/s has over the modem: error control, compression and
//! the guard tone.
//!
//! The same dialog, with less in it, is a line's own settings: it is
//! opened from the line's dialog, and what it holds is sent with the
//! line.

use std::cell::RefCell;
use std::rc::Rc;

use ctlproto::{
    Call, ErrorInfo, LineModem, LineModemSpec, ModemChange, ModemSettings, Settings,
    SettingsChange, SettingsResult,
};
use msg::message;
use win32::controls::{self, Combo, UpDown};
use win32::metrics::scale;
use win32::{Align, CheckList, Column, DialogTemplate, Event, Handler, Hwnd, Reply, id};

use super::{number_in, watch, watched};
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// The modulations.
    pub const LIST: u16 = 300;
    /// Move up.
    pub const UP: u16 = 301;
    /// Move down.
    pub const DOWN: u16 = 302;
    /// V.8.
    pub const V8: u16 = 303;
    /// The transmit level.
    pub const LEVEL: u16 = 304;
    /// The answer delay.
    pub const DELAY: u16 = 305;
    /// Its arrows.
    pub const DELAY_ARROWS: u16 = 306;
    /// The time each modulation is offered for.
    pub const FALLBACK: u16 = 307;
    /// Its arrows.
    pub const FALLBACK_ARROWS: u16 = 308;
    /// A line has settings of its own.
    pub const OWN: u16 = 309;
    /// The policy for error control.
    pub const ERROR_CONTROL: u16 = 310;
    /// Compression.
    pub const COMPRESSION: u16 = 311;
    /// The guard tone.
    pub const GUARD_TONE: u16 = 312;
}

/// The modulations there are: what the service calls each, its name and
/// its rate.
pub const MODULATIONS: [(&str, &str, &str); 5] = [
    ("v22bis", "V.22bis", "2400 bit/s"),
    ("v22", "V.22", "1200 bit/s"),
    ("bell212a", "Bell 212A", "1200 bit/s"),
    ("v21", "V.21", "300 bit/s"),
    ("bell103", "Bell 103", "300 bit/s"),
];

/// The policies for error control: what the service calls each, and what
/// the list says.
pub const ERROR_CONTROLS: [(&str, &str); 3] = [
    ("auto", "Auto: if the far end has it"),
    ("required", "Required: or the call is cleared"),
    ("off", "Off"),
];

/// The guard tones: what the service calls each, and what the list says.
pub const GUARD_TONES: [(&str, &str); 3] = [("off", "Off"), ("1800", "1800 Hz"), ("550", "550 Hz")];

/// Where a name of the service's is in a list of them; the first, which
/// is the default, for one this manager does not know.
fn place(names: &[(&str, &str)], name: &str) -> usize {
    names
        .iter()
        .position(|(known, _)| *known == name)
        .unwrap_or(0)
}

/// The levels a modem can send at, in dBm0.
pub const LEVELS: (i32, i32) = (-20, -9);

/// The answer delays the service takes, in milliseconds.
const DELAYS: (u32, u32) = (0, 10_000);

/// The times for each modulation the service takes, in milliseconds.
const FALLBACKS: (u32, u32) = (500, 20_000);

/// The columns of the list: their titles and their widths at 96 DPI.
const COLUMNS: [(&str, i32); 2] = [("Modulation", 130), ("Rate", 90)];

/// Which of the two the dialog is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The `[modem]` settings of the service.
    Service,
    /// A line's own.
    Line,
}

fn template(kind: Kind, title: &str) -> DialogTemplate {
    // A line's dialog has a box at the top and no times at the bottom.
    let (top, height) = match kind {
        Kind::Service => (0, 242),
        Kind::Line => (16, 222),
    };
    let mut dialog = DialogTemplate::new(title, 236, height);
    if kind == Kind::Line {
        dialog = dialog.check_box(
            ids::OWN,
            7,
            7,
            222,
            10,
            "This line has modem settings of its &own",
        );
    }
    dialog = dialog
        .label(
            7,
            top + 7,
            222,
            8,
            "&Modulations a modem may use, the one it prefers first:",
        )
        .check_list(ids::LIST, 7, top + 19, 168, 70)
        .button(ids::UP, 179, top + 19, 50, 14, "Move &up")
        .button(ids::DOWN, 179, top + 37, 50, 14, "Move &down")
        .check_box(
            ids::V8,
            7,
            top + 95,
            222,
            10,
            "Agree the modulation with &V.8",
        )
        .label(7, top + 113, 90, 8, "Transmit &level:")
        .combo(ids::LEVEL, 100, top + 111, 70, 130)
        .label(7, top + 131, 90, 8, "&Error control:")
        .combo(ids::ERROR_CONTROL, 100, top + 129, 129, 60)
        .check_box(
            ids::COMPRESSION,
            7,
            top + 147,
            222,
            10,
            "&Compress data with V.42bis where there is error control",
        )
        .label(7, top + 165, 90, 8, "&Guard tone:")
        .combo(ids::GUARD_TONE, 100, top + 163, 70, 60);
    if kind == Kind::Service {
        dialog = dialog
            .label(7, 183, 90, 8, "&Answer delay (ms):")
            .number(ids::DELAY, 100, 181, 50, 14)
            .up_down(ids::DELAY_ARROWS)
            .label(7, 201, 90, 8, "&Time for each (ms):")
            .number(ids::FALLBACK, 100, 199, 50, 14)
            .up_down(ids::FALLBACK_ARROWS);
    }
    dialog.ok_cancel()
}

/// A level as the list shows it.
fn level_text(level: i32) -> String {
    format!("{level} dBm0")
}

/// What the dialog holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    /// Every modulation there is, in the order of the list, with whether
    /// its box is checked.
    pub order: Vec<(String, bool)>,
    /// Whether the modems use V.8.
    pub v8: bool,
    /// The level they send at, in dBm0.
    pub level: i32,
    /// The policy for error control, as the service calls it.
    pub error_control: String,
    /// Whether V.42bis is asked for and agreed to.
    pub compression: bool,
    /// The guard tone, as the service calls it.
    pub guard_tone: String,
    /// The answer delay, as it was typed.
    pub delay: String,
    /// The time for each modulation, as it was typed.
    pub fallback: String,
    /// For a line: whether it has settings of its own.
    pub own: bool,
}

/// The modulations in the order of a list of the allowed ones: those
/// first, checked, and after them the others there are.
fn ordered(allowed: &[String]) -> Vec<(String, bool)> {
    let mut order: Vec<(String, bool)> = allowed
        .iter()
        .filter(|name| MODULATIONS.iter().any(|(known, _, _)| known == name))
        .map(|name| (name.clone(), true))
        .collect();
    for (name, _, _) in MODULATIONS {
        if !order.iter().any(|(listed, _)| listed == name) {
            order.push((name.to_string(), false));
        }
    }
    order
}

impl Form {
    /// What the dialog starts with for the service's settings.
    pub fn of_service(settings: &ModemSettings) -> Form {
        Form {
            order: ordered(&settings.modulations),
            v8: settings.v8,
            level: settings.tx_level_dbm0,
            error_control: settings.error_control.clone(),
            compression: settings.compression,
            guard_tone: settings.guard_tone.clone(),
            delay: settings.answer_delay_ms.to_string(),
            fallback: settings.fallback_ms.to_string(),
            own: true,
        }
    }

    /// What the dialog starts with for a line: the settings its calls
    /// use, and whether any of them is its own.
    pub fn of_line(line: &LineModem) -> Form {
        Form {
            order: ordered(&line.modulations),
            v8: line.v8,
            level: line.tx_level_dbm0,
            error_control: line.error_control.clone(),
            compression: line.compression,
            guard_tone: line.guard_tone.clone(),
            delay: String::new(),
            fallback: String::new(),
            own: !line.own.is_empty(),
        }
    }

    /// The modulations whose boxes are checked, in order. There must be
    /// one.
    fn chosen(&self) -> Result<Vec<String>, ErrorInfo> {
        let chosen: Vec<String> = self
            .order
            .iter()
            .filter(|(_, checked)| *checked)
            .map(|(name, _)| name.clone())
            .collect();
        if chosen.is_empty() {
            return Err(ErrorInfo::from(message("RMD4015", &[])));
        }
        Ok(chosen)
    }

    /// What to change of the service's settings: only what is not as it
    /// is. `None` when nothing is.
    pub fn change(&self, current: &ModemSettings) -> Result<Option<ModemChange>, ErrorInfo> {
        let chosen = self.chosen()?;
        let delay = number_in("Answer delay", &self.delay, DELAYS.0, DELAYS.1)?;
        let fallback = number_in(
            "Time for each modulation",
            &self.fallback,
            FALLBACKS.0,
            FALLBACKS.1,
        )?;
        let change = ModemChange {
            modulations: (chosen != current.modulations).then_some(chosen),
            v8: (self.v8 != current.v8).then_some(self.v8),
            tx_level_dbm0: (self.level != current.tx_level_dbm0).then_some(self.level),
            answer_delay_ms: (delay != current.answer_delay_ms).then_some(delay),
            fallback_ms: (fallback != current.fallback_ms).then_some(fallback),
            error_control: (self.error_control != current.error_control)
                .then(|| self.error_control.clone()),
            compression: (self.compression != current.compression).then_some(self.compression),
            guard_tone: (self.guard_tone != current.guard_tone).then(|| self.guard_tone.clone()),
        };
        Ok((change != ModemChange::default()).then_some(change))
    }

    /// A line's own settings: all of them, or for a line with none of its
    /// own, none.
    pub fn spec(&self) -> Result<LineModemSpec, ErrorInfo> {
        if !self.own {
            return Ok(LineModemSpec::default());
        }
        Ok(LineModemSpec {
            modulations: Some(self.chosen()?),
            v8: Some(self.v8),
            tx_level_dbm0: Some(self.level),
            error_control: Some(self.error_control.clone()),
            compression: Some(self.compression),
            guard_tone: Some(self.guard_tone.clone()),
        })
    }
}

/// The settings a line's calls would use with `spec` as its own, shown
/// as the service shows them: for the line's dialog to hold until it is
/// sent.
pub fn line_modem_of(spec: &LineModemSpec, current: &LineModem) -> LineModem {
    let mut own = Vec::new();
    for (key, set) in [
        ("modulations", spec.modulations.is_some()),
        ("v8", spec.v8.is_some()),
        ("tx_level_dbm0", spec.tx_level_dbm0.is_some()),
        ("error_control", spec.error_control.is_some()),
        ("compression", spec.compression.is_some()),
        ("guard_tone", spec.guard_tone.is_some()),
    ] {
        if set {
            own.push(key.to_string());
        }
    }
    LineModem {
        modulations: spec
            .modulations
            .clone()
            .unwrap_or_else(|| current.modulations.clone()),
        v8: spec.v8.unwrap_or(current.v8),
        tx_level_dbm0: spec.tx_level_dbm0.unwrap_or(current.tx_level_dbm0),
        error_control: spec
            .error_control
            .clone()
            .unwrap_or_else(|| current.error_control.clone()),
        compression: spec.compression.unwrap_or(current.compression),
        guard_tone: spec
            .guard_tone
            .clone()
            .unwrap_or_else(|| current.guard_tone.clone()),
        own,
    }
}

struct ModemDialog {
    manager: Rc<Manager>,
    kind: Kind,
    start: Form,
    /// The service's settings as they were read, for the service's
    /// dialog.
    current: ModemSettings,
    /// The modulations in the order of the rows.
    rows: RefCell<Vec<String>>,
    /// What a line's dialog was closed with.
    result: RefCell<Option<LineModemSpec>>,
}

impl ModemDialog {
    fn list(dialog: Hwnd) -> CheckList {
        CheckList::of(dialog.item(ids::LIST))
    }

    /// Fills the list with the modulations in an order, and selects one.
    fn fill_list(&self, dialog: Hwnd, order: &[(String, bool)], selected: Option<usize>) {
        let list = Self::list(dialog);
        list.clear();
        for (name, checked) in order {
            let (title, rate) = MODULATIONS
                .iter()
                .find(|(known, _, _)| known == name)
                .map_or((name.as_str(), ""), |(_, title, rate)| (*title, *rate));
            list.add_row(&[title.to_string(), rate.to_string()], *checked);
        }
        list.select(selected);
        *self.rows.borrow_mut() = order.iter().map(|(name, _)| name.clone()).collect();
        Self::show_moves(dialog);
    }

    /// The list as it stands: every modulation with its box.
    fn order(&self, dialog: Hwnd) -> Vec<(String, bool)> {
        let list = Self::list(dialog);
        self.rows
            .borrow()
            .iter()
            .enumerate()
            .map(|(row, name)| (name.clone(), list.checked(row)))
            .collect()
    }

    /// Moves the selected row up or down one place.
    fn move_row(&self, dialog: Hwnd, down: bool) {
        let Some(row) = Self::list(dialog).selected() else {
            return;
        };
        let mut order = self.order(dialog);
        let to = if down { row + 1 } else { row.wrapping_sub(1) };
        if to >= order.len() {
            return;
        }
        order.swap(row, to);
        self.fill_list(dialog, &order, Some(to));
        // A test is told that the list shows the move.
        watch(&self.manager, dialog);
    }

    /// Move up is for a row that has one above it, Move down for a row
    /// that has one below, and neither for a line with no settings of its
    /// own.
    fn show_moves(dialog: Hwnd) {
        let list = Self::list(dialog);
        let on = list.hwnd().enabled();
        let selected = list.selected();
        dialog
            .item(ids::UP)
            .set_enabled(on && selected.is_some_and(|row| row > 0));
        dialog
            .item(ids::DOWN)
            .set_enabled(on && selected.is_some_and(|row| row + 1 < list.row_count()));
    }

    /// A line with no settings of its own shows what it has from the
    /// service, which is not to be changed here.
    fn show_own(&self, dialog: Hwnd) {
        if self.kind != Kind::Line {
            return;
        }
        let own = controls::checked(dialog.item(ids::OWN));
        for control in [
            ids::LIST,
            ids::V8,
            ids::LEVEL,
            ids::ERROR_CONTROL,
            ids::COMPRESSION,
            ids::GUARD_TONE,
        ] {
            dialog.item(control).set_enabled(own);
        }
        Self::show_moves(dialog);
    }

    fn fill(&self, dialog: Hwnd) {
        let dpi = dialog.dpi();
        let columns: Vec<Column> = COLUMNS
            .iter()
            .map(|(title, width)| Column::new(title, scale(*width, dpi), Align::Left))
            .collect();
        Self::list(dialog).set_columns(&columns);
        self.fill_list(dialog, &self.start.order, Some(0));
        controls::set_checked(dialog.item(ids::V8), self.start.v8);
        let level = Combo::of(dialog.item(ids::LEVEL));
        // The loudest first, as a list of levels reads.
        for value in (LEVELS.0..=LEVELS.1).rev() {
            level.add(&level_text(value));
        }
        level.select(usize::try_from(LEVELS.1 - self.start.level).ok());
        let error_control = Combo::of(dialog.item(ids::ERROR_CONTROL));
        for (_, text) in ERROR_CONTROLS {
            error_control.add(text);
        }
        error_control.select(Some(place(&ERROR_CONTROLS, &self.start.error_control)));
        controls::set_checked(dialog.item(ids::COMPRESSION), self.start.compression);
        let guard_tone = Combo::of(dialog.item(ids::GUARD_TONE));
        for (_, text) in GUARD_TONES {
            guard_tone.add(text);
        }
        guard_tone.select(Some(place(&GUARD_TONES, &self.start.guard_tone)));
        match self.kind {
            Kind::Service => {
                let delay = dialog.item(ids::DELAY);
                UpDown::of(dialog.item(ids::DELAY_ARROWS)).attach(
                    delay,
                    DELAYS.0 as i32,
                    DELAYS.1 as i32,
                );
                // The up-down control writes its own idea of the number.
                delay.set_text(&self.start.delay);
                let fallback = dialog.item(ids::FALLBACK);
                UpDown::of(dialog.item(ids::FALLBACK_ARROWS)).attach(
                    fallback,
                    FALLBACKS.0 as i32,
                    FALLBACKS.1 as i32,
                );
                fallback.set_text(&self.start.fallback);
            }
            Kind::Line => {
                controls::set_checked(dialog.item(ids::OWN), self.start.own);
                self.show_own(dialog);
            }
        }
    }

    fn read(&self, dialog: Hwnd) -> Form {
        let text = |control: u16| match self.kind {
            Kind::Service => dialog.item(control).text(),
            Kind::Line => String::new(),
        };
        Form {
            order: self.order(dialog),
            v8: controls::checked(dialog.item(ids::V8)),
            level: Combo::of(dialog.item(ids::LEVEL))
                .selected()
                .map_or(self.start.level, |index| LEVELS.1 - index as i32),
            error_control: Combo::of(dialog.item(ids::ERROR_CONTROL))
                .selected()
                .and_then(|index| ERROR_CONTROLS.get(index))
                .map_or_else(
                    || self.start.error_control.clone(),
                    |(name, _)| name.to_string(),
                ),
            compression: controls::checked(dialog.item(ids::COMPRESSION)),
            guard_tone: Combo::of(dialog.item(ids::GUARD_TONE))
                .selected()
                .and_then(|index| GUARD_TONES.get(index))
                .map_or_else(
                    || self.start.guard_tone.clone(),
                    |(name, _)| name.to_string(),
                ),
            delay: text(ids::DELAY),
            fallback: text(ids::FALLBACK),
            own: match self.kind {
                Kind::Service => true,
                Kind::Line => controls::checked(dialog.item(ids::OWN)),
            },
        }
    }

    /// Takes what the dialog holds: the service's settings are sent to
    /// the service, a line's are kept for the line's dialog.
    fn accept(&self, dialog: Hwnd) -> Result<(), ErrorInfo> {
        let form = self.read(dialog);
        match self.kind {
            Kind::Service => {
                let Some(change) = form.change(&self.current)? else {
                    return Ok(());
                };
                let call = Call::ConfigSet(SettingsChange {
                    modem: Some(change),
                    ..SettingsChange::default()
                });
                self.manager.call::<SettingsResult>(dialog, call)?;
                // The lines show the settings their calls use.
                self.manager.refresh_lines();
            }
            Kind::Line => *self.result.borrow_mut() = Some(form.spec()?),
        }
        Ok(())
    }
}

impl Handler for ModemDialog {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                self.fill(dialog);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } => {
                let name = match self.kind {
                    Kind::Service => "modem",
                    Kind::Line => "line-modem",
                };
                if watched(&self.manager, dialog, id, name) {
                    Reply::Handled
                } else {
                    Reply::Default
                }
            }
            Event::Command { id: ids::UP, .. } => {
                self.move_row(dialog, false);
                Reply::Handled
            }
            Event::Command { id: ids::DOWN, .. } => {
                self.move_row(dialog, true);
                Reply::Handled
            }
            Event::Command { id: ids::OWN, .. } => {
                self.show_own(dialog);
                watch(&self.manager, dialog);
                Reply::Handled
            }
            Event::ListSelection { id: ids::LIST } => {
                Self::show_moves(dialog);
                Reply::Handled
            }
            Event::Command { id: id::OK, .. } => {
                match self.accept(dialog) {
                    Ok(()) => dialog.end_dialog(id::OK),
                    // The dialog stays, with what was chosen.
                    Err(error) => {
                        self.manager.show_error(dialog, &error);
                        watch(&self.manager, dialog);
                    }
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Rate cap and modulations: the `[modem]` settings of the service.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    let current = match manager.call::<Settings>(owner, Call::ConfigGet) {
        Ok(settings) => settings.modem,
        Err(error) => {
            manager.show_error(owner, &error);
            return;
        }
    };
    let handler = Rc::new(ModemDialog {
        manager: Rc::clone(manager),
        kind: Kind::Service,
        start: Form::of_service(&current),
        current,
        rows: RefCell::new(Vec::new()),
        result: RefCell::new(None),
    });
    template(Kind::Service, "Rate cap and modulations").show_modal(Some(owner), handler);
}

/// A line's own modem settings, starting from the ones its calls use.
/// Returns what the dialog was closed with by OK: the line's own
/// settings, which are none when the line is to have the service's.
pub fn line_settings(
    manager: &Rc<Manager>,
    owner: Hwnd,
    start: &LineModem,
) -> Option<LineModemSpec> {
    let handler = Rc::new(ModemDialog {
        manager: Rc::clone(manager),
        kind: Kind::Line,
        start: Form::of_line(start),
        current: ModemSettings::default(),
        rows: RefCell::new(Vec::new()),
        result: RefCell::new(None),
    });
    let dialog: Rc<dyn Handler> = Rc::clone(&handler) as Rc<dyn Handler>;
    let closed = template(Kind::Line, "Modem settings of the line").show_modal(Some(owner), dialog);
    if closed != id::OK {
        return None;
    }
    handler.result.borrow_mut().take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ModemSettings {
        ModemSettings {
            modulations: vec!["v21".to_string(), "bell103".to_string()],
            v8: true,
            tx_level_dbm0: -12,
            answer_delay_ms: 0,
            fallback_ms: 3000,
            error_control: "auto".to_string(),
            compression: true,
            guard_tone: "off".to_string(),
        }
    }

    /// The names of the rows in their order.
    fn names(form: &Form) -> Vec<(&str, bool)> {
        form.order
            .iter()
            .map(|(name, checked)| (name.as_str(), *checked))
            .collect()
    }

    #[test]
    fn the_list_has_every_modulation_with_the_allowed_ones_first() {
        let form = Form::of_service(&settings());
        assert_eq!(
            names(&form),
            [
                ("v21", true),
                ("bell103", true),
                ("v22bis", false),
                ("v22", false),
                ("bell212a", false)
            ]
        );
        let bell_first = ModemSettings {
            modulations: vec!["bell103".to_string(), "v22bis".to_string()],
            ..settings()
        };
        assert_eq!(
            names(&Form::of_service(&bell_first)),
            [
                ("bell103", true),
                ("v22bis", true),
                ("v22", false),
                ("bell212a", false),
                ("v21", false)
            ]
        );
        // A modulation this manager does not know is not shown.
        let newer = ModemSettings {
            modulations: vec!["v34".to_string(), "v21".to_string()],
            ..settings()
        };
        assert_eq!(names(&Form::of_service(&newer))[0], ("v21", true));
        assert_eq!(Form::of_service(&newer).order.len(), 5);
        // Nor is a policy or a guard tone it does not know: the first of
        // each list stands for it.
        assert_eq!(place(&ERROR_CONTROLS, "required"), 1);
        assert_eq!(place(&ERROR_CONTROLS, "sometimes"), 0);
        assert_eq!(place(&GUARD_TONES, "550"), 2);
    }

    #[test]
    fn only_what_was_changed_is_sent() {
        let current = settings();
        let form = Form::of_service(&current);
        assert_eq!(form.change(&current), Ok(None));

        let mut changed = form.clone();
        changed.order.swap(0, 1);
        changed.level = -15;
        let change = changed.change(&current).unwrap().unwrap();
        assert_eq!(
            change,
            ModemChange {
                modulations: Some(vec!["bell103".to_string(), "v21".to_string()]),
                tx_level_dbm0: Some(-15),
                ..ModemChange::default()
            }
        );

        let mut unchecked = form.clone();
        unchecked.order[1].1 = false;
        unchecked.v8 = false;
        unchecked.delay = " 1500 ".to_string();
        unchecked.fallback = "2000".to_string();
        unchecked.error_control = "required".to_string();
        unchecked.compression = false;
        unchecked.guard_tone = "1800".to_string();
        assert_eq!(
            unchecked.change(&current).unwrap().unwrap(),
            ModemChange {
                modulations: Some(vec!["v21".to_string()]),
                v8: Some(false),
                tx_level_dbm0: None,
                answer_delay_ms: Some(1500),
                fallback_ms: Some(2000),
                error_control: Some("required".to_string()),
                compression: Some(false),
                guard_tone: Some("1800".to_string()),
            }
        );
    }

    #[test]
    fn what_cannot_be_is_said_before_anything_is_sent() {
        let current = settings();
        let mut none = Form::of_service(&current);
        for row in &mut none.order {
            row.1 = false;
        }
        let error = none.change(&current).unwrap_err();
        assert_eq!(error.msgid, "RMD4015");
        assert_eq!(none.spec().unwrap_err().msgid, "RMD4015");

        let mut late = Form::of_service(&current);
        late.delay = "10001".to_string();
        let error = late.change(&current).unwrap_err();
        assert_eq!(
            error.text,
            "Answer delay must be a whole number from 0 to 10000."
        );
        let mut short = Form::of_service(&current);
        short.fallback = "499".to_string();
        assert_eq!(
            short.change(&current).unwrap_err().text,
            "Time for each modulation must be a whole number from 500 to 20000."
        );
    }

    #[test]
    fn a_line_has_all_of_its_own_settings_or_none() {
        let line = LineModem {
            modulations: vec!["bell103".to_string()],
            v8: false,
            tx_level_dbm0: -15,
            error_control: "required".to_string(),
            compression: false,
            guard_tone: "550".to_string(),
            own: vec!["v8".to_string()],
        };
        let form = Form::of_line(&line);
        assert!(form.own);
        let spec = form.spec().unwrap();
        assert_eq!(
            spec,
            LineModemSpec {
                modulations: Some(vec!["bell103".to_string()]),
                v8: Some(false),
                tx_level_dbm0: Some(-15),
                error_control: Some("required".to_string()),
                compression: Some(false),
                guard_tone: Some("550".to_string()),
            }
        );
        // As the line's dialog holds it until it is sent.
        let held = line_modem_of(&spec, &line);
        assert_eq!(
            held.own,
            [
                "modulations",
                "v8",
                "tx_level_dbm0",
                "error_control",
                "compression",
                "guard_tone"
            ]
        );
        assert_eq!(
            (
                held.error_control.as_str(),
                held.compression,
                held.guard_tone.as_str()
            ),
            ("required", false, "550")
        );
        assert_eq!(held.modulations, ["bell103"]);

        let mut none = form.clone();
        none.own = false;
        // Even with nothing checked: the list is not the line's to say.
        for row in &mut none.order {
            row.1 = false;
        }
        assert_eq!(none.spec(), Ok(LineModemSpec::default()));
        let held = line_modem_of(&LineModemSpec::default(), &line);
        assert!(held.own.is_empty());
        assert_eq!((held.v8, held.tx_level_dbm0), (false, -15));

        let inherits = LineModem {
            own: Vec::new(),
            ..line
        };
        assert!(!Form::of_line(&inherits).own);
    }

    #[test]
    fn the_templates_have_their_controls_in_tab_order() {
        let service = template(Kind::Service, "x").ids();
        assert!(!service.contains(&ids::OWN));
        assert!(service.contains(&ids::DELAY) && service.contains(&ids::FALLBACK));
        let line = template(Kind::Line, "x").ids();
        assert_eq!(line[0], ids::OWN);
        assert!(!line.contains(&ids::DELAY));
        for ids in [&service, &line] {
            let at = |id: u16| ids.iter().position(|other| *other == id).unwrap();
            assert!(at(ids::LIST) < at(ids::UP) && at(ids::UP) < at(ids::DOWN));
            assert!(at(ids::DOWN) < at(ids::V8) && at(ids::V8) < at(ids::LEVEL));
            assert!(at(ids::LEVEL) < at(ids::ERROR_CONTROL));
            assert!(at(ids::ERROR_CONTROL) < at(ids::COMPRESSION));
            assert!(at(ids::COMPRESSION) < at(ids::GUARD_TONE));
        }
        assert_eq!(level_text(-12), "-12 dBm0");
    }
}
