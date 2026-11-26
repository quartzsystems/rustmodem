// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The null engine: connection, transparent data and carrier loss.

use engine::{
    ConnectInfo, FRAME_10MS, FRAME_20MS, IDLE_OCTET, ModemEngine, ModemEvent, NullConfig,
    NullEngine, SILENCE_OCTET,
};
use proptest::prelude::*;

/// Two engines joined back to back: what each sends in one tick is what the
/// other receives in the next.
struct Pair {
    a: Box<dyn ModemEngine>,
    b: Box<dyn ModemEngine>,
    a_to_b: Vec<u8>,
    b_to_a: Vec<u8>,
}

impl Pair {
    fn new(config: NullConfig, frame: usize) -> Pair {
        Pair {
            a: Box::new(NullEngine::new(config)),
            b: Box::new(NullEngine::new(config)),
            a_to_b: vec![SILENCE_OCTET; frame],
            b_to_a: vec![SILENCE_OCTET; frame],
        }
    }

    fn tick(&mut self) {
        let mut from_a = vec![0; self.a_to_b.len()];
        let mut from_b = vec![0; self.b_to_a.len()];
        self.a.process(&self.b_to_a, &mut from_a);
        self.b.process(&self.a_to_b, &mut from_b);
        self.a_to_b = from_a;
        self.b_to_a = from_b;
    }

    fn connect(&mut self) {
        self.a.originate();
        self.b.answer();
        assert_eq!(
            self.a.poll_event(),
            Some(ModemEvent::Connect(ConnectInfo::unknown()))
        );
        assert_eq!(
            self.b.poll_event(),
            Some(ModemEvent::Connect(ConnectInfo::unknown()))
        );
    }
}

fn read_all(engine: &mut dyn ModemEngine, into: &mut Vec<u8>) {
    let mut buffer = [0; 97];
    loop {
        let count = engine.dte_read(&mut buffer);
        if count == 0 {
            break;
        }
        into.extend_from_slice(&buffer[..count]);
    }
}

/// Sends `a_data` from a to b and `b_data` from b to a, in pieces, and
/// returns what each side received.
fn exchange(pair: &mut Pair, a_data: &[u8], b_data: &[u8], piece: usize) -> (Vec<u8>, Vec<u8>) {
    let (mut a_sent, mut b_sent) = (0, 0);
    let (mut a_got, mut b_got) = (Vec::new(), Vec::new());
    let mut quiet_ticks = 0;
    while quiet_ticks < 4 {
        let a_end = (a_sent + piece).min(a_data.len());
        a_sent += pair.a.dte_write(&a_data[a_sent..a_end]);
        let b_end = (b_sent + piece).min(b_data.len());
        b_sent += pair.b.dte_write(&b_data[b_sent..b_end]);
        pair.tick();
        let before = a_got.len() + b_got.len();
        read_all(pair.a.as_mut(), &mut a_got);
        read_all(pair.b.as_mut(), &mut b_got);
        let moved = a_got.len() + b_got.len() != before;
        let all_sent = a_sent == a_data.len() && b_sent == b_data.len();
        quiet_ticks = if all_sent && !moved {
            quiet_ticks + 1
        } else {
            0
        };
    }
    (a_got, b_got)
}

#[test]
fn a_stopped_engine_sends_silence_and_takes_no_data() {
    let mut engine = NullEngine::new(NullConfig::default());
    let rx = [IDLE_OCTET; FRAME_20MS];
    let mut tx = [0; FRAME_20MS];
    engine.process(&rx, &mut tx);
    assert!(tx.iter().all(|o| *o == SILENCE_OCTET));
    assert_eq!(engine.dte_write(b"data"), 0);
    assert_eq!(engine.dte_read(&mut [0; 8]), 0);
    assert_eq!(engine.poll_event(), None);
    assert!(!engine.is_connected());
}

#[test]
fn a_started_engine_with_no_data_sends_the_idle_pattern() {
    let mut engine = NullEngine::new(NullConfig::default());
    engine.answer();
    assert!(engine.is_connected());
    let rx = [SILENCE_OCTET; FRAME_20MS];
    let mut tx = [0; FRAME_20MS];
    engine.process(&rx, &mut tx);
    assert!(tx.iter().all(|o| *o == IDLE_OCTET));
}

#[test]
fn connect_is_reported_at_once_with_no_delay_and_at_rate_zero() {
    let mut pair = Pair::new(NullConfig::default(), FRAME_20MS);
    pair.connect();
    assert_eq!(pair.a.poll_event(), None);
    // Starting an engine that is already started does nothing.
    pair.a.originate();
    pair.a.answer();
    assert_eq!(pair.a.poll_event(), None);
}

#[test]
fn connect_is_reported_after_the_configured_ticks() {
    let config = NullConfig {
        connect_delay_ticks: 5,
        ..NullConfig::default()
    };
    let mut pair = Pair::new(config, FRAME_20MS);
    pair.a.originate();
    pair.b.answer();
    for _ in 0..4 {
        pair.tick();
        assert_eq!(pair.a.poll_event(), None);
        assert_eq!(pair.b.poll_event(), None);
        assert_eq!(pair.a.dte_write(b"early"), 0);
    }
    pair.tick();
    assert_eq!(
        pair.a.poll_event(),
        Some(ModemEvent::Connect(ConnectInfo::unknown()))
    );
    assert_eq!(
        pair.b.poll_event(),
        Some(ModemEvent::Connect(ConnectInfo::unknown()))
    );
    let (a_got, b_got) = exchange(&mut pair, b"from a", b"from b", 64);
    assert_eq!(a_got, b"from b");
    assert_eq!(b_got, b"from a");
}

#[test]
fn every_byte_value_passes_including_the_reserved_octets() {
    for frame in [FRAME_10MS, FRAME_20MS] {
        let mut pair = Pair::new(NullConfig::default(), frame);
        pair.connect();
        let mut data: Vec<u8> = (0..=255).collect();
        // Runs of reserved octets, so escapes fall on tick boundaries.
        data.extend(std::iter::repeat_n([0xFF, 0x7E, 0x7D], 200).flatten());
        data.extend((0..=255).rev());
        let reversed: Vec<u8> = data.iter().rev().copied().collect();
        let (a_got, b_got) = exchange(&mut pair, &data, &reversed, 50);
        assert_eq!(b_got, data);
        assert_eq!(a_got, reversed);
    }
}

#[test]
fn dte_write_pushes_back_when_the_engine_is_full() {
    let mut engine = NullEngine::new(NullConfig::default());
    engine.originate();
    let big = vec![b'x'; 100_000];
    let accepted = engine.dte_write(&big);
    assert!(accepted > 0 && accepted < big.len());
    assert_eq!(engine.dte_write(&big), 0);
    // A tick drains some, and then there is room again.
    let rx = [IDLE_OCTET; FRAME_20MS];
    let mut tx = [0; FRAME_20MS];
    engine.process(&rx, &mut tx);
    assert_eq!(engine.dte_write(&big), FRAME_20MS);
}

#[test]
fn losing_the_idle_pattern_for_the_configured_time_is_carrier_lost() {
    let config = NullConfig {
        connect_delay_ticks: 0,
        carrier_loss_ticks: 7,
    };
    let mut pair = Pair::new(config, FRAME_20MS);
    pair.connect();
    let (a_got, _) = exchange(&mut pair, b"", b"before the loss", 64);
    assert_eq!(a_got, b"before the loss");

    pair.b.hangup();
    // One tick for the last carrier octets from b to reach a, then seven of
    // silence.
    pair.tick();
    for _ in 0..6 {
        pair.tick();
        assert_eq!(pair.a.poll_event(), None);
    }
    pair.tick();
    assert_eq!(pair.a.poll_event(), Some(ModemEvent::CarrierLost));
    assert_eq!(pair.a.poll_event(), None);
    assert_eq!(pair.a.dte_write(b"too late"), 0);
    // b was hung up by its host; it reports nothing.
    assert_eq!(pair.b.poll_event(), None);

    // Both can be started again.
    pair.tick();
    pair.connect();
    let (a_got, b_got) = exchange(&mut pair, b"again", b"once more", 64);
    assert_eq!(a_got, b"once more");
    assert_eq!(b_got, b"again");
}

#[test]
fn silence_before_the_carrier_has_been_seen_is_not_carrier_loss() {
    let mut engine = NullEngine::new(NullConfig::default());
    engine.originate();
    assert!(matches!(engine.poll_event(), Some(ModemEvent::Connect(_))));
    let rx = [SILENCE_OCTET; FRAME_20MS];
    let mut tx = [0; FRAME_20MS];
    for _ in 0..100 {
        engine.process(&rx, &mut tx);
    }
    assert_eq!(engine.poll_event(), None);
    assert!(engine.is_connected());
}

#[test]
fn hangup_discards_everything() {
    let mut pair = Pair::new(NullConfig::default(), FRAME_20MS);
    pair.connect();
    pair.a.dte_write(b"never sent");
    pair.a.hangup();
    assert_eq!(pair.a.poll_event(), None);
    let mut tx = [0; FRAME_20MS];
    pair.a.process(&[IDLE_OCTET; FRAME_20MS], &mut tx);
    assert!(tx.iter().all(|o| *o == SILENCE_OCTET));
    assert_eq!(pair.a.dte_read(&mut [0; 8]), 0);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 200,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn any_data_passes_unchanged_in_both_directions(
        a_data in prop::collection::vec(any::<u8>(), 0..3000),
        b_data in prop::collection::vec(
            prop_oneof![Just(0xFFu8), Just(0x7E), Just(0x7D), Just(0x5E), any::<u8>()],
            0..3000,
        ),
        piece in 1usize..400,
        short_frames in any::<bool>(),
    ) {
        let frame = if short_frames { FRAME_10MS } else { FRAME_20MS };
        let mut pair = Pair::new(NullConfig::default(), frame);
        pair.connect();
        let (a_got, b_got) = exchange(&mut pair, &a_data, &b_data, piece);
        prop_assert_eq!(a_got, b_data);
        prop_assert_eq!(b_got, a_data);
    }
}
