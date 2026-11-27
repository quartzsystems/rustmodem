// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The port as Windows asks for it: every serial I/O control code the
//! port has, with the layout it reads and writes, the codes it does not
//! have, and the control side's codes.

use vcomapi::{Event, State};
use vcomcore::ioctl::{Finished, Outcome, control_ioctl, host_ioctl, serial_code, status};
use vcomcore::{Port, RequestId, events, modem_status};

fn open_port() -> Port {
    let mut port = Port::new();
    port.control_open(0).unwrap();
    port.host_open(0).unwrap();
    port
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn done(data: Vec<u8>) -> Outcome {
    Outcome::Done {
        status: status::SUCCESS,
        data,
    }
}

fn failed(status: u32) -> Outcome {
    Outcome::Done {
        status,
        data: Vec::new(),
    }
}

/// A request from the host with an output buffer of 256 octets.
fn host(port: &mut Port, function: u32, input: &[u8]) -> Outcome {
    host_ioctl(port, RequestId(1), serial_code(function), input, 256, 0)
}

fn control(port: &mut Port, code: u32, input: &[u8]) -> Outcome {
    control_ioctl(port, RequestId(2), code, input, 256, 0)
}

fn finished(port: &mut Port) -> Vec<Finished> {
    std::iter::from_fn(|| port.take_completion())
        .map(|c| c.finish())
        .collect()
}

#[test]
fn what_is_set_is_read_back_in_the_layout_it_was_set_in() {
    // (set, get, the value on the wire)
    let cases: [(&str, u32, u32, Vec<u8>); 6] = [
        ("baud rate", 1, 20, words(&[2400])),
        ("line control", 3, 21, vec![2, 2, 7]),
        ("timeouts", 7, 8, words(&[1, 2, 3, 4, 5])),
        ("special characters", 23, 22, vec![26, 1, 2, 13, 0x11, 0x13]),
        ("flow control", 25, 24, words(&[0x01, 0x40, 100, 200])),
        ("wait mask", 17, 16, words(&[events::RXCHAR | events::RLSD])),
    ];
    for (name, set, get, wire) in cases {
        let mut port = open_port();
        assert_eq!(host(&mut port, set, &wire), done(Vec::new()), "{name}");
        assert_eq!(host(&mut port, get, &[]), done(wire.clone()), "{name}");
        // A buffer too short to hold the answer, and one too short to
        // hold the question.
        assert_eq!(
            host_ioctl(
                &mut port,
                RequestId(1),
                serial_code(get),
                &[],
                wire.len() - 1,
                0
            ),
            failed(status::BUFFER_TOO_SMALL),
            "{name}"
        );
        assert_eq!(
            host(&mut port, set, &wire[..wire.len() - 1]),
            failed(status::BUFFER_TOO_SMALL),
            "{name}"
        );
    }
}

#[test]
fn a_value_the_port_cannot_have_is_an_invalid_parameter() {
    let cases: [(&str, u32, Vec<u8>); 8] = [
        ("a baud rate of nothing", 1, words(&[0])),
        ("nine data bits", 3, vec![0, 0, 9]),
        ("a parity there is not", 3, vec![0, 5, 8]),
        ("XON the same as XOFF", 23, vec![0, 0, 0, 0, 7, 7]),
        ("a mask with a bit there is not", 17, words(&[0x2000])),
        ("a purge of nothing", 19, words(&[0])),
        (
            "an interval and a constant of MAXDWORD",
            7,
            words(&[u32::MAX, 0, u32::MAX, 0, 0]),
        ),
        ("a modem control bit there is not", 38, words(&[0x20])),
    ];
    for (name, function, input) in cases {
        let mut port = open_port();
        assert_eq!(
            host(&mut port, function, &input),
            failed(status::INVALID_PARAMETER),
            "{name}"
        );
    }
}

#[test]
fn the_lines_are_set_cleared_and_read() {
    let mut port = open_port();
    // GET_DTRRTS: DTR is bit 0 and RTS bit 1. A port opens with both.
    assert_eq!(host(&mut port, 30, &[]), done(words(&[3])));
    assert_eq!(host(&mut port, 10, &[]), done(Vec::new())); // CLR_DTR
    assert_eq!(host(&mut port, 30, &[]), done(words(&[2])));
    assert_eq!(host(&mut port, 13, &[]), done(Vec::new())); // CLR_RTS
    assert_eq!(host(&mut port, 9, &[]), done(Vec::new())); // SET_DTR
    assert_eq!(host(&mut port, 30, &[]), done(words(&[1])));
    assert_eq!(host(&mut port, 12, &[]), done(Vec::new())); // SET_RTS
    assert_eq!(host(&mut port, 37, &[]), done(words(&[3]))); // modem control
    assert_eq!(host(&mut port, 38, &words(&[0x0A])), done(Vec::new()));
    assert_eq!(host(&mut port, 37, &[]), done(words(&[0x0A])));
    assert_eq!(host(&mut port, 30, &[]), done(words(&[2])));

    // GET_MODEMSTATUS is what the control side set.
    assert_eq!(host(&mut port, 26, &[]), done(words(&[0])));
    let lines = vcomapi::lines::CTS | vcomapi::lines::DSR | vcomapi::lines::DCD;
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_SET_LINES, &words(&[lines])),
        done(Vec::new())
    );
    let status = modem_status::CTS | modem_status::DSR | modem_status::RLSD;
    assert_eq!(host(&mut port, 26, &[]), done(words(&[status])));
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_SET_LINES, &words(&[0x10])),
        failed(status::INVALID_PARAMETER)
    );
}

#[test]
fn the_status_the_properties_and_the_counts_have_their_layouts() {
    let mut port = open_port();
    control(&mut port, vcomapi::IOCTL_WRITE, b"hello");
    port.host_write(RequestId(9), b"abc", 0).unwrap();

    // SERIAL_STATUS: 20 octets, the queue lengths third and fourth.
    let Outcome::Done { data, .. } = host(&mut port, 27, &[]) else {
        panic!()
    };
    assert_eq!(data, [words(&[0, 0, 5, 3]), vec![0; 4]].concat());

    // SERIALPERF_STATS: 24 octets, received then transmitted.
    assert_eq!(host(&mut port, 35, &[]), done(words(&[5, 3, 0, 0, 0, 0])));
    assert_eq!(host(&mut port, 36, &[]), done(Vec::new()));
    assert_eq!(host(&mut port, 35, &[]), done(words(&[0; 6])));

    // SERIAL_COMMPROP: 64 octets, which say its own length, any baud
    // rate, and the queues as they are.
    assert_eq!(host(&mut port, 2, &words(&[8192, 2048])), done(Vec::new()));
    let Outcome::Done { data, .. } = host(&mut port, 29, &[]) else {
        panic!()
    };
    assert_eq!(data.len(), 64);
    let word = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    assert_eq!(u16::from_le_bytes([data[0], data[1]]), 64);
    assert_eq!(word(20), 0x1000_0000, "MaxBaud is BAUD_USER");
    assert_eq!(word(24), 1, "ProvSubType is PST_RS232");
    assert_eq!(word(44), 2048, "CurrentTxQueue");
    assert_eq!(word(48), 8192, "CurrentRxQueue");

    // CONFIG_SIZE: none.
    assert_eq!(host(&mut port, 32, &[]), done(words(&[0])));
}

#[test]
fn what_needs_nothing_done_is_accepted() {
    let mut port = open_port();
    for (name, function) in [("RESET_DEVICE", 11), ("SET_XOFF", 14), ("SET_XON", 15)] {
        assert_eq!(host(&mut port, function, &[]), done(Vec::new()), "{name}");
    }
    assert_eq!(host(&mut port, 4, &[]), done(Vec::new()), "SET_BREAK_ON");
    assert!(port.control_state().break_on);
    assert_eq!(host(&mut port, 5, &[]), done(Vec::new()), "SET_BREAK_OFF");
    assert!(!port.control_state().break_on);
    assert_eq!(
        host(&mut port, 6, &[0x11]),
        done(Vec::new()),
        "IMMEDIATE_CHAR"
    );
    assert_eq!(port.control_state().output_waiting, 1);
    assert_eq!(
        host(&mut port, 19, &words(&[0x4])),
        done(Vec::new()),
        "PURGE"
    );
    assert_eq!(port.control_state().output_waiting, 0);
}

#[test]
fn a_code_the_port_does_not_have_is_refused_as_a_serial_port_refuses_it() {
    let mut port = open_port();
    // XOFF_COUNTER, LSRMST_INSERT, GET_COMMCONFIG, SET_COMMCONFIG,
    // SET_FIFO_CONTROL, and numbers no serial code has.
    for function in [28, 31, 33, 34, 39, 0, 40, 100, 0xFFF] {
        assert_eq!(
            host(&mut port, function, &[0; 64]),
            failed(status::INVALID_PARAMETER),
            "function {function}"
        );
    }
    // A code of another device type: Windows' modem driver sends one
    // down when a modem is installed on the port.
    let modem = (0x2b << 16) | (11 << 2);
    assert_eq!(
        host_ioctl(&mut port, RequestId(1), modem, &[], 0, 0),
        failed(status::INVALID_PARAMETER)
    );
    // A serial function by another method.
    assert_eq!(
        host_ioctl(&mut port, RequestId(1), serial_code(20) | 3, &[], 4, 0),
        failed(status::INVALID_PARAMETER)
    );
    // The control side's codes are not the host's, nor the host's its.
    assert_eq!(
        host_ioctl(
            &mut port,
            RequestId(1),
            vcomapi::IOCTL_GET_STATE,
            &[],
            64,
            0
        ),
        failed(status::INVALID_PARAMETER)
    );
    assert_eq!(
        control(&mut port, serial_code(20), &[]),
        failed(status::INVALID_PARAMETER)
    );
}

#[test]
fn a_wait_pends_and_ends_with_its_events() {
    let mut port = open_port();
    // With no mask a wait is refused, and with no room for the answer.
    assert_eq!(host(&mut port, 18, &[]), failed(status::INVALID_PARAMETER));
    host(&mut port, 17, &words(&[events::RXCHAR]));
    assert_eq!(
        host_ioctl(&mut port, RequestId(1), serial_code(18), &[], 3, 0),
        failed(status::BUFFER_TOO_SMALL)
    );
    assert_eq!(host(&mut port, 18, &[]), Outcome::Pending);
    assert_eq!(finished(&mut port), []);
    control(&mut port, vcomapi::IOCTL_WRITE, b"a");
    assert_eq!(
        finished(&mut port),
        [Finished {
            id: RequestId(1),
            status: status::SUCCESS,
            data: words(&[events::RXCHAR]),
            information: 4,
        }]
    );
    // A new mask ends a wait with a mask of nothing.
    assert_eq!(host(&mut port, 18, &[]), Outcome::Pending);
    host(&mut port, 17, &words(&[events::RXCHAR]));
    assert_eq!(finished(&mut port)[0].data, words(&[0]));
    // A cancelled wait has nothing in it.
    assert_eq!(host(&mut port, 18, &[]), Outcome::Pending);
    port.cancel(RequestId(1), 0);
    assert_eq!(
        finished(&mut port),
        [Finished {
            id: RequestId(1),
            status: status::CANCELLED,
            data: Vec::new(),
            information: 0,
        }]
    );
}

#[test]
fn reads_and_writes_end_with_a_status_and_a_count() {
    let mut port = open_port();
    host(&mut port, 7, &words(&[0, 0, 100, 0, 50]));
    port.host_read(RequestId(1), 4, 0).unwrap();
    control(&mut port, vcomapi::IOCTL_WRITE, b"ab");
    port.advance(100);
    assert_eq!(
        finished(&mut port),
        [Finished {
            id: RequestId(1),
            status: status::TIMEOUT,
            data: b"ab".to_vec(),
            information: 2,
        }]
    );
    port.host_write(RequestId(2), &[7; 5000], 100).unwrap();
    port.advance(150);
    assert_eq!(
        finished(&mut port),
        [Finished {
            id: RequestId(2),
            status: status::TIMEOUT,
            data: Vec::new(),
            information: 4096,
        }]
    );
}

#[test]
fn the_control_side_reads_writes_waits_and_asks() {
    let mut port = Port::new();
    port.control_open(0).unwrap();
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_WAIT_EVENT, &[]),
        Outcome::Pending
    );
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_WAIT_EVENT, &[]),
        failed(status::INVALID_PARAMETER),
        "one wait at a time"
    );
    port.host_open(1).unwrap();
    let first = finished(&mut port);
    assert_eq!(first.len(), 1);
    assert_eq!(Event::decode(&first[0].data), Some(Event::HostOpened));
    assert_eq!(first[0].information, Event::SIZE);
    for expected in [Event::Dtr(true), Event::Rts(true)] {
        assert_eq!(
            control(&mut port, vcomapi::IOCTL_WAIT_EVENT, &[]),
            Outcome::Pending
        );
        assert_eq!(Event::decode(&finished(&mut port)[0].data), Some(expected));
    }
    assert_eq!(
        control_ioctl(
            &mut port,
            RequestId(2),
            vcomapi::IOCTL_WAIT_EVENT,
            &[],
            7,
            1
        ),
        failed(status::BUFFER_TOO_SMALL)
    );

    // A read waits for the host to write.
    assert_eq!(
        control_ioctl(&mut port, RequestId(3), vcomapi::IOCTL_READ, &[], 2, 1),
        Outcome::Pending
    );
    port.host_write(RequestId(4), b"AT\r", 2).unwrap();
    let ended = finished(&mut port);
    assert!(ended.contains(&Finished {
        id: RequestId(3),
        status: status::SUCCESS,
        data: b"AT".to_vec(),
        information: 2,
    }));

    // A write says how many octets were taken.
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_WRITE, &[1; 5000]),
        done(words(&[4096]))
    );
    assert_eq!(
        control_ioctl(&mut port, RequestId(2), vcomapi::IOCTL_WRITE, b"a", 3, 2),
        failed(status::BUFFER_TOO_SMALL)
    );

    let Outcome::Done { data, .. } = control(&mut port, vcomapi::IOCTL_GET_STATE, &[]) else {
        panic!()
    };
    let state = State::decode(&data).unwrap();
    assert_eq!(state.version, vcomapi::VERSION);
    assert!(state.host_open && state.dtr && state.rts && !state.break_on);
    assert_eq!(state.baud, 115_200);
    assert_eq!(state.input_free, 0);
    assert_eq!(state.output_waiting, 1);

    // A break toward the host is an event of the host's.
    host(&mut port, 17, &words(&[events::BREAK]));
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_BREAK, &[]),
        done(Vec::new())
    );
    assert_eq!(host(&mut port, 18, &[]), Outcome::Pending);
    assert_eq!(finished(&mut port)[0].data, words(&[events::BREAK]));
}

#[test]
fn a_side_that_is_not_open_is_an_invalid_device_state() {
    let mut port = Port::new();
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_WRITE, b"a"),
        failed(status::INVALID_DEVICE_STATE)
    );
    assert_eq!(
        control(&mut port, vcomapi::IOCTL_READ, &[]),
        failed(status::INVALID_DEVICE_STATE)
    );
    assert_eq!(
        host(&mut port, 6, &[1]),
        failed(status::INVALID_DEVICE_STATE)
    );
}
