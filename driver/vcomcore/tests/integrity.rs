// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! No octet is lost, repeated or put out of order, whatever the two sides
//! do and in whatever order: reads, writes, cancellations, timeouts and
//! aborts leave both streams whole, and a purge or a close only ever
//! takes octets out.

use proptest::prelude::*;
use vcomcore::{Completion, Done, Port, QueueSize, RequestId, Timeouts, purge};

#[derive(Clone, Debug)]
enum Op {
    HostWrite(Vec<u8>),
    HostRead(usize),
    ControlWrite(Vec<u8>),
    ControlRead(usize),
    /// Moves the clock.
    Wait(u64),
    /// Cancels one of the requests made so far, by its place.
    Cancel(usize),
    Timeouts(Timeouts),
    QueueSize(u32, u32),
    Purge(u32),
    HostReopen,
    ControlReopen,
}

fn timeout_value() -> impl Strategy<Value = u32> {
    prop_oneof![Just(0), Just(u32::MAX), 1..40u32]
}

fn timeouts() -> impl Strategy<Value = Timeouts> {
    (
        timeout_value(),
        timeout_value(),
        prop_oneof![Just(0), 1..40u32],
        0..3u32,
        0..40u32,
    )
        .prop_map(|(ri, rm, rc, wm, wc)| Timeouts {
            read_interval: ri,
            read_total_multiplier: rm,
            read_total_constant: rc,
            write_total_multiplier: wm,
            write_total_constant: wc,
        })
}

fn data() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..40),
        proptest::collection::vec(any::<u8>(), 100..400),
    ]
}

/// What both sides do when nothing is thrown away.
fn gentle_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => data().prop_map(Op::HostWrite),
        4 => (0..300usize).prop_map(Op::HostRead),
        4 => data().prop_map(Op::ControlWrite),
        4 => (0..300usize).prop_map(Op::ControlRead),
        3 => (0..60u64).prop_map(Op::Wait),
        2 => (0..64usize).prop_map(Op::Cancel),
        1 => timeouts().prop_map(Op::Timeouts),
        1 => (0..600u32, 0..600u32).prop_map(|(i, o)| Op::QueueSize(i, o)),
        1 => prop_oneof![Just(purge::TXABORT), Just(purge::RXABORT), Just(purge::TXABORT | purge::RXABORT)]
            .prop_map(Op::Purge),
    ]
}

/// And when it may be.
fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        20 => gentle_op(),
        1 => (1..16u32).prop_map(Op::Purge),
        1 => Just(Op::HostReopen),
        1 => Just(Op::ControlReopen),
    ]
}

/// What was offered and what came out, each way.
#[derive(Default)]
struct Streams {
    /// For each host write, in order, its octets.
    host_writes: Vec<(RequestId, Vec<u8>, Option<usize>)>,
    /// What the control side read, in order.
    to_control: Vec<u8>,
    /// What the port took from the control side for the host, in order.
    from_control: Vec<u8>,
    /// What the host read, in order.
    to_host: Vec<u8>,
}

impl Streams {
    fn take(&mut self, port: &mut Port) {
        while let Some(ended) = port.take_completion() {
            match ended {
                Completion::HostWrite { id, written, .. } => {
                    let write = self
                        .host_writes
                        .iter_mut()
                        .find(|w| w.0 == id)
                        .expect("a write that was made");
                    assert!(write.2.is_none(), "a write ended twice");
                    assert!(written <= write.1.len());
                    write.2 = Some(written);
                }
                Completion::HostRead { done, data, .. } => {
                    assert!(done != Done::Cancelled || data.is_empty());
                    self.to_host.extend(data);
                }
                Completion::ControlRead { data, .. } => self.to_control.extend(data),
                Completion::HostWait { .. } | Completion::ControlWait { .. } => {}
            }
        }
    }

    /// What the host's writes sent, by what each said it sent.
    fn sent_by_host(&self) -> Vec<u8> {
        let mut sent = Vec::new();
        for (_, data, written) in &self.host_writes {
            sent.extend(&data[..written.expect("every write has ended")]);
        }
        sent
    }
}

/// Runs the operations, then lets everything that is still on its way
/// arrive, and gives the streams.
fn run(ops: &[Op]) -> Streams {
    let mut port = Port::new();
    port.control_open(0).unwrap();
    port.host_open(0).unwrap();
    let mut streams = Streams::default();
    let mut now = 0u64;
    let mut next = 0u64;
    let mut id = || {
        next += 1;
        RequestId(next)
    };
    for op in ops {
        match op.clone() {
            Op::HostWrite(data) => {
                let id = id();
                streams.host_writes.push((id, data.clone(), None));
                port.host_write(id, &data, now).unwrap();
            }
            Op::HostRead(want) => port.host_read(id(), want, now).unwrap(),
            Op::ControlWrite(data) => {
                let taken = port.control_write(&data, now).unwrap();
                streams.from_control.extend(&data[..taken]);
            }
            Op::ControlRead(want) => port.control_read(id(), want, now).unwrap(),
            Op::Wait(by) => {
                now += by;
                port.advance(now);
            }
            Op::Cancel(which) => port.cancel(RequestId(which as u64), now),
            Op::Timeouts(value) => {
                let _ = port.set_timeouts(value);
            }
            Op::QueueSize(input, output) => port.set_queue_size(QueueSize { input, output }),
            Op::Purge(flags) => port.purge(flags, now).unwrap(),
            Op::HostReopen => {
                port.host_close(now);
                port.host_open(now).unwrap();
            }
            Op::ControlReopen => {
                port.control_close(now);
                port.control_open(now).unwrap();
            }
        }
        streams.take(&mut port);
        if let Some(deadline) = port.next_deadline() {
            assert!(deadline > now, "a deadline that has passed was left");
        }
    }

    // Let the rest through: no timeouts, reads from both sides until
    // nothing more comes.
    port.purge(purge::RXABORT, now).unwrap();
    port.set_timeouts(Timeouts {
        read_interval: u32::MAX,
        ..Timeouts::default()
    })
    .unwrap();
    loop {
        streams.take(&mut port);
        let before = streams.to_control.len() + streams.to_host.len();
        port.host_read(id(), 1000, now).unwrap();
        let reader = id();
        port.control_read(reader, 1000, now).unwrap();
        port.cancel(reader, now);
        streams.take(&mut port);
        if streams.to_control.len() + streams.to_host.len() == before {
            break;
        }
    }
    assert!(
        streams.host_writes.iter().all(|w| w.2.is_some()),
        "a write never ended"
    );
    streams
}

/// Whether `part` is `whole` with octets taken out and nothing else done.
fn is_subsequence(part: &[u8], whole: &[u8]) -> bool {
    let mut rest = whole.iter();
    part.iter().all(|octet| rest.any(|other| other == octet))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn with_nothing_thrown_away_both_streams_arrive_whole(
        ops in proptest::collection::vec(gentle_op(), 0..120),
    ) {
        let streams = run(&ops);
        prop_assert!(streams.to_control == streams.sent_by_host(), "from the host");
        prop_assert!(streams.to_host == streams.from_control, "to the host");
    }

    #[test]
    fn a_purge_or_a_close_only_takes_octets_out(
        ops in proptest::collection::vec(any_op(), 0..120),
    ) {
        let streams = run(&ops);
        prop_assert!(is_subsequence(&streams.to_control, &streams.sent_by_host()), "from the host");
        prop_assert!(is_subsequence(&streams.to_host, &streams.from_control), "to the host");
    }
}
