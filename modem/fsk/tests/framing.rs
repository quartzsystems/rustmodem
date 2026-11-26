// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Start-stop framing by itself: bits made by hand, with no modem.

use fsk::{Framed, StartStopReceiver, StartStopTransmitter, TX_CAPACITY};

/// The frame of a character: start, eight data bits from the least
/// significant, stop.
fn frame(byte: u8) -> Vec<bool> {
    let mut bits = vec![false];
    bits.extend((0..8).map(|n| byte >> n & 1 == 1));
    bits.push(true);
    bits
}

/// Bits as a line a receiver samples, each bit `samples_a_bit` long. The
/// length need not be whole: the edges fall where they fall.
fn line(bits: &[bool], samples_a_bit: f64) -> Vec<bool> {
    let len = (bits.len() as f64 * samples_a_bit) as usize;
    (0..len)
        .map(|n| bits[((n as f64 / samples_a_bit) as usize).min(bits.len() - 1)])
        .collect()
}

/// What a receiver makes of a line.
#[derive(Default)]
struct Got {
    characters: Vec<u8>,
    bits: Vec<bool>,
    framing_errors: u32,
    breaks: u32,
}

fn receive(receiver: &mut StartStopReceiver, line: &[bool]) -> Got {
    let mut got = Got::default();
    for &level in line {
        let out = receiver.process(level);
        got.characters.extend(out.character);
        got.bits.extend(out.bit);
        got.framing_errors += u32::from(out.framing_error);
        got.breaks += u32::from(out.line_break);
    }
    got
}

const NOMINAL: f64 = 8000.0 / 300.0;

#[test]
fn the_transmitter_frames_a_character_least_significant_bit_first() {
    let mut transmitter = StartStopTransmitter::new();
    assert!(transmitter.is_idle());
    // With nothing to send the line is at mark.
    assert!((0..25).all(|_| transmitter.next_bit()));
    assert_eq!(transmitter.write(&[0x53, 0x00, 0xFF]), 3);
    assert_eq!(transmitter.queued(), 3);
    let bits: Vec<bool> = (0..30).map(|_| transmitter.next_bit()).collect();
    // 0x53 is 0101 0011: start 0, then 1 1 0 0 1 0 1 0, then stop 1.
    let as_digits: String = bits.iter().map(|&b| if b { '1' } else { '0' }).collect();
    assert_eq!(&as_digits[..10], "0110010101");
    assert_eq!(&as_digits[10..20], "0000000001");
    assert_eq!(&as_digits[20..], "0111111111");
    assert_eq!(bits[..10], frame(0x53)[..]);
    assert!(transmitter.is_idle());
    assert!(transmitter.next_bit());
}

#[test]
fn the_transmitter_takes_what_fits_and_no_more() {
    let mut transmitter = StartStopTransmitter::new();
    let data = vec![0xA5u8; TX_CAPACITY + 50];
    assert_eq!(transmitter.write(&data), TX_CAPACITY);
    assert_eq!(transmitter.write(&data), 0);
    // A character's bits go out, and there is room for one more.
    for _ in 0..10 {
        transmitter.next_bit();
    }
    assert_eq!(transmitter.write(&[1, 2, 3]), 1);
    assert_eq!(transmitter.queued(), TX_CAPACITY);
    // The queue wraps and keeps its order.
    let mut out = Vec::new();
    for _ in 0..TX_CAPACITY {
        let bits: Vec<bool> = (0..10).map(|_| transmitter.next_bit()).collect();
        let byte = (0..8).fold(0u8, |byte, n| byte | (u8::from(bits[n + 1]) << n));
        out.push(byte);
    }
    assert_eq!(out[TX_CAPACITY - 1], 1);
    assert!(out[..TX_CAPACITY - 1].iter().all(|&byte| byte == 0xA5));
    transmitter.write(&[9, 9]);
    transmitter.next_bit();
    transmitter.clear();
    assert!(transmitter.is_idle());
    assert!(StartStopTransmitter::default().is_idle());
}

#[test]
fn every_character_is_received_back_to_back_and_apart() {
    let mut bits = vec![true; 5];
    for byte in 0..=255u8 {
        bits.extend(frame(byte));
        // Some with nothing between them, some with an idle line.
        bits.extend(std::iter::repeat_n(true, usize::from(byte % 4)));
    }
    bits.extend([true; 5]);
    let mut receiver = StartStopReceiver::new();
    let got = receive(&mut receiver, &line(&bits, NOMINAL));
    assert_eq!(got.characters, (0..=255).collect::<Vec<u8>>());
    assert_eq!(got.framing_errors + got.breaks, 0);
    assert_eq!(receiver.framing_errors(), 0);
    // The bit stream it gives is the line's bits, to the one at each end.
    let ones = |bits: &[bool]| bits.iter().filter(|&&b| b).count();
    assert!(got.bits.len().abs_diff(bits.len()) <= 2);
    assert!(ones(&got.bits).abs_diff(ones(&bits)) <= 2);
}

#[test]
fn the_bit_stream_has_every_bit_of_a_frame_in_order() {
    // Ten marks, and the frames of two characters: what a V.8 signal is
    // made of.
    let mut bits = vec![true; 10];
    bits.extend(frame(0xE0));
    bits.extend(frame(0xC1));
    bits.extend([true; 3]);
    let got = receive(&mut StartStopReceiver::new(), &line(&bits, NOMINAL));
    assert_eq!(got.characters, [0xE0, 0xC1]);
    // The last idle mark may not have reached its middle.
    assert!(got.bits.len() >= bits.len() - 1);
    assert_eq!(got.bits[..], bits[..got.bits.len()]);
}

#[test]
fn a_character_may_begin_at_any_moment() {
    // The far end's host starts a character when it likes: after an idle
    // time that is no whole number of bits.
    for gap in 0..60 {
        let mut samples = vec![true; 30 + gap];
        samples.extend(line(&frame(0x5A), NOMINAL));
        samples.extend(vec![true; 17 + gap % 7]);
        samples.extend(line(&frame(0xC3), NOMINAL));
        samples.extend(vec![true; 40]);
        let got = receive(&mut StartStopReceiver::new(), &samples);
        assert_eq!(got.characters, [0x5A, 0xC3], "gap {gap}");
        assert_eq!(got.framing_errors, 0);
    }
}

#[test]
fn a_far_end_whose_clock_is_out_is_followed() {
    // Three percent either way: far more than any modem's clock, and
    // what a mechanical terminal's might be.
    for percent in [-3.0, -1.0, -0.01, 0.01, 1.0, 3.0] {
        let samples_a_bit = NOMINAL * (1.0 + percent / 100.0);
        let mut bits = vec![true; 3];
        for byte in 0..=255u8 {
            bits.extend(frame(byte));
        }
        bits.extend([true; 3]);
        let got = receive(&mut StartStopReceiver::new(), &line(&bits, samples_a_bit));
        assert_eq!(
            got.characters,
            (0..=255).collect::<Vec<u8>>(),
            "{percent} percent"
        );
        assert_eq!(got.framing_errors, 0, "{percent} percent");
    }
}

#[test]
fn a_moment_of_noise_on_an_idle_line_is_not_a_character() {
    let mut samples = vec![true; 100];
    // Space for a third of a bit.
    samples.extend([false; 9]);
    samples.extend(vec![true; 400]);
    samples.extend(line(&frame(0x41), NOMINAL));
    samples.extend(vec![true; 40]);
    let got = receive(&mut StartStopReceiver::new(), &samples);
    assert_eq!(got.characters, [0x41]);
    assert_eq!(got.framing_errors + got.breaks, 0);
}

#[test]
fn a_wrong_stop_bit_loses_the_character_and_is_counted() {
    let mut bits = vec![true; 4];
    bits.extend(frame(0x31));
    // A frame whose stop bit is space.
    let mut bad = frame(0x32);
    bad[9] = false;
    bits.extend(bad);
    bits.extend([true; 4]);
    bits.extend(frame(0x33));
    bits.extend([true; 4]);
    let mut receiver = StartStopReceiver::new();
    let got = receive(&mut receiver, &line(&bits, NOMINAL));
    assert_eq!(got.characters, [0x31, 0x33]);
    assert_eq!(got.framing_errors, 1);
    assert_eq!(got.breaks, 0);
    assert_eq!(receiver.framing_errors(), 1);
    // The count outlives a reset of the line; a new receiver starts at
    // nothing.
    receiver.reset();
    assert_eq!(receiver.framing_errors(), 1);
}

#[test]
fn a_line_held_at_space_is_a_break_reported_once() {
    for held in [10usize, 15, 40, 300] {
        let mut bits = vec![true; 4];
        bits.extend(frame(0x61));
        bits.extend(vec![false; held]);
        bits.extend([true; 4]);
        bits.extend(frame(0x62));
        bits.extend([true; 4]);
        let got = receive(&mut StartStopReceiver::new(), &line(&bits, NOMINAL));
        assert_eq!(got.characters, [0x61, 0x62], "held for {held} bits");
        assert_eq!(got.breaks, 1, "held for {held} bits");
        assert_eq!(got.framing_errors, 0);
    }
    // A character of all zeros with its stop bit is a character.
    let mut bits = vec![true; 4];
    bits.extend(frame(0x00));
    bits.extend([true; 4]);
    let got = receive(&mut StartStopReceiver::new(), &line(&bits, NOMINAL));
    assert_eq!(got.characters, [0x00]);
    assert_eq!(got.breaks, 0);
}

#[test]
fn a_reset_drops_the_character_in_progress() {
    let mut receiver = StartStopReceiver::new();
    let samples = line(&frame(0x77), NOMINAL);
    receive(&mut receiver, &samples[..150]);
    receiver.reset();
    let mut after = vec![true; 60];
    after.extend(line(&frame(0x78), NOMINAL));
    after.extend(vec![true; 40]);
    let got = receive(&mut receiver, &after);
    assert_eq!(got.characters, [0x78]);
    assert_eq!(
        StartStopReceiver::default().process(true),
        Framed::default()
    );
}
