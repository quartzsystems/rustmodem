// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The detectors of the handshake's unscrambled signals.
//!
//! A handshake begins with signals that are not scrambled, and so are a
//! few spectral lines where data is spread over the whole channel:
//!
//! - **Unscrambled binary 1** is the dibit 11 for ever, a quarter turn
//!   back every symbol (Table 1): one line 150 Hz under the carrier.
//! - **S1**, the unscrambled double dibit 00 and 11 (clause 6.3.1.1.1 b),
//!   is a quarter turn on and a quarter turn back in turn: a line at the
//!   carrier and one 300 Hz to each side of it. After the matched filter
//!   the line at the carrier has two thirds of the power and the others a
//!   sixth each.
//!
//! Both are looked for in the spectrum, by correlating the output of the
//! matched filter against each line over a short window, and comparing
//! what is found with all the power in the window. That needs neither the
//! symbol timing nor the carrier's phase, neither of which the receiver
//! has when these signals come: an unscrambled 1 says nothing about
//! where the symbols are.
//!
//! The answering tone of the modems of the Bell System, 2225 Hz, is
//! 175 Hz under the carrier of the high channel, 25 Hz from where an
//! unscrambled 1 is (the note to clause 6.3.1.2.2). The window of the
//! detector of unscrambled 1 is 10 ms, short enough to take either.

use dspkit::Complex;
use dspkit::osc::phase_step;

/// The window of the detector of unscrambled 1, in samples: 10 ms.
pub(crate) const ONES_WINDOW: usize = 80;

/// The window of the detector of S1, in samples: 20 ms, twelve symbols.
pub(crate) const S1_WINDOW: usize = 160;

/// The part of the power in the window that must be in the line of an
/// unscrambled 1. Data leaves a tenth there, the answer tone of V.25 a
/// twentieth, and the Bell answering tone four fifths.
const ONES_PART: f32 = 0.6;

/// The parts of the power in the window that must be in the line of S1
/// at the carrier, in each of the lines beside it, and in the three
/// together. S1 has 0.67, 0.17 and all of it.
const S1_CENTRE_PART: f32 = 0.4;
const S1_SIDE_PART: f32 = 0.08;
const S1_WHOLE_PART: f32 = 0.8;

/// A sliding correlation of the last `N` samples against one frequency.
#[derive(Clone, Debug)]
struct Line<const N: usize> {
    // The samples of the window, each turned back by the frequency.
    ring: [Complex; N],
    at: usize,
    sum: Complex,
    phase: u32,
    step: u32,
}

impl<const N: usize> Line<N> {
    fn new(offset_hz: f32) -> Line<N> {
        Line {
            ring: [Complex::ZERO; N],
            at: 0,
            sum: Complex::ZERO,
            phase: 0,
            step: phase_step(offset_hz),
        }
    }

    /// Takes a sample and gives the power of the line: of a sine wave at
    /// the frequency that fills the window, its own power.
    #[inline]
    fn process(&mut self, sample: Complex) -> f32 {
        let turned = sample.rotate(self.phase.wrapping_neg());
        self.phase = self.phase.wrapping_add(self.step);
        self.sum += turned - self.ring[self.at];
        self.ring[self.at] = turned;
        self.at += 1;
        if self.at == N {
            self.at = 0;
            // The sum is made afresh once a window, so that what rounding
            // leaves in it cannot grow.
            let mut sum = Complex::ZERO;
            for value in &self.ring {
                sum += *value;
            }
            self.sum = sum;
        }
        self.sum.norm_sqr() / (N * N) as f32
    }
}

/// The mean power of the last `N` samples.
#[derive(Clone, Debug)]
struct Power<const N: usize> {
    ring: [f32; N],
    at: usize,
    sum: f32,
}

impl<const N: usize> Power<N> {
    fn new() -> Power<N> {
        Power {
            ring: [0.0; N],
            at: 0,
            sum: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, sample: Complex) -> f32 {
        let power = sample.norm_sqr();
        self.sum += power - self.ring[self.at];
        self.ring[self.at] = power;
        self.at += 1;
        if self.at == N {
            self.at = 0;
            self.sum = self.ring.iter().sum();
        }
        (self.sum / N as f32).max(0.0)
    }
}

/// What the detectors say of the signal at a sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Detected {
    /// Unscrambled binary 1, or the Bell answering tone.
    pub ones: bool,
    /// S1.
    pub s1: bool,
}

/// The detectors of unscrambled 1 and of S1 in one channel.
#[derive(Clone, Debug)]
pub(crate) struct Detectors {
    ones: Line<ONES_WINDOW>,
    ones_power: Power<ONES_WINDOW>,
    centre: Line<S1_WINDOW>,
    above: Line<S1_WINDOW>,
    below: Line<S1_WINDOW>,
    s1_power: Power<S1_WINDOW>,
}

impl Detectors {
    pub(crate) fn new() -> Detectors {
        Detectors {
            // A quarter turn back a symbol at 600 baud.
            ones: Line::new(-150.0),
            ones_power: Power::new(),
            // Half a turn a symbol either way.
            centre: Line::new(0.0),
            above: Line::new(300.0),
            below: Line::new(-300.0),
            s1_power: Power::new(),
        }
    }

    /// Takes a sample of the matched filter's output. `floor` is the
    /// least power in a window that is taken for a signal at all.
    #[inline]
    pub(crate) fn process(&mut self, baseband: Complex, floor: f32) -> Detected {
        let baseband = if baseband.is_finite() {
            baseband
        } else {
            Complex::ZERO
        };
        let ones = self.ones.process(baseband);
        let ones_all = self.ones_power.process(baseband);
        let centre = self.centre.process(baseband);
        let above = self.above.process(baseband);
        let below = self.below.process(baseband);
        let s1_all = self.s1_power.process(baseband);
        Detected {
            ones: ones_all > floor && ones > ONES_PART * ones_all,
            s1: s1_all > floor
                && centre > S1_CENTRE_PART * s1_all
                && above > S1_SIDE_PART * s1_all
                && below > S1_SIDE_PART * s1_all
                && centre + above + below > S1_WHOLE_PART * s1_all,
        }
    }
}
