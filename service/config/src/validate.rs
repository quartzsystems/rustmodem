// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Parsing and validation. Every problem is collected and reported at once,
//! each with the offending key and its line number.

use msg::{Message, message};
use toml_edit::{Document, DocumentMut, Item, TableLike};

use crate::schema::{
    Backend, Config, ControlConfig, LineConfig, LogLevel, MAX_LINE_ID, SerialConfig, ServiceConfig,
    UnknownCommand,
};

/// The longest pipe or port name accepted.
const MAX_NAME_LEN: usize = 64;

/// Parses and validates configuration text.
pub(crate) fn parse(text: &str) -> Result<(DocumentMut, Config), Vec<Message>> {
    let document = Document::parse(text).map_err(|e| {
        let line = e.span().map(|span| line_of(text, span.start)).unwrap_or(1);
        vec![message("RMD0012", &[&line.to_string(), e.message()])]
    })?;
    let mut reader = Reader {
        text,
        problems: Vec::new(),
    };
    let config = reader.config(document.as_table());
    if reader.problems.is_empty() {
        Ok((document.into_mut(), config))
    } else {
        Err(reader.problems)
    }
}

/// The 1-based line number of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

struct Reader<'a> {
    text: &'a str,
    problems: Vec<Message>,
}

impl Reader<'_> {
    fn line(&self, item: &Item) -> usize {
        item.span()
            .map(|s| line_of(self.text, s.start))
            .unwrap_or(0)
    }

    fn wrong_type(&mut self, key: &str, item: &Item, expected: &str) {
        let line = self.line(item).to_string();
        self.problems
            .push(message("RMD0013", &[key, &line, expected]));
    }

    fn not_allowed(&mut self, key: &str, item: &Item, value: &str, allowed: &str) {
        let line = self.line(item).to_string();
        self.problems
            .push(message("RMD0014", &[key, &line, value, allowed]));
    }

    /// A table-valued key, or `None` (with a problem recorded) if the key is
    /// present but is not a table.
    fn table<'t>(
        &mut self,
        parent: &'t dyn TableLike,
        key: &str,
        path: &str,
    ) -> Option<&'t dyn TableLike> {
        let item = parent.get(key)?;
        match item.as_table_like() {
            Some(table) => Some(table),
            None => {
                self.wrong_type(path, item, "table");
                None
            }
        }
    }

    fn string<'t>(
        &mut self,
        table: &'t dyn TableLike,
        key: &str,
        path: &str,
    ) -> Option<(&'t str, &'t Item)> {
        let item = table.get(key)?;
        match item.as_str() {
            Some(value) => Some((value, item)),
            None => {
                self.wrong_type(path, item, "string");
                None
            }
        }
    }

    fn integer<'t>(
        &mut self,
        table: &'t dyn TableLike,
        key: &str,
        path: &str,
    ) -> Option<(i64, &'t Item)> {
        let item = table.get(key)?;
        match item.as_integer() {
            Some(value) => Some((value, item)),
            None => {
                self.wrong_type(path, item, "integer");
                None
            }
        }
    }

    fn boolean(&mut self, table: &dyn TableLike, key: &str, path: &str) -> Option<bool> {
        let item = table.get(key)?;
        match item.as_bool() {
            Some(value) => Some(value),
            None => {
                self.wrong_type(path, item, "boolean");
                None
            }
        }
    }

    fn config(&mut self, root: &dyn TableLike) -> Config {
        let mut config = Config::default();
        if let Some(table) = self.table(root, "service", "service") {
            config.service = self.service(table);
        }
        if let Some(table) = self.table(root, "control", "control") {
            config.control = self.control(table);
        }
        if let Some(item) = root.get("line") {
            match item.as_array_of_tables() {
                Some(lines) => self.lines(lines, &mut config.lines),
                None => self.wrong_type("line", item, "array of tables ([[line]])"),
            }
        }
        config
    }

    fn service(&mut self, table: &dyn TableLike) -> ServiceConfig {
        let mut service = ServiceConfig::default();
        if let Some((value, item)) = self.string(table, "log_level", "service.log_level") {
            match LogLevel::parse(value) {
                Some(level) => service.log_level = level,
                None => self.not_allowed(
                    "service.log_level",
                    item,
                    value,
                    "error, warning, info, debug",
                ),
            }
        }
        if let Some((value, _)) = self.string(table, "log_dir", "service.log_dir") {
            service.log_dir = value.to_string();
        }
        service
    }

    fn control(&mut self, table: &dyn TableLike) -> ControlConfig {
        let mut control = ControlConfig::default();
        if let Some((value, item)) = self.string(table, "pipe", "control.pipe") {
            let valid = !value.is_empty()
                && value.len() <= MAX_NAME_LEN
                && value
                    .chars()
                    .all(|c| c.is_ascii_graphic() && c != '\\' && c != '/');
            if valid {
                control.pipe = value.to_string();
            } else {
                self.not_allowed(
                    "control.pipe",
                    item,
                    value,
                    "1 to 64 printable ASCII characters with no space, \\ or /",
                );
            }
        }
        control
    }

    fn lines(&mut self, tables: &toml_edit::ArrayOfTables, lines: &mut Vec<LineConfig>) {
        // (value, line number) of every id and port seen, to report
        // duplicates against where the value was first used.
        let mut ids: Vec<(u16, usize)> = Vec::new();
        let mut ports: Vec<(String, usize)> = Vec::new();

        for table in tables.iter() {
            let table_line = table
                .span()
                .map(|s| line_of(self.text, s.start))
                .unwrap_or(0);
            let before = self.problems.len();
            let mut line = LineConfig::new(0, "");

            match self.integer(table, "id", "line.id") {
                Some((value, item)) if (1..=i64::from(MAX_LINE_ID)).contains(&value) => {
                    line.id = value as u16;
                    let here = self.line(item);
                    match ids.iter().find(|(id, _)| *id == line.id) {
                        Some((_, first)) => self.problems.push(message(
                            "RMD0016",
                            &[&value.to_string(), &here.to_string(), &first.to_string()],
                        )),
                        None => ids.push((line.id, here)),
                    }
                }
                Some((value, item)) => {
                    self.not_allowed("line.id", item, &value.to_string(), "1 to 999");
                }
                None if table.get("id").is_none() => self
                    .problems
                    .push(message("RMD0015", &["line.id", &table_line.to_string()])),
                None => {}
            }

            if let Some((value, _)) = self.string(table, "name", "line.name") {
                line.name = Some(value.to_string());
            }

            match self.table(table, "serial", "line.serial") {
                Some(serial) => {
                    if let Some(serial) = self.serial(serial, table_line, &mut ports) {
                        line.serial = serial;
                    }
                }
                None if table.get("serial").is_none() => self.problems.push(message(
                    "RMD0015",
                    &["line.serial", &table_line.to_string()],
                )),
                None => {}
            }

            if let Some(value) = self.boolean(table, "enabled", "line.enabled") {
                line.enabled = value;
            }

            if let Some((value, item)) =
                self.integer(table, "auto_answer_rings", "line.auto_answer_rings")
            {
                if (0..=255).contains(&value) {
                    line.auto_answer_rings = value as u8;
                } else {
                    self.not_allowed(
                        "line.auto_answer_rings",
                        item,
                        &value.to_string(),
                        "0 to 255",
                    );
                }
            }

            if let Some((value, item)) =
                self.string(table, "unknown_command", "line.unknown_command")
            {
                match UnknownCommand::parse(value) {
                    Some(setting) => line.unknown_command = setting,
                    None => self.not_allowed("line.unknown_command", item, value, "ok, error"),
                }
            }

            if self.problems.len() == before {
                lines.push(line);
            }
        }
    }

    fn serial(
        &mut self,
        table: &dyn TableLike,
        table_line: usize,
        ports: &mut Vec<(String, usize)>,
    ) -> Option<SerialConfig> {
        let before = self.problems.len();
        let mut serial = SerialConfig {
            backend: Backend::default(),
            port: String::new(),
        };

        match self.string(table, "backend", "line.serial.backend") {
            Some((value, item)) => match Backend::parse(value) {
                Some(backend) => serial.backend = backend,
                None => {
                    let line = self.line(item).to_string();
                    self.problems.push(message("RMD0018", &[value, &line]));
                }
            },
            None if table.get("backend").is_none() => self.problems.push(message(
                "RMD0015",
                &["line.serial.backend", &table_line.to_string()],
            )),
            None => {}
        }

        match self.string(table, "port", "line.serial.port") {
            Some((value, item)) => {
                let valid = !value.is_empty()
                    && value.len() <= MAX_NAME_LEN
                    && value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
                if valid {
                    serial.port = value.to_string();
                    let here = self.line(item);
                    // Windows port names do not distinguish case.
                    let folded = value.to_ascii_lowercase();
                    match ports.iter().find(|(port, _)| *port == folded) {
                        Some((_, first)) => self.problems.push(message(
                            "RMD0017",
                            &[value, &here.to_string(), &first.to_string()],
                        )),
                        None => ports.push((folded, here)),
                    }
                } else {
                    self.not_allowed(
                        "line.serial.port",
                        item,
                        value,
                        "1 to 64 letters, digits, _, - or .",
                    );
                }
            }
            None if table.get("port").is_none() => self.problems.push(message(
                "RMD0015",
                &["line.serial.port", &table_line.to_string()],
            )),
            None => {}
        }

        (self.problems.len() == before).then_some(serial)
    }
}
