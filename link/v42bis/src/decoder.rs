// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The decoder (V.42bis clause 8).
//!
//! This is the half that reads what the far end sent. It believes
//! nothing: every codeword is looked up in the dictionary as it is at
//! that moment, and one that names no string there is an error, as are
//! the other conditions of clause 5.8. An error is final: the decoder
//! gives nothing more until it is reset, since from the first wrong
//! codeword on its dictionary is no longer the encoder's.

use crate::dictionary::{Dictionary, Matcher, NONE};
use crate::{
    ECM, EID, ESCAPE_STEP, ETM, FIRST_CODEWORD_BITS, FLUSH, MAX_STRING, Mode, N6, RESET, STEPUP,
    largest_codeword_bits,
};

/// The octets of output there must be room for before an octet is
/// taken: a codeword is one string, of 250 characters at the most.
pub const ROOM: usize = MAX_STRING as usize;

/// What the far end sent that it may not (clause 5.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Error {
    /// A STEPUP codeword that would make the codeword size more than
    /// was agreed (clause 5.8 a).
    CodewordSize,
    /// A codeword equal to C1, the empty entry that is next to be filled
    /// (clause 5.8 b), or one of an entry that holds no string (clause
    /// 5.8 c).
    EmptyEntry(u16),
    /// A reserved command code after the escape character (clause 5.8
    /// d).
    Command(u8),
}

/// The decoder of one direction.
#[derive(Clone, Debug)]
pub struct Decoder {
    dictionary: Dictionary,
    // The string matching of transparent mode, and in both modes the
    // entry made last.
    matcher: Matcher,
    mode: Mode,
    escape: u8,
    // In transparent mode: whether the last octet was the escape
    // character, and the next is a command code.
    escaped: bool,
    // C2 and N1.
    size: u8,
    largest: u8,
    // Bits received that are not yet a whole codeword, the first lowest.
    bits: u32,
    count: u8,
    // In compressed mode: the entry of the last codeword's string, which
    // the first character of the next one lengthens (clause 8).
    previous: u16,
    failed: Option<Error>,
}

impl Decoder {
    /// A decoder in its initial state, for a dictionary of `codewords`
    /// codewords and strings of up to `string_length`.
    pub fn new(codewords: u16, string_length: u8) -> Decoder {
        let dictionary = Dictionary::new(codewords, string_length);
        Decoder {
            largest: largest_codeword_bits(dictionary.size()),
            dictionary,
            matcher: Matcher::new(),
            mode: Mode::Transparent,
            escape: 0,
            escaped: false,
            size: FIRST_CODEWORD_BITS,
            bits: 0,
            count: 0,
            previous: NONE,
            failed: None,
        }
    }

    /// Makes the decoder one for a dictionary of `codewords` codewords
    /// and strings of up to `string_length`, in its initial state: the
    /// parameters agreed when the connection was made (clause 5.2). The
    /// decoder is changed where it is.
    pub fn configure(&mut self, codewords: u16, string_length: u8) {
        self.dictionary.configure(codewords, string_length);
        self.largest = largest_codeword_bits(self.dictionary.size());
        self.reset();
    }

    /// The initial state again: what C-INIT asks for, and what the RESET
    /// command code does (clauses 8 and 7.2).
    pub fn reset(&mut self) {
        self.dictionary.reset();
        self.matcher = Matcher::new();
        self.mode = Mode::Transparent;
        self.escape = 0;
        self.escaped = false;
        self.size = FIRST_CODEWORD_BITS;
        self.bits = 0;
        self.count = 0;
        self.previous = NONE;
        self.failed = None;
    }

    /// The mode the decoder is in.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The dictionary, for a test to hold against the encoder's.
    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }

    /// The error that stopped the decoder, if one did.
    pub fn error(&self) -> Option<Error> {
        self.failed
    }

    /// Decodes octets into `out`, as many as there is room for: an octet
    /// is taken only while [`ROOM`] octets are free. Gives how many
    /// octets were taken and how many characters were written, or the
    /// error of clause 5.8, after which nothing is taken until the
    /// decoder is reset.
    pub fn decode(&mut self, input: &[u8], out: &mut [u8]) -> Result<(usize, usize), Error> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let mut written = 0;
        let mut taken = 0;
        for &octet in input {
            if out.len() - written < ROOM {
                break;
            }
            taken += 1;
            let result = match self.mode {
                Mode::Transparent => self.transparent(octet, out, &mut written),
                Mode::Compressed => self.compressed(octet, out, &mut written),
            };
            if let Err(error) = result {
                self.failed = Some(error);
                return Err(error);
            }
        }
        Ok((taken, written))
    }

    /// A character of the data, in transparent mode: given, and the
    /// string matching and the dictionary kept as the encoder keeps
    /// them (clause 8).
    #[inline]
    fn character(&mut self, character: u8, out: &mut [u8], written: &mut usize) {
        out[*written] = character;
        *written += 1;
        self.matcher.step(&mut self.dictionary, character);
        if character == self.escape {
            // Clause 9.2 b).
            self.escape = self.escape.wrapping_add(ESCAPE_STEP);
        }
    }

    /// An octet in transparent mode.
    #[inline]
    fn transparent(&mut self, octet: u8, out: &mut [u8], written: &mut usize) -> Result<(), Error> {
        if std::mem::take(&mut self.escaped) {
            return match octet {
                ECM => {
                    // Clause 7.8.1: the string matched so far went as
                    // characters, and the first codeword's first
                    // character lengthens it.
                    self.previous = self.matcher.string;
                    self.mode = Mode::Compressed;
                    self.bits = 0;
                    self.count = 0;
                    Ok(())
                }
                EID => {
                    // Clause 9.2 a): the escape character itself.
                    self.character(self.escape, out, written);
                    Ok(())
                }
                RESET => {
                    // Clause 7.8.3.
                    self.reset();
                    Ok(())
                }
                other => Err(Error::Command(other)),
            };
        }
        if octet == self.escape {
            self.escaped = true;
        } else {
            self.character(octet, out, written);
        }
        Ok(())
    }

    /// An octet in compressed mode: at most one codeword is completed by
    /// it, since a codeword is nine bits or more.
    #[inline]
    fn compressed(&mut self, octet: u8, out: &mut [u8], written: &mut usize) -> Result<(), Error> {
        self.bits |= u32::from(octet) << self.count;
        self.count += 8;
        if self.count < self.size {
            return Ok(());
        }
        let codeword = (self.bits & ((1u32 << self.size) - 1)) as u16;
        self.bits >>= self.size;
        self.count -= self.size;
        match codeword {
            ETM => {
                // Clause 7.8.2: what is left of the octet is 0 bits. The
                // last string's entry is where the matching stands, and
                // the first character lengthens it.
                self.bits = 0;
                self.count = 0;
                self.mode = Mode::Transparent;
                self.matcher.string = self.previous;
                self.matcher.ended = self.previous != NONE;
            }
            FLUSH => {
                // Clause 7.9 c): 0 bits to the end of the octet.
                self.bits = 0;
                self.count = 0;
            }
            STEPUP => {
                // Clause 5.8 a).
                if self.size >= self.largest {
                    return Err(Error::CodewordSize);
                }
                self.size += 1;
            }
            _ => self.string(codeword, out, written)?,
        }
        Ok(())
    }

    /// A codeword that is to be a string's.
    #[inline]
    fn string(&mut self, codeword: u16, out: &mut [u8], written: &mut usize) -> Result<(), Error> {
        // Clause 5.8 b) and c). The encoder cannot have sent the entry
        // it had only just made (clause 6.3 b), which in this dictionary
        // is the one not yet made: C1.
        if codeword < N6
            || codeword == self.dictionary.next_empty()
            || !self.dictionary.in_use(codeword)
        {
            return Err(Error::EmptyEntry(codeword));
        }
        let mut string = [0u8; 256];
        let len = self.dictionary.string(codeword, &mut string);
        if len == 0 || len > out.len() - *written {
            return Err(Error::EmptyEntry(codeword));
        }
        out[*written..*written + len].copy_from_slice(&string[..len]);
        *written += len;
        for &character in &string[..len] {
            if character == self.escape {
                // Clause 9.2 b): in compressed mode too.
                self.escape = self.escape.wrapping_add(ESCAPE_STEP);
            }
        }
        // Clause 8: the last string lengthened by the first character of
        // this one is the entry the encoder made when it sent the last.
        if self.previous != NONE {
            self.matcher.last_made = self
                .dictionary
                .add(self.previous, string[0])
                .unwrap_or(NONE);
        }
        self.previous = codeword;
        Ok(())
    }
}
