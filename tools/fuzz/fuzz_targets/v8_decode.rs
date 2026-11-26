// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `v8_decode`: no run of received bits makes the V.8 decoder
//! or either side of the procedure panic or allocate; a menu that is
//! understood says the same thing when it is coded again; and each side
//! of the procedure always comes to an end.
//!
//! The first octet of the input chooses what each side can do. The rest
//! is the received bits, eight to an octet, as a V.21 receiver that is
//! hearing anything at all might give them.

#![no_main]

use libfuzzer_sys::fuzz_target;
use v8::{
    Answerer, Caller, Capabilities, Event, Menu, Modulation, Modulations, SequenceDecoder, Sending,
};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, octets)) = data.split_first() else {
        return;
    };
    let mut modes = Modulations::of(&[Modulation::V21]);
    if choice & 1 != 0 {
        modes = modes.with(Modulation::V34);
    }
    if choice & 2 != 0 {
        modes = modes.with(Modulation::V22bis);
    }
    let capabilities = Capabilities {
        lapm: choice & 4 != 0,
        ..Capabilities::data(modes)
    };
    let mut decoder = SequenceDecoder::new();
    let mut answerer = Answerer::new(capabilities).with_transmit_delay(64);
    let mut caller = Caller::new(capabilities).with_transmit_delay(64);

    let (allocations, ()) = alloccount::count(|| {
        // The octets as menus: whatever they are, they are read without
        // complaint, and what is understood of them survives coding.
        let menu = Menu::from_octets(octets);
        if let Some((understood, count)) = menu.capabilities() {
            let again = Menu::from_capabilities(&understood, count);
            let (back, _) = again.capabilities().expect("a coded menu is a menu");
            assert_eq!(back.call_function, understood.call_function);
            // The coding adds the V.34 bit to a menu that has either V.90
            // bit, because clause 6.3 requires the two together; nothing
            // else is added or lost.
            let v90 = understood
                .pcm
                .is_some_and(|pcm| pcm.v90_analogue || pcm.v90_digital);
            let expected = if v90 {
                understood.modulations.with(Modulation::V34)
            } else {
                understood.modulations
            };
            assert_eq!(back.modulations, expected);
            assert_eq!(back.lapm, understood.lapm);
        }

        // The octets as bits.
        for &octet in octets {
            for n in 0..8 {
                let bit = octet >> n & 1 == 1;
                let decoded = decoder.push(bit);
                if let Some(signal) = decoded.signal {
                    assert!(!signal.menu.octets().is_empty());
                    let _ = signal.menu.capabilities();
                }
                // A bit time is 26 or 27 samples.
                answerer.receive_bit(bit);
                answerer.advance(27);
                if answerer.sending() == Sending::Bits {
                    answerer.next_bit();
                }
                caller.receive_bit(bit);
                caller.advance(27);
                if caller.sending() == Sending::Bits {
                    caller.next_bit();
                }
                while answerer.poll().is_some() {}
                while caller.poll().is_some() {}
            }
        }

        // Left alone, each side ends, and says so once: the answering
        // side within its answer tone, its exchange and its silence, and
        // the calling side within its silence, its exchange and CJ.
        let mut answerer_done = 0;
        let mut caller_done = 0;
        for _ in 0..12 * 300 {
            answerer.advance(27);
            caller.advance(27);
            if answerer.sending() == Sending::Bits {
                answerer.next_bit();
            }
            if caller.sending() == Sending::Bits {
                caller.next_bit();
            }
            while let Some(event) = answerer.poll() {
                answerer_done += usize::from(matches!(event, Event::Done(_)));
            }
            while let Some(event) = caller.poll() {
                caller_done += usize::from(matches!(event, Event::Done(_)));
            }
        }
        assert!(answerer.is_done() && caller.is_done());
        assert!(answerer_done <= 1 && caller_done <= 1);
        assert_eq!(answerer.sending(), Sending::Silence);
        assert_eq!(caller.sending(), Sending::Silence);
    });
    assert_eq!(allocations, 0);
});
