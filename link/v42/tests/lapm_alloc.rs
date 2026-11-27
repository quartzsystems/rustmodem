// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The protocol allocates nothing once it is made: it runs on a media
//! thread, between the bits of the line and the octets of the host.

use v42::Role;
use v42::hdlc::MAX_FRAME;
use v42::lapm::{Config, Event, Lapm, State};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

/// What one end did while the count ran.
#[derive(Default)]
struct Tally {
    written: usize,
    read: usize,
    sum_written: u64,
    sum_read: u64,
    established: bool,
    released: bool,
    breaks: usize,
}

/// One step of an end: the clock, the host, and a frame to the other.
fn step(
    end: &mut Lapm,
    other: &mut Lapm,
    tally: &mut Tally,
    data: &[u8],
    total: usize,
    lose: bool,
) {
    end.advance(40);
    if tally.written < total {
        let from = tally.written % (data.len() - 64);
        let want = 64.min(total - tally.written);
        let taken = end.write(&data[from..from + want]);
        for &octet in &data[from..from + taken] {
            tally.sum_written += u64::from(octet);
        }
        tally.written += taken;
    }
    let mut buf = [0u8; 256];
    loop {
        let n = end.read(&mut buf);
        if n == 0 {
            break;
        }
        tally.read += n;
        for &octet in &buf[..n] {
            tally.sum_read += u64::from(octet);
        }
    }
    while let Some(event) = end.poll_event() {
        match event {
            Event::Established(_) => tally.established = true,
            Event::Released(_) => tally.released = true,
            Event::Break { .. } => tally.breaks += 1,
        }
    }
    let mut frame = [0u8; MAX_FRAME];
    let len = end.poll_transmit(&mut frame);
    if len > 0 && !lose {
        other.receive(&frame[..len]);
    }
}

#[test]
fn the_protocol_allocates_nothing() {
    // Everything is made before the counting starts.
    let mut a = Lapm::new(Role::Originator, Config::for_rate(2400));
    let mut b = Lapm::new(Role::Answerer, Config::for_rate(2400));
    let data: Vec<u8> = (0..4096u32).map(|n| (n * 31 + n / 7) as u8).collect();
    let total = 60_000;

    let (allocations, (at_a, at_b, states)) = alloccount::count(|| {
        let mut at_a = Tally::default();
        let mut at_b = Tally::default();
        a.establish();
        // Data both ways, with a frame in every 23 lost, so that the
        // recovery is counted too; then a break and the release.
        for n in 0..40_000u32 {
            let lose = n % 23 == 11;
            step(&mut a, &mut b, &mut at_a, &data, total, lose);
            step(&mut b, &mut a, &mut at_b, &data, total, lose);
            if at_a.read == total && at_b.read == total && a.drained() && b.drained() {
                break;
            }
        }
        // The break after the data, since a break may discard what is
        // on its way.
        a.send_break(3);
        for _ in 0..2000 {
            step(&mut a, &mut b, &mut at_a, &data, total, false);
            step(&mut b, &mut a, &mut at_b, &data, total, false);
        }
        a.release();
        for _ in 0..2000 {
            step(&mut a, &mut b, &mut at_a, &data, total, false);
            step(&mut b, &mut a, &mut at_b, &data, total, false);
        }
        (at_a, at_b, (a.state(), b.state()))
    });

    assert_eq!(allocations, 0);
    assert!(at_a.established && at_b.established);
    assert_eq!((at_a.written, at_b.written), (total, total));
    assert_eq!((at_a.read, at_b.read), (total, total));
    assert_eq!(at_a.sum_read, at_b.sum_written);
    assert_eq!(at_b.sum_read, at_a.sum_written);
    assert_eq!((at_a.breaks, at_b.breaks), (0, 1));
    assert!(at_a.released && at_b.released);
    assert_eq!(states, (State::Disconnected, State::Disconnected));
    // And the counter does count.
    let (allocations, boxed) = alloccount::count(|| std::hint::black_box(Box::new(7u64)));
    assert_eq!((allocations, *boxed), (1, 7));
}
