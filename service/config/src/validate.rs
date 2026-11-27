// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Parsing and validation. Every problem is collected and reported at once,
//! each with the offending key and its line number.

use msg::{Message, message};
use toml_edit::{Document, DocumentMut, Item, TableLike};

use crate::schema::{
    Backend, Cidr, CodecName, Config, ControlConfig, EngineKind, HuntGroup, LineConfig, LineModem,
    LogLevel, MAX_LINE_ID, MediaConfig, ModemConfig, ModulationName, PortRange, RegisterConfig,
    Route, SerialConfig, ServiceConfig, SipConfig, Strategy, Target, UnknownCommand,
    pattern_covers,
};

/// The longest pipe or port name accepted.
const MAX_NAME_LEN: usize = 64;

/// The longest hunt group name, pilot number or route pattern accepted.
const MAX_NUMBER_LEN: usize = 32;

/// A configuration that is valid: the document, what it says, and the
/// warnings about it.
pub(crate) struct Parsed {
    pub(crate) document: DocumentMut,
    pub(crate) config: Config,
    pub(crate) warnings: Vec<Message>,
}

/// Parses and validates configuration text.
pub(crate) fn parse(text: &str) -> Result<Parsed, Vec<Message>> {
    let document = Document::parse(text).map_err(|e| {
        let line = e.span().map(|span| line_of(text, span.start)).unwrap_or(1);
        vec![message("RMD0012", &[&line.to_string(), e.message()])]
    })?;
    let mut reader = Reader {
        text,
        problems: Vec::new(),
        warnings: Vec::new(),
    };
    let config = reader.config(document.as_table());
    if reader.problems.is_empty() {
        Ok(Parsed {
            document: document.into_mut(),
            config,
            warnings: reader.warnings,
        })
    } else {
        Err(reader.problems)
    }
}

/// Whether `text` is a host name or an IPv4 address.
pub(crate) fn is_host(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 253
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

/// Whether `text` is `host` or `host:port`.
pub(crate) fn is_host_port(text: &str) -> bool {
    match text.rsplit_once(':') {
        Some((host, port)) => {
            is_host(host)
                && !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit())
                && port.parse::<u16>().is_ok_and(|port| port != 0)
        }
        None => is_host(text),
    }
}

/// Whether `text` can be the user part of a SIP URI as it stands: the
/// unreserved characters of RFC 3261 section 25.1 and `+`.
fn is_sip_user(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_NAME_LEN
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.!~*'()+".contains(&b))
}

/// Whether `text` is a number someone can be called on: digits, `#` and
/// `+`.
fn is_number(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_NUMBER_LEN
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'#' | b'+'))
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
    warnings: Vec<Message>,
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
        if let Some(table) = self.table(root, "sip", "sip") {
            config.sip = self.sip(table);
        }
        if let Some(table) = self.table(root, "media", "media") {
            config.media = self.media(table);
        }
        if let Some(table) = self.table(root, "modem", "modem") {
            config.modem = self.modem(table);
        }
        if let Some(item) = root.get("line") {
            match item.as_array_of_tables() {
                Some(lines) => self.lines(lines, &mut config.lines),
                None => self.wrong_type("line", item, "array of tables ([[line]])"),
            }
        }
        if let Some(item) = root.get("hunt_group") {
            match item.as_array_of_tables() {
                Some(groups) => self.hunt_groups(groups, &config.lines, &mut config.hunt_groups),
                None => self.wrong_type("hunt_group", item, "array of tables ([[hunt_group]])"),
            }
        }
        if let Some(item) = root.get("route") {
            match item.as_array_of_tables() {
                Some(routes) => {
                    let (lines, groups) = (&config.lines, &config.hunt_groups);
                    self.routes(routes, lines, groups, &mut config.routes);
                }
                None => self.wrong_type("route", item, "array of tables ([[route]])"),
            }
        }
        config
    }

    /// The strings of an array-valued key, each with its line number.
    /// `None`, with a problem recorded, if the key is there and is not an
    /// array of strings.
    fn strings(
        &mut self,
        table: &dyn TableLike,
        key: &str,
        path: &str,
    ) -> Option<Vec<(String, usize)>> {
        let item = table.get(key)?;
        let values = item.as_array().and_then(|array| {
            array
                .iter()
                .map(|value| {
                    let line = value.span().map_or(0, |s| line_of(self.text, s.start));
                    value.as_str().map(|text| (text.to_string(), line))
                })
                .collect::<Option<Vec<_>>>()
        });
        if values.is_none() {
            self.wrong_type(path, item, "array of strings");
        }
        values
    }

    /// The integers of an array-valued key, each with its line number.
    fn integers(
        &mut self,
        table: &dyn TableLike,
        key: &str,
        path: &str,
    ) -> Option<Vec<(i64, usize)>> {
        let item = table.get(key)?;
        let values = item.as_array().and_then(|array| {
            array
                .iter()
                .map(|value| {
                    let line = value.span().map_or(0, |s| line_of(self.text, s.start));
                    value.as_integer().map(|number| (number, line))
                })
                .collect::<Option<Vec<_>>>()
        });
        if values.is_none() {
            self.wrong_type(path, item, "array of integers");
        }
        values
    }

    fn not_allowed_at(&mut self, key: &str, line: usize, value: &str, allowed: &str) {
        self.problems.push(message(
            "RMD0014",
            &[key, &line.to_string(), value, allowed],
        ));
    }

    fn sip(&mut self, table: &dyn TableLike) -> SipConfig {
        let mut sip = SipConfig::default();
        if let Some(value) = self.boolean(table, "enabled", "sip.enabled") {
            sip.enabled = value;
        }
        if let Some((value, item)) = self.string(table, "listen", "sip.listen") {
            match value.parse::<std::net::SocketAddrV4>() {
                Ok(address) if address.port() != 0 => sip.listen = address,
                _ => self.not_allowed(
                    "sip.listen",
                    item,
                    value,
                    "an IPv4 address and a port, such as 0.0.0.0:5060",
                ),
            }
        }
        if let Some((value, item)) = self.string(table, "address", "sip.address") {
            if value.is_empty() || is_host(value) {
                sip.address = value.to_string();
            } else {
                self.not_allowed(
                    "sip.address",
                    item,
                    value,
                    "an IPv4 address or a host name, or nothing",
                );
            }
        }
        if let Some((value, item)) = self.string(table, "user_agent", "sip.user_agent") {
            if value.len() <= 128 && value.chars().all(|c| (' '..='~').contains(&c)) {
                sip.user_agent = value.to_string();
            } else {
                self.not_allowed(
                    "sip.user_agent",
                    item,
                    value,
                    "up to 128 printable ASCII characters",
                );
            }
        }
        if let Some(values) = self.strings(table, "trusted_peers", "sip.trusted_peers") {
            for (value, line) in values {
                match Cidr::parse(&value) {
                    Some(peer) => sip.trusted_peers.push(peer),
                    None => self.not_allowed_at(
                        "sip.trusted_peers",
                        line,
                        &value,
                        "an IPv4 address or a CIDR block, such as 10.10.20.11 or 10.10.20.0/24",
                    ),
                }
            }
        }
        if let Some((value, item)) = self.string(table, "outbound_peer", "sip.outbound_peer") {
            if value.is_empty() || is_host_port(value) {
                sip.outbound_peer = value.to_string();
            } else {
                self.not_allowed(
                    "sip.outbound_peer",
                    item,
                    value,
                    "a host or host:port, or nothing",
                );
            }
        }
        if let Some((value, item)) = self.integer(table, "ring_timeout_s", "sip.ring_timeout_s") {
            if (5..=3600).contains(&value) {
                sip.ring_timeout_s = value as u32;
            } else {
                self.not_allowed("sip.ring_timeout_s", item, &value.to_string(), "5 to 3600");
            }
        }
        sip
    }

    /// The modulations of a `modulations` key: at least one, each once.
    fn modulations(&mut self, table: &dyn TableLike, path: &str) -> Option<Vec<ModulationName>> {
        let values = self.strings(table, "modulations", path)?;
        let mut modulations: Vec<ModulationName> = Vec::new();
        let mut valid = true;
        for (value, line) in &values {
            match ModulationName::parse(value) {
                Some(modulation) if !modulations.contains(&modulation) => {
                    modulations.push(modulation);
                }
                _ => {
                    valid = false;
                    self.not_allowed_at(path, *line, value, "v21 and bell103, each once");
                }
            }
        }
        if modulations.is_empty() && valid {
            let line = table.get("modulations").map_or(0, |item| self.line(item));
            self.not_allowed_at(path, line, "[]", "at least one of v21 and bell103");
            return None;
        }
        valid.then_some(modulations)
    }

    /// The transmit level of a `tx_level_dbm0` key.
    fn tx_level(&mut self, table: &dyn TableLike, path: &str) -> Option<i8> {
        let (value, item) = self.integer(table, "tx_level_dbm0", path)?;
        if ModemConfig::TX_LEVELS.contains(&value) {
            Some(value as i8)
        } else {
            self.not_allowed(path, item, &value.to_string(), "-20 to -9");
            None
        }
    }

    fn modem(&mut self, table: &dyn TableLike) -> ModemConfig {
        let mut modem = ModemConfig::default();
        if let Some(modulations) = self.modulations(table, "modem.modulations") {
            modem.modulations = modulations;
        }
        if let Some(value) = self.boolean(table, "v8", "modem.v8") {
            modem.v8 = value;
        }
        if let Some(level) = self.tx_level(table, "modem.tx_level_dbm0") {
            modem.tx_level_dbm0 = level;
        }
        if let Some((value, item)) = self.integer(table, "answer_delay_ms", "modem.answer_delay_ms")
        {
            if ModemConfig::ANSWER_DELAYS.contains(&value) {
                modem.answer_delay_ms = value as u32;
            } else {
                self.not_allowed(
                    "modem.answer_delay_ms",
                    item,
                    &value.to_string(),
                    "0 to 10000",
                );
            }
        }
        if let Some((value, item)) = self.integer(table, "fallback_ms", "modem.fallback_ms") {
            if ModemConfig::FALLBACK_TIMES.contains(&value) {
                modem.fallback_ms = value as u32;
            } else {
                self.not_allowed(
                    "modem.fallback_ms",
                    item,
                    &value.to_string(),
                    "500 to 20000",
                );
            }
        }
        modem
    }

    /// The `modem` table of a line.
    fn line_modem(&mut self, table: &dyn TableLike) -> LineModem {
        LineModem {
            modulations: self.modulations(table, "line.modem.modulations"),
            v8: self.boolean(table, "v8", "line.modem.v8"),
            tx_level_dbm0: self.tx_level(table, "line.modem.tx_level_dbm0"),
        }
    }

    fn media(&mut self, table: &dyn TableLike) -> MediaConfig {
        let mut media = MediaConfig::default();
        if let Some(values) = self.strings(table, "codecs", "media.codecs") {
            let mut codecs: Vec<CodecName> = Vec::new();
            let mut valid = true;
            for (value, line) in &values {
                match CodecName::parse(value) {
                    Some(codec) if !codecs.contains(&codec) => codecs.push(codec),
                    _ => {
                        valid = false;
                        self.not_allowed_at(
                            "media.codecs",
                            *line,
                            value,
                            "pcmu and pcma, each once",
                        );
                    }
                }
            }
            if codecs.is_empty() && valid {
                let line = table.get("codecs").map_or(0, |item| self.line(item));
                self.not_allowed_at("media.codecs", line, "[]", "at least one of pcmu and pcma");
            } else if valid {
                media.codecs = codecs;
            }
        }
        if let Some((value, item)) = self.integer(table, "ptime_ms", "media.ptime_ms") {
            match u32::try_from(value) {
                Ok(ptime) if MediaConfig::PTIMES.contains(&ptime) => media.ptime_ms = ptime,
                _ => self.not_allowed("media.ptime_ms", item, &value.to_string(), "10, 20, 30"),
            }
        }
        if let Some((value, item)) = self.string(table, "rtp_ports", "media.rtp_ports") {
            match PortRange::parse(value) {
                Some(range) => media.rtp_ports = range,
                None => self.not_allowed(
                    "media.rtp_ports",
                    item,
                    value,
                    "a range first-last from 1024 to 65535 that holds an even port, such as 20000-20999",
                ),
            }
        }
        if let Some((value, item)) = self.integer(table, "rtp_timeout_s", "media.rtp_timeout_s") {
            if (1..=3600).contains(&value) {
                media.rtp_timeout_s = value as u32;
            } else {
                self.not_allowed("media.rtp_timeout_s", item, &value.to_string(), "1 to 3600");
            }
        }
        if let Some((value, _)) = self.string(table, "capture_dir", "media.capture_dir") {
            media.capture_dir = value.to_string();
        }
        media
    }

    fn register(&mut self, table: &dyn TableLike, table_line: usize) -> Option<RegisterConfig> {
        let before = self.problems.len();
        let mut register = RegisterConfig {
            server: String::new(),
            user: String::new(),
            password: String::new(),
            auth_user: String::new(),
            expires_s: RegisterConfig::DEFAULT_EXPIRES_S,
        };
        match self.string(table, "server", "line.register.server") {
            Some((value, _)) if is_host_port(value) => register.server = value.to_string(),
            Some((value, item)) => {
                self.not_allowed("line.register.server", item, value, "a host or host:port");
            }
            None if table.get("server").is_none() => self.problems.push(message(
                "RMD0015",
                &["line.register.server", &table_line.to_string()],
            )),
            None => {}
        }
        match self.string(table, "user", "line.register.user") {
            Some((value, _)) if is_sip_user(value) => register.user = value.to_string(),
            Some((value, item)) => self.not_allowed(
                "line.register.user",
                item,
                value,
                "1 to 64 letters, digits or any of -_.!~*'()+",
            ),
            None if table.get("user").is_none() => self.problems.push(message(
                "RMD0015",
                &["line.register.user", &table_line.to_string()],
            )),
            None => {}
        }
        // The password is never put in a message, not even a wrong one.
        if let Some((value, _)) = self.string(table, "password", "line.register.password") {
            register.password = value.to_string();
        }
        if let Some((value, item)) = self.string(table, "auth_user", "line.register.auth_user") {
            if value.len() <= MAX_NAME_LEN && value.chars().all(|c| ('!'..='~').contains(&c)) {
                register.auth_user = value.to_string();
            } else {
                self.not_allowed(
                    "line.register.auth_user",
                    item,
                    value,
                    "up to 64 printable ASCII characters with no space",
                );
            }
        }
        if let Some((value, item)) = self.integer(table, "expires_s", "line.register.expires_s") {
            if (30..=86_400).contains(&value) {
                register.expires_s = value as u32;
            } else {
                self.not_allowed(
                    "line.register.expires_s",
                    item,
                    &value.to_string(),
                    "30 to 86400",
                );
            }
        }
        (self.problems.len() == before).then_some(register)
    }

    fn hunt_groups(
        &mut self,
        tables: &toml_edit::ArrayOfTables,
        lines: &[LineConfig],
        groups: &mut Vec<HuntGroup>,
    ) {
        let mut names: Vec<(String, usize)> = Vec::new();
        let mut pilots: Vec<(String, usize)> = Vec::new();

        for table in tables.iter() {
            let table_line = table
                .span()
                .map(|s| line_of(self.text, s.start))
                .unwrap_or(0);
            let before = self.problems.len();
            let mut group = HuntGroup {
                name: String::new(),
                pilot: String::new(),
                lines: Vec::new(),
                strategy: Strategy::default(),
            };

            match self.string(table, "name", "hunt_group.name") {
                Some((value, item)) => {
                    let valid = !value.is_empty()
                        && value.len() <= MAX_NUMBER_LEN
                        && value
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
                    if valid {
                        group.name = value.to_string();
                        let here = self.line(item);
                        match names.iter().find(|(name, _)| name == value) {
                            Some((_, first)) => self.problems.push(message(
                                "RMD0028",
                                &[value, &here.to_string(), &first.to_string()],
                            )),
                            None => names.push((value.to_string(), here)),
                        }
                    } else {
                        self.not_allowed(
                            "hunt_group.name",
                            item,
                            value,
                            "1 to 32 letters, digits, _, - or .",
                        );
                    }
                }
                None if table.get("name").is_none() => self.problems.push(message(
                    "RMD0015",
                    &["hunt_group.name", &table_line.to_string()],
                )),
                None => {}
            }

            match self.string(table, "pilot", "hunt_group.pilot") {
                Some((value, item)) => {
                    if is_number(value) {
                        group.pilot = value.to_string();
                        let here = self.line(item);
                        match pilots.iter().find(|(pilot, _)| pilot == value) {
                            Some((_, first)) => self.problems.push(message(
                                "RMD0029",
                                &[value, &here.to_string(), &first.to_string()],
                            )),
                            None => pilots.push((value.to_string(), here)),
                        }
                    } else {
                        self.not_allowed("hunt_group.pilot", item, value, "1 to 32 digits, # or +");
                    }
                }
                None if table.get("pilot").is_none() => self.problems.push(message(
                    "RMD0015",
                    &["hunt_group.pilot", &table_line.to_string()],
                )),
                None => {}
            }

            if let Some(values) = self.integers(table, "lines", "hunt_group.lines") {
                for (value, line) in values {
                    let id = u16::try_from(value)
                        .ok()
                        .filter(|id| *id >= 1 && *id <= MAX_LINE_ID);
                    match id {
                        Some(id) if group.lines.contains(&id) => self.not_allowed_at(
                            "hunt_group.lines",
                            line,
                            &value.to_string(),
                            "each line once",
                        ),
                        Some(id) if lines.iter().any(|l| l.id == id) => group.lines.push(id),
                        Some(id) => self.problems.push(message(
                            "RMD0027",
                            &[&group.name, &line.to_string(), &id.to_string()],
                        )),
                        None => self.not_allowed_at(
                            "hunt_group.lines",
                            line,
                            &value.to_string(),
                            "1 to 999",
                        ),
                    }
                }
            }

            if let Some((value, item)) = self.string(table, "strategy", "hunt_group.strategy") {
                match Strategy::parse(value) {
                    Some(strategy) => group.strategy = strategy,
                    None => self.not_allowed(
                        "hunt_group.strategy",
                        item,
                        value,
                        "first-idle, round-robin, least-recent",
                    ),
                }
            }

            if self.problems.len() == before {
                groups.push(group);
            }
        }
    }

    fn routes(
        &mut self,
        tables: &toml_edit::ArrayOfTables,
        lines: &[LineConfig],
        groups: &[HuntGroup],
        routes: &mut Vec<Route>,
    ) {
        // The pattern and the line number of every route before this one.
        let mut earlier: Vec<(String, usize)> = Vec::new();

        for table in tables.iter() {
            let table_line = table
                .span()
                .map(|s| line_of(self.text, s.start))
                .unwrap_or(0);
            let before = self.problems.len();
            let mut route = Route {
                called: String::new(),
                target: Target::Line(0),
            };

            match self.string(table, "called", "route.called") {
                Some((value, item)) => {
                    let digits = value.strip_suffix('*').unwrap_or(value);
                    if (digits.is_empty() && value == "*") || is_number(digits) {
                        route.called = value.to_string();
                        let here = self.line(item);
                        if let Some((first, first_line)) = earlier
                            .iter()
                            .find(|(pattern, _)| pattern_covers(pattern, value))
                        {
                            self.warnings.push(message(
                                "RMD0031",
                                &[value, &here.to_string(), first, &first_line.to_string()],
                            ));
                        }
                        earlier.push((value.to_string(), here));
                    } else {
                        self.not_allowed(
                            "route.called",
                            item,
                            value,
                            "digits, # or +, or a prefix of them ending in *",
                        );
                    }
                }
                None if table.get("called").is_none() => self.problems.push(message(
                    "RMD0015",
                    &["route.called", &table_line.to_string()],
                )),
                None => {}
            }

            match self.string(table, "target", "route.target") {
                Some((value, item)) => match Target::parse(value) {
                    Some(target) => {
                        let exists = match &target {
                            Target::Line(id) => lines.iter().any(|l| l.id == *id),
                            Target::Group(name) => groups.iter().any(|g| g.name == *name),
                        };
                        if exists {
                            route.target = target;
                        } else {
                            let line = self.line(item).to_string();
                            self.problems
                                .push(message("RMD0030", &[&route.called, &line, value]));
                        }
                    }
                    None => {
                        self.not_allowed("route.target", item, value, "line:<id> or group:<name>")
                    }
                },
                None if table.get("target").is_none() => self.problems.push(message(
                    "RMD0015",
                    &["route.target", &table_line.to_string()],
                )),
                None => {}
            }

            if self.problems.len() == before {
                routes.push(route);
            }
        }
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

            if let Some((value, item)) = self.string(table, "engine", "line.engine") {
                match EngineKind::parse(value) {
                    Some(engine) => line.engine = engine,
                    None => self.not_allowed("line.engine", item, value, "modem, null, echo"),
                }
            }

            if let Some(modem) = self.table(table, "modem", "line.modem") {
                line.modem = self.line_modem(modem);
            }

            if let Some(register) = self.table(table, "register", "line.register") {
                line.register = self.register(register, table_line);
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
