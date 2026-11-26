// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The tables of the two laws, built at compile time from the definitions
//! in Tables 1a, 1b, 2a and 2b of G.711.
//!
//! Each law is a list of 129 decision values, numbered 0 to 128, on the
//! positive side, mirrored on the negative. An input between decision
//! values `n` and `n + 1` is in interval `n`, and is sent as the character
//! signal of that interval and decoded as its quantized value.

use crate::{ALAW_OVERLOAD, ULAW_OVERLOAD};

/// The A-law's inversion of the even bits (Note 2 of Table 1a/G.711). Bit 1
/// is the polarity bit and the most significant, so bits 2, 4, 6 and 8 are
/// these.
const ALAW_EVEN_BITS: u8 = 0x55;

/// Decision value number `n` of the u-law, 0 to 128 (Table 2a/G.711, column
/// 5).
///
/// The first interval is one unit wide, from 0 to 1. After it the segments
/// have 15 intervals of 2 and then 16 each of 4, 8, 16, 32, 64, 128 and
/// 256 (column 2), which puts the segment end points at 31, 95, 223, 479,
/// 991, 2015, 4063 and 8159 (column 3).
const fn ulaw_decision(n: usize) -> i32 {
    if n == 0 {
        return 0;
    }
    // Decision value 16k is 2^(k+5) - 33, and the intervals that follow it
    // are 2^(k+1) wide.
    let k = if n == 128 { 7 } else { n / 16 };
    let base = (1 << (k + 5)) - 33;
    let step = 1 << (k + 1);
    base + (n as i32 - 16 * k as i32) * step
}

/// The quantized value of u-law interval `n`, 0 to 127 (Note 3 of Table
/// 2a/G.711): zero for the first, and otherwise the middle of the interval.
const fn ulaw_quantized(n: usize) -> i32 {
    if n == 0 {
        0
    } else {
        (ulaw_decision(n) + ulaw_decision(n + 1)) / 2
    }
}

/// Decision value number `n` of the A-law, 0 to 128 (Table 1a/G.711, column
/// 5).
///
/// The first segment has 32 intervals of 2, and the six after it 16 each of
/// 4, 8, 16, 32, 64 and 128 (column 2), which puts the segment end points
/// at 64, 128, 256, 512, 1024, 2048 and 4096 (column 3).
const fn alaw_decision(n: usize) -> i32 {
    if n <= 32 {
        return 2 * n as i32;
    }
    // Decision value 16s is 2^(s+4), and the intervals that follow it are
    // 2^s wide.
    let s = if n == 128 { 7 } else { n / 16 };
    (1 << (s + 4)) + (n as i32 - 16 * s as i32) * (1 << s)
}

/// The quantized value of A-law interval `n`, 0 to 127: the middle of the
/// interval. Note 3 of Table 1a/G.711 numbers the same values from 1.
const fn alaw_quantized(n: usize) -> i32 {
    (alaw_decision(n) + alaw_decision(n + 1)) / 2
}

/// The character signal of u-law interval `n` (Note 2 of Tables 2a/G.711
/// and 2b/G.711): `255 - n` for a positive input and `127 - n` for a
/// negative one.
pub(crate) const fn ulaw_character(negative: bool, n: u8) -> u8 {
    if negative { 127 - n } else { 255 - n }
}

/// The character signal of A-law interval `n` (Note 2 of Tables 1a/G.711
/// and 1b/G.711): `128 + n` for a positive input and `n` for a negative
/// one, with the even bits then inverted.
pub(crate) const fn alaw_character(negative: bool, n: u8) -> u8 {
    let before_inversion = if negative { n } else { 128 + n };
    before_inversion ^ ALAW_EVEN_BITS
}

/// The interval and the sign a u-law octet stands for.
const fn ulaw_interval_of(octet: u8) -> (bool, usize) {
    if octet >= 128 {
        (false, (255 - octet) as usize)
    } else {
        (true, (127 - octet) as usize)
    }
}

/// The interval and the sign an A-law octet stands for.
const fn alaw_interval_of(octet: u8) -> (bool, usize) {
    let before_inversion = octet ^ ALAW_EVEN_BITS;
    if before_inversion >= 128 {
        (false, (before_inversion - 128) as usize)
    } else {
        (true, before_inversion as usize)
    }
}

const fn ulaw_decode_uniform() -> [i16; 256] {
    let mut table = [0i16; 256];
    let mut octet = 0;
    while octet < 256 {
        let (negative, n) = ulaw_interval_of(octet as u8);
        let value = ulaw_quantized(n);
        table[octet] = if negative { -value } else { value } as i16;
        octet += 1;
    }
    table
}

const fn alaw_decode_uniform() -> [i16; 256] {
    let mut table = [0i16; 256];
    let mut octet = 0;
    while octet < 256 {
        let (negative, n) = alaw_interval_of(octet as u8);
        let value = alaw_quantized(n);
        table[octet] = if negative { -value } else { value } as i16;
        octet += 1;
    }
    table
}

const fn scaled(uniform: &[i16; 256], overload: i16, negative_zero: Option<u8>) -> [f32; 256] {
    let mut table = [0f32; 256];
    let mut octet = 0;
    while octet < 256 {
        table[octet] = uniform[octet] as f32 / overload as f32;
        octet += 1;
    }
    if let Some(octet) = negative_zero {
        table[octet as usize] = -0.0;
    }
    table
}

/// For each whole magnitude below the overload point, the interval it is
/// in.
const fn ulaw_interval() -> [u8; ULAW_OVERLOAD as usize] {
    let mut table = [0u8; ULAW_OVERLOAD as usize];
    let mut n = 0;
    while n < 128 {
        let mut value = ulaw_decision(n);
        while value < ulaw_decision(n + 1) {
            table[value as usize] = n as u8;
            value += 1;
        }
        n += 1;
    }
    table
}

const fn alaw_interval() -> [u8; ALAW_OVERLOAD as usize] {
    let mut table = [0u8; ALAW_OVERLOAD as usize];
    let mut n = 0;
    while n < 128 {
        let mut value = alaw_decision(n);
        while value < alaw_decision(n + 1) {
            table[value as usize] = n as u8;
            value += 1;
        }
        n += 1;
    }
    table
}

/// Column 7 of Tables 2a/G.711 and 2b/G.711, by octet.
pub(crate) static ULAW_DECODE_UNIFORM: [i16; 256] = ulaw_decode_uniform();

/// Column 7 of Tables 1a/G.711 and 1b/G.711, by octet.
pub(crate) static ALAW_DECODE_UNIFORM: [i16; 256] = alaw_decode_uniform();

/// The u-law's values with the overload point at 1.0. The negative octet of
/// zero, `0x7F`, is negative zero.
pub(crate) static ULAW_DECODE: [f32; 256] = scaled(
    &ulaw_decode_uniform(),
    ULAW_OVERLOAD,
    Some(ulaw_character(true, 0)),
);

/// The A-law's values with the overload point at 1.0.
pub(crate) static ALAW_DECODE: [f32; 256] = scaled(&alaw_decode_uniform(), ALAW_OVERLOAD, None);

pub(crate) static ULAW_INTERVAL: [u8; ULAW_OVERLOAD as usize] = ulaw_interval();

pub(crate) static ALAW_INTERVAL: [u8; ALAW_OVERLOAD as usize] = alaw_interval();

/// The decoder output value number of an octet and its sign, as the
/// conversion tables count them (column 8 of Tables 1/G.711 and 2/G.711):
/// the u-law's from 0 to 127 and the A-law's from 1 to 128.
pub(crate) const fn ulaw_number_of(octet: u8) -> (bool, u8) {
    let (negative, n) = ulaw_interval_of(octet);
    (negative, n as u8)
}

pub(crate) const fn alaw_number_of(octet: u8) -> (bool, u8) {
    let (negative, n) = alaw_interval_of(octet);
    (negative, n as u8 + 1)
}

/// The octet of a u-law decoder output value number, 0 to 127.
pub(crate) const fn ulaw_octet_of(negative: bool, number: u8) -> u8 {
    ulaw_character(negative, number)
}

/// The octet of an A-law decoder output value number, 1 to 128.
pub(crate) const fn alaw_octet_of(negative: bool, number: u8) -> u8 {
    alaw_character(negative, number - 1)
}
