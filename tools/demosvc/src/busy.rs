// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The hosts of the `idle` and `busy` scripts.
//!
//! A [`Driver`] plays the host on every line's port, as RRAS or a terminal
//! program would on the real ones: it opens the port and turns echo off.
//! In the `idle` script that is all it does. In the `busy` script each
//! host also dials other lines, answers or ignores the calls it is rung
//! for, sends data while it is connected, and hangs up, with timings from
//! a generator that gives the same run every time.

use std::collections::BTreeMap;

use ctlproto::{LineInfo, LineState};
use serial::MemoryHost;

use crate::hosts::Hosts;

/// How often the driver looks at the lines, in milliseconds.
pub const STEP_MS: u64 = 100;

/// An idle host dials in one step out of this many: every six seconds or
/// so.
const DIAL_ONE_IN: u64 = 60;

/// A sequence of numbers that looks random and is the same every run
/// (xorshift64).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator from a seed.
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `limit`, which is not zero.
    pub fn below(&mut self, limit: u64) -> u64 {
        (self.next() >> 16) % limit
    }

    /// A number from `low` up to but not including `high`.
    pub fn between(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high - low)
    }

    /// True one time in `n`.
    pub fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }
}

/// How a host ends its call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hang {
    /// It drops DTR, as a program closing the port does.
    Dtr,
    /// It escapes to command mode and gives `ATH`.
    Escape,
}

/// What a host means to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// Nothing in particular.
    Nothing,
    /// It has dialled, at `since`, and gives up at `give_up_at`.
    Dialling { since: u64, give_up_at: u64 },
    /// It is being rung, and answers at `answer_at` if at all.
    Rung { answer_at: Option<u64> },
    /// It has answered and waits for the modems.
    Answered,
    /// It is connected, and hangs up at `hang_at`.
    Talking { hang_at: u64, how: Hang },
    /// It has stopped sending and types `+++` at `at`.
    EscapeGuard { at: u64 },
    /// It has typed `+++` and gives `ATH` at `at`.
    Escaped { at: u64 },
}

/// The host of one line.
#[derive(Clone, Debug)]
struct Persona {
    /// The port it opened; when the pool opens the port again this is a
    /// different one.
    host: MemoryHost,
    plan: Plan,
    /// When to raise DTR again after dropping it.
    dtr_up_at: Option<u64>,
}

/// Plays the host on every line.
pub struct Driver {
    hosts: Hosts,
    /// Whether the hosts make calls: the `busy` script.
    active: bool,
    rng: Rng,
    personas: BTreeMap<u16, Persona>,
    /// No host has dialled yet; the first one that can does so at once.
    first_call: bool,
}

impl Driver {
    /// A driver for the ports of `hosts`.
    pub fn new(hosts: Hosts, active: bool) -> Driver {
        Driver {
            hosts,
            active,
            rng: Rng::new(0x5eed_cafe_f00d),
            personas: BTreeMap::new(),
            first_call: true,
        }
    }

    /// One look at the lines, `now_ms` after the driver started.
    pub fn step(&mut self, now_ms: u64, lines: &[LineInfo]) {
        self.personas
            .retain(|id, _| lines.iter().any(|line| line.id == *id));
        for line in lines {
            let Some(host) = self.hosts.get(&line.port) else {
                continue;
            };
            // Nobody reads what the modems say; throw it away so that it
            // does not pile up.
            host.read();
            let known = self
                .personas
                .get(&line.id)
                .is_some_and(|persona| persona.host.backend_alive());
            if !known {
                // A port the pool has just opened, at the start or after
                // the line was enabled, moved or added: open it as a
                // program would and turn echo off.
                host.open();
                host.write(b"ATE0\r");
                let persona = Persona {
                    host,
                    plan: Plan::Nothing,
                    dtr_up_at: None,
                };
                self.personas.insert(line.id, persona);
                continue;
            }
            if self.active {
                self.act(now_ms, line, lines);
            }
        }
    }

    /// What the host of one line does in this step.
    fn act(&mut self, now: u64, line: &LineInfo, lines: &[LineInfo]) {
        let Some(mut persona) = self.personas.get(&line.id).cloned() else {
            return;
        };
        let host = &persona.host;
        if let Some(at) = persona.dtr_up_at
            && now >= at
        {
            host.set_dtr(true);
            persona.dtr_up_at = None;
        }
        persona.plan = match (line.state, persona.plan) {
            (LineState::Disabled | LineState::NotReady, _) => Plan::Nothing,

            // The modem has not said yet that it is off hook.
            (LineState::Idle, Plan::Dialling { since, .. }) if now < since + 5 * STEP_MS => {
                persona.plan
            }
            (LineState::Idle, _) => {
                let dial = std::mem::take(&mut self.first_call) || self.rng.one_in(DIAL_ONE_IN);
                match self.number_to_dial(line.id, lines).filter(|_| dial) {
                    Some(number) => {
                        host.write(format!("ATD{number}\r").as_bytes());
                        Plan::Dialling {
                            since: now,
                            give_up_at: now + self.rng.between(6_000, 30_000),
                        }
                    }
                    None => Plan::Nothing,
                }
            }

            (LineState::Ringing, Plan::Rung { answer_at }) => match answer_at {
                Some(at) if now >= at => {
                    host.write(b"ATA\r");
                    Plan::Answered
                }
                _ => persona.plan,
            },
            (LineState::Ringing, Plan::Answered) => Plan::Answered,
            // One call in six is not answered; the caller gives up.
            (LineState::Ringing, _) => Plan::Rung {
                answer_at: (!self.rng.one_in(6)).then(|| now + self.rng.between(1_500, 9_000)),
            },

            (LineState::Handshaking, Plan::Dialling { give_up_at, .. }) if now >= give_up_at => {
                // Any character from the host aborts the call.
                host.write(b"\r");
                Plan::Nothing
            }
            (LineState::Handshaking, plan) => plan,

            (LineState::Online, Plan::Talking { hang_at, how }) => {
                if now < hang_at {
                    if self.rng.one_in(2) {
                        let length = self.rng.between(20, 400) as usize;
                        let data: Vec<u8> =
                            (0..length).map(|_| self.rng.below(256) as u8).collect();
                        host.write(&data);
                    }
                    persona.plan
                } else {
                    match how {
                        Hang::Dtr => {
                            host.set_dtr(false);
                            persona.dtr_up_at = Some(now + 5 * STEP_MS);
                            Plan::Nothing
                        }
                        // The escape needs a second of silence on each
                        // side of it.
                        Hang::Escape => Plan::EscapeGuard { at: now + 1_300 },
                    }
                }
            }
            (LineState::Online, Plan::EscapeGuard { at }) if now >= at => {
                host.write(b"+++");
                Plan::Escaped { at: now + 1_300 }
            }
            (LineState::Online, Plan::Escaped { at }) if now >= at => {
                host.write(b"ATH\r");
                Plan::Nothing
            }
            (LineState::Online, plan @ (Plan::EscapeGuard { .. } | Plan::Escaped { .. })) => plan,
            (LineState::Online, _) => Plan::Talking {
                hang_at: now + self.rng.between(4_000, 60_000),
                how: if self.rng.one_in(2) {
                    Hang::Dtr
                } else {
                    Hang::Escape
                },
            },
        };
        self.personas.insert(line.id, persona);
    }

    /// What a host dials: mostly another line that is in service, now and
    /// then a line that is in a call, which is busy, and now and then a
    /// number that is not a line at all.
    fn number_to_dial(&mut self, from: u16, lines: &[LineInfo]) -> Option<String> {
        let others: Vec<&LineInfo> = lines
            .iter()
            .filter(|line| line.id != from && line.state != LineState::Disabled)
            .collect();
        let pick = self.rng.below(100);
        if pick < 8 || others.is_empty() {
            return Some(format!("5550{}", self.rng.between(100, 200)));
        }
        let in_a_call: Vec<&&LineInfo> = others.iter().filter(|line| line.call.is_some()).collect();
        if pick < 15 && !in_a_call.is_empty() {
            let line = in_a_call[self.rng.below(in_a_call.len() as u64) as usize];
            return Some(line.id.to_string());
        }
        let line = others[self.rng.below(others.len() as u64) as usize];
        Some(line.id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_gives_the_same_run_every_time_and_stays_in_range() {
        let mut one = Rng::new(7);
        let mut two = Rng::new(7);
        let mut seen = [false; 10];
        for _ in 0..1000 {
            let value = one.below(10);
            assert_eq!(value, two.below(10));
            seen[value as usize] = true;
            let between = one.between(5, 8);
            assert_eq!(between, two.between(5, 8));
            assert!((5..8).contains(&between));
        }
        assert!(seen.iter().all(|s| *s), "every value below 10 turns up");
        assert_ne!(Rng::new(1).below(1 << 30), Rng::new(2).below(1 << 30));
        let ones = (0..6000).filter(|_| one.one_in(6)).count();
        assert!((800..1200).contains(&ones), "{ones} in 6000");
    }
}
