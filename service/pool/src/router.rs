// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The call router: what stands between the lines and the network.
//!
//! A [`CallRouter`] takes the calls the lines make and offers calls to
//! lines. When a call is through it hands the pool the far end of its
//! media, a [`MediaEndpoint`], and has nothing more to do with the audio.
//!
//! The [`NullSwitch`] is the router with no network: it connects lines to
//! each other. With SIP enabled the router of `service/telephony` takes
//! its place behind this same trait, without touching the lines.

use std::collections::VecDeque;

use ctlproto::LineState;

use crate::media::{LocalEndpoint, MediaEndpoint};

/// A line id.
pub type LineId = u16;

/// How long the null switch waits before failing a call to a number that
/// cannot be reached, in milliseconds.
pub const UNOBTAINABLE_DELAY_MS: u64 = 2000;

/// The ring cadence: on for this long, in milliseconds.
pub const RING_ON_MS: u64 = 2000;

/// The ring cadence: then off for this long, in milliseconds.
pub const RING_OFF_MS: u64 = 4000;

/// What a router may ask about the lines.
pub trait LineDirectory {
    /// The state of a line, or `None` if there is no such line.
    fn state(&self, line: LineId) -> Option<LineState>;
}

/// Something the router tells a line.
#[derive(Debug)]
pub enum RouterEvent {
    /// A call is being offered to the line.
    Incoming {
        /// The line being called.
        line: LineId,
        /// Who is calling.
        from: String,
    },
    /// A ring-on interval began on the line's incoming call.
    Ring(LineId),
    /// The ring-on interval ended.
    RingEnd(LineId),
    /// The caller gave up; the call offered to the line is gone.
    Abandoned(LineId),
    /// The call is through: the line's modem should start, as the
    /// originating or the answering modem.
    Connected {
        /// The line whose call is through.
        line: LineId,
        /// Whether the line made the call.
        originate: bool,
        /// The far end of the call's media.
        media: MediaEndpoint,
    },
    /// The number the line dialled is busy.
    Busy(LineId),
    /// The number the line dialled did not answer.
    NoAnswer(LineId),
    /// There is no dial tone for the line.
    NoDialtone(LineId),
    /// The line's call could not be set up.
    Failed(LineId),
    /// The far end cleared the line's call.
    Cleared(LineId),
}

/// What stands between the lines and the network.
pub trait CallRouter: Send {
    /// The line went off hook and dialled `number`.
    fn dial(&mut self, now_ms: u64, from: LineId, number: &str, lines: &dyn LineDirectory);

    /// The line went off hook to answer.
    fn answer(&mut self, now_ms: u64, line: LineId);

    /// The line went on hook, or can no longer take the call offered to it.
    fn hangup(&mut self, now_ms: u64, line: LineId);

    /// Runs the router's timers and gives the next thing it has to say.
    fn poll(&mut self, now_ms: u64) -> Option<RouterEvent>;

    /// Stops the router: every call is cleared and what it holds on the
    /// network is given up. A router with no network has nothing to do.
    fn shutdown(&mut self, _now_ms: u64) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The callee is being rung.
    Ringing { since_ms: u64, ring_on: bool },
    /// The number cannot be reached; the caller is told at `at_ms`.
    Unobtainable { at_ms: u64 },
    /// Both ends are through.
    Connected,
}

#[derive(Debug)]
struct SwitchCall {
    caller: LineId,
    /// The line called; `None` when the number is not a line.
    callee: Option<LineId>,
    phase: Phase,
}

impl SwitchCall {
    fn involves(&self, line: LineId) -> bool {
        self.caller == line || self.callee == Some(line)
    }
}

/// The stand-in for the network in phase 0: `ATD<n>`, where `<n>` is a line
/// id, calls that line.
#[derive(Debug, Default)]
pub struct NullSwitch {
    calls: Vec<SwitchCall>,
    events: VecDeque<RouterEvent>,
}

impl NullSwitch {
    /// A switch with no calls.
    pub fn new() -> NullSwitch {
        NullSwitch::default()
    }

    /// How many calls the switch is carrying or setting up.
    pub fn call_count(&self) -> usize {
        self.calls.len()
    }

    fn timers(&mut self, now_ms: u64) {
        let events = &mut self.events;
        self.calls.retain_mut(|call| match &mut call.phase {
            Phase::Ringing { since_ms, ring_on } => {
                let Some(callee) = call.callee else {
                    return true;
                };
                let position = now_ms.saturating_sub(*since_ms) % (RING_ON_MS + RING_OFF_MS);
                let on = position < RING_ON_MS;
                if on != *ring_on {
                    *ring_on = on;
                    events.push_back(if on {
                        RouterEvent::Ring(callee)
                    } else {
                        RouterEvent::RingEnd(callee)
                    });
                }
                true
            }
            Phase::Unobtainable { at_ms } => {
                if now_ms >= *at_ms {
                    events.push_back(RouterEvent::Failed(call.caller));
                    false
                } else {
                    true
                }
            }
            Phase::Connected => true,
        });
    }
}

impl CallRouter for NullSwitch {
    fn dial(&mut self, now_ms: u64, from: LineId, number: &str, lines: &dyn LineDirectory) {
        // A line that dials has left whatever call it was in.
        self.hangup(now_ms, from);

        let digits_only = !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
        let target = if digits_only {
            number.parse::<LineId>().ok()
        } else {
            None
        };
        let state = target.and_then(|line| lines.state(line));
        match (target, state) {
            (Some(callee), Some(LineState::Idle))
                if callee != from && !self.calls.iter().any(|c| c.involves(callee)) =>
            {
                self.calls.push(SwitchCall {
                    caller: from,
                    callee: Some(callee),
                    phase: Phase::Ringing {
                        since_ms: now_ms,
                        ring_on: true,
                    },
                });
                self.events.push_back(RouterEvent::Incoming {
                    line: callee,
                    from: from.to_string(),
                });
                self.events.push_back(RouterEvent::Ring(callee));
            }
            (
                Some(_),
                Some(
                    LineState::Idle
                    | LineState::Ringing
                    | LineState::Handshaking
                    | LineState::Online,
                ),
            ) => self.events.push_back(RouterEvent::Busy(from)),
            // Not a line, or a line that is disabled or not ready: the
            // number is unobtainable, and the caller hears so after a pause.
            _ => self.calls.push(SwitchCall {
                caller: from,
                callee: None,
                phase: Phase::Unobtainable {
                    at_ms: now_ms + UNOBTAINABLE_DELAY_MS,
                },
            }),
        }
    }

    fn answer(&mut self, _now_ms: u64, line: LineId) {
        let ringing = self
            .calls
            .iter_mut()
            .find(|call| call.callee == Some(line) && matches!(call.phase, Phase::Ringing { .. }));
        match ringing {
            Some(call) => {
                if let Phase::Ringing { ring_on: true, .. } = call.phase {
                    self.events.push_back(RouterEvent::RingEnd(line));
                }
                call.phase = Phase::Connected;
                // The media of the two lines is joined end to end.
                let (caller, callee) = LocalEndpoint::pair();
                self.events.push_back(RouterEvent::Connected {
                    line: call.caller,
                    originate: true,
                    media: MediaEndpoint::new(caller),
                });
                self.events.push_back(RouterEvent::Connected {
                    line,
                    originate: false,
                    media: MediaEndpoint::new(callee),
                });
            }
            // Answering when nothing is calling gets nothing.
            None => self.events.push_back(RouterEvent::Failed(line)),
        }
    }

    fn hangup(&mut self, _now_ms: u64, line: LineId) {
        let Some(index) = self.calls.iter().position(|call| call.involves(line)) else {
            return;
        };
        let call = self.calls.remove(index);
        match (call.phase, call.callee) {
            (Phase::Ringing { ring_on, .. }, Some(callee)) => {
                if line == call.caller {
                    if ring_on {
                        self.events.push_back(RouterEvent::RingEnd(callee));
                    }
                    self.events.push_back(RouterEvent::Abandoned(callee));
                } else {
                    // The line being rung went away.
                    self.events.push_back(RouterEvent::Failed(call.caller));
                }
            }
            (Phase::Connected, Some(callee)) => {
                let other = if line == call.caller {
                    callee
                } else {
                    call.caller
                };
                self.events.push_back(RouterEvent::Cleared(other));
            }
            _ => {}
        }
    }

    fn poll(&mut self, now_ms: u64) -> Option<RouterEvent> {
        self.timers(now_ms);
        self.events.pop_front()
    }
}
