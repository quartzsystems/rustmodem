// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Write-back: changes made to the document in place, so comments and
//! ordering outside the change survive.

use toml_edit::{
    Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value, value,
};

use crate::schema::{
    Cidr, Config, EngineKind, HuntGroup, LineConfig, LineEdit, MediaConfig, RegisterConfig, Route,
    SettingsEdit, SipConfig, UnknownCommand,
};

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

    // The sections that came with SIP are written only when they say
    // something: a configuration without them stays without them.
    if config.sip != SipConfig::default() {
        let sip = &config.sip;
        let mut table = Table::new();
        table["enabled"] = value(sip.enabled);
        table["listen"] = value(sip.listen.to_string());
        table["address"] = value(sip.address.as_str());
        table["user_agent"] = value(sip.user_agent.as_str());
        table["trusted_peers"] = value(strings(sip.trusted_peers.iter().map(Cidr::to_string)));
        table["outbound_peer"] = value(sip.outbound_peer.as_str());
        table["ring_timeout_s"] = value(i64::from(sip.ring_timeout_s));
        document["sip"] = Item::Table(table);
    }
    if config.media != MediaConfig::default() {
        let media = &config.media;
        let mut table = Table::new();
        table["codecs"] = value(strings(
            media.codecs.iter().map(|codec| codec.as_str().to_string()),
        ));
        table["ptime_ms"] = value(i64::from(media.ptime_ms));
        table["rtp_ports"] = value(media.rtp_ports.to_string());
        table["rtp_timeout_s"] = value(i64::from(media.rtp_timeout_s));
        table["capture_dir"] = value(media.capture_dir.as_str());
        document["media"] = Item::Table(table);
    }

    for line in &config.lines {
        add_line(&mut document, line);
    }
    for group in &config.hunt_groups {
        add_hunt_group(&mut document, group);
    }
    for route in &config.routes {
        add_route(&mut document, route);
    }
    document.to_string()
}

/// An array of strings.
fn strings(values: impl Iterator<Item = String>) -> Array {
    let mut array = Array::new();
    for value in values {
        array.push(value);
    }
    array
}

/// The `register` table of a line, written inline.
pub(crate) fn register_table(register: &RegisterConfig) -> InlineTable {
    let mut table = InlineTable::new();
    table.insert("server", register.server.as_str().into());
    table.insert("user", register.user.as_str().into());
    table.insert("password", register.password.as_str().into());
    if !register.auth_user.is_empty() {
        table.insert("auth_user", register.auth_user.as_str().into());
    }
    if register.expires_s != RegisterConfig::DEFAULT_EXPIRES_S {
        table.insert("expires_s", i64::from(register.expires_s).into());
    }
    table
}

/// Appends a `[[hunt_group]]` table.
pub(crate) fn add_hunt_group(document: &mut DocumentMut, group: &HuntGroup) {
    let mut table = Table::new();
    table["name"] = value(group.name.as_str());
    table["pilot"] = value(group.pilot.as_str());
    let mut lines = Array::new();
    for line in &group.lines {
        lines.push(i64::from(*line));
    }
    table["lines"] = value(lines);
    table["strategy"] = value(group.strategy.as_str());
    tables_mut(document, "hunt_group").push(table);
}

/// Appends a `[[route]]` table.
pub(crate) fn add_route(document: &mut DocumentMut, route: &Route) {
    let mut table = Table::new();
    table["called"] = value(route.called.as_str());
    table["target"] = value(route.target.to_string());
    tables_mut(document, "route").push(table);
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
    if line.engine != EngineKind::default() {
        table["engine"] = value(line.engine.as_str());
    }
    if let Some(register) = &line.register {
        table["register"] = value(register_table(register));
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

/// Changes the keys `edit` names in `[service]` and `[control]`. A table
/// the document does not have yet is added.
pub(crate) fn edit_settings(document: &mut DocumentMut, edit: &SettingsEdit) {
    if edit.log_level.is_some() || edit.log_dir.is_some() {
        let service = section(document, "service");
        if let Some(level) = edit.log_level {
            set_item(
                service.entry("log_level").or_insert(Item::None),
                level.as_str().into(),
            );
        }
        if let Some(directory) = &edit.log_dir {
            set_item(
                service.entry("log_dir").or_insert(Item::None),
                directory.as_str().into(),
            );
        }
    }
    if let Some(pipe) = &edit.pipe {
        let control = section(document, "control");
        set_item(
            control.entry("pipe").or_insert(Item::None),
            pipe.as_str().into(),
        );
    }
}

/// A top-level table, written either as `[name]` or inline, created as
/// `[name]` if the document has none.
fn section<'d>(document: &'d mut DocumentMut, name: &str) -> &'d mut dyn TableLike {
    let item = document.entry(name).or_insert(Item::Table(Table::new()));
    if item.as_table_like().is_none() {
        // A document that passed validation has a table here or nothing;
        // this only replaces what validation would have refused.
        *item = Item::Table(Table::new());
    }
    match item.as_table_like_mut() {
        Some(table) => table,
        None => unreachable!("the item was just made a table"),
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
    tables_mut(document, "line")
}

/// The array of tables called `name`, created if the document has none.
pub(crate) fn tables_mut<'d>(document: &'d mut DocumentMut, name: &str) -> &'d mut ArrayOfTables {
    let item = document
        .entry(name)
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    if !item.is_array_of_tables() {
        // A document that passed validation has an array of tables here or
        // nothing; this only replaces what validation would have refused.
        *item = Item::ArrayOfTables(ArrayOfTables::new());
    }
    match item {
        Item::ArrayOfTables(tables) => tables,
        _ => unreachable!("the item was just made an array of tables"),
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
