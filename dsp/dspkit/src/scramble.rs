// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A self-synchronising scrambler and its descrambler.
//!
//! The scrambler divides the data by a generating polynomial of three
//! terms, `1 + x^-a + x^-b`: each bit it sends is the data bit added,
//! modulo 2, to the bits it sent `a` and `b` bits before. The descrambler
//! multiplies by the same polynomial, adding to each bit it receives the
//! bits it received `a` and `b` bits before, and needs nothing but the
//! last `b` bits of the line to be in step: after an error or at the start
//! it is right again `b` bits later. One error on the line is three in the
//! data, where the polynomial's three terms meet it.
//!
//! The modems that use one differ in the polynomial and nothing else, so
//! it is a parameter. V.22 clause 5 has `1 + x^-14 + x^-17`.
//!
//! # The guard against a run of ones
//!
//! A scrambler given the right data from the right state sends ones for
//! ever, and a V.22 modem that hears unscrambled ones takes them for a
//! command. So V.22 clause 5.1 has the scrambler count the ones it sends,
//! and after 64 in a row invert the next bit it is given; the descrambler
//! of clause 5.2 counts the same ones as they arrive and inverts the next
//! bit it gives, which puts the data right. The guard is a parameter too,
//! and may be turned off while the modem sends the unbroken ones of a
//! handshake.

/// A generating polynomial of three terms, `1 + x^-first + x^-second`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Polynomial {
    first: u8,
    second: u8,
}

impl Polynomial {
    /// The longest delay a polynomial may have.
    pub const MAX_DELAY: u8 = 63;

    /// The polynomial `1 + x^-first + x^-second`. Each delay is held
    /// between 1 and [`Polynomial::MAX_DELAY`].
    pub const fn new(first: u8, second: u8) -> Polynomial {
        Polynomial {
            first: clamp_delay(first),
            second: clamp_delay(second),
        }
    }

    /// The two delays.
    pub const fn delays(self) -> (u8, u8) {
        (self.first, self.second)
    }

    /// The sum, modulo 2, of the two bits of `history` the polynomial
    /// names. Bit 0 of `history` is the bit of one bit ago.
    #[inline]
    fn feedback(self, history: u64) -> bool {
        ((history >> (self.first - 1)) ^ (history >> (self.second - 1))) & 1 == 1
    }
}

const fn clamp_delay(delay: u8) -> u8 {
    if delay < 1 {
        1
    } else if delay > Polynomial::MAX_DELAY {
        Polynomial::MAX_DELAY
    } else {
        delay
    }
}

/// The count of ones, and the inversion that follows a full count: the
/// same on both sides, which is what keeps them in step.
#[derive(Clone, Copy, Debug)]
struct Guard {
    // Ones in a row that turn the guard on; zero is no guard.
    limit: u32,
    run: u32,
    invert_next: bool,
}

impl Guard {
    const fn new(limit: u32) -> Guard {
        Guard {
            limit,
            run: 0,
            invert_next: false,
        }
    }

    /// Whether the bit now passing is to be inverted.
    #[inline]
    fn take(&mut self) -> bool {
        std::mem::take(&mut self.invert_next)
    }

    /// Counts a bit of the line.
    #[inline]
    fn count(&mut self, line_bit: bool) {
        if self.limit == 0 {
            return;
        }
        if line_bit {
            self.run += 1;
            if self.run >= self.limit {
                // V.22bis clause 5.1 has the counter reset here.
                self.invert_next = true;
                self.run = 0;
            }
        } else {
            self.run = 0;
        }
    }

    fn set_limit(&mut self, limit: u32) {
        self.limit = limit;
        self.run = 0;
        self.invert_next = false;
    }
}

/// The scrambler.
#[derive(Clone, Copy, Debug)]
pub struct Scrambler {
    polynomial: Polynomial,
    // The bits sent, the last in bit 0.
    history: u64,
    guard: Guard,
}

impl Scrambler {
    /// A scrambler with this polynomial, no guard, and a history of
    /// zeros.
    pub const fn new(polynomial: Polynomial) -> Scrambler {
        Scrambler {
            polynomial,
            history: 0,
            guard: Guard::new(0),
        }
    }

    /// The same with the guard: after `ones` ones in a row on the line the
    /// next data bit is inverted. Zero is no guard.
    pub const fn with_guard(polynomial: Polynomial, ones: u32) -> Scrambler {
        Scrambler {
            polynomial,
            history: 0,
            guard: Guard::new(ones),
        }
    }

    /// Changes the guard, and starts its count again. Zero is no guard.
    pub fn set_guard(&mut self, ones: u32) {
        self.guard.set_limit(ones);
    }

    /// Scrambles one bit.
    #[inline]
    pub fn bit(&mut self, data: bool) -> bool {
        let data = data ^ self.guard.take();
        let line = data ^ self.polynomial.feedback(self.history);
        self.history = (self.history << 1) | u64::from(line);
        self.guard.count(line);
        line
    }

    /// Passes a bit to the line as it is, and keeps the history of the
    /// line all the same: what a modem does while it sends an unscrambled
    /// signal, so that the scrambling that follows starts from the line.
    #[inline]
    pub fn bypass(&mut self, line: bool) -> bool {
        self.history = (self.history << 1) | u64::from(line);
        line
    }

    /// A history of zeros and a count of none.
    pub fn reset(&mut self) {
        self.history = 0;
        self.guard.set_limit(self.guard.limit);
    }
}

/// The descrambler.
#[derive(Clone, Copy, Debug)]
pub struct Descrambler {
    polynomial: Polynomial,
    // The bits received, the last in bit 0.
    history: u64,
    guard: Guard,
}

impl Descrambler {
    /// A descrambler with this polynomial, no guard, and a history of
    /// zeros.
    pub const fn new(polynomial: Polynomial) -> Descrambler {
        Descrambler {
            polynomial,
            history: 0,
            guard: Guard::new(0),
        }
    }

    /// The same with the guard: after `ones` ones in a row on the line the
    /// next bit given is inverted. Zero is no guard.
    pub const fn with_guard(polynomial: Polynomial, ones: u32) -> Descrambler {
        Descrambler {
            polynomial,
            history: 0,
            guard: Guard::new(ones),
        }
    }

    /// Changes the guard, and starts its count again. Zero is no guard.
    pub fn set_guard(&mut self, ones: u32) {
        self.guard.set_limit(ones);
    }

    /// Descrambles one bit.
    #[inline]
    pub fn bit(&mut self, line: bool) -> bool {
        let data = line ^ self.polynomial.feedback(self.history) ^ self.guard.take();
        self.history = (self.history << 1) | u64::from(line);
        self.guard.count(line);
        data
    }

    /// A history of zeros and a count of none.
    pub fn reset(&mut self) {
        self.history = 0;
        self.guard.set_limit(self.guard.limit);
    }
}
