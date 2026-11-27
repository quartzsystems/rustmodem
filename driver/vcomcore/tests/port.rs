// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The port's rules, each against a clock the test moves. The timeout
//! cases are those of the Win32 documentation of `COMMTIMEOUTS`, and the
//! event cases those of `SetCommMask` and `WaitCommEvent`.

use vcomcore::{
    Chars, Completion, ControlEvent, Done, Fault, HandFlow, LineControl, ModemLines, Port,
    QueueSize, RequestId, Timeouts, events, modem_control, modem_status, purge,
};

const MAXDWORD: u32 = u32::MAX;

/// A port with both sides open and what the opening said taken away.
fn open_port() -> Port {
    let mut port = Port::new();
    port.control_open(0).unwrap();
    port.host_open(0).unwrap();
    while port.take_completion().is_some() {}
    port
}

fn completions(port: &mut Port) -> Vec<Completion> {
    std::iter::from_fn(|| port.take_completion()).collect()
}

fn read_timeouts(interval: u32, multiplier: u32, constant: u32) -> Timeouts {
    Timeouts {
        read_interval: interval,
        read_total_multiplier: multiplier,
        read_total_constant: constant,
        ..Timeouts::default()
    }
}

fn host_read(id: u64, done: Done, data: &[u8]) -> Completion {
    Completion::HostRead {
        id: RequestId(id),
        done,
        data: data.to_vec(),
    }
}

fn host_write(id: u64, done: Done, written: usize) -> Completion {
    Completion::HostWrite {
        id: RequestId(id),
        done,
        written,
    }
}

fn host_wait(id: u64, done: Done, events: u32) -> Completion {
    Completion::HostWait {
        id: RequestId(id),
        done,
        events,
    }
}

/// Takes every event that waits for the control side.
fn control_events(port: &mut Port) -> Vec<ControlEvent> {
    let mut seen = Vec::new();
    let mut id = 9_000_000;
    loop {
        port.control_wait(RequestId(id), 0).unwrap();
        match port.take_completion() {
            Some(Completion::ControlWait {
                event: Some(event), ..
            }) => seen.push(event),
            None => {
                port.cancel(RequestId(id), 0);
                port.take_completion();
                return seen;
            }
            other => panic!("not an event: {other:?}"),
        }
        id += 1;
    }
}

// ----- Read timeouts -----

/// One step of a read's story.
enum Step {
    /// Octets arrive for the host at a time.
    Arrive(u64, &'static [u8]),
    /// The clock is moved, and nothing is expected to have ended.
    Quiet(u64),
    /// The clock is moved, and the read is expected to end so.
    Ends(u64, Done, &'static [u8]),
}
use Step::{Arrive, Ends, Quiet};

struct ReadCase {
    name: &'static str,
    timeouts: Timeouts,
    /// In the queue before the read is made.
    waiting: &'static [u8],
    want: usize,
    steps: &'static [Step],
}

#[test]
fn a_read_ends_as_the_timeouts_say() {
    let cases = [
        ReadCase {
            name: "interval MAXDWORD, no totals: at once with nothing",
            timeouts: read_timeouts(MAXDWORD, 0, 0),
            waiting: b"",
            want: 10,
            steps: &[Ends(0, Done::Ok, b"")],
        },
        ReadCase {
            name: "interval MAXDWORD, no totals: at once with what is there",
            timeouts: read_timeouts(MAXDWORD, 0, 0),
            waiting: b"abc",
            want: 10,
            steps: &[Ends(0, Done::Ok, b"abc")],
        },
        ReadCase {
            name: "interval MAXDWORD, no totals: no more than was asked",
            timeouts: read_timeouts(MAXDWORD, 0, 0),
            waiting: b"abcdef",
            want: 4,
            steps: &[Ends(0, Done::Ok, b"abcd")],
        },
        ReadCase {
            name: "interval and multiplier MAXDWORD: at once with what is there",
            timeouts: read_timeouts(MAXDWORD, MAXDWORD, 100),
            waiting: b"ab",
            want: 10,
            steps: &[Ends(0, Done::Ok, b"ab")],
        },
        ReadCase {
            name: "interval and multiplier MAXDWORD: waits for a byte, then at once",
            timeouts: read_timeouts(MAXDWORD, MAXDWORD, 100),
            waiting: b"",
            want: 10,
            steps: &[Quiet(50), Arrive(60, b"xyz"), Ends(60, Done::Ok, b"xyz")],
        },
        ReadCase {
            name: "interval and multiplier MAXDWORD: times out at the constant",
            timeouts: read_timeouts(MAXDWORD, MAXDWORD, 100),
            waiting: b"",
            want: 10,
            steps: &[Quiet(99), Ends(100, Done::TimedOut, b"")],
        },
        ReadCase {
            name: "interval MAXDWORD with a total: the first of anything, or the total",
            timeouts: read_timeouts(MAXDWORD, 2, 100),
            waiting: b"",
            want: 10,
            steps: &[Quiet(119), Ends(120, Done::TimedOut, b"")],
        },
        ReadCase {
            name: "no timeouts: waits until it is full",
            timeouts: read_timeouts(0, 0, 0),
            waiting: b"a",
            want: 3,
            steps: &[
                Quiet(1_000_000),
                Arrive(1_000_000, b"b"),
                Quiet(9_000_000),
                Arrive(9_000_000, b"cd"),
                Ends(9_000_000, Done::Ok, b"abc"),
            ],
        },
        ReadCase {
            name: "a total constant",
            timeouts: read_timeouts(0, 0, 200),
            waiting: b"",
            want: 4,
            steps: &[
                Arrive(50, b"ab"),
                Quiet(199),
                Ends(200, Done::TimedOut, b"ab"),
            ],
        },
        ReadCase {
            name: "a total of multiplier times octets plus constant",
            timeouts: read_timeouts(0, 10, 100),
            waiting: b"",
            want: 5,
            steps: &[Quiet(149), Ends(150, Done::TimedOut, b"")],
        },
        ReadCase {
            name: "a total does not hold a read that fills",
            timeouts: read_timeouts(0, 10, 100),
            waiting: b"",
            want: 2,
            steps: &[Arrive(10, b"abc"), Ends(10, Done::Ok, b"ab")],
        },
        ReadCase {
            name: "an interval is not timed before the first octet",
            timeouts: read_timeouts(20, 0, 0),
            waiting: b"",
            want: 4,
            steps: &[
                Quiet(100_000),
                Arrive(100_000, b"a"),
                Quiet(100_019),
                Arrive(100_019, b"b"),
                Quiet(100_038),
                Ends(100_039, Done::TimedOut, b"ab"),
            ],
        },
        ReadCase {
            name: "an interval is timed from octets that were waiting",
            timeouts: read_timeouts(20, 0, 0),
            waiting: b"a",
            want: 4,
            steps: &[Quiet(19), Ends(20, Done::TimedOut, b"a")],
        },
        ReadCase {
            name: "an interval and a total: the interval first",
            timeouts: read_timeouts(20, 0, 500),
            waiting: b"",
            want: 4,
            steps: &[
                Arrive(100, b"a"),
                Quiet(119),
                Ends(120, Done::TimedOut, b"a"),
            ],
        },
        ReadCase {
            name: "an interval and a total: the total first",
            timeouts: read_timeouts(20, 0, 50),
            waiting: b"",
            want: 4,
            steps: &[Arrive(40, b"a"), Quiet(49), Ends(50, Done::TimedOut, b"a")],
        },
        ReadCase {
            name: "a read of nothing ends at once",
            timeouts: read_timeouts(0, 0, 0),
            waiting: b"abc",
            want: 0,
            steps: &[Ends(0, Done::Ok, b"")],
        },
    ];
    for case in cases {
        let mut port = open_port();
        port.set_timeouts(case.timeouts).unwrap();
        assert_eq!(port.control_write(case.waiting, 0), Ok(case.waiting.len()));
        port.host_read(RequestId(1), case.want, 0).unwrap();
        let mut ended = false;
        for step in case.steps {
            match *step {
                Arrive(at, data) => {
                    assert_eq!(
                        port.control_write(data, at),
                        Ok(data.len()),
                        "{}",
                        case.name
                    );
                }
                Quiet(at) => {
                    port.advance(at);
                    assert_eq!(completions(&mut port), [], "{}: at {at}", case.name);
                }
                Ends(at, done, data) => {
                    port.advance(at);
                    assert_eq!(
                        completions(&mut port),
                        [host_read(1, done, data)],
                        "{}: at {at}",
                        case.name
                    );
                    ended = true;
                }
            }
        }
        assert!(ended, "{}: the case has no end", case.name);
        assert_eq!(port.next_deadline(), None, "{}", case.name);
    }
}

#[test]
fn the_deadline_is_when_the_read_would_time_out() {
    let mut port = open_port();
    port.set_timeouts(read_timeouts(20, 10, 100)).unwrap();
    assert_eq!(port.next_deadline(), None);
    port.host_read(RequestId(1), 5, 1000).unwrap();
    assert_eq!(port.next_deadline(), Some(1150));
    port.control_write(b"a", 1010).unwrap();
    assert_eq!(port.next_deadline(), Some(1030));
    port.control_write(b"b", 1140).unwrap();
    // The interval ran out at 1030, before the second octet came.
    assert_eq!(completions(&mut port), [host_read(1, Done::TimedOut, b"a")]);
    assert_eq!(port.comm_status().in_queue, 1);
}

#[test]
fn an_interval_of_maxdword_with_a_constant_of_maxdword_is_refused() {
    let mut port = open_port();
    let before = port.timeouts();
    assert_eq!(
        port.set_timeouts(read_timeouts(MAXDWORD, 0, MAXDWORD)),
        Err(Fault::InvalidParameter)
    );
    assert_eq!(port.timeouts(), before);
    let stored = Timeouts {
        read_interval: 1,
        read_total_multiplier: 2,
        read_total_constant: 3,
        write_total_multiplier: 4,
        write_total_constant: 5,
    };
    port.set_timeouts(stored).unwrap();
    assert_eq!(port.timeouts(), stored);
}

#[test]
fn reads_are_served_in_order_and_each_is_timed_from_its_turn() {
    let mut port = open_port();
    port.set_timeouts(read_timeouts(0, 0, 100)).unwrap();
    port.host_read(RequestId(1), 2, 0).unwrap();
    port.host_read(RequestId(2), 2, 10).unwrap();
    assert_eq!(port.next_deadline(), Some(100));
    port.advance(100);
    assert_eq!(completions(&mut port), [host_read(1, Done::TimedOut, b"")]);
    // The second read came at 10 and is timed from 100, when it became
    // the one being served.
    assert_eq!(port.next_deadline(), Some(200));
    port.control_write(b"abc", 150).unwrap();
    assert_eq!(completions(&mut port), [host_read(2, Done::Ok, b"ab")]);
    port.host_read(RequestId(3), 2, 150).unwrap();
    port.advance(250);
    assert_eq!(completions(&mut port), [host_read(3, Done::TimedOut, b"c")]);
}

#[test]
fn a_read_keeps_the_timeouts_it_started_with() {
    let mut port = open_port();
    port.set_timeouts(read_timeouts(0, 0, 100)).unwrap();
    port.host_read(RequestId(1), 2, 0).unwrap();
    port.set_timeouts(read_timeouts(MAXDWORD, 0, 0)).unwrap();
    port.advance(99);
    assert_eq!(completions(&mut port), []);
    port.advance(100);
    assert_eq!(completions(&mut port), [host_read(1, Done::TimedOut, b"")]);
    port.host_read(RequestId(2), 2, 100).unwrap();
    assert_eq!(completions(&mut port), [host_read(2, Done::Ok, b"")]);
}

// ----- Write timeouts -----

fn write_timeouts(multiplier: u32, constant: u32) -> Timeouts {
    Timeouts {
        write_total_multiplier: multiplier,
        write_total_constant: constant,
        ..Timeouts::default()
    }
}

#[test]
fn a_write_ends_when_its_octets_are_in_the_queue() {
    let mut port = open_port();
    port.host_write(RequestId(1), b"AT\r", 0).unwrap();
    assert_eq!(completions(&mut port), [host_write(1, Done::Ok, 3)]);
    port.control_read(RequestId(2), 100, 0).unwrap();
    assert_eq!(
        completions(&mut port),
        [Completion::ControlRead {
            id: RequestId(2),
            done: Done::Ok,
            data: b"AT\r".to_vec()
        }]
    );
}

#[test]
fn a_write_waits_for_room_with_no_timeout() {
    let mut port = open_port();
    let (_, room) = port.queue_capacity();
    let data = vec![7u8; room as usize + 10];
    port.host_write(RequestId(1), &data, 0).unwrap();
    port.advance(10_000_000);
    assert_eq!(completions(&mut port), []);
    assert_eq!(port.next_deadline(), None);
    assert_eq!(port.comm_status().out_queue, room + 10);
    port.control_read(RequestId(2), 10, 10_000_000).unwrap();
    let ended = completions(&mut port);
    assert_eq!(ended.len(), 2);
    assert!(ended.contains(&host_write(1, Done::Ok, data.len())));
}

#[test]
fn a_write_times_out_with_what_it_sent() {
    let cases = [
        ("a constant", write_timeouts(0, 100), 100u64),
        ("a multiplier", write_timeouts(1, 0), 4096 + 10),
        ("both", write_timeouts(1, 50), 4096 + 60),
    ];
    for (name, timeouts, deadline) in cases {
        let mut port = open_port();
        port.set_timeouts(timeouts).unwrap();
        let data = vec![7u8; 4096 + 10];
        port.host_write(RequestId(1), &data, 0).unwrap();
        assert_eq!(port.next_deadline(), Some(deadline), "{name}");
        port.advance(deadline - 1);
        assert_eq!(completions(&mut port), [], "{name}");
        port.advance(deadline);
        assert_eq!(
            completions(&mut port),
            [host_write(1, Done::TimedOut, 4096)],
            "{name}"
        );
    }
}

#[test]
fn writes_are_served_in_order() {
    let mut port = open_port();
    port.host_write(RequestId(1), &[1u8; 4096], 0).unwrap();
    port.host_write(RequestId(2), &[2u8; 8], 0).unwrap();
    port.host_write(RequestId(3), &[3u8; 8], 0).unwrap();
    assert_eq!(completions(&mut port), [host_write(1, Done::Ok, 4096)]);
    port.control_read(RequestId(4), 4096 + 16, 0).unwrap();
    let ended = completions(&mut port);
    assert!(matches!(&ended[0], Completion::ControlRead { data, .. } if data == &[1u8; 4096]));
    assert_eq!(
        ended[1..],
        [host_write(2, Done::Ok, 8), host_write(3, Done::Ok, 8)]
    );
    port.control_read(RequestId(5), 100, 0).unwrap();
    let mut both = vec![2u8; 8];
    both.extend([3u8; 8]);
    assert_eq!(
        completions(&mut port),
        [Completion::ControlRead {
            id: RequestId(5),
            done: Done::Ok,
            data: both
        }]
    );
}

#[test]
fn an_immediate_octet_goes_ahead_of_what_waits() {
    let mut port = open_port();
    port.host_write(RequestId(1), b"abc", 0).unwrap();
    port.transmit_immediate(0x13, 0).unwrap();
    port.control_read(RequestId(2), 100, 0).unwrap();
    assert!(completions(&mut port).contains(&Completion::ControlRead {
        id: RequestId(2),
        done: Done::Ok,
        data: b"\x13abc".to_vec()
    }));
}

// ----- The wait mask -----

#[test]
fn each_event_is_reported_to_a_wait() {
    type Cause = fn(&mut Port);
    let lines = |cts, dsr, ri, dcd| ModemLines { cts, dsr, ri, dcd };
    let cases: [(&str, u32, Cause); 9] = [
        ("EV_RXCHAR", events::RXCHAR, |p| {
            p.control_write(b"a", 5).unwrap();
        }),
        ("EV_RXFLAG", events::RXFLAG, |p| {
            p.control_write(b"ab\r", 5).unwrap();
        }),
        ("EV_TXEMPTY", events::TXEMPTY, |p| {
            p.host_write(RequestId(50), b"abc", 5).unwrap();
            p.control_read(RequestId(51), 100, 6).unwrap();
        }),
        ("EV_CTS", events::CTS, |p| {
            p.control_set_lines(
                ModemLines {
                    cts: true,
                    ..ModemLines::default()
                },
                5,
            )
            .unwrap();
        }),
        ("EV_DSR", events::DSR, |p| {
            p.control_set_lines(
                ModemLines {
                    dsr: true,
                    ..ModemLines::default()
                },
                5,
            )
            .unwrap();
        }),
        ("EV_RLSD", events::RLSD, |p| {
            p.control_set_lines(
                ModemLines {
                    dcd: true,
                    ..ModemLines::default()
                },
                5,
            )
            .unwrap();
        }),
        ("EV_RING", events::RING, |p| {
            p.control_set_lines(
                ModemLines {
                    ri: true,
                    ..ModemLines::default()
                },
                5,
            )
            .unwrap();
        }),
        ("EV_BREAK", events::BREAK, |p| {
            p.control_break(5).unwrap();
        }),
        ("EV_RX80FULL", events::RX80FULL, |p| {
            let (room, _) = p.queue_capacity();
            p.control_write(&vec![1u8; room as usize / 5 * 4], 5)
                .unwrap();
        }),
    ];
    let _ = lines;
    for (name, event, cause) in cases {
        // To a wait that is pending.
        let mut port = open_port();
        port.set_chars(
            Chars {
                event: b'\r',
                ..Chars::default()
            },
            0,
        )
        .unwrap();
        port.set_wait_mask(event, 0).unwrap();
        port.host_wait(RequestId(1), 0).unwrap();
        assert_eq!(completions(&mut port), [], "{name}");
        cause(&mut port);
        assert!(
            completions(&mut port).contains(&host_wait(1, Done::Ok, event)),
            "{name}: to a pending wait"
        );

        // Kept for a wait that comes after.
        let mut port = open_port();
        port.set_chars(
            Chars {
                event: b'\r',
                ..Chars::default()
            },
            0,
        )
        .unwrap();
        port.set_wait_mask(event, 0).unwrap();
        cause(&mut port);
        completions(&mut port);
        port.host_wait(RequestId(1), 10).unwrap();
        assert_eq!(
            completions(&mut port),
            [host_wait(1, Done::Ok, event)],
            "{name}: kept"
        );
        // And reported once.
        port.host_wait(RequestId(2), 10).unwrap();
        assert_eq!(completions(&mut port), [], "{name}: once");

        // Not reported to a mask that does not have it.
        let mut port = open_port();
        port.set_chars(
            Chars {
                event: b'\r',
                ..Chars::default()
            },
            0,
        )
        .unwrap();
        port.set_wait_mask(events::ALL & !event, 0).unwrap();
        cause(&mut port);
        completions(&mut port);
        port.host_wait(RequestId(1), 10).unwrap();
        for ended in completions(&mut port) {
            if let Completion::HostWait { events: seen, .. } = ended {
                assert_eq!(seen & event, 0, "{name}: not in the mask");
            }
        }
    }
}

#[test]
fn events_that_come_together_are_reported_together() {
    let mut port = open_port();
    port.set_wait_mask(events::CTS | events::DSR | events::RLSD, 0)
        .unwrap();
    port.host_wait(RequestId(1), 0).unwrap();
    let lines = ModemLines {
        cts: true,
        dsr: true,
        ri: false,
        dcd: false,
    };
    port.control_set_lines(lines, 1).unwrap();
    assert_eq!(
        completions(&mut port),
        [host_wait(1, Done::Ok, events::CTS | events::DSR)]
    );
    // What happens with no wait pending is gathered for the next.
    port.control_set_lines(ModemLines { dcd: true, ..lines }, 2)
        .unwrap();
    port.control_set_lines(
        ModemLines {
            cts: false,
            dcd: true,
            ..lines
        },
        3,
    )
    .unwrap();
    port.host_wait(RequestId(2), 4).unwrap();
    assert_eq!(
        completions(&mut port),
        [host_wait(2, Done::Ok, events::CTS | events::RLSD)]
    );
}

#[test]
fn there_is_one_wait_at_a_time_and_none_with_an_empty_mask() {
    let mut port = open_port();
    assert_eq!(
        port.host_wait(RequestId(1), 0),
        Err(Fault::InvalidParameter)
    );
    port.set_wait_mask(events::RXCHAR, 0).unwrap();
    port.host_wait(RequestId(2), 0).unwrap();
    assert_eq!(
        port.host_wait(RequestId(3), 0),
        Err(Fault::InvalidParameter)
    );
    assert_eq!(
        port.set_wait_mask(events::ALL + 1, 0),
        Err(Fault::InvalidParameter)
    );
    assert_eq!(port.wait_mask(), events::RXCHAR);
    assert_eq!(completions(&mut port), []);
}

#[test]
fn a_new_mask_ends_a_pending_wait_with_no_events() {
    let mut port = open_port();
    port.set_wait_mask(events::RXCHAR, 0).unwrap();
    port.host_wait(RequestId(1), 0).unwrap();
    port.set_wait_mask(events::RXCHAR | events::RLSD, 1)
        .unwrap();
    assert_eq!(completions(&mut port), [host_wait(1, Done::Ok, 0)]);
    // A mask of nothing ends it too, and is how a program stops waiting.
    port.host_wait(RequestId(2), 2).unwrap();
    port.set_wait_mask(0, 3).unwrap();
    assert_eq!(completions(&mut port), [host_wait(2, Done::Ok, 0)]);
}

#[test]
fn a_new_mask_keeps_the_events_it_still_has() {
    let mut port = open_port();
    port.set_wait_mask(events::RXCHAR | events::CTS, 0).unwrap();
    port.control_write(b"a", 1).unwrap();
    port.control_set_lines(
        ModemLines {
            cts: true,
            ..ModemLines::default()
        },
        1,
    )
    .unwrap();
    port.set_wait_mask(events::RXCHAR | events::RLSD, 2)
        .unwrap();
    port.host_wait(RequestId(1), 3).unwrap();
    assert_eq!(
        completions(&mut port),
        [host_wait(1, Done::Ok, events::RXCHAR)]
    );
}

#[test]
fn the_ring_event_is_the_ring_coming_on() {
    let mut port = open_port();
    port.set_wait_mask(events::RING, 0).unwrap();
    let ringing = ModemLines {
        ri: true,
        ..ModemLines::default()
    };
    port.control_set_lines(ringing, 1).unwrap();
    port.host_wait(RequestId(1), 1).unwrap();
    assert_eq!(
        completions(&mut port),
        [host_wait(1, Done::Ok, events::RING)]
    );
    port.control_set_lines(ModemLines::default(), 2).unwrap();
    port.host_wait(RequestId(2), 2).unwrap();
    assert_eq!(completions(&mut port), []);
    port.control_set_lines(ringing, 3).unwrap();
    assert_eq!(
        completions(&mut port),
        [host_wait(2, Done::Ok, events::RING)]
    );
}

#[test]
fn the_output_queue_is_empty_once_for_each_time_it_empties() {
    let mut port = open_port();
    port.set_wait_mask(events::TXEMPTY, 0).unwrap();
    port.host_write(RequestId(1), b"abc", 0).unwrap();
    port.host_write(RequestId(2), b"def", 0).unwrap();
    port.host_wait(RequestId(3), 0).unwrap();
    assert_eq!(
        completions(&mut port),
        [host_write(1, Done::Ok, 3), host_write(2, Done::Ok, 3)]
    );
    port.control_read(RequestId(4), 4, 1).unwrap();
    assert_eq!(completions(&mut port).len(), 1, "not empty yet");
    port.control_read(RequestId(5), 4, 2).unwrap();
    let ended = completions(&mut port);
    assert!(ended.contains(&host_wait(3, Done::Ok, events::TXEMPTY)));
    port.host_wait(RequestId(6), 3).unwrap();
    port.control_read(RequestId(7), 4, 3).unwrap();
    assert_eq!(completions(&mut port), []);
}

// ----- Lines -----

#[test]
fn the_host_reads_the_lines_the_control_side_sets() {
    let cases = [
        (
            ModemLines {
                cts: true,
                ..ModemLines::default()
            },
            modem_status::CTS,
        ),
        (
            ModemLines {
                dsr: true,
                ..ModemLines::default()
            },
            modem_status::DSR,
        ),
        (
            ModemLines {
                ri: true,
                ..ModemLines::default()
            },
            modem_status::RING,
        ),
        (
            ModemLines {
                dcd: true,
                ..ModemLines::default()
            },
            modem_status::RLSD,
        ),
        (
            ModemLines {
                cts: true,
                dsr: true,
                ri: true,
                dcd: true,
            },
            modem_status::CTS | modem_status::DSR | modem_status::RING | modem_status::RLSD,
        ),
        (ModemLines::default(), 0),
    ];
    let mut port = open_port();
    assert_eq!(port.modem_status(), 0);
    for (lines, bits) in cases {
        port.control_set_lines(lines, 0).unwrap();
        assert_eq!(port.modem_status(), bits);
    }
}

#[test]
fn the_control_side_is_told_of_dtr_and_rts() {
    let mut port = Port::new();
    port.control_open(0).unwrap();
    port.host_open(0).unwrap();
    // A port opens with both on.
    assert_eq!(
        control_events(&mut port),
        [
            ControlEvent::HostOpened,
            ControlEvent::Dtr(true),
            ControlEvent::Rts(true)
        ]
    );
    assert_eq!(port.dtr_rts(), (true, true));
    port.set_dtr(false, 1).unwrap();
    port.set_dtr(false, 1).unwrap();
    port.set_rts(false, 2).unwrap();
    port.set_dtr(true, 3).unwrap();
    assert_eq!(
        control_events(&mut port),
        [
            ControlEvent::Dtr(false),
            ControlEvent::Rts(false),
            ControlEvent::Dtr(true)
        ]
    );
    assert_eq!(port.dtr_rts(), (true, false));
    let state = port.control_state();
    assert!(state.host_open && state.dtr && !state.rts);
}

#[test]
fn the_modem_control_register_is_dtr_and_rts_and_three_bits_kept() {
    let mut port = open_port();
    control_events(&mut port);
    assert_eq!(
        port.modem_control(),
        modem_control::DTR | modem_control::RTS
    );
    port.set_modem_control(modem_control::RTS | modem_control::OUT2, 1)
        .unwrap();
    assert_eq!(
        port.modem_control(),
        modem_control::RTS | modem_control::OUT2
    );
    assert_eq!(port.dtr_rts(), (false, true));
    assert_eq!(control_events(&mut port), [ControlEvent::Dtr(false)]);
    assert_eq!(
        port.set_modem_control(0x20, 2),
        Err(Fault::InvalidParameter)
    );
}

#[test]
fn the_flow_control_settings_say_where_dtr_and_rts_are() {
    let mut port = open_port();
    control_events(&mut port);
    let off = HandFlow {
        control_handshake: 0,
        flow_replace: 0,
        xon_limit: 100,
        xoff_limit: 200,
    };
    port.set_handflow(off, 1).unwrap();
    assert_eq!(port.handflow(), off);
    assert_eq!(port.dtr_rts(), (false, false));
    assert_eq!(
        control_events(&mut port),
        [
            ControlEvent::Dtr(false),
            ControlEvent::Rts(false),
            ControlEvent::Settings
        ]
    );
    // With the lines the host's to set, it sets them.
    port.set_dtr(true, 2).unwrap();
    port.set_rts(true, 2).unwrap();
    assert_eq!(port.dtr_rts(), (true, true));
    // A setting that changes neither mode moves neither line.
    port.set_handflow(
        HandFlow {
            xon_limit: 1,
            ..off
        },
        3,
    )
    .unwrap();
    assert_eq!(port.dtr_rts(), (true, true));
    // As flow control the lines are on, and are not the host's to set.
    let handshake = HandFlow {
        control_handshake: HandFlow::DTR_HANDSHAKE,
        flow_replace: HandFlow::RTS_HANDSHAKE,
        ..off
    };
    port.set_dtr(false, 4).unwrap();
    port.set_handflow(handshake, 5).unwrap();
    assert_eq!(port.dtr_rts(), (true, true));
    assert_eq!(port.set_dtr(false, 6), Err(Fault::InvalidParameter));
    assert_eq!(port.set_rts(false, 6), Err(Fault::InvalidParameter));
}

#[test]
fn a_break_from_the_host_is_told_once_each_way() {
    let mut port = open_port();
    control_events(&mut port);
    port.set_break(true, 1);
    port.set_break(true, 2);
    port.set_break(false, 3);
    assert_eq!(
        control_events(&mut port),
        [ControlEvent::Break(true), ControlEvent::Break(false)]
    );
}

// ----- Stored settings -----

#[test]
fn settings_are_stored_and_read_back_and_move_nothing() {
    let mut port = open_port();
    control_events(&mut port);
    for baud in [300, 2400, 115_200, 921_600] {
        port.set_baud(baud, 0).unwrap();
        assert_eq!(port.baud(), baud);
        assert_eq!(port.control_state().baud, baud);
    }
    assert_eq!(port.set_baud(0, 0), Err(Fault::InvalidParameter));
    let seven_even = LineControl {
        stop_bits: 2,
        parity: 2,
        word_length: 7,
    };
    port.set_line_control(seven_even, 0).unwrap();
    assert_eq!(port.line_control(), seven_even);
    for bad in [
        LineControl {
            stop_bits: 3,
            ..seven_even
        },
        LineControl {
            parity: 5,
            ..seven_even
        },
        LineControl {
            word_length: 4,
            ..seven_even
        },
        LineControl {
            word_length: 9,
            ..seven_even
        },
    ] {
        assert_eq!(port.set_line_control(bad, 0), Err(Fault::InvalidParameter));
    }
    let chars = Chars {
        eof: 26,
        error: 1,
        break_char: 2,
        event: 3,
        xon: 4,
        xoff: 5,
    };
    port.set_chars(chars, 0).unwrap();
    assert_eq!(port.chars(), chars);
    assert_eq!(
        port.set_chars(
            Chars {
                xon: 9,
                xoff: 9,
                ..chars
            },
            0
        ),
        Err(Fault::InvalidParameter)
    );
    assert_eq!(port.chars(), chars);
    // Each change is told, and a setting to what it was is not.
    port.set_baud(921_600, 0).unwrap();
    assert_eq!(control_events(&mut port), [ControlEvent::Settings; 6]);

    // At 300 bit/s a megabyte crosses in no time at all.
    port.set_baud(300, 0).unwrap();
    port.set_timeouts(read_timeouts(MAXDWORD, 0, 0)).unwrap();
    let sent: Vec<u8> = (0..1_000_000u32).map(|i| (i * 7 + i / 255) as u8).collect();
    let mut got = Vec::new();
    let mut at = 0;
    let mut id = 100;
    while got.len() < sent.len() {
        if at < sent.len() {
            at += port.control_write(&sent[at..], 0).unwrap();
        }
        port.host_read(RequestId(id), 1000, 0).unwrap();
        id += 1;
        for ended in completions(&mut port) {
            if let Completion::HostRead { data, .. } = ended {
                got.extend(data);
            }
        }
    }
    assert!(got == sent);
}

#[test]
fn the_queues_are_as_large_as_the_host_asks_within_bounds() {
    let mut port = open_port();
    assert_eq!(port.queue_capacity(), (4096, 4096));
    port.set_queue_size(QueueSize {
        input: 16_384,
        output: 8192,
    });
    assert_eq!(port.queue_capacity(), (16_384, 8192));
    assert_eq!(port.control_write(&[0u8; 20_000], 0), Ok(16_384));
    assert_eq!(port.control_state().input_free, 0);
    port.set_queue_size(QueueSize {
        input: 1,
        output: u32::MAX,
    });
    assert_eq!(port.queue_capacity(), (64, 65_536));
    // Nothing that was in the queue is lost by making it smaller.
    assert_eq!(port.comm_status().in_queue, 16_384);
    assert_eq!(port.control_write(b"a", 0), Ok(0));
}

#[test]
fn the_counts_and_the_queue_lengths_are_reported() {
    let mut port = open_port();
    port.control_write(b"hello", 0).unwrap();
    port.host_write(RequestId(1), b"abc", 0).unwrap();
    assert_eq!(port.stats().received, 5);
    assert_eq!(port.stats().transmitted, 3);
    assert_eq!(port.comm_status().in_queue, 5);
    assert_eq!(port.comm_status().out_queue, 3);
    port.clear_stats();
    assert_eq!(port.stats().received, 0);
    assert_eq!(port.stats().transmitted, 0);
    assert_eq!(port.comm_status().in_queue, 5);
}

// ----- Purge and cancel -----

#[test]
fn a_purge_does_what_each_flag_says() {
    assert_eq!(open_port().purge(0, 0), Err(Fault::InvalidParameter));
    assert_eq!(open_port().purge(0x10, 0), Err(Fault::InvalidParameter));

    let story = |flags: u32| {
        let mut port = open_port();
        control_events(&mut port);
        // A read that has taken one octet and waits for more, an octet
        // that came after the read was satisfied, a write that is half
        // in the queue.
        port.control_write(b"a", 0).unwrap();
        port.host_read(RequestId(1), 2, 0).unwrap();
        port.host_write(RequestId(2), &[5u8; 5000], 0).unwrap();
        assert_eq!(completions(&mut port), []);
        port.purge(flags, 1).unwrap();
        let ended = completions(&mut port);
        let status = port.comm_status();
        (
            ended,
            status.in_queue,
            status.out_queue,
            control_events(&mut port),
        )
    };

    let (ended, input, output, told) = story(purge::RXABORT);
    assert_eq!(ended, [host_read(1, Done::Cancelled, b"")]);
    assert_eq!((input, output), (1, 5000), "the read gave back what it had");
    assert_eq!(told, []);

    let (ended, input, output, told) = story(purge::TXABORT);
    assert_eq!(ended, [host_write(2, Done::Cancelled, 4096)]);
    assert_eq!((input, output), (0, 4096));
    assert_eq!(told, []);

    let (ended, input, output, told) = story(purge::RXCLEAR);
    assert_eq!(ended, []);
    assert_eq!((input, output), (0, 5000));
    assert_eq!(
        told,
        [ControlEvent::Purge {
            input: true,
            output: false
        }]
    );

    let (ended, _, output, told) = story(purge::TXCLEAR);
    // The queue is emptied, and the write that waited for room has it.
    assert_eq!(ended, [host_write(2, Done::Ok, 5000)]);
    assert_eq!(output, 5000 - 4096);
    assert_eq!(
        told,
        [ControlEvent::Purge {
            input: false,
            output: true
        }]
    );

    let (ended, input, output, told) = story(purge::ALL);
    assert_eq!(
        ended,
        [
            host_write(2, Done::Cancelled, 4096),
            host_read(1, Done::Cancelled, b"")
        ]
    );
    assert_eq!((input, output), (0, 0));
    assert_eq!(
        told,
        [ControlEvent::Purge {
            input: true,
            output: true
        }]
    );
}

#[test]
fn a_cancelled_request_ends_cancelled_and_loses_nothing() {
    let mut port = open_port();
    port.control_write(b"ab", 0).unwrap();
    port.host_read(RequestId(1), 5, 0).unwrap();
    port.host_read(RequestId(2), 5, 0).unwrap();
    port.cancel(RequestId(1), 1);
    // What the first had taken goes to the second.
    assert_eq!(completions(&mut port), [host_read(1, Done::Cancelled, b"")]);
    port.control_write(b"cde", 2).unwrap();
    assert_eq!(completions(&mut port), [host_read(2, Done::Ok, b"abcde")]);

    port.host_write(RequestId(3), &[1u8; 5000], 3).unwrap();
    port.cancel(RequestId(3), 4);
    assert_eq!(
        completions(&mut port),
        [host_write(3, Done::Cancelled, 4096)]
    );

    port.set_wait_mask(events::RXCHAR, 5).unwrap();
    port.host_wait(RequestId(4), 5).unwrap();
    port.cancel(RequestId(4), 6);
    assert_eq!(completions(&mut port), [host_wait(4, Done::Cancelled, 0)]);
    port.host_wait(RequestId(5), 7).unwrap();

    port.purge(purge::TXCLEAR, 8).unwrap();
    port.control_read(RequestId(6), 10, 8).unwrap();
    port.cancel(RequestId(6), 9);
    assert_eq!(
        completions(&mut port),
        [Completion::ControlRead {
            id: RequestId(6),
            done: Done::Cancelled,
            data: Vec::new()
        }]
    );
    control_events(&mut port);
    port.control_wait(RequestId(8), 10).unwrap();
    port.cancel(RequestId(8), 11);
    assert_eq!(
        completions(&mut port),
        [Completion::ControlWait {
            id: RequestId(8),
            done: Done::Cancelled,
            event: None
        }]
    );
    // A name that is not waiting is nothing.
    port.cancel(RequestId(999), 12);
    assert_eq!(completions(&mut port), []);
}

// ----- Opening and closing -----

#[test]
fn each_side_is_one_at_a_time() {
    let mut port = Port::new();
    assert_eq!(port.host_read(RequestId(1), 1, 0), Err(Fault::NotOpen));
    assert_eq!(port.host_write(RequestId(1), b"a", 0), Err(Fault::NotOpen));
    assert_eq!(port.control_write(b"a", 0), Err(Fault::NotOpen));
    assert_eq!(port.control_read(RequestId(1), 1, 0), Err(Fault::NotOpen));
    assert_eq!(port.control_wait(RequestId(1), 0), Err(Fault::NotOpen));
    assert_eq!(
        port.control_set_lines(ModemLines::default(), 0),
        Err(Fault::NotOpen)
    );
    port.host_open(0).unwrap();
    assert_eq!(port.host_open(0), Err(Fault::Busy));
    port.control_open(0).unwrap();
    assert_eq!(port.control_open(0), Err(Fault::Busy));
    port.host_close(1);
    port.host_open(2).unwrap();
    port.control_close(3);
    port.control_open(4).unwrap();
    port.control_wait(RequestId(1), 5).unwrap();
    assert_eq!(
        port.control_wait(RequestId(2), 5),
        Err(Fault::InvalidParameter)
    );
}

#[test]
fn closing_the_host_cancels_purges_and_drops_dtr_and_rts() {
    let mut port = open_port();
    control_events(&mut port);
    port.set_baud(2400, 0).unwrap();
    port.set_wait_mask(events::RLSD, 0).unwrap();
    port.control_write(b"a", 0).unwrap();
    port.host_read(RequestId(1), 2, 0).unwrap();
    port.host_write(RequestId(2), &[5u8; 5000], 0).unwrap();
    port.host_wait(RequestId(3), 0).unwrap();
    port.set_break(true, 0);
    control_events(&mut port);

    port.host_close(1);
    assert_eq!(
        completions(&mut port),
        [
            host_read(1, Done::Cancelled, b""),
            host_write(2, Done::Cancelled, 4096),
            host_wait(3, Done::Cancelled, 0),
        ]
    );
    assert_eq!(
        control_events(&mut port),
        [
            ControlEvent::Dtr(false),
            ControlEvent::Rts(false),
            ControlEvent::Break(false),
            ControlEvent::HostClosed,
        ]
    );
    let state = port.control_state();
    assert!(!state.host_open && !state.dtr && !state.rts && !state.break_on);
    assert_eq!(state.output_waiting, 0);
    assert_eq!(port.comm_status().in_queue, 0);
    // With no host, what the service writes is taken and goes nowhere.
    assert_eq!(port.control_write(b"RING\r\n", 2), Ok(6));

    // The next host finds an empty port with the settings as they were
    // left, and no mask.
    port.host_open(3).unwrap();
    assert_eq!(port.comm_status().in_queue, 0);
    assert_eq!(port.baud(), 2400);
    assert_eq!(port.wait_mask(), 0);
    assert_eq!(port.dtr_rts(), (true, true));
}

#[test]
fn a_port_with_no_control_side_is_usable_and_dead() {
    let mut port = open_port();
    let up = ModemLines {
        cts: true,
        dsr: true,
        ri: false,
        dcd: true,
    };
    port.control_set_lines(up, 0).unwrap();
    port.set_wait_mask(events::CTS | events::DSR | events::RLSD, 0)
        .unwrap();
    port.host_wait(RequestId(1), 0).unwrap();
    port.control_read(RequestId(2), 10, 0).unwrap();
    control_events(&mut port);
    port.control_wait(RequestId(4), 0).unwrap();
    port.host_write(RequestId(5), &[1u8; 5000], 0).unwrap();
    completions(&mut port);

    port.control_close(1);
    let ended = completions(&mut port);
    // The service's own requests are cancelled; the host sees every line
    // go low; and the write that waited for the service ends, its octets
    // gone where a dead line's go.
    assert!(ended.contains(&Completion::ControlWait {
        id: RequestId(4),
        done: Done::Cancelled,
        event: None
    }));
    assert!(ended.contains(&host_wait(
        1,
        Done::Ok,
        events::CTS | events::DSR | events::RLSD
    )));
    assert!(ended.contains(&host_write(5, Done::Ok, 5000)));
    assert_eq!(port.modem_status(), 0);

    // The host can go on using it.
    port.host_write(RequestId(6), b"AT\r", 2).unwrap();
    assert_eq!(completions(&mut port), [host_write(6, Done::Ok, 3)]);
    port.set_timeouts(read_timeouts(0, 0, 100)).unwrap();
    port.host_read(RequestId(7), 2, 2).unwrap();
    port.advance(102);
    assert_eq!(completions(&mut port), [host_read(7, Done::TimedOut, b"")]);

    // A service that comes back finds nothing of before, and the state.
    port.control_open(200).unwrap();
    assert_eq!(control_events(&mut port), []);
    let state = port.control_state();
    assert!(state.host_open && state.dtr && state.rts);
    assert_eq!(state.output_waiting, 0);
    port.control_set_lines(up, 201).unwrap();
    assert_eq!(
        port.modem_status(),
        modem_status::CTS | modem_status::DSR | modem_status::RLSD
    );
}

#[test]
fn a_control_side_that_does_not_listen_is_told_it_missed_events() {
    let mut port = open_port();
    control_events(&mut port);
    for at in 0..300 {
        port.set_dtr(at % 2 == 0, at).unwrap();
    }
    let told = control_events(&mut port);
    assert_eq!(told[0], ControlEvent::Lost);
    assert!(told.len() < 300);
    // The state is what it reads to catch up.
    assert!(!port.control_state().dtr);
}
