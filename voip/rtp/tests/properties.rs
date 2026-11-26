// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The invariant of the receive session: for any sequence of arrivals, the
//! frames delivered are the frames the far end's sample clock produced
//! between the first and the last delivered packet. The sample count is
//! never lost and never invented.

use proptest::prelude::*;
use rtp::{Dropped, RtpHeader, RtpReceiver, Verdict};

/// One packet of a far end's stream.
#[derive(Clone, Copy, Debug)]
struct Sent {
    header: RtpHeader,
    /// The frames in its payload.
    frames: u32,
    /// The far end's sample clock at its first sample, in frames since the
    /// stream began.
    clock: u32,
}

/// A far end's stream of at most 2.7 seconds, so that no loss can look like
/// a new stream: packets of 10, 20 or 30 ms from any starting point of the
/// sequence number and the timestamp.
fn stream() -> impl Strategy<Value = Vec<Sent>> {
    (
        any::<u16>(),
        any::<u32>(),
        any::<u32>(),
        proptest::collection::vec(1u32..=3, 1..90),
    )
        .prop_map(|(sequence, timestamp, ssrc, sizes)| {
            let mut clock = 0;
            sizes
                .iter()
                .enumerate()
                .map(|(n, frames)| {
                    let sent = Sent {
                        header: RtpHeader {
                            marker: n == 0,
                            payload_type: 8,
                            sequence: sequence.wrapping_add(n as u16),
                            timestamp: timestamp.wrapping_add(80 * clock),
                            ssrc,
                        },
                        frames: *frames,
                        clock,
                    };
                    clock += frames;
                    sent
                })
                .collect()
        })
}

/// A stream, and its packets as a network might deliver them: any of them
/// lost, any of them more than once, in any order.
fn arrivals() -> impl Strategy<Value = (Vec<Sent>, Vec<usize>)> {
    stream().prop_flat_map(|sent| {
        let n = sent.len();
        let order = proptest::collection::vec(0..n, 1..2 * n + 1);
        (Just(sent), order)
    })
}

/// A stream with loss and duplication but every packet in its place.
fn arrivals_in_order() -> impl Strategy<Value = (Vec<Sent>, Vec<usize>)> {
    arrivals().prop_map(|(sent, mut order)| {
        order.sort_unstable();
        (sent, order)
    })
}

/// Feeds the arrivals to a receiver. Returns the frames delivered, and the
/// index of the first and of the last packet delivered.
fn run(
    receiver: &mut RtpReceiver,
    sent: &[Sent],
    order: &[usize],
) -> Result<(u64, usize, usize), TestCaseError> {
    let mut delivered = 0u64;
    let mut first = None;
    let mut last = 0;
    for (step, index) in order.iter().enumerate() {
        let packet = &sent[*index];
        let verdict = receiver.receive(&packet.header, 80 * packet.frames as usize, step as u32);
        match verdict {
            Verdict::Deliver {
                fill_frames,
                frames,
                resync,
            } => {
                prop_assert!(!resync, "a stream this short never resynchronises");
                prop_assert_eq!(frames, packet.frames);
                delivered += u64::from(fill_frames) + u64::from(frames);
                if first.is_none() {
                    prop_assert_eq!(fill_frames, 0, "nothing is filled before the first packet");
                    first = Some(*index);
                } else {
                    prop_assert!(*index > last, "a packet is never delivered out of order");
                }
                last = *index;
            }
            Verdict::Drop(Dropped::Ignored) => prop_assert!(false, "the payload type is ours"),
            Verdict::Drop(_) => {
                prop_assert!(first.is_some(), "the first arrival is always delivered");
                prop_assert!(*index <= last, "only what is not newer is dropped");
            }
        }
    }
    Ok((delivered, first.expect("there is an arrival"), last))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn the_frames_delivered_are_the_frames_the_far_end_produced((sent, order) in arrivals()) {
        let mut receiver = RtpReceiver::new(8);
        let (delivered, first, last) = run(&mut receiver, &sent, &order)?;

        // The far end's clock from the first sample of the first delivered
        // packet to the last sample of the last one.
        let produced = sent[last].clock + sent[last].frames - sent[first].clock;
        prop_assert_eq!(delivered, u64::from(produced));

        let stats = receiver.stats();
        prop_assert_eq!(stats.resync, 0);
        prop_assert_eq!(stats.ignored, 0);
        // Every arrival is counted exactly once.
        prop_assert_eq!(stats.packets + stats.late + stats.duplicate, order.len() as u64);
        // Every packet between the first and the last was delivered or lost.
        prop_assert_eq!(stats.packets + stats.lost, (last - first + 1) as u64);
        prop_assert_eq!(stats.octets + 80 * stats.fill_frames, 80 * delivered);
    }

    #[test]
    fn without_reordering_only_what_did_not_arrive_is_filled((sent, order) in arrivals_in_order()) {
        let mut receiver = RtpReceiver::new(8);
        let (_, first, last) = run(&mut receiver, &sent, &order)?;

        let mut arrived = vec![false; sent.len()];
        for index in &order {
            arrived[*index] = true;
        }
        let missing: u32 = (first..=last).filter(|i| !arrived[*i]).map(|i| sent[i].frames).sum();
        let stats = receiver.stats();
        prop_assert_eq!(stats.fill_frames, u64::from(missing));
        prop_assert_eq!(stats.late, 0);
        let copies = order.len() - arrived.iter().filter(|a| **a).count();
        prop_assert_eq!(stats.duplicate, copies as u64);
    }

    #[test]
    fn any_headers_at_all_never_panic_and_never_fill_more_than_three_seconds(
        headers in proptest::collection::vec(
            (any::<bool>(), 0u8..3, any::<u16>(), any::<u32>(), 0u32..3, 0usize..400, any::<u32>()),
            0..200,
        )
    ) {
        let mut receiver = RtpReceiver::new(0);
        let mut expected_packets = 0;
        for (marker, payload_type, sequence, timestamp, ssrc, len, arrival) in headers {
            let header = RtpHeader { marker, payload_type, sequence, timestamp, ssrc };
            match receiver.receive(&header, len, arrival) {
                Verdict::Deliver { fill_frames, frames, resync } => {
                    expected_packets += 1;
                    prop_assert!(fill_frames <= 300);
                    prop_assert_eq!(frames as usize * 80, len);
                    prop_assert!(!(resync && fill_frames != 0));
                    prop_assert_eq!(payload_type, 0);
                }
                Verdict::Drop(_) => {}
            }
        }
        prop_assert_eq!(receiver.stats().packets, expected_packets);
    }
}
