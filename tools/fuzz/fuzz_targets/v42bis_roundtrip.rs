// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `v42bis_roundtrip`: whatever the data and wherever it is
//! flushed, what the decoder gives is what the encoder was given, the
//! two dictionaries are the same after every flush, and neither
//! allocates.
//!
//! The first two octets of the input choose the dictionary size, the
//! longest string, how the encoder chooses its mode, and how often it is
//! flushed. The rest is the data.

#![no_main]

use libfuzzer_sys::fuzz_target;
use v42bis::{Decoder, Encoder, Policy};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

fuzz_target!(|data: &[u8]| {
    let [choice, pace, data @ ..] = data else {
        return;
    };
    let codewords = [512u16, 600, 1024, 2048, 4096, 3000, 513, 4095][usize::from(choice & 7)];
    let string_length = [6u8, 7, 20, 32, 100, 250, 249, 8][usize::from((choice >> 3) & 7)];
    let policy = [Policy::Adaptive, Policy::Compressed, Policy::Transparent, Policy::Adaptive]
        [usize::from(choice >> 6)];
    // A flush after every so many octets, from every one to every 256.
    let piece = usize::from(*pace) + 1;

    // Making them may allocate; running them may not.
    let mut encoder = Encoder::new(codewords, string_length);
    encoder.set_policy(policy);
    let mut decoder = Decoder::new(codewords, string_length);
    let (allocations, ()) = alloccount::count(|| {
        let mut coded = [0u8; 2048];
        let mut plain = [0u8; 2048];
        let mut checked = 0usize;
        let mut check = |coded: &[u8], decoder: &mut Decoder, checked: &mut usize| {
            let mut rest = coded;
            while !rest.is_empty() {
                let (taken, written) = decoder.decode(rest, &mut plain).expect("a decoder error");
                assert!(taken > 0);
                assert_eq!(&plain[..written], &data[*checked..*checked + written]);
                *checked += written;
                rest = &rest[taken..];
            }
        };
        let mut sent = 0usize;
        for chunk in data.chunks(piece) {
            let mut rest = chunk;
            while !rest.is_empty() {
                let (taken, written) = encoder.encode(rest, &mut coded);
                assert!(taken > 0);
                check(&coded[..written], &mut decoder, &mut checked);
                rest = &rest[taken..];
            }
            sent += chunk.len();
            let written = encoder.flush(&mut coded);
            check(&coded[..written], &mut decoder, &mut checked);
            // After a flush nothing is held back, and the dictionaries
            // are the same.
            assert_eq!(checked, sent);
            assert!(encoder.dictionary() == decoder.dictionary());
            assert_eq!(encoder.mode(), decoder.mode());
        }
    });
    assert_eq!(allocations, 0);
});
