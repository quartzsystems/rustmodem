// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! LAPM (V.42 clause 8): two machines joined by a line that loses,
//! repeats and delays frames, on a clock the test moves.
//!
//! The edition of 2002 has no state tables. The states here are those of
//! its clauses 8.3, 8.7 and 8.8, disconnected, awaiting establishment,
//! connected and awaiting release, with the conditions of the connected
//! state: timer recovery (8.5.3), peer-receiver busy (8.4.6), own-receiver
//! busy (8.4.7) and reject (8.5.1). Each test names the clause it holds
//! the machine to.

mod common;

use common::{Faults, Pair, RATE, Rng};
use proptest::prelude::*;
use v42::Role;
use v42::hdlc::MAX_FRAME;
use v42::lapm::{
    CompressionWanted, Config, Event, HOST_QUEUE, Lapm, MAX_N401, MAX_WINDOW, Negotiated, Release,
    State,
};
use v42::xid::{Compression, Parameters};

fn config() -> Config {
    Config::for_rate(RATE)
}

/// The frames a machine has to send now, each as its octets.
fn drain(lapm: &mut Lapm) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut frame = [0u8; MAX_FRAME];
    loop {
        let len = lapm.poll_transmit(&mut frame);
        if len == 0 {
            return frames;
        }
        frames.push(frame[..len].to_vec());
    }
}

fn events(lapm: &mut Lapm) -> Vec<Event> {
    std::iter::from_fn(|| lapm.poll_event()).collect()
}

/// An answering machine with a connection, made by hand: SABME in, UA
/// out.
fn connected_answerer() -> Lapm {
    let mut lapm = Lapm::new(Role::Answerer, config());
    lapm.receive(&[0x03, 0x7F]);
    assert_eq!(drain(&mut lapm), [vec![0x03, 0x73]]);
    assert!(matches!(events(&mut lapm)[..], [Event::Established(_)]));
    lapm
}

// The frames, octet by octet.

#[test]
fn the_frames_of_a_connection_are_those_of_tables_6_and_8() {
    // No negotiation: SABME, UA, an I frame each way, RR, DISC, UA.
    let no_xid = Config {
        negotiate: false,
        ..config()
    };
    let mut pair = Pair::with(1, no_xid, no_xid);
    pair.connect();
    pair.a.send(b"AT");
    pair.run(200);
    pair.b.send(b"OK");
    pair.run(200);
    pair.a.lapm.release();
    pair.run(200);
    assert_eq!(pair.b.received, b"AT");
    assert_eq!(pair.a.received, b"OK");
    assert_eq!(pair.a.released(), Some(Release::Local));
    assert_eq!(pair.b.released(), Some(Release::Remote));
    // Table 6: a command to the answerer and a response from it have the
    // C/R bit set, 0x03 with DLCI 0 and EA; the others have it clear,
    // 0x01. Table 8: SABME is 011P1111, UA 011F0011, DISC 010P0011; an I
    // frame is N(S) and 0, then N(R) and P; RR is 00000001, then N(R) and
    // the P/F bit. SABME and DISC go with P set (clauses 8.3.2.1 and
    // 8.7.2, the notes).
    assert_eq!(
        pair.a_to_b.log,
        [
            vec![0x03, 0x7F],                   // SABME, P
            vec![0x03, 0x00, 0x00, b'A', b'T'], // I, N(S) 0, N(R) 0
            vec![0x01, 0x01, 0x02],             // RR response, N(R) 1
            vec![0x03, 0x53],                   // DISC, P
        ]
    );
    assert_eq!(
        pair.b_to_a.log,
        [
            vec![0x03, 0x73],                   // UA, F
            vec![0x03, 0x01, 0x02],             // RR response, N(R) 1
            vec![0x01, 0x00, 0x02, b'O', b'K'], // I, N(S) 0, N(R) 1
            vec![0x03, 0x73],                   // UA, F
        ]
    );
}

#[test]
fn the_xid_frame_is_that_of_clause_12_2() {
    let wanted = CompressionWanted {
        transmit: true,
        receive: true,
        codewords: 2048,
        string_length: 32,
    };
    let mut lapm = Lapm::new(
        Role::Originator,
        Config {
            compression: Some(wanted),
            ..config()
        },
    );
    lapm.establish();
    assert_eq!(lapm.state(), State::Negotiating);
    let frames = drain(&mut lapm);
    #[rustfmt::skip]
    let expected: Vec<u8> = vec![
        0x03, 0xAF,             // a command to the answerer; XID, P/F 0 (clause 8.2.4.13)
        0x82,                   // the format identifier (clause 12.2.2)
        0x80, 0x00, 0x13,       // parameter negotiation, 19 octets
        0x03, 0x03, 0x8A, 0x89, 0x00, // HDLC optional functions: bits 2, 4, 8, 9, 12, 16
        0x05, 0x02, 0x04, 0x00, // N401 to the answerer: 1024 bits
        0x06, 0x02, 0x04, 0x00, // N401 from it
        0x07, 0x01, 0x0F,       // k to the answerer: 15
        0x08, 0x01, 0x0F,       // k from it
        0xF0, 0x00, 0x0F,       // private parameter negotiation, 15 octets
        0x00, 0x03, b'V', b'4', b'2', // the parameter set identification
        0x01, 0x01, 0x03,       // P0: both directions
        0x02, 0x02, 0x08, 0x00, // P1: 2048 codewords
        0x03, 0x01, 0x20,       // P2: strings of 32
    ];
    assert_eq!(frames, std::slice::from_ref(&expected));
    // What is read back is what was written.
    let read = Parameters::decode(&expected[2..]).unwrap();
    assert_eq!(
        read,
        Parameters {
            compression: Some(Compression {
                initiator_to_responder: true,
                responder_to_initiator: true,
                codewords: 2048,
                string_length: 32,
            }),
            ..Parameters::default()
        }
    );
    // Without compression there is no private group, and absent
    // parameters are the defaults (clauses 9.2.3 and 9.2.4).
    let mut plain = [0u8; 64];
    let len = Parameters::default().encode(&mut plain);
    assert_eq!(len, 23);
    assert_eq!(
        Parameters::decode(&plain[..len]),
        Some(Parameters::default())
    );
    assert_eq!(Parameters::decode(&[0x82]), Some(Parameters::default()));
    // Another format is not this Recommendation's.
    assert_eq!(Parameters::decode(&[0x81, 0x80, 0x00, 0x00]), None);
    assert_eq!(Parameters::decode(&[]), None);
    // The optional functions may be three octets or four (Corrigendum 1),
    // what is not known is passed over, and a group or a parameter that
    // says it is longer than the field ends the reading and loses
    // nothing that was whole.
    #[rustfmt::skip]
    let odd: Vec<u8> = vec![
        0x82,
        0x80, 0x00, 0x0C,
        0x03, 0x04, 0x8A, 0x89, 0x00, 0x00,
        0x07, 0x01, 0x08,
        0x63, 0x01, 0x00,       // a parameter there is none of
        0xF0, 0x00, 0x40,       // 64 octets, of which 8 are there
        0x00, 0x03, 0x2A, 0x34, 0x32, // the set identification as V.42 prints it
        0x01, 0x01, 0x01,
        0x02, 0x09,             // cut off
    ];
    let read = Parameters::decode(&odd).unwrap();
    assert_eq!(read.k_initiator, 8);
    assert_eq!(read.k_responder, 15);
    assert_eq!(
        read.compression,
        Some(Compression {
            initiator_to_responder: true,
            responder_to_initiator: false,
            codewords: 512,
            string_length: 6,
        })
    );
}

// Negotiation.

#[test]
fn every_offer_of_compression_gets_the_answer_of_v42bis_clause_5_1() {
    let want = |transmit, receive, codewords, string_length| {
        (transmit || receive).then_some(CompressionWanted {
            transmit,
            receive,
            codewords,
            string_length,
        })
    };
    let directions = [(false, false), (true, false), (false, true), (true, true)];
    for &(a_tx, a_rx) in &directions {
        for &(b_tx, b_rx) in &directions {
            for (a_size, b_size) in [
                ((2048, 32), (2048, 32)),
                ((512, 6), (4096, 250)),
                ((4096, 250), (1024, 20)),
            ] {
                let a = Config {
                    compression: want(a_tx, a_rx, a_size.0, a_size.1),
                    ..config()
                };
                let b = Config {
                    compression: want(b_tx, b_rx, b_size.0, b_size.1),
                    ..config()
                };
                let mut pair = Pair::with(3, a, b);
                pair.connect();
                let (at_a, at_b) = (pair.a.lapm.negotiated(), pair.b.lapm.negotiated());
                let what = format!("a {a_tx} {a_rx} {a_size:?}, b {b_tx} {b_rx} {b_size:?}");
                // A direction is compressed only if both ends want it:
                // the originator to send and the answerer to receive, or
                // the other way.
                assert_eq!(at_a.compress_transmit, a_tx && b_rx, "{what}");
                assert_eq!(at_a.compress_receive, a_rx && b_tx, "{what}");
                // The two ends have the same of it.
                assert_eq!(at_b.compress_transmit, at_a.compress_receive, "{what}");
                assert_eq!(at_b.compress_receive, at_a.compress_transmit, "{what}");
                if at_a.compress_transmit || at_a.compress_receive {
                    // The lower of each (V.42bis clause 5.1).
                    assert_eq!(at_a.codewords, a_size.0.min(b_size.0), "{what}");
                    assert_eq!(at_a.string_length, a_size.1.min(b_size.1), "{what}");
                    assert_eq!(
                        (at_b.codewords, at_b.string_length),
                        (at_a.codewords, at_a.string_length)
                    );
                }
                assert_eq!(at_a.n401_transmit, 128);
                assert_eq!(at_a.window_transmit, 15);
                // And data passes.
                pair.a.send(b"hello");
                pair.b.send(b"world");
                assert!(pair.run_until(2000, Pair::delivered), "{what}");
            }
        }
    }
}

#[test]
fn an_answerer_keeps_n401_and_k_between_what_is_asked_and_the_default() {
    // Clauses 9.2.3 and 9.2.4, with an XID command made by hand for each
    // case: (N401 asked in octets, k asked) and what the answer has.
    for ((n401, k), (n401_answer, k_answer)) in [
        ((128u16, 15u8), (128u16, 15u8)),
        ((64, 8), (64, 8)),
        ((256, 32), (128, 15)),
        ((2048, 127), (128, 15)),
        ((16, 1), (16, 1)),
    ] {
        let asked = Parameters {
            n401_initiator: n401,
            n401_responder: n401,
            k_initiator: k,
            k_responder: k,
            compression: None,
        };
        let mut frame = vec![0x03, 0xAF];
        let mut field = [0u8; 64];
        let len = asked.encode(&mut field);
        frame.extend_from_slice(&field[..len]);
        let mut lapm = Lapm::new(Role::Answerer, config());
        lapm.receive(&frame);
        let answer = drain(&mut lapm);
        assert_eq!(answer.len(), 1);
        // An XID response from the answerer: its own address.
        assert_eq!(answer[0][..2], [0x03, 0xAF]);
        let read = Parameters::decode(&answer[0][2..]).unwrap();
        assert_eq!(
            (read.n401_initiator, read.n401_responder),
            (n401_answer, n401_answer)
        );
        assert_eq!((read.k_initiator, read.k_responder), (k_answer, k_answer));
        // And it holds itself to them.
        lapm.receive(&[0x03, 0x7F]);
        let negotiated = lapm.negotiated();
        assert_eq!(negotiated.n401_transmit, n401_answer);
        assert_eq!(negotiated.window_transmit, k_answer);
        assert_eq!(drain(&mut lapm), [vec![0x03, 0x73]]);
        lapm.write(&[0x55; 600]);
        let frames = drain(&mut lapm);
        assert_eq!(
            frames.len(),
            usize::from(k_answer).min(600usize.div_ceil(usize::from(n401_answer)))
        );
        assert!(
            frames
                .iter()
                .all(|f| f.len() <= 3 + usize::from(n401_answer))
        );
    }
    // An originator that is answered with more than it asked for takes
    // no more.
    let mut lapm = Lapm::new(Role::Originator, config());
    lapm.establish();
    drain(&mut lapm);
    let greedy = Parameters {
        n401_initiator: 4096,
        n401_responder: 4096,
        k_initiator: 127,
        k_responder: 127,
        compression: Some(Compression {
            initiator_to_responder: true,
            responder_to_initiator: true,
            codewords: 4096,
            string_length: 250,
        }),
    };
    let mut frame = vec![0x03, 0xAF];
    let mut field = [0u8; 64];
    let len = greedy.encode(&mut field);
    frame.extend_from_slice(&field[..len]);
    lapm.receive(&frame);
    assert_eq!(lapm.state(), State::Establishing);
    assert_eq!(lapm.negotiated(), Negotiated::default());
}

// Set-up and release.

#[test]
fn set_up_is_tried_n400_times_and_then_given_up() {
    // Clauses 8.10.3 and 8.3.2.2: the XID and then the SABME, each again
    // on every expiry of T401, N400 times.
    let mut pair = Pair::new(5);
    pair.a_to_b.cut = true;
    pair.a.lapm.establish();
    let t401 = u64::from(config().t401) / 8;
    let n400 = u64::from(config().n400);
    pair.run(t401 * (n400 + 1) + 500);
    assert_eq!(pair.a.released(), Some(Release::NoResponse));
    assert_eq!(pair.a.lapm.state(), State::Disconnected);
    assert_eq!(pair.a_to_b.log.len() as u64, n400 + 1);
    assert!(pair.a_to_b.log.iter().all(|f| f[..2] == [0x03, 0xAF]));

    // The XID answered and the SABME not.
    let mut pair = Pair::new(5);
    pair.a.lapm.establish();
    assert!(pair.run_until(1000, |pair| pair.a.lapm.state() == State::Establishing));
    pair.b_to_a.cut = true;
    pair.run(t401 * (n400 + 1) + 500);
    assert_eq!(pair.a.released(), Some(Release::NoResponse));
    let sabmes = pair
        .a_to_b
        .log
        .iter()
        .filter(|f| f[..] == [0x03, 0x7F])
        .count();
    assert_eq!(sabmes as u64, n400 + 1);
    // The answerer had every one of them, answered each with UA, and is
    // connected to nobody: its inactivity timer finds that out (clause
    // 8.12).
    assert_eq!(pair.b.lapm.state(), State::Connected);
    pair.a_to_b.cut = true;
    pair.run(u64::from(config().t403) / 8 + t401 * (n400 + 1) + 500);
    assert_eq!(pair.b.released(), Some(Release::TimedOut));
}

#[test]
fn a_lost_ua_is_made_up_for() {
    // Clause 8.4.9.1: the originator may send SABME again if the UA is
    // lost, and the answerer, connected, answers again.
    let mut pair = Pair::new(7);
    pair.a.lapm.establish();
    assert!(pair.run_until(1000, |pair| pair.a.lapm.state() == State::Establishing));
    pair.b_to_a.cut = true;
    pair.run(400);
    assert_eq!(pair.b.lapm.state(), State::Connected);
    assert_eq!(pair.a.lapm.state(), State::Establishing);
    pair.b_to_a.cut = false;
    assert!(pair.run_until(5000, |pair| pair.a.established()));
    assert_eq!(pair.b.released(), None);
    assert_eq!(pair.b.events.len(), 1);

    // Clause 8.3.2.1: an I frame or a supervisory frame while SABME is
    // out is as good as the UA.
    for frame in [vec![0x01, 0x00, 0x00, 0x41], vec![0x03, 0x01, 0x00]] {
        let mut lapm = Lapm::new(
            Role::Originator,
            Config {
                negotiate: false,
                ..config()
            },
        );
        lapm.establish();
        assert_eq!(drain(&mut lapm), [vec![0x03, 0x7F]]);
        lapm.receive(&frame);
        assert_eq!(lapm.state(), State::Connected);
    }

    // But where numbered frames have passed, an SABME ends the connection
    // (clause 8.4.9.1): the edition of 2002 has no reset.
    let mut pair = Pair::new(7);
    pair.connect();
    pair.a.send(b"x");
    pair.run(300);
    pair.b.lapm.receive(&[0x03, 0x7F]);
    pair.run(10);
    assert_eq!(pair.b.released(), Some(Release::Protocol));
}

#[test]
fn a_refusal_and_the_collisions_of_clause_8_9() {
    // DM with F set to an SABME: refused (clause 8.3.2.1). With F clear
    // it is ignored (clause 8.9.3).
    let no_xid = Config {
        negotiate: false,
        ..config()
    };
    let mut lapm = Lapm::new(Role::Originator, no_xid);
    lapm.establish();
    drain(&mut lapm);
    lapm.receive(&[0x03, 0x0F]);
    assert_eq!(lapm.state(), State::Establishing);
    lapm.receive(&[0x03, 0x1F]);
    assert_eq!(events(&mut lapm), [Event::Released(Release::Refused)]);

    // Clause 8.8: DISC where there is no connection is answered DM.
    let mut lapm = Lapm::new(Role::Answerer, config());
    lapm.receive(&[0x03, 0x53]);
    assert_eq!(drain(&mut lapm), [vec![0x03, 0x1F]]);
    assert!(events(&mut lapm).is_empty());

    // Clause 8.9.1: both release at once. Each answers the other's DISC
    // with UA, and each is disconnected on the UA it has.
    let mut pair = Pair::new(9);
    pair.connect();
    pair.a.lapm.release();
    pair.b.lapm.release();
    pair.run(500);
    assert_eq!(pair.a.released(), Some(Release::Local));
    assert_eq!(pair.b.released(), Some(Release::Local));

    // Clause 8.7.3: a DISC that is not answered is sent N400 times more,
    // and then the connection is released all the same.
    let mut pair = Pair::new(9);
    pair.connect();
    pair.a_to_b.cut = true;
    pair.a.lapm.release();
    assert_eq!(pair.a.lapm.state(), State::Releasing);
    let t401 = u64::from(config().t401) / 8;
    pair.run(t401 * (u64::from(config().n400) + 1) + 500);
    assert_eq!(pair.a.released(), Some(Release::Local));
    let discs = pair
        .a_to_b
        .log
        .iter()
        .filter(|f| f[..] == [0x03, 0x53])
        .count();
    assert_eq!(discs, usize::from(config().n400) + 1);

    // Clause 8.9.2: SABME to an end that is releasing is answered DM.
    let mut lapm = connected_answerer();
    lapm.release();
    assert_eq!(drain(&mut lapm), [vec![0x01, 0x53]]);
    lapm.receive(&[0x03, 0x7F]);
    assert_eq!(drain(&mut lapm), [vec![0x03, 0x1F]]);
}

// Information transfer.

#[test]
fn a_connection_that_has_ended_is_not_made_again() {
    // The edition has no re-establishment (its clause 8.4.9 releases),
    // and a late SABME, one that was held up on its way and comes after
    // the connection it asked for has been and gone, must not make a
    // connection that the other end knows nothing of. It is refused
    // with DM (clause 8.3.2.1).
    let mut lapm = connected_answerer();
    // DISC with P, and its UA with F.
    lapm.receive(&[0x03, 0x53]);
    assert_eq!(drain(&mut lapm), [vec![0x03, 0x73]]);
    assert_eq!(events(&mut lapm), [Event::Released(Release::Remote)]);
    assert_eq!(lapm.state(), State::Disconnected);
    // SABME with P: DM with F.
    lapm.receive(&[0x03, 0x7F]);
    assert_eq!(drain(&mut lapm), [vec![0x03, 0x1F]]);
    assert!(events(&mut lapm).is_empty());
    assert_eq!(lapm.state(), State::Disconnected);
    // Nor is an XID answered, nor data taken.
    lapm.receive(&[0x03, 0xAF]);
    assert!(drain(&mut lapm).is_empty());
    assert_eq!(lapm.write(b"data"), 0);

    // The originator's side of the same: a machine whose connection
    // could not be made does not try again.
    let mut lapm = Lapm::new(
        Role::Originator,
        Config {
            negotiate: false,
            ..config()
        },
    );
    lapm.establish();
    assert_eq!(drain(&mut lapm), [vec![0x03, 0x7F]]);
    // DM with F: refused.
    lapm.receive(&[0x03, 0x1F]);
    assert_eq!(events(&mut lapm), [Event::Released(Release::Refused)]);
    lapm.establish();
    assert!(drain(&mut lapm).is_empty());
    assert_eq!(lapm.state(), State::Disconnected);
}

fn data(seed: u64, len: usize) -> Vec<u8> {
    Rng(seed).octets(len)
}

#[test]
fn data_is_carried_both_ways_in_order() {
    let mut pair = Pair::new(11);
    pair.connect();
    let (out, back) = (data(1, 60_000), data(2, 45_000));
    pair.a.send(&out);
    pair.b.send(&back);
    assert!(pair.run_until(600_000, Pair::delivered));
    assert_eq!(pair.b.received, out);
    assert_eq!(pair.a.received, back);
    // On a clean line nothing is sent twice, and with data both ways the
    // acknowledgements ride on the I frames (clause 8.4.2.2).
    let (a, b) = (pair.a.lapm.stats(), pair.b.lapm.stats());
    assert_eq!((a.i_resent, b.i_resent), (0, 0));
    assert_eq!(
        (a.rej_sent, b.rej_sent, a.t401_expired, b.t401_expired),
        (0, 0, 0, 0)
    );
    assert_eq!(a.i_sent, 60_000u64.div_ceil(128));
    // The last acknowledgements are a moment behind the last data.
    pair.run(1000);
    assert!(pair.a.lapm.drained() && pair.b.lapm.drained());
    // The line was full: at 2400 bit/s, 60,000 octets in frames of 128
    // with 6 octets of flag, address, control and check sequence each
    // take 209 s, and they took no more than a few percent over.
    let seconds = pair.now as f64 / 8000.0;
    assert!(seconds < 209.0 * 1.05, "{seconds} s");
}

#[test]
fn the_window_closes_at_k_and_the_host_is_held_off() {
    // Clause 8.4.1: no new I frame when V(S) is V(A) + k.
    // With no acknowledgement at all T401 would run out before fifteen
    // frames had gone, so it is a minute here.
    let patient = Config {
        t401: 480_000,
        ..config()
    };
    let mut pair = Pair::with(13, patient, patient);
    pair.connect();
    pair.b_to_a.cut = true;
    pair.a.send(&data(3, 8000));
    pair.run(10_000);
    assert_eq!(pair.a.lapm.outstanding(), MAX_WINDOW);
    let sent = pair.a_to_b.log.iter().filter(|f| f[1] & 1 == 0).count();
    assert_eq!(sent, usize::from(MAX_WINDOW));
    // The machine has taken the window's worth and a queue's worth, and
    // no more: the rest is still the host's.
    assert_eq!(
        pair.a.sent.len(),
        usize::from(MAX_WINDOW) * MAX_N401 + HOST_QUEUE
    );
    assert_eq!(pair.a.lapm.space(), 0);
    assert!(!pair.a.to_send.is_empty());
    // When the acknowledgements come again it all goes through.
    pair.b_to_a.cut = false;
    assert!(pair.run_until(120_000, Pair::delivered));
    assert_eq!(pair.b.received, data(3, 8000));
}

#[test]
fn a_host_that_does_not_read_stops_the_far_end_with_rnr() {
    // Clause 8.4.7: own-receiver busy; clause 8.4.6: peer-receiver busy.
    let mut pair = Pair::new(17);
    pair.connect();
    pair.b.reading = false;
    let sent = data(4, 20_000);
    pair.a.send(&sent);
    pair.run(30_000);
    assert_eq!(pair.b.lapm.busy(), (false, true));
    assert_eq!(pair.a.lapm.busy(), (true, false));
    assert!(pair.b.lapm.stats().rnr_sent >= 1);
    assert!(pair.a.lapm.stats().rnr_received >= 1);
    // Nothing was lost for it: what the far end has is the first of what
    // was sent, and it is not sent more while it is busy.
    let held = pair.b.lapm.readable();
    assert!(held > 1500, "{held}");
    let frames = pair.a_to_b.log.len();
    pair.run(20_000);
    let after: Vec<&Vec<u8>> = pair.a_to_b.log[frames..].iter().collect();
    assert!(
        after.iter().all(|f| f[1] & 1 == 1),
        "I frames to a busy end"
    );
    // The enquiry of clause 8.4.6 goes on while it is busy: RR with P,
    // answered RNR with F.
    assert!(
        after
            .iter()
            .any(|f| f[..2] == [0x03, 0x01] && f[2] & 1 == 1)
    );
    assert_eq!(pair.a.released(), None);
    // The host reads again: RR, and the rest comes.
    pair.b.reading = true;
    assert!(pair.run_until(200_000, Pair::delivered));
    assert_eq!(pair.b.received, sent);
    assert_eq!(pair.b.lapm.busy(), (false, false));
}

#[test]
fn a_lost_i_frame_is_asked_for_again_with_rej() {
    // Clause 8.5.1 and 8.4.4. The third I frame is lost; the fourth is
    // out of sequence and is answered REJ, once; the third and all after
    // it are sent again.
    let sent = data(5, 128 * 8);
    let mut pair = Pair::new(19);
    pair.connect();
    pair.a.send(&sent);
    // Run until the third I frame is on the line, and lose it.
    let mut lost = false;
    while !pair.delivered() && pair.now < 8000 * 120 {
        let i_frames = pair.a_to_b.log.iter().filter(|f| f[1] & 1 == 0).count();
        pair.a_to_b.cut = i_frames == 2 && !lost;
        if i_frames == 3 && !lost {
            lost = true;
            pair.a_to_b.cut = false;
        }
        pair.run(5);
    }
    assert!(lost);
    assert_eq!(pair.b.received, sent);
    let (a, b) = (pair.a.lapm.stats(), pair.b.lapm.stats());
    assert_eq!(b.rej_sent, 1);
    assert_eq!(a.rej_received, 1);
    assert!(a.i_resent >= 1 && a.i_resent <= u64::from(MAX_WINDOW));
    assert!(b.i_discarded >= 1);
    // The REJ on the line: a response from the answerer, N(R) 2.
    assert!(pair.b_to_a.log.contains(&vec![0x03, 0x09, 0x04]));
    // No timer was needed.
    assert_eq!(a.t401_expired, 0);
}

#[test]
fn a_frame_sent_before_going_back_may_be_acknowledged_after() {
    // Clauses 8.4.4 and 8.5.2. Three I frames are sent. The far end has
    // the first two, and answers a frame out of sequence with REJ, N(R)
    // 2; then the third arrives, which was on its way, and it
    // acknowledges that. The REJ took V(S) back to 2, and the
    // acknowledgement is of a frame after it: it is not an N(R) sequence
    // error, and the third frame is not sent again.
    let mut lapm = connected_answerer();
    let sent = data(11, 3 * 128);
    assert_eq!(lapm.write(&sent), sent.len());
    let frames = drain(&mut lapm);
    assert_eq!(frames.len(), 3);
    for (n, frame) in frames.iter().enumerate() {
        // Commands of the answerer: N(S), and N(R) 0.
        assert_eq!(frame[..3], [0x01, (n as u8) << 1, 0x00]);
    }
    assert_eq!(lapm.outstanding(), 3);
    // REJ, a response of the originator, N(R) 2.
    lapm.receive(&[0x01, 0x09, 0x04]);
    assert_eq!(lapm.outstanding(), 1);
    // RR, N(R) 3, before anything more has been sent.
    lapm.receive(&[0x01, 0x01, 0x06]);
    assert_eq!(lapm.state(), State::Connected);
    assert_eq!(lapm.outstanding(), 0);
    assert!(drain(&mut lapm).is_empty());
    assert!(events(&mut lapm).is_empty());
    assert_eq!(lapm.stats().i_resent, 0);
    // The next frame is the fourth.
    assert_eq!(lapm.write(&sent[..16]), 16);
    let frames = drain(&mut lapm);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0][..3], [0x01, 3 << 1, 0x00]);
    // An acknowledgement of a frame that was never sent is the error it
    // was: the connection ends.
    lapm.receive(&[0x01, 0x01, 5 << 1]);
    assert_eq!(lapm.state(), State::Disconnected);
    assert_eq!(events(&mut lapm), [Event::Released(Release::Protocol)]);
}

#[test]
fn a_lost_last_frame_is_found_by_the_timer() {
    // Clauses 8.4.8 and 8.5.3. The only I frame is lost, so nothing is
    // out of sequence at the far end and nothing is said. T401 runs out,
    // the sender asks with RR and P, is answered RR with F and N(R) 0,
    // and sends the frame again.
    let mut pair = Pair::new(23);
    pair.connect();
    pair.a_to_b.cut = true;
    pair.a.send(b"lost once");
    pair.run(200);
    pair.a_to_b.cut = false;
    assert_eq!(pair.a.lapm.outstanding(), 1);
    assert!(!pair.a.lapm.timer_recovery());
    let t401 = u64::from(config().t401) / 8;
    // Not before T401 has run out; and then the condition is entered,
    // and is over as soon as the answer has come.
    pair.run(t401 - 300);
    assert_eq!(pair.a.lapm.stats().t401_expired, 0);
    assert!(pair.b.received.is_empty());
    assert!(pair.run_until(1000, |pair| pair.a.lapm.timer_recovery()));
    assert!(pair.run_until(3000, Pair::delivered));
    assert_eq!(pair.b.received, b"lost once");
    assert!(!pair.a.lapm.timer_recovery());
    assert_eq!(pair.a.lapm.stats().t401_expired, 1);
    assert_eq!(pair.a.lapm.stats().i_resent, 1);
    // The enquiry on the line: RR command with P, RR response with F.
    assert!(pair.a_to_b.log.contains(&vec![0x03, 0x01, 0x01]));
    assert!(pair.b_to_a.log.contains(&vec![0x03, 0x01, 0x01]));

    // The acknowledgement lost and not the frame: the same enquiry, and
    // nothing is sent again, since the answer says it arrived.
    let mut pair = Pair::new(23);
    pair.connect();
    pair.b_to_a.cut = true;
    pair.a.send(b"heard");
    pair.run(400);
    pair.b_to_a.cut = false;
    assert_eq!(pair.b.received, b"heard");
    pair.run(t401 + 1000);
    assert_eq!(pair.a.lapm.outstanding(), 0);
    assert_eq!(pair.a.lapm.stats().i_resent, 0);
    assert_eq!(pair.a.lapm.stats().t401_expired, 1);
}

#[test]
fn a_far_end_that_stops_answering_is_given_up_after_n400_polls() {
    // Clause 8.4.8 b). With data out, and with none (clause 8.12).
    for with_data in [true, false] {
        let mut pair = Pair::new(29);
        pair.connect();
        pair.b_to_a.cut = true;
        if with_data {
            pair.a.send(b"into the void");
        }
        let t401 = u64::from(config().t401) / 8;
        let t403 = u64::from(config().t403) / 8;
        let n400 = u64::from(config().n400);
        let limit = if with_data { 0 } else { t403 } + t401 * (n400 + 1) + 1000;
        assert!(pair.run_until(limit, |pair| pair.a.released().is_some()));
        assert_eq!(pair.a.released(), Some(Release::TimedOut));
        assert_eq!(pair.a.lapm.state(), State::Disconnected);
        // N400 polls, each RR with P.
        let polls = pair
            .a_to_b
            .log
            .iter()
            .filter(|f| f[..] == [0x03, 0x01, 0x01])
            .count();
        assert_eq!(polls as u64, n400, "with data {with_data}");
        // Nothing more is taken from the host.
        assert_eq!(pair.a.lapm.write(b"more"), 0);
    }
}

#[test]
fn an_idle_connection_is_checked_every_t403() {
    // Clause 8.12.3.3: with nothing passing, RR with P when T403 runs
    // out, answered RR with F.
    let mut pair = Pair::new(31);
    pair.connect();
    let (a_frames, b_frames) = (pair.a_to_b.log.len(), pair.b_to_a.log.len());
    pair.run(u64::from(config().t403) / 8 * 3 + 500);
    assert_eq!(pair.a.released(), None);
    assert_eq!(pair.b.released(), None);
    let polls = |log: &[Vec<u8>], address: u8| {
        log.iter()
            .filter(|f| f[..] == [address, 0x01, 0x01])
            .count()
    };
    // Each end asked three times or near it and was answered each time;
    // the two timers are not in step, and an end that has just been
    // asked has its own timer started again.
    let asked =
        polls(&pair.a_to_b.log[a_frames..], 0x03) + polls(&pair.b_to_a.log[b_frames..], 0x01);
    let answered =
        polls(&pair.b_to_a.log[b_frames..], 0x03) + polls(&pair.a_to_b.log[a_frames..], 0x01);
    assert!((3..=6).contains(&asked), "{asked}");
    assert_eq!(asked, answered);
    assert_eq!(pair.a.lapm.stats().t401_expired, 0);
}

// What ends a connection, and what is ignored.

#[test]
fn what_cannot_be_right_ends_the_connection() {
    // Clause 8.4.9.1, each on a connection made by hand.
    #[rustfmt::skip]
    let fatal: [(&str, Vec<u8>); 9] = [
        ("an N(R) of a frame never sent (8.5.2)", vec![0x03, 0x01, 0x02]),
        ("an I frame with such an N(R) (8.5.2)", vec![0x03, 0x00, 0x02, 0x41]),
        ("FRMR (8.5.6)", vec![0x01, 0x87, 0, 0, 0, 0, 0]),
        ("DM with F clear (8.4.9.1)", vec![0x01, 0x0F]),
        ("SREJ, which was not agreed (8.4.5.1)", vec![0x03, 0x0D, 0x00]),
        ("a supervisory frame too long (8.2.4.12 b)", vec![0x03, 0x01, 0x00, 0x00]),
        ("a control field there is none of (8.2.4.1)", vec![0x03, 0xFF]),
        ("an I frame as a response (Table 8)", vec![0x01, 0x00, 0x00, 0x41]),
        ("SABME with an information field (8.2.4.3)", vec![0x03, 0x7F, 0x00]),
    ];
    for (what, frame) in fatal {
        let mut lapm = connected_answerer();
        lapm.receive(&[0x03, 0x00, 0x00, 0x41]);
        lapm.receive(&frame);
        assert_eq!(
            events(&mut lapm),
            [Event::Released(Release::Protocol)],
            "{what}"
        );
        assert_eq!(lapm.state(), State::Disconnected, "{what}");
        // Nothing is sent after but what was owed before.
        let mut buf = [0u8; 16];
        assert_eq!(lapm.read(&mut buf), 1, "{what}");
    }
    // Clause 8.2.4.12 d): an information field longer than N401.
    let mut lapm = connected_answerer();
    let mut long = vec![0x03, 0x00, 0x00];
    long.extend([0x55; MAX_N401 + 1]);
    lapm.receive(&long);
    assert_eq!(events(&mut lapm), [Event::Released(Release::Protocol)]);

    // And what is only ignored (clause 8.1.3 and Table 9).
    #[rustfmt::skip]
    let harmless: [(&str, Vec<u8>); 8] = [
        ("nothing", vec![]),
        ("an address alone", vec![0x03]),
        ("another DLCI (8.1.3 e)", vec![0x07, 0x01, 0x00]),
        ("an address of two octets (8.1.3 e)", vec![0x02, 0x01, 0x01, 0x00]),
        ("an I frame of four octets with the check sequence (8.1.3 b)", vec![0x03, 0x00]),
        ("UA not asked for (Table 9)", vec![0x01, 0x73]),
        ("RR with F not asked for (Table 9)", vec![0x01, 0x01, 0x01]),
        ("TEST, which was not agreed (8.11)", vec![0x03, 0xE3, 0x01, 0x02]),
    ];
    let mut lapm = connected_answerer();
    for (what, frame) in harmless {
        lapm.receive(&frame);
        assert!(events(&mut lapm).is_empty(), "{what}");
        assert_eq!(lapm.state(), State::Connected, "{what}");
    }
    assert_eq!(lapm.stats().ignored, 8);
    // Without a connection, frames that would end one are discarded
    // (clause 8.5.5).
    let mut lapm = Lapm::new(Role::Answerer, config());
    for frame in [
        vec![0x03, 0x01, 0x02],
        vec![0x03, 0xFF],
        vec![0x03, 0x00, 0x00, 0x41],
    ] {
        lapm.receive(&frame);
    }
    assert!(events(&mut lapm).is_empty());
    assert!(drain(&mut lapm).is_empty());
}

// The break.

#[test]
fn a_break_is_carried_and_given_once() {
    // Clause 8.13. The break on the line: a UI command, BRK with N(SB) 0,
    // not destructive and expedited, 300 ms; and its acknowledgement, a
    // UI response, BRKACK with N(RB) 1.
    let mut pair = Pair::new(37);
    pair.connect();
    assert!(pair.a.lapm.send_break(30));
    // One at a time (clause 7.4).
    assert!(!pair.a.lapm.send_break(5));
    pair.run(300);
    assert_eq!(
        pair.b.events[1..],
        [Event::Break {
            length: 30,
            destructive: false
        }]
    );
    assert!(pair.a_to_b.log.contains(&vec![0x03, 0x03, 0x40, 0x40, 30]));
    assert!(pair.b_to_a.log.contains(&vec![0x03, 0x03, 0xE0]));
    // The next has the other sequence number.
    assert!(pair.a.lapm.send_break(0));
    pair.run(300);
    assert!(pair.a_to_b.log.contains(&vec![0x03, 0x03, 0xC0, 0x40, 0]));
    assert!(pair.b_to_a.log.contains(&vec![0x03, 0x03, 0x60]));
    assert_eq!(pair.b.events.len(), 3);

    // The acknowledgement lost: the break is sent again on T401 (clause
    // 8.13.3.4), acknowledged again, and given to the host once (clause
    // 8.13.3.2).
    let mut pair = Pair::new(37);
    pair.connect();
    pair.b_to_a.cut = true;
    pair.a.lapm.send_break(7);
    pair.run(u64::from(config().t401) / 8 + 300);
    pair.b_to_a.cut = false;
    pair.run(u64::from(config().t401) / 8 + 300);
    let breaks = pair
        .b
        .events
        .iter()
        .filter(|e| matches!(e, Event::Break { .. }))
        .count();
    assert_eq!(breaks, 1);
    let sent = pair
        .a_to_b
        .log
        .iter()
        .filter(|f| f.starts_with(&[0x03, 0x03, 0x40]))
        .count();
    assert!(sent >= 2);
    assert!(pair.a.lapm.send_break(1));
    // Data is not disturbed by any of it.
    pair.a.send(b"after the break");
    assert!(pair.run_until(3000, Pair::delivered));

    // A destructive break from the far end, made by hand (Tables 4 and
    // 5): what was not delivered is discarded and the sequence numbers
    // begin again.
    let mut lapm = connected_answerer();
    lapm.receive(&[0x03, 0x00, 0x00, b'x']);
    lapm.receive(&[0x03, 0x03, 0x40, 0xC0]);
    assert_eq!(
        events(&mut lapm),
        [Event::Break {
            length: 0,
            destructive: true
        }]
    );
    let mut buf = [0u8; 8];
    assert_eq!(lapm.read(&mut buf), 0);
    // The acknowledgement is a response from the answerer: its own
    // address.
    assert_eq!(drain(&mut lapm), [vec![0x03, 0x03, 0xE0]]);
    lapm.receive(&[0x03, 0x00, 0x00, b'y']);
    assert_eq!(lapm.read(&mut buf), 1);
    assert_eq!(buf[0], b'y');
}

// The property that matters.

/// Runs a pair with faults both ways, sends data both ways, and gives
/// the pair when everything is delivered or the time is up.
fn carry(seed: u64, faults: Faults, len: usize) -> Pair {
    let mut pair = Pair::new(seed);
    pair.connect();
    pair.faults(faults);
    pair.a.send(&data(seed, len));
    pair.b.send(&data(seed + 1, len / 2));
    pair.run_until(3_000_000, |pair| {
        pair.delivered() || pair.a.released().is_some() || pair.b.released().is_some()
    });
    pair
}

#[test]
fn whatever_is_lost_what_is_delivered_is_what_was_sent() {
    // Frames lost at rates from one in a hundred to one in three, each
    // way. Below the threshold at which N400 polls in a row are all
    // lost, which none of these comes near, the connection holds and
    // every octet arrives once and in order.
    for (seed, loss) in [(41u64, 0.01), (43, 0.05), (47, 0.1), (53, 0.2), (59, 0.33)] {
        let pair = carry(
            seed,
            Faults {
                loss,
                ..Faults::default()
            },
            6000,
        );
        assert_eq!(pair.a.released(), None, "loss {loss}");
        assert_eq!(pair.b.released(), None, "loss {loss}");
        assert_eq!(pair.b.received, pair.a.sent, "loss {loss}");
        assert_eq!(pair.a.received, pair.b.sent, "loss {loss}");
        assert_eq!(pair.a.sent.len(), 6000);
        let stats = pair.a.lapm.stats();
        println!(
            "loss {loss}: {} I frames, {} again, {} REJ received, {} timer expiries, {:.0} s",
            stats.i_sent,
            stats.i_resent,
            stats.rej_received,
            stats.t401_expired,
            pair.now as f64 / 8000.0
        );
        assert!(stats.i_resent > 0);
    }
}

#[test]
fn a_line_that_loses_everything_ends_the_connection_and_nothing_wrong_is_delivered() {
    // Above the threshold: nine frames in ten lost. The connection ends,
    // and what was delivered before it did is the first of what was
    // sent.
    let pair = carry(
        61,
        Faults {
            loss: 0.9,
            ..Faults::default()
        },
        20_000,
    );
    assert!(pair.a.released().is_some() || pair.b.released().is_some());
    assert!(pair.a.sent.starts_with(&pair.b.received));
    assert!(pair.b.sent.starts_with(&pair.a.received));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn any_loss_below_the_threshold_loses_no_data(
        seed in 1u64..1_000_000,
        loss in 0.0f64..0.3,
        len in 1usize..4000,
    ) {
        let pair = carry(seed, Faults { loss, ..Faults::default() }, len);
        prop_assert_eq!(pair.a.released(), None);
        prop_assert_eq!(pair.b.released(), None);
        prop_assert_eq!(&pair.b.received, &pair.a.sent);
        prop_assert_eq!(&pair.a.received, &pair.b.sent);
        prop_assert_eq!(pair.a.sent.len(), len);
    }

    #[test]
    fn frames_repeated_and_out_of_order_never_put_wrong_data_through(
        seed in 1u64..1_000_000,
        loss in 0.0f64..0.2,
        duplicate in 0.0f64..0.3,
        delay in 0.0f64..0.3,
        len in 1usize..3000,
    ) {
        // A line of bits cannot repeat a frame or put one before
        // another, and the Recommendation ends a connection on an N(R)
        // that goes backwards (clause 8.5.2), which a frame out of its
        // order has. So the connection may end; what is delivered is
        // still the first of what was sent, with nothing twice.
        let pair = carry(seed, Faults { loss, duplicate, delay }, len);
        prop_assert!(pair.a.sent.starts_with(&pair.b.received));
        prop_assert!(pair.b.sent.starts_with(&pair.a.received));
        if pair.a.released().is_none() && pair.b.released().is_none() {
            prop_assert_eq!(&pair.b.received, &pair.a.sent);
            prop_assert_eq!(&pair.a.received, &pair.b.sent);
        }
    }

    #[test]
    fn any_frames_at_all_are_borne(
        frames in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..140), 0..200),
        originator in any::<bool>(),
        connected in any::<bool>(),
    ) {
        // Whatever the far end sends, the machine does not panic, and
        // what it gives the host is no more than came in I frames.
        let role = if originator { Role::Originator } else { Role::Answerer };
        let mut lapm = Lapm::new(role, config());
        if connected && !originator {
            lapm.receive(&[0x03, 0x7F]);
        } else if originator {
            lapm.establish();
        }
        let mut out = [0u8; MAX_FRAME];
        let mut buf = [0u8; 4096];
        let (mut given, mut read) = (0usize, 0usize);
        for frame in &frames {
            lapm.receive(frame);
            given += frame.len();
            lapm.advance(80);
            let len = lapm.poll_transmit(&mut out);
            prop_assert!(len <= MAX_FRAME);
            lapm.write(&[0xAA; 16]);
            read += lapm.read(&mut buf);
            while lapm.poll_event().is_some() {}
        }
        prop_assert!(read <= given);
    }
}
