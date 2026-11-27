// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The detection phase of V.42 clause 7.2.1: the patterns bit for bit,
//! two modems that both have error control, and a modem against one that
//! sends nothing, one that sends data at once, one that does not want
//! error control and one that goes straight to the protocol.

use v42::{Detection, Framer, Outcome, Role, T400_MS};

/// T400 in samples.
const T400: u32 = T400_MS * 8;

/// The line rate the tests are timed for, and its samples to a bit.
const RATE: u32 = 1200;

fn bits(text: &str) -> Vec<bool> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c == '1')
        .collect()
}

/// Runs a detection against bits from somewhere else for some bit
/// times, and gives the bits it sent.
fn run(detection: &mut Detection, mut far: impl FnMut() -> bool, bit_times: u32) -> Vec<bool> {
    let mut sent = Vec::new();
    for n in 0..bit_times {
        // A bit is six samples and two thirds.
        detection.advance(if n % 3 == 0 { 6 } else { 7 });
        detection.receive_bit(far());
        sent.push(detection.next_bit());
        if detection.outcome() != Outcome::Pending {
            break;
        }
    }
    sent
}

/// Two detections against each other, with a line that takes `delay`
/// bit times each way. Gives the outcomes and how many bit times it
/// took.
fn pair(originator_wants: bool, answerer_wants: bool, delay: usize) -> (Outcome, Outcome, u32) {
    let mut originator = Detection::new(Role::Originator, T400, originator_wants);
    let mut answerer = Detection::new(Role::Answerer, T400, answerer_wants);
    let mut to_answerer = std::collections::VecDeque::from(vec![true; delay]);
    let mut to_originator = std::collections::VecDeque::from(vec![true; delay]);
    for n in 0..4 * RATE {
        let samples = if n % 3 == 0 { 6 } else { 7 };
        originator.advance(samples);
        answerer.advance(samples);
        to_answerer.push_back(originator.next_bit());
        to_originator.push_back(answerer.next_bit());
        answerer.receive_bit(to_answerer.pop_front().unwrap());
        originator.receive_bit(to_originator.pop_front().unwrap());
        if originator.outcome() != Outcome::Pending && answerer.outcome() != Outcome::Pending {
            return (originator.outcome(), answerer.outcome(), n);
        }
    }
    (originator.outcome(), answerer.outcome(), 4 * RATE)
}

#[test]
fn the_patterns_are_those_of_clause_7_2_1_bit_for_bit() {
    // Clause 7.2.1.2, left to right in order of transmission: DC1 with
    // even parity, 8 to 16 ones, DC1 with odd parity, 8 to 16 ones.
    let odp = bits("0 1000 1000 1 11111111  0 1000 1001 1 11111111");
    let mut originator = Detection::new(Role::Originator, T400, true);
    let sent = run(&mut originator, || true, 72);
    assert_eq!(sent[..36], odp[..]);
    assert_eq!(sent[36..72], odp[..]);

    // Table 3: E and C, the low-order bit first, with 8 to 16 ones after
    // each; sent once the originator's pattern is seen, which is four
    // DC1s of alternating parity (clause 7.2.1.3).
    let adp = bits("0 1010 0010 1 11111111  0 1100 0010 1 11111111");
    let mut answerer = Detection::new(Role::Answerer, T400, true);
    let mut far = odp.iter().copied().cycle();
    // Until the fourth DC1 has ended, which is with the 64th bit, it
    // sends ones.
    let before = run(&mut answerer, || far.next().unwrap(), 63);
    assert!(before.iter().all(|&bit| bit));
    let sent = run(&mut answerer, || far.next().unwrap(), 400);
    let start = sent.iter().position(|&bit| !bit).expect("no reply");
    assert!(start < 8);
    // At least ten times (clause 7.2.1.3), and then it is done.
    for n in 0..10 {
        assert_eq!(
            sent[start + 36 * n..start + 36 * (n + 1)],
            adp[..],
            "repeat {n}"
        );
    }
    assert_eq!(answerer.outcome(), Outcome::ErrorControl);

    // An answerer that wants no error control says so: E and NUL.
    let none = bits("0 1010 0010 1 11111111  0 0000 0000 1 11111111");
    let mut answerer = Detection::new(Role::Answerer, T400, false);
    let mut far = odp.iter().copied().cycle();
    run(&mut answerer, || far.next().unwrap(), 63);
    let sent = run(&mut answerer, || far.next().unwrap(), 400);
    let start = sent.iter().position(|&bit| !bit).unwrap();
    assert_eq!(sent[start..start + 36], none[..]);
    assert_eq!(answerer.outcome(), Outcome::None);
}

#[test]
fn two_modems_with_error_control_find_each_other() {
    // On a line that takes nothing, and on one that takes 300 ms each
    // way, a satellite's worth.
    for delay in [0, 12, 360] {
        let (originator, answerer, bit_times) = pair(true, true, delay);
        assert_eq!(originator, Outcome::ErrorControl, "delay {delay}");
        assert_eq!(answerer, Outcome::ErrorControl, "delay {delay}");
        // The originator has it from two adjacent patterns (clause
        // 7.2.1.2): four DC1s out, the delay there and back, and four
        // characters of reply. Well inside T400 but for the satellite.
        let ms = bit_times * 1000 / RATE;
        println!("delay {delay} bit times: done in {ms} ms");
        assert!(ms < 400 + 2 * delay as u32 * 1000 / RATE + 100, "{ms} ms");
    }
}

#[test]
fn an_answerer_that_wants_none_is_believed_at_once() {
    let (originator, answerer, bit_times) = pair(true, false, 12);
    assert_eq!(originator, Outcome::None);
    assert_eq!(answerer, Outcome::None);
    // The originator does not wait for its timer: it has E and NUL
    // twice.
    assert!(bit_times * 1000 / RATE < 600);
}

#[test]
fn a_far_end_that_sends_nothing_is_given_t400() {
    // A modem without error control sends ones while its host is idle.
    for role in [Role::Originator, Role::Answerer] {
        let mut detection = Detection::new(role, T400, true);
        let sent = run(&mut detection, || true, 4 * RATE);
        assert_eq!(detection.outcome(), Outcome::None, "{role:?}");
        // T400 is 750 ms (clause 9.1.1): 900 bit times at 1200 bit/s. The
        // originator finishes the pattern it is in (clause 7.2.1.2).
        let bit_times = sent.len() as u32;
        assert!(
            (900..=900 + 36).contains(&bit_times),
            "{role:?}: {bit_times} bit times"
        );
        if role == Role::Answerer {
            // And an answerer sends ones throughout (clause 7.2.1.3).
            assert!(sent.iter().all(|&bit| bit));
        }
    }
    // The line dead at 0 does no better.
    let mut detection = Detection::new(Role::Answerer, T400, true);
    run(&mut detection, || false, 4 * RATE);
    assert_eq!(detection.outcome(), Outcome::None);
}

#[test]
fn a_far_end_that_sends_data_at_once_is_not_taken_for_a_pattern() {
    // Start-stop characters of a host that begins to talk: text, the
    // character DC1 itself over and over, E and C in the wrong order,
    // and noise.
    let character = |c: u8| {
        let mut frame = vec![false];
        frame.extend((0..8).map(|n| (c >> n) & 1 == 1));
        frame.push(true);
        frame
    };
    let streams: Vec<(&str, Vec<bool>)> = vec![
        (
            "text",
            b"login: user\r\npassword: "
                .iter()
                .flat_map(|&c| character(c))
                .collect(),
        ),
        (
            "DC1 of one parity",
            std::iter::repeat_n(character(0x11), 40).flatten().collect(),
        ),
        (
            "E E C C",
            b"EECC".iter().flat_map(|&c| character(c)).collect(),
        ),
        (
            "E and something else",
            b"ED".iter().flat_map(|&c| character(c)).collect(),
        ),
    ];
    for (name, stream) in &streams {
        for role in [Role::Originator, Role::Answerer] {
            let mut detection = Detection::new(role, T400, true);
            let mut far = stream.iter().copied().cycle();
            run(&mut detection, || far.next().unwrap(), 4 * RATE);
            assert_eq!(detection.outcome(), Outcome::None, "{name}, {role:?}");
        }
    }
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    for role in [Role::Originator, Role::Answerer] {
        let mut detection = Detection::new(role, T400, true);
        run(
            &mut detection,
            || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state & 1 == 1
            },
            4 * RATE,
        );
        assert_eq!(detection.outcome(), Outcome::None, "noise, {role:?}");
    }
}

#[test]
fn an_originator_that_goes_straight_to_the_protocol_is_followed() {
    // Clause 7.2.1.3: continuous flags are the start of the protocol
    // phase, from an originator whose detection phase is disabled.
    let mut framer = Framer::new();
    let mut answerer = Detection::new(Role::Answerer, T400, true);
    let sent = run(&mut answerer, || framer.next_bit(), 200);
    assert_eq!(answerer.outcome(), Outcome::ErrorControl);
    // On the third flag.
    assert_eq!(sent.len(), 24);
    // One flag in the middle of something else is not that.
    let mut answerer = Detection::new(Role::Answerer, T400, true);
    let stream = bits("1111 01111110 1111 0 1010 0101 1 1111 01111110 1111111111");
    let mut far = stream.iter().copied().cycle();
    run(&mut answerer, || far.next().unwrap(), 300);
    assert_eq!(answerer.outcome(), Outcome::Pending);
}
