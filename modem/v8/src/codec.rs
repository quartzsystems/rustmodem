// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The coding of the V.8 signals (V.8 clauses 5 and 6): menus to octets,
//! octets to bits, and back.
//!
//! A sequence is ten ONEs, ten bits of synchronization, and then octets,
//! each with a start bit (ZERO) before it and a stop bit (ONE) after it.
//! Within an octet bit b0 is sent first. A signal is its sequence over and
//! over.

use crate::{CallFunction, Capabilities, Modulation, Modulations, PcmModems, PstnAccess};

/// The most octets of a sequence that are kept. A sequence of this
/// crate's own is seven at the most; a longer one from another modem,
/// which would have non-standard facilities in it, loses its end.
pub const MAX_OCTETS: usize = 24;

/// The bits of CJ: three octets of all ZEROs with their start and stop
/// bits (V.8 clause 3.5).
pub const CJ_BITS: usize = 30;

/// The ten bits of synchronization of a CI sequence, in the order they
/// are sent (V.8 clause 6, Table 1).
const SYNC_CI: [bool; 10] = bits_of("0000000001");

/// The ten bits of synchronization of a CM or JM sequence (Table 1).
const SYNC_MENU: [bool; 10] = bits_of("0000001111");

const fn bits_of(text: &str) -> [bool; 10] {
    let text = text.as_bytes();
    let mut bits = [false; 10];
    let mut n = 0;
    while n < 10 {
        bits[n] = text[n] == b'1';
        n += 1;
    }
    bits
}

// The category tags: bits b0 to b3 of a category octet (V.8 clause 6,
// Table 2). The table prints them in the order sent, b0 first; here b0 is
// the lowest bit.

/// 1000: call function.
const TAG_CALL_FUNCTION: u8 = 0b0001;
/// 1010: modulation modes.
const TAG_MODULATION: u8 = 0b0101;
/// 0101: protocols.
const TAG_PROTOCOL: u8 = 0b1010;
/// 1011: PSTN access.
const TAG_ACCESS: u8 = 0b1101;
/// 1110: PCM modem availability.
const TAG_PCM: u8 = 0b0111;

/// Bit b4, which is ZERO in a category octet (V.8 clause 5.1).
const B4: u8 = 0x10;

/// Bits b3, b4 and b5, which are 0, 1 and 0 in an extension octet (V.8
/// clause 5.2).
const EXTENSION_MASK: u8 = 0x38;
const EXTENSION: u8 = 0x10;

/// The option bits b5, b6 and b7 of a category octet.
const B5: u8 = 0x20;
const B6: u8 = 0x40;
const B7: u8 = 0x80;

/// The option bits b0, b1 and b2 of an extension octet.
const B0: u8 = 0x01;
const B1: u8 = 0x02;
const B2: u8 = 0x04;

/// The LAPM code of the protocol category: b5 b6 b7 are 1 0 0 (V.8 clause
/// 6.4, Table 6).
const PROTOCOL_LAPM: u8 = B5;

/// The information octets of a CM or JM sequence: a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Menu {
    octets: [u8; MAX_OCTETS],
    len: u8,
}

impl Menu {
    /// A menu of these octets. Those beyond [`MAX_OCTETS`] are left off.
    pub fn from_octets(octets: &[u8]) -> Menu {
        let mut menu = Menu {
            octets: [0; MAX_OCTETS],
            len: 0,
        };
        for &octet in octets.iter().take(MAX_OCTETS) {
            menu.octets[menu.len as usize] = octet;
            menu.len += 1;
        }
        menu
    }

    /// The octets.
    pub fn octets(&self) -> &[u8] {
        &self.octets[..self.len as usize]
    }

    fn push(&mut self, octet: u8) {
        if (self.len as usize) < MAX_OCTETS {
            self.octets[self.len as usize] = octet;
            self.len += 1;
        }
    }

    /// The menu that says what a modem can do.
    ///
    /// `modulation_octets` is how many octets of the modulation modes
    /// category to send, from 1 to 3, if more than the modes themselves
    /// need: a joint menu has as many as the call menu it answers (V.8
    /// clauses 7.4 and 8.2.3). The order is the call function first, as
    /// clause 5 requires, and then the modulation modes, the protocol,
    /// PSTN access and PCM modem availability.
    pub fn from_capabilities(capabilities: &Capabilities, modulation_octets: u8) -> Menu {
        let mut menu = Menu::from_octets(&[]);
        let modes = capabilities.modulations;

        // Octet callf0 (Table 3).
        menu.push(TAG_CALL_FUNCTION | capabilities.call_function.option_bits() << 5);

        // Octets modn0, modn1 and modn2 (Table 4).
        let pcm = capabilities.pcm.filter(|pcm| *pcm != PcmModems::default());
        // If either V.90 bit is set the V.34 bit is set too (clause 6.3).
        let v34 = modes.has(Modulation::V34)
            || pcm.is_some_and(|pcm| pcm.v90_analogue || pcm.v90_digital);
        let mut modn0 = TAG_MODULATION;
        if pcm.is_some() {
            // Item 0: the PCM modem availability category is present.
            modn0 |= B5;
        }
        if v34 {
            modn0 |= B6;
        }
        if modes.has(Modulation::V34HalfDuplex) {
            modn0 |= B7;
        }
        let mut modn1 = EXTENSION;
        for (mode, bit) in [
            (Modulation::V32bis, B0),
            (Modulation::V22bis, B1),
            (Modulation::V17, B2),
            (Modulation::V29, B6),
            (Modulation::V27ter, B7),
        ] {
            if modes.has(mode) {
                modn1 |= bit;
            }
        }
        let mut modn2 = EXTENSION;
        for (mode, bit) in [
            (Modulation::V26ter, B0),
            (Modulation::V26bis, B1),
            (Modulation::V23, B2),
            (Modulation::V23HalfDuplex, B6),
            (Modulation::V21, B7),
        ] {
            if modes.has(mode) {
                modn2 |= bit;
            }
        }
        let needed = if modn2 != EXTENSION {
            3
        } else if modn1 != EXTENSION {
            2
        } else {
            1
        };
        let count = needed.max(modulation_octets.min(3));
        menu.push(modn0);
        if count >= 2 {
            menu.push(modn1);
        }
        if count >= 3 {
            menu.push(modn2);
        }

        // Octet prot0 (Table 6).
        if capabilities.lapm {
            menu.push(TAG_PROTOCOL | PROTOCOL_LAPM);
        }

        // Octet access0 (Table 7). It is present whenever the PCM modem
        // availability category is (clause 6.3).
        let access = match (capabilities.access, pcm) {
            (Some(access), _) => Some(access),
            (None, Some(_)) => Some(PstnAccess::default()),
            (None, None) => None,
        };
        if let Some(access) = access {
            let mut octet = TAG_ACCESS;
            if access.call_cellular {
                octet |= B5;
            }
            if access.answer_cellular {
                octet |= B6;
            }
            if access.digital {
                octet |= B7;
            }
            menu.push(octet);
        }

        // Octet pcm0 (Table 5).
        if let Some(pcm) = pcm {
            let mut octet = TAG_PCM;
            if pcm.v90_analogue {
                octet |= B5;
            }
            if pcm.v90_digital {
                octet |= B6;
            }
            if pcm.v91 {
                octet |= B7;
            }
            menu.push(octet);
        }
        menu
    }

    /// What the menu says, and how many octets its modulation modes
    /// category has. `None` if the menu does not begin with the call
    /// function, as every menu must (V.8 clause 5).
    ///
    /// Categories, extension octets and bits this does not know are
    /// passed over, as clause 10 requires of a receiver.
    pub fn capabilities(&self) -> Option<(Capabilities, u8)> {
        let octets = self.octets();
        let first = *octets.first()?;
        if first & B4 != 0 || first & 0x0F != TAG_CALL_FUNCTION {
            return None;
        }
        let mut capabilities = Capabilities {
            call_function: CallFunction::from_option_bits(first >> 5),
            modulations: Modulations::NONE,
            lapm: false,
            access: None,
            pcm: None,
        };
        let mut modulation_octets = 0u8;
        // The category the octets being read belong to, and how many
        // extension octets of it have gone by.
        let mut category = TAG_CALL_FUNCTION;
        let mut extensions = 0u8;
        for &octet in &octets[1..] {
            if octet & EXTENSION_MASK == EXTENSION {
                extensions = extensions.saturating_add(1);
                if category == TAG_MODULATION {
                    let modes = match extensions {
                        1 => [
                            (Modulation::V32bis, B0),
                            (Modulation::V22bis, B1),
                            (Modulation::V17, B2),
                            (Modulation::V29, B6),
                            (Modulation::V27ter, B7),
                        ],
                        2 => [
                            (Modulation::V26ter, B0),
                            (Modulation::V26bis, B1),
                            (Modulation::V23, B2),
                            (Modulation::V23HalfDuplex, B6),
                            (Modulation::V21, B7),
                        ],
                        // Further octets are for a later edition.
                        _ => continue,
                    };
                    modulation_octets = modulation_octets.saturating_add(1);
                    for (mode, bit) in modes {
                        if octet & bit != 0 {
                            capabilities.modulations = capabilities.modulations.with(mode);
                        }
                    }
                }
                continue;
            }
            if octet & B4 != 0 {
                // Neither a category octet nor an extension octet.
                continue;
            }
            category = octet & 0x0F;
            extensions = 0;
            match category {
                TAG_MODULATION => {
                    modulation_octets = 1;
                    // b5 says the PCM modem availability category is
                    // present; the category itself is what is read.
                    if octet & B6 != 0 {
                        capabilities.modulations = capabilities.modulations.with(Modulation::V34);
                    }
                    if octet & B7 != 0 {
                        capabilities.modulations =
                            capabilities.modulations.with(Modulation::V34HalfDuplex);
                    }
                }
                TAG_PROTOCOL => capabilities.lapm = octet & 0xE0 == PROTOCOL_LAPM,
                TAG_ACCESS => {
                    capabilities.access = Some(PstnAccess {
                        call_cellular: octet & B5 != 0,
                        answer_cellular: octet & B6 != 0,
                        digital: octet & B7 != 0,
                    });
                }
                TAG_PCM => {
                    capabilities.pcm = Some(PcmModems {
                        v90_analogue: octet & B5 != 0,
                        v90_digital: octet & B6 != 0,
                        v91: octet & B7 != 0,
                    });
                }
                _ => {}
            }
        }
        Some((capabilities, modulation_octets))
    }
}

/// Which signal a sequence is of, by its synchronization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// CI, the call indicator signal.
    CallIndicator,
    /// CM or JM: the two are told apart by who sends them.
    Menu,
}

/// A sequence that was received whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Signal {
    /// The signal it is a sequence of.
    pub kind: SignalKind,
    /// Its information octets.
    pub menu: Menu,
}

/// Makes the bits of a signal: its sequence, over and over.
#[derive(Clone, Copy, Debug)]
pub struct SequenceEncoder {
    menu: Menu,
    sync: [bool; 10],
    // The ten-bit unit being sent: 0 is the ten ONEs, 1 the
    // synchronization, and 2 onwards the octets. And the bit within it.
    unit: u8,
    bit: u8,
}

impl SequenceEncoder {
    /// An encoder of a CM or JM signal with this menu.
    pub fn menu(menu: Menu) -> SequenceEncoder {
        SequenceEncoder {
            menu,
            sync: SYNC_MENU,
            unit: 0,
            bit: 0,
        }
    }

    /// An encoder of a CI signal with this call function.
    pub fn call_indicator(call_function: CallFunction) -> SequenceEncoder {
        SequenceEncoder {
            menu: Menu::from_octets(&[TAG_CALL_FUNCTION | call_function.option_bits() << 5]),
            sync: SYNC_CI,
            unit: 0,
            bit: 0,
        }
    }

    /// Whether the next bit begins a ten-bit unit: the ten ONEs, the
    /// synchronization, or an octet with its start and stop bits. A
    /// calling modem ends CM at such a place (V.8 clause 8.1.2).
    pub fn at_unit_boundary(&self) -> bool {
        self.bit == 0
    }

    /// The next bit.
    #[inline]
    pub fn next_bit(&mut self) -> bool {
        let bit = match self.unit {
            0 => true,
            1 => self.sync[self.bit as usize],
            unit => {
                let octet = self.menu.octets()[unit as usize - 2];
                match self.bit {
                    0 => false,
                    9 => true,
                    n => octet >> (n - 1) & 1 == 1,
                }
            }
        };
        self.bit += 1;
        if self.bit == 10 {
            self.bit = 0;
            self.unit += 1;
            if self.unit as usize == 2 + self.menu.octets().len() {
                self.unit = 0;
            }
        }
        bit
    }
}

/// The bit of CJ at a place from 0 to 29: three octets of ZEROs, each
/// with its start bit, ZERO, and its stop bit, ONE (V.8 clause 3.5).
pub const fn cj_bits(n: usize) -> bool {
    n % 10 == 9
}

/// The least ONEs before a synchronization for it to be taken as one. A
/// sequence has ten; the first of them may be lost where a receiver's
/// timing settles.
const ONES_BEFORE_SYNC: u32 = 8;

/// Takes received bits and gives the sequences in them, and says when CJ
/// has gone by.
///
/// A sequence ends where the ten ONEs of the next begin, so each is given
/// when the one after it starts. A sequence that ends any other way, at
/// an octet without its start or its stop bit, was spoiled by the line
/// and is not given at all.
#[derive(Clone, Copy, Debug)]
pub struct SequenceDecoder {
    // The bits so far, the latest lowest.
    history: u32,
    seen: u32,
    // The sequence being read, if a synchronization has been found.
    reading: Option<(SignalKind, Menu)>,
    // The frame being read: its bits, the first lowest, and how many.
    frame: u16,
    frame_bits: u8,
}

/// What a bit brought.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decoded {
    /// A sequence that has just ended.
    pub signal: Option<Signal>,
    /// The last bit of CJ has just been received.
    pub terminator: bool,
}

impl SequenceDecoder {
    /// A decoder that has seen nothing.
    pub fn new() -> SequenceDecoder {
        SequenceDecoder {
            history: 0,
            seen: 0,
            reading: None,
            frame: 0,
            frame_bits: 0,
        }
    }

    /// Takes one received bit.
    #[inline]
    pub fn push(&mut self, bit: bool) -> Decoded {
        let mut out = Decoded::default();
        self.history = self.history << 1 | u32::from(bit);
        self.seen = self.seen.saturating_add(1);

        // CJ: thirty bits, each tenth a ONE and the rest ZEROs.
        if self.seen >= CJ_BITS as u32 && self.history & 0x3FFF_FFFF == 0x0010_0401 {
            out.terminator = true;
        }

        // A sequence being read: ten bits at a time.
        if let Some((kind, mut menu)) = self.reading {
            self.frame |= u16::from(bit) << self.frame_bits;
            self.frame_bits += 1;
            if self.frame_bits < 10 {
                return out;
            }
            let frame = self.frame;
            self.frame = 0;
            self.frame_bits = 0;
            let start = frame & 1 != 0;
            let stop = frame >> 9 & 1 != 0;
            if !start && stop {
                menu.push((frame >> 1) as u8);
                self.reading = Some((kind, menu));
                return out;
            }
            // Not an octet. If it is ten ONEs it is the beginning of the
            // next sequence, or the idle line after the signal, and what
            // was read is a sequence. If it is anything else the line
            // spoiled it, and what was read is the beginning of a
            // sequence and not the whole of one: it is dropped, because
            // a menu cut short reads as a shorter menu.
            self.reading = None;
            if frame == 0x3FF && !menu.octets().is_empty() {
                out.signal = Some(Signal { kind, menu });
            }
            return out;
        }

        // Looking for a synchronization with ONEs before it.
        if self.seen < ONES_BEFORE_SYNC + 10 {
            return out;
        }
        let ones = (1 << ONES_BEFORE_SYNC) - 1;
        let pattern = |sync: &[bool; 10]| {
            let mut word = ones;
            for &bit in sync {
                word = word << 1 | u32::from(bit);
            }
            word
        };
        let mask = (1 << (ONES_BEFORE_SYNC + 10)) - 1;
        let latest = self.history & mask;
        let kind = if latest == pattern(&SYNC_MENU) {
            Some(SignalKind::Menu)
        } else if latest == pattern(&SYNC_CI) {
            Some(SignalKind::CallIndicator)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.reading = Some((kind, Menu::from_octets(&[])));
            self.frame = 0;
            self.frame_bits = 0;
        }
        out
    }
}

impl Default for SequenceDecoder {
    fn default() -> Self {
        SequenceDecoder::new()
    }
}

/// The last few sequences received, to tell when one has come twice.
///
/// JM is sent, and CJ, "after a minimum of two identical" sequences have
/// been received (V.8 clauses 7.4, 8.1.2 and 8.2.2). The two need not
/// follow each other: on a line that loses a packet now and then, every
/// second sequence may be spoiled, and the good ones are still alike.
/// Three are kept. A sequence spoiled by the line is not like any other,
/// spoiled or not, but by a chance too small to count.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Recent {
    menus: [Option<Menu>; 3],
    next: usize,
}

impl Recent {
    /// Notes a sequence, and says whether one like it is among the last
    /// three before it.
    pub(crate) fn seen(&mut self, menu: Menu) -> bool {
        let seen = self.menus.contains(&Some(menu));
        self.menus[self.next] = Some(menu);
        self.next = (self.next + 1) % self.menus.len();
        seen
    }
}
