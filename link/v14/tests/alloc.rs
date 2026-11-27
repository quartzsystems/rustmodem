// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Nothing in the conversion allocates: it runs on a media thread, a bit
//! at a time.

use v14::{Format, Receiver, Transmitter};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

#[test]
fn the_bit_path_allocates_nothing() {
    for format in [Format::N81, Format::E71, Format::O71] {
        let mut transmitter = Transmitter::new(format);
        let mut receiver = Receiver::new(format);
        let data: Vec<u8> = (0..=255).collect();
        let mut received = [0u8; 1024];

        let (allocations, got) = alloccount::count(|| {
            let mut got = 0;
            let mut rest = &data[..];
            for n in 0..20_000 {
                let taken = transmitter.write(rest);
                rest = &rest[taken..];
                if n == 6000 {
                    transmitter.send_break();
                }
                let out = receiver.process(transmitter.next_bit());
                for &character in out.characters() {
                    received[got] = character;
                    got += 1;
                }
            }
            got
        });

        assert_eq!(allocations, 0, "{format:?}");
        let expected: Vec<u8> = data.iter().map(|c| c & format.mask()).collect();
        assert_eq!(&received[..got], &expected[..], "{format:?}");
        assert_eq!(receiver.breaks(), 1);
    }
    // And the counter does count.
    let (allocations, boxed) = alloccount::count(|| std::hint::black_box(Box::new(7u64)));
    assert_eq!((allocations, *boxed), (1, 7));
}
