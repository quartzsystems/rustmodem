// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Nothing on the bit path allocates: the framing and the detection
//! phase run on a media thread, a bit at a time.

use v42::{Deframed, Deframer, Detection, Framer, Outcome, Role, T400_MS};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

#[test]
fn the_bit_path_allocates_nothing() {
    // Everything is made before the counting starts.
    let mut framer = Framer::new();
    let mut deframer = Deframer::new();
    let mut originator = Detection::new(Role::Originator, T400_MS * 8, true);
    let mut answerer = Detection::new(Role::Answerer, T400_MS * 8, true);
    let frame: Vec<u8> = (0..100).collect();

    let (allocations, (frames, outcomes)) = alloccount::count(|| {
        // The detection phase between two modems, to its end.
        for _ in 0..2000 {
            originator.advance(7);
            answerer.advance(7);
            let to_answerer = originator.next_bit();
            let to_originator = answerer.next_bit();
            answerer.receive_bit(to_answerer);
            originator.receive_bit(to_originator);
        }
        // Frames, an abort, and the fill between.
        let mut frames = 0;
        for n in 0..40_000u32 {
            if framer.ready() && n % 3 == 0 {
                framer.send(&frame[..2 + (n as usize / 7) % 90]);
            }
            if n % 9973 == 5000 {
                framer.abort();
            }
            if let Deframed::Frame(_) = deframer.process(framer.next_bit()) {
                frames += usize::from(!deframer.frame().is_empty());
            }
        }
        (frames, (originator.outcome(), answerer.outcome()))
    });

    assert_eq!(allocations, 0);
    assert!(frames > 50, "{frames} frames");
    assert_eq!(outcomes, (Outcome::ErrorControl, Outcome::ErrorControl));
    // And the counter does count.
    let (allocations, boxed) = alloccount::count(|| std::hint::black_box(Box::new(7u64)));
    assert_eq!((allocations, *boxed), (1, 7));
}
