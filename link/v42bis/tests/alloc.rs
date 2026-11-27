// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Encoding and decoding allocate nothing: the dictionaries are made
//! with the encoder and the decoder, and the data path runs on a media
//! thread.

use v42bis::{Decoder, Encoder, MAX_STRING, Mode, Policy};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

/// Text that compresses, then octets that do not, then text again: both
/// modes and the changes between them, and a dictionary that fills.
fn data() -> Vec<u8> {
    let words = [
        "the ", "modem ", "answers ", "a ", "call ", "and ", "sends ", "its ", "tone ", "to ",
        "line ", "frame ", "octet ", "window ",
    ];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut out = Vec::new();
    while out.len() < 60_000 {
        out.extend_from_slice(words[(next() % words.len() as u64) as usize].as_bytes());
    }
    while out.len() < 90_000 {
        out.push((next() >> 24) as u8);
    }
    while out.len() < 150_000 {
        out.extend_from_slice(words[(next() % words.len() as u64) as usize].as_bytes());
    }
    out
}

#[test]
fn the_data_path_allocates_nothing() {
    // Everything is made before the counting starts.
    let data = data();
    let mut encoder = Encoder::new(2048, 32);
    let mut decoder = Decoder::new(2048, 32);
    encoder.set_policy(Policy::Adaptive);

    let (allocations, (same, decoded, coded, modes)) = alloccount::count(|| {
        let mut same = true;
        let mut decoded = 0usize;
        let mut coded = 0usize;
        let mut modes = (false, false);
        let mut wire = [0u8; 300];
        let mut plain = [0u8; 2 * MAX_STRING as usize];
        let mut decode = |decoder: &mut Decoder, mut octets: &[u8], decoded: &mut usize| {
            let mut same = true;
            while !octets.is_empty() {
                let Ok((taken, written)) = decoder.decode(octets, &mut plain) else {
                    return false;
                };
                same &= plain[..written] == data[*decoded..*decoded + written];
                *decoded += written;
                octets = &octets[taken..];
            }
            same
        };
        let mut at = 0;
        let mut chunk = 1;
        while at < data.len() {
            // Chunks of every size from 1 to 200, as a host gives them,
            // and a flush after each, as the link does before a frame.
            let end = (at + chunk).min(data.len());
            chunk = chunk % 200 + 1;
            while at < end {
                let (taken, written) = encoder.encode(&data[at..end], &mut wire);
                at += taken;
                coded += written;
                same &= decode(&mut decoder, &wire[..written], &mut decoded);
            }
            let written = encoder.flush(&mut wire);
            coded += written;
            same &= decode(&mut decoder, &wire[..written], &mut decoded);
            match encoder.mode() {
                Mode::Compressed => modes.0 = true,
                Mode::Transparent => modes.1 = true,
            }
        }
        (same, decoded, coded, modes)
    });

    assert_eq!(allocations, 0);
    assert!(same);
    assert_eq!(decoded, data.len());
    assert!(
        coded < data.len() * 3 / 4,
        "{coded} octets for {}",
        data.len()
    );
    assert_eq!(modes, (true, true));
    // And the counter does count.
    let (allocations, boxed) = alloccount::count(|| std::hint::black_box(Box::new(7u64)));
    assert_eq!((allocations, *boxed), (1, 7));
}
