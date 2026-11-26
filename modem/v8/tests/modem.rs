// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! V.8 with a modem under it: the answering side sends ANSam and JM, the
//! calling side detects ANSam and sends CM and CJ, each on its V.21
//! channel, through the channel simulator, clean and impaired.

use chansim::{Channel, ChannelConfig, Echo, Impairments, Noise, Packets, Rng};
use fsk::{Band, Modulator, Receiver, Standard, TRANSMIT_DELAY_SAMPLES, Tones};
use g711::Law;
use tones::{AnswerTone, AnswerToneDetector, AnswerToneKind};
use v8::{
    ANSAM_MS, Answerer, CallFunction, Caller, Capabilities, Event, GAP_MS, Modulation, Modulations,
    Outcome, Sending,
};

const V21_DATA: Capabilities = Capabilities::data(Modulations::of(&[Modulation::V21]));

const AGREED_V21: Outcome = Outcome::Agreed {
    modulation: Modulation::V21,
    call_function: CallFunction::Data,
    lapm: false,
};

/// The level both ends send at, in dBm0.
const LEVEL: f32 = -12.0;

/// How long the V.21 receivers of the exchange wait before they call a
/// carrier a carrier: short, because the first sequence is on its way.
const CARRIER_ON_MS: u32 = 20;

/// The silence the answering side keeps before ANSam, in milliseconds
/// (V.8 clause 8.2: at least 0.2 s).
const SILENCE_MS: u32 = 200;

/// The answering modem's side with a modem under it.
struct AnswerEnd {
    capabilities: Capabilities,
    answerer: Option<Answerer>,
    tone: AnswerTone,
    modulator: Modulator,
    receiver: Receiver,
    law: Law,
    samples: u32,
    events: Vec<(u32, Event)>,
}

impl AnswerEnd {
    fn new(capabilities: Capabilities, law: Law) -> AnswerEnd {
        AnswerEnd {
            capabilities,
            answerer: None,
            tone: AnswerTone::new(AnswerToneKind::AnsamReversing, LEVEL, law),
            modulator: Modulator::new(Tones::of(Standard::V21, Band::Answerer), LEVEL, law),
            receiver: Receiver::new(Standard::V21, Band::Caller, CARRIER_ON_MS, law),
            law,
            samples: 0,
            events: Vec::new(),
        }
    }

    fn tick(&mut self, rx: &[u8; 80], tx: &mut [u8; 80]) {
        for (&received, sent) in rx.iter().zip(tx) {
            if self.samples == SILENCE_MS * 8 {
                let answerer = Answerer::new(self.capabilities);
                self.answerer = Some(answerer.with_transmit_delay(TRANSMIT_DELAY_SAMPLES));
            }
            let mut sample = 0.0;
            if let Some(answerer) = &mut self.answerer {
                let out = self.receiver.process(self.law.decode(received));
                if let Some(bit) = out.framed.bit {
                    answerer.receive_bit(bit);
                }
                sample = match answerer.sending() {
                    // What is left in the transmit filter of the bits
                    // already sent, and then nothing.
                    Sending::Silence => self.modulator.flush_sample(),
                    Sending::AnswerTone => self.tone.next_sample(),
                    Sending::Bits => self.modulator.next_sample(|| answerer.next_bit()),
                };
                answerer.advance(1);
                while let Some(event) = answerer.poll() {
                    self.events.push((self.samples / 8, event));
                }
            }
            *sent = self.law.encode(sample);
            self.samples += 1;
        }
    }
}

/// What the calling side sends in place of V.8.
#[derive(Clone, Copy, PartialEq)]
enum CallerKind {
    /// A V.8 calling modem.
    V8,
    /// Nothing at all.
    Silent,
    /// Random bits on V.21 channel No. 1, from the moment it hears ANSam.
    Garbage,
}

/// The calling modem's side with a modem under it.
struct CallEnd {
    kind: CallerKind,
    capabilities: Capabilities,
    caller: Option<Caller>,
    detector: AnswerToneDetector,
    modulator: Modulator,
    receiver: Receiver,
    law: Law,
    rng: Rng,
    samples: u32,
    detected_at: Option<u32>,
    events: Vec<(u32, Event)>,
}

impl CallEnd {
    fn new(kind: CallerKind, capabilities: Capabilities, law: Law) -> CallEnd {
        CallEnd {
            kind,
            capabilities,
            caller: None,
            detector: AnswerToneDetector::new(law),
            modulator: Modulator::new(Tones::of(Standard::V21, Band::Caller), LEVEL, law),
            receiver: Receiver::new(Standard::V21, Band::Answerer, CARRIER_ON_MS, law),
            law,
            rng: Rng::new(99),
            samples: 0,
            detected_at: None,
            events: Vec::new(),
        }
    }

    fn tick(&mut self, rx: &[u8; 80], tx: &mut [u8; 80]) {
        for (&received, sent) in rx.iter().zip(tx) {
            let heard = self.law.decode(received);
            // A calling modem sends CM only if it has detected ANSam,
            // and not plain answer tone (V.8 clause 7.2).
            let status = self.detector.process(heard);
            if self.detected_at.is_none() && status.modulated == Some(true) {
                self.detected_at = Some(self.samples / 8);
                if self.kind == CallerKind::V8 {
                    let caller = Caller::new(self.capabilities);
                    self.caller = Some(caller.with_transmit_delay(TRANSMIT_DELAY_SAMPLES));
                }
            }
            let mut sample = 0.0;
            if let Some(caller) = &mut self.caller {
                let out = self.receiver.process(heard);
                if let Some(bit) = out.framed.bit {
                    caller.receive_bit(bit);
                }
                sample = if caller.sending() == Sending::Bits {
                    self.modulator.next_sample(|| caller.next_bit())
                } else {
                    self.modulator.flush_sample()
                };
                caller.advance(1);
                while let Some(event) = caller.poll() {
                    self.events.push((self.samples / 8, event));
                }
            } else if self.kind == CallerKind::Garbage && self.detected_at.is_some() {
                let rng = &mut self.rng;
                sample = self.modulator.next_sample(|| rng.next_u64() & 1 == 1);
            }
            *sent = self.law.encode(sample);
            self.samples += 1;
        }
    }
}

/// How an exchange through a channel came out.
struct Exchange {
    answerer: Option<(u32, Outcome)>,
    caller: Option<(u32, Outcome)>,
    detected_at: Option<u32>,
    answer_events: Vec<Event>,
    call_events: Vec<Event>,
}

fn outcome(events: &[(u32, Event)]) -> Option<(u32, Outcome)> {
    events.iter().find_map(|&(at, event)| match event {
        Event::Done(outcome) => Some((at, outcome)),
        _ => None,
    })
}

/// Runs an exchange: end A answers and end B calls.
fn run(
    config: &ChannelConfig,
    answer: Capabilities,
    call: Capabilities,
    kind: CallerKind,
) -> Exchange {
    let mut channel = Channel::new(config);
    let mut answer_end = AnswerEnd::new(answer, config.law_a);
    let mut call_end = CallEnd::new(kind, call, config.law_b);
    let (mut a_tx, mut b_tx) = ([config.law_a.silence(); 80], [config.law_b.silence(); 80]);
    let (mut a_rx, mut b_rx) = ([0u8; 80], [0u8; 80]);
    // Fifteen seconds at the most.
    for _ in 0..1500 {
        channel.tick(&a_tx, &b_tx, &mut a_rx, &mut b_rx);
        answer_end.tick(&a_rx, &mut a_tx);
        call_end.tick(&b_rx, &mut b_tx);
        let answered = outcome(&answer_end.events).is_some();
        let called = call_end.caller.is_none() || outcome(&call_end.events).is_some();
        if answered && called {
            break;
        }
    }
    Exchange {
        answerer: outcome(&answer_end.events),
        caller: outcome(&call_end.events),
        detected_at: call_end.detected_at,
        answer_events: answer_end.events.iter().map(|&(_, event)| event).collect(),
        call_events: call_end.events.iter().map(|&(_, event)| event).collect(),
    }
}

/// The line the impaired exchanges are run over: 18 dB of loss each way,
/// so that each end hears the other at -30 dBm0.
fn lossy(more: Impairments) -> Impairments {
    Impairments {
        gain_db: -18.0,
        ..more
    }
}

/// An exchange that is to end with both sides agreed on V.21, and at
/// nearly the same moment.
fn assert_agreed(name: &str, config: &ChannelConfig, latest_ms: u32) -> Exchange {
    let exchange = run(config, V21_DATA, V21_DATA, CallerKind::V8);
    let (answered, answer) = exchange
        .answerer
        .unwrap_or_else(|| panic!("{name}: the answering side did not finish"));
    let (called, call) = exchange
        .caller
        .unwrap_or_else(|| panic!("{name}: the calling side did not finish"));
    assert_eq!(answer, AGREED_V21, "{name}");
    assert_eq!(call, AGREED_V21, "{name}");
    // Each starts V.21 where it is done. The answering side is the
    // later, by what is between the end of CJ being sent and its being
    // received: the two band filters and the correlators, 19 ms, and the
    // line, which here is from 10 to 30 ms.
    assert!(
        answered.abs_diff(called) <= 60,
        "{name}: {answered} and {called}"
    );
    assert!(called <= latest_ms, "{name}: done at {called} ms");
    assert_eq!(
        exchange.answer_events,
        [
            Event::CallMenu(V21_DATA),
            Event::Terminator,
            Event::Done(AGREED_V21)
        ],
        "{name}"
    );
    assert_eq!(
        exchange.call_events,
        [Event::JointMenu(V21_DATA), Event::Done(AGREED_V21)],
        "{name}"
    );
    exchange
}

#[test]
fn two_modems_agree_on_a_clean_line() {
    for (law_a, law_b) in [
        (Law::Ulaw, Law::Ulaw),
        (Law::Alaw, Law::Alaw),
        (Law::Ulaw, Law::Alaw),
    ] {
        let config = ChannelConfig {
            law_a,
            law_b,
            ..ChannelConfig::default()
        };
        let exchange = assert_agreed("clean", &config, 3000);
        // ANSam is known for what it is 350 ms or so after it begins,
        // which is 200 ms after the answering side is connected.
        let detected = exchange.detected_at.unwrap();
        assert!((500..700).contains(&detected), "{detected}");
        // From there: Te of 1 s, two CM and the start of a third, two JM
        // and the start of a third, CJ, and 75 ms.
        let (called, _) = exchange.caller.unwrap();
        assert!(
            (2000..2400).contains(&(called - detected)),
            "{}",
            called - detected
        );
    }
}

#[test]
fn two_modems_agree_on_impaired_lines() {
    // At -30 dBm0, with noise 15 dB below that across the band.
    let noisy = lossy(Impairments {
        noise: Some(Noise::Level(-45.0)),
        ..Impairments::default()
    });
    assert_agreed("noise", &ChannelConfig::symmetric(noisy), 3200);

    // A frequency offset of 7 Hz, one way up and the other way down.
    let config = ChannelConfig {
        a_to_b: lossy(Impairments {
            frequency_offset_hz: 7.0,
            ..Impairments::default()
        }),
        b_to_a: lossy(Impairments {
            frequency_offset_hz: -7.0,
            ..Impairments::default()
        }),
        ..ChannelConfig::default()
    };
    assert_agreed("frequency offset", &config, 3200);

    // Each end's own signal coming back 12 dB down: 18 dB louder than the
    // other end.
    let echoing = lossy(Impairments {
        echo: Some(Echo {
            delay_samples: 200,
            loss_db: 12.0,
        }),
        ..Impairments::default()
    });
    assert_agreed("echo", &ChannelConfig::symmetric(echoing), 3200);

    // A loop that slopes 6 dB across the band, a second codec, and a
    // faint signal: -34 dBm0 at 1000 Hz, and 4 dB less where the answer
    // tone is.
    let sloping = Impairments {
        gain_db: -22.0,
        loop_slope_db: Some(6.0),
        second_codec: Some(Law::Alaw),
        ..Impairments::default()
    };
    assert_agreed("slope", &ChannelConfig::symmetric(sloping), 3200);

    // All of it at once.
    let everything = lossy(Impairments {
        noise: Some(Noise::Level(-48.0)),
        frequency_offset_hz: 5.0,
        loop_slope_db: Some(3.0),
        echo: Some(Echo {
            delay_samples: 320,
            loss_db: 15.0,
        }),
        ..Impairments::default()
    });
    assert_agreed("everything", &ChannelConfig::symmetric(everything), 3200);
}

#[test]
fn a_lost_packet_in_the_exchange_is_made_good() {
    // CM and JM are sequences sent over and over until the other side
    // has two alike, so a lost packet in one costs a sequence or two and
    // no more. A 20 ms packet is lost in each part of the exchange, each
    // way.
    for lost in [
        // CM begins about 1.6 s in; these are in its first sequence.
        vec![82u64],
        vec![85, 86],
        // In the second CM and in the first JM.
        vec![93, 104],
        // One in every 23, all the way through: more than 4 percent.
        (0..30).map(|n| 60 + 23 * n).collect(),
    ] {
        let impaired = lossy(Impairments {
            packets: Some(Packets {
                ptime_ms: 20,
                lost: lost.clone(),
                loss_probability: 0.0,
            }),
            ..Impairments::default()
        });
        let config = ChannelConfig::symmetric(impaired);
        let exchange = run(&config, V21_DATA, V21_DATA, CallerKind::V8);
        let (called, call) = exchange.caller.expect("the calling side finishes");
        let (answered, answer) = exchange.answerer.expect("the answering side finishes");
        assert_eq!(answer, AGREED_V21, "lost {lost:?}");
        assert_eq!(call, AGREED_V21, "lost {lost:?}");
        // If the loss took CJ, the answering side is a second behind.
        let heard_cj = exchange.answer_events.contains(&Event::Terminator);
        let allowed = if heard_cj { 60 } else { 1100 };
        assert!(answered.abs_diff(called) <= allowed, "lost {lost:?}");
        // No later than two seconds behind a clean exchange.
        assert!(called <= 5000, "lost {lost:?}: done at {called} ms");
    }
}

#[test]
fn a_lost_cj_is_made_good_by_the_absence_of_cm() {
    // CJ is sent once. Where it is, is known from a clean exchange: it
    // is the 100 ms that end 83 ms before the calling side is done, 75 ms
    // of silence and the 8 ms its modulator takes to fall quiet.
    let config = ChannelConfig::symmetric(lossy(Impairments::default()));
    let clean = run(&config, V21_DATA, V21_DATA, CallerKind::V8);
    let (done, _) = clean.caller.unwrap();
    let cj_began = u64::from(done) - 83 - 100;
    // The packets the answering side would have had it in: the calling
    // side is end B, and the line's delay is a tick.
    let lost: Vec<u64> = (cj_began / 20..=(u64::from(done) - 83) / 20 + 1).collect();
    let config = ChannelConfig {
        b_to_a: lossy(Impairments {
            packets: Some(Packets {
                ptime_ms: 20,
                lost,
                loss_probability: 0.0,
            }),
            ..Impairments::default()
        }),
        ..config
    };
    let exchange = run(&config, V21_DATA, V21_DATA, CallerKind::V8);
    let (called, call) = exchange.caller.unwrap();
    let (answered, answer) = exchange.answerer.unwrap();
    assert_eq!(called, done);
    assert_eq!((call, answer), (AGREED_V21, AGREED_V21));
    // The answering side did not hear CJ. It went on when it had heard no
    // CM for a second (clause 8.2.3): a second and a little from the
    // last CM, which ended a little before CJ began.
    assert_eq!(
        exchange.answer_events,
        [Event::CallMenu(V21_DATA), Event::Done(AGREED_V21)]
    );
    let late = answered - called;
    assert!((700..=1100).contains(&late), "{late}");
}

#[test]
fn a_caller_without_v8_leaves_the_answerer_to_its_answer_tone() {
    for kind in [CallerKind::Silent, CallerKind::Garbage] {
        // With noise on the line, which is not a call menu either.
        let noisy = lossy(Impairments {
            noise: Some(Noise::Level(-45.0)),
            ..Impairments::default()
        });
        let exchange = run(&ChannelConfig::symmetric(noisy), V21_DATA, V21_DATA, kind);
        // ANSam for its 5 s, the 75 ms of silence, and then on without
        // V.8 (clause 8.2.2).
        let (at, outcome) = exchange.answerer.expect("the answering side finishes");
        assert_eq!(outcome, Outcome::NoResponse);
        assert_eq!(at, SILENCE_MS + ANSAM_MS + GAP_MS - 1);
        assert_eq!(exchange.answer_events, [Event::Done(Outcome::NoResponse)]);
        assert!(exchange.detected_at.is_some());
    }
}

#[test]
fn a_caller_with_nothing_in_common_is_told_so() {
    let caller = Capabilities::data(Modulations::of(&[Modulation::V34, Modulation::V22bis]));
    let exchange = run(&ChannelConfig::default(), V21_DATA, caller, CallerKind::V8);
    assert_eq!(exchange.answerer.unwrap().1, Outcome::NothingInCommon);
    assert_eq!(exchange.caller.unwrap().1, Outcome::NothingInCommon);
    assert_eq!(exchange.answer_events[0], Event::CallMenu(caller));
    // JM has as many modulation octets as CM, and zeros in them.
    assert_eq!(
        exchange.call_events[0],
        Event::JointMenu(Capabilities::data(Modulations::NONE))
    );
}
