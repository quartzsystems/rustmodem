// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The echo engine.

use engine::{
    ConnectInfo, ECHO_CARRIER, EchoEngine, FRAME_10MS, FRAME_20MS, ModemEngine, ModemEvent,
    SILENCE_OCTET,
};
use proptest::prelude::*;

fn connected() -> ModemEvent {
    ModemEvent::Connect(ConnectInfo {
        rate: 0,
        carrier: Some("ECHO".to_owned()),
        protocol: None,
        compression: None,
    })
}

#[test]
fn it_connects_at_once_at_rate_0_on_the_carrier_echo() {
    for answer in [true, false] {
        let mut engine = EchoEngine::new();
        assert_eq!(engine.poll_event(), None);
        if answer {
            engine.answer();
        } else {
            engine.originate();
        }
        assert_eq!(engine.poll_event(), Some(connected()));
        assert_eq!(engine.poll_event(), None);
        // Starting what is started says nothing more.
        engine.answer();
        engine.originate();
        assert_eq!(engine.poll_event(), None);
    }
    assert_eq!(ECHO_CARRIER, "ECHO");
}

#[test]
fn a_stopped_engine_sends_silence() {
    let mut engine = EchoEngine::new();
    let mut tx = [0u8; FRAME_20MS];
    engine.process(&[0x55; FRAME_20MS], &mut tx);
    assert_eq!(tx, [SILENCE_OCTET; FRAME_20MS]);

    engine.answer();
    engine.process(&[0x55; FRAME_20MS], &mut tx);
    assert_eq!(tx, [0x55; FRAME_20MS]);

    engine.hangup();
    engine.process(&[0x55; FRAME_20MS], &mut tx);
    assert_eq!(tx, [SILENCE_OCTET; FRAME_20MS]);
    assert_eq!(engine.poll_event(), None);

    // And it can be started again.
    engine.originate();
    assert_eq!(engine.poll_event(), Some(connected()));
}

#[test]
fn data_from_the_host_is_taken_and_dropped() {
    let mut engine = EchoEngine::new();
    assert_eq!(engine.dte_write(b"before the call"), 0);
    engine.answer();
    assert_eq!(engine.dte_write(b"hello"), 5);
    assert_eq!(engine.dte_write(&[0u8; 100_000]), 100_000);
    let mut buffer = [0u8; 64];
    assert_eq!(engine.dte_read(&mut buffer), 0);
    // It is not in what is sent either.
    let mut tx = [0u8; FRAME_10MS];
    engine.process(&[0x11; FRAME_10MS], &mut tx);
    assert_eq!(tx, [0x11; FRAME_10MS]);
    // The event of a hangup that came before it was read is gone.
    let mut engine = EchoEngine::new();
    engine.answer();
    engine.hangup();
    assert_eq!(engine.poll_event(), None);
}

proptest! {
    /// Whatever arrives goes back, octet for octet, at either frame size.
    #[test]
    fn what_is_received_is_what_is_sent(
        frames in proptest::collection::vec(
            prop_oneof![
                proptest::collection::vec(any::<u8>(), FRAME_10MS),
                proptest::collection::vec(any::<u8>(), FRAME_20MS),
            ],
            1..20,
        )
    ) {
        let mut engine = EchoEngine::new();
        engine.originate();
        for rx in frames {
            let mut tx = vec![0u8; rx.len()];
            engine.process(&rx, &mut tx);
            prop_assert_eq!(tx, rx);
        }
    }
}
