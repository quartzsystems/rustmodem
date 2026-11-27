// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The converter of V.14: the formats, round trips, a host faster than
//! the line, bit errors, and breaks.

use proptest::prelude::*;
use v14::{Format, Parity, Received, Receiver, TX_CAPACITY, Transmitter};

const FORMATS: [Format; 3] = [Format::N81, Format::E71, Format::O71];

/// Numbers from a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 16) % n
    }
}

fn bits(text: &str) -> Vec<bool> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c == '1')
        .collect()
}

/// The stream a transmitter makes of characters given all at once, until
/// it has sent them and `tail` bits of idle after.
fn stream(transmitter: &mut Transmitter, characters: &[u8], tail: usize) -> Vec<bool> {
    let mut rest = characters;
    let mut out = Vec::new();
    loop {
        let taken = transmitter.write(rest);
        rest = &rest[taken..];
        if rest.is_empty() && transmitter.idle() {
            break;
        }
        out.push(transmitter.next_bit());
    }
    out.extend((0..tail).map(|_| transmitter.next_bit()));
    out
}

/// What a receiver makes of a stream.
#[derive(Default)]
struct Got {
    characters: Vec<u8>,
    framing_errors: usize,
    parity_errors: usize,
    breaks: usize,
}

fn receive(receiver: &mut Receiver, stream: &[bool]) -> Got {
    let mut got = Got::default();
    for &bit in stream {
        let out: Received = receiver.process(bit);
        got.characters.extend_from_slice(out.characters());
        got.framing_errors += usize::from(out.framing_error);
        got.parity_errors += usize::from(out.parity_error);
        got.breaks += usize::from(out.break_began);
    }
    got
}

#[test]
fn the_formats_are_ten_bits_and_carry_their_parity() {
    for format in FORMATS {
        // Clause 4 b): a start element, eight units and a stop element.
        assert_eq!(format.character_bits(), 10);
        assert_eq!(format.payload_bits(), 8);
        // Clause 7.3: a break is 2M + 3 bits.
        assert_eq!(format.break_bits(), 23);
    }
    assert_eq!(
        (Format::N81.data_bits(), Format::N81.parity()),
        (8, Parity::None)
    );
    assert_eq!(
        (Format::E71.data_bits(), Format::E71.parity()),
        (7, Parity::Even)
    );
    assert_eq!(
        (Format::O71.data_bits(), Format::O71.parity()),
        (7, Parity::Odd)
    );
    assert_eq!(Format::default(), Format::N81);

    // 'A' is 100 0001: two ones. 'C' is 100 0011: three.
    assert_eq!(Format::N81.payload(0xC1), 0xC1);
    assert_eq!(Format::E71.payload(b'A'), 0x41);
    assert_eq!(Format::E71.payload(b'C'), 0xC3);
    assert_eq!(Format::O71.payload(b'A'), 0xC1);
    assert_eq!(Format::O71.payload(b'C'), 0x43);
    // What a format of seven units does not carry is left out.
    assert_eq!(Format::E71.payload(0xC1), Format::E71.payload(0x41));

    for format in FORMATS {
        for character in 0..=255u8 {
            let payload = format.payload(character);
            assert_eq!(format.character(payload), (character & format.mask(), true));
            if format.parity() != Parity::None {
                // Any one unit wrong is a parity error.
                for unit in 0..8 {
                    assert!(!format.character(payload ^ (1 << unit)).1);
                }
            }
        }
    }
}

#[test]
fn a_character_is_a_start_element_its_units_and_a_stop_element() {
    // 'A', 0x41, least significant bit first, between a 0 and a 1; the
    // stop polarity before and after.
    let mut transmitter = Transmitter::new(Format::N81);
    assert!(transmitter.next_bit() && transmitter.next_bit());
    assert_eq!(transmitter.write(b"A"), 1);
    let sent: Vec<bool> = (0..13).map(|_| transmitter.next_bit()).collect();
    assert_eq!(sent, bits("0 10000010 1 111"));
    assert!(transmitter.idle());

    // With even parity 'C' has a parity unit of 1, with odd of 0.
    let mut transmitter = Transmitter::new(Format::E71);
    transmitter.write(b"C");
    let sent: Vec<bool> = (0..10).map(|_| transmitter.next_bit()).collect();
    assert_eq!(sent, bits("0 1100001 1 1"));
    let mut transmitter = Transmitter::new(Format::O71);
    transmitter.write(b"C");
    let sent: Vec<bool> = (0..10).map(|_| transmitter.next_bit()).collect();
    assert_eq!(sent, bits("0 1100001 0 1"));

    // Two characters given together follow one another with nothing
    // between: clause 4, "continuously".
    let mut transmitter = Transmitter::new(Format::N81);
    transmitter.set_deletion(false);
    transmitter.write(&[0x00, 0xFF]);
    let sent: Vec<bool> = (0..21).map(|_| transmitter.next_bit()).collect();
    assert_eq!(sent, bits("0 00000000 1  0 11111111 1  1"));
}

#[test]
fn characters_go_round_in_every_format() {
    for format in FORMATS {
        for deletion in [false, true] {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let mut transmitter = Transmitter::new(format);
            transmitter.set_deletion(deletion);
            let mut receiver = Receiver::new(format);
            let mut sent = Vec::new();
            let mut got = Got::default();
            // Characters in bursts of 1 to 100, with up to 50 bits of
            // nothing between: a host slower than the line, as fast, and
            // faster.
            for _ in 0..400 {
                let burst: Vec<u8> = (0..1 + rng.below(100)).map(|_| rng.next() as u8).collect();
                sent.extend(burst.iter().map(|c| c & format.mask()));
                let tail = rng.below(50) as usize;
                let line = stream(&mut transmitter, &burst, tail);
                let more = receive(&mut receiver, &line);
                got.characters.extend(more.characters);
                got.framing_errors += more.framing_errors;
                got.parity_errors += more.parity_errors;
                got.breaks += more.breaks;
            }
            assert_eq!(got.characters, sent, "{format:?}, deletion {deletion}");
            assert_eq!(
                (got.framing_errors, got.parity_errors, got.breaks),
                (0, 0, 0)
            );
            assert_eq!(receiver.reinserted_stops(), transmitter.deleted_stops());
            assert_eq!(transmitter.deleted_stops() > 0, deletion);
        }
    }
}

#[test]
fn a_host_faster_than_the_line_has_one_stop_element_in_eight_left_out() {
    for format in FORMATS {
        let mut rng = Rng(77);
        let characters: Vec<u8> = (0..8000)
            .map(|_| rng.next() as u8 & format.mask())
            .collect();
        let mut transmitter = Transmitter::new(format);
        let line = stream(&mut transmitter, &characters, 0);
        // Clause 7.1.1: no more than one for any eight consecutive
        // characters; and with a character always waiting, no fewer.
        assert_eq!(transmitter.deleted_stops(), 1000);
        assert_eq!(line.len(), 8000 * 10 - 1000);

        // Which stop elements: walked through the stream by its own
        // rules, a start element being wherever a character ends.
        let mut at = 0;
        let mut left_out = Vec::new();
        for (n, &character) in characters.iter().enumerate() {
            assert!(!line[at], "{format:?}: character {n} has no start element");
            let mut payload = 0u16;
            for unit in 0..8 {
                payload |= u16::from(line[at + 1 + unit]) << unit;
            }
            assert_eq!(format.character(payload), (character, true));
            at += 9;
            if at < line.len() && line[at] {
                at += 1;
            } else {
                left_out.push(n);
            }
        }
        assert_eq!(left_out.len(), 1000);
        assert!(left_out.windows(2).all(|pair| pair[1] - pair[0] >= 8));

        // The receiver gives every character, and takes each stop element
        // left out as read (clause 7.2).
        let mut receiver = Receiver::new(format);
        let got = receive(&mut receiver, &line);
        assert_eq!(got.characters, characters, "{format:?}");
        assert_eq!((got.framing_errors, got.parity_errors), (0, 0));
        assert_eq!(receiver.reinserted_stops(), 1000);

        // A host no faster than the line has none left out: here each
        // character is given when the one before has gone.
        let mut transmitter = Transmitter::new(format);
        for &character in &characters[..200] {
            transmitter.write(&[character]);
            while !transmitter.idle() {
                transmitter.next_bit();
            }
        }
        assert_eq!(transmitter.deleted_stops(), 0);
    }
}

#[test]
fn the_transmitter_holds_so_many_characters_and_no_more() {
    let mut transmitter = Transmitter::new(Format::N81);
    transmitter.set_deletion(false);
    assert_eq!(transmitter.space(), TX_CAPACITY);
    let many = vec![0x55u8; TX_CAPACITY + 10];
    assert_eq!(transmitter.write(&many), TX_CAPACITY);
    assert_eq!(transmitter.write(&many), 0);
    assert_eq!(transmitter.space(), 0);
    // A character's worth of bits makes room for one.
    for _ in 0..10 {
        transmitter.next_bit();
    }
    assert_eq!(transmitter.space(), 1);
    transmitter.clear();
    assert!(transmitter.idle());
    assert!((0..40).all(|_| transmitter.next_bit()));
}

/// How many characters at the place where two sequences differ: of the
/// first, and of the second.
fn difference(sent: &[u8], got: &[u8]) -> (usize, usize) {
    let prefix = sent.iter().zip(got).take_while(|(a, b)| a == b).count();
    let suffix = sent[prefix..]
        .iter()
        .rev()
        .zip(got[prefix.min(got.len())..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (
        sent.len() - prefix - suffix,
        got.len().saturating_sub(prefix + suffix),
    )
}

#[test]
fn a_wrong_unit_costs_one_character_and_a_wrong_element_is_recovered_from() {
    // One bit of the stream wrong, at every place in turn, in 200
    // characters that follow one another without a gap: the hardest
    // case, since there is no idle line to find the framing by.
    for format in FORMATS {
        let mut rng = Rng(123);
        let characters: Vec<u8> = (0..200).map(|_| rng.next() as u8 & format.mask()).collect();
        let mut line = Vec::new();
        for burst in characters.chunks(50) {
            let mut transmitter = Transmitter::new(format);
            transmitter.set_deletion(false);
            line.extend(stream(&mut transmitter, burst, 0));
        }
        line.extend([true; 30]);
        assert_eq!(line.len(), 2030);

        // A wrong data or parity unit: that character is wrong and no
        // other, the framing is not disturbed, and with parity it is
        // seen.
        let mut costs = Vec::new();
        for at in 0..2000 {
            let mut damaged = line.clone();
            damaged[at] = !damaged[at];
            let mut receiver = Receiver::new(format);
            let got = receive(&mut receiver, &damaged);
            let (lost, wrong) = difference(&characters, &got.characters);
            let element = at % 10 == 0 || at % 10 == 9;
            if !element {
                // A wrong parity unit leaves the character itself right.
                let parity_unit = format.parity() != Parity::None && at % 10 == 8;
                let expected = if parity_unit { (0, 0) } else { (1, 1) };
                assert_eq!((lost, wrong), expected, "{format:?}, unit at bit {at}");
                assert_eq!(got.framing_errors, 0);
                assert_eq!(
                    got.parity_errors,
                    usize::from(format.parity() != Parity::None)
                );
            } else {
                costs.push(lost.max(wrong));
            }
        }
        // A wrong start or stop element moves the framing. Nothing marks
        // a character but a stop element with a start element after it,
        // and random data is full of those, so the receiver may take a
        // few characters to be right again, as any start-stop receiver
        // does: here at the middle of the cases, and at the worst.
        costs.sort_unstable();
        let (median, worst) = (costs[costs.len() / 2], costs[costs.len() - 1]);
        println!(
            "{format:?}: a wrong element costs {median} characters at the median, {worst} at the worst"
        );
        assert!(median <= 2, "{format:?}: median {median}");
        assert!(worst <= 40, "{format:?}: worst {worst}");
    }

    // With gaps between the characters, as a host types, the framing is
    // found at the next gap whatever was wrong.
    let characters: Vec<u8> = (0..40).map(|n| n as u8 ^ 0x5A).collect();
    let mut line = Vec::new();
    let mut transmitter = Transmitter::new(Format::N81);
    for &character in &characters {
        line.extend(stream(&mut transmitter, &[character], 12));
    }
    for at in 0..line.len() - 60 {
        let mut damaged = line.clone();
        damaged[at] = !damaged[at];
        let mut receiver = Receiver::new(Format::N81);
        let got = receive(&mut receiver, &damaged);
        let (lost, wrong) = difference(&characters, &got.characters);
        assert!(
            lost <= 1 && wrong <= 2,
            "bit {at}: {lost} lost, {wrong} wrong"
        );
    }
}

#[test]
fn a_break_is_2m_plus_3_bits_and_no_character_comes_of_it() {
    for format in FORMATS {
        // The transmitter: 23 bits of the start polarity and then 20 of
        // the stop polarity (clause 7.3.1 and its note), after the
        // character that was going out.
        let mut transmitter = Transmitter::new(format);
        transmitter.write(&[0x12, 0x34]);
        for _ in 0..5 {
            transmitter.next_bit();
        }
        transmitter.send_break();
        assert!(!transmitter.idle());
        transmitter.write(&[0x56]);
        let line: Vec<bool> = (0..120).map(|_| transmitter.next_bit()).collect();
        // The rest of the first character and its stop element.
        assert!(line[4]);
        let zeros = line[5..].iter().take_while(|&&bit| !bit).count();
        assert_eq!(zeros, 23, "{format:?}");
        let ones = line[28..].iter().take_while(|&&bit| bit).count();
        assert_eq!(ones, 20, "{format:?}");
        assert!(transmitter.idle());

        // The receiver: the characters either side, one break, and no
        // NUL before it (clause 7.3.2 and its note).
        let mut transmitter = Transmitter::new(format);
        transmitter.write(&[0x12]);
        let mut all = stream(&mut transmitter, &[], 0);
        transmitter.send_break();
        all.extend((0..43).map(|_| transmitter.next_bit()));
        all.extend(stream(&mut transmitter, &[0x34, 0x00, 0x56], 10));
        let mut receiver = Receiver::new(format);
        let got = receive(&mut receiver, &all);
        assert_eq!(got.characters, [0x12, 0x34, 0x00, 0x56], "{format:?}");
        assert_eq!((got.breaks, got.framing_errors), (1, 0));
        assert_eq!(receiver.breaks(), 1);

        // A long break is one break, and lasts until the stop polarity.
        let mut receiver = Receiver::new(format);
        let mut began = 0;
        for n in 0..5000 {
            began += usize::from(receiver.process(false).break_began);
            assert_eq!(receiver.in_break(), n >= 22);
        }
        assert_eq!(began, 1);
        assert!(receiver.process(true).characters().is_empty());
        assert!(!receiver.in_break());
        // And the framing is found again at once (the note to 7.3.1 gives
        // the far converter 2M bits; this one needs one).
        let line = stream(&mut Transmitter::new(format), &[0x41], 2);
        assert_eq!(receive(&mut receiver, &line).characters, [0x41]);
    }
}

#[test]
fn a_nul_with_its_stop_element_left_out_is_a_character_and_not_a_break() {
    // NUL, no stop element, and 'A' at once: ten bits of the start
    // polarity and then a 1. It is two characters.
    let line = bits("1  0 00000000  0 10000010 1  1");
    let mut receiver = Receiver::new(Format::N81);
    let got = receive(&mut receiver, &line);
    assert_eq!(got.characters, [0x00, 0x41]);
    assert_eq!((got.breaks, got.framing_errors), (0, 0));
    assert_eq!(receiver.reinserted_stops(), 1);

    // 22 bits of the start polarity are one short of a break: a NUL with
    // no stop element, and what follows it is not a character.
    let mut line = vec![true];
    line.extend([false; 22]);
    line.extend([true; 4]);
    let mut receiver = Receiver::new(Format::N81);
    let got = receive(&mut receiver, &line);
    assert_eq!(got.characters, [0x00]);
    assert_eq!((got.breaks, got.framing_errors), (0, 1));

    // 23 are a break, and nothing else.
    let mut line = vec![true];
    line.extend([false; 23]);
    line.extend([true; 4]);
    let mut receiver = Receiver::new(Format::N81);
    let got = receive(&mut receiver, &line);
    assert!(got.characters.is_empty());
    assert_eq!((got.breaks, got.framing_errors), (1, 0));
}

#[test]
fn a_wrong_parity_is_counted_and_the_character_given() {
    let mut transmitter = Transmitter::new(Format::E71);
    let line = stream(&mut transmitter, b"modem", 4);
    // Read as odd parity, every character is wrong and every one is
    // given.
    let mut receiver = Receiver::new(Format::O71);
    let got = receive(&mut receiver, &line);
    assert_eq!(got.characters, b"modem");
    assert_eq!(got.parity_errors, 5);
    assert_eq!(receiver.parity_errors(), 5);
    receiver.set_format(Format::E71);
    assert_eq!(receiver.format(), Format::E71);
    let got = receive(&mut receiver, &line);
    assert_eq!(
        (got.characters.as_slice(), got.parity_errors),
        (&b"modem"[..], 0)
    );
}

proptest! {
    #[test]
    fn any_characters_at_any_pace_go_round(
        bursts in prop::collection::vec((prop::collection::vec(any::<u8>(), 0..120), 0usize..40), 1..40),
        which in 0usize..3,
        deletion in any::<bool>(),
    ) {
        let format = FORMATS[which];
        let mut transmitter = Transmitter::new(format);
        transmitter.set_deletion(deletion);
        let mut receiver = Receiver::new(format);
        let mut sent = Vec::new();
        let mut got = Vec::new();
        for (burst, gap) in &bursts {
            sent.extend(burst.iter().map(|c| c & format.mask()));
            let line = stream(&mut transmitter, burst, *gap);
            for bit in line {
                let out = receiver.process(bit);
                prop_assert!(!out.framing_error && !out.parity_error && !out.break_began);
                got.extend_from_slice(out.characters());
            }
        }
        prop_assert_eq!(got, sent);
    }

    #[test]
    fn any_stream_of_bits_is_borne(
        stream in prop::collection::vec(any::<bool>(), 0..4000),
        which in 0usize..3,
    ) {
        // What the far end sends is the far end's to choose. Nothing it
        // sends makes more characters than there are bits for.
        let format = FORMATS[which];
        let mut receiver = Receiver::new(format);
        let mut characters = 0usize;
        for &bit in &stream {
            let out = receiver.process(bit);
            characters += out.characters().len();
            prop_assert!(out.characters().iter().all(|c| c & !format.mask() == 0));
        }
        prop_assert!(characters * 9 <= stream.len() + 9);
    }
}
