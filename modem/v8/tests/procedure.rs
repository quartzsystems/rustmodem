// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The procedure of V.8 (V.8, 11/2000, clause 8): the two sides against
//! each other, bit to bit, with nothing between them but what a test puts
//! there. `modem.rs` has the same through the V.21 modem.

use chansim::Rng;
use v8::{
    ANSAM_MS, Answerer, CM_ABSENT_MS, CallFunction, Caller, Capabilities, EXCHANGE_TIMEOUT_MS,
    Event, GAP_MS, Modulation, Modulations, Outcome, Sending, TE_MS,
};

const V21: Modulations = Modulations::of(&[Modulation::V21]);
const V21_DATA: Capabilities = Capabilities::data(V21);

const AGREED_V21: Outcome = Outcome::Agreed {
    modulation: Modulation::V21,
    call_function: CallFunction::Data,
    lapm: false,
};

/// What happened at each side, with the millisecond it happened in.
#[derive(Debug, Default)]
struct Report {
    answerer: Vec<(u32, Event)>,
    caller: Vec<(u32, Event)>,
    /// When the answering side stopped sending answer tone.
    answer_tone_ended: Option<u32>,
    /// When the calling side first sent a bit.
    call_menu_began: Option<u32>,
}

impl Report {
    fn outcome(events: &[(u32, Event)]) -> Option<(u32, Outcome)> {
        events.iter().find_map(|&(at, event)| match event {
            Event::Done(outcome) => Some((at, outcome)),
            _ => None,
        })
    }

    fn answerer_outcome(&self) -> Option<(u32, Outcome)> {
        Report::outcome(&self.answerer)
    }

    fn caller_outcome(&self) -> Option<(u32, Outcome)> {
        Report::outcome(&self.caller)
    }
}

/// When the calling side comes to have detected ANSam, in milliseconds
/// after the answering side began it.
const DETECTED_MS: u32 = 300;

/// Runs the two sides against each other with nothing between them but
/// `on_the_way`, which is given each bit the calling side sends, with the
/// millisecond, and says what the answering side receives of it, and
/// `back`, the same for the other way.
fn exchange(
    answer: Capabilities,
    call: Option<Capabilities>,
    mut on_the_way: impl FnMut(u32, bool) -> Option<bool>,
    mut back: impl FnMut(u32, bool) -> Option<bool>,
) -> Report {
    let mut answerer = Answerer::new(answer);
    let mut caller: Option<Caller> = None;
    let mut report = Report::default();
    let mut bit_clock = 0u32;
    for sample in 0..8 * 14_000u32 {
        let ms = sample / 8;
        if sample == DETECTED_MS * 8 {
            caller = call.map(Caller::new);
        }
        bit_clock += 300;
        if bit_clock >= 8000 {
            bit_clock -= 8000;
            if let Some(caller) = &mut caller
                && caller.sending() == Sending::Bits
            {
                report.call_menu_began.get_or_insert(ms);
                if let Some(bit) = on_the_way(ms, caller.next_bit()) {
                    answerer.receive_bit(bit);
                }
            }
            if answerer.sending() == Sending::Bits {
                let bit = answerer.next_bit();
                if let (Some(caller), Some(bit)) = (&mut caller, back(ms, bit)) {
                    caller.receive_bit(bit);
                }
            }
        }
        if answerer.sending() != Sending::AnswerTone {
            report.answer_tone_ended.get_or_insert(ms);
        }
        answerer.advance(1);
        while let Some(event) = answerer.poll() {
            report.answerer.push((ms, event));
        }
        if let Some(caller) = &mut caller {
            caller.advance(1);
            while let Some(event) = caller.poll() {
                report.caller.push((ms, event));
            }
        }
        let caller_done = caller.as_ref().is_none_or(Caller::is_done);
        if answerer.is_done() && caller_done && sample > DETECTED_MS * 8 {
            break;
        }
    }
    report
}

fn untouched(_: u32, bit: bool) -> Option<bool> {
    Some(bit)
}

#[test]
fn two_v21_modems_agree_on_v21() {
    let report = exchange(V21_DATA, Some(V21_DATA), untouched, untouched);
    // The answering side: it had two CM alike, sent JM, had CJ, and was
    // done.
    let kinds: Vec<Event> = report.answerer.iter().map(|&(_, event)| event).collect();
    assert_eq!(
        kinds,
        [
            Event::CallMenu(V21_DATA),
            Event::Terminator,
            Event::Done(AGREED_V21)
        ]
    );
    let kinds: Vec<Event> = report.caller.iter().map(|&(_, event)| event).collect();
    assert_eq!(kinds, [Event::JointMenu(V21_DATA), Event::Done(AGREED_V21)]);

    // Clause 8.1.1: the calling side is silent for Te after it detects
    // ANSam, and then sends CM, from the next bit time.
    let began = report.call_menu_began.unwrap() - (DETECTED_MS + TE_MS);
    assert!(began <= 4, "{began}");
    // Clause 8.2.2: JM once two CM alike have been received. A CM of four
    // octets is 60 bits, 200 ms, and the second is known to be whole when
    // the third begins: 433 ms after CM began, and the bit it is in.
    let call_menu_at = report.answerer[0].0;
    let after = call_menu_at - (DETECTED_MS + TE_MS);
    assert!((425..=440).contains(&after), "{after}");
    assert_eq!(report.answer_tone_ended, Some(call_menu_at));
    // Clause 8.1.2: CJ after two JM alike, at the end of the octet CM is
    // in; then both are silent for 75 ms and are done within a bit or two
    // of each other, which is where each starts V.21.
    let (answerer_done, _) = report.answerer_outcome().unwrap();
    let (caller_done, _) = report.caller_outcome().unwrap();
    assert!(
        answerer_done.abs_diff(caller_done) <= 8,
        "{answerer_done} {caller_done}"
    );
    let terminator_at = report.answerer[1].0;
    assert!((GAP_MS..=GAP_MS + 1).contains(&(answerer_done - terminator_at)));
    // The whole exchange, from the detection of ANSam: under two and a
    // half seconds.
    assert!(caller_done - DETECTED_MS < 2500, "{caller_done}");
}

#[test]
fn the_joint_menu_is_what_both_have_and_the_first_of_it_is_used() {
    let all = Modulations::of(&[
        Modulation::V34,
        Modulation::V32bis,
        Modulation::V22bis,
        Modulation::V21,
    ]);
    // A caller with everything and an answerer with V.21: V.21.
    let caller = Capabilities {
        lapm: true,
        ..Capabilities::data(all)
    };
    let report = exchange(V21_DATA, Some(caller), untouched, untouched);
    assert_eq!(report.answerer[0].1, Event::CallMenu(caller));
    assert_eq!(report.caller[0].1, Event::JointMenu(V21_DATA));
    assert_eq!(report.caller_outcome().unwrap().1, AGREED_V21);
    assert_eq!(report.answerer_outcome().unwrap().1, AGREED_V21);

    // Both with everything, and both asking for LAPM: the mode with the
    // lowest item number, which is V.34 (clause 7.4), and LAPM.
    let report = exchange(caller, Some(caller), untouched, untouched);
    let agreed = Outcome::Agreed {
        modulation: Modulation::V34,
        call_function: CallFunction::Data,
        lapm: true,
    };
    assert_eq!(report.caller_outcome().unwrap().1, agreed);
    assert_eq!(report.answerer_outcome().unwrap().1, agreed);

    // V.22 bis and V.21 in common, with V.32 bis on one side only:
    // V.22 bis.
    let one = Capabilities::data(Modulations::of(&[
        Modulation::V32bis,
        Modulation::V22bis,
        Modulation::V21,
    ]));
    let other = Capabilities::data(Modulations::of(&[Modulation::V22bis, Modulation::V21]));
    let report = exchange(one, Some(other), untouched, untouched);
    let agreed = Outcome::Agreed {
        modulation: Modulation::V22bis,
        call_function: CallFunction::Data,
        lapm: false,
    };
    assert_eq!(report.caller_outcome().unwrap().1, agreed);
    assert_eq!(report.answerer_outcome().unwrap().1, agreed);
}

#[test]
fn a_call_menu_with_nothing_in_common_gets_a_joint_menu_of_zeros() {
    // Clause 7.4: with no modes in common JM has as many modulation
    // octets as CM and shows zeros for them all; clauses 8.1.2 and 8.2.3:
    // either side may then disconnect.
    let caller = Capabilities::data(Modulations::of(&[Modulation::V22bis]));
    let report = exchange(V21_DATA, Some(caller), untouched, untouched);
    let empty = Capabilities::data(Modulations::NONE);
    assert_eq!(report.caller[0].1, Event::JointMenu(empty));
    assert_eq!(report.caller_outcome().unwrap().1, Outcome::NothingInCommon);
    assert_eq!(
        report.answerer_outcome().unwrap().1,
        Outcome::NothingInCommon
    );

    // Clause 8.2.3: a call function the answering side does not do gets
    // one it does, and zeros for the modes, though they have V.21 in
    // common.
    let fax = Capabilities {
        call_function: CallFunction::TransmitFax,
        ..V21_DATA
    };
    let report = exchange(V21_DATA, Some(fax), untouched, untouched);
    assert_eq!(report.answerer[0].1, Event::CallMenu(fax));
    assert_eq!(report.caller[0].1, Event::JointMenu(empty));
    assert_eq!(report.caller_outcome().unwrap().1, Outcome::NothingInCommon);
    assert_eq!(
        report.answerer_outcome().unwrap().1,
        Outcome::NothingInCommon
    );
}

#[test]
fn with_no_call_menu_the_answerer_gives_up_after_its_answer_tone() {
    // Clause 8.2.2: ANSam for 5 s, 1 s either way, then 75 ms of silence,
    // and then on as a modem without V.8 would go.
    let report = exchange(V21_DATA, None, untouched, untouched);
    assert_eq!(report.answer_tone_ended, Some(ANSAM_MS));
    assert_eq!(
        report.answerer,
        [(ANSAM_MS + GAP_MS - 1, Event::Done(Outcome::NoResponse))]
    );
    assert!(report.caller.is_empty());

    let mut answerer = Answerer::with_ansam_ms(V21_DATA, 4000);
    assert_eq!(answerer.sending(), Sending::AnswerTone);
    answerer.advance(4000 * 8 - 1);
    assert_eq!(answerer.sending(), Sending::AnswerTone);
    answerer.advance(1);
    assert_eq!(answerer.sending(), Sending::Silence);
    assert!(!answerer.is_done() && answerer.poll().is_none());
    answerer.advance(GAP_MS * 8);
    assert!(answerer.is_done());
    assert_eq!(answerer.poll(), Some(Event::Done(Outcome::NoResponse)));
    assert_eq!(answerer.poll(), None);
    // Done, it sends nothing and takes no notice of anything.
    assert!(answerer.next_bit());
    answerer.receive_bit(false);
    answerer.advance(1_000_000);
    assert_eq!(answerer.sending(), Sending::Silence);
    assert_eq!(answerer.poll(), None);
}

#[test]
fn a_caller_that_sends_garbage_is_no_caller() {
    let mut rng = Rng::new(17);
    let report = exchange(
        V21_DATA,
        Some(V21_DATA),
        |_, _| Some(rng.next_u64() & 1 == 1),
        untouched,
    );
    assert_eq!(
        report.answerer_outcome(),
        Some((ANSAM_MS + GAP_MS - 1, Outcome::NoResponse))
    );
    // And the calling side, which was sending CM all the while, has no
    // JM and gives up.
    let (at, outcome) = report.caller_outcome().unwrap();
    assert_eq!(outcome, Outcome::NoResponse);
    assert_eq!(at, DETECTED_MS + TE_MS + EXCHANGE_TIMEOUT_MS - 1);
}

#[test]
fn a_caller_that_stops_in_the_middle_is_no_caller() {
    // One sequence and a third, and then nothing.
    let stop = DETECTED_MS + TE_MS + 270;
    let report = exchange(
        V21_DATA,
        Some(V21_DATA),
        |ms, bit| (ms < stop).then_some(bit),
        untouched,
    );
    assert_eq!(
        report.answerer,
        [(ANSAM_MS + GAP_MS - 1, Event::Done(Outcome::NoResponse))]
    );
}

#[test]
fn two_call_menus_must_be_alike() {
    // Every sequence is wrong in a different octet from the last three:
    // no two are alike, and JM is never sent (clause 8.2.2).
    let began = DETECTED_MS + TE_MS;
    // A sequence is 200 ms: ten bits of ONEs, ten of synchronization, and
    // four octets of 33 ms each from 67 ms in. A bit in the middle of an
    // octet is changed, a different bit of a different octet each time:
    // 4 ms of the line, which is a bit or two.
    let spoil = |ms: u32, bit: bool, every: u32| {
        let into = ms - began;
        let sequence = into / 200;
        let octet = sequence % 4;
        let place = 67 + 33 * octet + 10 + 3 * (sequence / 4 % 4);
        let spoiled = sequence.is_multiple_of(every) && (place..place + 4).contains(&(into % 200));
        Some(if spoiled { !bit } else { bit })
    };
    let report = exchange(
        V21_DATA,
        Some(V21_DATA),
        |ms, bit| spoil(ms, bit, 1),
        untouched,
    );
    assert_eq!(report.answerer_outcome().unwrap().1, Outcome::NoResponse);

    // Every other sequence wrong: the right ones are alike, though no
    // two of them follow each other, and that is two identical sequences.
    let report = exchange(
        V21_DATA,
        Some(V21_DATA),
        |ms, bit| spoil(ms, bit, 2),
        untouched,
    );
    assert_eq!(report.answerer_outcome().unwrap().1, AGREED_V21);
    assert_eq!(report.caller_outcome().unwrap().1, AGREED_V21);
    // The first is wrong, the second right, the third wrong, the fourth
    // right and known to be whole when the fifth begins: 833 ms.
    let after = report.answerer[0].0 - began;
    assert!((825..=840).contains(&after), "{after}");

    // One wrong and then right ones: JM a sequence later than with none
    // wrong.
    let report = exchange(
        V21_DATA,
        Some(V21_DATA),
        |ms, bit| {
            Some(if (124..130).contains(&(ms - began)) {
                !bit
            } else {
                bit
            })
        },
        untouched,
    );
    assert_eq!(report.answerer_outcome().unwrap().1, AGREED_V21);
    assert_eq!(report.caller_outcome().unwrap().1, AGREED_V21);
    let after = report.answerer[0].0 - began;
    assert!((625..=640).contains(&after), "{after}");
}

#[test]
fn a_joint_menu_that_is_never_acknowledged_is_given_up() {
    // The calling side hears nothing of JM: it goes on with CM and never
    // sends CJ. Clause 8.2.3 leaves the answering side to end JM by
    // other criteria; here, time.
    let report = exchange(V21_DATA, Some(V21_DATA), untouched, |_, _| None);
    assert_eq!(report.answerer[0].1, Event::CallMenu(V21_DATA));
    let (began, _) = report.answerer[0];
    assert_eq!(
        report.answerer[1],
        (
            began + EXCHANGE_TIMEOUT_MS + GAP_MS,
            Event::Done(Outcome::Incomplete)
        )
    );
    assert_eq!(report.caller_outcome().unwrap().1, Outcome::NoResponse);
}

#[test]
fn a_lost_cj_is_made_good_by_the_absence_of_cm() {
    // The calling side's CJ never arrives. It has sent it and is done;
    // the answering side hears no more CM, and after a second of that
    // goes on as if it had heard CJ (clause 8.2.3).
    let mut cm_ended = None;
    let mut zeros = 0u32;
    let report = exchange(
        V21_DATA,
        Some(V21_DATA),
        |ms, bit| {
            // CJ is the only place the calling side sends nine ZEROs in
            // a row: from the ninth on, nothing gets through.
            zeros = if bit { 0 } else { zeros + 1 };
            if zeros >= 9 {
                cm_ended.get_or_insert(ms);
            }
            cm_ended.is_none().then_some(bit)
        },
        untouched,
    );
    assert_eq!(report.caller_outcome().unwrap().1, AGREED_V21);
    let events: Vec<Event> = report.answerer.iter().map(|&(_, event)| event).collect();
    // No terminator was seen, and the outcome is the joint menu's.
    assert_eq!(events, [Event::CallMenu(V21_DATA), Event::Done(AGREED_V21)]);
    let (answerer_done, _) = report.answerer_outcome().unwrap();
    let (caller_done, _) = report.caller_outcome().unwrap();
    // A second after the last CM that was received whole, and the 75 ms.
    let late = answerer_done - caller_done;
    assert!(
        (CM_ABSENT_MS - 400..=CM_ABSENT_MS).contains(&late),
        "{late}"
    );
}

#[test]
fn a_caller_is_silent_then_sends_cm_then_cj_then_is_silent() {
    let mut caller = Caller::new(V21_DATA);
    assert_eq!(caller.sending(), Sending::Silence);
    assert!(caller.next_bit());
    caller.advance(TE_MS * 8 - 1);
    assert_eq!(caller.sending(), Sending::Silence);
    caller.advance(1);
    assert_eq!(caller.sending(), Sending::Bits);
    // Two and a half sequences of CM go out.
    let mut sent = String::new();
    for _ in 0..150 {
        sent.push(if caller.next_bit() { '1' } else { '0' });
    }
    let cm = "111111111100000011110100000111010100000100000100010000010011";
    assert_eq!(&sent[..60], cm);
    assert_eq!(&sent[60..120], cm);
    // JM arrives, three times: two are known to be alike when the third
    // begins.
    let mut answerer = Answerer::new(V21_DATA);
    for _ in 0..3 {
        for bit in cm.chars() {
            answerer.receive_bit(bit == '1');
        }
    }
    assert_eq!(answerer.poll(), Some(Event::CallMenu(V21_DATA)));
    for _ in 0..190 {
        caller.receive_bit(answerer.next_bit());
    }
    assert_eq!(caller.poll(), Some(Event::JointMenu(V21_DATA)));
    // CM finishes the octet it is in, and CJ follows: three octets of
    // ZEROs with their start and stop bits (clauses 3.5 and 8.1.2). The
    // caller had sent 150 bits, so its octet is whole already.
    let mut cj = String::new();
    for _ in 0..30 {
        assert_eq!(caller.sending(), Sending::Bits);
        cj.push(if caller.next_bit() { '1' } else { '0' });
    }
    assert_eq!(cj, "000000000100000000010000000001");
    // It is still sending while the last bit goes out, and is silent
    // from the moment that bit has ended and another is asked for.
    assert_eq!(caller.sending(), Sending::Bits);
    assert!(caller.next_bit());
    assert_eq!(caller.sending(), Sending::Silence);
    assert!(!caller.is_done());
    caller.advance(GAP_MS * 8);
    assert!(caller.is_done());
    assert_eq!(caller.poll(), Some(Event::Done(AGREED_V21)));
    assert_eq!(caller.poll(), None);
}
