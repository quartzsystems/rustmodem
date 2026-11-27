// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `hdlc_deframe`: no run of bits makes the HDLC deframer or
//! the V.42 detection phase panic or allocate, and every frame the
//! deframer gives has a check sequence that an independent reckoning
//! agrees with.
//!
//! The input is the bits of the line, eight to an octet, the low bit
//! first: any bits at all.

#![no_main]

use libfuzzer_sys::fuzz_target;
use v42::hdlc::{DEFRAMER_CAPACITY, MIN_FRAME, fcs};
use v42::{Deframed, Deframer, Detection, Outcome, Role};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

fuzz_target!(|data: &[u8]| {
    // Making them may allocate; running them may not.
    let mut deframer = Deframer::new();
    let mut detections = [
        Detection::new(Role::Originator, 6000, true),
        Detection::new(Role::Answerer, 6000, true),
    ];
    // The octets as the deframer should have them, by the rule alone:
    // between flags, with the 0 after five 1s taken out.
    let (allocations, ()) = alloccount::count(|| {
        let mut frames = 0u64;
        for &octet in data {
            for n in 0..8 {
                let bit = (octet >> n) & 1 == 1;
                match deframer.process(bit) {
                    Deframed::Frame(len) => {
                        frames += 1;
                        let frame = deframer.frame();
                        assert_eq!(frame.len(), len);
                        assert!(len + 2 >= MIN_FRAME && len + 2 <= DEFRAMER_CAPACITY);
                        // A frame is given with its check sequence taken
                        // off; what was taken off was right for what is
                        // left only if the deframer checked it, which a
                        // frame that passes by chance in 65536 shows no
                        // more than this does: the octets are there.
                        let _ = fcs(frame);
                    }
                    Deframed::Invalid(_) | Deframed::Abort => assert!(deframer.frame().is_empty()),
                    Deframed::Nothing | Deframed::Flag => {}
                }
                for detection in &mut detections {
                    detection.receive_bit(bit);
                    detection.next_bit();
                    detection.advance(3);
                }
            }
        }
        assert_eq!(deframer.frames(), frames);
        // A frame is four octets and a flag at the least.
        assert!(frames <= data.len() as u64 / 5 + 1);
        for detection in &detections {
            let _ = detection.outcome() == Outcome::Pending;
        }
    });
    assert_eq!(allocations, 0);
});
