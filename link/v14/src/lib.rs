// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! ITU-T V.14: start-stop characters over a synchronous channel.
//!
//! The modems of V.22 and after carry a synchronous stream: a bit at
//! every bit time, whether there is anything to say or not. A host's
//! characters are asynchronous, each with a start element before it and a
//! stop element after, and come when they come. V.14 is how the one is
//! put into the other when no error-correcting protocol is doing it:
//!
//! - [`Transmitter`] puts each character into the stream as it is, start
//!   element, data and stop element, and fills the time between
//!   characters with more of the stop element. When the host has more to
//!   send than the line takes, it may leave out a stop element now and
//!   then, which carries 1% more (clause 7.1).
//! - [`Receiver`] finds the start elements in the stream, gives the
//!   characters, and takes a start element where a stop element should
//!   have been as the stop element left out (clause 7.2).
//! - A break is a run of the start polarity longer than any character
//!   (clause 7.3).
//!
//! [`Format`] is the character format: the three a host is likely to
//! ask for, 8N1, 7E1 and 7O1, each ten bits long. It is here and not in
//! the receiver alone because the 300 bit/s modems, which are
//! asynchronous on the line and use none of the conversion, frame their
//! characters the same way.
//!
//! The edition is V.14 (03/93) with its Corrigendum 1 (09/98).
//!
//! Not built: the extended signalling rate range of clause 3 b), in which
//! a stop element may be left out of one character in four; the formats
//! of nine data units and of six of clause 4 c) and d).

#![forbid(unsafe_code)]

mod receiver;
mod transmitter;

pub use receiver::{Received, Receiver};
pub use transmitter::{TX_CAPACITY, Transmitter};

/// The parity of a character format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Parity {
    /// No parity unit.
    None,
    /// A parity unit that makes the number of ones even.
    Even,
    /// A parity unit that makes the number of ones odd.
    Odd,
}

/// A start-stop character format: a start element, the data units, a
/// parity unit or none, and a stop element of one unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Format {
    data_bits: u8,
    parity: Parity,
}

impl Format {
    /// Eight data units and no parity: a character of ten bits (clause
    /// 4 b).
    pub const N81: Format = Format {
        data_bits: 8,
        parity: Parity::None,
    };

    /// Seven data units and even parity: ten bits, the eighth data unit
    /// of clause 4 b) being the parity.
    pub const E71: Format = Format {
        data_bits: 7,
        parity: Parity::Even,
    };

    /// Seven data units and odd parity.
    pub const O71: Format = Format {
        data_bits: 7,
        parity: Parity::Odd,
    };

    /// The data units of a character.
    pub const fn data_bits(self) -> u8 {
        self.data_bits
    }

    /// The parity.
    pub const fn parity(self) -> Parity {
        self.parity
    }

    /// The units between the start element and the stop element: the
    /// data and the parity.
    pub const fn payload_bits(self) -> u8 {
        self.data_bits
            + match self.parity {
                Parity::None => 0,
                Parity::Even | Parity::Odd => 1,
            }
    }

    /// The bits of a whole character with its start and stop elements:
    /// the M of clause 7.3.
    pub const fn character_bits(self) -> u8 {
        self.payload_bits() + 2
    }

    /// The bits of a break as it is sent, and the fewest that are taken
    /// for one: 2M + 3 (clause 7.3).
    pub const fn break_bits(self) -> u16 {
        2 * self.character_bits() as u16 + 3
    }

    /// The character's mask: what of an octet the format carries.
    pub const fn mask(self) -> u8 {
        if self.data_bits >= 8 {
            0xFF
        } else {
            (1 << self.data_bits) - 1
        }
    }

    /// The units between the start and stop elements for a character,
    /// the first to be sent in bit 0: the data, least significant bit
    /// first, and after it the parity (Recommendation V.4, which clause 4
    /// follows). What of the character the format does not carry is left
    /// out.
    pub const fn payload(self, character: u8) -> u16 {
        let data = (character & self.mask()) as u16;
        let ones = data.count_ones() & 1;
        let parity = match self.parity {
            Parity::None => return data,
            Parity::Even => ones,
            Parity::Odd => ones ^ 1,
        };
        data | ((parity as u16) << self.data_bits)
    }

    /// The character of the units received between a start and a stop
    /// element, and whether its parity was right.
    pub const fn character(self, payload: u16) -> (u8, bool) {
        let character = (payload as u8) & self.mask();
        (
            character,
            self.payload(character) == payload & ((1 << self.payload_bits()) - 1),
        )
    }
}

impl Default for Format {
    fn default() -> Self {
        Format::N81
    }
}
