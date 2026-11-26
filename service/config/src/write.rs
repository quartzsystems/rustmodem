// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Write-back: changes made to the document in place, so comments and
//! ordering outside the change survive.

use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::schema::{Config, LineConfig, LineEdit, UnknownCommand};

/// Renders a configuration as a new document with no comments.
pub(crate) fn render(config: &Config) -> String {
    let mut document = DocumentMut::new();

    let mut service = Table::new();
    service["log_level"] = value(config.service.log_level.as_str());
    service["log_dir"] = value(config.service.log_dir.as_str());
    document["service"] = Item::Table(service);

    let mut control = Table::new();
    control["pipe"] = value(config.control.pipe.as_str());
    document["control"] = Item::Table(control);

    for line in &config.lines {
        add_line(&mut document, line);
    }
    document.to_string()
}

/// Appends a `[[line]]` table. Keys at their defaults are left out.
pub(crate) fn add_line(document: &mut DocumentMut, line: &LineConfig) {
    let mut table = Table::new();
    table["id"] = value(i64::from(line.id));
    if let Some(name) = &line.name {
        table["name"] = value(name.as_str());
    }
    let mut serial = InlineTable::new();
    serial.insert("backend", line.serial.backend.as_str().into());
    serial.insert("port", line.serial.port.as_str().into());
    table["serial"] = value(serial);
    if !line.enabled {
        table["enabled"] = value(false);
    }
    if line.auto_answer_rings != 0 {
        table["auto_answer_rings"] = value(i64::from(line.auto_answer_rings));
    }
    if line.unknown_command != UnknownCommand::default() {
        table["unknown_command"] = value(line.unknown_command.as_str());
    }
    lines_mut(document).push(table);
}

/// Changes the keys `edit` names in the `index`th `[[line]]` table.
pub(crate) fn edit_line(document: &mut DocumentMut, index: usize, edit: &LineEdit) {
    let Some(table) = lines_mut(document).get_mut(index) else {
        return;
    };
    match &edit.name {
        Some(Some(name)) => set(table, "name", name.as_str().into()),
        Some(None) => {
            table.remove("name");
        }
        None => {}
    }
    if edit.backend.is_some() || edit.port.is_some() {
        // The serial table may be written inline or as [line.serial]; both
        // are edited through the same view.
        if let Some(serial) = table.get_mut("serial").and_then(Item::as_table_like_mut) {
            if let Some(backend) = edit.backend {
                set_item(
                    serial.entry("backend").or_insert(Item::None),
                    backend.as_str().into(),
                );
            }
            if let Some(port) = &edit.port {
                set_item(
                    serial.entry("port").or_insert(Item::None),
                    port.as_str().into(),
                );
            }
        }
    }
    if let Some(enabled) = edit.enabled {
        set(table, "enabled", enabled.into());
    }
    if let Some(rings) = edit.auto_answer_rings {
        set(table, "auto_answer_rings", i64::from(rings).into());
    }
    if let Some(setting) = edit.unknown_command {
        set(table, "unknown_command", setting.as_str().into());
    }
}

/// Removes the `index`th `[[line]]` table.
pub(crate) fn remove_line(document: &mut DocumentMut, index: usize) {
    let lines = lines_mut(document);
    if index < lines.len() {
        lines.remove(index);
    }
}

/// The `line` array of tables, created if the document has none.
fn lines_mut(document: &mut DocumentMut) -> &mut ArrayOfTables {
    let item = document
        .entry("line")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    if !item.is_array_of_tables() {
        // A document that passed validation has an array of tables here or
        // nothing; this only replaces what validation would have refused.
        *item = Item::ArrayOfTables(ArrayOfTables::new());
    }
    match item {
        Item::ArrayOfTables(lines) => lines,
        _ => unreachable!("line was just made an array of tables"),
    }
}

/// Sets `key` in `table`, keeping what surrounds an existing value.
fn set(table: &mut Table, key: &str, new: Value) {
    set_item(table.entry(key).or_insert(Item::None), new);
}

/// Replaces the value of an item. If the item already holds a value, the
/// new one takes over its decoration: the spacing around it and any comment
/// after it on the same line.
fn set_item(item: &mut Item, mut new: Value) {
    if let Item::Value(old) = item {
        *new.decor_mut() = old.decor().clone();
    }
    *item = Item::Value(new);
}
