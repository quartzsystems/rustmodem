// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Nothing on the sample path allocates. The modem runs on a real-time
//! media thread, where an allocation is a lock and a wait.

use fsk::{Band, Modulator, Receiver, Standard, StartStopTransmitter, Tones};
use g711::Law;

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

#[test]
fn the_sample_path_allocates_nothing() {
    for standard in [Standard::V21, Standard::Bell103] {
        // Everything is made before the counting starts: making a modem
        // may allocate, running one may not.
        let law = Law::Ulaw;
        let mut transmitter = StartStopTransmitter::new();
        let mut modulator = Modulator::new(Tones::of(standard, Band::Caller), -12.0, law);
        let mut receiver = Receiver::new(standard, Band::Caller, 100, law);
        let data: Vec<u8> = (0..=255).collect();
        let mut received = [0u8; 4096];

        let (allocations, got) = alloccount::count(|| {
            let mut got = 0;
            let mut rest = &data[..];
            // Thirty seconds: a carrier, 256 characters, and the carrier
            // going.
            for n in 0..240_000 {
                // The data waits half a second, for the far end's line
                // signal detector.
                if n >= 4000 {
                    let taken = transmitter.write(rest);
                    rest = &rest[taken..];
                }
                let sample = if n < 200_000 {
                    modulator.next_sample(|| transmitter.next_bit())
                } else {
                    0.0
                };
                let out = receiver.process(law.decode(law.encode(sample)));
                if let Some(character) = out.framed.character {
                    received[got] = character;
                    got += 1;
                }
            }
            // The measurements a diagnostic display reads.
            let _ = receiver.demodulator().level_dbm0();
            let _ = receiver.demodulator().frequency_error_hz();
            let _ = receiver.eye_opening();
            receiver.reset();
            modulator.restart();
            transmitter.clear();
            got
        });

        assert_eq!(allocations, 0, "{standard:?}");
        assert_eq!(&received[..got], &data[..], "{standard:?}");
    }
    // And the counter does count.
    let (allocations, boxed) = alloccount::count(|| std::hint::black_box(Box::new(7u64)));
    assert_eq!((allocations, *boxed), (1, 7));
}
