// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! ITU-T V.8: how two modems agree, before either trains, on what they
//! will be to each other.
//!
//! The answering modem sends a modified answer tone, ANSam. A calling
//! modem that knows V.8 answers it with a call menu, CM: the kind of call
//! it wants and the modulations it has. The answering modem replies with
//! a joint menu, JM: the modulations they have in common. The calling
//! modem ends the exchange with CJ, and both begin the modulation the
//! joint menu puts first. CM, JM and CJ are sent at 300 bit/s on the two
//! channels of V.21.
//!
//! This crate is the procedure and the coding and nothing of the signal:
//! it is given the bits a V.21 receiver recovers and the samples that
//! pass, and says what is to be sent, as silence, answer tone or bits, and
//! in the end how it came out. [`Answerer`] is the answering modem's side
//! and [`Caller`] the calling modem's.
//!
//! The edition is V.8 (11/2000). Every signal, code and time here is from
//! it, with the clause beside it.
//!
//! Not built, because nothing yet needs them: the call indicator signal
//! CI, which a calling modem may send and need not (V.8 clause 7.1), is
//! decoded and never sent; the non-standard facilities field (clause 6.6)
//! is skipped over when received and never sent.

#![forbid(unsafe_code)]

mod answerer;
mod caller;
mod codec;

pub use answerer::Answerer;
pub use caller::Caller;
pub use codec::{
    CJ_BITS, Decoded, MAX_OCTETS, Menu, SequenceDecoder, SequenceEncoder, Signal, SignalKind,
    cj_bits,
};

/// Samples in this many milliseconds, at 8000 samples a second.
pub(crate) const fn samples_of_ms(ms: u32) -> u32 {
    ms * 8
}

/// How long the answering modem sends ANSam if neither CM nor anything
/// else ends it, in milliseconds: 5 s, 1 s either way (V.8 clause 8.2.2).
pub const ANSAM_MS: u32 = 5000;

/// The silence between the end of the V.8 signals and the modulation's
/// own, in milliseconds: 75 ms, 5 ms either way (V.8 clauses 8.1.2, 8.2.2
/// and 8.2.3).
pub const GAP_MS: u32 = 75;

/// The silence Te a calling modem keeps between detecting ANSam and
/// sending CM, in milliseconds. V.8 clause 8.1.1 asks for at least 0.5 s,
/// and at least 1 s if network echo cancellers are to be disabled as V.25
/// has it, which is the case here.
pub const TE_MS: u32 = 1000;

/// How long each side waits for the other's part of the exchange before
/// giving up, in milliseconds: the calling modem for two JM once it has
/// begun CM, and the answering modem for CJ once it has begun JM.
///
/// V.8 gives no figure. Clause 8.2.3 leaves the answering modem to end JM
/// by "other criteria" when CJ does not come, and says nothing of a
/// calling modem that hears no JM. A sequence is a fifth of a second, so
/// five seconds is 25 of them.
pub const EXCHANGE_TIMEOUT_MS: u32 = 5000;

/// How long an answering modem that is sending JM goes without a CM
/// before it takes the calling modem to have sent CJ and gone on, in
/// milliseconds.
///
/// CJ is sent once, and a line that loses it leaves the answering modem
/// sending JM to a calling modem that has finished. V.8 clause 8.2.3
/// allows JM to be ended then by "the absence of CM for a suitably long
/// period of time", and gives no figure. A CM is a fifth of a second, so
/// this is five of them: a line that spoils four in a row has more wrong
/// with it than V.8.
pub const CM_ABSENT_MS: u32 = 1000;

/// What a call is for: the call function category (V.8 clause 6.1, Table
/// 3). The number is the three option bits b5, b6 and b7 of the octet
/// `callf0`, with b5 the least significant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallFunction {
    /// 000: to be determined by the ITU-T.
    Unassigned,
    /// 100: a PSTN multimedia terminal (ITU-T H.324).
    Multimedia,
    /// 010: a textphone (ITU-T V.18).
    Textphone,
    /// 110: videotext (ITU-T T.101).
    Videotext,
    /// 001: facsimile, transmitted from the calling terminal (ITU-T T.30).
    TransmitFax,
    /// 101: facsimile, received at the calling terminal (ITU-T T.30).
    ReceiveFax,
    /// 011: data, of no specified application.
    Data,
    /// 111: the call function is in an extension octet.
    Extension,
}

impl CallFunction {
    /// The option bits, b5 lowest.
    pub const fn option_bits(self) -> u8 {
        // Table 3 prints the bits in the order b5 b6 b7.
        match self {
            CallFunction::Unassigned => 0b000,
            CallFunction::Multimedia => 0b001,
            CallFunction::Textphone => 0b010,
            CallFunction::Videotext => 0b011,
            CallFunction::TransmitFax => 0b100,
            CallFunction::ReceiveFax => 0b101,
            CallFunction::Data => 0b110,
            CallFunction::Extension => 0b111,
        }
    }

    /// The call function of three option bits, b5 lowest.
    pub const fn from_option_bits(bits: u8) -> CallFunction {
        match bits & 0b111 {
            0b000 => CallFunction::Unassigned,
            0b001 => CallFunction::Multimedia,
            0b010 => CallFunction::Textphone,
            0b011 => CallFunction::Videotext,
            0b100 => CallFunction::TransmitFax,
            0b101 => CallFunction::ReceiveFax,
            0b110 => CallFunction::Data,
            _ => CallFunction::Extension,
        }
    }
}

/// A modulation mode of the modulation modes category (V.8 clause 6.2,
/// Table 4), in the order of the table's item numbers: of the modes both
/// modems have, the one with the lowest item number is used (clause 7.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Modulation {
    /// Item 1: V.34 duplex.
    V34,
    /// Item 2: V.34 half-duplex.
    V34HalfDuplex,
    /// Item 3: V.32 bis and V.32.
    V32bis,
    /// Item 4: V.22 bis and V.22.
    V22bis,
    /// Item 5: V.17.
    V17,
    /// Item 6: V.29 half-duplex, as T.30 and others use it.
    V29,
    /// Item 7: V.27 ter.
    V27ter,
    /// Item 8: V.26 ter.
    V26ter,
    /// Item 9: V.26 bis.
    V26bis,
    /// Item 10: V.23 duplex.
    V23,
    /// Item 11: V.23 half-duplex.
    V23HalfDuplex,
    /// Item 12: V.21.
    V21,
}

impl Modulation {
    /// Every mode, in the order of the item numbers.
    pub const ALL: [Modulation; 12] = [
        Modulation::V34,
        Modulation::V34HalfDuplex,
        Modulation::V32bis,
        Modulation::V22bis,
        Modulation::V17,
        Modulation::V29,
        Modulation::V27ter,
        Modulation::V26ter,
        Modulation::V26bis,
        Modulation::V23,
        Modulation::V23HalfDuplex,
        Modulation::V21,
    ];

    /// The item number of Table 4/V.8. Item 0 is not a mode: it is the
    /// bit that says the PCM modem availability category is present.
    pub const fn item(self) -> u8 {
        self as u8 + 1
    }
}

/// A set of modulation modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modulations(u16);

impl Modulations {
    /// No mode.
    pub const NONE: Modulations = Modulations(0);

    /// The set of these modes.
    pub const fn of(modes: &[Modulation]) -> Modulations {
        let mut bits = 0;
        let mut n = 0;
        while n < modes.len() {
            bits |= 1 << modes[n].item();
            n += 1;
        }
        Modulations(bits)
    }

    /// The set with a mode added.
    pub const fn with(self, mode: Modulation) -> Modulations {
        Modulations(self.0 | 1 << mode.item())
    }

    /// Whether a mode is in the set.
    pub const fn has(self, mode: Modulation) -> bool {
        self.0 & (1 << mode.item()) != 0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The modes in both sets.
    pub const fn common(self, other: Modulations) -> Modulations {
        Modulations(self.0 & other.0)
    }

    /// The mode with the lowest item number, which is the one two modems
    /// use of those they have in common (V.8 clause 7.4).
    pub fn first(self) -> Option<Modulation> {
        Modulation::ALL.into_iter().find(|&mode| self.has(mode))
    }
}

/// The PCM modem availability category (V.8 clause 6.3, Table 5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PcmModems {
    /// b5: a V.90 or V.92 analogue modem.
    pub v90_analogue: bool,
    /// b6: a V.90 or V.92 digital modem.
    pub v90_digital: bool,
    /// b7: V.91.
    pub v91: bool,
}

/// The PSTN access category (V.8 clause 6.5, Table 7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PstnAccess {
    /// b5: the calling modem is on a cellular connection.
    pub call_cellular: bool,
    /// b6: the answering modem is on a cellular connection.
    pub answer_cellular: bool,
    /// b7: the modem is on a digital network connection.
    pub digital: bool,
}

/// What a modem can do and wants, as CM and JM say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// The call function.
    pub call_function: CallFunction,
    /// The modulation modes.
    pub modulations: Modulations,
    /// The protocol category asks for LAPM, the protocol of V.42 (V.8
    /// clause 6.4).
    pub lapm: bool,
    /// The PSTN access category, if it is sent.
    pub access: Option<PstnAccess>,
    /// The PCM modem availability category, if it is sent. V.8 clause 6.3
    /// then requires the PSTN access category and V.34 as well, and
    /// [`Menu::from_capabilities`] supplies them.
    pub pcm: Option<PcmModems>,
}

impl Capabilities {
    /// A data call with these modulation modes and nothing else.
    pub const fn data(modulations: Modulations) -> Capabilities {
        Capabilities {
            call_function: CallFunction::Data,
            modulations,
            lapm: false,
            access: None,
            pcm: None,
        }
    }
}

/// How the exchange came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The two modems have a modulation in common. It is to be started
    /// now: the silence that follows the V.8 signals is over.
    Agreed {
        /// The modulation to use: of those in the joint menu, the one
        /// with the lowest item number (V.8 clause 7.4).
        modulation: Modulation,
        /// The call function of the joint menu.
        call_function: CallFunction,
        /// Both modems asked for LAPM in the protocol category.
        lapm: bool,
    },
    /// The joint menu has no modulation in it: the modems have none in
    /// common, or the answering modem does not do the call function asked
    /// for (V.8 clauses 7.4 and 8.2.3). Either may disconnect.
    NothingInCommon,
    /// The other modem did not take part: an answering modem heard no CM
    /// for the whole of its ANSam, or a calling modem no JM. The caller
    /// is not a V.8 modem, or cannot be heard; the answering modem goes
    /// on as modems did before V.8 (clause 8.2.2).
    NoResponse,
    /// The exchange began and did not finish: an answering modem sent JM,
    /// and the calling modem went on sending CM as if it had heard none.
    Incomplete,
}

/// What a side of the exchange is sending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sending {
    /// Nothing.
    Silence,
    /// The answering modem's ANSam (V.8 clause 7.2).
    AnswerTone,
    /// Bits at 300 bit/s on this side's V.21 channel: the caller's on
    /// channel No. 1 and the answerer's on channel No. 2 (V.8 clauses 3.4
    /// and 3.6). Each bit is had from `next_bit`.
    Bits,
}

/// Something that happened in the exchange, for a display or a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// The answering modem has had two CM alike and begins JM.
    CallMenu(Capabilities),
    /// The calling modem has had two JM alike and begins CJ.
    JointMenu(Capabilities),
    /// The answering modem has had all of CJ.
    Terminator,
    /// The exchange is over.
    Done(Outcome),
}
