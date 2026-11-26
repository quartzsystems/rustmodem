// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The call history: the calls that have ended, newest first.

use std::collections::VecDeque;

use ctlproto::{CallRecord, Direction, EndReason};

use crate::format::{EMPTY, TimeZone, csv_row, duration, grouped};

/// How many ended calls the history keeps: as many as the service does.
pub const CALL_LIMIT: usize = 1000;

/// The columns of the call history, in order.
pub const CALL_COLUMNS: [(&str, i32, bool); 8] = [
    ("Time", 130, false),
    ("Line", 40, true),
    ("Direction", 70, false),
    ("Number", 90, false),
    ("Duration", 60, true),
    ("Reason", 130, false),
    ("Rate", 100, false),
    ("Bytes TX/RX", 110, true),
];

/// The ended calls, newest first.
#[derive(Clone, Debug, Default)]
pub struct CallHistory {
    calls: VecDeque<CallRecord>,
}

/// Why a call ended, for people.
pub fn reason_text(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Hangup => "Host hung up",
        EndReason::Dtr => "Host dropped DTR",
        EndReason::Aborted => "Aborted by host",
        EndReason::NoAnswer => "No answer",
        EndReason::Busy => "Busy",
        EndReason::NoDialtone => "No dial tone",
        EndReason::Remote => "Far end cleared",
        EndReason::Abandoned => "Caller gave up",
        EndReason::Failed => "Failed",
        EndReason::Reset => "Host reset",
        EndReason::Operator => "Operator",
        EndReason::Shutdown => "Service stopped",
    }
}

impl CallHistory {
    /// An empty history.
    pub fn new() -> CallHistory {
        CallHistory::default()
    }

    /// How many calls the history holds.
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// Whether the history holds no calls.
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// The calls, newest first.
    pub fn calls(&self) -> impl Iterator<Item = &CallRecord> {
        self.calls.iter()
    }

    /// The call in a row of the history.
    pub fn row(&self, row: usize) -> Option<&CallRecord> {
        self.calls.get(row)
    }

    /// Empties the history: the manager is connecting again.
    pub fn clear(&mut self) {
        self.calls.clear();
    }

    /// Takes in the answer to `calls.list`, which is oldest first. Calls
    /// that ended while it was on its way, and arrived as events, are
    /// kept.
    pub fn set_all(&mut self, calls: Vec<CallRecord>) {
        let early: Vec<CallRecord> = self.calls.drain(..).rev().collect();
        for call in calls {
            self.push(call);
        }
        for call in early {
            self.push(call);
        }
    }

    /// Takes in a call that has ended. Returns whether it was added: a
    /// call the history already has, by its id, is not.
    pub fn push(&mut self, call: CallRecord) -> bool {
        // A call that has just ended is among the newest.
        if self.calls.iter().any(|known| known.id == call.id) {
            return false;
        }
        if self.calls.len() == CALL_LIMIT {
            self.calls.pop_back();
        }
        self.calls.push_front(call);
        true
    }

    /// What a cell of the history shows.
    pub fn cell(&self, row: usize, column: usize, zone: &dyn TimeZone) -> String {
        let Some(call) = self.calls.get(row) else {
            return String::new();
        };
        match column {
            0 => zone.wall(call.started_ms).date_time(),
            1 => call.line.to_string(),
            2 => match call.direction {
                Direction::Incoming => "Incoming",
                Direction::Outgoing => "Outgoing",
            }
            .to_string(),
            3 => call.number.clone(),
            // How long the call was connected; a call that never
            // connected has no length.
            4 => match call.connected_ms {
                Some(connected) => duration(call.ended_ms.saturating_sub(connected)),
                None => EMPTY.to_string(),
            },
            5 => reason_text(call.reason).to_string(),
            6 => match &call.connect {
                Some(connect) => match (connect.rate, &connect.carrier) {
                    (0, None) => "Not known".to_string(),
                    (0, Some(carrier)) => carrier.clone(),
                    (rate, None) => format!("{rate} bit/s"),
                    (rate, Some(carrier)) => format!("{rate} {carrier}"),
                },
                None => EMPTY.to_string(),
            },
            7 => format!("{} / {}", grouped(call.bytes_tx), grouped(call.bytes_rx)),
            _ => String::new(),
        }
    }

    /// The history as a CSV file, newest first, with the numbers as
    /// numbers: a length in seconds, bytes without separators.
    pub fn csv(&self, zone: &dyn TimeZone) -> String {
        let mut out = csv_row(&[
            "Time",
            "Line",
            "Direction",
            "Number",
            "Connected seconds",
            "Reason",
            "Rate",
            "Carrier",
            "Bytes TX",
            "Bytes RX",
        ]);
        for call in &self.calls {
            let connected = call
                .connected_ms
                .map(|connected| (call.ended_ms.saturating_sub(connected) / 1000).to_string())
                .unwrap_or_default();
            let (rate, carrier) = match &call.connect {
                Some(connect) => (
                    connect.rate.to_string(),
                    connect.carrier.clone().unwrap_or_default(),
                ),
                None => (String::new(), String::new()),
            };
            out.push_str(&csv_row(&[
                &zone.wall(call.started_ms).date_time(),
                &call.line.to_string(),
                call.direction.as_str(),
                &call.number,
                &connected,
                call.reason.as_str(),
                &rate,
                &carrier,
                &call.bytes_tx.to_string(),
                &call.bytes_rx.to_string(),
            ]));
        }
        out
    }
}
