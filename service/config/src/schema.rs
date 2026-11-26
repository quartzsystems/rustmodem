// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The configuration as the rest of the service uses it.

use std::fmt;

/// The highest line id.
pub const MAX_LINE_ID: u16 = 999;

/// How much the service logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Errors and warnings.
    Warning,
    /// Errors, warnings and information.
    #[default]
    Info,
    /// Everything, including every AT command line and result code.
    Debug,
}

impl LogLevel {
    /// Every level, least verbose first.
    pub const ALL: [LogLevel; 4] = [
        LogLevel::Error,
        LogLevel::Warning,
        LogLevel::Info,
        LogLevel::Debug,
    ];

    /// The level as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warning => "warning",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    /// Reads a level as the configuration spells it.
    pub fn parse(text: &str) -> Option<LogLevel> {
        LogLevel::ALL.into_iter().find(|l| l.as_str() == text)
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which serial backend a line uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Backend {
    /// One side of a com0com null-modem pair.
    #[default]
    Com0com,
}

impl Backend {
    /// The backend as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Com0com => "com0com",
        }
    }

    /// Reads a backend as the configuration spells it.
    pub fn parse(text: &str) -> Option<Backend> {
        match text {
            "com0com" => Some(Backend::Com0com),
            _ => None,
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a line answers to an AT command it does not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum UnknownCommand {
    /// Skip the command and carry on, as if it had worked.
    #[default]
    Ok,
    /// Stop the command line with `ERROR`.
    Error,
}

impl UnknownCommand {
    /// The setting as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            UnknownCommand::Ok => "ok",
            UnknownCommand::Error => "error",
        }
    }

    /// Reads the setting as the configuration spells it.
    pub fn parse(text: &str) -> Option<UnknownCommand> {
        match text {
            "ok" => Some(UnknownCommand::Ok),
            "error" => Some(UnknownCommand::Error),
            _ => None,
        }
    }
}

impl fmt::Display for UnknownCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `[service]` table.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ServiceConfig {
    /// How much the service logs.
    pub log_level: LogLevel,
    /// The log directory; empty means the default one.
    pub log_dir: String,
}

/// The `[control]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlConfig {
    /// The control pipe is `\\.\pipe\<pipe>`.
    pub pipe: String,
}

impl ControlConfig {
    /// The default pipe name.
    pub const DEFAULT_PIPE: &'static str = "rustmodem-control";
}

impl Default for ControlConfig {
    fn default() -> Self {
        ControlConfig {
            pipe: ControlConfig::DEFAULT_PIPE.to_string(),
        }
    }
}

/// The `serial` table of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialConfig {
    /// Which backend opens the port.
    pub backend: Backend,
    /// The port name as the backend knows it, for example `CNCB0`.
    pub port: String,
}

/// One `[[line]]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineConfig {
    /// The line id, 1 to [`MAX_LINE_ID`], unique.
    pub id: u16,
    /// A name for people; optional.
    pub name: Option<String>,
    /// The serial port of the line.
    pub serial: SerialConfig,
    /// Whether the line is in service.
    pub enabled: bool,
    /// The initial value of S0: rings before auto-answer, 0 for off.
    pub auto_answer_rings: u8,
    /// What the line answers to an AT command it does not know.
    pub unknown_command: UnknownCommand,
}

impl LineConfig {
    /// A line with the given id and port and every other key at its default.
    pub fn new(id: u16, port: &str) -> LineConfig {
        LineConfig {
            id,
            name: None,
            serial: SerialConfig {
                backend: Backend::default(),
                port: port.to_string(),
            },
            enabled: true,
            auto_answer_rings: 0,
            unknown_command: UnknownCommand::default(),
        }
    }
}

/// A change to one line: each `Some` replaces that key, each `None` leaves
/// it alone. For `name`, `Some(None)` removes the key.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineEdit {
    /// A new name, or `Some(None)` to remove the name.
    pub name: Option<Option<String>>,
    /// A new serial backend.
    pub backend: Option<Backend>,
    /// A new serial port.
    pub port: Option<String>,
    /// Whether the line is in service.
    pub enabled: Option<bool>,
    /// A new initial S0.
    pub auto_answer_rings: Option<u8>,
    /// A new unknown-command setting.
    pub unknown_command: Option<UnknownCommand>,
}

/// A change to the settings that are not lines: each `Some` replaces that
/// key, each `None` leaves it alone.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SettingsEdit {
    /// A new `service.log_level`.
    pub log_level: Option<LogLevel>,
    /// A new `service.log_dir`; empty means the default directory.
    pub log_dir: Option<String>,
    /// A new `control.pipe`.
    pub pipe: Option<String>,
}

/// A whole, valid configuration.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Config {
    /// The `[service]` table.
    pub service: ServiceConfig,
    /// The `[control]` table.
    pub control: ControlConfig,
    /// The `[[line]]` tables, in file order.
    pub lines: Vec<LineConfig>,
}

impl Config {
    /// The line with the given id.
    pub fn line(&self, id: u16) -> Option<&LineConfig> {
        self.lines.iter().find(|l| l.id == id)
    }
}
