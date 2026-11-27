// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The framing of V.42 clause 8.1.1: the check sequence against the
//! clause's own words, round trips, every place a 0 is put in, and what
//! is made of frames that are damaged, cut short, too long or aborted.

use proptest::prelude::*;
use v42::hdlc::{DEFRAMER_CAPACITY, MAX_FRAME, MIN_FRAME, fcs};
use v42::{Deframed, Deframer, Fill, Framer, Invalid};

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

    fn octets(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next() >> 24) as u8).collect()
    }
}

/// The bits a framer sends for some frames given one after another, each
/// as soon as it is taken, and `tail` bits after the last has gone.
fn line(framer: &mut Framer, frames: &[Vec<u8>], tail: usize) -> Vec<bool> {
    let mut out = Vec::new();
    let mut next = 0;
    loop {
        if next < frames.len() && framer.ready() {
            assert!(framer.send(&frames[next]));
            next += 1;
        }
        if next == frames.len() && framer.idle() {
            break;
        }
        out.push(framer.next_bit());
    }
    out.extend((0..tail).map(|_| framer.next_bit()));
    out
}

/// What a deframer makes of a line: the good frames, and everything else
/// it said that was not nothing or a flag.
fn deframe(deframer: &mut Deframer, line: &[bool]) -> (Vec<Vec<u8>>, Vec<Deframed>) {
    let mut frames = Vec::new();
    let mut others = Vec::new();
    for &bit in line {
        match deframer.process(bit) {
            Deframed::Nothing | Deframed::Flag => {}
            Deframed::Frame(len) => {
                assert_eq!(deframer.frame().len(), len);
                frames.push(deframer.frame().to_vec());
            }
            other => others.push(other),
        }
    }
    (frames, others)
}

/// The check sequence by the words of clause 8.1.1.6.1, a bit at a time
/// and with the polynomials as polynomials: the ones complement of the
/// sum of the remainder of x^k (x^15 + ... + 1) and the remainder of x^16
/// times the content, both by x^16 + x^12 + x^5 + 1.
fn fcs_by_the_clause(octets: &[u8]) -> u16 {
    const GENERATOR: u32 = 0x1_1021;
    // Remainder of a polynomial given as bits, highest power first.
    let remainder = |bits: &[bool]| -> u16 {
        let mut register = 0u32;
        for &bit in bits {
            register = (register << 1) | u32::from(bit);
            if register & 0x1_0000 != 0 {
                register ^= GENERATOR;
            }
        }
        register as u16
    };
    // The content as a polynomial: the first bit transmitted is the
    // coefficient of the highest power, and bit 1 of each octet is
    // transmitted first.
    let content: Vec<bool> = octets
        .iter()
        .flat_map(|octet| (0..8).map(move |bit| (octet >> bit) & 1 == 1))
        .collect();
    let k = content.len();
    // a) x^k (x^15 + ... + 1): sixteen ones and k zeros.
    let mut a = vec![true; 16];
    a.extend(std::iter::repeat_n(false, k));
    // b) x^16 times the content.
    let mut b = content;
    b.extend([false; 16]);
    let sum = !(remainder(&a) ^ remainder(&b));
    // The sequence is transmitted from its highest power, and the octets
    // of a frame from their bit 1: as two octets of the frame, the first
    // has x^15 in its bit 1 (clause 8.1.2.3 and Figure 7).
    sum.reverse_bits()
}

#[test]
fn the_check_sequence_is_that_of_clause_8_1_1_6_1() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for len in 0..300 {
        let octets = rng.octets(len);
        assert_eq!(fcs(&octets), fcs_by_the_clause(&octets), "{len} octets");
    }
    // The value every catalogue of check sequences has for this one, of
    // the nine characters "123456789".
    assert_eq!(fcs(b"123456789"), 0x906E);
    // A frame with its check sequence leaves the remainder the clause
    // names, 0001 1101 0000 1111 from x^15 to x^0: so the check of a
    // frame and its own check sequence is always the same number.
    for len in 1..50 {
        let mut octets = rng.octets(len);
        let check = fcs(&octets);
        octets.push(check as u8);
        octets.push((check >> 8) as u8);
        assert_eq!(!fcs(&octets), 0x1D0Fu16.reverse_bits());
    }
}

#[test]
fn a_frame_is_flags_its_octets_and_its_check_sequence() {
    // The shortest frame there is, by hand: an address and a control
    // octet. Bit 1 of each octet goes first (clause 8.1.2.2), and nothing
    // in these has five 1s in a row.
    let frame = [0x03u8, 0x13];
    let check = fcs(&frame);
    let mut framer = Framer::new();
    assert!(framer.send(&frame));
    let sent: Vec<bool> = (0..48).map(|_| framer.next_bit()).collect();
    let flag = [false, true, true, true, true, true, true, false];
    assert_eq!(sent[..8], flag);
    let mut expected = Vec::new();
    for octet in [0x03, 0x13, check as u8, (check >> 8) as u8] {
        expected.extend((0..8).map(|bit| (octet >> bit) & 1 == 1));
    }
    // The check sequence of this frame has no five 1s in a row either,
    // which the next line would show if it had.
    assert_eq!(sent[8..40], expected[..]);
    assert_eq!(sent[40..48], flag);
    assert_eq!(framer.frames_sent(), 1);
    assert_eq!(framer.flags_sent(), 2);

    // Between frames the line carries flags (clause 8.1.5), or 1s when
    // it is told to.
    let fill: Vec<bool> = (0..32).map(|_| framer.next_bit()).collect();
    for flag_bits in fill.chunks(8) {
        assert_eq!(flag_bits, flag);
    }
    framer.set_fill(Fill::Marks);
    assert!((0..40).all(|_| framer.next_bit()));
    // And a frame after 1s has its opening flag.
    assert!(framer.send(&frame));
    let sent: Vec<bool> = (0..8).map(|_| framer.next_bit()).collect();
    assert_eq!(sent, flag);
}

#[test]
fn frames_of_every_length_go_round() {
    let mut rng = Rng(0xDEAD_BEEF_0BAD_F00D);
    let frames: Vec<Vec<u8>> = (MIN_FRAME - 2..=MAX_FRAME)
        .map(|len| rng.octets(len))
        .collect();
    for fill in [Fill::Flags, Fill::Marks] {
        let mut framer = Framer::new();
        framer.set_fill(fill);
        // Each frame is given as the last one's closing flag goes out,
        // so that one flag is between them (clause 8.1.1.2).
        let bits = line(&mut framer, &frames, 20);
        let mut deframer = Deframer::new();
        let (got, others) = deframe(&mut deframer, &bits);
        assert_eq!(got, frames);
        assert!(others.is_empty(), "{others:?}");
        assert_eq!(deframer.frames(), frames.len() as u64);
        assert_eq!(deframer.invalid_frames(), 0);
        // One flag between two frames, and one more at each end.
        assert_eq!(framer.frames_sent(), frames.len() as u64);
        let flags = framer.flags_sent();
        match fill {
            Fill::Flags => assert!(flags > frames.len() as u64),
            Fill::Marks => assert_eq!(flags, frames.len() as u64 + 1),
        }
    }
    // With time between the frames as well.
    let mut framer = Framer::new();
    let mut deframer = Deframer::new();
    for frame in &frames {
        let gap = rng.below(40) as usize;
        let bits = line(&mut framer, std::slice::from_ref(frame), gap);
        let (got, others) = deframe(&mut deframer, &bits);
        assert_eq!(got, std::slice::from_ref(frame));
        assert!(others.is_empty());
    }
}

/// The octets of a line's bits with the flags taken off and the 0s that
/// were put in taken out, by the rule alone: for checking the framer
/// without the deframer.
fn content_of(bits: &[bool]) -> Vec<bool> {
    assert_eq!(
        bits[..8],
        [false, true, true, true, true, true, true, false]
    );
    let mut out = Vec::new();
    let mut ones = 0;
    for &bit in &bits[8..] {
        if ones == 5 {
            if bit {
                // The sixth 1: the closing flag. Its 0 and five 1s are
                // the last six bits taken.
                out.truncate(out.len() - 6);
                return out;
            }
            ones = 0;
            continue;
        }
        ones = if bit { ones + 1 } else { 0 };
        out.push(bit);
    }
    panic!("no closing flag");
}

#[test]
fn a_zero_follows_every_five_ones_wherever_they_fall() {
    // Runs of 1s of every length from 1 to 40, starting at every bit of
    // an octet, so that the fifth 1 falls on every place there is: in an
    // octet, at its end, across two, at the end of the frame and in the
    // check sequence.
    let mut cases: Vec<Vec<u8>> = Vec::new();
    for start in 0..8 {
        for run in 1..=40 {
            let mut bits = vec![false; start];
            bits.extend(std::iter::repeat_n(true, run));
            while bits.len() % 8 != 0 || bits.len() < 16 {
                bits.push(false);
            }
            let octets: Vec<u8> = bits
                .chunks(8)
                .map(|octet| {
                    octet
                        .iter()
                        .enumerate()
                        .fold(0, |acc, (n, &bit)| acc | (u8::from(bit) << n))
                })
                .collect();
            cases.push(octets);
        }
    }
    // The octets that look most like a flag, and frames of nothing but
    // 1s, which have the most 0s put in.
    for octet in [0x7E, 0x7D, 0x7F, 0xFE, 0xFC, 0x3F, 0x1F, 0xF8, 0xFF] {
        for len in [2, 3, 5, 8, MAX_FRAME] {
            cases.push(vec![octet; len]);
        }
    }
    for frame in &cases {
        let mut framer = Framer::new();
        let bits = line(&mut framer, std::slice::from_ref(frame), 8);
        // Between the flags there are never six 1s in a row.
        let inside = &bits[8..bits.len() - 16];
        let longest = inside.split(|&bit| !bit).map(<[bool]>::len).max();
        assert!(longest.unwrap_or(0) <= 5, "{frame:02X?}");
        // What is between them, with the 0s taken out by the rule, is the
        // frame and its check sequence, bit 1 of each octet first.
        let mut expected: Vec<bool> = Vec::new();
        let check = fcs(frame);
        for octet in frame
            .iter()
            .copied()
            .chain([check as u8, (check >> 8) as u8])
        {
            expected.extend((0..8).map(|bit| (octet >> bit) & 1 == 1));
        }
        assert_eq!(content_of(&bits), expected, "{frame:02X?}");
        // And the deframer has the frame.
        let mut deframer = Deframer::new();
        let (got, others) = deframe(&mut deframer, &bits);
        assert_eq!(got, std::slice::from_ref(frame), "{frame:02X?}");
        assert!(others.is_empty(), "{frame:02X?}: {others:?}");
    }
}

#[test]
fn a_damaged_frame_is_not_a_frame() {
    // One bit of the line wrong, at every place of a few frames in turn.
    // Whatever becomes of it, a flag made or unmade, an abort, a frame cut
    // in two, nothing comes out as a good frame but the frames that were
    // not touched.
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let frames: Vec<Vec<u8>> = [2usize, 5, 17, 40]
        .iter()
        .map(|&len| rng.octets(len))
        .collect();
    let mut framer = Framer::new();
    let bits = line(&mut framer, &frames, 16);
    let mut kinds = std::collections::BTreeMap::new();
    for at in 0..bits.len() {
        let mut damaged = bits.clone();
        damaged[at] = !damaged[at];
        let mut deframer = Deframer::new();
        let (got, others) = deframe(&mut deframer, &damaged);
        for frame in &got {
            assert!(
                frames.contains(frame),
                "bit {at}: a frame that was not sent"
            );
        }
        // One of the four is lost, the one the bit was in; or two, if
        // it was in the one flag between two of them, which then run
        // together; or none, if it was in the fill.
        assert!(got.len() >= 2, "bit {at}: {} frames", got.len());
        for other in others {
            *kinds.entry(format!("{other:?}")).or_insert(0u32) += 1;
        }
    }
    println!("{kinds:#?}");
    // Each kind of invalid frame was seen.
    for kind in [
        "Invalid(CheckSequence)",
        "Invalid(NotOctets)",
        "Invalid(TooShort)",
        "Abort",
    ] {
        assert!(kinds.contains_key(kind), "no {kind}");
    }
}

#[test]
fn what_is_cut_short_too_short_too_long_or_aborted_is_said_to_be() {
    let flag = [false, true, true, true, true, true, true, false];
    let octet = |value: u8| (0..8).map(move |bit| (value >> bit) & 1 == 1);

    // Clause 8.1.3 b): fewer than four octets between flags.
    for len in 1..MIN_FRAME {
        let mut bits: Vec<bool> = flag.to_vec();
        for _ in 0..len {
            bits.extend(octet(0x55));
        }
        bits.extend(flag);
        let mut deframer = Deframer::new();
        let (got, others) = deframe(&mut deframer, &bits);
        assert!(got.is_empty());
        assert_eq!(
            others,
            [Deframed::Invalid(Invalid::TooShort)],
            "{len} octets"
        );
    }

    // Clause 8.1.3 c): not a whole number of octets.
    for extra in 1..8 {
        let mut bits: Vec<bool> = flag.to_vec();
        for _ in 0..5 {
            bits.extend(octet(0x55));
        }
        bits.extend(std::iter::repeat_n(false, extra));
        bits.extend(flag);
        let mut deframer = Deframer::new();
        let (_, others) = deframe(&mut deframer, &bits);
        assert_eq!(
            others,
            [Deframed::Invalid(Invalid::NotOctets)],
            "{extra} bits over"
        );
    }

    // A frame cut short by a flag: its check sequence is wrong.
    let mut framer = Framer::new();
    let bits = line(&mut framer, &[vec![0xA5; 30]], 0);
    let mut cut = bits[..150].to_vec();
    cut.extend(flag);
    let mut deframer = Deframer::new();
    let (got, others) = deframe(&mut deframer, &cut);
    assert!(got.is_empty());
    assert_eq!(others.len(), 1);
    assert!(matches!(others[0], Deframed::Invalid(_)));

    // Clause 8.1.4: an aborted frame. The framer sends seven 1s, the
    // deframer says so, and the frame after it is good.
    let mut framer = Framer::new();
    assert!(framer.send(&[0x11; 40]));
    let mut bits: Vec<bool> = (0..100).map(|_| framer.next_bit()).collect();
    assert!(framer.abort());
    assert!(!framer.abort());
    bits.extend(line(&mut framer, &[vec![0x22; 6]], 8));
    let mut deframer = Deframer::new();
    let (got, others) = deframe(&mut deframer, &bits);
    assert_eq!(got, [vec![0x22; 6]]);
    assert_eq!(others, [Deframed::Abort]);
    assert_eq!(deframer.aborts(), 1);

    // A line that goes idle after a flag has lost no frame, and one that
    // is idle for ever says nothing.
    let mut deframer = Deframer::new();
    let mut bits = flag.to_vec();
    bits.extend([true; 100]);
    let (got, others) = deframe(&mut deframer, &bits);
    assert!(got.is_empty() && others.is_empty());

    // A frame longer than a frame may be, with a good check sequence, is
    // given as it is: the protocol is to tell it from noise (the note to
    // clause 8.5.5).
    let long: Vec<u8> = (0..MAX_FRAME as u32 + 40).map(|n| n as u8).collect();
    let mut bits = flag.to_vec();
    let check = fcs(&long);
    let mut ones = 0;
    for value in long
        .iter()
        .copied()
        .chain([check as u8, (check >> 8) as u8])
    {
        for bit in octet(value) {
            bits.push(bit);
            ones = if bit { ones + 1 } else { 0 };
            if ones == 5 {
                bits.push(false);
                ones = 0;
            }
        }
    }
    bits.extend(flag);
    let mut deframer = Deframer::new();
    let (got, others) = deframe(&mut deframer, &bits);
    assert_eq!(got, [long]);
    assert!(others.is_empty());

    // More octets than twice the longest frame and two, with no flag,
    // is unbounded: nothing is written past the buffer, and it is said
    // once, at the flag that ends it.
    let mut bits = flag.to_vec();
    for n in 0..DEFRAMER_CAPACITY + 500 {
        bits.extend(octet(if n % 2 == 0 { 0x55 } else { 0x33 }));
    }
    bits.extend(flag);
    bits.extend(line(&mut Framer::new(), &[vec![0x44; 4]], 8));
    let mut deframer = Deframer::new();
    let (got, others) = deframe(&mut deframer, &bits);
    assert_eq!(got, [vec![0x44; 4]]);
    assert_eq!(others, [Deframed::Invalid(Invalid::Unbounded)]);
}

#[test]
fn flags_in_a_row_are_counted() {
    let mut framer = Framer::new();
    let mut deframer = Deframer::new();
    for n in 1..=20 {
        for _ in 0..8 {
            deframer.process(framer.next_bit());
        }
        assert_eq!(deframer.flags_in_a_row(), n);
    }
    // A frame is not a flag, and after it the count begins again with
    // its closing flag.
    let bits = line(&mut framer, &[vec![1, 2, 3]], 0);
    let (got, _) = deframe(&mut deframer, &bits);
    assert_eq!(got.len(), 1);
    assert_eq!(deframer.flags_in_a_row(), 1);
    // Flags that share their 0s are flags too.
    let mut deframer = Deframer::new();
    let shared = [false, true, true, true, true, true, true];
    for _ in 0..5 {
        for bit in shared {
            deframer.process(bit);
        }
    }
    deframer.process(false);
    assert_eq!(deframer.flags_in_a_row(), 5);
    // Seven 1s end the run.
    for _ in 0..7 {
        deframer.process(true);
    }
    assert_eq!(deframer.flags_in_a_row(), 0);
    deframer.reset();
    assert_eq!(deframer.flags_in_a_row(), 0);
}

#[test]
fn the_framer_takes_one_frame_at_a_time_and_none_too_long() {
    let mut framer = Framer::new();
    assert!(!framer.send(&[]));
    assert!(!framer.send(&[0; MAX_FRAME + 1]));
    assert!(framer.ready() && framer.idle());
    assert!(framer.send(&[0; MAX_FRAME]));
    assert!(!framer.ready() && !framer.idle());
    assert!(!framer.send(&[1, 2]));
    // Not while the frame is going out either.
    for _ in 0..20 {
        framer.next_bit();
    }
    assert!(!framer.ready());
    assert!(!framer.send(&[1, 2]));
    while !framer.ready() {
        framer.next_bit();
    }
    // When its closing flag has begun, the next is taken.
    assert!(framer.send(&[1, 2]));
    assert!(!framer.idle());
}

proptest! {
    #[test]
    fn any_frames_with_any_gaps_go_round(
        frames in prop::collection::vec(
            (prop::collection::vec(any::<u8>(), 2..=MAX_FRAME), 0usize..30),
            1..12,
        ),
        marks in any::<bool>(),
    ) {
        let mut framer = Framer::new();
        framer.set_fill(if marks { Fill::Marks } else { Fill::Flags });
        let mut deframer = Deframer::new();
        for (frame, gap) in &frames {
            let bits = line(&mut framer, std::slice::from_ref(frame), *gap);
            let (got, others) = deframe(&mut deframer, &bits);
            prop_assert_eq!(got, vec![frame.clone()]);
            prop_assert!(others.is_empty());
        }
    }

    #[test]
    fn any_bits_at_all_are_borne(bits in prop::collection::vec(any::<bool>(), 0..6000)) {
        // Whatever the line carries, a frame that comes out fits the
        // buffer, and there are no more frames than there are octets.
        let mut deframer = Deframer::new();
        for &bit in &bits {
            if let Deframed::Frame(len) = deframer.process(bit) {
                prop_assert!(len + 2 >= MIN_FRAME && len + 2 <= DEFRAMER_CAPACITY);
                prop_assert_eq!(deframer.frame().len(), len);
            }
        }
        prop_assert!(deframer.frames() + deframer.invalid_frames() <= bits.len() as u64 / 8 + 1);
    }
}
