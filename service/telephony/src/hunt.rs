// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Routing and hunting: the line an incoming call is offered to.
//!
//! The called number is the user part of the Request-URI.
//!
//! 1. A call that arrived for a registered line goes to that line only
//!    ([`Hunter::route_to_line`]).
//! 2. Otherwise the `[[route]]` entries are tried in file order, and the
//!    first that matches decides; then the pilot numbers of the hunt
//!    groups ([`Hunter::route`]).
//! 3. Nothing matches: 404.
//!
//! A line can take a call when it is idle: ready, and not in a call. The
//! line is chosen once, when the call arrives; it is not hunted again if
//! the line does not answer. When no line of the target is idle the call
//! is turned away with 486 if any of them is in a call, and with 480 if
//! none of them is ready at all.

use std::collections::BTreeMap;

use config::{HuntGroup, Route, Strategy, Target};
use ctlproto::LineState;

/// A line id.
pub type LineId = u16;

/// Where a call goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Offer it to this line, which is idle.
    Line(LineId),
    /// Nothing is called by that number: 404 Not Found.
    NotFound,
    /// Every line it could go to that is in service is in a call: 486
    /// Busy Here.
    Busy,
    /// No line it could go to is ready: 480 Temporarily Unavailable.
    Unavailable,
}

impl Decision {
    /// The status the call is refused with, when it is refused.
    pub fn status(self) -> Option<u16> {
        match self {
            Decision::Line(_) => None,
            Decision::NotFound => Some(404),
            Decision::Busy => Some(486),
            Decision::Unavailable => Some(480),
        }
    }
}

/// What a line's state means to a call that wants it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// It can take the call.
    Idle,
    /// It has a call.
    InCall,
    /// It is disabled, not ready, or not there.
    NotReady,
}

fn standing(state: Option<LineState>) -> Standing {
    match state {
        Some(LineState::Idle) => Standing::Idle,
        Some(LineState::Ringing | LineState::Handshaking | LineState::Online) => Standing::InCall,
        Some(LineState::Disabled | LineState::NotReady) | None => Standing::NotReady,
    }
}

/// The routes and the hunt groups, with what hunting has to remember.
#[derive(Clone, Debug, Default)]
pub struct Hunter {
    routes: Vec<Route>,
    groups: Vec<HuntGroup>,
    /// For round-robin: the line that took each group's last call, by the
    /// group's name.
    last_taken: BTreeMap<String, LineId>,
    /// For least-recent: when each line's last call ended.
    released: BTreeMap<LineId, u64>,
}

impl Hunter {
    pub fn new(routes: &[Route], groups: &[HuntGroup]) -> Hunter {
        Hunter {
            routes: routes.to_vec(),
            groups: groups.to_vec(),
            last_taken: BTreeMap::new(),
            released: BTreeMap::new(),
        }
    }

    /// Takes new routes and hunt groups: the configuration changed. What
    /// is remembered of the groups that are still there is kept.
    pub fn reconfigure(&mut self, routes: &[Route], groups: &[HuntGroup]) {
        self.routes = routes.to_vec();
        self.groups = groups.to_vec();
        self.last_taken
            .retain(|name, _| groups.iter().any(|group| group.name == *name));
    }

    /// A call on `line` ended at `now_ms`: the line has been idle since.
    pub fn released(&mut self, line: LineId, now_ms: u64) {
        self.released.insert(line, now_ms);
    }

    /// The names of the hunt groups, with their pilot numbers.
    pub fn groups(&self) -> &[HuntGroup] {
        &self.groups
    }

    /// Routes a call that arrived for one line: a registered line's own
    /// contact.
    pub fn route_to_line(
        &self,
        line: LineId,
        state: &dyn Fn(LineId) -> Option<LineState>,
    ) -> Decision {
        match standing(state(line)) {
            Standing::Idle => Decision::Line(line),
            Standing::InCall => Decision::Busy,
            Standing::NotReady => Decision::Unavailable,
        }
    }

    /// Routes a call to a number: the routes in order, then the pilots.
    /// `state` is the state of a line now, `None` for a line that is not
    /// there.
    pub fn route(&mut self, called: &str, state: &dyn Fn(LineId) -> Option<LineState>) -> Decision {
        let target = self
            .routes
            .iter()
            .find(|route| route.matches(called))
            .map(|route| route.target.clone())
            .or_else(|| {
                self.groups
                    .iter()
                    .find(|group| group.pilot == called)
                    .map(|group| Target::Group(group.name.clone()))
            });
        match target {
            None => Decision::NotFound,
            Some(Target::Line(line)) => self.route_to_line(line, state),
            Some(Target::Group(name)) => self.hunt(&name, state),
        }
    }

    fn hunt(&mut self, name: &str, state: &dyn Fn(LineId) -> Option<LineState>) -> Decision {
        let Some(group) = self.groups.iter().find(|group| group.name == name) else {
            // A route to a group that is gone: the configuration was
            // changed under it.
            return Decision::Unavailable;
        };
        let standings: Vec<Standing> = group.lines.iter().map(|l| standing(state(*l))).collect();
        let idle = |index: &usize| standings[*index] == Standing::Idle;
        let count = group.lines.len();

        let chosen = match group.strategy {
            Strategy::FirstIdle => (0..count).find(idle),
            Strategy::RoundRobin => {
                // Start after the line that took the last call.
                let start = self
                    .last_taken
                    .get(name)
                    .and_then(|last| group.lines.iter().position(|line| line == last))
                    .map_or(0, |index| index + 1);
                (0..count).map(|n| (start + n) % count).find(idle)
            }
            // Idle longest: the earliest release, and a line that has had
            // no call before any that has. The first such in list order.
            Strategy::LeastRecent => (0..count)
                .filter(idle)
                .min_by_key(|index| self.released.get(&group.lines[*index]).copied()),
        };

        match chosen {
            Some(index) => {
                let line = group.lines[index];
                if group.strategy == Strategy::RoundRobin {
                    self.last_taken.insert(name.to_owned(), line);
                }
                Decision::Line(line)
            }
            None if standings.contains(&Standing::InCall) => Decision::Busy,
            None => Decision::Unavailable,
        }
    }
}
