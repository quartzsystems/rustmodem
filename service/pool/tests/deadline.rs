// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! When the pool says it is next due, and that a pool served only then
//! keeps its timers.

mod common;

use common::Harness;
use ctlproto::LineState;
use pool::{RING_OFF_MS, RING_ON_MS, TICK_MS};

const RING: &str = "\r\nRING\r\n";

#[test]
fn the_pool_is_next_due_at_its_tick() {
    let mut h = Harness::new(2);
    let due = h.pool.next_deadline();
    assert!(due > h.mono_ms && due <= h.mono_ms + TICK_MS, "{due}");

    // A turn before the tick, for something that arrived, moves nothing.
    h.mono_ms = due - 1;
    h.service();
    assert_eq!(h.pool.next_deadline(), due);
    // The tick does.
    h.mono_ms = due;
    h.service();
    assert_eq!(h.pool.next_deadline(), due + TICK_MS);
}

#[test]
fn a_pool_served_only_when_it_is_due_keeps_the_ring_cadence() {
    let mut h = Harness::new(2);
    h.quiet(1);
    h.quiet(2);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), RING);
    assert_eq!(h.state(2), LineState::Ringing);
    let first = h.mono_ms;

    // From here the clock goes from one deadline to the next and the pool
    // is served at each, as its task serves it when nothing arrives. The
    // rings that follow the first are the ones timed.
    let mut rings = Vec::new();
    while rings.len() < 3 {
        let due = h.pool.next_deadline();
        assert!(due > h.mono_ms, "the deadline is ahead");
        h.mono_ms = due;
        h.service();
        if h.read(2) == RING {
            rings.push(h.mono_ms);
        }
        assert!(h.mono_ms < first + 60_000, "the line stopped ringing");
    }
    let cadence = RING_ON_MS + RING_OFF_MS;
    for pair in rings.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= cadence && gap < cadence + 2 * TICK_MS,
            "rings {gap} ms apart: {rings:?}"
        );
    }
}
