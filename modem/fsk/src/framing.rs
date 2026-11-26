// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Start-stop framing: characters into bits and bits into characters.
//!
//! A character is a start bit, which is space; eight data bits, the least
//! significant first; and a stop bit, which is mark. Between characters
//! the line is at mark.
//!
//! The receiver works on the demodulator's decision at every sample, not
//! on bits someone else has timed, because a start-stop character brings
//! its own timing: the far end's host may begin a character at any moment,
//! and its bits are as long as its clock makes them. The fall from mark to
//! space that begins a character is where the timing of that character is
//! taken from.

use dspkit::SAMPLE_RATE;

use crate::BIT_RATE;

/// How many characters a [`StartStopTransmitter`] holds waiting to be
/// sent: about eight seconds of them at 300 bit/s.
pub const TX_CAPACITY: usize = 256;

/// Puts characters into start-stop frames, a bit at a time.
#[derive(Clone, Debug)]
pub struct StartStopTransmitter {
    queue: [u8; TX_CAPACITY],
    head: usize,
    len: usize,
    // The frame being sent, the next bit to send lowest, and how many of
    // its bits are left.
    shift: u16,
    left: u8,
}

impl StartStopTransmitter {
    /// A transmitter with nothing to send.
    pub fn new() -> StartStopTransmitter {
        StartStopTransmitter {
            queue: [0; TX_CAPACITY],
            head: 0,
            len: 0,
            shift: 0,
            left: 0,
        }
    }

    /// Takes characters to send. Returns how many were taken, which is
    /// fewer than offered when the queue is full.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let taken = data.len().min(TX_CAPACITY - self.len);
        for &byte in &data[..taken] {
            self.queue[(self.head + self.len) % TX_CAPACITY] = byte;
            self.len += 1;
        }
        taken
    }

    /// The characters waiting, not counting one whose bits are going out.
    pub fn queued(&self) -> usize {
        self.len
    }

    /// Whether there is nothing waiting and nothing going out.
    pub fn is_idle(&self) -> bool {
        self.len == 0 && self.left == 0
    }

    /// The next bit to send: `true` is mark. With nothing to send it is
    /// mark.
    #[inline]
    pub fn next_bit(&mut self) -> bool {
        if self.left == 0 {
            if self.len == 0 {
                return true;
            }
            let byte = self.queue[self.head];
            self.head = (self.head + 1) % TX_CAPACITY;
            self.len -= 1;
            // From the lowest bit up: the start bit, which is 0, the
            // eight data bits, and the stop bit, which is 1.
            self.shift = (u16::from(byte) << 1) | 0x200;
            self.left = 10;
        }
        let bit = self.shift & 1 != 0;
        self.shift >>= 1;
        self.left -= 1;
        bit
    }

    /// Drops everything waiting and going out.
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.left = 0;
    }
}

impl Default for StartStopTransmitter {
    fn default() -> Self {
        StartStopTransmitter::new()
    }
}

/// What the start-stop receiver made of a sample. Most samples make
/// nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Framed {
    /// A bit, at the sample in the middle of it: every bit of every
    /// frame, start and stop bits too, and a mark for each bit time the
    /// line idles. This is the received bit stream, for a protocol such as
    /// V.8 that reads it for itself.
    pub bit: Option<bool>,
    /// A character, at its stop bit.
    pub character: Option<u8>,
    /// A character was dropped because its stop bit was space.
    pub framing_error: bool,
    /// The line has been at space for a whole frame and more: a break.
    /// Reported once, when it is recognised.
    pub line_break: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// The line is at mark; a fall to space begins a character.
    Idle,
    /// In a frame: `bit` 0 is the start bit, 1 to 8 the data bits, and 9
    /// the stop bit.
    Frame { bit: u8, data: u8 },
    /// The line is held at space; nothing is taken until it is at mark
    /// again.
    Break,
}

/// Takes characters out of start-stop frames, from the demodulator's
/// decision at every sample.
#[derive(Clone, Debug)]
pub struct StartStopReceiver {
    state: State,
    // The time since the last bit boundary, in units of which SAMPLE_RATE
    // make a bit. A bit is sampled when this passes half a bit.
    into_bit: u32,
    sampled: bool,
    last: bool,
    framing_errors: u32,
}

/// Half a bit, in the units of `into_bit`.
const HALF_BIT: u32 = SAMPLE_RATE / 2;

impl StartStopReceiver {
    /// A receiver on an idle line.
    pub fn new() -> StartStopReceiver {
        StartStopReceiver {
            state: State::Idle,
            into_bit: 0,
            sampled: false,
            last: true,
            framing_errors: 0,
        }
    }

    /// Takes the decision at one sample: `true` is mark.
    #[inline]
    pub fn process(&mut self, mark: bool) -> Framed {
        let mut out = Framed::default();
        let fell = self.last && !mark;
        let changed = self.last != mark;
        self.last = mark;

        match self.state {
            State::Idle => {
                if fell {
                    // The beginning of a start bit: the character's
                    // timing is from here.
                    self.state = State::Frame { bit: 0, data: 0 };
                    self.into_bit = 0;
                    self.sampled = false;
                    return out;
                }
                // The idle line is mark, a bit time at a time.
                if self.tick() {
                    out.bit = Some(true);
                }
            }
            State::Frame { bit, data } => {
                if changed && bit > 0 {
                    // A change inside the frame is at a bit boundary.
                    // Move a quarter of the way to it: enough to follow a
                    // far end whose clock is not ours, without being
                    // thrown by one noisy edge.
                    self.nudge();
                }
                if self.tick() {
                    out.bit = Some(mark);
                    self.state = match bit {
                        // A start bit that is mark in its middle was a
                        // moment of noise, not a start bit.
                        0 if mark => State::Idle,
                        0 => State::Frame { bit: 1, data: 0 },
                        1..=8 => {
                            let data = data | (u8::from(mark) << (bit - 1));
                            State::Frame { bit: bit + 1, data }
                        }
                        _ if mark => {
                            out.character = Some(data);
                            State::Idle
                        }
                        // The stop bit is space. With every data bit
                        // space too the line has been at space for the
                        // whole frame: a break. Otherwise the character
                        // is lost.
                        _ if data == 0 => {
                            out.line_break = true;
                            State::Break
                        }
                        _ => {
                            out.framing_error = true;
                            self.framing_errors = self.framing_errors.saturating_add(1);
                            State::Break
                        }
                    };
                }
            }
            State::Break => {
                // After a break, or a character that did not end, the
                // line must be at mark before a fall can begin a
                // character.
                if self.tick() {
                    out.bit = Some(mark);
                    if mark {
                        self.state = State::Idle;
                    }
                }
            }
        }
        out
    }

    /// Moves the time on by a sample. Says whether this is the sample in
    /// the middle of a bit.
    #[inline]
    fn tick(&mut self) -> bool {
        self.into_bit += BIT_RATE;
        let mut middle = false;
        if !self.sampled && self.into_bit >= HALF_BIT {
            self.sampled = true;
            middle = true;
        }
        if self.into_bit >= SAMPLE_RATE {
            self.into_bit -= SAMPLE_RATE;
            self.sampled = false;
        }
        middle
    }

    /// Moves the bit timing a quarter of the way towards having a bit
    /// boundary now.
    #[inline]
    fn nudge(&mut self) {
        if self.into_bit < HALF_BIT {
            // The boundary was expected `into_bit` ago: the timing is
            // early, and is put back.
            self.into_bit -= self.into_bit / 4;
        } else {
            // The boundary was expected later: the timing is late, and is
            // put on, but not past the boundary itself.
            self.into_bit += (SAMPLE_RATE - self.into_bit) / 4;
        }
    }

    /// The characters lost to a wrong stop bit so far.
    pub fn framing_errors(&self) -> u32 {
        self.framing_errors
    }

    /// An idle line again. The count of framing errors is kept.
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.into_bit = 0;
        self.sampled = false;
        self.last = true;
    }
}

impl Default for StartStopReceiver {
    fn default() -> Self {
        StartStopReceiver::new()
    }
}
