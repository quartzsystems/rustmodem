// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The data model of the rustmodem manager.
//!
//! Everything the manager's window knows is here, and nothing here knows
//! about windows: there is no Win32 in this crate, so it builds and is
//! tested on any host. The window is given what the service says, hands
//! it to the [`Model`], and draws what the model then holds.
//!
//! - [`ConnectionState`] is whether the manager has the service.
//! - [`LineTable`] is the lines as the grid shows them, with the filter
//!   and the sort applied, and says exactly which row a change touched.
//! - [`StatsView`] is the rows of the two statistics panels.
//! - [`LogBuffer`] is the event log, and [`CallHistory`] the ended calls.
//! - [`Command`] is every menu item and toolbar button, and
//!   [`Model::availability`] says which can be chosen now and, for the
//!   ones that cannot, why not.

#![forbid(unsafe_code)]

mod calls;
mod commands;
pub mod format;
mod lines;
mod log;
mod stats;

use ctlproto::{
    CallRecord, ErrorInfo, Event, LineInfo, LineState, LogEvent, ServiceInfo, ServiceMode, Stats,
};

pub use calls::{CALL_COLUMNS, CALL_LIMIT, CallHistory, reason_text};
pub use commands::{Availability, Command, MenuEntry, lines_menu, menu_bar, toolbar};
pub use format::{EMPTY, TimeZone, Utc, WallTime};
pub use lines::{
    Column, Filter, LineTable, RowChange, SortOrder, StateColour, StateLook, on_a_call,
};
pub use log::{LOG_LIMIT, LogBuffer, LogClass, LogLine};
pub use stats::{CALL_LABELS, CARRIER_LABELS, RATE_ROWS, RateRow, StatsView};

/// The name of the file the service logs to, for the title of the event
/// log.
const LOG_FILE: &str = "rustmodem.log";

/// Whether the manager has the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected. The reason is the `RMD` message that says why; there
    /// is none before the first attempt and after the user disconnects.
    Disconnected(Option<ErrorInfo>),
    /// Opening the pipe, or waiting for the service's hello.
    Connecting,
    /// Connected to this service.
    Connected(ServiceInfo),
}

/// What taking something in changed, for the window to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Changes {
    /// What happened to the rows of the grid.
    pub rows: RowChange,
    /// The event log has new lines.
    pub log: bool,
    /// The call history has a new call.
    pub calls: bool,
}

impl Changes {
    /// Nothing changed.
    pub const NONE: Changes = Changes {
        rows: RowChange::None,
        log: false,
        calls: false,
    };

    fn rows(rows: RowChange) -> Changes {
        Changes {
            rows,
            ..Changes::NONE
        }
    }
}

/// Everything the manager's window knows.
#[derive(Clone, Debug)]
pub struct Model {
    connection: ConnectionState,
    /// The lines.
    pub lines: LineTable,
    stats: Option<Stats>,
    /// The event log.
    pub log: LogBuffer,
    /// The ended calls.
    pub calls: CallHistory,
    /// The lines reported by events since the manager connected and
    /// before the answer to its first `lines.list`: they are newer than
    /// the answer may be, and are applied on top of it.
    early_lines: Option<Vec<LineInfo>>,
}

impl Default for Model {
    fn default() -> Self {
        Model::new()
    }
}

impl Model {
    /// A model that has never been connected.
    pub fn new() -> Model {
        Model {
            connection: ConnectionState::Disconnected(None),
            lines: LineTable::new(),
            stats: None,
            log: LogBuffer::new(),
            calls: CallHistory::new(),
            early_lines: None,
        }
    }

    /// Whether the manager has the service.
    pub fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    /// The service the manager is connected to.
    pub fn service(&self) -> Option<&ServiceInfo> {
        match &self.connection {
            ConnectionState::Connected(info) => Some(info),
            _ => None,
        }
    }

    /// Whether the manager is connected.
    pub fn connected(&self) -> bool {
        self.service().is_some()
    }

    /// The statistics as the service last reported them.
    pub fn stats(&self) -> Option<&Stats> {
        self.stats.as_ref()
    }

    // ----------------------------------------------------------------------
    // What the connection reports
    // ----------------------------------------------------------------------

    /// The manager is trying to connect. What the window shows of the
    /// last connection stays until there is a new one.
    pub fn connecting(&mut self) {
        self.connection = ConnectionState::Connecting;
    }

    /// The manager has connected and has the answer to `service.info`.
    /// The log and the call history start over; the lines stay as they
    /// were until `lines.list` answers.
    pub fn connected_to(&mut self, info: ServiceInfo) {
        self.connection = ConnectionState::Connected(info);
        self.early_lines = Some(Vec::new());
        self.log.clear();
        self.calls.clear();
        self.stats = None;
    }

    /// A later answer to `service.info`: the service is the same one.
    pub fn service_info(&mut self, info: ServiceInfo) {
        if self.connected() {
            self.connection = ConnectionState::Connected(info);
        }
    }

    /// The connection is gone, for this reason, or the user closed it.
    pub fn disconnected(&mut self, reason: Option<ErrorInfo>) {
        self.connection = ConnectionState::Disconnected(reason);
        self.early_lines = None;
        self.stats = None;
    }

    /// The answer to `lines.list`.
    pub fn lines_listed(&mut self, lines: Vec<LineInfo>) -> RowChange {
        let mut change = self.lines.set_all(lines);
        // What events said in the meantime is at least as new as the
        // answer.
        for line in self.early_lines.take().unwrap_or_default() {
            change = combine(change, self.lines.apply(line));
        }
        change
    }

    /// The answer to a request about one line: the line as it is now.
    pub fn line_updated(&mut self, line: LineInfo) -> RowChange {
        self.lines.apply(line)
    }

    /// The answer to `lines.remove`.
    pub fn line_removed(&mut self, id: u16) -> RowChange {
        self.lines.remove(id)
    }

    /// The answer to `calls.list`.
    pub fn calls_listed(&mut self, calls: Vec<CallRecord>) {
        self.calls.set_all(calls);
    }

    /// The answer to `log.tail`.
    pub fn log_tailed(&mut self, lines: Vec<LogEvent>) {
        self.log.set_tail(lines);
    }

    /// The answer to `stats.get`.
    pub fn stats_received(&mut self, stats: Stats) {
        self.stats = Some(stats);
    }

    /// An event from the service.
    pub fn event(&mut self, event: Event) -> Changes {
        if !self.connected() {
            return Changes::NONE;
        }
        match event {
            Event::LineState(line) => {
                if let Some(early) = &mut self.early_lines {
                    early.push((*line).clone());
                }
                Changes::rows(self.lines.apply(*line))
            }
            // The line's own event carries the call.
            Event::CallStarted(_) | Event::CallConnected(_) => Changes::NONE,
            Event::CallEnded(record) => Changes {
                calls: self.calls.push(record),
                ..Changes::NONE
            },
            Event::Log(line) => Changes {
                log: self.log.push(line),
                ..Changes::NONE
            },
        }
    }

    // ----------------------------------------------------------------------
    // What the window shows
    // ----------------------------------------------------------------------

    /// The title of the window.
    pub fn title(&self, machine: &str) -> String {
        let Some(info) = self.service() else {
            return format!("rustmodem Manager {EMPTY} not connected");
        };
        // The name of the file, not the whole path.
        let file = info
            .config_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&info.config_path);
        let lines = self.lines.len();
        let mut title = format!(
            "rustmodem Manager {EMPTY} {file} [{lines} line{}] {EMPTY} {machine}",
            if lines == 1 { "" } else { "s" }
        );
        match info.mode {
            ServiceMode::Service => {}
            ServiceMode::Console => title.push_str(" (console)"),
            ServiceMode::Demo => title.push_str(" (demo)"),
        }
        title
    }

    /// The title of the event log's group box.
    pub fn log_title(&self) -> String {
        match self.service() {
            // The demo service keeps no log file.
            Some(info) if info.mode == ServiceMode::Demo => {
                format!("Event log {EMPTY} demo service")
            }
            _ => format!("Event log {EMPTY} {LOG_FILE}"),
        }
    }

    /// The statistics panels. `now_ms` is the time now.
    pub fn stats_view(&self, now_ms: u64, zone: &dyn TimeZone) -> StatsView {
        match (&self.stats, self.service()) {
            (Some(stats), Some(info)) => StatsView::new(stats, info, now_ms, zone),
            _ => StatsView::empty(),
        }
    }

    /// The message part of the status bar: why the manager is not
    /// connected, or that it is ready.
    pub fn status_message(&self) -> String {
        match &self.connection {
            ConnectionState::Connected(_) => "Ready".to_string(),
            ConnectionState::Connecting => "Connecting to the service…".to_string(),
            ConnectionState::Disconnected(Some(reason)) => reason.to_string(),
            ConnectionState::Disconnected(None) => "Not connected.".to_string(),
        }
    }

    /// The parts of the status bar after the message: the connection, SIP,
    /// how many lines can answer, RTP, the processor, and the clock.
    pub fn status_parts(&self, now_ms: u64, zone: &dyn TimeZone) -> [String; 6] {
        let connection = match &self.connection {
            ConnectionState::Connected(_) => "Service: connected",
            ConnectionState::Connecting => "Service: connecting",
            ConnectionState::Disconnected(_) => "Service: not connected",
        };
        let answering = if self.connected() {
            let ready = self
                .lines
                .lines()
                .filter(|line| !matches!(line.state, LineState::Disabled | LineState::NotReady))
                .count();
            format!("Answering: {ready} / {}", self.lines.len())
        } else {
            format!("Answering: {EMPTY}")
        };
        [
            connection.to_string(),
            // BLOCKED(phase2): the state of SIP and of RTP come with
            // telephony.
            "SIP: off".to_string(),
            answering,
            format!("RTP: {EMPTY}"),
            // BLOCKED(phase9): the service does not report its processor
            // time.
            format!("CPU: {EMPTY}"),
            zone.wall(now_ms).clock(),
        ]
    }

    /// Whether a command can be chosen now, and if not, why not.
    pub fn availability(&self, command: Command) -> Availability {
        use Availability::{Disabled, Enabled};
        if let Some(reason) = command.blocked_reason() {
            return Disabled(reason);
        }
        let not_connected = || Disabled("Not connected to the service.".to_string());
        let no_selection = || Disabled("Select a line first.".to_string());
        // A command on the selected line needs the service and a line.
        let on_line = |allowed: &dyn Fn(&LineInfo) -> Availability| {
            if !self.connected() {
                return not_connected();
            }
            match self.lines.selected() {
                Some(line) => allowed(line),
                None => no_selection(),
            }
        };
        match command {
            // What does not depend on the service.
            Command::Connect
            | Command::Exit
            | Command::AtConsole
            | Command::ShowAll
            | Command::ShowOnCall
            | Command::ShowIdle
            | Command::ShowNotReady
            | Command::ViewToolbar
            | Command::ViewStatusBar
            | Command::About
            | Command::ServiceStart
            | Command::ServiceStop
            | Command::ServiceRestart
            | Command::ServiceInstall
            | Command::ServiceUninstall => Enabled,

            Command::Disconnect => match self.connection {
                ConnectionState::Disconnected(None) => Disabled(
                    "The manager is not connected and is not trying to connect.".to_string(),
                ),
                _ => Enabled,
            },

            // What needs the service and nothing more.
            Command::ReloadConfig
            | Command::NewLine
            | Command::Refresh
            | Command::Settings
            | Command::PortsAndDrivers
            | Command::CallHistory
            | Command::ExportStatistics => {
                if self.connected() {
                    Enabled
                } else {
                    not_connected()
                }
            }
            Command::ExportLog => {
                if self.log.is_empty() {
                    Disabled("The event log is empty.".to_string())
                } else {
                    Enabled
                }
            }

            Command::Answer => on_line(&|line| {
                if line.state == LineState::Ringing {
                    Enabled
                } else {
                    Disabled(format!("Line {} is not ringing.", line.id))
                }
            }),
            Command::HangUp => on_line(&|line| {
                if on_a_call(line) {
                    Enabled
                } else {
                    Disabled(format!("Line {} has no call.", line.id))
                }
            }),
            Command::Reset | Command::EditLine => on_line(&|_| Enabled),
            Command::Enable => on_line(&|line| {
                if line.enabled {
                    Disabled(format!("Line {} is already enabled.", line.id))
                } else {
                    Enabled
                }
            }),
            Command::Disable => on_line(&|line| {
                if line.enabled {
                    Enabled
                } else {
                    Disabled(format!("Line {} is already disabled.", line.id))
                }
            }),
            Command::RemoveLine => on_line(&|line| {
                if on_a_call(line) {
                    Disabled(format!(
                        "Line {} is in a call. Hang up before removing it.",
                        line.id
                    ))
                } else {
                    Enabled
                }
            }),

            // The commands that are not built yet were answered above.
            Command::Reregister
            | Command::TrunkPeers
            | Command::HuntGroups
            | Command::Routes
            | Command::SipTrace
            | Command::RateCap
            | Command::LineDiagnostics
            | Command::LoopbackTest
            | Command::CallCapture => Disabled("Not available yet.".to_string()),
        }
    }

    /// What the status bar says while a command is highlighted: why it
    /// cannot be chosen, or what it does.
    pub fn help(&self, command: Command) -> String {
        match self.availability(command) {
            Availability::Enabled => command.description().to_string(),
            Availability::Disabled(reason) => reason,
        }
    }

    /// The View menu command that goes with the filter.
    pub fn filter_command(&self) -> Command {
        match self.lines.filter() {
            Filter::All => Command::ShowAll,
            Filter::OnCall => Command::ShowOnCall,
            Filter::Idle => Command::ShowIdle,
            Filter::NotReady => Command::ShowNotReady,
        }
    }
}

/// The filter a View menu command asks for.
pub fn filter_of(command: Command) -> Option<Filter> {
    match command {
        Command::ShowAll => Some(Filter::All),
        Command::ShowOnCall => Some(Filter::OnCall),
        Command::ShowIdle => Some(Filter::Idle),
        Command::ShowNotReady => Some(Filter::NotReady),
        _ => None,
    }
}

/// What two changes to the rows, one after the other, did together.
fn combine(first: RowChange, second: RowChange) -> RowChange {
    match (first, second) {
        (RowChange::None, other) | (other, RowChange::None) => other,
        // More than one row was touched: draw them all.
        _ => RowChange::Reset,
    }
}
