// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The 300 bit/s modems: ITU-T V.21 and Bell 103.
//!
//! Both are binary frequency-shift keying at 300 baud, duplex by frequency
//! division: the calling modem sends in one band and the answering modem
//! in another, and each hears both on the one line. They differ in their
//! four frequencies and in nothing else that matters here.
//!
//! - [`Modulator`] makes the line signal from bits, with a continuous
//!   phase.
//! - [`Demodulator`] filters out the band of the far end, compares the
//!   energy at the two frequencies over a bit, and says whether there is a
//!   carrier.
//! - [`StartStopTransmitter`] and [`StartStopReceiver`] put characters
//!   into start-stop frames and take them out: V.21 carries the host's
//!   asynchronous characters as they are (V.21 clause 4).
//! - [`Receiver`] is the demodulator and the start-stop receiver together,
//!   which is what a modem engine uses.
//!
//! The edition of V.21 is the Blue Book's (11/1988). There is no Bell
//! publication on the desk: the Bell 103 frequencies are the well-known
//! ones, as the phase 3 handoff lists them.
//!
//! A character is ten bits on the line: a start bit, eight units and one
//! stop bit. The units are eight data bits, or seven and a parity bit:
//! the three formats of [`Format`], which is `link/v14`'s.

#![forbid(unsafe_code)]

mod demodulator;
mod framing;
mod modulator;

pub use demodulator::{Demodulated, Demodulator};
pub use framing::{Framed, StartStopReceiver, StartStopTransmitter, TX_CAPACITY};
pub use modulator::{Modulator, TRANSMIT_DELAY_SAMPLES};
pub use v14::{Format, Parity};

use g711::Law;

/// The modulation rate in baud, which with two frequencies is the data
/// signalling rate in bit/s (V.21 clause 2).
pub const BIT_RATE: u32 = 300;

/// The level at or above which the line signal detector comes on, in
/// dBm0 (V.21 clause 8.3: greater than -43 dBm, circuit 109 ON).
pub const CARRIER_ON_DBM0: f32 = -43.0;

/// The level below which the line signal detector goes off, in dBm0
/// (V.21 clause 8.3: less than -48 dBm, circuit 109 OFF). The 5 dB between
/// the two is more than the 2 dB of hysteresis the clause asks for.
pub const CARRIER_OFF_DBM0: f32 = -48.0;

/// How long the carrier must be there before the detector comes on, in
/// milliseconds, unless the caller says otherwise. V.21 clause 8.2.2,
/// Table 2, gives 300 to 700 ms for circuit 109 on the switched network.
pub const CARRIER_ON_MS: u32 = 500;

/// How long the level must be below the threshold before the detector
/// goes off, in milliseconds. V.21 clause 8.2.2, Table 2, gives 20 to
/// 80 ms from the carrier's going to the detector's; the band filter and
/// the level meter take 10 to 35 ms of that to fall, the more the louder
/// the carrier was, and this is the rest.
pub const CARRIER_OFF_MS: u32 = 30;

/// How far the line may move the frequencies it carries, with the
/// tolerance of the far modulator, in hertz: what a demodulator must bear
/// (V.21 clause 3: 6 Hz at the modulator and 6 Hz on the line).
pub const FREQUENCY_TOLERANCE_HZ: f32 = 12.0;

/// The taps of the band filter each modulator and demodulator has.
pub(crate) const BAND_TAPS: usize = 129;

/// How far beyond a band's two frequencies its filter is at half
/// amplitude, in hertz. The two frequencies are 200 Hz apart, and the
/// keying at 300 baud spreads each by some 150 Hz more.
pub(crate) const BAND_MARGIN_HZ: f32 = 170.0;

/// The Kaiser window of the band filter: 60 dB in the stop band.
pub(crate) const BAND_BETA: f32 = 5.65;

/// Which of the two 300 bit/s modems.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Standard {
    /// ITU-T V.21.
    V21,
    /// Bell 103.
    Bell103,
}

impl Standard {
    /// The name a connection on this modem is reported by.
    pub const fn carrier_name(self) -> &'static str {
        match self {
            Standard::V21 => "V.21",
            Standard::Bell103 => "BELL103",
        }
    }
}

/// Which of the two bands of a duplex connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Band {
    /// The band the calling modem transmits in: V.21's channel No. 1
    /// (V.21 clause 7 a), and Bell 103's originate band.
    Caller,
    /// The band the answering modem transmits in: V.21's channel No. 2,
    /// and Bell 103's answer band.
    Answerer,
}

impl Band {
    /// The other band: the one a modem that transmits in this one
    /// receives in.
    pub const fn other(self) -> Band {
        match self {
            Band::Caller => Band::Answerer,
            Band::Answerer => Band::Caller,
        }
    }
}

/// The two frequencies of a band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tones {
    /// The frequency of binary 1, mark, which is also the idle condition,
    /// in hertz.
    pub mark_hz: f32,
    /// The frequency of binary 0, space, in hertz.
    pub space_hz: f32,
}

impl Tones {
    /// The frequencies of a band of a modem.
    ///
    /// V.21 clause 3: the mean frequency of channel No. 1 is 1080 Hz and
    /// of channel No. 2 1750 Hz, the deviation is 100 Hz either way, and
    /// in each channel the higher frequency is binary 0. Its footnote
    /// gives them: 1180 and 980 Hz, 1850 and 1650 Hz.
    ///
    /// Bell 103 is the other way up: the higher frequency is mark.
    pub const fn of(standard: Standard, band: Band) -> Tones {
        let (mark_hz, space_hz) = match (standard, band) {
            (Standard::V21, Band::Caller) => (980.0, 1180.0),
            (Standard::V21, Band::Answerer) => (1650.0, 1850.0),
            (Standard::Bell103, Band::Caller) => (1270.0, 1070.0),
            (Standard::Bell103, Band::Answerer) => (2225.0, 2025.0),
        };
        Tones { mark_hz, space_hz }
    }

    /// The frequency halfway between the two.
    pub const fn centre_hz(self) -> f32 {
        (self.mark_hz + self.space_hz) / 2.0
    }
}

/// The demodulator and the start-stop receiver of one band: bits and
/// characters from samples.
#[derive(Clone, Debug)]
pub struct Receiver {
    demodulator: Demodulator,
    framer: StartStopReceiver,
    // The mean size of the demodulator's output where the bits are
    // sampled.
    eye: f32,
}

/// What a [`Receiver`] has after a sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Received {
    /// The demodulator's output: positive for mark, negative for space,
    /// from -1 to 1.
    pub soft: f32,
    /// Whether the carrier is detected.
    pub carrier: bool,
    /// What the start-stop receiver made of the sample.
    pub framed: Framed,
}

impl Receiver {
    /// A receiver for a band, with a line signal detector that takes
    /// `carrier_on_ms` to come on.
    pub fn new(standard: Standard, band: Band, carrier_on_ms: u32, law: Law) -> Receiver {
        Receiver {
            demodulator: Demodulator::new(Tones::of(standard, band), carrier_on_ms, law),
            framer: StartStopReceiver::new(),
            eye: 0.0,
        }
    }

    /// Takes one sample.
    #[inline]
    pub fn process(&mut self, sample: f32) -> Received {
        let demodulated = self.demodulator.process(sample);
        let framed = if demodulated.carrier {
            self.framer.process(demodulated.soft > 0.0)
        } else {
            // Without a carrier the received data is held at mark, as
            // circuit 104 is while circuit 109 is off.
            self.framer.reset();
            Framed::default()
        };
        if framed.bit.is_some() {
            self.eye += (demodulated.soft.abs() - self.eye) / 32.0;
        }
        Received {
            soft: demodulated.soft,
            carrier: demodulated.carrier,
            framed,
        }
    }

    /// The demodulator, for its measurements.
    pub fn demodulator(&self) -> &Demodulator {
        &self.demodulator
    }

    /// How open the eye is where the bits are sampled: the mean size of
    /// the demodulator's output there, over the last 32 bits or so. On a
    /// clean line it is about 0.7, which is as far as the output goes: the
    /// two frequencies are closer than a bit can tell wholly apart. Below
    /// about 0.3 errors are near.
    pub fn eye_opening(&self) -> f32 {
        self.eye
    }

    /// The characters lost to a wrong stop bit since the receiver was made
    /// or reset.
    pub fn framing_errors(&self) -> u32 {
        self.framer.framing_errors()
    }

    /// The characters whose parity was wrong since the receiver was made
    /// or reset.
    pub fn parity_errors(&self) -> u32 {
        self.framer.parity_errors()
    }

    /// The character format, from the next character.
    pub fn set_format(&mut self, format: Format) {
        self.framer.set_format(format);
    }

    /// Forgets the signal: no carrier, no character in progress. The
    /// counts of errors are cleared too; the character format is kept.
    pub fn reset(&mut self) {
        let format = self.framer.format();
        self.demodulator.reset();
        self.framer = StartStopReceiver::new();
        self.framer.set_format(format);
        self.eye = 0.0;
    }
}
