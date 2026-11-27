// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The receiver: characters out of the synchronous stream.
//!
//! The bits of the stream are already at the bit times, so there is no
//! sampling to do: a character begins at a bit of the start polarity
//! after the stop polarity, and its bits are the ones that follow.
//!
//! # A stop element left out
//!
//! Where a stop element should be and a bit of the start polarity is,
//! the far converter has left the stop element out (clause 7.1), and that
//! bit is the start element of the next character. The character is good
//! and is given. It cannot happen twice running, since the converter
//! leaves out one in eight at the most; a second in a row is a framing
//! error, and the receiver looks for the stop polarity before it takes
//! another start element.
//!
//! # A break
//!
//! 2M + 3 or more bits of the start polarity are a break (clause 7.3.2).
//! A break begins like the character NUL with its stop element left out,
//! so that character is held back until the bit that shows which of the
//! two it was: no NUL comes before a break (the clause's note).

use crate::Format;

/// What the receiver made of one bit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Received {
    characters: [u8; 2],
    len: u8,
    /// A character's parity was wrong. The character is given all the
    /// same.
    pub parity_error: bool,
    /// A stop element was neither there nor left out as it may be: the
    /// character is lost.
    pub framing_error: bool,
    /// A break began.
    pub break_began: bool,
}

impl Received {
    /// The characters that this bit completed: none, one, or two when a
    /// NUL that was held back is given with the character after it.
    pub fn characters(&self) -> &[u8] {
        &self.characters[..self.len as usize]
    }

    fn push(&mut self, character: u8) {
        if (self.len as usize) < self.characters.len() {
            self.characters[self.len as usize] = character;
            self.len += 1;
        }
    }
}

/// Where the receiver is in the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Between characters: the next bit of the start polarity is a start
    /// element.
    Idle,
    /// In a character: `count` of its units have come, the first in bit
    /// 0 of `bits`.
    Payload { bits: u16, count: u8 },
    /// After a framing error or in a break: nothing is a start element
    /// until the stop polarity has been seen.
    Lost,
}

/// The receiver.
#[derive(Clone, Debug)]
pub struct Receiver {
    format: Format,
    state: State,
    // Bits of the start polarity in a row.
    zeros: u16,
    // Whether the character before this one had its stop element left
    // out.
    after_deletion: bool,
    // A NUL whose stop element was not there, held until it is known
    // whether a break follows.
    held_nul: bool,
    // A framing error after the held NUL, which is one only if no break
    // comes of it.
    held_error: bool,
    in_break: bool,
    framing_errors: u32,
    parity_errors: u32,
    breaks: u32,
    reinserted: u32,
}

impl Receiver {
    /// A receiver between characters.
    pub fn new(format: Format) -> Receiver {
        Receiver {
            format,
            state: State::Idle,
            zeros: 0,
            after_deletion: false,
            held_nul: false,
            held_error: false,
            in_break: false,
            framing_errors: 0,
            parity_errors: 0,
            breaks: 0,
            reinserted: 0,
        }
    }

    /// The character format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Changes the character format, and begins between characters.
    pub fn set_format(&mut self, format: Format) {
        self.format = format;
        self.reset();
    }

    /// The characters lost to a wrong stop element so far.
    pub fn framing_errors(&self) -> u32 {
        self.framing_errors
    }

    /// The characters whose parity was wrong so far.
    pub fn parity_errors(&self) -> u32 {
        self.parity_errors
    }

    /// The breaks so far.
    pub fn breaks(&self) -> u32 {
        self.breaks
    }

    /// The stop elements that the far converter had left out and this one
    /// took as read (clause 7.2).
    pub fn reinserted_stops(&self) -> u32 {
        self.reinserted
    }

    /// Whether a break is on the line now.
    pub fn in_break(&self) -> bool {
        self.in_break
    }

    /// Between characters again, with nothing held. The counts are kept.
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.zeros = 0;
        self.after_deletion = false;
        self.held_nul = false;
        self.held_error = false;
        self.in_break = false;
    }

    /// Gives a character, counting its parity.
    #[inline]
    fn give(&mut self, payload: u16, out: &mut Received) {
        let (character, parity_ok) = self.format.character(payload);
        if !parity_ok {
            self.parity_errors += 1;
            out.parity_error = true;
        }
        out.push(character);
    }

    /// Takes one bit of the stream: `true` is binary 1, the stop
    /// polarity.
    #[inline]
    pub fn process(&mut self, bit: bool) -> Received {
        let mut out = Received::default();
        if bit {
            self.zeros = 0;
            self.in_break = false;
            if self.held_nul {
                // It was a character after all, with its stop element
                // left out.
                self.held_nul = false;
                self.reinserted += 1;
                self.give(0, &mut out);
            }
            if self.held_error {
                self.held_error = false;
                self.framing_errors += 1;
                out.framing_error = true;
            }
        } else {
            self.zeros = self.zeros.saturating_add(1);
            if self.zeros == self.format.break_bits() {
                // Clause 7.3.2. What was taken for characters on the way
                // here was the break.
                self.held_nul = false;
                self.held_error = false;
                self.in_break = true;
                self.breaks += 1;
                self.state = State::Lost;
                out.break_began = true;
                return out;
            }
        }
        match self.state {
            State::Idle => {
                if !bit {
                    self.state = State::Payload { bits: 0, count: 0 };
                }
            }
            State::Lost => {
                if bit {
                    self.state = State::Idle;
                    self.after_deletion = false;
                }
            }
            State::Payload { bits, count } => {
                let wanted = self.format.payload_bits();
                if count < wanted {
                    self.state = State::Payload {
                        bits: bits | (u16::from(bit) << count),
                        count: count + 1,
                    };
                } else if bit {
                    // The stop element.
                    self.give(bits, &mut out);
                    self.after_deletion = false;
                    self.state = State::Idle;
                } else if self.after_deletion {
                    // No stop element twice running: the bits are not
                    // where they were thought to be, or a break has
                    // begun.
                    if self.held_nul {
                        self.held_error = true;
                    } else {
                        self.framing_errors += 1;
                        out.framing_error = true;
                    }
                    self.state = State::Lost;
                } else {
                    // The stop element was left out, and this bit is the
                    // next start element.
                    self.after_deletion = true;
                    if bits == 0 {
                        // Or it is the beginning of a break.
                        self.held_nul = true;
                    } else {
                        self.reinserted += 1;
                        self.give(bits, &mut out);
                    }
                    self.state = State::Payload { bits: 0, count: 0 };
                }
            }
        }
        out
    }
}
