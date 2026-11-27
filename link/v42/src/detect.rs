// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The detection phase (V.42 clause 7.2.1): finding out whether the far
//! modem does error control at all, before a frame is sent that a modem
//! without it would pass to its host as rubbish.
//!
//! The originator sends the originator detection pattern, which to a
//! modem without error control is the character DC1 with its parity even
//! and then odd: harmless to nearly any host. An answerer that has error
//! control knows the pattern and replies with the answerer detection
//! pattern, the characters `E` and `C`. An originator that has that goes
//! on to the protocol; one that has not had it when the timer T400 runs
//! out takes the answerer to have no error control. An answerer that
//! hears no pattern in T400, and no flag either, takes the same of the
//! originator.
//!
//! Both patterns are start-stop characters with eight to sixteen 1s
//! between them, sent as bits of the synchronous line (clause 7.2.1.2:
//! "without use of any inherent asynchronous speed-matching"), so the
//! converter of V.14 reads them and this is built on its receiver.

use v14::{Format, Receiver};

use crate::Role;

/// DC1 with even parity and with odd: the two characters of the
/// originator detection pattern (clause 7.2.1.2), as octets of eight
/// units, parity the highest.
const DC1_EVEN: u8 = 0x11;
const DC1_ODD: u8 = 0x91;

/// The two characters of the answerer detection pattern that says V.42
/// is supported (Table 3): `E` and `C`.
const ADP_FIRST: u8 = b'E';
const ADP_V42: u8 = b'C';

/// The second character of the pattern that says no error-correcting
/// protocol is desired (Table 3).
const ADP_NONE: u8 = 0x00;

/// The 1s between the characters of a pattern (clauses 7.2.1.2 and
/// 7.2.1.3: 8 to 16). The fewest, so that the patterns take the least
/// time.
const ONES_BETWEEN: u8 = 8;

/// The DC1s of alternating parity an answerer must have to take the
/// originator's pattern as seen (clause 7.2.1.3).
const DC1S_WANTED: u8 = 4;

/// The characters of the answerer's pattern an originator must have:
/// those of two adjacent patterns (clause 7.2.1.2).
const ADP_CHARACTERS_WANTED: u8 = 4;

/// How many times the answerer sends its pattern (clause 7.2.1.3: at
/// least ten).
const ADP_REPEATS: u8 = 10;

/// The flags in a row that an answerer takes for the start of the
/// protocol (clause 7.2.1.3: "receipt of continuous flags").
const FLAGS_WANTED: u8 = 3;

/// How the detection phase ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// It has not ended.
    Pending,
    /// The far modem does V.42: on to the protocol.
    ErrorControl,
    /// The far modem does not, or does not want to: no error control.
    None,
}

/// What a character of a pattern is being sent as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sending {
    /// Binary 1.
    Marks,
    /// A character: the start element, eight units, the stop element and
    /// the 1s after, `left` bits of them in all still to go.
    Character { octet: u8, left: u8 },
}

/// The detection phase of one modem.
#[derive(Clone, Debug)]
pub struct Detection {
    role: Role,
    wanted: bool,
    receiver: Receiver,
    sending: Sending,
    // Which of the pattern's two characters is next.
    second: bool,
    // Whole patterns sent.
    patterns: u8,
    // Whether the answerer is sending its pattern.
    answering: bool,
    // The characters of the far end's pattern received in their order.
    matched: u8,
    last: u8,
    // The far end's reply, for the originator: whether its second
    // character was the one for V.42.
    reply_v42: bool,
    // The 0s and 1s of a flag as they come, for the answerer.
    shift: u8,
    flags: u8,
    since_flag: u8,
    // What is left of T400, in samples.
    left: u32,
    expired: bool,
    outcome: Outcome,
}

impl Detection {
    /// The detection phase of a modem, with a timer T400 of so many
    /// samples (clause 9.1.1: 750 ms). `wanted` is whether this modem
    /// wants error control: an answerer that does not says so in its
    /// reply (Table 3), which saves the originator its timer.
    pub fn new(role: Role, t400_samples: u32, wanted: bool) -> Detection {
        Detection {
            role,
            wanted,
            // Eight units and no parity: the parity of DC1 is read as
            // the eighth.
            receiver: Receiver::new(Format::N81),
            sending: Sending::Marks,
            second: false,
            patterns: 0,
            answering: false,
            matched: 0,
            last: 0,
            reply_v42: false,
            shift: 0xFF,
            flags: 0,
            since_flag: 0,
            left: t400_samples,
            expired: false,
            outcome: Outcome::Pending,
        }
    }

    /// How the phase ended, if it has.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// Time passes: so many samples.
    pub fn advance(&mut self, samples: u32) {
        self.left = self.left.saturating_sub(samples);
        if self.left == 0 {
            self.expired = true;
        }
    }

    /// The next bit to send.
    #[inline]
    pub fn next_bit(&mut self) -> bool {
        if let Sending::Character { octet, left } = self.sending {
            // Of the bits of a character and the 1s after it: the first
            // is the start element, the next eight the units, least
            // significant first, and the rest are 1s.
            let total = 10 + ONES_BETWEEN;
            let at = total - left;
            self.sending = if left > 1 {
                Sending::Character {
                    octet,
                    left: left - 1,
                }
            } else {
                Sending::Marks
            };
            return match at {
                0 => false,
                1..=8 => (octet >> (at - 1)) & 1 == 1,
                _ => true,
            };
        }
        // Between characters: is there another to send?
        let next = match self.role {
            Role::Originator => {
                // Clause 7.2.1.2: the pattern for the period of T400 or
                // until the answerer's is received. The last repetition
                // is completed.
                if self.outcome != Outcome::Pending {
                    None
                } else if self.expired && !self.second {
                    // Clause 7.2.1.2: not observed within T400, following
                    // completion of the last repetition.
                    self.outcome = Outcome::None;
                    None
                } else {
                    Some(if self.second { DC1_ODD } else { DC1_EVEN })
                }
            }
            Role::Answerer => {
                if !self.answering {
                    None
                } else if self.patterns >= ADP_REPEATS {
                    // Clause 7.2.1.3: sent ten times.
                    self.answering = false;
                    if self.outcome == Outcome::Pending {
                        self.outcome = if self.wanted {
                            Outcome::ErrorControl
                        } else {
                            Outcome::None
                        };
                    }
                    None
                } else if !self.second {
                    Some(ADP_FIRST)
                } else if self.wanted {
                    Some(ADP_V42)
                } else {
                    Some(ADP_NONE)
                }
            }
        };
        let Some(octet) = next else {
            return true;
        };
        if self.second {
            self.patterns = self.patterns.saturating_add(1);
        }
        self.second = !self.second;
        self.sending = Sending::Character {
            octet,
            left: 10 + ONES_BETWEEN - 1,
        };
        // The start element.
        false
    }

    /// Takes a bit received.
    #[inline]
    pub fn receive_bit(&mut self, bit: bool) {
        if self.role == Role::Answerer && self.outcome == Outcome::Pending {
            // Clause 7.2.1.3: continuous flags are the start of the
            // protocol, from an originator that sends no pattern.
            self.shift = (self.shift << 1) | u8::from(bit);
            self.since_flag = self.since_flag.saturating_add(1);
            if self.shift == 0x7E {
                // Flags that follow one another are 8 bits apart, or 7
                // where they share a 0.
                self.flags = if self.since_flag <= 8 || self.flags == 0 {
                    self.flags + 1
                } else {
                    1
                };
                self.since_flag = 0;
                if self.flags >= FLAGS_WANTED {
                    self.outcome = Outcome::ErrorControl;
                    self.answering = false;
                }
            }
        }
        let received = self.receiver.process(bit);
        for &character in received.characters() {
            self.character(character);
        }
        if self.role == Role::Answerer
            && self.expired
            && !self.answering
            && self.outcome == Outcome::Pending
        {
            // Clause 7.2.1.3: neither the pattern nor the start of the
            // protocol within T400.
            self.outcome = Outcome::None;
        }
    }

    /// A character of the far end's pattern, perhaps.
    fn character(&mut self, character: u8) {
        if self.outcome != Outcome::Pending {
            return;
        }
        match self.role {
            Role::Answerer => {
                // DC1s whose parity alternates (clause 7.2.1.3).
                let dc1 = character == DC1_EVEN || character == DC1_ODD;
                self.matched = if dc1 && self.matched > 0 && character != self.last {
                    self.matched + 1
                } else {
                    u8::from(dc1)
                };
                self.last = character;
                if self.matched >= DC1S_WANTED && !self.answering {
                    // Clause 7.2.1.3: the pattern is observed; the reply
                    // at once.
                    self.answering = true;
                    self.second = false;
                    self.patterns = 0;
                }
            }
            Role::Originator => {
                // E and its second character in turn, the same second
                // character each time (clause 7.2.1.2: two adjacent
                // patterns).
                let expects_first = self.matched.is_multiple_of(2);
                let fits = if expects_first {
                    character == ADP_FIRST
                } else if self.matched == 1 {
                    character == ADP_V42 || character == ADP_NONE
                } else {
                    character == self.last
                };
                if fits {
                    if !expects_first {
                        self.last = character;
                        self.reply_v42 = character == ADP_V42;
                    }
                    self.matched += 1;
                } else {
                    self.matched = u8::from(character == ADP_FIRST);
                }
                if self.matched >= ADP_CHARACTERS_WANTED {
                    // Clause 7.2.1.2: "if EC is received, initiate LAPM".
                    self.outcome = if self.reply_v42 {
                        Outcome::ErrorControl
                    } else {
                        Outcome::None
                    };
                }
            }
        }
    }
}
