// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Fuzz target `lapm_frames`: no sequence of frames makes LAPM panic or
//! allocate, and nothing is ever given to a host that the honest machine
//! at the other end did not send.
//!
//! Two machines are joined, one of them honest and both under test. The
//! input is a script. Each step is an octet that says what happens to the
//! next frame between them, passed on, lost, passed twice or held back
//! behind the next, or that a frame of the input's own making is given
//! to one of them: any octets at all, but that a frame which is an I
//! frame is not made up, since data that was never sent is exactly what
//! a made-up I frame is. Everything else an attacker on the line could
//! send is in reach: supervisory frames with any sequence number, every
//! unnumbered frame, XID with any field, frames too short and too long.

#![no_main]

use libfuzzer_sys::fuzz_target;
use v42::Role;
use v42::hdlc::MAX_FRAME;
use v42::lapm::{Config, Lapm};

#[global_allocator]
static ALLOCATOR: alloccount::Counting = alloccount::Counting;

/// What one end has been given to send and has had delivered.
struct Host {
    sent: usize,
    received: usize,
}

/// The octet the honest data has at a place: the place itself, so that
/// what arrives can be checked without keeping it.
fn pattern(at: usize) -> u8 {
    (at as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3]
}

fuzz_target!(|data: &[u8]| {
    let config = Config {
        // Short timers, so that a script of some hundreds of steps sees
        // them run out.
        t401: 400,
        t403: 2000,
        n400: 3,
        ..Config::for_rate(2400)
    };
    // Making the machines may allocate; running them may not.
    let mut ends = [
        Lapm::new(Role::Originator, config),
        Lapm::new(Role::Answerer, config),
    ];
    let mut hosts = [Host { sent: 0, received: 0 }, Host { sent: 0, received: 0 }];
    ends[0].establish();

    let (allocations, ()) = alloccount::count(|| {
        let mut script = data.iter().copied();
        // A frame held back, and which end it was going to.
        let mut held: Option<([u8; MAX_FRAME], usize, usize)> = None;
        let mut frame = [0u8; MAX_FRAME];
        let mut made = [0u8; 160];
        while let Some(step) = script.next() {
            let from = usize::from(step & 1);
            let to = 1 - from;
            match (step >> 1) & 7 {
                // A frame of the input's own making, to one end.
                6 | 7 => {
                    let len = usize::from(script.next().unwrap_or(0)) % made.len();
                    for octet in &mut made[..len] {
                        *octet = script.next().unwrap_or(0);
                    }
                    // Not an I frame: bit 1 of the control octet is 1.
                    if len >= 2 {
                        made[1] |= 1;
                    }
                    // Nor a destructive break, which is a frame whose
                    // whole purpose is that data sent is not delivered
                    // (V.42 Tables 4 and 5): the D bit of a UI frame's
                    // second information octet is cleared.
                    if len >= 4 && made[1] & !0x10 == 0x03 {
                        made[3] &= 0x7F;
                    }
                    ends[to].receive(&made[..len]);
                }
                action => {
                    let len = ends[from].poll_transmit(&mut frame);
                    if len > 0 {
                        match action {
                            // Passed on.
                            0 | 1 | 2 => ends[to].receive(&frame[..len]),
                            // Lost.
                            3 => {}
                            // Passed twice.
                            4 => {
                                ends[to].receive(&frame[..len]);
                                ends[to].receive(&frame[..len]);
                            }
                            // Held back behind the next.
                            _ => {
                                if let Some((old, old_len, old_to)) = held.replace((frame, len, to)) {
                                    ends[old_to].receive(&old[..old_len]);
                                }
                            }
                        }
                    }
                }
            }
            // Time passes, and the hosts give and take.
            for (end, host) in ends.iter_mut().zip(&mut hosts) {
                end.advance(u32::from(step >> 4) * 40);
                let mut out = [0u8; 48];
                for (n, octet) in out.iter_mut().enumerate() {
                    *octet = pattern(host.sent + n);
                }
                host.sent += end.write(&out);
                while end.poll_event().is_some() {}
            }
            for n in 0..2 {
                let mut buf = [0u8; 256];
                let got = ends[n].read(&mut buf);
                // What is delivered is the next of what the other end
                // was given, octet for octet.
                for (k, &octet) in buf[..got].iter().enumerate() {
                    assert_eq!(octet, pattern(hosts[n].received + k));
                }
                hosts[n].received += got;
                assert!(hosts[n].received <= hosts[1 - n].sent);
            }
        }
    });
    assert_eq!(allocations, 0);
});
