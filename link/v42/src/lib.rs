// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! ITU-T V.42: error control for modems.
//!
//! A modem's line makes errors, and V.42 is the protocol two modems run
//! between themselves so that their hosts see none: the data goes in
//! numbered frames, each with a check sequence, and a frame that does
//! not arrive whole is sent again.
//!
//! - [`detect`] is the detection phase (clause 7.2.1): how each modem
//!   finds out, without disturbing a host, whether the other does error
//!   control.
//! - [`hdlc`] is the framing (clause 8.1.1): flags, the 0 put in after
//!   five 1s, the frame check sequence, the abort.
//!
//! The procedure itself, LAPM (clause 8), is built on these.
//!
//! Everything here is given bits and the time and gives back bits:
//! nothing reads a clock, allocates or calls the system, and it runs on
//! a media thread's tick path. What the far end sends is the far end's
//! to choose, and none of it can make this crate panic, write past a
//! buffer or work without bound.
//!
//! The edition is V.42 (03/2002) with its Corrigendum 1 (07/2003).
//!
//! Not built: the 32-bit frame check sequence (clause 8.1.1.6.2), the
//! loop-back test (clause 8.11), and the alternative procedure of Annex
//! A, which the 2002 edition deleted.

#![forbid(unsafe_code)]

pub mod detect;
pub mod hdlc;

pub use detect::{Detection, Outcome};
pub use hdlc::{Deframed, Deframer, Fill, Framer, Invalid};

/// The most octets of an information field this modem sends or takes:
/// the default of N401 (clause 9.2.3: 128 octets), which it never agrees
/// to raise.
pub const MAX_INFORMATION: usize = 128;

/// The detection phase timer T400, in milliseconds (clause 9.1.1).
pub const T400_MS: u32 = 750;

/// Which end of the call a modem is, for the detection phase and for the
/// C/R bit of a frame's address (clause 7.2.1.1 and Table 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The originating modem.
    Originator,
    /// The answering modem.
    Answerer,
}
