// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The memory backend and the error messages of the serial crate.

use std::io;

use serial::{
    ModemSignals, SerialBackend, SerialEvent, SerialOperation, WaitHandle, error_message,
    memory_pair, unavailable_message,
};

fn events(backend: &mut dyn SerialBackend) -> Vec<SerialEvent> {
    let mut seen = Vec::new();
    while let Some(event) = backend.poll().unwrap() {
        seen.push(event);
    }
    seen
}

#[test]
fn a_new_port_is_closed_and_quiet() {
    let (mut backend, host) = memory_pair();
    assert_eq!(events(&mut backend), []);
    assert_eq!(backend.read(&mut [0; 16]).unwrap(), 0);
    assert_eq!(host.read(), b"");
    assert_eq!(host.signals(), ModemSignals::default());
    assert_eq!(backend.wait_handle(), WaitHandle::NONE);
    assert_eq!(backend.wait_handle().raw(), None);
    assert_eq!(WaitHandle::from_raw(42).raw(), Some(42));
}

#[test]
fn opening_and_closing_are_seen_with_dtr_and_rts() {
    let (mut backend, host) = memory_pair();
    host.open();
    assert_eq!(
        events(&mut backend),
        [
            SerialEvent::HostOpened,
            SerialEvent::Dtr(true),
            SerialEvent::Rts(true)
        ]
    );
    // Opening an open port changes nothing.
    host.open();
    assert_eq!(events(&mut backend), []);

    host.set_dtr(false);
    host.set_dtr(false);
    host.set_rts(false);
    host.set_dtr(true);
    assert_eq!(
        events(&mut backend),
        [
            SerialEvent::Dtr(false),
            SerialEvent::Rts(false),
            SerialEvent::Dtr(true)
        ]
    );

    host.write(b"never read");
    host.close();
    assert_eq!(
        events(&mut backend),
        [SerialEvent::Dtr(false), SerialEvent::HostClosed]
    );
    assert_eq!(backend.read(&mut [0; 16]).unwrap(), 0);
}

#[test]
fn bytes_pass_both_ways_in_order() {
    let (mut backend, host) = memory_pair();
    host.open();
    host.write(b"AT");
    host.write(b"\r");
    assert_eq!(host.unread_by_modem(), 3);
    let mut buffer = [0; 2];
    assert_eq!(backend.read(&mut buffer).unwrap(), 2);
    assert_eq!(&buffer, b"AT");
    assert_eq!(backend.read(&mut buffer).unwrap(), 1);
    assert_eq!(buffer[0], b'\r');
    assert_eq!(backend.read(&mut buffer).unwrap(), 0);

    assert_eq!(backend.write(b"\r\nOK").unwrap(), 4);
    assert_eq!(backend.write(b"\r\n").unwrap(), 2);
    assert_eq!(host.read_some(2), b"\r\n");
    assert_eq!(host.read(), b"OK\r\n");
    assert_eq!(host.read(), b"");
}

#[test]
fn a_full_host_makes_writes_come_up_short() {
    let (mut backend, host) = memory_pair();
    host.set_capacity(Some(5));
    assert_eq!(backend.write(b"12345678").unwrap(), 5);
    assert_eq!(backend.write(b"678").unwrap(), 0);
    assert_eq!(host.read_some(2), b"12");
    assert_eq!(backend.write(b"678").unwrap(), 2);
    host.set_capacity(None);
    assert_eq!(backend.write(b"8").unwrap(), 1);
    assert_eq!(host.read(), b"345678");
}

#[test]
fn signals_reach_the_host() {
    let (mut backend, host) = memory_pair();
    let signals = ModemSignals {
        dcd: true,
        dsr: true,
        cts: false,
        ri: true,
    };
    backend.set_signals(signals).unwrap();
    assert_eq!(host.signals(), signals);
}

#[test]
fn an_injected_failure_fails_every_call_until_cleared() {
    let (mut backend, host) = memory_pair();
    host.inject_failure(Some(io::ErrorKind::BrokenPipe));
    assert_eq!(
        backend.read(&mut [0; 4]).unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    assert!(backend.write(b"x").is_err());
    assert!(backend.set_signals(ModemSignals::default()).is_err());
    assert!(backend.poll().is_err());
    host.inject_failure(None);
    assert_eq!(backend.write(b"x").unwrap(), 1);
}

#[test]
fn the_host_can_tell_when_the_backend_is_gone() {
    let (backend, host) = memory_pair();
    assert!(host.backend_alive());
    drop(backend);
    assert!(!host.backend_alive());
}

#[test]
fn the_backend_is_a_send_trait_object() {
    let (backend, host) = memory_pair();
    let mut boxed: Box<dyn SerialBackend> = Box::new(backend);
    host.write(b"x");
    let handle = std::thread::spawn(move || {
        let mut buffer = [0; 1];
        boxed.read(&mut buffer).unwrap()
    });
    assert_eq!(handle.join().unwrap(), 1);
}

#[test]
fn io_errors_become_serial_messages() {
    let error = io::Error::new(io::ErrorKind::NotFound, "no such port");
    let cases = [
        (SerialOperation::Open, "RMD3001"),
        (SerialOperation::Read, "RMD3002"),
        (SerialOperation::Write, "RMD3003"),
        (SerialOperation::SetSignals, "RMD3004"),
        (SerialOperation::Poll, "RMD3005"),
        (SerialOperation::Configure, "RMD3006"),
    ];
    for (operation, id) in cases {
        let message = error_message("CNCB0", operation, &error);
        assert_eq!(message.id, id);
        assert!(message.text.contains("CNCB0"), "{}", message.text);
        assert!(message.text.contains("no such port"), "{}", message.text);
    }
    let message = unavailable_message("com0com");
    assert_eq!(message.id, "RMD3007");
    assert!(message.text.contains("com0com"));
}

#[cfg(windows)]
#[test]
fn opening_a_port_that_does_not_exist_is_message_3001() {
    let Err(message) = serial::Com0comBackend::open("RUSTMODEM_NO_SUCH_PORT") else {
        panic!("a port with this name should not exist");
    };
    assert_eq!(message.id, "RMD3001");
    assert!(message.text.contains("RUSTMODEM_NO_SUCH_PORT"));
}
