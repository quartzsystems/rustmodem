// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Settings: the settings of the service that are not lines, as a
//! property sheet.
//!
//! There are two pages now, Service and Control. A later phase adds a
//! page by adding to [`pages`]: each page is a template, and a handler
//! that fills it from the settings, says what the user changed, and is
//! asked to apply it.
//!
//! OK and Apply send `config.set` with what was changed and nothing else.
//! The service checks it; if it refuses, its message is shown and the
//! sheet stays open on the page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ctlproto::{
    Call, ControlChange, ErrorInfo, LogLevel, ServiceChange, Settings, SettingsChange,
    SettingsResult,
};
use win32::controls::Combo;
use win32::{DialogTemplate, Event, Handler, Hwnd, Reply, code, property_sheet};

use super::{must_not_be_empty, watch, watched};
use crate::window::Manager;

/// The ids of the controls of the pages.
pub mod ids {
    /// The log level, on the Service page.
    pub const LEVEL: u16 = 300;
    /// The log directory, on the Service page.
    pub const DIRECTORY: u16 = 301;
    /// Browse, beside the log directory.
    pub const BROWSE: u16 = 302;
    /// The pipe name, on the Control page.
    pub const PIPE: u16 = 303;
}

use ids::{BROWSE as ID_BROWSE, DIRECTORY as ID_DIRECTORY, LEVEL as ID_LEVEL, PIPE as ID_PIPE};

/// The log levels, in the order the combo box lists them.
const LEVELS: [(LogLevel, &str); 4] = [
    (LogLevel::Error, "Error"),
    (LogLevel::Warning, "Warning"),
    (LogLevel::Info, "Info"),
    (LogLevel::Debug, "Debug"),
];

/// What the Service page says under its fields.
const SERVICE_NOTE: &str = "Changes take effect as soon as they are applied.";

/// What the Control page says under its field.
const CONTROL_NOTE: &str = "A change takes effect when the service next starts.";

fn service_template() -> DialogTemplate {
    DialogTemplate::new("", 240, 110)
        .label(7, 9, 70, 8, "&Log level:")
        .combo(ID_LEVEL, 80, 7, 80, 70)
        .label(7, 27, 70, 8, "Log &directory:")
        .edit(ID_DIRECTORY, 80, 25, 104, 14)
        .button(ID_BROWSE, 188, 25, 45, 14, "B&rowse\u{2026}")
        .label(7, 48, 226, 8, SERVICE_NOTE)
}

fn control_template() -> DialogTemplate {
    DialogTemplate::new("", 240, 110)
        .label(7, 9, 70, 8, "&Pipe name:")
        .edit(ID_PIPE, 80, 7, 153, 14)
        .label(7, 30, 226, 8, CONTROL_NOTE)
}

/// What the pages share: the settings as the service last gave them, and
/// whether a change needs the service started again.
struct Shared {
    manager: Rc<Manager>,
    settings: RefCell<Settings>,
    restart: Cell<bool>,
}

impl Shared {
    /// Sends a change. On a refusal the message is shown over the sheet
    /// and the page is told to stay.
    fn apply(&self, page: Hwnd, change: SettingsChange) -> Reply {
        let sheet = page.parent().unwrap_or(page);
        match self
            .manager
            .call::<SettingsResult>(sheet, Call::ConfigSet(change))
        {
            Ok(result) => {
                *self.settings.borrow_mut() = result.config;
                if result.restart_required {
                    self.restart.set(true);
                }
                Reply::Default
            }
            Err(error) => {
                self.manager.show_error(sheet, &error);
                Reply::Value(1)
            }
        }
    }

    fn refuse(&self, page: Hwnd, error: &ErrorInfo) -> Reply {
        self.manager
            .show_error(page.parent().unwrap_or(page), error);
        Reply::Value(1)
    }
}

/// What the Service page changes: only what is not as the service has
/// it.
fn service_change(settings: &Settings, level: LogLevel, directory: &str) -> Option<ServiceChange> {
    let directory = directory.trim();
    let change = ServiceChange {
        log_level: (level != settings.service.log_level).then_some(level),
        log_dir: (directory != settings.service.log_dir).then(|| directory.to_string()),
    };
    (change.log_level.is_some() || change.log_dir.is_some()).then_some(change)
}

/// What the Control page changes.
fn control_change(settings: &Settings, pipe: &str) -> Result<Option<ControlChange>, ErrorInfo> {
    let pipe = pipe.trim();
    if pipe.is_empty() {
        return Err(must_not_be_empty("Pipe name"));
    }
    Ok((pipe != settings.control.pipe).then(|| ControlChange {
        pipe: Some(pipe.to_string()),
    }))
}

struct ServicePage {
    shared: Rc<Shared>,
    /// The page has been filled in: a change from here on is the user's.
    filled: Cell<bool>,
}

impl Handler for ServicePage {
    fn event(&self, page: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let settings = self.shared.settings.borrow();
                let level = Combo::of(page.item(ID_LEVEL));
                for (_, name) in LEVELS {
                    level.add(name);
                }
                level.select(
                    LEVELS
                        .iter()
                        .position(|(level, _)| *level == settings.service.log_level),
                );
                page.item(ID_DIRECTORY).set_text(&settings.service.log_dir);
                self.filled.set(true);
                // The first page is the one a test is told of.
                watch(&self.shared.manager, page);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.shared.manager, page, id, "settings") => {
                Reply::Handled
            }
            Event::Command { id: ID_BROWSE, .. } => {
                let sheet = page.parent().unwrap_or(page);
                if let Some(folder) = self.shared.manager.pick_folder(sheet, "Log directory") {
                    page.item(ID_DIRECTORY)
                        .set_text(&folder.display().to_string());
                    page.page_changed();
                }
                Reply::Handled
            }
            Event::Command {
                id: ID_LEVEL,
                code: code::COMBO_SELECTION,
            }
            | Event::Command {
                id: ID_DIRECTORY,
                code: code::EDIT_CHANGE,
            } => {
                if self.filled.get() {
                    page.page_changed();
                }
                Reply::Handled
            }
            Event::Apply => {
                let level = Combo::of(page.item(ID_LEVEL))
                    .selected()
                    .and_then(|index| LEVELS.get(index))
                    .map(|(level, _)| *level);
                let change = {
                    let settings = self.shared.settings.borrow();
                    let level = level.unwrap_or(settings.service.log_level);
                    service_change(&settings, level, &page.item(ID_DIRECTORY).text())
                };
                match change {
                    Some(service) => self.shared.apply(
                        page,
                        SettingsChange {
                            service: Some(service),
                            control: None,
                            ..SettingsChange::default()
                        },
                    ),
                    None => Reply::Default,
                }
            }
            _ => Reply::Default,
        }
    }
}

struct ControlPage {
    shared: Rc<Shared>,
    /// The page has been filled in: a change from here on is the user's.
    filled: Cell<bool>,
}

impl Handler for ControlPage {
    fn event(&self, page: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let settings = self.shared.settings.borrow();
                page.item(ID_PIPE).set_text(&settings.control.pipe);
                self.filled.set(true);
                watch(&self.shared.manager, page);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.shared.manager, page, id, "settings-control") => {
                Reply::Handled
            }
            Event::Command {
                id: ID_PIPE,
                code: code::EDIT_CHANGE,
            } => {
                if self.filled.get() {
                    page.page_changed();
                }
                Reply::Handled
            }
            Event::Apply => {
                let change = {
                    let settings = self.shared.settings.borrow();
                    control_change(&settings, &page.item(ID_PIPE).text())
                };
                match change {
                    Ok(Some(control)) => self.shared.apply(
                        page,
                        SettingsChange {
                            service: None,
                            control: Some(control),
                            ..SettingsChange::default()
                        },
                    ),
                    Ok(None) => Reply::Default,
                    Err(error) => self.shared.refuse(page, &error),
                }
            }
            _ => Reply::Default,
        }
    }
}

/// The pages of the sheet. A later phase adds its page here.
fn pages(shared: &Rc<Shared>) -> Vec<(String, DialogTemplate, Rc<dyn Handler>)> {
    vec![
        (
            "Service".to_string(),
            service_template(),
            Rc::new(ServicePage {
                shared: Rc::clone(shared),
                filled: Cell::new(false),
            }),
        ),
        (
            "Control".to_string(),
            control_template(),
            Rc::new(ControlPage {
                shared: Rc::clone(shared),
                filled: Cell::new(false),
            }),
        ),
    ]
}

/// Shows the settings and sends what the user changes. Returns the
/// settings as the service has them afterwards when a change needs the
/// service started again, and `None` when nothing does.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) -> Option<Settings> {
    // Asked for before the sheet opens, so that it opens filled in.
    let settings = match manager.call::<Settings>(owner, Call::ConfigGet) {
        Ok(settings) => settings,
        Err(error) => {
            manager.show_error(owner, &error);
            return None;
        }
    };
    let shared = Rc::new(Shared {
        manager: Rc::clone(manager),
        settings: RefCell::new(settings),
        restart: Cell::new(false),
    });
    property_sheet(Some(owner), "Settings", pages(&shared));
    let settings = shared.settings.borrow().clone();
    shared.restart.get().then_some(settings)
}

#[cfg(test)]
mod tests {
    use ctlproto::{ControlSettings, ServiceSettings};

    use super::*;

    fn settings() -> Settings {
        Settings {
            service: ServiceSettings {
                log_level: LogLevel::Info,
                log_dir: r"C:\ProgramData\Quartz Systems\rustmodem\logs".to_string(),
            },
            control: ControlSettings {
                pipe: "rustmodem-control".to_string(),
            },
            path: r"C:\ProgramData\Quartz Systems\rustmodem\rustmodem.toml".to_string(),
            sip: ctlproto::SipSettings::default(),
            media: ctlproto::MediaSettings::default(),
        }
    }

    #[test]
    fn a_page_that_was_not_touched_changes_nothing() {
        let settings = settings();
        assert_eq!(
            service_change(&settings, LogLevel::Info, &settings.service.log_dir),
            None
        );
        assert_eq!(control_change(&settings, " rustmodem-control "), Ok(None));
    }

    #[test]
    fn only_what_was_changed_is_sent() {
        let settings = settings();
        let level = service_change(&settings, LogLevel::Debug, &settings.service.log_dir).unwrap();
        assert_eq!(level.log_level, Some(LogLevel::Debug));
        assert_eq!(level.log_dir, None);

        let directory = service_change(&settings, LogLevel::Info, r" D:\logs ").unwrap();
        assert_eq!(directory.log_level, None);
        assert_eq!(directory.log_dir.as_deref(), Some(r"D:\logs"));

        // An empty directory is the default one, and is a change.
        let cleared = service_change(&settings, LogLevel::Info, "").unwrap();
        assert_eq!(cleared.log_dir.as_deref(), Some(""));

        let pipe = control_change(&settings, "other").unwrap().unwrap();
        assert_eq!(pipe.pipe.as_deref(), Some("other"));
    }

    #[test]
    fn a_pipe_must_have_a_name() {
        let error = control_change(&settings(), "   ").unwrap_err();
        assert_eq!(error.msgid, "RMD4009");
        assert_eq!(error.text, "Pipe name must not be empty.");
    }

    #[test]
    fn every_log_level_can_be_chosen() {
        assert_eq!(LEVELS.len(), 4);
        for level in [
            LogLevel::Error,
            LogLevel::Warning,
            LogLevel::Info,
            LogLevel::Debug,
        ] {
            assert!(LEVELS.iter().any(|(listed, _)| *listed == level));
        }
    }
}
