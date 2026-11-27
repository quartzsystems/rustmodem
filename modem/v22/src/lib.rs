// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The 1200 and 2400 bit/s modems: ITU-T V.22 and V.22bis, and Bell 212A.
//!
//! All three are duplex by frequency division at 600 baud: the calling
//! modem sends on a carrier of 1200 Hz, the low channel, and the
//! answering modem on one of 2400 Hz, the high channel, and each hears
//! both on the one line. At 1200 bit/s a symbol is one of four phase
//! changes and carries two bits; at 2400 bit/s, which is V.22bis, it is
//! one of sixteen points and carries four. The bits on the line are
//! scrambled, and the line is synchronous: there is a symbol every 600th
//! of a second whether the host has anything to send or not.
//!
//! - [`Modulator`] makes the line signal from bits.
//! - [`Demodulator`] makes bits from the line signal: it filters out the
//!   far end's channel, finds the symbol timing and the carrier, equalises
//!   the line and decides each symbol.
//! - [`Modem`] is the two together with the handshake of clause 6.3, the
//!   fall back to 1200 bit/s and the retrain of clause 6.4: one end of a
//!   call, from the end of the answer sequence on.
//!
//! The editions are the Blue Book's (11/1988) of both Recommendations.
//! Clause numbers are V.22bis's unless V.22 is named. Where V.22bis at
//! 1200 bit/s and V.22 say the same thing, V.22bis is cited.
//!
//! Not built: the 600 bit/s mode of V.22 (its modes iii and iv), the
//! 1205 and 1223 bit/s line rates of its Alternative C, the fixed
//! compromise equaliser of clause 2.3, whose characteristic is left to
//! each Administration and which the adaptive equaliser stands in for,
//! the optional rate change of clause 6.6, and the test loops of clause
//! 7.

#![forbid(unsafe_code)]

mod demodulator;
mod detect;
mod handshake;
mod modulator;

pub use demodulator::{Demodulated, Demodulator, Symbol};
pub use handshake::{Allowed, BitIo, Event, Failure, Modem, Role, Settings, Signal, Standard};
pub use modulator::{Modulator, TRANSMIT_DELAY_SAMPLES};

use dspkit::Complex;
use dspkit::scramble::Polynomial;

/// The modulation rate in baud (clause 2.5.1: 600 baud, within 0.01%).
pub const SYMBOL_RATE: u32 = 600;

/// The carrier of the low channel, which the calling modem sends, in
/// hertz (clause 2.1: 1200 Hz, within 0.5 Hz).
pub const LOW_CARRIER_HZ: f32 = 1200.0;

/// The carrier of the high channel, which the answering modem sends, in
/// hertz (clause 2.1: 2400 Hz, within 1 Hz).
pub const HIGH_CARRIER_HZ: f32 = 2400.0;

/// The roll-off of the transmitted spectrum, which is the square root of
/// a raised cosine (clause 2.4: 75%).
pub const ROLLOFF: f64 = 0.75;

/// The scrambler's generating polynomial, `1 + x^-14 + x^-17` (clause
/// 5.1).
pub const SCRAMBLER: Polynomial = Polynomial::new(14, 17);

/// The ones in a row at the scrambler's output after which the next bit
/// into it is inverted (clause 5.1).
pub const SCRAMBLER_GUARD_ONES: u32 = 64;

/// The level at or above which the line signal detector is on, in dBm0
/// (clause 3.3: greater than -43 dBm, circuit 109 ON). The detector
/// comes on a little under this, so that a signal at this level is
/// received.
pub const CARRIER_ON_DBM0: f32 = -43.0;

/// The level below which the line signal detector goes off, in dBm0
/// (clause 3.3: less than -48 dBm, circuit 109 OFF). The 5 dB between
/// the two is more than the 2 dB of hysteresis the clause asks for.
pub const CARRIER_OFF_DBM0: f32 = -48.0;

/// How far the line may move the frequencies it carries, in hertz: what
/// the receiver must bear (clause 2.6).
pub const FREQUENCY_TOLERANCE_HZ: f32 = 7.0;

/// Samples in a symbol: thirteen and a third.
pub(crate) const SAMPLES_PER_SYMBOL: f64 = 8000.0 / SYMBOL_RATE as f64;

/// The phases of the transmit filter's table in a symbol, and how many
/// of them a sample moves: 40 over 3 is thirteen and a third.
pub(crate) const SHAPER_PHASES: usize = 40;
pub(crate) const SHAPER_STEP: usize = 3;

/// The length of the pulse in symbols, at both ends.
pub(crate) const PULSE_SYMBOLS: usize = 12;

/// The Kaiser window of the pulse at both ends. With twelve symbols it
/// puts everything 675 Hz and more from the carrier, which is where the
/// other channel begins, 77 dB down: at each end, so the modem's own
/// transmitter is that far down in what it sends and again in what its
/// receiver hears.
pub(crate) const PULSE_BETA: f32 = 4.0;

/// The size of a unit of the constellation's grid, for a mean power of
/// one: the sixteen points have a mean square of ten units.
pub(crate) const UNIT: f32 = 0.316_227_77;

/// Which of the two channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Band {
    /// The low channel, at 1200 Hz: the calling modem transmits in it
    /// (clause 6.1.1).
    Low,
    /// The high channel, at 2400 Hz: the answering modem transmits in it.
    High,
}

impl Band {
    /// The other channel: the one a modem that transmits in this one
    /// receives in.
    pub const fn other(self) -> Band {
        match self {
            Band::Low => Band::High,
            Band::High => Band::Low,
        }
    }

    /// The carrier, in hertz.
    pub const fn carrier_hz(self) -> f32 {
        match self {
            Band::Low => LOW_CARRIER_HZ,
            Band::High => HIGH_CARRIER_HZ,
        }
    }
}

/// The data signalling rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rate {
    /// 1200 bit/s: two bits a symbol (clause 2.5.2.2).
    B1200,
    /// 2400 bit/s: four bits a symbol (clause 2.5.2.1).
    B2400,
}

impl Rate {
    /// The rate in bit/s.
    pub const fn bits_per_second(self) -> u32 {
        match self {
            Rate::B1200 => 1200,
            Rate::B2400 => 2400,
        }
    }

    /// The bits a symbol carries.
    pub const fn bits_per_symbol(self) -> u8 {
        match self {
            Rate::B1200 => 2,
            Rate::B2400 => 4,
        }
    }
}

/// The guard tone an answering modem may send with its data (clause
/// 2.1), for the networks whose signalling a 2400 Hz carrier alone would
/// disturb.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GuardTone {
    /// None.
    #[default]
    Off,
    /// 1800 Hz, 6 dB below the data (clause 2.2).
    Hz1800,
    /// 550 Hz, 3 dB below the data (clause 2.2).
    Hz550,
}

impl GuardTone {
    /// The tone's frequency in hertz and its power as a part of the
    /// data's.
    pub(crate) const fn tone(self) -> Option<(f32, f32)> {
        match self {
            GuardTone::Off => None,
            // 6 dB below is a quarter of the power, and 3 dB a half.
            GuardTone::Hz1800 => Some((1800.0, 0.251_188_64)),
            GuardTone::Hz550 => Some((550.0, 0.501_187_2)),
        }
    }
}

/// How many quadrants on, anticlockwise, a dibit moves the signal
/// (Table 1/V.22 bis; the same as Table 1/V.22): 00 is 90 degrees, 01
/// none, 11 is 270 and 10 is 180. The dibit's first bit is its bit 1.
pub const fn quadrant_change(dibit: u8) -> u8 {
    match dibit & 3 {
        0b00 => 1,
        0b01 => 0,
        0b11 => 3,
        _ => 2,
    }
}

/// The dibit that a move of this many quadrants is.
pub const fn dibit_of_change(quadrants: u8) -> u8 {
    match quadrants & 3 {
        0 => 0b01,
        1 => 0b00,
        2 => 0b10,
        _ => 0b11,
    }
}

/// A point of the constellation, in units of its grid (Figure 2/V.22
/// bis). `quadrant` is 0 for phase quadrant 1, to 3 for quadrant 4, and
/// `element` is the last two bits of the quadbit, the first of them its
/// bit 1.
///
/// In the first quadrant 00 is at (1, 1), 01 at (3, 1), 10 at (1, 3) and
/// 11 at (3, 3); the other quadrants are the first turned.
pub fn point(quadrant: u8, element: u8) -> Complex {
    let first = match element & 3 {
        0b00 => Complex::new(1.0, 1.0),
        0b01 => Complex::new(3.0, 1.0),
        0b10 => Complex::new(1.0, 3.0),
        _ => Complex::new(3.0, 3.0),
    };
    first.rotate_quarters(quadrant)
}

/// The element of each quadrant sent at 1200 bit/s (clause 2.5.2.2: the
/// signalling elements corresponding to 01).
pub const ELEMENT_1200: u8 = 0b01;

/// The quadrant, 0 to 3, and the element of a point of the grid. Any
/// point is taken for the nearest of the sixteen.
pub fn quadrant_and_element(point: Complex) -> (u8, u8) {
    let quadrant = match (point.re >= 0.0, point.im >= 0.0) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    };
    // Turned back into the first quadrant.
    let first = point.rotate_quarters(4 - quadrant);
    let element = (u8::from(first.im >= 2.0) << 1) | u8::from(first.re >= 2.0);
    (quadrant, element)
}
