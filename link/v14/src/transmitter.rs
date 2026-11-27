// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The transmitter: characters into the synchronous stream.

use crate::Format;

/// The characters the transmitter holds for the line. It is the line
/// that paces the host: when this is full the host is to be held off.
pub const TX_CAPACITY: usize = 64;

/// The fewest characters from one whose stop element is left out to the
/// next, the first of them counted (clause 7.1.1: no more than one for
/// any eight consecutive characters).
const DELETION_SPACING: u8 = 8;

/// What is going out now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sending {
    /// The stop polarity, between characters.
    Idle,
    /// The units of a character after its start element: `left` of them,
    /// the next in bit 0 of `bits`.
    Payload { bits: u16, left: u8 },
    /// The stop element of the character just sent.
    Stop,
    /// A break: `left` bits of the start polarity.
    Break { left: u16 },
    /// The stop polarity after a break: `left` bits of it.
    AfterBreak { left: u16 },
}

/// The transmitter.
#[derive(Clone, Debug)]
pub struct Transmitter {
    format: Format,
    // A ring of the characters waiting.
    queue: [u8; TX_CAPACITY],
    head: usize,
    len: usize,
    sending: Sending,
    deletion: bool,
    // Characters since the last whose stop element was left out, that one
    // counted; held at the spacing.
    since_deletion: u8,
    deleted: u32,
    break_asked: bool,
}

impl Transmitter {
    /// A transmitter with nothing to send, which leaves stop elements
    /// out when it has more than the line takes.
    pub fn new(format: Format) -> Transmitter {
        Transmitter {
            format,
            queue: [0; TX_CAPACITY],
            head: 0,
            len: 0,
            sending: Sending::Idle,
            deletion: true,
            since_deletion: DELETION_SPACING,
            deleted: 0,
            break_asked: false,
        }
    }

    /// The character format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Changes the character format, from the next character.
    pub fn set_format(&mut self, format: Format) {
        self.format = format;
    }

    /// Whether a stop element is left out, one character in eight, while
    /// there is another character waiting behind it (clause 7.1.1).
    pub fn set_deletion(&mut self, deletion: bool) {
        self.deletion = deletion;
    }

    /// Takes characters to send, as many as there is room for, and says
    /// how many it took.
    pub fn write(&mut self, characters: &[u8]) -> usize {
        let taken = characters.len().min(TX_CAPACITY - self.len);
        for &character in &characters[..taken] {
            let at = (self.head + self.len) % TX_CAPACITY;
            self.queue[at] = character;
            self.len += 1;
        }
        taken
    }

    /// The characters there is room for.
    pub fn space(&self) -> usize {
        TX_CAPACITY - self.len
    }

    /// Whether everything given has gone out, its last stop element with
    /// it, and no break is going.
    pub fn idle(&self) -> bool {
        self.len == 0 && self.sending == Sending::Idle && !self.break_asked
    }

    /// The stop elements left out so far.
    pub fn deleted_stops(&self) -> u32 {
        self.deleted
    }

    /// Sends a break when the character going out has gone: 2M + 3 bits
    /// of the start polarity (clause 7.3.1), and after them 2M bits of
    /// the stop polarity, which the far converter needs to find the
    /// characters again (the clause's note).
    pub fn send_break(&mut self) {
        self.break_asked = true;
    }

    /// Forgets the characters waiting and the one going out.
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.sending = Sending::Idle;
        self.break_asked = false;
    }

    fn take(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let character = self.queue[self.head];
        self.head = (self.head + 1) % TX_CAPACITY;
        self.len -= 1;
        Some(character)
    }

    /// Begins the next thing there is to send, at a place where a start
    /// element may go, and gives its first bit. `stop_due` says that the
    /// stop element of the character before has not been sent.
    #[inline]
    fn begin(&mut self, stop_due: bool) -> bool {
        if stop_due {
            // The stop element is left out only where the next character
            // is ready to take its place.
            let may_delete = self.deletion
                && self.since_deletion >= DELETION_SPACING
                && self.len > 0
                && !self.break_asked;
            if !may_delete {
                self.sending = Sending::Stop;
                return self.next_bit();
            }
            self.since_deletion = 0;
            self.deleted += 1;
        }
        if self.break_asked {
            self.break_asked = false;
            self.sending = Sending::Break {
                left: self.format.break_bits() - 1,
            };
            return false;
        }
        match self.take() {
            Some(character) => {
                self.since_deletion = (self.since_deletion + 1).min(DELETION_SPACING);
                self.sending = Sending::Payload {
                    bits: self.format.payload(character),
                    left: self.format.payload_bits(),
                };
                // The start element.
                false
            }
            None => {
                self.sending = Sending::Idle;
                true
            }
        }
    }

    /// The next bit of the synchronous stream: `true` is binary 1, the
    /// stop polarity.
    #[inline]
    pub fn next_bit(&mut self) -> bool {
        match self.sending {
            Sending::Idle => self.begin(false),
            Sending::Payload { bits, left } => {
                if left == 0 {
                    return self.begin(true);
                }
                self.sending = Sending::Payload {
                    bits: bits >> 1,
                    left: left - 1,
                };
                bits & 1 == 1
            }
            Sending::Stop => {
                self.sending = Sending::Idle;
                true
            }
            Sending::Break { left } => {
                if left == 0 {
                    self.sending = Sending::AfterBreak {
                        left: 2 * u16::from(self.format.character_bits()) - 1,
                    };
                    return true;
                }
                self.sending = Sending::Break { left: left - 1 };
                false
            }
            Sending::AfterBreak { left } => {
                if left == 0 {
                    return self.begin(false);
                }
                self.sending = Sending::AfterBreak { left: left - 1 };
                true
            }
        }
    }
}
