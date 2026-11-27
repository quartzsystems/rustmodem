// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Write-back: changes made to the document in place, so comments and
//! ordering outside the change survive.

use toml_edit::{
    Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value, value,
};

use crate::schema::{
    Cidr, Config, EngineKind, HuntGroup, HuntGroupEdit, LineConfig, LineEdit, LineModem,
    MediaConfig, ModemConfig, RegisterConfig, Route, RouteEdit, SettingsEdit, SipConfig,
    UnknownCommand,
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

    if config.modem != ModemConfig::default() {
        let modem = &config.modem;
        let mut table = Table::new();
        table["modulations"] = value(strings(
            modem.modulations.iter().map(|m| m.as_str().to_string()),
        ));
        table["v8"] = value(modem.v8);
        table["tx_level_dbm0"] = value(i64::from(modem.tx_level_dbm0));
        table["answer_delay_ms"] = value(i64::from(modem.answer_delay_ms));
        table["fallback_ms"] = value(i64::from(modem.fallback_ms));
        table["error_control"] = value(modem.error_control.as_str());
        table["compression"] = value(modem.compression);
        table["guard_tone"] = value(modem.guard_tone.as_str());
        document["modem"] = Item::Table(table);
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
/// The `modem` table of a line: the keys the line has its own value
/// for.
pub(crate) fn line_modem_table(modem: &LineModem) -> InlineTable {
    let mut table = InlineTable::new();
    if let Some(modulations) = &modem.modulations {
        let names = strings(modulations.iter().map(|m| m.as_str().to_string()));
        table.insert("modulations", Value::Array(names));
    }
    if let Some(v8) = modem.v8 {
        table.insert("v8", v8.into());
    }
    if let Some(level) = modem.tx_level_dbm0 {
        table.insert("tx_level_dbm0", i64::from(level).into());
    }
    if let Some(policy) = modem.error_control {
        table.insert("error_control", policy.as_str().into());
    }
    if let Some(compression) = modem.compression {
        table.insert("compression", compression.into());
    }
    if let Some(tone) = modem.guard_tone {
        table.insert("guard_tone", tone.as_str().into());
    }
    table
}

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
    if !line.modem.is_empty() {
        table["modem"] = value(line_modem_table(&line.modem));
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
    if let Some(engine) = edit.engine {
        set(table, "engine", engine.as_str().into());
    }
    match &edit.register {
        // The table is written again whole: it is one value, and a key
        // that went back to its default is left out of it.
        Some(Some(register)) => {
            table.remove("register");
            table["register"] = value(register_table(register));
        }
        Some(None) => {
            table.remove("register");
        }
        None => {}
    }
    if let Some(modem) = &edit.modem {
        // Written again whole, as the registration is: a key the line
        // no longer has its own value for is left out.
        table.remove("modem");
        let mut own = InlineTable::new();
        if let Some(modulations) = &modem.modulations {
            own.insert("modulations", strings(modulations.iter().cloned()).into());
        }
        if let Some(v8) = modem.v8 {
            own.insert("v8", v8.into());
        }
        if let Some(level) = modem.tx_level_dbm0 {
            own.insert("tx_level_dbm0", level.into());
        }
        if let Some(policy) = &modem.error_control {
            own.insert("error_control", policy.as_str().into());
        }
        if let Some(compression) = modem.compression {
            own.insert("compression", compression.into());
        }
        if let Some(tone) = &modem.guard_tone {
            own.insert("guard_tone", tone.as_str().into());
        }
        if !own.is_empty() {
            table["modem"] = value(own);
        }
    }
}

/// Changes the keys `edit` names in the `index`th `[[hunt_group]]` table,
/// which is called `name`. When the name changes, the targets of the
/// routes to the group change with it.
pub(crate) fn edit_hunt_group(
    document: &mut DocumentMut,
    index: usize,
    name: &str,
    edit: &HuntGroupEdit,
) {
    if let Some(table) = tables_mut(document, "hunt_group").get_mut(index) {
        if let Some(new) = &edit.name {
            set(table, "name", new.as_str().into());
        }
        if let Some(pilot) = &edit.pilot {
            set(table, "pilot", pilot.as_str().into());
        }
        if let Some(lines) = &edit.lines {
            let mut array = Array::new();
            for line in lines {
                array.push(i64::from(*line));
            }
            set(table, "lines", array.into());
        }
        if let Some(strategy) = &edit.strategy {
            set(table, "strategy", strategy.as_str().into());
        }
    }
    if let Some(new) = &edit.name {
        let old = format!("group:{name}");
        for route in tables_mut(document, "route").iter_mut() {
            if route.get("target").and_then(Item::as_str) == Some(old.as_str()) {
                set(route, "target", format!("group:{new}").into());
            }
        }
    }
}

/// Changes the keys `edit` names in the `index`th `[[route]]` table.
pub(crate) fn edit_route(document: &mut DocumentMut, index: usize, edit: &RouteEdit) {
    let Some(table) = tables_mut(document, "route").get_mut(index) else {
        return;
    };
    if let Some(called) = &edit.called {
        set(table, "called", called.as_str().into());
    }
    if let Some(target) = &edit.target {
        set(table, "target", target.as_str().into());
    }
}

/// Removes the `index`th table of the array of tables called `name`.
pub(crate) fn remove_table(document: &mut DocumentMut, name: &str, index: usize) {
    let tables = tables_mut(document, name);
    if index < tables.len() {
        tables.remove(index);
    }
}

/// Moves the table at `from` in the array of tables called `name` so that
/// it is at `to`.
///
/// Where a table is written in the file goes by its position, not by its
/// place in the array, so the tables trade positions: the array, in its
/// new order, takes the places in the file the array had. A table that
/// was just added has no position yet and is written after the one before
/// it, which is what the last table of the array wants.
pub(crate) fn move_table(document: &mut DocumentMut, name: &str, from: usize, to: usize) {
    let tables = tables_mut(document, name);
    if from >= tables.len() || to >= tables.len() {
        return;
    }
    let mut positions: Vec<isize> = tables.iter().filter_map(Table::position).collect();
    positions.sort_unstable();
    let moved = tables.remove(from);
    tables.insert(to, moved);
    let mut positions = positions.into_iter();
    for table in tables.iter_mut() {
        table.set_position(positions.next());
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
    if !edit.sip.is_empty() {
        let sip = section(document, "sip");
        let mut put = |key: &str, new: Option<Value>| {
            if let Some(new) = new {
                set_item(sip.entry(key).or_insert(Item::None), new);
            }
        };
        put("enabled", edit.sip.enabled.map(Value::from));
        put("listen", edit.sip.listen.as_deref().map(Value::from));
        put("address", edit.sip.address.as_deref().map(Value::from));
        put(
            "user_agent",
            edit.sip.user_agent.as_deref().map(Value::from),
        );
        put(
            "trusted_peers",
            edit.sip
                .trusted_peers
                .as_ref()
                .map(|peers| strings(peers.iter().cloned()).into()),
        );
        put(
            "outbound_peer",
            edit.sip.outbound_peer.as_deref().map(Value::from),
        );
        put(
            "ring_timeout_s",
            edit.sip.ring_timeout_s.map(|s| i64::from(s).into()),
        );
    }
    if !edit.media.is_empty() {
        let media = section(document, "media");
        let mut put = |key: &str, new: Option<Value>| {
            if let Some(new) = new {
                set_item(media.entry(key).or_insert(Item::None), new);
            }
        };
        put(
            "codecs",
            edit.media
                .codecs
                .as_ref()
                .map(|codecs| strings(codecs.iter().cloned()).into()),
        );
        put(
            "ptime_ms",
            edit.media.ptime_ms.map(|ms| i64::from(ms).into()),
        );
        put(
            "rtp_ports",
            edit.media.rtp_ports.as_deref().map(Value::from),
        );
        put(
            "rtp_timeout_s",
            edit.media.rtp_timeout_s.map(|s| i64::from(s).into()),
        );
        put(
            "capture_dir",
            edit.media.capture_dir.as_deref().map(Value::from),
        );
    }
    if !edit.modem.is_empty() {
        let modem = section(document, "modem");
        let mut put = |key: &str, new: Option<Value>| {
            if let Some(new) = new {
                set_item(modem.entry(key).or_insert(Item::None), new);
            }
        };
        put(
            "modulations",
            edit.modem
                .modulations
                .as_ref()
                .map(|names| strings(names.iter().cloned()).into()),
        );
        put("v8", edit.modem.v8.map(Value::from));
        put("tx_level_dbm0", edit.modem.tx_level_dbm0.map(Value::from));
        put(
            "answer_delay_ms",
            edit.modem.answer_delay_ms.map(|ms| i64::from(ms).into()),
        );
        put(
            "fallback_ms",
            edit.modem.fallback_ms.map(|ms| i64::from(ms).into()),
        );
        put(
            "error_control",
            edit.modem
                .error_control
                .as_ref()
                .map(|policy| policy.as_str().into()),
        );
        put("compression", edit.modem.compression.map(Value::from));
        put(
            "guard_tone",
            edit.modem
                .guard_tone
                .as_ref()
                .map(|tone| tone.as_str().into()),
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
