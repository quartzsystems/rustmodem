// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The line table: the lines the service has, as the grid shows them.
//!
//! The table holds every line by id. The grid shows the lines the filter
//! lets through, in the order of the sort. Every change to the table says
//! what it did to the rows of the grid ([`RowChange`]), so that the grid
//! redraws the one row that changed and keeps its selection and its
//! scroll position. The selection is a line id, never a row number: a row
//! number means another line as soon as a line is added or the sort
//! changes.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use ctlproto::{Direction, LineInfo, LineState, RegistrationState, SipMode};

use crate::format::{EMPTY, duration, grouped};

/// The columns of the grid, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    /// `#`: the line id.
    Number,
    /// `COM port`: the port the line is on.
    Port,
    /// `SIP`: the registration of the line.
    Sip,
    /// `Hunt group`: the group the line answers for.
    HuntGroup,
    /// `State`.
    State,
    /// `Calling party`: who is calling, or who was dialled.
    CallingParty,
    /// `Modulation`: the carrier negotiated.
    Modulation,
    /// `TX/RX bps`: the rates negotiated.
    Rate,
    /// `Duration`: how long the call has lasted.
    Duration,
    /// `Bytes TX/RX`: the data the line has carried.
    Bytes,
    /// `Retr.`: retrains.
    Retrains,
    /// `Err`: errors.
    Errors,
    /// `Binding`: what the host side of the port is bound to.
    Binding,
}

impl Column {
    /// Every column, in the order of the grid.
    pub const ALL: [Column; 13] = [
        Column::Number,
        Column::Port,
        Column::Sip,
        Column::HuntGroup,
        Column::State,
        Column::CallingParty,
        Column::Modulation,
        Column::Rate,
        Column::Duration,
        Column::Bytes,
        Column::Retrains,
        Column::Errors,
        Column::Binding,
    ];

    /// The column at a position of the grid.
    pub fn at(index: usize) -> Option<Column> {
        Column::ALL.get(index).copied()
    }

    /// The position of the column in the grid.
    pub fn index(self) -> usize {
        Column::ALL
            .iter()
            .position(|column| *column == self)
            .unwrap_or(0)
    }

    /// The text of the header.
    pub fn title(self) -> &'static str {
        match self {
            Column::Number => "#",
            Column::Port => "COM port",
            Column::Sip => "SIP",
            Column::HuntGroup => "Hunt group",
            Column::State => "State",
            Column::CallingParty => "Calling party",
            Column::Modulation => "Modulation",
            Column::Rate => "TX/RX bps",
            Column::Duration => "Duration",
            Column::Bytes => "Bytes TX/RX",
            Column::Retrains => "Retr.",
            Column::Errors => "Err",
            Column::Binding => "Binding",
        }
    }

    /// The width the column starts with, in pixels at 96 DPI. The last
    /// column takes what is left of the grid; this is the least it gets.
    pub fn default_width(self) -> i32 {
        match self {
            // The handoff gives 30, which is the number alone: the state
            // image is at the start of the row and takes 16 of them.
            Column::Number => 48,
            Column::Port => 70,
            Column::Sip => 60,
            Column::HuntGroup => 80,
            Column::State => 100,
            Column::CallingParty => 110,
            Column::Modulation => 90,
            Column::Rate => 90,
            Column::Duration => 70,
            Column::Bytes => 110,
            Column::Retrains => 45,
            Column::Errors => 40,
            Column::Binding => 200,
        }
    }

    /// Whether the column holds numbers, which are set to the right.
    pub fn right_aligned(self) -> bool {
        matches!(
            self,
            Column::Number
                | Column::Rate
                | Column::Duration
                | Column::Bytes
                | Column::Retrains
                | Column::Errors
        )
    }
}

/// Which lines the grid shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Filter {
    /// Every line.
    #[default]
    All,
    /// The lines with a call: ringing, handshaking or online.
    OnCall,
    /// The lines waiting for a call.
    Idle,
    /// The lines that are not ready or are disabled.
    NotReady,
}

impl Filter {
    /// Every filter, in the order of the drop-down list.
    pub const ALL: [Filter; 4] = [Filter::All, Filter::OnCall, Filter::Idle, Filter::NotReady];

    /// The filter as the drop-down list and the View menu name it.
    pub fn title(self) -> &'static str {
        match self {
            Filter::All => "All lines",
            Filter::OnCall => "On a call",
            Filter::Idle => "Idle",
            Filter::NotReady => "Not ready or disabled",
        }
    }

    /// The position of the filter in the drop-down list.
    pub fn index(self) -> usize {
        Filter::ALL
            .iter()
            .position(|filter| *filter == self)
            .unwrap_or(0)
    }

    /// The filter at a position of the drop-down list.
    pub fn at(index: usize) -> Option<Filter> {
        Filter::ALL.get(index).copied()
    }

    /// Whether the filter lets a line through.
    pub fn admits(self, line: &LineInfo) -> bool {
        match self {
            Filter::All => true,
            Filter::OnCall => on_a_call(line),
            Filter::Idle => line.state == LineState::Idle,
            Filter::NotReady => {
                matches!(line.state, LineState::NotReady | LineState::Disabled)
            }
        }
    }
}

/// Whether a line has a call: one being offered to it, set up or connected.
pub fn on_a_call(line: &LineInfo) -> bool {
    matches!(
        line.state,
        LineState::Ringing | LineState::Handshaking | LineState::Online
    )
}

/// How the grid is sorted: by one column, one way or the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SortOrder {
    /// The column.
    pub column: Column,
    /// Largest first.
    pub descending: bool,
}

impl Default for SortOrder {
    fn default() -> Self {
        SortOrder {
            column: Column::Number,
            descending: false,
        }
    }
}

/// How the state of a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StateLook {
    /// Out of service by configuration.
    Disabled,
    /// The port is closed to the host, or the host has DTR off.
    NotReady,
    /// Waiting for a call.
    Idle,
    /// A call is being offered.
    Ringing,
    /// A call is being set up.
    Handshaking,
    /// A call is connected.
    Online,
    /// The port will not open.
    PortError,
}

impl StateLook {
    /// Every look, in the order of the images of the line states.
    pub const ALL: [StateLook; 7] = [
        StateLook::Disabled,
        StateLook::NotReady,
        StateLook::Idle,
        StateLook::Ringing,
        StateLook::Handshaking,
        StateLook::Online,
        StateLook::PortError,
    ];

    /// How a line looks.
    pub fn of(line: &LineInfo) -> StateLook {
        match line.state {
            LineState::Disabled => StateLook::Disabled,
            LineState::NotReady if line.error.is_some() => StateLook::PortError,
            LineState::NotReady => StateLook::NotReady,
            LineState::Idle => StateLook::Idle,
            LineState::Ringing => StateLook::Ringing,
            LineState::Handshaking => StateLook::Handshaking,
            LineState::Online => StateLook::Online,
        }
    }

    /// The index of the image of the look, in the order of the images of
    /// the line states.
    pub fn image(self) -> usize {
        StateLook::ALL
            .iter()
            .position(|look| *look == self)
            .unwrap_or(0)
    }

    /// What the State column says. An idle line is in AT command mode,
    /// which is worth saying: it is the mode in which the host can dial.
    pub fn text(self) -> &'static str {
        match self {
            StateLook::Disabled => "Disabled",
            StateLook::NotReady => "Not ready",
            StateLook::Idle => "Idle (AT)",
            StateLook::Ringing => "Ringing",
            StateLook::Handshaking => "Handshaking",
            StateLook::Online => "Online",
            StateLook::PortError => "Port error",
        }
    }

    /// The colour of the State column's text.
    pub fn colour(self) -> StateColour {
        match self {
            StateLook::Online => StateColour::Green,
            StateLook::Ringing | StateLook::Handshaking => StateColour::Yellow,
            StateLook::Idle => StateColour::Blue,
            StateLook::PortError => StateColour::Red,
            StateLook::Disabled | StateLook::NotReady => StateColour::Default,
        }
    }
}

/// The colour of a state's text: that of the state's image at the start
/// of the row. A line that is out of service has the control's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StateColour {
    /// The control's own text colour.
    Default,
    /// Online.
    Green,
    /// A call is on its way: ringing, or handshaking.
    Yellow,
    /// Idle, waiting for a call.
    Blue,
    /// A port error.
    Red,
}

/// Whether a line's registration to its PBX has failed.
pub fn registration_failed(line: &LineInfo) -> bool {
    line.sip
        .registration
        .as_ref()
        .is_some_and(|registration| registration.state == RegistrationState::Failed)
}

/// The image at the start of a line's row: that of its state, except that
/// a line that waits for calls and whose registration has failed has the
/// red one, because no call will come to it.
pub fn row_image(line: &LineInfo) -> usize {
    let look = StateLook::of(line);
    if look == StateLook::Idle && registration_failed(line) {
        StateLook::PortError.image()
    } else {
        look.image()
    }
}

/// What the tip of a line's SIP cell says: how the line is reached, and
/// for a line that registers, how its registration stands. `None` for a
/// line of a service with no SIP.
pub fn sip_tip(line: &LineInfo) -> Option<String> {
    match line.sip.mode {
        SipMode::None => None,
        SipMode::Trunk => {
            Some("Reached by the trunk: calls come from the trusted peers.".to_string())
        }
        SipMode::Registered => {
            let who = format!(
                "Extension {} of {}",
                line.sip.extension.as_deref().unwrap_or(EMPTY),
                line.sip.server.as_deref().unwrap_or(EMPTY)
            );
            Some(match &line.sip.registration {
                None => format!("{who}: not registering."),
                Some(registration) => match (&registration.error, registration.expires_in_s) {
                    (Some(error), _) => format!("{who}: registration failed. {error}"),
                    (None, Some(left)) => format!(
                        "{who}: {}, {left} s left.",
                        registration_text(registration.state)
                    ),
                    (None, None) => {
                        format!("{who}: {}.", registration_text(registration.state))
                    }
                },
            })
        }
    }
}

/// A registration state, for people.
pub fn registration_text(state: RegistrationState) -> &'static str {
    match state {
        RegistrationState::Unregistered => "not registered",
        RegistrationState::Registering => "registering",
        RegistrationState::Registered => "registered",
        RegistrationState::Failed => "registration failed",
    }
}

/// What a change to the table did to the rows of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowChange {
    /// Nothing the grid shows changed.
    None,
    /// One row shows something else; the rows are the same lines in the
    /// same order.
    Changed(usize),
    /// A row was put in at this position; the rows from there on moved
    /// down one.
    Added(usize),
    /// The row at this position was taken out; the rows after it moved up
    /// one.
    Removed(usize),
    /// The rows are other lines or in another order: all of them are to
    /// be drawn again.
    Reset,
}

/// The lines, and the rows of the grid.
#[derive(Clone, Debug, Default)]
pub struct LineTable {
    lines: BTreeMap<u16, LineInfo>,
    filter: Filter,
    /// The hunt group the grid is narrowed to, if it is.
    group: Option<String>,
    sort: SortOrder,
    /// The ids of the lines the grid shows, in the order it shows them.
    rows: Vec<u16>,
    selected: Option<u16>,
}

/// The text of a cell, without the moving parts: what sorting compares.
fn sort_text(line: &LineInfo, column: Column) -> String {
    cell_text(line, column, 0)
}

/// What a cell shows. `now_ms` is the time now, for how long a call has
/// lasted.
fn cell_text(line: &LineInfo, column: Column, now_ms: u64) -> String {
    let empty = || EMPTY.to_string();
    let connect = line.call.as_ref().and_then(|call| call.connect.as_ref());
    match column {
        Column::Number => line.id.to_string(),
        Column::Port => line.port.clone(),
        Column::Sip => match line.sip.mode {
            SipMode::None => empty(),
            SipMode::Trunk => "trunk".to_string(),
            SipMode::Registered => line.sip.extension.clone().unwrap_or_else(empty),
        },
        Column::HuntGroup => {
            if line.hunt_groups.is_empty() {
                empty()
            } else {
                line.hunt_groups.join(", ")
            }
        }
        Column::State => StateLook::of(line).text().to_string(),
        Column::CallingParty => match &line.call {
            Some(call) if call.direction == Direction::Incoming => call.number.clone(),
            // The line dialled: the arrow says the number is the one
            // called, not the caller.
            Some(call) => format!("\u{2192} {}", call.number),
            None => empty(),
        },
        Column::Modulation => connect
            .and_then(|connect| connect.carrier.clone())
            .unwrap_or_else(empty),
        Column::Rate => match connect {
            Some(connect) if connect.rate > 0 => format!("{0}/{0}", connect.rate),
            _ => empty(),
        },
        Column::Duration => match &line.call {
            Some(call) => {
                // From the connection once there is one, and from the
                // start of the call until then.
                let since = call.connected_ms.unwrap_or(call.started_ms);
                duration(now_ms.saturating_sub(since))
            }
            None => empty(),
        },
        Column::Bytes => format!(
            "{} / {}",
            grouped(line.counters.bytes_tx),
            grouped(line.counters.bytes_rx)
        ),
        // BLOCKED(phase3): retrains are counted by a real modem engine.
        Column::Retrains => empty(),
        // What went wrong with the RTP of the call in progress: packets
        // lost and late, and streams started again.
        // BLOCKED(phase4): the errors the error-control protocol counts
        // are added to these.
        Column::Errors => match line.call.as_ref().and_then(|call| call.media.as_ref()) {
            Some(media) => grouped(media.lost + media.late + media.resync),
            None => empty(),
        },
        Column::Binding => line.binding.clone().unwrap_or_else(empty),
    }
}

/// The order of two lines by a column. Numbers compare as numbers; text
/// compares without regard to case; empty cells come after everything
/// else. Lines that compare equal are in id order, whichever way the sort
/// goes, so that the order is always the same for the same lines.
fn compare(a: &LineInfo, b: &LineInfo, order: SortOrder) -> Ordering {
    let number = |line: &LineInfo| -> Option<u64> {
        match order.column {
            Column::Number => Some(u64::from(line.id)),
            Column::Rate => line
                .call
                .as_ref()
                .and_then(|call| call.connect.as_ref())
                .map(|connect| u64::from(connect.rate))
                .filter(|rate| *rate > 0),
            // The call that started first has lasted longest.
            Column::Duration => line
                .call
                .as_ref()
                .map(|call| u64::MAX - call.connected_ms.unwrap_or(call.started_ms)),
            Column::Bytes => Some(line.counters.bytes_tx + line.counters.bytes_rx),
            Column::State => Some(StateLook::of(line).image() as u64),
            _ => None,
        }
    };
    let numeric = matches!(
        order.column,
        Column::Number | Column::Rate | Column::Duration | Column::Bytes | Column::State
    );
    let by_column = if numeric {
        match (number(a), number(b)) {
            (Some(x), Some(y)) => {
                let ordering = x.cmp(&y);
                if order.descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            }
            // Empty cells last, whichever way the sort goes.
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    } else {
        let (x, y) = (sort_text(a, order.column), sort_text(b, order.column));
        match (x == EMPTY, y == EMPTY) {
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            (true, true) => Ordering::Equal,
            (false, false) => {
                let ordering = natural(&x, &y);
                if order.descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            }
        }
    };
    by_column.then(a.id.cmp(&b.id))
}

/// Text order without regard to case, with a number at the end compared
/// as a number: `CNCB9` before `CNCB10`.
fn natural(a: &str, b: &str) -> Ordering {
    let split = |text: &str| -> (String, Option<u64>) {
        let folded = text.to_lowercase();
        let digits = folded.bytes().rev().take_while(u8::is_ascii_digit).count();
        let (letters, number) = folded.split_at(folded.len() - digits);
        (letters.to_string(), number.parse().ok())
    };
    let (a_letters, a_number) = split(a);
    let (b_letters, b_number) = split(b);
    a_letters
        .cmp(&b_letters)
        .then(a_number.cmp(&b_number))
        .then(a.cmp(b))
}

impl LineTable {
    /// A table with no lines, showing all lines in id order.
    pub fn new() -> LineTable {
        LineTable::default()
    }

    /// Every line, in id order, whatever the filter.
    pub fn lines(&self) -> impl Iterator<Item = &LineInfo> {
        self.lines.values()
    }

    /// One line, whatever the filter.
    pub fn line(&self, id: u16) -> Option<&LineInfo> {
        self.lines.get(&id)
    }

    /// How many lines there are, whatever the filter.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether there are no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// How many lines have a call.
    pub fn on_call(&self) -> usize {
        self.lines.values().filter(|line| on_a_call(line)).count()
    }

    /// How many lines are waiting for a call.
    pub fn waiting(&self) -> usize {
        self.lines
            .values()
            .filter(|line| line.state == LineState::Idle)
            .count()
    }

    /// The lowest line id that no line has, for a new line.
    pub fn lowest_free_id(&self) -> u16 {
        (1..=u16::MAX)
            .find(|id| !self.lines.contains_key(id))
            .unwrap_or(u16::MAX)
    }

    /// The filter.
    pub fn filter(&self) -> Filter {
        self.filter
    }

    /// The hunt group the grid is narrowed to, if it is.
    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    /// The names of the hunt groups the lines are in, in order of name.
    pub fn groups(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .lines
            .values()
            .flat_map(|line| line.hunt_groups.iter().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Whether the grid shows a line: the filter lets it through, and it
    /// is in the hunt group the grid is narrowed to.
    fn admits(&self, line: &LineInfo) -> bool {
        self.filter.admits(line)
            && self
                .group
                .as_ref()
                .is_none_or(|group| line.hunt_groups.contains(group))
    }

    /// Narrows the grid to the lines of a hunt group, or widens it again.
    pub fn set_group(&mut self, group: Option<String>) -> RowChange {
        if self.group == group {
            return RowChange::None;
        }
        let before = self.rows.clone();
        self.group = group;
        self.rebuild();
        if self.rows == before {
            RowChange::None
        } else {
            RowChange::Reset
        }
    }

    /// How the grid is sorted.
    pub fn sort(&self) -> SortOrder {
        self.sort
    }

    /// How many rows the grid has.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The line a row shows.
    pub fn row(&self, row: usize) -> Option<&LineInfo> {
        self.rows.get(row).and_then(|id| self.lines.get(id))
    }

    /// The row a line is shown in, if the filter lets it through.
    pub fn row_of(&self, id: u16) -> Option<usize> {
        self.rows.iter().position(|row| *row == id)
    }

    /// What a cell of the grid shows. `now_ms` is the time now, for how
    /// long a call has lasted.
    pub fn cell(&self, row: usize, column: Column, now_ms: u64) -> String {
        self.row(row)
            .map(|line| cell_text(line, column, now_ms))
            .unwrap_or_default()
    }

    /// How the state of the line in a row is drawn.
    pub fn look(&self, row: usize) -> Option<StateLook> {
        self.row(row).map(StateLook::of)
    }

    /// The rows whose Duration cell changes as time passes: the lines
    /// with a call. They are drawn again every second.
    pub fn ticking_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, id)| self.lines.get(id).is_some_and(|line| line.call.is_some()))
            .map(|(row, _)| row)
            .collect()
    }

    /// The selected line, by id.
    pub fn selected(&self) -> Option<&LineInfo> {
        self.selected.and_then(|id| self.lines.get(&id))
    }

    /// The row of the selected line, if the filter lets it through.
    pub fn selected_row(&self) -> Option<usize> {
        self.selected.and_then(|id| self.row_of(id))
    }

    /// Selects the line a row shows, or with `None` no line.
    pub fn select_row(&mut self, row: Option<usize>) {
        self.selected = row.and_then(|row| self.rows.get(row).copied());
    }

    /// Selects a line by id. A line the table does not have cannot be
    /// selected.
    pub fn select(&mut self, id: Option<u16>) {
        self.selected = id.filter(|id| self.lines.contains_key(id));
    }

    /// Where a line belongs among the rows.
    fn position(&self, line: &LineInfo) -> usize {
        self.rows.partition_point(|row| match self.lines.get(row) {
            Some(other) => compare(other, line, self.sort) == Ordering::Less,
            None => true,
        })
    }

    fn rebuild(&mut self) {
        let mut shown: Vec<&LineInfo> = self
            .lines
            .values()
            .filter(|line| self.admits(line))
            .collect();
        shown.sort_by(|a, b| compare(a, b, self.sort));
        self.rows = shown.into_iter().map(|line| line.id).collect();
    }

    /// Replaces every line: the answer to `lines.list`. A selected line
    /// that is still there stays selected.
    pub fn set_all(&mut self, lines: Vec<LineInfo>) -> RowChange {
        let before = self.rows.clone();
        let unchanged = self.lines.len() == lines.len()
            && lines
                .iter()
                .all(|line| self.lines.get(&line.id) == Some(line));
        self.lines = lines.into_iter().map(|line| (line.id, line)).collect();
        if self
            .selected
            .is_some_and(|id| !self.lines.contains_key(&id))
        {
            self.selected = None;
        }
        // A hunt group that is gone narrows the grid to nothing: the grid
        // is widened again.
        if self
            .group
            .as_ref()
            .is_some_and(|group| !self.groups().contains(group))
        {
            self.group = None;
        }
        self.rebuild();
        if unchanged && self.rows == before {
            RowChange::None
        } else {
            RowChange::Reset
        }
    }

    /// Takes in one line as the service reports it now: a `line.state`
    /// event, or the answer to a request about the line. The line is
    /// added if the table does not have it.
    pub fn apply(&mut self, line: LineInfo) -> RowChange {
        let id = line.id;
        if self.lines.get(&id) == Some(&line) {
            return RowChange::None;
        }
        let old_row = self.row_of(id);
        if let Some(row) = old_row {
            self.rows.remove(row);
        }
        let new_row = self.admits(&line).then(|| self.position(&line));
        if let Some(row) = new_row {
            self.rows.insert(row, id);
        }
        self.lines.insert(id, line);
        match (old_row, new_row) {
            (None, None) => RowChange::None,
            (None, Some(row)) => RowChange::Added(row),
            (Some(row), None) => RowChange::Removed(row),
            (Some(old), Some(new)) if old == new => RowChange::Changed(new),
            // The line moved among the rows.
            (Some(_), Some(_)) => RowChange::Reset,
        }
    }

    /// Takes a line out: it was removed from the service.
    pub fn remove(&mut self, id: u16) -> RowChange {
        if self.lines.remove(&id).is_none() {
            return RowChange::None;
        }
        if self.selected == Some(id) {
            self.selected = None;
        }
        match self.row_of(id) {
            Some(row) => {
                self.rows.remove(row);
                RowChange::Removed(row)
            }
            None => RowChange::None,
        }
    }

    /// Takes every line out: the service is not there.
    pub fn clear(&mut self) -> RowChange {
        if self.lines.is_empty() {
            return RowChange::None;
        }
        self.lines.clear();
        self.rows.clear();
        self.selected = None;
        RowChange::Reset
    }

    /// Changes which lines the grid shows.
    pub fn set_filter(&mut self, filter: Filter) -> RowChange {
        if self.filter == filter {
            return RowChange::None;
        }
        let before = self.rows.clone();
        self.filter = filter;
        self.rebuild();
        if self.rows == before {
            RowChange::None
        } else {
            RowChange::Reset
        }
    }

    /// Changes how the grid is sorted.
    pub fn set_sort(&mut self, sort: SortOrder) -> RowChange {
        if self.sort == sort {
            return RowChange::None;
        }
        let before = self.rows.clone();
        self.sort = sort;
        self.rebuild();
        if self.rows == before {
            RowChange::None
        } else {
            RowChange::Reset
        }
    }

    /// What clicking a column header does to the sort: a click on the
    /// column the grid is sorted by turns the sort round, and a click on
    /// another column sorts by it, smallest first.
    pub fn sort_after_click(&self, column: Column) -> SortOrder {
        if self.sort.column == column {
            SortOrder {
                column,
                descending: !self.sort.descending,
            }
        } else {
            SortOrder {
                column,
                descending: false,
            }
        }
    }

    /// The title of the grid's group box.
    pub fn title(&self) -> String {
        format!(
            "Modem lines {EMPTY} {} configured, {} on a call, {} waiting",
            self.len(),
            self.on_call(),
            self.waiting()
        )
    }
}
