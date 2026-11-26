// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The configuration file: schema, validation and comment-preserving
//! write-back.
//!
//! The file is TOML. [`ConfigFile`] holds both the parsed, validated
//! [`Config`] and the document it came from, so a change made through the
//! API is written back without disturbing comments, ordering, or sections
//! this version does not know (later phases add `[sip]`, `[media]` and
//! others; they are accepted and preserved untouched).
//!
//! Validation reports every problem at once, each as an `RMD` message naming
//! the offending key and its line number.

#![forbid(unsafe_code)]

mod schema;
mod validate;
mod write;

use std::path::{Path, PathBuf};

use msg::{Message, message};
use toml_edit::DocumentMut;

pub use schema::{
    Backend, Config, ControlConfig, LineConfig, LineEdit, LogLevel, MAX_LINE_ID, SerialConfig,
    ServiceConfig, SettingsEdit, UnknownCommand,
};

/// The directory under `%ProgramData%` that holds rustmodem's files.
const DATA_SUBDIR: [&str; 2] = ["Quartz Systems", "rustmodem"];

/// The directory rustmodem keeps its configuration and logs in:
/// `%ProgramData%\Quartz Systems\rustmodem`.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join(DATA_SUBDIR[0]).join(DATA_SUBDIR[1])
}

/// The default configuration file path.
pub fn default_path() -> PathBuf {
    data_dir().join("rustmodem.toml")
}

/// The log directory used when `service.log_dir` is empty.
pub fn default_log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// A configuration file: the validated configuration and the document that
/// carries its comments and ordering.
#[derive(Clone, Debug)]
pub struct ConfigFile {
    document: DocumentMut,
    config: Config,
}

impl ConfigFile {
    /// Parses and validates configuration text. Every problem found is
    /// returned, not just the first.
    pub fn parse(text: &str) -> Result<ConfigFile, Vec<Message>> {
        let (document, config) = validate::parse(text)?;
        Ok(ConfigFile { document, config })
    }

    /// Reads, parses and validates the file at `path`.
    pub fn load(path: &Path) -> Result<ConfigFile, Vec<Message>> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            vec![message(
                "RMD0010",
                &[&path.display().to_string(), &e.to_string()],
            )]
        })?;
        ConfigFile::parse(&text)
    }

    /// Builds a new file, with no comments, from a configuration.
    pub fn from_config(config: &Config) -> Result<ConfigFile, Vec<Message>> {
        ConfigFile::parse(&write::render(config))
    }

    /// The validated configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The file as text, comments and ordering intact.
    pub fn to_toml(&self) -> String {
        self.document.to_string()
    }

    /// Writes the file to `path`. The text goes to a temporary file beside
    /// it first, so a failed write does not leave half a configuration.
    pub fn save(&self, path: &Path) -> Result<(), Message> {
        let failed =
            |e: std::io::Error| message("RMD0011", &[&path.display().to_string(), &e.to_string()]);
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".new");
        let temporary = PathBuf::from(temporary);
        std::fs::write(&temporary, self.to_toml()).map_err(failed)?;
        std::fs::rename(&temporary, path).map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            failed(e)
        })
    }

    /// Adds a line. Fails, changing nothing, if the id is in use or the
    /// result would not be valid.
    pub fn add_line(&mut self, line: &LineConfig) -> Result<(), Vec<Message>> {
        if self.config.line(line.id).is_some() {
            return Err(vec![message("RMD0020", &[&line.id.to_string()])]);
        }
        let mut document = self.document.clone();
        write::add_line(&mut document, line);
        self.commit(document)
    }

    /// Changes the keys of line `id` that `edit` names. Fails, changing
    /// nothing, if there is no such line or the result would not be valid.
    pub fn edit_line(&mut self, id: u16, edit: &LineEdit) -> Result<(), Vec<Message>> {
        let Some(index) = self.config.lines.iter().position(|l| l.id == id) else {
            return Err(vec![message("RMD0019", &[&id.to_string()])]);
        };
        let mut document = self.document.clone();
        write::edit_line(&mut document, index, edit);
        self.commit(document)
    }

    /// Removes line `id`. Fails, changing nothing, if there is no such line.
    pub fn remove_line(&mut self, id: u16) -> Result<(), Vec<Message>> {
        let Some(index) = self.config.lines.iter().position(|l| l.id == id) else {
            return Err(vec![message("RMD0019", &[&id.to_string()])]);
        };
        let mut document = self.document.clone();
        write::remove_line(&mut document, index);
        self.commit(document)
    }

    /// Changes the keys of `[service]` and `[control]` that `edit` names.
    /// Fails, changing nothing, if the result would not be valid.
    pub fn edit_settings(&mut self, edit: &SettingsEdit) -> Result<(), Vec<Message>> {
        let mut document = self.document.clone();
        write::edit_settings(&mut document, edit);
        self.commit(document)
    }

    /// Validates a changed document by reading its text back, exactly as a
    /// later load would, and adopts it only if it is valid.
    fn commit(&mut self, document: DocumentMut) -> Result<(), Vec<Message>> {
        let (document, config) = validate::parse(&document.to_string())?;
        self.document = document;
        self.config = config;
        Ok(())
    }
}
