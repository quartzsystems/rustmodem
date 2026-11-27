// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! ITU-T V.42bis: data compression for modems that use V.42.
//!
//! The data is compressed a character at a time against a dictionary of
//! the strings seen so far, which the encoder at one end and the decoder
//! at the other each build in the same way from the same data, so that
//! a string in it can be sent as its number, a codeword. What does not
//! compress, data that is compressed or enciphered already, is sent as
//! it is, in transparent mode, and the encoder goes from one mode to the
//! other as the data changes.
//!
//! - [`Encoder`] takes the host's octets and gives the octets for the
//!   error-corrected connection.
//! - [`Decoder`] does the reverse, and checks every codeword it is given
//!   against the dictionary as it then is.
//!
//! One direction of a connection has one encoder and one decoder, at its
//! two ends; a connection compressed both ways has two of each.
//!
//! # What the far end sends
//!
//! The decoder's input is the far end's to choose. A codeword that names
//! no string, the control codewords and command codes that are not
//! there, a step in codeword size past what was agreed: each is the
//! error of clause 5.8 and is reported, and nothing that can be sent
//! makes the decoder read or write outside its arrays, loop without
//! end, or allocate.
//!
//! The edition is V.42bis (01/90).

#![forbid(unsafe_code)]

mod decoder;
mod dictionary;
mod encoder;

pub use decoder::{Decoder, Error, ROOM as DECODER_ROOM};
pub use dictionary::Dictionary;
pub use encoder::{Encoder, Policy, ROOM as ENCODER_ROOM};

/// N3: the bits of a character (clause 10).
pub const N3: u8 = 8;

/// N4: the characters of the alphabet (clause 10).
pub const N4: u16 = 256;

/// N6: the control codewords (clause 10).
pub const N6: u16 = 3;

/// N5: the first entry that holds a string of more than one character
/// (clause 10: N4 + N6).
pub const N5: u16 = N4 + N6;

/// The control codewords, used in compressed mode (Table 2): enter
/// transparent mode, flush, and step up the codeword size.
pub const ETM: u16 = 0;
pub const FLUSH: u16 = 1;
pub const STEPUP: u16 = 2;

/// The command codes, used in transparent mode after the escape
/// character (Table 2): enter compressed mode, escape character in data,
/// and force reinitialisation.
pub const ECM: u8 = 0;
pub const EID: u8 = 1;
pub const RESET: u8 = 2;

/// What is added to the escape character each time it appears in the
/// data, modulo 256 (clause 9.2 b).
pub const ESCAPE_STEP: u8 = 51;

/// The fewest codewords there may be, which is the default of P1
/// (clause 5.1), and the most this modem has.
pub const MIN_CODEWORDS: u16 = 512;
pub const MAX_CODEWORDS: usize = 4096;

/// The range of the longest string, P2 (clause 5.1): its lower end is
/// the default.
pub const MIN_STRING: u8 = 6;
pub const MAX_STRING: u8 = 250;

/// The codeword size at the start: N3 + 1 (clause 7.2 b).
pub(crate) const FIRST_CODEWORD_BITS: u8 = N3 + 1;

/// N1: the largest codeword size for a number of codewords, in bits: the
/// fewest bits that number them all.
pub(crate) const fn largest_codeword_bits(codewords: u16) -> u8 {
    let mut bits = FIRST_CODEWORD_BITS;
    while (1u32 << bits) < codewords as u32 {
        bits += 1;
    }
    bits
}

/// The mode of an encoder or decoder (clause 2.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// The data goes as it is, with an escape character for the command
    /// codes.
    Transparent,
    /// The data goes as codewords.
    Compressed,
}
