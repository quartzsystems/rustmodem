// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `v22_demod`: no run of received octets makes the 1200 and
//! 2400 bit/s demodulator panic or allocate, and what it gives stays
//! within what it may give.
//!
//! The first octet of the input chooses the channel, the law and the
//! rate. The rest are the G.711 octets of the line, as an RTP stream
//! brings them: any octet at all. An octet of the input in every 256 is
//! also taken as a command, so that the rate changes, the retrains and
//! the holds of a call come at any moment.

#![no_main]

use g711::Law;
use libfuzzer_sys::fuzz_target;
use v22::{Band, Demodulator, Rate};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, octets)) = data.split_first() else {
        return;
    };
    let band = if choice & 1 == 0 { Band::Low } else { Band::High };
    let law = if choice & 2 == 0 { Law::Ulaw } else { Law::Alaw };

    // Making a demodulator may allocate; running one may not.
    let mut demodulator = Demodulator::new(band, law);
    if choice & 4 != 0 {
        demodulator.set_rate(Rate::B2400);
    }
    let (allocations, ()) = alloccount::count(|| {
        let mut symbols = 0usize;
        let mut last_symbol: Option<usize> = None;
        for (n, &octet) in octets.iter().enumerate() {
            if n % 256 == 255 {
                match octet & 7 {
                    0 => demodulator.set_rate(Rate::B1200),
                    1 => demodulator.set_rate(Rate::B2400),
                    2 => demodulator.retrain(),
                    3 => demodulator.set_adapting(false),
                    4 => demodulator.set_adapting(true),
                    5 => demodulator.reset(),
                    _ => {}
                }
            }
            let out = demodulator.process(law.decode(octet));
            if let Some(symbol) = out.symbol {
                // A symbol comes only with a carrier, carries two bits
                // or four, and its point is a number.
                assert!(out.carrier);
                assert!(symbol.bits == 2 || symbol.bits == 4);
                assert!(symbol.line < (1 << symbol.bits));
                assert!(symbol.data < (1 << symbol.bits));
                assert!(symbol.point.is_finite());
                // Whatever the line does, no two symbols are closer than
                // half a symbol: the timing loop is held to a thousandth
                // of the rate.
                if let Some(last) = last_symbol {
                    assert!(n - last >= 6);
                }
                last_symbol = Some(n);
                symbols += 1;
            }
            assert_eq!(out.carrier, demodulator.carrier());
        }
        // No more symbols than a hundredth over the line's rate.
        assert!(symbols <= octets.len() * 606 / 8000 + 2);
        assert!(demodulator.level_dbm0().is_finite());
        assert!(demodulator.level_dbm0() < 7.0);
        assert!((0.0..=60.0).contains(&demodulator.snr_db()));
        assert!(demodulator.mse().is_finite());
        assert!(demodulator.timing_offset_ppm().abs() <= 1001.0);
        assert!(demodulator.carrier_offset_hz().abs() <= 15.01);
    });
    assert_eq!(allocations, 0);
});
