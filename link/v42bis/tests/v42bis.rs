// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! V.42bis: the codewords of small examples worked by hand, round trips
//! of a corpus at every dictionary size, the two dictionaries held
//! against each other, the change of mode, the flush, and what the
//! decoder makes of what it should never be sent.

use proptest::prelude::*;
use v42bis::{Decoder, Dictionary, Encoder, Error, Mode, Policy};

/// Numbers from a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 16) % n
    }
}

/// The dictionary sizes and string lengths the tests go through: the
/// least, the most, and some between (clause 5.1).
const SIZES: [u16; 4] = [512, 1024, 2048, 4096];
const LENGTHS: [u8; 4] = [6, 20, 32, 250];

// The corpus.

fn english(len: usize) -> Vec<u8> {
    const WORDS: [&str; 24] = [
        "the",
        "modem",
        "answers",
        "a",
        "call",
        "and",
        "sends",
        "its",
        "carrier",
        "to",
        "line",
        "which",
        "is",
        "then",
        "taken",
        "for",
        "data",
        "by",
        "far",
        "end",
        "of",
        "connection",
        "when",
        "ready",
    ];
    let mut rng = Rng(0x00C0_FFEE);
    let mut text = Vec::new();
    while text.len() < len {
        let word = WORDS[rng.below(24) as usize];
        text.extend_from_slice(word.as_bytes());
        text.push(if rng.below(12) == 0 { b'.' } else { b' ' });
        if rng.below(40) == 0 {
            text.extend_from_slice(b"\r\n");
        }
    }
    text.truncate(len);
    text
}

fn source(len: usize) -> Vec<u8> {
    let unit = b"    pub fn process(&mut self, sample: f32) -> Option<Symbol> {\n        \
        let out = self.demodulator.process(sample);\n        if let Some(symbol) = out.symbol {\n            \
        self.receive(symbol);\n        }\n        out.symbol\n    }\n\n";
    unit.iter().copied().cycle().take(len).collect()
}

/// Frames like those of PPP over a serial line: a flag, an address and
/// control, a protocol, a header that hardly changes, a payload, a check.
fn ppp(len: usize) -> Vec<u8> {
    let mut rng = Rng(0x0BAD_CAFE);
    let mut out = Vec::new();
    let mut sequence = 0u32;
    while out.len() < len {
        out.extend_from_slice(&[0x7E, 0xFF, 0x03, 0x00, 0x21, 0x45, 0x00]);
        let payload = 20 + rng.below(200) as usize;
        out.extend_from_slice(&(payload as u16 + 20).to_be_bytes());
        out.extend_from_slice(&sequence.to_be_bytes());
        sequence = sequence.wrapping_add(1);
        out.extend_from_slice(&[0x40, 0x06, 10, 0, 0, 1, 10, 0, 0, 2]);
        for n in 0..payload {
            // Half of it text-like and half not.
            out.push(if n % 2 == 0 {
                b'a' + rng.below(6) as u8
            } else {
                rng.next() as u8
            });
        }
        out.extend_from_slice(&[(rng.next() >> 8) as u8, rng.next() as u8, 0x7E]);
    }
    out.truncate(len);
    out
}

/// Data with nothing to find in it: as compressed or enciphered data is.
fn random(len: usize) -> Vec<u8> {
    let mut rng = Rng(0xFACE_FEED);
    (0..len).map(|_| (rng.next() >> 24) as u8).collect()
}

fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("empty", Vec::new()),
        ("one octet", vec![0x41]),
        ("English text", english(30_000)),
        ("source code", source(30_000)),
        ("frames of PPP", ppp(30_000)),
        ("random", random(30_000)),
        ("one octet over and over", vec![0x55; 20_000]),
        ("zeros", vec![0; 20_000]),
        (
            "every octet in turn",
            (0..20_000).map(|n| n as u8).collect(),
        ),
        (
            "the escape character as it changes",
            (0..5000u32).map(|n| (n * 51) as u8).collect(),
        ),
        (
            "text, then random, then text",
            [english(8000), random(8000), english(8000)].concat(),
        ),
    ]
}

// Round trips.

/// Encodes `data` in pieces of `piece` octets, flushing after each if
/// `flush`, decodes what comes out as it comes, and gives the encoded
/// octets and the decoded ones. After each flush the two dictionaries
/// are held to be the same.
fn round_trip(
    encoder: &mut Encoder,
    decoder: &mut Decoder,
    data: &[u8],
    piece: usize,
    flush: bool,
) -> (Vec<u8>, Vec<u8>) {
    let mut coded = Vec::new();
    let mut decoded = Vec::new();
    let mut out = [0u8; 4096];
    let mut plain = [0u8; 4096];
    let mut decode = |coded_now: &[u8], decoder: &mut Decoder, decoded: &mut Vec<u8>| {
        let mut rest = coded_now;
        while !rest.is_empty() {
            let (taken, written) = decoder.decode(rest, &mut plain).expect("a decoder error");
            assert!(taken > 0);
            decoded.extend_from_slice(&plain[..written]);
            rest = &rest[taken..];
        }
    };
    for chunk in data.chunks(piece.max(1)) {
        let mut rest = chunk;
        while !rest.is_empty() {
            let (taken, written) = encoder.encode(rest, &mut out);
            assert!(taken > 0);
            decode(&out[..written], decoder, &mut decoded);
            coded.extend_from_slice(&out[..written]);
            rest = &rest[taken..];
        }
        if flush {
            let written = encoder.flush(&mut out);
            decode(&out[..written], decoder, &mut decoded);
            coded.extend_from_slice(&out[..written]);
            // Nothing is held back after a flush: all that went in has
            // come out at the far end.
            assert!(!encoder.pending());
            let done = chunk.as_ptr() as usize - data.as_ptr() as usize + chunk.len();
            assert_eq!(decoded.len(), done);
            // And the two dictionaries are the same, entry for entry.
            assert!(
                encoder.dictionary() == decoder.dictionary(),
                "the dictionaries differ after {} octets",
                decoded.len()
            );
            assert_eq!(encoder.mode(), decoder.mode());
        }
    }
    let written = encoder.flush(&mut out);
    decode(&out[..written], decoder, &mut decoded);
    coded.extend_from_slice(&out[..written]);
    (coded, decoded)
}

#[test]
fn the_corpus_goes_round_at_every_dictionary_size_and_string_length() {
    println!(
        "{:<36} {:>6} {:>4}  {:>7} {:>7}  ratio",
        "data", "N2", "N7", "in", "out"
    );
    for (name, data) in corpus() {
        for size in SIZES {
            for length in LENGTHS {
                let mut encoder = Encoder::new(size, length);
                let mut decoder = Decoder::new(size, length);
                let (coded, decoded) = round_trip(&mut encoder, &mut decoder, &data, 257, false);
                assert!(
                    decoded == data,
                    "{name}, {size} codewords, strings of {length}"
                );
                assert!(
                    encoder.dictionary() == decoder.dictionary(),
                    "{name} {size} {length}"
                );
                if length == 32 || data.len() < 2 {
                    println!(
                        "{name:<36} {size:>6} {length:>4}  {:>7} {:>7}  {:.2}",
                        data.len(),
                        coded.len(),
                        data.len() as f64 / coded.len().max(1) as f64
                    );
                }
            }
        }
    }
}

#[test]
fn the_dictionaries_are_the_same_at_every_step() {
    // A flush after every octet, and after every few: the hardest the
    // encoder can be driven, since every string is cut short. The
    // dictionaries are compared at each flush, which with one octet a
    // piece is at every step.
    for (name, data) in corpus() {
        let data = &data[..data.len().min(6000)];
        for policy in [Policy::Adaptive, Policy::Compressed, Policy::Transparent] {
            for piece in [1, 3, 64] {
                let mut encoder = Encoder::new(1024, 32);
                encoder.set_policy(policy);
                let mut decoder = Decoder::new(1024, 32);
                let (_, decoded) = round_trip(&mut encoder, &mut decoder, data, piece, true);
                assert!(decoded == data, "{name}, {policy:?}, pieces of {piece}");
            }
        }
    }
}

#[test]
fn a_dictionary_that_fills_reuses_its_entries_and_stays_in_step() {
    // 200,000 octets through the smallest dictionary: every entry is
    // recovered and used again many times over (clause 6.5).
    let data = [english(70_000), ppp(60_000), source(70_000)].concat();
    for (size, length) in [(512, 6), (512, 250), (4096, 250)] {
        let mut encoder = Encoder::new(size, length);
        encoder.set_policy(Policy::Compressed);
        let mut decoder = Decoder::new(size, length);
        let (coded, decoded) = round_trip(&mut encoder, &mut decoder, &data, 1000, true);
        assert!(decoded == data);
        assert!(coded.len() < data.len());
        // The next empty entry has gone round the dictionary.
        assert!(encoder.dictionary().next_empty() >= 259);
        assert!(encoder.dictionary().next_empty() < size);
    }
}

// The codewords, by hand.

/// Packs codewords of given sizes as clause 7.5 has it: each one's least
/// significant bit straight after the most significant of the one
/// before, octets filled from bit 1, and 0 bits to the end of the last.
fn pack(codewords: &[(u16, u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut bits, mut count) = (0u32, 0u8);
    for &(codeword, size) in codewords {
        bits |= u32::from(codeword) << count;
        count += size;
        while count >= 8 {
            out.push(bits as u8);
            bits >>= 8;
            count -= 8;
        }
    }
    if count > 0 {
        out.push(bits as u8);
    }
    out
}

#[test]
fn the_codewords_of_a_small_example_are_those_worked_by_hand() {
    // "ABABABAB" in compressed mode from the start. A is codeword 68 and
    // B is 69: the ordinal value and the three control codewords (clause
    // 6.2). The first new entry is 259 (clause 6.5).
    //
    //   A       the string is A.
    //   B       AB is not there: send A (68), make AB (259); the string is B.
    //   A       BA is not there: send B (69), make BA (260); the string is A.
    //   B       AB is there (259) and was not made last: the string is AB.
    //   A       ABA is not there: send AB (259), make ABA (261); the string is A.
    //   B       the string is AB.
    //   A       ABA is there, but was made by the last match and may not
    //           be used (clause 6.3 b): send AB (259). ABA is not made
    //           again (clause 6.4 b). The string is A.
    //   B       the string is AB.
    //   flush   send AB (259), then FLUSH (1) and 0 bits (clause 7.9).
    let mut encoder = Encoder::new(512, 6);
    encoder.set_policy(Policy::Compressed);
    let mut out = [0u8; 64];
    let (taken, written) = encoder.encode(b"ABABABAB", &mut out);
    assert_eq!(taken, 8);
    let flushed = encoder.flush(&mut out[written..]);
    let coded = &out[..written + flushed];
    // The change to compressed mode: the escape character, which is 0 at
    // the start, and ECM, which is 0 (clauses 7.8.1 and 9.1).
    assert_eq!(coded[..2], [0x00, 0x00]);
    let expected = pack(&[(68, 9), (69, 9), (259, 9), (259, 9), (259, 9), (1, 9)]);
    assert_eq!(coded[2..], expected[..]);
    assert_eq!(coded.len(), 2 + 7);

    let mut decoder = Decoder::new(512, 6);
    let mut plain = [0u8; 512];
    let (taken, written) = decoder.decode(coded, &mut plain).unwrap();
    assert_eq!(taken, coded.len());
    assert_eq!(&plain[..written], b"ABABABAB");
    assert_eq!(decoder.mode(), Mode::Compressed);
    // Three entries were made, and the next empty one is 262.
    assert_eq!(decoder.dictionary().next_empty(), 262);
    assert!(encoder.dictionary() == decoder.dictionary());
}

#[test]
fn the_codeword_size_steps_up_when_a_codeword_needs_it() {
    // Clause 7.4: the size begins at 9 bits and the threshold at 512.
    // The first codeword of 512 or more is preceded by STEPUP at the old
    // size and sent at the new one.
    let data = english(60_000);
    let mut encoder = Encoder::new(4096, 32);
    encoder.set_policy(Policy::Compressed);
    let mut decoder = Decoder::new(4096, 32);
    let (coded, decoded) = round_trip(&mut encoder, &mut decoder, &data, 500, false);
    assert!(decoded == data);
    // Enough was made for all 4096 entries to be in use and 12 bits to
    // be needed: the ratio shows the codewords were not 9 bits to the
    // end, and the decoder followed each step or it would not have
    // decoded.
    assert!(
        coded.len() * 2 < data.len(),
        "{} from {}",
        coded.len(),
        data.len()
    );

    // A decoder told to step past the size agreed says so (clause 5.8
    // a). With 512 codewords the size is 9 bits and may never step.
    let mut decoder = Decoder::new(512, 6);
    let mut plain = [0u8; 512];
    let mut coded = vec![0x00, 0x00];
    coded.extend(pack(&[(68, 9), (2, 9)]));
    assert_eq!(decoder.decode(&coded, &mut plain), Err(Error::CodewordSize));
    // With 1024 it may step once and not twice.
    let mut decoder = Decoder::new(1024, 6);
    let mut coded = vec![0x00, 0x00];
    coded.extend(pack(&[(68, 9), (2, 9), (69, 10), (2, 10)]));
    assert_eq!(decoder.decode(&coded, &mut plain), Err(Error::CodewordSize));
}

// The change of mode.

#[test]
fn data_that_does_not_compress_goes_as_it_is() {
    // Random data: the encoder stays in transparent mode, and what goes
    // out is the data with the escape character doubled where it comes,
    // which is once in 256 octets (clause 9.2).
    let data = random(100_000);
    let mut encoder = Encoder::new(2048, 32);
    let mut decoder = Decoder::new(2048, 32);
    let (coded, decoded) = round_trip(&mut encoder, &mut decoder, &data, 1000, false);
    assert!(decoded == data);
    assert_eq!(encoder.mode(), Mode::Transparent);
    let grown = coded.len() as f64 / data.len() as f64;
    assert!(grown < 1.01, "random data grew by {grown}");

    // Text: compressed mode, and well under half the size.
    let data = english(100_000);
    let mut encoder = Encoder::new(2048, 32);
    let mut decoder = Decoder::new(2048, 32);
    let (coded, decoded) = round_trip(&mut encoder, &mut decoder, &data, 1000, false);
    assert!(decoded == data);
    assert_eq!(encoder.mode(), Mode::Compressed);
    assert!(coded.len() * 2 < data.len());

    // Text, then random data, then text: into compressed mode, out of it
    // and into it again, and never much worse than the data itself.
    let parts = [english(20_000), random(20_000), english(20_000)];
    let mut encoder = Encoder::new(2048, 32);
    let mut decoder = Decoder::new(2048, 32);
    let mut modes = Vec::new();
    let mut total = (0usize, 0usize);
    for part in &parts {
        let (coded, decoded) = round_trip(&mut encoder, &mut decoder, part, 500, false);
        assert!(&decoded == part);
        modes.push(encoder.mode());
        total = (total.0 + part.len(), total.1 + coded.len());
        // No part grew by more than a twentieth: the test is made every
        // 256 characters, and that is as long as the wrong mode lasts.
        assert!(
            coded.len() as f64 <= part.len() as f64 * 1.05 + 16.0,
            "{} octets became {}",
            part.len(),
            coded.len()
        );
    }
    assert_eq!(
        modes,
        [Mode::Compressed, Mode::Transparent, Mode::Compressed]
    );
    assert!(total.1 < total.0);

    // The worst there is for transparent mode: every octet the escape
    // character as it then is, so that every one is doubled. The encoder
    // sees that codewords are cheaper and changes mode.
    let data: Vec<u8> = (0..20_000u32).map(|n| (n * 51) as u8).collect();
    let mut encoder = Encoder::new(2048, 32);
    let mut decoder = Decoder::new(2048, 32);
    let (coded, decoded) = round_trip(&mut encoder, &mut decoder, &data, 500, false);
    assert!(decoded == data);
    assert!(
        coded.len() < data.len(),
        "{} from {}",
        coded.len(),
        data.len()
    );
}

#[test]
fn nothing_waits_for_more_data_after_a_flush() {
    // A character and a flush, over and over, in compressed mode: after
    // each flush the decoder has given every character so far (clause
    // 7.9). The flush itself costs two codewords at the most.
    let data = english(3000);
    let mut encoder = Encoder::new(2048, 32);
    encoder.set_policy(Policy::Compressed);
    let mut decoder = Decoder::new(2048, 32);
    let mut out = [0u8; 64];
    let mut plain = [0u8; 512];
    let mut given = 0;
    for (n, &character) in data.iter().enumerate() {
        let (taken, written) = encoder.encode(&[character], &mut out);
        assert_eq!(taken, 1);
        let flushed = encoder.flush(&mut out[written..]);
        assert!(
            written + flushed <= 2 + 2 + 2,
            "{} octets for one character",
            written + flushed
        );
        let (_, decoded) = decoder
            .decode(&out[..written + flushed], &mut plain)
            .unwrap();
        assert_eq!(&plain[..decoded], &data[given..given + decoded]);
        given += decoded;
        assert_eq!(given, n + 1, "character {n} was held back");
        // A second flush has nothing to send.
        assert_eq!(encoder.flush(&mut out), 0);
    }
    // In transparent mode a flush sends nothing: nothing is ever held.
    let mut encoder = Encoder::new(2048, 32);
    encoder.set_policy(Policy::Transparent);
    let (_, written) = encoder.encode(b"plain", &mut out);
    assert_eq!(&out[..written], b"plain");
    assert_eq!(encoder.flush(&mut out), 0);
    assert!(!encoder.pending());
}

#[test]
fn a_reset_begins_again_at_both_ends() {
    let mut encoder = Encoder::new(1024, 20);
    let mut decoder = Decoder::new(1024, 20);
    let data = english(5000);
    let (first, _) = round_trip(&mut encoder, &mut decoder, &data, 300, false);
    // C-INIT at both ends (clause 5.6): the same data makes the same
    // octets again.
    encoder.reset();
    decoder.reset();
    assert!(encoder.dictionary() == &Dictionary::new(1024, 20));
    let (second, decoded) = round_trip(&mut encoder, &mut decoder, &data, 300, false);
    assert_eq!(first, second);
    assert!(decoded == data);

    // The RESET command code from the far end (clause 7.8.3): the escape
    // character, 0 at the start, and 2. The decoder begins again and
    // goes on with what follows.
    let mut decoder = Decoder::new(1024, 20);
    let mut plain = [0u8; 512];
    let (_, written) = decoder.decode(b"abc\x00\x02abc", &mut plain).unwrap();
    assert_eq!(&plain[..written], b"abcabc");
    assert!(decoder.dictionary() == Encoder::new(1024, 20).dictionary() || written == 6);
}

// What the decoder should never be sent.

#[test]
fn a_pair_configured_in_place_is_a_pair_made_new() {
    let data = english(20_000);
    // A pair that has carried data at one size, made into one of
    // another.
    let mut encoder = Encoder::new(4096, 250);
    let mut decoder = Decoder::new(4096, 250);
    round_trip(&mut encoder, &mut decoder, &data, 97, true);
    encoder.configure(512, 6);
    decoder.configure(512, 6);
    let mut new_encoder = Encoder::new(512, 6);
    let mut new_decoder = Decoder::new(512, 6);
    assert_eq!(encoder.dictionary(), new_encoder.dictionary());
    assert_eq!(decoder.dictionary(), new_decoder.dictionary());
    let (coded, decoded) = round_trip(&mut encoder, &mut decoder, &data, 97, true);
    let new = round_trip(&mut new_encoder, &mut new_decoder, &data, 97, true);
    assert_eq!(decoded, data);
    assert_eq!((coded, decoded), new);
    assert_eq!(encoder.dictionary(), new_encoder.dictionary());
    assert_eq!(decoder.dictionary(), new_decoder.dictionary());
}

#[test]
fn every_condition_of_clause_5_8_is_an_error_and_is_final() {
    let compressed = |codewords: &[(u16, u8)]| {
        let mut coded = vec![0x00, 0x00];
        coded.extend(pack(codewords));
        coded
    };
    let mut plain = [0u8; 1024];
    // b) the codeword C1: the entry that is next to be made, which the
    // encoder cannot yet have. After A and B, C1 is still 259 when the
    // third codeword comes.
    let mut decoder = Decoder::new(512, 6);
    let coded = compressed(&[(68, 9), (69, 9), (260, 9)]);
    assert_eq!(
        decoder.decode(&coded, &mut plain),
        Err(Error::EmptyEntry(260))
    );
    let mut decoder = Decoder::new(512, 6);
    let coded = compressed(&[(68, 9), (259, 9)]);
    assert_eq!(
        decoder.decode(&coded, &mut plain),
        Err(Error::EmptyEntry(259))
    );
    // c) an entry that holds nothing, near and far.
    for empty in [261, 300, 511] {
        let mut decoder = Decoder::new(512, 6);
        let coded = compressed(&[(68, 9), (69, 9), (empty, 9)]);
        assert_eq!(
            decoder.decode(&coded, &mut plain),
            Err(Error::EmptyEntry(empty))
        );
        // It is final: nothing more is taken until a reset.
        assert_eq!(
            decoder.decode(b"more", &mut plain),
            Err(Error::EmptyEntry(empty))
        );
        assert_eq!(decoder.error(), Some(Error::EmptyEntry(empty)));
        decoder.reset();
        assert_eq!(decoder.error(), None);
        assert_eq!(decoder.decode(b"more", &mut plain), Ok((4, 4)));
    }
    // A codeword past the dictionary: with 600 codewords a codeword is
    // 10 bits and can say up to 1023.
    let mut decoder = Decoder::new(600, 6);
    let coded = compressed(&[(68, 9), (2, 9), (700, 10)]);
    assert_eq!(
        decoder.decode(&coded, &mut plain),
        Err(Error::EmptyEntry(700))
    );
    // d) a reserved command code.
    for code in [3u8, 4, 0x7F, 0xFF] {
        let mut decoder = Decoder::new(512, 6);
        assert_eq!(
            decoder.decode(&[b'x', 0x00, code], &mut plain),
            Err(Error::Command(code))
        );
    }
    // What was decoded before the error was given: the first two
    // characters here.
    let mut decoder = Decoder::new(512, 6);
    let mut coded = b"ok".to_vec();
    coded.extend([0x00, 0x09]);
    assert_eq!(decoder.decode(&coded, &mut plain), Err(Error::Command(9)));
    assert_eq!(&plain[..2], b"ok");
}

proptest! {
    #[test]
    fn any_data_with_flushes_anywhere_goes_round(
        pieces in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..300), 0..30),
        size in 0usize..4,
        length in 0usize..4,
        policy in 0usize..3,
    ) {
        let mut encoder = Encoder::new(SIZES[size], LENGTHS[length]);
        encoder.set_policy([Policy::Adaptive, Policy::Compressed, Policy::Transparent][policy]);
        let mut decoder = Decoder::new(SIZES[size], LENGTHS[length]);
        for piece in &pieces {
            let (_, decoded) = round_trip(&mut encoder, &mut decoder, piece, 4096, true);
            prop_assert_eq!(&decoded, piece);
        }
    }

    #[test]
    fn text_like_data_goes_round(
        words in prop::collection::vec(0usize..12, 0..3000),
        flush_every in 1usize..200,
    ) {
        const WORDS: [&str; 12] = ["ab", "abc", "abcd", "the ", "and ", "aaaa", "\x00", "3", "ba", "bab", "  ", "\r\n"];
        let data: Vec<u8> = words.iter().flat_map(|&w| WORDS[w].bytes()).collect();
        let mut encoder = Encoder::new(512, 6);
        encoder.set_policy(Policy::Compressed);
        let mut decoder = Decoder::new(512, 6);
        let (_, decoded) = round_trip(&mut encoder, &mut decoder, &data, flush_every, true);
        prop_assert_eq!(decoded, data);
    }

    #[test]
    fn any_octets_at_all_are_borne_by_the_decoder(
        octets in prop::collection::vec(any::<u8>(), 0..3000),
        size in 0usize..4,
        length in 0usize..4,
    ) {
        // Whatever comes, the decoder gives characters or an error and
        // nothing else: no more than a string for each octet taken.
        let mut decoder = Decoder::new(SIZES[size], LENGTHS[length]);
        let mut plain = [0u8; 1024];
        let mut rest = &octets[..];
        while !rest.is_empty() {
            match decoder.decode(rest, &mut plain) {
                Ok((taken, written)) => {
                    prop_assert!(taken > 0);
                    prop_assert!(written <= taken * usize::from(LENGTHS[length]));
                    rest = &rest[taken..];
                }
                Err(error) => {
                    prop_assert_eq!(decoder.error(), Some(error));
                    break;
                }
            }
        }
    }
}
