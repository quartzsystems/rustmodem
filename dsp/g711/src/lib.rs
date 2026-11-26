// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! G.711: the u-law and A-law codecs.
//!
//! Every modem of rustmodem meets the telephone network as G.711 octets, 8000
//! a second. This crate turns an octet into a linear sample and a linear
//! sample into an octet, in either law, and converts an octet of one law to
//! the other. All of it is tables built when the crate is compiled, from the
//! definitions in ITU-T Recommendation G.711 (Blue Book, 11/1988).
//!
//! # The two scales
//!
//! G.711 defines each law on a uniform scale of its own (G.711 clause 3.6):
//! 14 bits for the u-law, where the largest decision value is 8159, and 13
//! bits for the A-law, where it is 4096. Those are the units of
//! [`Law::decode_uniform`] and [`Law::encode_uniform`], and they are the
//! numbers printed in Tables 1/G.711 and 2/G.711.
//!
//! The signal processing works in `f32`, with full scale, plus or minus 1.0,
//! at the overload point of the law: the largest decision value. A sine wave
//! of amplitude 1.0 is then at the theoretical load capacity of the law,
//! [`Law::overload_dbm0`] (G.711 clause 5). [`Law::decode`] and
//! [`Law::encode`] work on that scale.
//!
//! # What is not here
//!
//! The suppression of the all-zero character signal that G.711 clause 3.2
//! allows a u-law network to ask for (the octet `0x00` sent as `0x02`) is not
//! applied: it is a property of some transmission systems, not of the law,
//! and an RTP path carries all 256 octets.

#![forbid(unsafe_code)]

mod convert;
mod tables;

pub use convert::{alaw_to_ulaw, ulaw_to_alaw};

/// The largest decision value of the u-law, the virtual one, in the units of
/// its 14-bit uniform scale (Table 2a/G.711, columns 3 and 5).
pub const ULAW_OVERLOAD: i16 = 8159;

/// The largest decision value of the A-law, the virtual one, in the units of
/// its 13-bit uniform scale (Table 1a/G.711, columns 3 and 5).
pub const ALAW_OVERLOAD: i16 = 4096;

/// An encoding law of G.711.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Law {
    /// The u-law (Tables 2a/G.711 and 2b/G.711): PCMU in RTP.
    Ulaw,
    /// The A-law (Tables 1a/G.711 and 1b/G.711): PCMA in RTP.
    Alaw,
}

impl Law {
    /// The overload point of the law in the units of its uniform scale: what
    /// 1.0 on the `f32` scale is.
    pub const fn overload(self) -> i16 {
        match self {
            Law::Ulaw => ULAW_OVERLOAD,
            Law::Alaw => ALAW_OVERLOAD,
        }
    }

    /// The level in dBm0 of a sine wave whose peaks are at the overload
    /// point: the theoretical load capacity, Tmax (G.711 clause 5, and
    /// Note 1 of Tables 1/G.711 and 2/G.711).
    // The A-law's 3.14 dBm0 is the figure of the Recommendation, not pi.
    #[allow(clippy::approx_constant)]
    pub const fn overload_dbm0(self) -> f32 {
        match self {
            Law::Ulaw => 3.17,
            Law::Alaw => 3.14,
        }
    }

    /// The octet of the smallest positive value: what a silent line carries.
    ///
    /// In the u-law it is `0xFF` and decodes to zero. The A-law has no zero:
    /// its `0xD5` decodes to one unit of 4096, the smallest value there is
    /// (Table 1a/G.711, decoder output value number 1).
    pub const fn silence(self) -> u8 {
        match self {
            Law::Ulaw => 0xFF,
            Law::Alaw => 0xD5,
        }
    }

    /// The value at the decoder output for an octet, in the units of the
    /// law's uniform scale: column 7 of Tables 1/G.711 and 2/G.711.
    #[inline]
    pub const fn decode_uniform(self, octet: u8) -> i16 {
        match self {
            Law::Ulaw => tables::ULAW_DECODE_UNIFORM[octet as usize],
            Law::Alaw => tables::ALAW_DECODE_UNIFORM[octet as usize],
        }
    }

    /// The octet for a value on the law's uniform scale. A value beyond the
    /// overload point is given the outermost octet of its sign.
    ///
    /// A value of zero is positive. In the u-law, zero and the octet `0x7F`
    /// both decode to zero, so `0x7F` is the one octet this function never
    /// gives back for its own decoded value.
    #[inline]
    pub fn encode_uniform(self, value: i16) -> u8 {
        let magnitude = value.unsigned_abs() as usize;
        self.octet(value < 0, magnitude)
    }

    /// The linear sample for an octet, with 1.0 at the overload point.
    ///
    /// The u-law has two octets for zero, `0xFF` and `0x7F`. The second
    /// decodes to negative zero, so that [`Law::encode`] gives every octet
    /// back.
    #[inline]
    pub const fn decode(self, octet: u8) -> f32 {
        match self {
            Law::Ulaw => tables::ULAW_DECODE[octet as usize],
            Law::Alaw => tables::ALAW_DECODE[octet as usize],
        }
    }

    /// The octet for a linear sample, with 1.0 at the overload point. A
    /// sample beyond it is given the outermost octet of its sign, and one
    /// that is not a number the octet of silence.
    #[inline]
    pub fn encode(self, sample: f32) -> u8 {
        if sample.is_nan() {
            return self.silence();
        }
        let scaled = sample.abs() * f32::from(self.overload());
        // A float to integer cast saturates, so infinity is the last entry.
        self.octet(sample.is_sign_negative(), scaled as usize)
    }

    /// Decodes octets into samples. The two slices are of equal length; if
    /// they are not, the shorter decides how many are done.
    pub fn decode_slice(self, octets: &[u8], samples: &mut [f32]) {
        for (sample, &octet) in samples.iter_mut().zip(octets) {
            *sample = self.decode(octet);
        }
    }

    /// Encodes samples into octets. The two slices are of equal length; if
    /// they are not, the shorter decides how many are done.
    pub fn encode_slice(self, samples: &[f32], octets: &mut [u8]) {
        for (octet, &sample) in octets.iter_mut().zip(samples) {
            *octet = self.encode(sample);
        }
    }

    /// The octet for a magnitude, in whole units of the uniform scale, and a
    /// sign. The magnitude is the integer part of the input, which names the
    /// interval between two decision values the input is in, because every
    /// decision value is a whole number.
    #[inline]
    fn octet(self, negative: bool, magnitude: usize) -> u8 {
        match self {
            Law::Ulaw => {
                let last = tables::ULAW_INTERVAL.len() - 1;
                let n = tables::ULAW_INTERVAL[magnitude.min(last)];
                tables::ulaw_character(negative, n)
            }
            Law::Alaw => {
                let last = tables::ALAW_INTERVAL.len() - 1;
                let n = tables::ALAW_INTERVAL[magnitude.min(last)];
                tables::alaw_character(negative, n)
            }
        }
    }
}
