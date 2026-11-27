// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! No received octets, in any state, make the modem engine panic or do
//! what an engine may not.
//!
//! A real call is run for a time chosen by the test, which leaves the
//! engine in whichever state that time falls in: the silence, the answer
//! tone, the V.8 exchange, the fallback, the carriers or the data. Then
//! what the engine receives is whatever the test likes.

use engine::{
    CallSettings, EngineState, Law, Modem, ModemConfig, ModemEngine, ModemEvent, Standard,
};
use proptest::prelude::*;

fn modem(first: bool, second: bool, bell_first: bool, v8: bool) -> ModemConfig {
    let (a, b) = if bell_first {
        (Standard::Bell103, Standard::V21)
    } else {
        (Standard::V21, Standard::Bell103)
    };
    ModemConfig {
        modulations: [first.then_some(a), second.then_some(b)],
        v8,
        // A short fallback, so that the states after it are reached.
        fallback_ms: 800,
        ..ModemConfig::default()
    }
}

/// What the engine under test receives once the call is taken away.
#[derive(Clone, Debug)]
enum Noise {
    /// Any octets at all.
    Octets(Vec<u8>),
    /// One octet over and over.
    Constant(u8, u16),
    /// Its own transmission, a tick late: a loud echo and nothing else.
    Echo(u16),
}

fn noise() -> impl Strategy<Value = Noise> {
    prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..4000).prop_map(Noise::Octets),
        (any::<u8>(), 0u16..400).prop_map(|(octet, ticks)| Noise::Constant(octet, ticks)),
        (0u16..400).prop_map(Noise::Echo),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn nothing_received_in_any_state_breaks_the_engine(
        answers in any::<bool>(),
        (first, second, bell_first, v8) in any::<(bool, bool, bool, bool)>(),
        (far_first, far_second, far_v8) in any::<(bool, bool, bool)>(),
        alaw in any::<bool>(),
        frame_20ms in any::<bool>(),
        s9 in 1u32..30,
        s10 in 1u32..50,
        real_ticks in 0usize..1400,
        noises in proptest::collection::vec(noise(), 1..4),
        writes in proptest::collection::vec(any::<u8>(), 0..600),
    ) {
        let law = if alaw { Law::Alaw } else { Law::Ulaw };
        let frame = if frame_20ms { 160 } else { 80 };
        let settings = CallSettings {
            carrier_response_ms: s9 * 100,
            carrier_loss_ms: s10 * 100,
            ..CallSettings::default()
        };
        let mut under_test = Modem::new(modem(first, second, bell_first, v8));
        let mut far = Modem::new(modem(far_first, far_second, false, far_v8));
        for engine in [&mut under_test, &mut far] {
            engine.set_law(law);
            engine.set_call(&settings);
        }
        if answers {
            under_test.answer();
            far.originate();
        } else {
            under_test.originate();
            far.answer();
        }

        let connects = std::cell::Cell::new(0);
        let losses = std::cell::Cell::new(0);
        let tick = |engine: &mut Modem, rx: &[u8], tx: &mut [u8], writes: &[u8]| {
            let taken = engine.dte_write(writes);
            assert!(taken <= writes.len());
            // The frame is filled with something else first, to see that
            // the engine writes all of it.
            tx.fill(0x2A);
            let stopped = |state| matches!(state, EngineState::Idle | EngineState::Ended);
            let before = engine.diagnostics().unwrap().state;
            engine.process(rx, tx);
            let state = engine.diagnostics().unwrap().state;
            // A stopped engine sends the octet of silence. The frame in
            // which it stops has its last signal and then silence.
            if stopped(before) && stopped(state) {
                assert!(tx.iter().all(|&octet| octet == law.silence()));
            }
            if stopped(state) {
                assert_eq!(tx.last(), Some(&law.silence()));
            }
            let mut buffer = [0u8; 97];
            let read = engine.dte_read(&mut buffer);
            assert!(read <= buffer.len());
            while let Some(event) = engine.poll_event() {
                match event {
                    ModemEvent::Connect(info) => {
                        connects.set(connects.get() + 1);
                        assert_eq!(info.rate, 300);
                        assert!(matches!(info.carrier.as_deref(), Some("V.21" | "BELL103")));
                        assert_eq!(state, EngineState::Data);
                    }
                    ModemEvent::CarrierLost => {
                        losses.set(losses.get() + 1);
                        assert_eq!(state, EngineState::Ended);
                    }
                    other => panic!("an event the 300 bit/s modems do not have: {other:?}"),
                }
            }
            let diagnostics = engine.diagnostics().unwrap();
            assert!(diagnostics.rx_level_dbm0.is_finite() && diagnostics.rx_level_dbm0 < 4.0);
            assert!(diagnostics.tx_level_dbm0.is_finite() && diagnostics.tx_level_dbm0 < 4.0);
            assert!(diagnostics.frequency_error_hz.abs() <= 150.0);
            assert!((0.0..=1.0).contains(&diagnostics.quality));
            assert!(!diagnostics.history().is_empty());
            taken
        };

        // The real call, for as long as the test says.
        let mut to_far = vec![law.silence(); frame];
        let mut to_test = vec![law.silence(); frame];
        let mut from_test = vec![0u8; frame];
        let mut from_far = vec![0u8; frame];
        let mut written = 0;
        for _ in 0..real_ticks * 80 / frame {
            written += tick(&mut under_test, &to_test, &mut from_test, &writes[written..]);
            far.process(&to_far, &mut from_far);
            while far.poll_event().is_some() {}
            let mut sink = [0u8; 64];
            far.dte_read(&mut sink);
            std::mem::swap(&mut to_far, &mut from_test);
            std::mem::swap(&mut to_test, &mut from_far);
        }

        // And then anything.
        let mut rx = vec![law.silence(); frame];
        for noise in &noises {
            match noise {
                Noise::Octets(octets) => {
                    for chunk in octets.chunks(frame) {
                        // The last chunk may be short: the engine is to
                        // bear slices of unequal length.
                        written += tick(&mut under_test, chunk, &mut from_test, &writes[written..]);
                    }
                }
                Noise::Constant(octet, ticks) => {
                    rx.fill(*octet);
                    for _ in 0..*ticks {
                        written += tick(&mut under_test, &rx, &mut from_test, &writes[written..]);
                    }
                }
                Noise::Echo(ticks) => {
                    for _ in 0..*ticks {
                        rx.copy_from_slice(&from_test);
                        written += tick(&mut under_test, &rx, &mut from_test, &writes[written..]);
                    }
                }
            }
        }

        // A call connects once at the most and ends once at the most.
        prop_assert!(connects.get() <= 1 && losses.get() <= 1);
        // Stopped, it is as it was before the call.
        under_test.hangup();
        prop_assert_eq!(under_test.poll_event(), None);
        prop_assert_eq!(under_test.dte_write(b"x"), 0);
        prop_assert_eq!(under_test.diagnostics().unwrap().state, EngineState::Idle);
        tick(&mut under_test, &rx, &mut from_test, &[]);
        prop_assert!(from_test.iter().all(|&octet| octet == law.silence()));
    }
}
