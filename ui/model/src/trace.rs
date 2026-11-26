// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The SIP trace: the last messages the service sent and received.
//!
//! The buffer is filled from `sip.trace` events while the trace window is
//! open. While it is paused what arrives is dropped: the window stands
//! still so that it can be read.

use std::collections::VecDeque;

use ctlproto::{SipTrace, TraceDirection};

use crate::format::TimeZone;

/// How many messages the buffer keeps.
pub const TRACE_LIMIT: usize = 1000;

/// The last SIP messages.
#[derive(Clone, Debug, Default)]
pub struct TraceBuffer {
    messages: VecDeque<SipTrace>,
    paused: bool,
    /// How many messages have been taken in since the buffer was last
    /// emptied, the ones that have fallen out of it with them.
    taken: u64,
}

impl TraceBuffer {
    /// An empty buffer.
    pub fn new() -> TraceBuffer {
        TraceBuffer::default()
    }

    /// How many messages the buffer holds.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the buffer holds no messages.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// How many messages have been taken in since the buffer was last
    /// emptied.
    pub fn taken(&self) -> u64 {
        self.taken
    }

    /// Whether what arrives is dropped.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Stops or starts taking what arrives.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Empties the buffer.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.taken = 0;
    }

    /// Takes in a message. Returns whether it was: while paused it is
    /// not.
    pub fn push(&mut self, message: SipTrace) -> bool {
        if self.paused {
            return false;
        }
        if self.messages.len() == TRACE_LIMIT {
            self.messages.pop_front();
        }
        self.messages.push_back(message);
        self.taken += 1;
        true
    }

    /// The messages, oldest first.
    pub fn messages(&self) -> impl Iterator<Item = &SipTrace> {
        self.messages.iter()
    }

    /// The message that was taken in last.
    pub fn last(&self) -> Option<&SipTrace> {
        self.messages.back()
    }

    /// The whole trace as text, oldest first: what the window shows and
    /// what Save writes.
    pub fn text(&self, zone: &dyn TimeZone) -> String {
        self.messages
            .iter()
            .map(|message| entry(message, zone))
            .collect()
    }
}

/// One message as the trace shows it: a line that says when, which way
/// and with whom, the message itself, and an empty line.
pub fn entry(message: &SipTrace, zone: &dyn TimeZone) -> String {
    let way = match message.direction {
        TraceDirection::Sent => "sent to",
        TraceDirection::Received => "received from",
    };
    // The message has the line ends of the wire; the window wants one
    // kind.
    let text = message.text.replace("\r\n", "\n");
    format!(
        "--- {}.{:03} {way} {} ---\n{}\n\n",
        zone.wall(message.time_ms).time(),
        message.time_ms % 1000,
        message.peer,
        text.trim_end()
    )
}
