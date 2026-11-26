// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The builder: a description as text, in the line order of RFC 8866
//! section 5.

use std::fmt::Write;

use crate::{Attribute, Connection, Line, Media, SessionDescription, media_rank, session_rank};

pub(crate) fn build(description: &SessionDescription) -> String {
    let mut out = String::with_capacity(256);
    line(&mut out, 'v', "0");
    let origin = &description.origin;
    line(
        &mut out,
        'o',
        &format!(
            "{} {} {} {} {} {}",
            origin.username,
            origin.session_id,
            origin.session_version,
            origin.net_type,
            origin.addr_type,
            origin.address
        ),
    );
    // RFC 8866 section 5.3: the session name must not be empty.
    if description.session_name.is_empty() {
        line(&mut out, 's', "-");
    } else {
        line(&mut out, 's', &description.session_name);
    }
    kept(&mut out, &description.extra, session_rank, 0);
    connection(&mut out, description.connection.as_ref());
    kept(&mut out, &description.extra, session_rank, 1);
    line(
        &mut out,
        't',
        &format!("{} {}", description.time.0, description.time.1),
    );
    kept(&mut out, &description.extra, session_rank, 2);
    attributes(&mut out, &description.attributes);
    for section in &description.media {
        media(&mut out, section);
    }
    out
}

fn media(out: &mut String, section: &Media) {
    let mut value = String::new();
    let _ = write!(value, "{} {}", section.kind, section.port);
    if let Some(count) = section.port_count {
        let _ = write!(value, "/{count}");
    }
    let _ = write!(value, " {}", section.proto);
    for format in &section.formats {
        let _ = write!(value, " {format}");
    }
    line(out, 'm', &value);
    kept(out, &section.extra, media_rank, 0);
    connection(out, section.connection.as_ref());
    kept(out, &section.extra, media_rank, 1);
    attributes(out, &section.attributes);
}

fn connection(out: &mut String, connection: Option<&Connection>) {
    if let Some(connection) = connection {
        line(
            out,
            'c',
            &format!(
                "{} {} {}",
                connection.net_type, connection.addr_type, connection.address
            ),
        );
    }
}

fn attributes(out: &mut String, attributes: &[Attribute]) {
    for attribute in attributes {
        match &attribute.value {
            Some(value) => line(out, 'a', &format!("{}:{}", attribute.name, value)),
            None => line(out, 'a', &attribute.name),
        }
    }
}

fn kept(out: &mut String, lines: &[Line], rank: fn(char) -> u8, wanted: u8) {
    for kept in lines.iter().filter(|kept| rank(kept.kind) == wanted) {
        line(out, kept.kind, &kept.value);
    }
}

/// One line. A value can never start a line of its own: line breaks and
/// NULs in it are written as spaces.
fn line(out: &mut String, kind: char, value: &str) {
    out.push(kind);
    out.push('=');
    for c in value.chars() {
        out.push(if matches!(c, '\r' | '\n' | '\0') {
            ' '
        } else {
            c
        });
    }
    out.push_str("\r\n");
}
