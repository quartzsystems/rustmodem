// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The RMD message catalogue.
//!
//! Every user-visible error in rustmodem is a message from this catalogue.
//! A message id is `RMD` and four digits. Each message has a severity, a
//! one-line text, and second-level text in two parts: cause, then recovery.
//! The texts take substitution variables `&1` to `&9`.
//!
//! The catalogue is `messages.toml`, compiled into the crate and parsed once
//! on first use.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use toml_edit::{DocumentMut, Item};

/// The catalogue source, compiled in.
const CATALOGUE: &str = include_str!("../messages.toml");

/// The id of the message issued when an id is not in the catalogue.
pub const NOT_IN_CATALOGUE: &str = "RMD0999";

/// How serious a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Something happened; nothing is wrong.
    Info,
    /// Something is wrong but the operation went on.
    Warning,
    /// The operation failed.
    Error,
}

impl Severity {
    /// The severity as the catalogue and the control API spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }

    /// Reads a severity as the catalogue and the control API spell it.
    pub fn parse(text: &str) -> Option<Severity> {
        match text {
            "info" => Some(Severity::Info),
            "warning" => Some(Severity::Warning),
            "error" => Some(Severity::Error),
            _ => None,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One catalogue entry: a message before its variables are substituted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The message id, `RMD` and four digits.
    pub id: String,
    /// The severity.
    pub severity: Severity,
    /// How many substitution variables the texts take.
    pub vars: u8,
    /// The first-level text.
    pub text: String,
    /// The second-level text, part one.
    pub cause: String,
    /// The second-level text, part two.
    pub recovery: String,
}

/// An issued message: a catalogue entry with its variables substituted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The message id, `RMD` and four digits.
    pub id: String,
    /// The severity.
    pub severity: Severity,
    /// The first-level text.
    pub text: String,
    /// The second-level text, part one.
    pub cause: String,
    /// The second-level text, part two.
    pub recovery: String,
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.text)
    }
}

impl std::error::Error for Message {}

/// Issues the message `id` with `args` substituted for `&1`, `&2` and so on.
///
/// A variable with no argument is replaced by nothing. An id that is not in
/// the catalogue yields message `RMD0999` naming the id asked for.
pub fn message(id: &str, args: &[&str]) -> Message {
    match catalogue().get(id) {
        Some(entry) => issue(entry, args),
        None => match catalogue().get(NOT_IN_CATALOGUE) {
            Some(entry) => issue(entry, &[id]),
            None => Message {
                id: NOT_IN_CATALOGUE.to_string(),
                severity: Severity::Error,
                text: format!("Message {id} is not in the message catalogue."),
                cause: String::new(),
                recovery: String::new(),
            },
        },
    }
}

/// The whole catalogue, in id order.
pub fn entries() -> impl Iterator<Item = &'static Entry> {
    catalogue().values()
}

/// Whether `id` has the form of a message id: `RMD` and four digits.
pub fn is_well_formed_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 7 && &bytes[..3] == b"RMD" && bytes[3..].iter().all(u8::is_ascii_digit)
}

/// Substitutes `args` into one text. `&n` is a variable only when `n` is a
/// digit from 1 to 9; any other `&` stands for itself.
pub fn substitute(text: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&'
            && let Some(digit) = chars.peek().and_then(|d| d.to_digit(10))
            && digit >= 1
        {
            chars.next();
            if let Some(arg) = args.get(digit as usize - 1) {
                out.push_str(arg);
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The highest variable number a text uses, or 0 if it uses none.
pub fn highest_variable(text: &str) -> u8 {
    let mut highest = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&'
            && let Some(digit) = chars.peek().and_then(|d| d.to_digit(10))
            && digit >= 1
        {
            highest = highest.max(digit as u8);
        }
    }
    highest
}

fn issue(entry: &Entry, args: &[&str]) -> Message {
    Message {
        id: entry.id.clone(),
        severity: entry.severity,
        text: substitute(&entry.text, args),
        cause: substitute(&entry.cause, args),
        recovery: substitute(&entry.recovery, args),
    }
}

fn catalogue() -> &'static BTreeMap<String, Entry> {
    static PARSED: OnceLock<BTreeMap<String, Entry>> = OnceLock::new();
    PARSED.get_or_init(|| match parse_catalogue(CATALOGUE) {
        Ok(map) => map,
        // The catalogue is compiled in and the tests parse it, so a release
        // cannot reach this.
        Err(problem) => panic!("messages.toml is not valid: {problem}"),
    })
}

/// Parses catalogue source. The error is a plain description: it can only be
/// seen by a developer who has broken `messages.toml`.
fn parse_catalogue(source: &str) -> Result<BTreeMap<String, Entry>, String> {
    let document: DocumentMut = source.parse().map_err(|e| format!("{e}"))?;
    let mut map = BTreeMap::new();
    for (id, item) in document.iter() {
        let Item::Table(table) = item else {
            return Err(format!("{id} is not a table"));
        };
        let text_of = |key: &str| -> Result<String, String> {
            table
                .get(key)
                .and_then(Item::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("{id} has no string {key}"))
        };
        let severity = Severity::parse(&text_of("severity")?)
            .ok_or_else(|| format!("{id} has an unknown severity"))?;
        let vars = table
            .get("vars")
            .and_then(Item::as_integer)
            .filter(|n| (0..=9).contains(n))
            .ok_or_else(|| format!("{id} has no vars count from 0 to 9"))?;
        let entry = Entry {
            id: id.to_string(),
            severity,
            vars: vars as u8,
            text: text_of("text")?,
            cause: text_of("cause")?,
            recovery: text_of("recovery")?,
        };
        if map.insert(id.to_string(), entry).is_some() {
            return Err(format!("{id} appears twice"));
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_id_and_text() {
        let m = message("RMD0010", &["C:\\x.toml", "not found"]);
        assert_eq!(
            m.to_string(),
            "RMD0010: Configuration file C:\\x.toml could not be read: not found"
        );
        assert_eq!(m.severity, Severity::Error);
        assert!(!m.cause.is_empty());
        assert!(!m.recovery.is_empty());
    }

    #[test]
    fn variables_substitute_in_all_three_texts() {
        let m = message("RMD0004", &["rustmodem"]);
        assert_eq!(m.text, "Service rustmodem installed.");
        assert_eq!(m.recovery, "Start it with: net start rustmodem");
    }

    #[test]
    fn missing_argument_becomes_nothing() {
        let m = message("RMD1001", &[]);
        assert_eq!(m.text, "Line  does not exist.");
    }

    #[test]
    fn unknown_id_yields_the_not_in_catalogue_message() {
        let m = message("RMD9876", &[]);
        assert_eq!(m.id, NOT_IN_CATALOGUE);
        assert!(m.text.contains("RMD9876"));
    }

    #[test]
    fn substitute_leaves_other_ampersands_alone() {
        assert_eq!(
            substitute("a & b &0 &x &1&2", &["X", "Y"]),
            "a & b &0 &x XY"
        );
        assert_eq!(substitute("ends with &", &[]), "ends with &");
        assert_eq!(substitute("&9", &["a"]), "");
    }

    #[test]
    fn every_id_is_well_formed_and_in_a_range_that_is_in_use() {
        for entry in entries() {
            assert!(is_well_formed_id(&entry.id), "{} is malformed", entry.id);
            let number: u32 = entry.id[3..].parse().unwrap();
            // Service and configuration, lines and AT, the control API,
            // serial, from phase 1 the manager, from phase 2 telephony and
            // media, from phase 3 the modem, and from phase 4 the link.
            assert!(
                (1..=8999).contains(&number),
                "{} is outside the ranges in use",
                entry.id
            );
        }
        assert!(!is_well_formed_id("RMD123"));
        assert!(!is_well_formed_id("RMD12345"));
        assert!(!is_well_formed_id("rmd0001"));
        assert!(!is_well_formed_id("RMD00a1"));
    }

    #[test]
    fn every_id_is_unique() {
        // The TOML parser refuses a repeated table, but the catalogue is the
        // one place ids are defined, so count the headers in the source too.
        let mut seen = std::collections::BTreeSet::new();
        for line in CATALOGUE.lines() {
            let line = line.trim();
            if let Some(id) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                assert!(seen.insert(id.to_string()), "{id} appears twice");
            }
        }
        assert_eq!(seen.len(), entries().count());
        assert!(seen.len() > 40);
    }

    #[test]
    fn no_text_uses_a_variable_above_its_documented_count() {
        for entry in entries() {
            for (part, text) in [
                ("text", &entry.text),
                ("cause", &entry.cause),
                ("recovery", &entry.recovery),
            ] {
                assert!(
                    highest_variable(text) <= entry.vars,
                    "{} {part} uses &{} but documents {} variables",
                    entry.id,
                    highest_variable(text),
                    entry.vars
                );
            }
        }
    }

    #[test]
    fn every_documented_variable_is_used_somewhere() {
        for entry in entries() {
            let used = highest_variable(&entry.text)
                .max(highest_variable(&entry.cause))
                .max(highest_variable(&entry.recovery));
            assert_eq!(
                used, entry.vars,
                "{} documents {} variables but uses {}",
                entry.id, entry.vars, used
            );
        }
    }

    #[test]
    fn every_text_is_present_and_one_line() {
        for entry in entries() {
            for text in [&entry.text, &entry.cause, &entry.recovery] {
                assert!(!text.trim().is_empty(), "{} has an empty text", entry.id);
                assert!(!text.contains('\n'), "{} has a multi-line text", entry.id);
            }
        }
    }

    #[test]
    fn a_broken_catalogue_is_refused() {
        assert!(parse_catalogue("[RMD0001]\nseverity = \"loud\"\n").is_err());
        assert!(parse_catalogue("[RMD0001]\nseverity = \"info\"\nvars = 12\n").is_err());
        assert!(parse_catalogue("RMD0001 = 3\n").is_err());
    }
}
