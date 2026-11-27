// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The information field of an XID frame (V.42 clause 12.2): what two
//! modems say to each other to agree the parameters of the link and of
//! data compression before the link is set up.
//!
//! The field is a format identifier and then groups, each a group
//! identifier, a length and parameters, each of those an identifier, a
//! length and a value (Figure 11). Two groups are used: the parameter
//! negotiation group of ISO/IEC 8885, for the link's own parameters
//! (Table 11a), and the private parameter negotiation group, for V.42bis
//! (Table 11b and V.42bis Annex A).
//!
//! What is read here was written by the far end. Every length is checked
//! against what is left of the field, a field that is cut short gives
//! what was whole before the cut, and what is not known is passed over
//! (clause 12.2.2: "fields that are not recognized are ignored").

/// The format identifier: the general purpose format of ISO/IEC 8885
/// (clause 12.2.2).
const FORMAT: u8 = 0x82;

/// The group identifiers (clause 12.2.2): parameter negotiation, private
/// parameter negotiation, and the user data subfield, which has no
/// length and runs to the end of the field.
const GROUP_PARAMETERS: u8 = 0x80;
const GROUP_PRIVATE: u8 = 0xF0;
const GROUP_USER: u8 = 0xFF;

/// The parameter identifiers of the parameter negotiation group (Table
/// 11a).
const PI_OPTIONS: u8 = 3;
const PI_N401_TX: u8 = 5;
const PI_N401_RX: u8 = 6;
const PI_K_TX: u8 = 7;
const PI_K_RX: u8 = 8;

/// The parameter identifiers of the private group (Table 11b).
const PI_SET: u8 = 0;
const PI_P0: u8 = 1;
const PI_P1: u8 = 2;
const PI_P2: u8 = 3;

/// The parameter set identification: "V42" (V.42bis Annex A, Table A-1:
/// `01010110 00110100 00110010`). Table 11b of V.42 has the first octet
/// as `00101010`, which is no letter; the modems in the field send what
/// V.42bis has, and so does this one. What is received is not checked.
const SET_V42: [u8; 3] = *b"V42";

/// The HDLC optional functions mask (Table 11a, Note 1, as Corrigendum 1
/// has it): bits 2, 4, 8, 9, 12 and 16 are set for conformance with
/// ISO/IEC 8885, and none of the options of clause 10 is asked for:
/// neither selective reject (bits 3 and 24), nor the loop-back test (bit
/// 14), nor the 32-bit check sequence (bit 17). Three octets, as ISO/IEC
/// 8885 has it; four are taken too.
const OPTIONS: [u8; 3] = [0x8A, 0x89, 0x00];

/// The default of N401 in octets (clause 9.2.3) and of k (clause 9.2.4).
pub const DEFAULT_N401: u16 = 128;
pub const DEFAULT_K: u8 = 15;

/// The defaults of V.42bis: P1, the number of codewords, which is also
/// the fewest there may be, and P2, the longest string (V.42bis clause
/// 5.1).
pub const DEFAULT_P1: u16 = 512;
pub const DEFAULT_P2: u8 = 6;

/// The range of P2 (V.42bis clause 5.1).
pub const P2_RANGE: (u8, u8) = (6, 250);

/// A request for data compression, or the answer to one: the three
/// parameters of V.42bis clause 5.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Compression {
    /// P0: compression from the negotiation's initiator to its
    /// responder.
    pub initiator_to_responder: bool,
    /// P0: compression from the responder to the initiator.
    pub responder_to_initiator: bool,
    /// P1: the number of codewords.
    pub codewords: u16,
    /// P2: the longest string.
    pub string_length: u8,
}

impl Compression {
    /// Whether compression is asked for in either direction.
    pub const fn any(self) -> bool {
        self.initiator_to_responder || self.responder_to_initiator
    }
}

/// What an XID frame says. A parameter that is absent has its default
/// (clauses 9.2.3 and 9.2.4), and compression that is absent is not
/// asked for (Table 11b, Note 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Parameters {
    /// N401 from the initiator to the responder, in octets.
    pub n401_initiator: u16,
    /// N401 from the responder to the initiator, in octets.
    pub n401_responder: u16,
    /// The window from the initiator to the responder.
    pub k_initiator: u8,
    /// The window from the responder to the initiator.
    pub k_responder: u8,
    /// Data compression.
    pub compression: Option<Compression>,
}

impl Default for Parameters {
    fn default() -> Self {
        Parameters {
            n401_initiator: DEFAULT_N401,
            n401_responder: DEFAULT_N401,
            k_initiator: DEFAULT_K,
            k_responder: DEFAULT_K,
            compression: None,
        }
    }
}

impl Parameters {
    /// The answer of a responder that can do `ours` to an initiator that
    /// asks for `theirs`.
    ///
    /// N401 and k: the value is between the initiator's and the default
    /// (clauses 9.2.3 and 9.2.4), and no more than this modem can do.
    /// Compression: a direction is used only if both want it, and the
    /// lower of each of P1 and P2 is taken (V.42bis clause 5.1).
    pub fn answer(theirs: &Parameters, ours: &Parameters) -> Parameters {
        let between = |asked: u16, default: u16, most: u16| {
            // Towards the default from what was asked, and no further
            // than this modem goes.
            if asked <= default {
                asked.max(1)
            } else {
                asked.min(most.max(default))
            }
        };
        let k = |asked: u8, most: u8| between(asked.into(), DEFAULT_K.into(), most.into()) as u8;
        let compression = match (theirs.compression, ours.compression) {
            (Some(asked), Some(able)) => {
                let answer = Compression {
                    initiator_to_responder: asked.initiator_to_responder
                        && able.initiator_to_responder,
                    responder_to_initiator: asked.responder_to_initiator
                        && able.responder_to_initiator,
                    codewords: asked.codewords.min(able.codewords),
                    string_length: asked.string_length.min(able.string_length),
                };
                // Less than the fewest codewords or a string length out
                // of its range is a procedural error (V.42bis clause
                // 5.1); no compression is the answer to it.
                let sound = asked.codewords >= DEFAULT_P1
                    && asked.string_length >= P2_RANGE.0
                    && asked.string_length <= P2_RANGE.1;
                (answer.any() && sound).then_some(answer)
            }
            _ => None,
        };
        Parameters {
            n401_initiator: between(theirs.n401_initiator, DEFAULT_N401, ours.n401_initiator),
            n401_responder: between(theirs.n401_responder, DEFAULT_N401, ours.n401_responder),
            k_initiator: k(theirs.k_initiator, ours.k_initiator),
            k_responder: k(theirs.k_responder, ours.k_responder),
            compression,
        }
    }

    /// What an initiator that asked for `asked` makes of a responder's
    /// answer: the answer, held to what was asked, so that a responder
    /// that answers with more than it was offered gets no more.
    pub fn agreed(asked: &Parameters, answer: &Parameters) -> Parameters {
        let hold = |answered: u16, asked: u16, default: u16| answered.clamp(1, asked.max(default));
        let compression = match (asked.compression, answer.compression) {
            (Some(asked), Some(answer)) => {
                let agreed = Compression {
                    initiator_to_responder: asked.initiator_to_responder
                        && answer.initiator_to_responder,
                    responder_to_initiator: asked.responder_to_initiator
                        && answer.responder_to_initiator,
                    codewords: answer.codewords.min(asked.codewords),
                    string_length: answer.string_length.min(asked.string_length),
                };
                let sound = answer.codewords >= DEFAULT_P1 && answer.string_length >= P2_RANGE.0;
                (agreed.any() && sound).then_some(agreed)
            }
            _ => None,
        };
        Parameters {
            n401_initiator: hold(answer.n401_initiator, asked.n401_initiator, DEFAULT_N401),
            n401_responder: hold(answer.n401_responder, asked.n401_responder, DEFAULT_N401),
            k_initiator: hold(
                answer.k_initiator.into(),
                asked.k_initiator.into(),
                DEFAULT_K.into(),
            ) as u8,
            k_responder: hold(
                answer.k_responder.into(),
                asked.k_responder.into(),
                DEFAULT_K.into(),
            ) as u8,
            compression,
        }
    }

    /// Writes the information field into `out` and gives its length:
    /// zero if `out` is too short, which 40 octets never is.
    pub fn encode(&self, out: &mut [u8]) -> usize {
        let mut field = Writer { out, len: 0 };
        field.put(&[FORMAT]);
        // Clause 12.2.2: N401 in bits; within an octet the low bit first
        // and of several octets the high one first (Table 11a, Notes 3
        // and 4).
        let bits = |octets: u16| (u32::from(octets) * 8).min(0xFFFF) as u16;
        field.put(&[GROUP_PARAMETERS, 0, 3 + 2 + 4 + 4 + 3 + 3]);
        field.put(&[PI_OPTIONS, 3]);
        field.put(&OPTIONS);
        field.put(&[PI_N401_TX, 2]);
        field.put(&bits(self.n401_initiator).to_be_bytes());
        field.put(&[PI_N401_RX, 2]);
        field.put(&bits(self.n401_responder).to_be_bytes());
        field.put(&[PI_K_TX, 1, self.k_initiator]);
        field.put(&[PI_K_RX, 1, self.k_responder]);
        if let Some(compression) = self.compression {
            // V.42bis Annex A: the private group, its set identification
            // first, immediately before any user data subfield.
            field.put(&[GROUP_PRIVATE, 0, 5 + 3 + 4 + 3]);
            field.put(&[PI_SET, 3]);
            field.put(&SET_V42);
            let p0 = u8::from(compression.initiator_to_responder)
                | (u8::from(compression.responder_to_initiator) << 1);
            field.put(&[PI_P0, 1, p0]);
            field.put(&[PI_P1, 2]);
            field.put(&compression.codewords.to_be_bytes());
            field.put(&[PI_P2, 1, compression.string_length]);
        }
        field.len
    }

    /// Reads an information field. `None` if it is not of the format
    /// this Recommendation uses; otherwise what of it could be read, with
    /// the defaults for the rest.
    pub fn decode(field: &[u8]) -> Option<Parameters> {
        let (&format, mut rest) = field.split_first()?;
        if format != FORMAT {
            return None;
        }
        let mut parameters = Parameters::default();
        // The parameters of V.42bis as they come: P0 says whether
        // compression is asked for at all.
        let (mut p0, mut p1, mut p2) = (None, DEFAULT_P1, DEFAULT_P2);
        while let Some((&group, after)) = rest.split_first() {
            if group == GROUP_USER {
                // No length: it is the rest of the field (clause
                // 12.2.1.3). Nothing in it is used here.
                break;
            }
            let Some((length, after)) = after.split_first_chunk::<2>() else {
                break;
            };
            let length = usize::from(u16::from_be_bytes(*length));
            // A group that says it is longer than what is left is taken
            // for what is left.
            let (mut body, after) = after.split_at(length.min(after.len()));
            rest = after;
            while let Some((&[identifier, length], after)) = body.split_first_chunk::<2>() {
                let length = usize::from(length);
                if length > after.len() {
                    break;
                }
                let (value, after) = after.split_at(length);
                body = after;
                // The value as a number, the high octet first; no value
                // used here is longer than two octets.
                let number = value
                    .iter()
                    .fold(0u32, |n, &octet| (n << 8) | u32::from(octet));
                let small = length <= 2 && length > 0;
                match (group, identifier) {
                    (GROUP_PARAMETERS, PI_N401_TX) if small => {
                        parameters.n401_initiator = (number / 8).max(1) as u16;
                    }
                    (GROUP_PARAMETERS, PI_N401_RX) if small => {
                        parameters.n401_responder = (number / 8).max(1) as u16;
                    }
                    (GROUP_PARAMETERS, PI_K_TX) if length == 1 => {
                        parameters.k_initiator = (number as u8).clamp(1, 127);
                    }
                    (GROUP_PARAMETERS, PI_K_RX) if length == 1 => {
                        parameters.k_responder = (number as u8).clamp(1, 127);
                    }
                    (GROUP_PRIVATE, PI_P0) if length == 1 => p0 = Some(number as u8 & 3),
                    (GROUP_PRIVATE, PI_P1) if small => p1 = number as u16,
                    (GROUP_PRIVATE, PI_P2) if length == 1 => p2 = number as u8,
                    _ => {}
                }
            }
        }
        // Table 11b, Note 3: the presence of P0 is the request.
        if let Some(p0) = p0 {
            parameters.compression = Some(Compression {
                initiator_to_responder: p0 & 1 != 0,
                responder_to_initiator: p0 & 2 != 0,
                codewords: p1,
                string_length: p2,
            });
        }
        Some(parameters)
    }
}

/// Octets into a buffer, with a count that stays at nothing once one put
/// has not fitted.
struct Writer<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Writer<'_> {
    fn put(&mut self, octets: &[u8]) {
        match self.out.get_mut(self.len..self.len + octets.len()) {
            Some(place) if self.len != usize::MAX => {
                place.copy_from_slice(octets);
                self.len += octets.len();
            }
            _ => {
                // Nothing more is written, and the length says nothing
                // was.
                self.out = &mut [];
                self.len = 0;
            }
        }
    }
}
