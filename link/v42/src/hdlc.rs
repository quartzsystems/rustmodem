// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! HDLC framing over the synchronous bit stream (V.42 clause 8.1.1).
//!
//! A frame is the octets between two flags, `01111110`. So that no flag
//! is seen inside a frame, the transmitter puts a 0 after every five 1s
//! of the frame's content and the receiver takes it out again (clause
//! 8.1.1.2). The last two octets of a frame are its frame check sequence
//! (clause 8.1.1.6.1). Seven 1s in a row abort the frame they are in
//! (clause 8.1.4), and between frames the line carries flags (clause
//! 8.1.5).
//!
//! - [`Framer`] takes a frame's octets, less the check sequence, and
//!   gives the line's bits.
//! - [`Deframer`] takes the line's bits and gives the frames that are
//!   whole and good, and says what was wrong with the others.
//!
//! The bits come from the far end, and it may send anything: the
//! deframer writes into a buffer of a fixed size and never past it, and
//! does a fixed amount of work for a bit, but for the one bit that ends
//! a frame, when it checks the frame it has.
//!
//! The 32-bit frame check sequence of clause 8.1.1.6.2 is optional and
//! is not built.

/// The most octets of a frame, less its check sequence, that a
/// [`Framer`] takes: an address octet, two control octets and the
/// largest information field.
pub const MAX_FRAME: usize = 3 + crate::MAX_INFORMATION;

/// The octets a [`Deframer`] holds of a frame. The longest frame there
/// may be is [`MAX_FRAME`] and its two octets of check sequence. It
/// holds twice that and two more, so that a frame too long to be allowed
/// can still be told from line noise by its check sequence (the note to
/// clause 8.5.5).
pub const DEFRAMER_CAPACITY: usize = 2 * (MAX_FRAME + 2) + 2;

/// The fewest octets between two flags that are a frame: an address, a
/// control octet and the check sequence (clause 8.1.3 b).
pub const MIN_FRAME: usize = 4;

/// The frame check sequence of some octets (clause 8.1.1.6.1): the ones
/// complement of the remainder, by `x^16 + x^12 + x^5 + 1`, of the
/// octets with the register preset to ones. Its low octet is sent first.
pub fn fcs(octets: &[u8]) -> u16 {
    !update(0xFFFF, octets)
}

/// What the register holds after good octets and their check sequence:
/// the clause's `0001 1101 0000 1111`, which is of the register's bits
/// from x^15 to x^0. The register here has them the other way round.
const GOOD: u16 = 0xF0B8;

/// The register after more octets. The register holds x^15 in its bit 0,
/// since the octets come least significant bit first.
#[inline]
fn update(mut register: u16, octets: &[u8]) -> u16 {
    for &octet in octets {
        register = (register >> 8) ^ TABLE[usize::from((register ^ u16::from(octet)) & 0xFF)];
    }
    register
}

/// The register's change for each octet.
static TABLE: [u16; 256] = table();

const fn table() -> [u16; 256] {
    // The generator with its bits the other way round, x^16 left out.
    const GENERATOR: u16 = 0x8408;
    let mut table = [0u16; 256];
    let mut octet = 0;
    while octet < 256 {
        let mut register = octet as u16;
        let mut bit = 0;
        while bit < 8 {
            register = if register & 1 == 1 {
                (register >> 1) ^ GENERATOR
            } else {
                register >> 1
            };
            bit += 1;
        }
        table[octet] = register;
        octet += 1;
    }
    table
}

/// The 1s of an abort (clause 8.1.4: seven or more). The frame may have
/// ended in up to five of its own, which only makes more.
const ABORT_ONES: u8 = 7;

/// What a [`Framer`] sends when it has no frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fill {
    /// Flags (clause 8.1.5).
    Flags,
    /// Binary 1: what an answering modem sends before the protocol
    /// begins (clause 7.2.1.3).
    Marks,
}

/// What the framer is sending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sending {
    /// Nothing of a frame.
    Idle,
    /// A flag: `left` of its bits to go. `then_frame` if the frame in the
    /// buffer follows it, and `closing` if it ends a frame.
    Flag {
        left: u8,
        then_frame: bool,
        closing: bool,
    },
    /// The frame's bit `at`.
    Frame { at: usize },
    /// An abort: `left` 1s to go.
    Abort { left: u8 },
}

/// The transmitter: frames into bits.
#[derive(Clone, Debug)]
pub struct Framer {
    // The frame being sent or waiting, with its check sequence.
    frame: [u8; MAX_FRAME + 2],
    len: usize,
    waiting: bool,
    sending: Sending,
    // 1s of the frame in a row.
    ones: u8,
    fill: Fill,
    flags: u64,
    frames: u64,
}

impl Framer {
    /// A framer with no frame, filling with flags.
    pub fn new() -> Framer {
        Framer {
            frame: [0; MAX_FRAME + 2],
            len: 0,
            waiting: false,
            sending: Sending::Idle,
            ones: 0,
            fill: Fill::Flags,
            flags: 0,
            frames: 0,
        }
    }

    /// What is sent between frames.
    pub fn set_fill(&mut self, fill: Fill) {
        self.fill = fill;
    }

    /// Whether a frame can be given: none is waiting to begin and none is
    /// going out. One given while the closing flag of the last is going
    /// out follows it with that one flag between them.
    pub fn ready(&self) -> bool {
        !self.busy()
    }

    /// Whether the buffer holds a frame that has not all gone: one
    /// waiting, one whose opening flag is going out, one going out, or
    /// one being aborted.
    fn busy(&self) -> bool {
        self.waiting
            || matches!(
                self.sending,
                Sending::Frame { .. }
                    | Sending::Abort { .. }
                    | Sending::Flag {
                        then_frame: true,
                        ..
                    }
            )
    }

    /// Whether nothing of a frame is going out or waiting: the last
    /// frame's closing flag has gone too.
    pub fn idle(&self) -> bool {
        !self.busy() && !matches!(self.sending, Sending::Flag { closing: true, .. })
    }

    /// The flags sent so far, each counted when it has all gone.
    pub fn flags_sent(&self) -> u64 {
        self.flags
    }

    /// The frames sent so far, each counted when its last bit has gone.
    pub fn frames_sent(&self) -> u64 {
        self.frames
    }

    /// Gives a frame to send: its octets from the address to the end of
    /// its information field. The check sequence is added. Says whether
    /// it was taken, which it is not if one is waiting or it is longer
    /// than [`MAX_FRAME`] or empty.
    pub fn send(&mut self, octets: &[u8]) -> bool {
        // A frame still going out is finished first; the next has the
        // buffer when that one has gone.
        if self.busy() || octets.is_empty() || octets.len() > MAX_FRAME {
            return false;
        }
        self.frame[..octets.len()].copy_from_slice(octets);
        let check = fcs(octets);
        self.frame[octets.len()] = check as u8;
        self.frame[octets.len() + 1] = (check >> 8) as u8;
        self.len = octets.len() + 2;
        self.waiting = true;
        true
    }

    /// The next bit of the line.
    #[inline]
    pub fn next_bit(&mut self) -> bool {
        match self.sending {
            Sending::Idle => {
                if self.waiting || self.fill == Fill::Flags {
                    // An opening flag, or one of the fill.
                    self.sending = Sending::Flag {
                        left: 7,
                        then_frame: self.waiting,
                        closing: false,
                    };
                    self.waiting = false;
                    false
                } else {
                    true
                }
            }
            Sending::Flag {
                left,
                then_frame,
                closing,
            } => {
                if left > 1 {
                    self.sending = Sending::Flag {
                        left: left - 1,
                        then_frame,
                        closing,
                    };
                    return true;
                }
                // The flag's last bit.
                self.flags += 1;
                if then_frame {
                    self.ones = 0;
                    self.sending = Sending::Frame { at: 0 };
                } else if self.waiting {
                    // The closing flag of one frame, or a flag of the
                    // fill, is the opening flag of the next (clause
                    // 8.1.1.2).
                    self.waiting = false;
                    self.ones = 0;
                    self.sending = Sending::Frame { at: 0 };
                } else {
                    self.sending = Sending::Idle;
                }
                false
            }
            Sending::Abort { left } => {
                self.sending = if left > 1 {
                    Sending::Abort { left: left - 1 }
                } else {
                    Sending::Idle
                };
                true
            }
            Sending::Frame { at } => {
                if self.ones == 5 {
                    // Clause 8.1.1.2: a 0 after five 1s.
                    self.ones = 0;
                    return false;
                }
                if at == self.len * 8 {
                    // The closing flag.
                    self.frames += 1;
                    self.sending = Sending::Flag {
                        left: 7,
                        then_frame: false,
                        closing: true,
                    };
                    return false;
                }
                // Clause 8.1.2.2: bit 1 of each octet first.
                let bit = (self.frame[at / 8] >> (at % 8)) & 1 == 1;
                self.ones = if bit { self.ones + 1 } else { 0 };
                self.sending = Sending::Frame { at: at + 1 };
                bit
            }
        }
    }

    /// Forgets the frame going out and the one waiting. If a frame was
    /// going out, the line has seven 1s next, which abort it (clause
    /// 8.1.4), and says so.
    pub fn abort(&mut self) -> bool {
        self.waiting = false;
        match self.sending {
            Sending::Frame { .. } => {
                self.sending = Sending::Abort { left: ABORT_ONES };
                true
            }
            // An abort that is going out goes on, and a closing flag is
            // finished: the frame it closes has gone.
            Sending::Abort { .. } | Sending::Flag { closing: true, .. } => false,
            Sending::Idle | Sending::Flag { .. } => {
                self.sending = match self.sending {
                    // The opening flag of the frame forgotten becomes a
                    // flag of the fill.
                    Sending::Flag { left, .. } => Sending::Flag {
                        left,
                        then_frame: false,
                        closing: false,
                    },
                    other => other,
                };
                false
            }
        }
    }
}

impl Default for Framer {
    fn default() -> Self {
        Framer::new()
    }
}

/// What was wrong with what came between two flags (clause 8.1.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Invalid {
    /// Fewer than four octets (clause 8.1.3 b).
    TooShort,
    /// Not a whole number of octets (clause 8.1.3 c).
    NotOctets,
    /// A frame check sequence error (clause 8.1.3 d).
    CheckSequence,
    /// More octets than the deframer holds: an unbounded frame (the note
    /// to clause 8.5.5).
    Unbounded,
}

/// What the deframer made of a bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Deframed {
    /// Nothing ended.
    Nothing,
    /// A flag that ended no frame: fill, or the first of a frame.
    Flag,
    /// A good frame of this many octets, the check sequence not counted:
    /// [`Deframer::frame`] has it until the next bit is given.
    Frame(usize),
    /// What came between the last two flags was not a frame.
    Invalid(Invalid),
    /// Seven 1s in a row: the frame that was being received is gone
    /// (clause 8.1.4).
    Abort,
}

/// The receiver: bits into frames.
#[derive(Clone, Debug)]
pub struct Deframer {
    octets: [u8; DEFRAMER_CAPACITY],
    // Bits of the frame so far, the bits of what may yet be a flag
    // among them.
    bits: usize,
    // 1s in a row on the line.
    ones: u8,
    // Whether a flag has been seen and no abort since.
    in_frame: bool,
    overrun: bool,
    // The length of the last good frame.
    good: usize,
    frames: u64,
    invalid: u64,
    aborts: u64,
    flags_run: u32,
}

impl Deframer {
    /// A deframer that has seen no flag.
    pub fn new() -> Deframer {
        Deframer {
            octets: [0; DEFRAMER_CAPACITY],
            bits: 0,
            ones: 0,
            in_frame: false,
            overrun: false,
            good: 0,
            frames: 0,
            invalid: 0,
            aborts: 0,
            flags_run: 0,
        }
    }

    /// The octets of the last good frame, from its address to the end of
    /// its information field. They are good until the next bit is given.
    pub fn frame(&self) -> &[u8] {
        &self.octets[..self.good]
    }

    /// The good frames so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// What came between flags and was not a frame, so far: the frame
    /// check errors, in the main.
    pub fn invalid_frames(&self) -> u64 {
        self.invalid
    }

    /// The aborts so far.
    pub fn aborts(&self) -> u64 {
        self.aborts
    }

    /// The flags in a row last seen with nothing between them: zero
    /// since anything else came.
    pub fn flags_in_a_row(&self) -> u32 {
        self.flags_run
    }

    /// As it was made, but for the counts.
    pub fn reset(&mut self) {
        self.bits = 0;
        self.ones = 0;
        self.in_frame = false;
        self.overrun = false;
        self.good = 0;
        self.flags_run = 0;
    }

    #[inline]
    fn push(&mut self, bit: bool) {
        let at = self.bits / 8;
        if at >= DEFRAMER_CAPACITY {
            self.overrun = true;
            return;
        }
        let mask = 1u8 << (self.bits % 8);
        if bit {
            self.octets[at] |= mask;
        } else {
            self.octets[at] &= !mask;
        }
        self.bits += 1;
    }

    /// Takes one bit of the line.
    #[inline]
    pub fn process(&mut self, bit: bool) -> Deframed {
        self.good = 0;
        if bit {
            self.ones += 1;
            if self.ones < 7 {
                if self.in_frame {
                    self.push(true);
                }
                return Deframed::Nothing;
            }
            // Clause 8.1.4. More 1s after the seventh are the same abort,
            // or the idle line.
            self.ones = 7;
            self.flags_run = 0;
            if self.in_frame {
                self.in_frame = false;
                // Six bits of what is held are the abort itself. With
                // nothing before them, a flag was followed by an idle
                // line and no frame was lost.
                let had = self.bits > 6;
                self.bits = 0;
                self.overrun = false;
                if had {
                    self.aborts += 1;
                    return Deframed::Abort;
                }
            }
            return Deframed::Nothing;
        }
        let ones = std::mem::take(&mut self.ones);
        match ones {
            // A 0 after five 1s was put in by the transmitter (clause
            // 8.1.1.2).
            5 => Deframed::Nothing,
            6 => self.flag(),
            _ => {
                if self.in_frame {
                    self.push(false);
                }
                Deframed::Nothing
            }
        }
    }

    /// A flag has ended: the 0 before its six 1s and the 1s are in the
    /// buffer, if a frame was being received.
    fn flag(&mut self) -> Deframed {
        let was_in_frame = std::mem::replace(&mut self.in_frame, true);
        let overrun = std::mem::take(&mut self.overrun);
        let bits = std::mem::take(&mut self.bits);
        // The flag's first seven bits were taken for the frame's. With
        // fewer than eight bits before them there was nothing between
        // this flag and the last: flags that follow one another, or share
        // a 0, or have a few 1s of an idle line between them.
        if !was_in_frame || bits < 7 + 8 {
            self.flags_run = self.flags_run.saturating_add(1);
            return Deframed::Flag;
        }
        self.flags_run = 1;
        let bits = bits - 7;
        let invalid = if overrun {
            Some(Invalid::Unbounded)
        } else if !bits.is_multiple_of(8) {
            Some(Invalid::NotOctets)
        } else if bits / 8 < MIN_FRAME {
            Some(Invalid::TooShort)
        } else if update(0xFFFF, &self.octets[..bits / 8]) != GOOD {
            Some(Invalid::CheckSequence)
        } else {
            None
        };
        match invalid {
            Some(invalid) => {
                self.invalid += 1;
                Deframed::Invalid(invalid)
            }
            None => {
                self.good = bits / 8 - 2;
                self.frames += 1;
                Deframed::Frame(self.good)
            }
        }
    }
}

impl Default for Deframer {
    fn default() -> Self {
        Deframer::new()
    }
}
