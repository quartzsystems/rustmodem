// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The lexical pieces of RFC 3261 section 25.1 that the parsers share.

use std::fmt;

/// Whether `text` is a `token`: one or more of the characters RFC 3261
/// section 25.1 allows in one.
pub(crate) fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-.!%*_+`'~".contains(&b))
}

/// Splits at every `separator` that is not inside a quoted string or
/// between angle brackets.
pub(crate) fn split_top(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut angle = false;
    for (at, c) in text.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        } else if c == '<' {
            angle = true;
        } else if c == '>' {
            angle = false;
        } else if c == separator && !angle {
            parts.push(&text[start..at]);
            start = at + c.len_utf8();
        }
    }
    parts.push(&text[start..]);
    parts
}

/// The index of the first `wanted` that is not inside a quoted string.
pub(crate) fn find_unquoted(text: &str, wanted: char) -> Option<usize> {
    let mut quoted = false;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c == wanted {
            return Some(at);
        } else if c == '"' {
            quoted = true;
        }
    }
    None
}

/// The text of a `quoted-string`, without its quotes and escapes. `None`
/// when `text` is not one.
pub(crate) fn unquote(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// `text` as a `quoted-string`.
pub(crate) fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A value that is a quoted string, or a token taken as it is.
pub(crate) fn unquote_or_token(text: &str) -> Option<String> {
    if text.starts_with('"') {
        unquote(text)
    } else {
        Some(text.to_owned())
    }
}

/// The parameters after a URI or a header value: `;name=value;name`.
///
/// Names are compared without regard to case. Values are kept as written,
/// quotes included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Params(pub Vec<(String, Option<String>)>);

impl Params {
    pub fn new() -> Params {
        Params(Vec::new())
    }

    /// Parses `;a=b;c`, with or without the first semicolon. White space
    /// around names and values is dropped (RFC 3261 section 7.3.1 allows
    /// it between the tokens of a header).
    pub(crate) fn parse(text: &str) -> Option<Params> {
        let mut params = Vec::new();
        for part in split_top(text, ';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (name, value) = match part.split_once('=') {
                Some((name, value)) => (name.trim(), Some(value.trim().to_owned())),
                None => (part, None),
            };
            if !is_token(name) {
                return None;
            }
            params.push((name.to_owned(), value));
        }
        Some(Params(params))
    }

    /// Whether the parameter is there, with a value or without.
    pub fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// The value of the parameter, when it is there and has one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| value.as_deref())
    }

    /// Sets the parameter, replacing it where it stands or adding it.
    pub fn set(&mut self, name: &str, value: Option<&str>) {
        let value = value.map(str::to_owned);
        match self
            .0
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(slot) => slot.1 = value,
            None => self.0.push((name.to_owned(), value)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Params {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, value) in &self.0 {
            match value {
                Some(value) => write!(f, ";{name}={value}")?,
                None => write!(f, ";{name}")?,
            }
        }
        Ok(())
    }
}
