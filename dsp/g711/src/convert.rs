// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Conversion of an octet of one law to the other, by Tables 3/G.711 and
//! 4/G.711 (G.711 clause 3.5).
//!
//! The Recommendation gives the conversion as a table between decoder
//! output value numbers, not as a rule: it is close to "the nearest value
//! of the other law" but was adjusted by hand so that a conversion there
//! and back changes at most the last bit (Note 2 of each table). The tables
//! here are those tables.

use crate::tables::{alaw_number_of, alaw_octet_of, ulaw_number_of, ulaw_octet_of};

/// Table 3/G.711 as far as it is printed in full: the A-law decoder output
/// value number for each u-law number from 0 to 87. From 88 to 127 the
/// table runs on with the A-law number one more than the u-law's.
const ULAW_TO_ALAW_NUMBER: [u8; 88] = [
    1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, // 0 to 15
    9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, // 16 to 31
    25, 27, 29, 31, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, // 32 to 47
    46, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, // 48 to 63
    64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, // 64 to 79
    81, 82, 83, 84, 85, 86, 87, 88, // 80 to 87
];

/// Table 4/G.711 as far as it is printed in full: the u-law decoder output
/// value number for each A-law number from 1 to 98; the entry at index 0
/// stands for no number and is not used. From 99 to 128 the table runs on
/// with the u-law number one less than the A-law's.
const ALAW_TO_ULAW_NUMBER: [u8; 99] = [
    0, // not a number of the A-law
    1, 3, 5, 7, 9, 11, 13, 15, 16, 17, 18, 19, 20, 21, 22, 23, // 1 to 16
    24, 25, 26, 27, 28, 29, 30, 31, 32, 32, 33, 33, 34, 34, 35, 35, // 17 to 32
    36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 48, 49, 49, // 33 to 48
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 64, // 49 to 64
    65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 79, // 65 to 80
    80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, // 81 to 96
    96, 97, // 97 and 98
];

const fn ulaw_to_alaw_number(number: u8) -> u8 {
    if (number as usize) < ULAW_TO_ALAW_NUMBER.len() {
        ULAW_TO_ALAW_NUMBER[number as usize]
    } else {
        number + 1
    }
}

const fn alaw_to_ulaw_number(number: u8) -> u8 {
    if (number as usize) < ALAW_TO_ULAW_NUMBER.len() {
        ALAW_TO_ULAW_NUMBER[number as usize]
    } else {
        number - 1
    }
}

const fn ulaw_to_alaw_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut octet = 0;
    while octet < 256 {
        let (negative, number) = ulaw_number_of(octet as u8);
        // The octet that comes out has the A-law's inversion of the even
        // bits in it (Note 1 of Table 3/G.711).
        table[octet] = alaw_octet_of(negative, ulaw_to_alaw_number(number));
        octet += 1;
    }
    table
}

const fn alaw_to_ulaw_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut octet = 0;
    while octet < 256 {
        // The octet that goes in has the inversion of the even bits in it,
        // which is taken out here (Note 1 of Table 4/G.711).
        let (negative, number) = alaw_number_of(octet as u8);
        table[octet] = ulaw_octet_of(negative, alaw_to_ulaw_number(number));
        octet += 1;
    }
    table
}

static ULAW_TO_ALAW: [u8; 256] = ulaw_to_alaw_table();

static ALAW_TO_ULAW: [u8; 256] = alaw_to_ulaw_table();

/// The A-law octet for a u-law octet (Table 3/G.711).
#[inline]
pub const fn ulaw_to_alaw(octet: u8) -> u8 {
    ULAW_TO_ALAW[octet as usize]
}

/// The u-law octet for an A-law octet (Table 4/G.711).
#[inline]
pub const fn alaw_to_ulaw(octet: u8) -> u8 {
    ALAW_TO_ULAW[octet as usize]
}
