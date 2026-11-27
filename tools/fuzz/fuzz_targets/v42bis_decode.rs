// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `v42bis_decode`: no stream of octets makes the V.42bis
//! decoder panic, allocate, loop, or give more than a string for each
//! octet it takes.
//!
//! The first octet of the input chooses the dictionary size and the
//! longest string. The rest is what the far end's encoder is supposed to
//! have sent: any octets at all, which is escape sequences and codewords
//! of every kind, in and out of order.

#![no_main]

use libfuzzer_sys::fuzz_target;
use v42bis::{Decoder, Mode};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, octets)) = data.split_first() else {
        return;
    };
    let codewords = [512u16, 600, 1024, 2048, 4096, 3000, 513, 4095][usize::from(choice & 7)];
    let string_length = [6u8, 7, 20, 32, 100, 250, 249, 8][usize::from((choice >> 3) & 7)];

    // Making a decoder may allocate; running one may not.
    let mut decoder = Decoder::new(codewords, string_length);
    let (allocations, ()) = alloccount::count(|| {
        let mut out = [0u8; 1024];
        let mut rest = octets;
        let mut written_in_all = 0usize;
        let mut taken_in_all = 0usize;
        while !rest.is_empty() {
            match decoder.decode(rest, &mut out) {
                Ok((taken, written)) => {
                    // Room for a string is all it needs to go on.
                    assert!(taken > 0);
                    assert!(written <= taken * usize::from(string_length));
                    taken_in_all += taken;
                    written_in_all += written;
                    rest = &rest[taken..];
                }
                Err(error) => {
                    // An error is final until a reset.
                    assert_eq!(decoder.error(), Some(error));
                    assert_eq!(decoder.decode(rest, &mut out), Err(error));
                    break;
                }
            }
        }
        assert!(written_in_all <= taken_in_all * usize::from(string_length));
        // The dictionary is still one: its next empty entry is an entry.
        let dictionary = decoder.dictionary();
        assert!(dictionary.next_empty() >= 259 && dictionary.next_empty() < dictionary.size());
        let _ = decoder.mode() == Mode::Compressed;
        decoder.reset();
        assert_eq!(decoder.error(), None);
        assert_eq!(decoder.decode(b"a", &mut out), Ok((1, 1)));
    });
    assert_eq!(allocations, 0);
});
