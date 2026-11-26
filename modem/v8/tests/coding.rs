// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The coding of V.8 against the tables of the Recommendation (V.8,
//! 11/2000, clauses 5 and 6).
//!
//! The fixtures are written by hand from the tables, as strings of bits in
//! the order they are sent, with a space between ten-bit units. In an
//! octet the tables print the start bit, then b0 to b7, then the stop bit,
//! which is the order of transmission.

use v8::{
    CJ_BITS, CallFunction, Capabilities, MAX_OCTETS, Menu, Modulation, Modulations, PcmModems,
    PstnAccess, SequenceDecoder, SequenceEncoder, Signal, SignalKind, cj_bits,
};

/// The bits of a fixture.
fn bits(text: &str) -> Vec<bool> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '1' => true,
            '0' => false,
            other => panic!("not a bit: {other}"),
        })
        .collect()
}

fn encoded(encoder: &mut SequenceEncoder, len: usize) -> Vec<bool> {
    (0..len).map(|_| encoder.next_bit()).collect()
}

/// The sequences a decoder finds in some bits, and the places CJ ended.
fn decode(stream: &[bool]) -> (Vec<Signal>, Vec<usize>) {
    let mut decoder = SequenceDecoder::new();
    let mut signals = Vec::new();
    let mut terminators = Vec::new();
    for (n, &bit) in stream.iter().enumerate() {
        let out = decoder.push(bit);
        signals.extend(out.signal);
        if out.terminator {
            terminators.push(n);
        }
    }
    (signals, terminators)
}

const V21_ONLY: Capabilities = Capabilities::data(Modulations::of(&[Modulation::V21]));

// Octets.

#[test]
fn the_call_function_octet_is_table_3() {
    // Table 3/V.8: tag 1000, b4 0, and b5 b6 b7 as listed.
    for (function, as_sent, octet) in [
        (CallFunction::Unassigned, "0 1000 0 000 1", 0x01),
        (CallFunction::Multimedia, "0 1000 0 100 1", 0x21),
        (CallFunction::Textphone, "0 1000 0 010 1", 0x41),
        (CallFunction::Videotext, "0 1000 0 110 1", 0x61),
        (CallFunction::TransmitFax, "0 1000 0 001 1", 0x81),
        (CallFunction::ReceiveFax, "0 1000 0 101 1", 0xA1),
        (CallFunction::Data, "0 1000 0 011 1", 0xC1),
        (CallFunction::Extension, "0 1000 0 111 1", 0xE1),
    ] {
        let capabilities = Capabilities {
            call_function: function,
            ..Capabilities::data(Modulations::NONE)
        };
        let menu = Menu::from_capabilities(&capabilities, 0);
        assert_eq!(menu.octets()[0], octet, "{function:?}");
        // And as bits: the first octet of a CI sequence.
        let mut encoder = SequenceEncoder::call_indicator(function);
        let sent = encoded(&mut encoder, 30);
        assert_eq!(sent[20..], bits(as_sent)[..], "{function:?}");
        assert_eq!(CallFunction::from_option_bits(octet >> 5), function);
        assert_eq!(function.option_bits(), octet >> 5);
    }
}

#[test]
fn each_modulation_mode_has_the_bit_of_table_4() {
    // Table 4/V.8: for each mode, the three octets with that mode alone,
    // and its item number.
    for (mode, item, octets) in [
        (Modulation::V34, 1, &[0x45u8][..]),
        (Modulation::V34HalfDuplex, 2, &[0x85]),
        (Modulation::V32bis, 3, &[0x05, 0x11]),
        (Modulation::V22bis, 4, &[0x05, 0x12]),
        (Modulation::V17, 5, &[0x05, 0x14]),
        (Modulation::V29, 6, &[0x05, 0x50]),
        (Modulation::V27ter, 7, &[0x05, 0x90]),
        (Modulation::V26ter, 8, &[0x05, 0x10, 0x11]),
        (Modulation::V26bis, 9, &[0x05, 0x10, 0x12]),
        (Modulation::V23, 10, &[0x05, 0x10, 0x14]),
        (Modulation::V23HalfDuplex, 11, &[0x05, 0x10, 0x50]),
        (Modulation::V21, 12, &[0x05, 0x10, 0x90]),
    ] {
        assert_eq!(mode.item(), item);
        let capabilities = Capabilities::data(Modulations::of(&[mode]));
        let menu = Menu::from_capabilities(&capabilities, 0);
        assert_eq!(&menu.octets()[1..], octets, "{mode:?}");
        // With all three octets asked for, as in a joint menu that
        // answers a call menu of three.
        let menu = Menu::from_capabilities(&capabilities, 3);
        assert_eq!(menu.octets().len(), 4, "{mode:?}");
        let (back, count) = menu.capabilities().unwrap();
        assert_eq!((back, count), (capabilities, 3), "{mode:?}");
    }
    assert_eq!(Modulation::ALL.len(), 12);
    // The octets as Table 4 prints them, for V.21 alone.
    let mut encoder = SequenceEncoder::menu(Menu::from_capabilities(&V21_ONLY, 0));
    let sent = encoded(&mut encoder, 60);
    assert_eq!(
        sent[30..],
        bits("0 1010 0 000 1   0 000 010 00 1   0 000 010 01 1")[..]
    );
}

#[test]
fn the_other_categories_are_tables_5_6_and_7() {
    // Table 6/V.8: the protocol category, tag 0101, LAPM 100.
    let lapm = Capabilities {
        lapm: true,
        ..V21_ONLY
    };
    assert_eq!(
        Menu::from_capabilities(&lapm, 0).octets(),
        [0xC1, 0x05, 0x10, 0x90, 0x2A]
    );

    // Table 7/V.8: PSTN access, tag 1011; b5 the call modem cellular, b6
    // the answer modem cellular, b7 digital.
    for (access, octet) in [
        (PstnAccess::default(), 0x0Du8),
        (
            PstnAccess {
                call_cellular: true,
                ..PstnAccess::default()
            },
            0x2D,
        ),
        (
            PstnAccess {
                answer_cellular: true,
                ..PstnAccess::default()
            },
            0x4D,
        ),
        (
            PstnAccess {
                digital: true,
                ..PstnAccess::default()
            },
            0x8D,
        ),
    ] {
        let capabilities = Capabilities {
            access: Some(access),
            ..V21_ONLY
        };
        let menu = Menu::from_capabilities(&capabilities, 0);
        assert_eq!(menu.octets(), [0xC1, 0x05, 0x10, 0x90, octet]);
        assert_eq!(menu.capabilities().unwrap().0, capabilities);
    }

    // Table 5/V.8: PCM modem availability, tag 1110; b5 V.90 analogue,
    // b6 V.90 digital, b7 V.91. Clause 6.3: with it, the PSTN access
    // category is present, b5 of modn0 is ONE, and if b5 or b6 of pcm0
    // is set so is the V.34 bit.
    let digital = Capabilities {
        pcm: Some(PcmModems {
            v90_digital: true,
            ..PcmModems::default()
        }),
        access: Some(PstnAccess {
            digital: true,
            ..PstnAccess::default()
        }),
        ..Capabilities::data(Modulations::of(&[Modulation::V34, Modulation::V21]))
    };
    let menu = Menu::from_capabilities(&digital, 0);
    assert_eq!(menu.octets(), [0xC1, 0x65, 0x10, 0x90, 0x8D, 0x47]);
    assert_eq!(menu.capabilities().unwrap().0, digital);
    // The V.34 bit and the access category are supplied when they are
    // left out.
    let careless = Capabilities {
        pcm: Some(PcmModems {
            v90_analogue: true,
            ..PcmModems::default()
        }),
        ..V21_ONLY
    };
    let menu = Menu::from_capabilities(&careless, 0);
    assert_eq!(menu.octets(), [0xC1, 0x65, 0x10, 0x90, 0x0D, 0x27]);
    let v91 = Capabilities {
        pcm: Some(PcmModems {
            v91: true,
            ..PcmModems::default()
        }),
        ..V21_ONLY
    };
    assert_eq!(
        Menu::from_capabilities(&v91, 0).octets(),
        [0xC1, 0x25, 0x10, 0x90, 0x0D, 0x87]
    );
}

#[test]
fn every_set_of_modes_survives_coding() {
    for set in 0u32..1 << 12 {
        let modes: Vec<Modulation> = Modulation::ALL
            .into_iter()
            .enumerate()
            .filter(|(n, _)| set >> n & 1 == 1)
            .map(|(_, mode)| mode)
            .collect();
        for lapm in [false, true] {
            let capabilities = Capabilities {
                lapm,
                ..Capabilities::data(Modulations::of(&modes))
            };
            let menu = Menu::from_capabilities(&capabilities, 0);
            let (back, count) = menu.capabilities().unwrap();
            assert_eq!(back, capabilities, "{modes:?}");
            assert_eq!(
                usize::from(count),
                menu.octets().len() - 1 - usize::from(lapm)
            );
            assert_eq!(back.modulations.first(), modes.first().copied());
            // No octet can be taken for an HDLC flag: b4 is ZERO in a
            // category octet, and b3 and b5 in an extension octet
            // (clause 5).
            for &octet in menu.octets() {
                assert!(octet & 0x10 == 0 || octet & 0x28 == 0, "{octet:#04x}");
            }
        }
    }
    let both = Modulations::of(&[Modulation::V34, Modulation::V21]);
    let one = Modulations::of(&[Modulation::V21, Modulation::V22bis]);
    assert_eq!(both.common(one), Modulations::of(&[Modulation::V21]));
    assert!(both.common(Modulations::NONE).is_empty());
    assert_eq!(Modulations::NONE.first(), None);
    assert!(one.has(Modulation::V22bis) && !one.has(Modulation::V34));
}

#[test]
fn what_a_receiver_does_not_know_it_passes_over() {
    // Clause 10: bits, codes and octets reserved for the future are
    // ignored.
    let menu = Menu::from_octets(&[
        0xC1, // data
        0x0E, // the category defined in ITU-T T.66, tag 0111
        0x05, // modulation modes
        0x10, // modn1
        0x90, // modn2: V.21
        0x17, // a fourth octet of modulation modes, for a later edition
        0xEF, // non-standard facilities, tag 1111
        0x13, 0x52, // its field, as extension octets
        0x08, // a category of tag 0001, which there is none of
        0x2A, // LAPM
    ]);
    let (capabilities, count) = menu.capabilities().unwrap();
    assert_eq!(
        capabilities,
        Capabilities {
            lapm: true,
            ..V21_ONLY
        }
    );
    assert_eq!(count, 3);
    // A menu that does not begin with the call function is not one.
    assert_eq!(Menu::from_octets(&[0x05, 0x10, 0x90]).capabilities(), None);
    assert_eq!(Menu::from_octets(&[]).capabilities(), None);
    assert_eq!(Menu::from_octets(&[0xD1]).capabilities(), None);
    // More octets than are kept lose their end and nothing else.
    let long = vec![0xC1u8; MAX_OCTETS + 10];
    assert_eq!(Menu::from_octets(&long).octets().len(), MAX_OCTETS);
}

// Sequences.

/// CM of a data modem with V.21 and nothing else: what this phase's
/// calling modem sends. Clause 5: ten ONEs, the synchronization of Table
/// 1 for CM and JM, and then callf0 (Table 3, data), modn0, modn1 and
/// modn2 (Table 4, V.21).
const CM_V21: &str = "1111111111 0000001111 0100000111 0101000001 0000010001 0000010011";

/// CM of a modem with V.34, V.32 bis, V.22 bis and V.21 that asks for
/// LAPM: callf0, modn0 with b6, modn1 with b0 and b1, modn2 with b7, and
/// prot0 (Table 6).
const CM_FULL: &str =
    "1111111111 0000001111 0100000111 0101000101 0110010001 0000010011 0010101001";

/// CI of a data call: ten ONEs, the synchronization of Table 1 for CI,
/// and callf0 (clause 7.1).
const CI_DATA: &str = "1111111111 0000000001 0100000111";

/// CJ: three octets of ZEROs with start and stop bits (clause 3.5).
const CJ: &str = "0000000001 0000000001 0000000001";

#[test]
fn the_sequences_are_bit_for_bit_what_the_recommendation_gives() {
    let mut encoder = SequenceEncoder::menu(Menu::from_capabilities(&V21_ONLY, 0));
    assert_eq!(encoded(&mut encoder, 60), bits(CM_V21));
    // And again: a signal is its sequence over and over.
    assert_eq!(encoded(&mut encoder, 60), bits(CM_V21));

    let full = Capabilities {
        lapm: true,
        ..Capabilities::data(Modulations::of(&[
            Modulation::V34,
            Modulation::V32bis,
            Modulation::V22bis,
            Modulation::V21,
        ]))
    };
    let mut encoder = SequenceEncoder::menu(Menu::from_capabilities(&full, 0));
    assert_eq!(encoded(&mut encoder, 70), bits(CM_FULL));

    let mut encoder = SequenceEncoder::call_indicator(CallFunction::Data);
    assert_eq!(encoded(&mut encoder, 30), bits(CI_DATA));

    let cj: Vec<bool> = (0..CJ_BITS).map(cj_bits).collect();
    assert_eq!(cj, bits(CJ));
    assert_eq!(CJ_BITS, 30);
}

#[test]
fn an_encoder_knows_where_its_units_begin() {
    let mut encoder = SequenceEncoder::menu(Menu::from_capabilities(&V21_ONLY, 0));
    for n in 0..180 {
        assert_eq!(encoder.at_unit_boundary(), n % 10 == 0, "{n}");
        encoder.next_bit();
    }
}

#[test]
fn the_fixtures_decode_to_what_they_say() {
    // Three sequences: the third's ONEs are what ends the second.
    let stream = bits(&[CM_V21, CM_V21, CM_V21].concat());
    let (signals, terminators) = decode(&stream);
    assert_eq!(signals.len(), 2);
    assert!(terminators.is_empty());
    for signal in &signals {
        assert_eq!(signal.kind, SignalKind::Menu);
        assert_eq!(signal.menu.octets(), [0xC1, 0x05, 0x10, 0x90]);
        assert_eq!(signal.menu.capabilities().unwrap().0, V21_ONLY);
    }

    let stream = bits(&[CM_FULL, CM_FULL].concat());
    let (signals, _) = decode(&stream);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].menu.octets(), [0xC1, 0x45, 0x13, 0x90, 0x2A]);
    let (capabilities, count) = signals[0].menu.capabilities().unwrap();
    assert!(capabilities.lapm);
    assert_eq!(capabilities.modulations.first(), Some(Modulation::V34));
    assert_eq!(count, 3);

    let stream = bits(&[CI_DATA, CI_DATA, CI_DATA].concat());
    let (signals, _) = decode(&stream);
    assert_eq!(signals.len(), 2);
    assert_eq!(signals[0].kind, SignalKind::CallIndicator);
    assert_eq!(signals[0].menu.octets(), [0xC1]);
}

#[test]
fn cj_is_recognised_where_it_ends_and_ends_the_sequence_before_it() {
    // CM, cut where an octet ends, and then CJ: what a calling modem
    // sends (clause 8.1.2).
    let mut stream = bits(CM_V21);
    stream.extend(bits(&CM_V21[..43]));
    let cj_ends = stream.len() + 29;
    stream.extend(bits(CJ));
    stream.extend([true; 20]);
    let (signals, terminators) = decode(&stream);
    assert_eq!(terminators, [cj_ends]);
    assert_eq!(signals.len(), 2);
    assert_eq!(signals[0].menu.octets(), [0xC1, 0x05, 0x10, 0x90]);
    // The second was cut after modn0, and CJ's octets came next.
    assert_eq!(signals[1].menu.octets(), [0xC1, 0x05, 0x00, 0x00, 0x00]);
    // Nothing else in a signal looks like CJ.
    for fixture in [CM_V21, CM_FULL, CI_DATA] {
        let (_, terminators) = decode(&bits(&fixture.repeat(4)));
        assert!(terminators.is_empty(), "{fixture}");
    }
}

#[test]
fn a_sequence_is_found_after_anything_and_with_a_one_or_two_lost() {
    // Whatever came before: silence read as ZEROs, an idle line, noise.
    for before in [
        "",
        "0000000000000",
        "1111111111111111111111",
        "0110100111010110",
    ] {
        let stream = bits(&[before, CM_V21, CM_V21].concat());
        let (signals, _) = decode(&stream);
        assert_eq!(signals.len(), 1, "after {before:?}");
        assert_eq!(signals[0].menu.capabilities().unwrap().0, V21_ONLY);
    }
    // A receiver whose timing is settling may lose a ONE or two of the
    // ten: eight are enough.
    for lost in 0..=4 {
        let stream = bits(&[&CM_V21[lost..], CM_V21].concat());
        let (signals, _) = decode(&stream);
        assert_eq!(signals.len(), usize::from(lost <= 2), "{lost} lost");
    }
    // A synchronization with no ONEs before it is not one: of these three
    // sequences the first is not found, the second is, and the third has
    // nothing after it to end it.
    let stream = bits(&["0000000000", &CM_V21[11..], CM_V21, CM_V21].concat());
    assert_eq!(decode(&stream).0.len(), 1);
}

#[test]
fn a_sequence_that_stops_is_not_a_sequence_until_something_ends_it() {
    // A caller that stops in the middle of an octet: nothing.
    let stream = bits(&CM_V21[..49]);
    assert!(decode(&stream).0.is_empty());
    // The same, and then the line idles at mark, as a start-stop receiver
    // reports a dead line. The idle ONEs fill out the half octet, as they
    // would for any start-stop receiver: five ZEROs and then ONEs is a
    // start bit, 0xF0 and a stop bit. That is neither a category octet
    // nor an extension octet, and is passed over.
    let mut stream = bits(&CM_V21[..49]);
    stream.extend([true; 30]);
    let (signals, _) = decode(&stream);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].menu.octets(), [0xC1, 0x05, 0xF0]);
    let (capabilities, count) = signals[0].menu.capabilities().unwrap();
    assert_eq!(capabilities, Capabilities::data(Modulations::NONE));
    assert_eq!(count, 1);
    // An octet with a wrong stop bit spoils its sequence: what came
    // before it is not given as a shorter sequence, which would be a
    // menu with no V.21 in it. The sequence after is found as usual.
    let broken = CM_V21.replace("0000010001", "0000010000");
    let stream = bits(&[&broken, CM_V21, CM_V21].concat());
    let (signals, _) = decode(&stream);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].menu.octets(), [0xC1, 0x05, 0x10, 0x90]);
    // So is one that a stretch of silence, read as ZEROs, cuts into.
    let silenced = [&CM_V21[..49], "000000000000", CM_V21, CM_V21].concat();
    let (signals, _) = decode(&bits(&silenced));
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].menu.octets(), [0xC1, 0x05, 0x10, 0x90]);
    assert_eq!(SequenceDecoder::default().push(true).signal, None);
}
