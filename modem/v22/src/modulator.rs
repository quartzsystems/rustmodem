// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modulator: bits to a line signal.

use dspkit::Complex;
use dspkit::fir::Window;
use dspkit::level::dbm0_to_amplitude;
use dspkit::osc::Oscillator;
use dspkit::passband;
use dspkit::scramble::Scrambler;
use dspkit::shape::PulseShaper;
use g711::Law;

use crate::{
    Band, ELEMENT_1200, GuardTone, PULSE_BETA, PULSE_SYMBOLS, ROLLOFF, Rate, SCRAMBLER,
    SCRAMBLER_GUARD_ONES, SHAPER_PHASES, SHAPER_STEP, UNIT, point, quadrant_change,
};

/// How long the transmit filter delays the signal, in samples: half the
/// pulse, six symbols, 10 ms.
pub const TRANSMIT_DELAY_SAMPLES: u32 = (PULSE_SYMBOLS * SHAPER_PHASES / SHAPER_STEP / 2) as u32;

/// How much of the guard tone comes or goes in a sample: all of it in
/// 5 ms.
const GUARD_RAMP: f32 = 1.0 / 40.0;

/// The modulator of one channel.
///
/// Bits are scrambled (clause 5.1), taken two or four at a time, and made
/// into a symbol: the first two bits move the signal on by a number of
/// quadrants (Table 1), and at 2400 bit/s the last two choose the point
/// in the quadrant (Figure 2), where at 1200 bit/s it is always the same
/// one (clause 2.5.2.2). The symbols are shaped by the square root of a
/// raised cosine (clause 2.4) and put on the carrier.
///
/// A symbol is thirteen and a third samples, and the modulator's 600
/// symbols fill 8000 samples exactly: its symbol rate is its sample
/// clock's.
#[derive(Clone, Debug)]
pub struct Modulator {
    shaper: PulseShaper<SHAPER_PHASES, PULSE_SYMBOLS>,
    carrier: passband::Modulator,
    guard: Option<(Oscillator, f32)>,
    // How much of the guard tone is on, from none to all: it comes and
    // goes over 5 ms, as the data does through its filter.
    guard_on: f32,
    scrambler: Scrambler,
    scrambled: bool,
    rate: Rate,
    // The quadrant of the last symbol, 0 to 3.
    quadrant: u8,
    // What a symbol of the unit grid is multiplied by.
    scale: f32,
    silent: bool,
}

impl Modulator {
    /// A modulator for a channel, at a level in dBm0, which is the level
    /// of all it sends: the data and the guard tone together (V.22 clause
    /// 2.2). The guard tone is sent in the high channel only (clause
    /// 2.1). It begins at 1200 bit/s, scrambling.
    pub fn new(band: Band, level_dbm0: f32, guard: GuardTone, law: Law) -> Modulator {
        // The mean square of a sine wave at the level.
        let amplitude = dbm0_to_amplitude(level_dbm0, law);
        let total = amplitude * amplitude / 2.0;
        let tone = match band {
            Band::High => guard.tone(),
            Band::Low => None,
        };
        // The data's power and the tone's are the whole.
        let ratio = tone.map_or(0.0, |(_, ratio)| ratio);
        let data = total / (1.0 + ratio);
        Modulator {
            shaper: PulseShaper::new(ROLLOFF, SHAPER_STEP, Window::Kaiser(PULSE_BETA)),
            carrier: passband::Modulator::new(band.carrier_hz()),
            guard: tone.map(|(hz, ratio)| (Oscillator::new(hz), (2.0 * data * ratio).sqrt())),
            guard_on: 0.0,
            scrambler: Scrambler::with_guard(SCRAMBLER, SCRAMBLER_GUARD_ONES),
            scrambled: true,
            rate: Rate::B1200,
            quadrant: 0,
            // The shaper makes a signal of the symbols' power, and the
            // grid's unit makes that one.
            scale: data.sqrt() * UNIT,
            silent: false,
        }
    }

    /// The rate, from the next symbol.
    pub fn set_rate(&mut self, rate: Rate) {
        self.rate = rate;
    }

    /// The rate.
    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// Whether the bits are scrambled, from the next symbol. Unscrambled,
    /// they still pass through the scrambler's memory, so the scrambling
    /// that follows goes on from what the line carried.
    pub fn set_scrambled(&mut self, scrambled: bool) {
        self.scrambled = scrambled;
    }

    /// Whether the modulator sends nothing, from the next symbol: no
    /// data and no guard tone. What is in the transmit filter still goes
    /// out, so the signal ends and begins without a step.
    pub fn set_silent(&mut self, silent: bool) {
        self.silent = silent;
    }

    /// Whether the modulator is sending nothing.
    pub fn silent(&self) -> bool {
        self.silent
    }

    #[inline]
    fn line_bit(&mut self, bit: bool) -> bool {
        if self.scrambled {
            self.scrambler.bit(bit)
        } else {
            self.scrambler.bypass(bit)
        }
    }

    /// The next sample. When a symbol ends, `next_bit` is asked for the
    /// two or four bits of the next, the first to be sent first: `true`
    /// is binary 1.
    #[inline]
    pub fn next_sample(&mut self, mut next_bit: impl FnMut() -> bool) -> f32 {
        if self.shaper.wants_symbol() {
            let symbol = if self.silent {
                Complex::ZERO
            } else {
                let first = self.line_bit(next_bit());
                let second = self.line_bit(next_bit());
                let dibit = (u8::from(first) << 1) | u8::from(second);
                self.quadrant = (self.quadrant + quadrant_change(dibit)) & 3;
                let element = match self.rate {
                    Rate::B1200 => ELEMENT_1200,
                    Rate::B2400 => {
                        let third = self.line_bit(next_bit());
                        let fourth = self.line_bit(next_bit());
                        (u8::from(third) << 1) | u8::from(fourth)
                    }
                };
                point(self.quadrant, element).scale(self.scale)
            };
            self.shaper.push(symbol);
        }
        let mut sample = self.carrier.process(self.shaper.next_sample());
        if let Some((oscillator, amplitude)) = &mut self.guard {
            let target = if self.silent { 0.0 } else { 1.0 };
            self.guard_on += (target - self.guard_on).clamp(-GUARD_RAMP, GUARD_RAMP);
            sample += *amplitude * self.guard_on * oscillator.next_sine();
        }
        sample
    }
}
