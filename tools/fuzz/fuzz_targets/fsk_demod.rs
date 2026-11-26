// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `fsk_demod`: no run of received octets makes the 300 bit/s
//! receiver panic or allocate, and what it gives stays within what it may
//! give.
//!
//! The first octet of the input chooses the modem, the band and the law.
//! The rest are the G.711 octets of the line, as an RTP stream brings
//! them: any octet at all.

#![no_main]

use fsk::{Band, Receiver, Standard};
use g711::Law;
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, octets)) = data.split_first() else {
        return;
    };
    let standard = if choice & 1 == 0 {
        Standard::V21
    } else {
        Standard::Bell103
    };
    let band = if choice & 2 == 0 {
        Band::Caller
    } else {
        Band::Answerer
    };
    let law = if choice & 4 == 0 { Law::Ulaw } else { Law::Alaw };
    // A detector that comes on in 10 ms or in 100, so that short inputs
    // reach the framing too.
    let carrier_on_ms = if choice & 8 == 0 { 10 } else { 100 };

    // Making a receiver may allocate; running one may not.
    let mut receiver = Receiver::new(standard, band, carrier_on_ms, law);
    let (allocations, ()) = alloccount::count(|| {
        let mut characters = 0usize;
        let mut bits = 0usize;
        for &octet in octets {
            let out = receiver.process(law.decode(octet));
            assert!(out.soft.is_finite() && (-1.0..=1.0).contains(&out.soft));
            if out.framed.character.is_some() {
                // A character comes with its stop bit and only with a
                // carrier.
                assert_eq!(out.framed.bit, Some(true));
                assert!(out.carrier);
                characters += 1;
            }
            if out.framed.bit.is_some() {
                bits += 1;
            }
            assert!(!(out.framed.framing_error && out.framed.character.is_some()));
        }
        // No more bits than the line rate allows, and ten to a
        // character.
        assert!(bits <= octets.len() * 300 / 8000 + 2);
        assert!(characters * 10 <= bits + 10);
        assert!(receiver.demodulator().level_dbm0() < 4.0);
        assert!(receiver.demodulator().frequency_error_hz().abs() <= 150.0);
        assert!((0.0..=1.0).contains(&receiver.eye_opening()));
    });
    assert_eq!(allocations, 0);
});
