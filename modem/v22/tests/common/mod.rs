// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Two modems through the channel simulator, in data mode: each sends
//! random data to the other from the first sample, with no handshake.

#![allow(dead_code)]

use chansim::{Channel, ChannelConfig, Rng, Side};
use dspkit::Complex;
use dspkit::scramble::Scrambler;
use g711::Law;
use v22::{Band, Demodulator, GuardTone, Modulator, Rate, SCRAMBLER, SCRAMBLER_GUARD_ONES, Symbol};

/// One end: a modulator, a demodulator, and an account of the bits.
pub struct End {
    pub law: Law,
    pub modulator: Modulator,
    pub demodulator: Demodulator,
    rng: Rng,
    scrambler: Scrambler,
    /// The data bits given to the modulator.
    pub sent: Vec<bool>,
    /// The same bits as the line carries them: scrambled.
    pub sent_line: Vec<bool>,
    /// The data bits the demodulator gave.
    pub received: Vec<bool>,
    /// The line bits the demodulator gave.
    pub received_line: Vec<bool>,
    /// The points the demodulator decided from.
    pub points: Vec<Complex>,
    /// The sample at which each change of the carrier detector was seen.
    pub carrier_changes: Vec<(u64, bool)>,
    pub samples: u64,
    carrier: bool,
    time: f64,
}

impl End {
    fn new(band: Band, level_dbm0: f32, guard: GuardTone, law: Law, seed: u64) -> End {
        End {
            law,
            modulator: Modulator::new(band, level_dbm0, guard, law),
            demodulator: Demodulator::new(band.other(), law),
            rng: Rng::new(seed),
            scrambler: Scrambler::with_guard(SCRAMBLER, SCRAMBLER_GUARD_ONES),
            sent: Vec::new(),
            sent_line: Vec::new(),
            received: Vec::new(),
            received_line: Vec::new(),
            points: Vec::new(),
            carrier_changes: Vec::new(),
            samples: 0,
            carrier: false,
            time: 0.0,
        }
    }

    fn tick(&mut self, side: Side, channel: &mut Channel) {
        let mut rx = [0u8; 80];
        let mut tx = [0u8; 80];
        channel.receive(side, &mut rx);
        for &octet in &rx {
            let out = self.demodulator.process(self.law.decode(octet));
            if out.carrier != self.carrier {
                self.carrier = out.carrier;
                self.carrier_changes.push((self.samples, out.carrier));
            }
            if let Some(symbol) = out.symbol {
                self.take(symbol);
            }
            self.samples += 1;
        }
        for octet in &mut tx {
            let (rng, sent, sent_line, scrambler) = (
                &mut self.rng,
                &mut self.sent,
                &mut self.sent_line,
                &mut self.scrambler,
            );
            let sample = self.modulator.next_sample(|| {
                let bit = rng.next_u64() & 1 == 1;
                sent.push(bit);
                sent_line.push(scrambler.bit(bit));
                bit
            });
            *octet = self.law.encode(sample);
        }
        channel.send(side, &tx);
    }

    fn take(&mut self, symbol: Symbol) {
        self.received.extend(symbol.data_bits());
        self.received_line.extend(symbol.line_bits());
        self.points.push(symbol.point);
    }
}

/// The two ends and the channel between them. End A is the calling
/// modem: it sends in the low channel.
pub struct Rig {
    pub a: End,
    pub b: End,
    pub channel: Channel,
    a_tick: f64,
    pub ticks: u64,
}

impl Rig {
    pub fn new(config: &ChannelConfig, level_dbm0: f32) -> Rig {
        Rig::with_guard(config, level_dbm0, GuardTone::Off)
    }

    pub fn with_guard(config: &ChannelConfig, level_dbm0: f32, guard: GuardTone) -> Rig {
        let a_per_b = 1.0 + config.clock_offset_ppm * 1.0e-6;
        Rig {
            a: End::new(
                Band::Low,
                level_dbm0,
                guard,
                config.law_a,
                config.seed ^ 0xA,
            ),
            b: End::new(
                Band::High,
                level_dbm0,
                guard,
                config.law_b,
                config.seed ^ 0xB,
            ),
            channel: Channel::new(config),
            a_tick: 80.0 / a_per_b,
            ticks: 0,
        }
    }

    /// Runs for this many ticks of 10 ms of end B's clock.
    pub fn run(&mut self, ticks: u64) {
        let until = self.ticks + ticks;
        while self.ticks < until {
            if self.a.time <= self.b.time {
                self.a.tick(Side::A, &mut self.channel);
                self.a.time += self.a_tick;
            } else {
                self.b.tick(Side::B, &mut self.channel);
                self.b.time += 80.0;
                self.ticks += 1;
            }
        }
    }

    /// Both ends change rate, the modulators and the demodulators, at
    /// once: as the timers of a handshake have them do.
    pub fn set_rate(&mut self, rate: Rate) {
        for end in [&mut self.a, &mut self.b] {
            end.modulator.set_rate(rate);
            end.demodulator.set_rate(rate);
        }
    }

    /// Runs the ends at 1200 bit/s for `settle` ticks and, if the rate is
    /// 2400, changes to it and runs as long again; then forgets the bits
    /// so far.
    pub fn start(&mut self, rate: Rate, settle: u64) {
        self.run(settle);
        if rate == Rate::B2400 {
            self.set_rate(rate);
            self.run(settle);
        }
        for end in [&mut self.a, &mut self.b] {
            end.received.clear();
            end.received_line.clear();
            end.points.clear();
        }
    }
}

/// What a comparison of the bits received with the bits sent found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Compared {
    /// Bits compared.
    pub bits: usize,
    /// Of them, wrong.
    pub errors: usize,
}

impl Compared {
    pub fn rate(self) -> f64 {
        self.errors as f64 / self.bits.max(1) as f64
    }
}

/// Finds where in what was sent the received bits begin, and counts the
/// bits that differ from there on. The place is found by the stretch of
/// 256 bits, a hundred bits in, that fits best; `None` if nothing fits
/// better than chance.
pub fn compare(sent: &[bool], received: &[bool]) -> Option<Compared> {
    const SKIP: usize = 100;
    const WINDOW: usize = 256;
    if received.len() < SKIP + WINDOW + 8 {
        return None;
    }
    let probe = &received[SKIP..SKIP + WINDOW];
    let mut best: Option<(usize, usize)> = None;
    for at in 0..sent.len().saturating_sub(WINDOW) {
        let wrong = probe
            .iter()
            .zip(&sent[at..at + WINDOW])
            .filter(|(a, b)| a != b)
            .count();
        if best.is_none_or(|(_, least)| wrong < least) {
            best = Some((at, wrong));
            if wrong == 0 {
                break;
            }
        }
    }
    let (at, wrong) = best?;
    // A stretch that does not belong fits in half its bits.
    if wrong > WINDOW / 4 || at < SKIP {
        return None;
    }
    let start = at - SKIP;
    let bits = received.len().min(sent.len() - start);
    let errors = received[..bits]
        .iter()
        .zip(&sent[start..start + bits])
        .filter(|(a, b)| a != b)
        .count();
    Some(Compared { bits, errors })
}
