// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Two LAPM machines joined by a line that can lose, repeat and delay
//! frames, on a clock the test moves.

#![allow(dead_code)]

use std::collections::VecDeque;

use v42::Role;
use v42::hdlc::MAX_FRAME;
use v42::lapm::{Config, Event, Lapm, State};

/// Numbers from a seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn chance(&mut self, probability: f64) -> bool {
        ((self.next() >> 11) as f64) < probability * (1u64 << 53) as f64
    }

    pub fn octets(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next() >> 24) as u8).collect()
    }
}

/// What a direction of the line does to the frames on it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Faults {
    /// The chance that a frame is lost: a frame whose check sequence
    /// fails is lost, so this is corruption too.
    pub loss: f64,
    /// The chance that a frame arrives twice.
    pub duplicate: f64,
    /// The chance that a frame is held back long enough for the next to
    /// pass it.
    pub delay: f64,
}

/// One direction of the line.
pub struct Wire {
    /// Frames on their way, and the sample at which each arrives.
    frames: VecDeque<(u64, Vec<u8>)>,
    pub faults: Faults,
    /// When set, every frame is lost.
    pub cut: bool,
    /// Every frame that was put on the line, lost or not.
    pub log: Vec<Vec<u8>>,
}

impl Wire {
    fn new() -> Wire {
        Wire {
            frames: VecDeque::new(),
            faults: Faults::default(),
            cut: false,
            log: Vec::new(),
        }
    }

    fn put(&mut self, frame: &[u8], arrives: u64, rng: &mut Rng) {
        self.log.push(frame.to_vec());
        if self.cut || rng.chance(self.faults.loss) {
            return;
        }
        let late = if rng.chance(self.faults.delay) {
            // Two longest frames' time at 2400 bit/s.
            8000
        } else {
            0
        };
        self.frames.push_back((arrives + late, frame.to_vec()));
        if rng.chance(self.faults.duplicate) {
            self.frames
                .push_back((arrives + late + 400, frame.to_vec()));
        }
    }

    fn take(&mut self, now: u64) -> Option<Vec<u8>> {
        let at = self
            .frames
            .iter()
            .position(|(arrives, _)| *arrives <= now)?;
        self.frames.remove(at).map(|(_, frame)| frame)
    }
}

/// One end.
pub struct End {
    pub lapm: Lapm,
    /// The octets the test has still to give it.
    pub to_send: VecDeque<u8>,
    /// What it was given.
    pub sent: Vec<u8>,
    /// What it delivered.
    pub received: Vec<u8>,
    /// Whether the test reads what is delivered.
    pub reading: bool,
    pub events: Vec<Event>,
    /// The sample at which its line is free for another frame.
    free_at: u64,
}

impl End {
    fn new(role: Role, config: Config) -> End {
        End {
            lapm: Lapm::new(role, config),
            to_send: VecDeque::new(),
            sent: Vec::new(),
            received: Vec::new(),
            reading: true,
            events: Vec::new(),
            free_at: 0,
        }
    }

    pub fn send(&mut self, octets: &[u8]) {
        self.to_send.extend(octets);
    }

    pub fn established(&self) -> bool {
        self.events
            .iter()
            .any(|e| matches!(e, Event::Established(_)))
    }

    pub fn released(&self) -> Option<v42::lapm::Release> {
        self.events.iter().find_map(|e| match e {
            Event::Released(why) => Some(*why),
            _ => None,
        })
    }
}

/// The line rate the pair is timed for.
pub const RATE: u32 = 2400;

/// The step of the clock, in samples: 5 ms.
const STEP: u32 = 40;

/// Two machines and the line between them. End A is the originator.
pub struct Pair {
    pub a: End,
    pub b: End,
    pub a_to_b: Wire,
    pub b_to_a: Wire,
    pub rng: Rng,
    pub now: u64,
}

impl Pair {
    pub fn new(seed: u64) -> Pair {
        Pair::with(seed, Config::for_rate(RATE), Config::for_rate(RATE))
    }

    pub fn with(seed: u64, a: Config, b: Config) -> Pair {
        Pair {
            a: End::new(Role::Originator, a),
            b: End::new(Role::Answerer, b),
            a_to_b: Wire::new(),
            b_to_a: Wire::new(),
            rng: Rng(seed | 1),
            now: 0,
        }
    }

    pub fn faults(&mut self, faults: Faults) {
        self.a_to_b.faults = faults;
        self.b_to_a.faults = faults;
    }

    fn serve(end: &mut End, incoming: &mut Wire, outgoing: &mut Wire, now: u64, rng: &mut Rng) {
        end.lapm.advance(STEP);
        while let Some(frame) = incoming.take(now) {
            end.lapm.receive(&frame);
        }
        // The host: gives what it has, and takes what has come.
        let (front, _) = end.to_send.as_slices();
        if !front.is_empty() {
            let taken = end.lapm.write(front);
            end.sent.extend(end.to_send.drain(..taken));
        }
        if end.reading {
            let mut buf = [0u8; 512];
            loop {
                let n = end.lapm.read(&mut buf);
                if n == 0 {
                    break;
                }
                end.received.extend_from_slice(&buf[..n]);
            }
        }
        while let Some(event) = end.lapm.poll_event() {
            end.events.push(event);
        }
        // The line: one frame at a time, each taking its length at the
        // line rate, with a flag and its check sequence.
        if now >= end.free_at {
            let mut frame = [0u8; MAX_FRAME];
            let len = end.lapm.poll_transmit(&mut frame);
            if len > 0 {
                let samples = (len as u64 + 3) * 8 * 8000 / u64::from(RATE);
                end.free_at = now + samples;
                // It arrives when its last bit does, and the line takes
                // 20 ms.
                outgoing.put(&frame[..len], end.free_at + 160, rng);
            }
        }
    }

    /// Runs for some milliseconds.
    pub fn run(&mut self, ms: u64) {
        let until = self.now + ms * 8;
        while self.now < until {
            self.now += u64::from(STEP);
            Pair::serve(
                &mut self.a,
                &mut self.b_to_a,
                &mut self.a_to_b,
                self.now,
                &mut self.rng,
            );
            Pair::serve(
                &mut self.b,
                &mut self.a_to_b,
                &mut self.b_to_a,
                self.now,
                &mut self.rng,
            );
        }
    }

    /// Runs until `done` says so or the time is up, and says which.
    pub fn run_until(&mut self, ms: u64, mut done: impl FnMut(&Pair) -> bool) -> bool {
        let until = self.now + ms * 8;
        while self.now < until {
            self.run(5);
            if done(self) {
                return true;
            }
        }
        false
    }

    /// Makes the connection, on a line with no faults.
    pub fn connect(&mut self) {
        self.a.lapm.establish();
        assert!(
            self.run_until(5000, |pair| pair.a.established() && pair.b.established()),
            "no connection: a {:?} b {:?}",
            self.a.events,
            self.b.events
        );
        assert_eq!(self.a.lapm.state(), State::Connected);
        assert_eq!(self.b.lapm.state(), State::Connected);
    }

    /// Whether everything each end was given has been delivered at the
    /// other.
    pub fn delivered(&self) -> bool {
        self.a.to_send.is_empty()
            && self.b.to_send.is_empty()
            && self.a.received.len() == self.b.sent.len()
            && self.b.received.len() == self.a.sent.len()
    }
}
