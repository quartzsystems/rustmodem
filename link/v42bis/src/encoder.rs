// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The encoder (V.42bis clause 7).

use crate::dictionary::{Dictionary, Matched, Matcher, NONE};
use crate::{
    ECM, EID, ESCAPE_STEP, ETM, FIRST_CODEWORD_BITS, FLUSH, Mode, N4, STEPUP, largest_codeword_bits,
};

/// The octets of output there must be room for before a character is
/// taken: the most one character can make. In compressed mode that is
/// three steps of the codeword size and a codeword, of twelve bits each
/// at the most; at a change of mode a codeword, ETM and the bits to the
/// end of the octet, or the escape character and ECM.
pub const ROOM: usize = 16;

/// How often the compressibility of the data is tested, in characters
/// (clause 7.8: "periodically").
const TEST_INTERVAL: u32 = 256;

/// How the encoder chooses its mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Policy {
    /// By the data compressibility test (clause 7.8): the mode that
    /// costs the fewer bits.
    #[default]
    Adaptive,
    /// Compressed mode always: the test always finds for it.
    Compressed,
    /// Transparent mode always.
    Transparent,
}

/// The encoder of one direction.
#[derive(Clone, Debug)]
pub struct Encoder {
    dictionary: Dictionary,
    matcher: Matcher,
    mode: Mode,
    // The escape character (clause 9.2).
    escape: u8,
    // C2, the codeword size, and C3, the threshold for changing it
    // (clause 7.4); N1, the largest size.
    size: u8,
    threshold: u32,
    largest: u8,
    // Bits of codewords not yet a whole octet, the first to go lowest.
    bits: u32,
    count: u8,
    // The compressibility test: what the characters since the last test
    // cost as they are, and what they cost as codewords.
    plain_bits: u32,
    coded_bits: u32,
    since_test: u32,
    policy: Policy,
}

impl Encoder {
    /// An encoder in its initial state (clause 7.2), for a dictionary of
    /// `codewords` codewords and strings of up to `string_length`.
    pub fn new(codewords: u16, string_length: u8) -> Encoder {
        let dictionary = Dictionary::new(codewords, string_length);
        Encoder {
            largest: largest_codeword_bits(dictionary.size()),
            dictionary,
            matcher: Matcher::new(),
            mode: Mode::Transparent,
            escape: 0,
            size: FIRST_CODEWORD_BITS,
            threshold: 2 * u32::from(N4),
            bits: 0,
            count: 0,
            plain_bits: 0,
            coded_bits: 0,
            since_test: 0,
            policy: Policy::Adaptive,
        }
    }

    /// Makes the encoder one for a dictionary of `codewords` codewords
    /// and strings of up to `string_length`, in its initial state: the
    /// parameters agreed when the connection was made (clause 5.2). The
    /// encoder is changed where it is, and its policy is kept.
    pub fn configure(&mut self, codewords: u16, string_length: u8) {
        self.dictionary.configure(codewords, string_length);
        self.largest = largest_codeword_bits(self.dictionary.size());
        self.reset();
    }

    /// How the mode is chosen, from the next character.
    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
    }

    /// The initial state again (clause 7.2): what C-INIT asks for.
    pub fn reset(&mut self) {
        self.dictionary.reset();
        self.matcher = Matcher::new();
        self.mode = Mode::Transparent;
        self.escape = 0;
        self.size = FIRST_CODEWORD_BITS;
        self.threshold = 2 * u32::from(N4);
        self.bits = 0;
        self.count = 0;
        self.plain_bits = 0;
        self.coded_bits = 0;
        self.since_test = 0;
    }

    /// The mode the encoder is in.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The dictionary, for a test to hold against the decoder's.
    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }

    /// Whether a flush would send anything: a string is matched and not
    /// sent, or bits of a codeword wait for the rest of their octet.
    pub fn pending(&self) -> bool {
        self.mode == Mode::Compressed
            && (self.count > 0 || (self.matcher.string != NONE && !self.matcher.ended))
    }

    /// Encodes characters into `out`, as many as there is room for: a
    /// character is taken only while [`ROOM`] octets are free. Gives how
    /// many characters were taken and how many octets were written.
    pub fn encode(&mut self, input: &[u8], out: &mut [u8]) -> (usize, usize) {
        let mut writer = Writer { out, len: 0 };
        let mut taken = 0;
        for &character in input {
            if writer.out.len() - writer.len < ROOM {
                break;
            }
            self.character(character, &mut writer);
            taken += 1;
        }
        (taken, writer.len)
    }

    /// One character (Figure I-3).
    #[inline]
    fn character(&mut self, character: u8, writer: &mut Writer) {
        let forced = match self.policy {
            Policy::Adaptive => false,
            Policy::Compressed => self.mode != Mode::Compressed,
            Policy::Transparent => self.mode != Mode::Transparent,
        };
        if self.since_test >= TEST_INTERVAL || forced {
            self.test(writer);
        }
        self.since_test += 1;
        let escape = character == self.escape;
        let matched = self.matcher.step(&mut self.dictionary, character);
        if let Matched::Complete(codeword) = matched {
            // What the string costs as a codeword, whichever mode it is
            // sent in.
            let needed = (32 - u32::from(codeword).leading_zeros()).max(u32::from(self.size));
            self.coded_bits += needed;
            if self.mode == Mode::Compressed {
                self.codeword(codeword, writer);
            }
        }
        self.plain_bits += if escape { 16 } else { 8 };
        if self.mode == Mode::Transparent {
            // Clause 7.5: as it is; clause 9.2 a): the escape character
            // is followed by EID.
            writer.octet(character);
            if escape {
                writer.octet(EID);
            }
        }
        if escape {
            // Clause 9.2 b): in both modes.
            self.escape = self.escape.wrapping_add(ESCAPE_STEP);
        }
    }

    /// Sends a codeword, stepping the codeword size up first if it needs
    /// it (clause 7.4).
    #[inline]
    fn codeword(&mut self, codeword: u16, writer: &mut Writer) {
        while u32::from(codeword) >= self.threshold && self.size < self.largest {
            self.put(STEPUP, writer);
            self.size += 1;
            self.threshold *= 2;
        }
        self.put(codeword, writer);
    }

    /// Packs a codeword of the current size: its least significant bit
    /// straight after the most significant of the one before (clause
    /// 7.5).
    #[inline]
    fn put(&mut self, codeword: u16, writer: &mut Writer) {
        self.bits |= u32::from(codeword) << self.count;
        self.count += self.size;
        while self.count >= 8 {
            writer.octet(self.bits as u8);
            self.bits >>= 8;
            self.count -= 8;
        }
    }

    /// 0 bits to the end of the octet (clause 7.5).
    #[inline]
    fn align(&mut self, writer: &mut Writer) {
        if self.count > 0 {
            writer.octet(self.bits as u8);
            self.bits = 0;
            self.count = 0;
        }
    }

    /// Ends the string being matched where it is, sending its codeword:
    /// what a flush and a change to transparent mode both begin with
    /// (clauses 7.8.2 a and 7.9 a).
    fn end_string(&mut self, writer: &mut Writer) {
        if self.matcher.string != NONE && !self.matcher.ended {
            self.codeword(self.matcher.string, writer);
            // Clauses 7.8.2 b) and 7.9 b): the dictionary is updated
            // with this string and the next character when that comes.
            self.matcher.ended = true;
        }
    }

    /// The data compressibility test (clause 7.8), and the change of
    /// mode it may call for. The test is of the characters since the
    /// last one: what they cost as they are against what they cost as
    /// codewords. The mode changes to compressed when codewords are the
    /// cheaper at all, and back when they cost an eighth more than the
    /// characters, so that data on the edge does not change it at every
    /// test.
    fn test(&mut self, writer: &mut Writer) {
        let (plain, coded) = (self.plain_bits, self.coded_bits);
        self.plain_bits = 0;
        self.coded_bits = 0;
        self.since_test = 0;
        let to_compressed = match self.policy {
            Policy::Adaptive => coded < plain,
            Policy::Compressed => true,
            Policy::Transparent => false,
        };
        let to_transparent = match self.policy {
            Policy::Adaptive => coded > plain + plain / 8,
            Policy::Compressed => false,
            Policy::Transparent => true,
        };
        match self.mode {
            Mode::Transparent if to_compressed => {
                // Clause 7.8.1: the string matched so far has gone as
                // characters; the escape character and ECM; compressed
                // mode, the first codeword from the next octet.
                if self.matcher.string != NONE {
                    self.matcher.ended = true;
                }
                writer.octet(self.escape);
                writer.octet(ECM);
                self.mode = Mode::Compressed;
            }
            Mode::Compressed if to_transparent => {
                // Clause 7.8.2.
                self.end_string(writer);
                self.put(ETM, writer);
                self.align(writer);
                self.mode = Mode::Transparent;
            }
            _ => {}
        }
    }

    /// Sends what is held back (clause 7.9): in compressed mode the
    /// codeword of the string matched so far, and the FLUSH codeword and
    /// 0 bits if that leaves an octet part filled. In transparent mode
    /// there is nothing held. `out` is to have [`ROOM`] octets; gives how
    /// many were written.
    pub fn flush(&mut self, out: &mut [u8]) -> usize {
        let mut writer = Writer { out, len: 0 };
        if self.mode == Mode::Compressed && writer.out.len() >= ROOM {
            self.end_string(&mut writer);
            if self.count > 0 {
                self.put(FLUSH, &mut writer);
                self.align(&mut writer);
            }
        }
        writer.len
    }
}

/// Octets into a buffer that the caller has seen to be long enough.
struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Writer<'_> {
    #[inline]
    fn octet(&mut self, octet: u8) {
        if let Some(place) = self.out.get_mut(self.len) {
            *place = octet;
            self.len += 1;
        }
    }
}
