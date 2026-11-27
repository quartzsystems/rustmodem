// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Nothing on the sample path allocates. The modem runs on a real-time
//! media thread, where an allocation is a lock and a wait.

use g711::Law;
use v22::{Band, Demodulator, GuardTone, Modulator, Rate};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

#[test]
fn the_sample_path_allocates_nothing() {
    for band in [Band::Low, Band::High] {
        // Everything is made before the counting starts: making a modem
        // may allocate, running one may not.
        let law = Law::Ulaw;
        let mut modulator = Modulator::new(band, -12.0, GuardTone::Hz1800, law);
        let mut demodulator = Demodulator::new(band, law);

        let (allocations, (symbols, snr)) = alloccount::count(|| {
            let mut state = 0x2545_F491_4F6C_DD1Du64;
            let mut symbols = 0u32;
            let mut snr = [0f32; 2];
            // Thirty seconds: a carrier at 1200 bit/s, the change to
            // 2400, a silence, the carrier again and a retrain.
            for n in 0..240_000u32 {
                match n {
                    16_000 => {
                        modulator.set_rate(Rate::B2400);
                        demodulator.set_rate(Rate::B2400);
                    }
                    // The measurements a diagnostic display reads.
                    79_999 => snr[0] = demodulator.snr_db(),
                    80_000 => modulator.set_silent(true),
                    88_000 => modulator.set_silent(false),
                    159_999 => snr[1] = demodulator.snr_db(),
                    160_000 => demodulator.retrain(),
                    200_000 => demodulator.set_adapting(false),
                    _ => {}
                }
                let sample = modulator.next_sample(|| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state & 1 == 1
                });
                let out = demodulator.process(law.decode(law.encode(sample)));
                symbols += u32::from(out.symbol.is_some());
            }
            let _ = demodulator.level_dbm0();
            let _ = demodulator.mse();
            let _ = demodulator.timing_offset_ppm();
            let _ = demodulator.carrier_offset_hz();
            demodulator.reset();
            (symbols, snr)
        });

        assert_eq!(allocations, 0, "{band:?}");
        // It was a call that was counted: all but the second of silence
        // was received, and the points were tight before the silence and
        // after it.
        assert!(symbols > 17_000, "{band:?}: {symbols} symbols");
        assert!(snr[0] > 30.0 && snr[1] > 30.0, "{band:?}: {snr:?} dB");
    }
    // And the counter does count.
    let (allocations, boxed) = alloccount::count(|| std::hint::black_box(Box::new(7u64)));
    assert_eq!((allocations, *boxed), (1, 7));
}
