// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Two modems through the channel simulator, from the end of the answer
//! sequence: the handshake of V.22bis clause 6.3 in every pairing, its
//! times against the clauses, the fall back to 1200 bit/s, lost packets,
//! and the retrain of clause 6.4.

mod common;

use chansim::{Channel, ChannelConfig, Echo, Impairments, Noise, Packets, Rng, Side};
use common::compare;
use g711::Law;
use v22::{
    Allowed, BitIo, Event, Failure, GuardTone, Modem, Rate, Role, Settings, Signal, Standard,
    TRANSMIT_DELAY_SAMPLES,
};

/// The bits of one end: random data out, and what came in.
struct Data {
    rng: Rng,
    sent: Vec<bool>,
    received: Vec<bool>,
}

impl BitIo for Data {
    fn tx_bit(&mut self) -> bool {
        let bit = self.rng.next_u64() & 1 == 1;
        self.sent.push(bit);
        bit
    }

    fn rx_bit(&mut self, bit: bool) {
        self.received.push(bit);
    }
}

/// One end of a call.
struct Party {
    law: Law,
    modem: Modem,
    data: Data,
    /// What the modem said, and at which of its samples.
    events: Vec<(u64, Event)>,
    /// Each signal the modem sent, and the sample at which it began on
    /// the line.
    signals: Vec<(u64, Signal)>,
    samples: u64,
    time: f64,
}

impl Party {
    fn new(role: Role, allowed: Allowed, law: Law, seed: u64) -> Party {
        let modem = Modem::new(Settings {
            role,
            allowed,
            guard: GuardTone::Off,
            level_dbm0: -12.0,
            law,
        });
        Party {
            law,
            signals: vec![(u64::from(TRANSMIT_DELAY_SAMPLES), modem.signal())],
            modem,
            data: Data {
                rng: Rng::new(seed),
                sent: Vec::new(),
                received: Vec::new(),
            },
            events: Vec::new(),
            samples: 0,
            time: 0.0,
        }
    }

    fn tick(&mut self, side: Side, channel: &mut Channel) {
        let mut rx = [0u8; 80];
        let mut tx = [0u8; 80];
        channel.receive(side, &mut rx);
        for (octet, out) in rx.iter().zip(&mut tx) {
            let sample = self.modem.process(self.law.decode(*octet), &mut self.data);
            *out = self.law.encode(sample);
            self.samples += 1;
            while let Some(event) = self.modem.poll_event() {
                self.events.push((self.samples, event));
            }
            let signal = self.modem.signal();
            if self.signals.last().map(|last| last.1) != Some(signal) {
                let on_line = self.modem.signal_since() + u64::from(TRANSMIT_DELAY_SAMPLES);
                self.signals.push((on_line, signal));
            }
        }
        channel.send(side, &tx);
    }

    /// When the modem began to send a signal for the `nth` time, in
    /// milliseconds of its own clock, on the line.
    fn sent(&self, signal: Signal, nth: usize) -> f64 {
        let at = self
            .signals
            .iter()
            .filter(|(_, s)| *s == signal)
            .nth(nth)
            .unwrap_or_else(|| panic!("{signal:?} was not sent: {:?}", self.signals))
            .0;
        at as f64 / 8.0
    }

    fn sent_signals(&self) -> Vec<Signal> {
        self.signals.iter().map(|s| s.1).collect()
    }

    fn connected(&self) -> Option<Standard> {
        self.events.iter().find_map(|(_, event)| match event {
            Event::Connected { standard } => Some(*standard),
            _ => None,
        })
    }

    fn failure(&self) -> Option<Failure> {
        self.events.iter().find_map(|(_, event)| match event {
            Event::Failed(failure) => Some(*failure),
            _ => None,
        })
    }

    fn count(&self, wanted: Event) -> usize {
        self.events.iter().filter(|(_, e)| *e == wanted).count()
    }
}

/// A call: end A calls and end B answers.
struct Call {
    a: Party,
    b: Party,
    channel: Channel,
    a_tick: f64,
    ticks: u64,
}

const BIS: Allowed = Allowed::ALL;
const V22: Allowed = Allowed {
    v22bis: false,
    v22: true,
    bell212a: false,
};
const BELL: Allowed = Allowed {
    v22bis: false,
    v22: false,
    bell212a: true,
};
const BIS_ONLY: Allowed = Allowed {
    v22bis: true,
    v22: false,
    bell212a: false,
};

impl Call {
    fn new(calling: Allowed, answering: Allowed, config: &ChannelConfig) -> Call {
        let a_per_b = 1.0 + config.clock_offset_ppm * 1.0e-6;
        Call {
            a: Party::new(Role::Calling, calling, config.law_a, config.seed ^ 0xA),
            b: Party::new(Role::Answering, answering, config.law_b, config.seed ^ 0xB),
            channel: Channel::new(config),
            a_tick: 80.0 / a_per_b,
            ticks: 0,
        }
    }

    /// Runs for this many ticks of 10 ms of end B's clock.
    fn run(&mut self, ticks: u64) {
        let until = self.ticks + ticks;
        while self.ticks < until {
            if self.a.time <= self.b.time {
                self.a.tick(Side::A, &mut self.channel);
                self.a.time += self.a_tick;
            } else {
                self.b.tick(Side::B, &mut self.channel);
                self.b.time += 80.0;
                self.ticks += 1;
            }
        }
    }

    /// Runs until both ends are connected or have given up, for at most
    /// `limit` ticks. Says whether both came to one or the other.
    fn run_handshake(&mut self, limit: u64) -> bool {
        let settled = |party: &Party| party.modem.data() || party.modem.failed();
        while self.ticks < limit {
            self.run(1);
            if settled(&self.a) && settled(&self.b) {
                return true;
            }
        }
        false
    }

    /// Forgets the data so far, runs for some seconds, and gives the bits
    /// each end received against those the other sent.
    fn carry(&mut self, seconds: u64) -> [common::Compared; 2] {
        self.run(30);
        self.a.data.received.clear();
        self.b.data.received.clear();
        self.run(seconds * 100);
        let a = compare(&self.b.data.sent, &self.a.data.received);
        let b = compare(&self.a.data.sent, &self.b.data.received);
        [
            a.unwrap_or_else(|| panic!("end A received nothing that was sent")),
            b.unwrap_or_else(|| panic!("end B received nothing that was sent")),
        ]
    }
}

fn clean() -> ChannelConfig {
    ChannelConfig {
        seed: 1,
        ..ChannelConfig::default()
    }
}

/// The delay of the simulated line each way, in milliseconds: what one
/// end sends, the other has 10 ms later.
const LINE_MS: f64 = 10.0;

/// A line with a little of everything.
fn impaired(seed: u64) -> ChannelConfig {
    let impairments = Impairments {
        gain_db: -16.0,
        loop_slope_db: Some(4.0),
        second_codec: Some(Law::Alaw),
        frequency_offset_hz: -5.0,
        noise: Some(Noise::Snr {
            signal_dbm0: -28.0,
            snr_db: 28.0,
        }),
        echo: Some(Echo {
            delay_samples: 120,
            loss_db: 12.0,
        }),
        ..Impairments::default()
    };
    ChannelConfig {
        seed,
        clock_offset_ppm: -120.0,
        ..ChannelConfig::symmetric(impairments)
    }
}

/// What each pairing of modems is to come to.
const PAIRINGS: [(&str, Allowed, Allowed, Standard, Standard); 9] = [
    (
        "V.22bis calls V.22bis",
        BIS,
        BIS,
        Standard::V22bis,
        Standard::V22bis,
    ),
    ("V.22bis calls V.22", BIS, V22, Standard::V22, Standard::V22),
    ("V.22 calls V.22bis", V22, BIS, Standard::V22, Standard::V22),
    ("V.22 calls V.22", V22, V22, Standard::V22, Standard::V22),
    (
        "Bell 212A calls V.22bis",
        BELL,
        BIS,
        Standard::Bell212A,
        Standard::V22,
    ),
    (
        "V.22bis calls Bell 212A",
        BIS,
        BELL,
        Standard::V22,
        Standard::Bell212A,
    ),
    (
        "Bell 212A calls Bell 212A",
        BELL,
        BELL,
        Standard::Bell212A,
        Standard::Bell212A,
    ),
    (
        "V.22 calls Bell 212A",
        V22,
        BELL,
        Standard::V22,
        Standard::Bell212A,
    ),
    (
        "2400 only calls 2400 only",
        BIS_ONLY,
        BIS_ONLY,
        Standard::V22bis,
        Standard::V22bis,
    ),
];

#[test]
fn every_pairing_reaches_its_mode_and_carries_data() {
    for (line, config) in [("clean", clean()), ("impaired", impaired(3))] {
        for (name, calling, answering, a_is, b_is) in PAIRINGS {
            let mut call = Call::new(calling, answering, &config);
            assert!(call.run_handshake(1200), "{line}, {name}: never settled");
            assert_eq!(
                call.a.connected(),
                Some(a_is),
                "{line}, {name}: {:?}",
                call.a.events
            );
            assert_eq!(
                call.b.connected(),
                Some(b_is),
                "{line}, {name}: {:?}",
                call.b.events
            );
            assert_eq!(call.a.modem.connected(), Some(a_is));
            assert_eq!(a_is.rate(), b_is.rate());
            // Inside four seconds of the answering modem's first signal.
            assert!(call.ticks < 400, "{line}, {name}: {} ticks", call.ticks);

            let [a, b] = call.carry(20);
            let expected = 20 * a_is.rate().bits_per_second() as usize;
            for (end, compared) in [("A", a), ("B", b)] {
                assert_eq!(
                    compared.errors, 0,
                    "{line}, {name}, end {end}: {compared:?}"
                );
                assert!(
                    compared.bits + 64 >= expected,
                    "{line}, {name}: {compared:?}"
                );
            }
            // Nothing else was said.
            assert_eq!(
                call.a.events.len(),
                1,
                "{line}, {name}: {:?}",
                call.a.events
            );
            assert_eq!(
                call.b.events.len(),
                1,
                "{line}, {name}: {:?}",
                call.b.events
            );
        }
    }
}

#[test]
fn a_modem_allowed_2400_only_does_not_fall_back() {
    for (calling, answering) in [(BIS_ONLY, V22), (V22, BIS_ONLY)] {
        let mut call = Call::new(calling, answering, &clean());
        assert!(call.run_handshake(1500));
        let only = if calling == BIS_ONLY {
            &call.a
        } else {
            &call.b
        };
        assert_eq!(only.failure(), Some(Failure::RateNotAllowed));
        assert_eq!(only.connected(), None);
        // The one that gave up is silent, and the other is not left
        // thinking it has a call for long: its carrier is gone.
        assert_eq!(only.modem.signal(), Signal::Silence);
        call.run(100);
        let other = if calling == BIS_ONLY {
            &call.b
        } else {
            &call.a
        };
        assert!(!other.modem.carrier());
    }
}

/// Asserts that a time is within a tolerance of what a clause has.
fn assert_ms(what: &str, measured: f64, clause: f64, tolerance: f64) {
    println!("{what:<66} {measured:>7.1} ms  (clause: {clause} +-{tolerance})");
    assert!(
        (measured - clause).abs() <= tolerance,
        "{what}: {measured} ms, the clause has {clause} within {tolerance}"
    );
}

#[test]
fn the_handshake_at_2400_has_the_times_of_clause_6_3_1_1() {
    let mut call = Call::new(BIS, BIS, &clean());
    assert!(call.run_handshake(600));
    let (a, b) = (&call.a, &call.b);
    // The signals each sent, in order (Figure 5).
    assert_eq!(
        a.sent_signals(),
        [
            Signal::Silence,
            Signal::S1,
            Signal::ScrambledOnes1200,
            Signal::ScrambledOnes2400,
            Signal::Data
        ]
    );
    assert_eq!(
        b.sent_signals(),
        [
            Signal::UnscrambledOnes,
            Signal::S1,
            Signal::ScrambledOnes1200,
            Signal::ScrambledOnes2400,
            Signal::Data
        ]
    );
    // Every time is of the signals on the line, at the modem that the
    // clause is about: what the other sends reaches it LINE_MS later.
    let heard_ones = b.sent(Signal::UnscrambledOnes, 0) + LINE_MS;
    assert_ms(
        "calling: unscrambled 1 detected (155) and silence (456), to S1",
        a.sent(Signal::S1, 0) - heard_ones,
        155.0 + 456.0,
        20.0,
    );
    assert_ms(
        "calling: S1 lasts",
        a.sent(Signal::ScrambledOnes1200, 0) - a.sent(Signal::S1, 0),
        100.0,
        3.0,
    );
    let b_heard_s1_end = a.sent(Signal::ScrambledOnes1200, 0) + LINE_MS;
    assert_ms(
        "answering: end of the caller's S1 to its own S1",
        b.sent(Signal::S1, 0) - b_heard_s1_end,
        20.0,
        20.0,
    );
    assert_ms(
        "answering: S1 lasts",
        b.sent(Signal::ScrambledOnes1200, 0) - b.sent(Signal::S1, 0),
        100.0,
        3.0,
    );
    assert_ms(
        "answering: end of the caller's S1 to scrambled 1 at 2400",
        b.sent(Signal::ScrambledOnes2400, 0) - b_heard_s1_end,
        600.0,
        10.0,
    );
    let a_heard_s1_end = b.sent(Signal::ScrambledOnes1200, 0) + LINE_MS;
    assert_ms(
        "calling: end of the answerer's S1 to scrambled 1 at 2400",
        a.sent(Signal::ScrambledOnes2400, 0) - a_heard_s1_end,
        600.0,
        10.0,
    );
    for (name, party) in [("calling", a), ("answering", b)] {
        assert_ms(
            &format!("{name}: scrambled 1 at 2400 before data"),
            party.sent(Signal::Data, 0) - party.sent(Signal::ScrambledOnes2400, 0),
            200.0,
            10.0,
        );
    }
    // Each is ready to receive when it has had 32 ones at 2400 bit/s
    // from the other, which is 13 ms of them and what the receiver takes
    // to give them: well inside the 200 ms they are sent for.
    let a_ready = a.modem.receive_ready_at().unwrap() as f64 / 8.0;
    let b_ready = b.modem.receive_ready_at().unwrap() as f64 / 8.0;
    let a_heard_2400 = b.sent(Signal::ScrambledOnes2400, 0) + LINE_MS;
    let b_heard_2400 = a.sent(Signal::ScrambledOnes2400, 0) + LINE_MS;
    assert_ms(
        "calling: 32 ones at 2400 detected after",
        a_ready - a_heard_2400,
        60.0,
        60.0,
    );
    assert_ms(
        "answering: 32 ones at 2400 detected after",
        b_ready - b_heard_2400,
        60.0,
        60.0,
    );
}

#[test]
fn the_handshake_at_1200_has_the_times_of_clause_6_3_1_2() {
    // A V.22 modem calls (Figure 6), and a V.22bis modem calls a V.22
    // one (Figure 7).
    for (name, calling, answering) in [("V.22 calls", V22, BIS), ("V.22bis calls", BIS, V22)] {
        let mut call = Call::new(calling, answering, &clean());
        assert!(call.run_handshake(600));
        let (a, b) = (&call.a, &call.b);
        let mut expected = vec![Signal::Silence, Signal::ScrambledOnes1200, Signal::Data];
        if calling == BIS {
            expected.insert(1, Signal::S1);
        }
        assert_eq!(a.sent_signals(), expected, "{name}");
        assert_eq!(
            b.sent_signals(),
            [
                Signal::UnscrambledOnes,
                Signal::ScrambledOnes1200,
                Signal::Data
            ],
            "{name}"
        );
        let first = if calling == BIS {
            Signal::S1
        } else {
            Signal::ScrambledOnes1200
        };
        assert_ms(
            &format!("{name}, calling: unscrambled 1 (155) and silence (456)"),
            a.sent(first, 0) - (b.sent(Signal::UnscrambledOnes, 0) + LINE_MS),
            155.0 + 456.0,
            20.0,
        );
        // Clause 6.3.1.2.2 b: the answering modem detects scrambled 1 in
        // 270 ms, within 40, and then sends its own.
        assert_ms(
            &format!("{name}, answering: scrambled 1 detected, to its own"),
            b.sent(Signal::ScrambledOnes1200, 0) - (a.sent(Signal::ScrambledOnes1200, 0) + LINE_MS),
            270.0,
            40.0,
        );
        // Clause 6.3.1.2.2 c: 765 ms of scrambled 1, then data.
        assert_ms(
            &format!("{name}, answering: scrambled 1 before data"),
            b.sent(Signal::Data, 0) - b.sent(Signal::ScrambledOnes1200, 0),
            765.0,
            10.0,
        );
        // Clause 6.3.1.2.1 c: the calling modem detects scrambled 1 in
        // 270 ms, within 40; d: and is ready to send 765 ms after.
        let a_ready = a.modem.receive_ready_at().unwrap() as f64 / 8.0;
        assert_ms(
            &format!("{name}, calling: scrambled 1 detected"),
            a_ready - (b.sent(Signal::ScrambledOnes1200, 0) + LINE_MS),
            270.0,
            40.0,
        );
        assert_ms(
            &format!("{name}, calling: ready to send after"),
            a.sent(Signal::Data, 0) - a_ready,
            765.0 + f64::from(TRANSMIT_DELAY_SAMPLES) / 8.0,
            10.0,
        );
    }
}

#[test]
fn the_bell_answering_tone_is_2225_hz_and_a_calling_modem_takes_it() {
    // A modem that is Bell 212A and nothing else answers with its tone.
    let mut call = Call::new(BIS, BELL, &clean());
    call.run(20);
    assert_eq!(call.b.modem.signal(), Signal::AnswerTone);
    assert!(call.run_handshake(600));
    assert_eq!(
        call.b.sent_signals(),
        [Signal::AnswerTone, Signal::ScrambledOnes1200, Signal::Data]
    );
    // The tone itself: measured on the line.
    let mut modem = Modem::new(Settings {
        role: Role::Answering,
        allowed: BELL,
        guard: GuardTone::Off,
        level_dbm0: -12.0,
        law: Law::Ulaw,
    });
    let mut data = Data {
        rng: Rng::new(1),
        sent: Vec::new(),
        received: Vec::new(),
    };
    let samples: Vec<f32> = (0..8800).map(|_| modem.process(0.0, &mut data)).collect();
    let tau = std::f64::consts::TAU;
    let amplitude = |hz: f64| {
        let (mut s, mut c) = (0.0, 0.0);
        for (n, &x) in samples[800..].iter().enumerate() {
            let (sin, cos) = (tau * hz * n as f64 / 8000.0).sin_cos();
            s += f64::from(x) * sin;
            c += f64::from(x) * cos;
        }
        2.0 * s.hypot(c) / 8000.0
    };
    let stated = f64::from(dspkit::level::dbm0_to_amplitude(-12.0, Law::Ulaw));
    assert!((amplitude(2225.0) / stated - 1.0).abs() < 0.01);
    assert!(amplitude(2250.0) < stated * 0.01);
    assert!(amplitude(2100.0) < stated * 0.01);
}

#[test]
fn a_lost_packet_at_any_stage_of_the_handshake_is_borne_or_ends_it_cleanly() {
    // A packet of 20 ms lost, in either direction, at each place from
    // the start to after data begins: 2.4 s, 120 packets.
    let mut connected = 0;
    let mut gave_up = Vec::new();
    for (name, calling, answering) in [("2400", BIS, BIS), ("1200", V22, BIS)] {
        for towards_caller in [false, true] {
            for packet in 0..120u64 {
                let lossy = Impairments {
                    packets: Some(Packets {
                        ptime_ms: 20,
                        lost: vec![packet],
                        loss_probability: 0.0,
                    }),
                    ..Impairments::default()
                };
                let mut config = clean();
                if towards_caller {
                    config.b_to_a = lossy;
                } else {
                    config.a_to_b = lossy;
                }
                let mut call = Call::new(calling, answering, &config);
                // It never hangs: inside 15 s each end is connected or
                // has given up.
                let what = format!("{name}, packet {packet}, towards caller {towards_caller}");
                assert!(call.run_handshake(1500), "{what}: never settled");
                match (call.a.connected(), call.b.connected()) {
                    (Some(a), Some(b))
                        if call.a.failure().is_none() && call.b.failure().is_none() =>
                    {
                        // Connected: at the same rate, and the data is
                        // whole, from when the lost packet is well past:
                        // the bits that were in it are lost, as any are
                        // on a line without error control.
                        assert_eq!(a.rate(), b.rate(), "{what}");
                        call.run(280u64.saturating_sub(call.ticks));
                        let [a, b] = call.carry(4);
                        assert_eq!((a.errors, b.errors), (0, 0), "{what}");
                        connected += 1;
                    }
                    _ => {
                        // Given up: the end that did is silent, and
                        // neither is left sending to nobody as though it
                        // had a call.
                        call.run(200);
                        for party in [&call.a, &call.b] {
                            assert!(
                                party.modem.failed() || !party.modem.carrier(),
                                "{what}: {:?} {:?}",
                                call.a.events,
                                call.b.events
                            );
                        }
                        gave_up.push(what);
                    }
                }
            }
        }
    }
    println!("{connected} of 480 connected; gave up: {gave_up:#?}");
    // A handshake survives a lost packet nearly always.
    assert!(connected >= 440, "{connected} of 480");
}

#[test]
fn a_retrain_asked_for_by_either_end_is_answered_and_the_data_goes_on() {
    for asked_by_caller in [true, false] {
        let mut call = Call::new(BIS, BIS, &clean());
        assert!(call.run_handshake(600));
        let [a, b] = call.carry(5);
        assert_eq!((a.errors, b.errors), (0, 0));

        if asked_by_caller {
            call.a.modem.retrain();
        } else {
            call.b.modem.retrain();
        }
        call.run(10);
        // No data passes while it goes on.
        assert!(!call.a.modem.data() || !call.b.modem.data());
        assert!(
            call.run_handshake(call.ticks + 400),
            "the retrain never ended"
        );
        let (asker, other) = if asked_by_caller {
            (&call.a, &call.b)
        } else {
            (&call.b, &call.a)
        };
        for party in [asker, other] {
            assert_eq!(party.count(Event::Retrain), 1, "{:?}", party.events);
            assert_eq!(party.count(Event::Retrained), 1, "{:?}", party.events);
            assert_eq!(party.failure(), None);
        }
        // Clause 6.4 and Figure 8: the one that asked sends S1 and
        // scrambled 1 at 1200 bit/s; the other answers with S1 at the
        // end of that one; each sends scrambled 1 at 2400 bit/s 600 ms
        // after the end of the other's S1, for 200 ms.
        let retrain = [
            Signal::S1,
            Signal::ScrambledOnes1200,
            Signal::ScrambledOnes2400,
            Signal::Data,
        ];
        assert_eq!(asker.sent_signals()[5..], retrain);
        assert_eq!(other.sent_signals()[5..], retrain);
        let who = if asked_by_caller {
            "caller"
        } else {
            "answerer"
        };
        assert_ms(
            &format!("retrain by the {who}: S1 lasts"),
            asker.sent(Signal::ScrambledOnes1200, 1) - asker.sent(Signal::S1, 1),
            100.0,
            3.0,
        );
        let other_heard_end = asker.sent(Signal::ScrambledOnes1200, 1) + LINE_MS;
        assert_ms(
            &format!("retrain by the {who}: the other's S1 after the end of it"),
            other.sent(Signal::S1, 1) - other_heard_end,
            20.0,
            20.0,
        );
        assert_ms(
            &format!("retrain by the {who}: the other's scrambled 1 at 2400"),
            other.sent(Signal::ScrambledOnes2400, 1) - other_heard_end,
            600.0,
            10.0,
        );
        let asker_heard_end = other.sent(Signal::ScrambledOnes1200, 1) + LINE_MS;
        assert_ms(
            &format!("retrain by the {who}: its own scrambled 1 at 2400"),
            asker.sent(Signal::ScrambledOnes2400, 1) - asker_heard_end,
            600.0,
            10.0,
        );
        for party in [asker, other] {
            assert_ms(
                &format!("retrain by the {who}: scrambled 1 at 2400 before data"),
                party.sent(Signal::Data, 1) - party.sent(Signal::ScrambledOnes2400, 1),
                200.0,
                10.0,
            );
        }
        // The data after is whole, both ways.
        let [a, b] = call.carry(10);
        assert_eq!((a.errors, b.errors), (0, 0), "after the retrain");
        // A retrain is not asked for at 1200 bit/s.
        let mut call = Call::new(V22, BIS, &clean());
        assert!(call.run_handshake(600));
        call.a.modem.retrain();
        call.b.modem.retrain();
        call.run(100);
        assert_eq!(call.a.events.len(), 1);
        assert_eq!(call.b.events.len(), 1);
    }
}

#[test]
fn both_ends_asking_for_a_retrain_at_once_come_back_together() {
    let mut call = Call::new(BIS, BIS, &clean());
    assert!(call.run_handshake(600));
    call.a.modem.retrain();
    call.b.modem.retrain();
    call.run(10);
    assert!(call.run_handshake(call.ticks + 400));
    for party in [&call.a, &call.b] {
        assert_eq!(party.count(Event::Retrain), 1);
        assert_eq!(party.count(Event::Retrained), 1);
        assert_eq!(party.failure(), None);
    }
    let [a, b] = call.carry(10);
    assert_eq!((a.errors, b.errors), (0, 0));
}

/// A call whose line can be changed for another while it goes on.
fn change_line(call: &mut Call, config: &ChannelConfig) {
    call.channel = Channel::new(config);
}

#[test]
fn a_line_that_gets_worse_makes_the_modems_retrain_and_they_recover() {
    // The line's loss changes by 6 dB at once, and its response with it:
    // the equalisers are wrong for it, and sixteen points 6 dB too small
    // are decided wrongly. One end or the other finds its error too high
    // and retrains (clause 6.4), and after it the data is whole again.
    let mut call = Call::new(BIS, BIS, &clean());
    assert!(call.run_handshake(600));
    let [a, b] = call.carry(5);
    assert_eq!((a.errors, b.errors), (0, 0));

    let worse = ChannelConfig {
        seed: 5,
        ..ChannelConfig::symmetric(Impairments {
            gain_db: -6.0,
            loop_slope_db: Some(6.0),
            ..Impairments::default()
        })
    };
    change_line(&mut call, &worse);
    // Within five seconds there has been a retrain and it is over.
    call.run(500);
    for party in [&call.a, &call.b] {
        assert!(party.count(Event::Retrain) >= 1, "{:?}", party.events);
        assert_eq!(party.count(Event::Retrain), party.count(Event::Retrained));
        assert_eq!(party.failure(), None, "{:?}", party.events);
        assert!(party.modem.data());
    }
    let [a, b] = call.carry(20);
    assert_eq!((a.errors, b.errors), (0, 0), "after the retrain");
}

#[test]
fn a_line_too_bad_for_2400_ends_the_call_after_a_few_retrains() {
    // Noise 8 dB under the signal: no retrain can make sixteen points of
    // that. The modems try, and give up, and say so.
    let mut call = Call::new(BIS, BIS, &clean());
    assert!(call.run_handshake(600));
    let hopeless = ChannelConfig {
        seed: 7,
        ..ChannelConfig::symmetric(Impairments {
            noise: Some(Noise::Snr {
                signal_dbm0: -12.0,
                snr_db: 0.0,
            }),
            ..Impairments::default()
        })
    };
    change_line(&mut call, &hopeless);
    let mut gave_up = false;
    for _ in 0..6000 {
        call.run(1);
        if call.a.modem.failed() || call.b.modem.failed() {
            gave_up = true;
            break;
        }
    }
    assert!(gave_up, "a: {:?} b: {:?}", call.a.events, call.b.events);
    let failed = if call.a.modem.failed() {
        &call.a
    } else {
        &call.b
    };
    assert_eq!(failed.failure(), Some(Failure::Training));
    assert!(failed.count(Event::Retrain) >= 1);
    assert_eq!(failed.modem.signal(), Signal::Silence);
    // The other is not left with a carrier.
    call.run(200);
    let other = if call.a.modem.failed() && !call.b.modem.failed() {
        &call.b
    } else {
        &call.a
    };
    assert!(other.modem.failed() || !other.modem.carrier());
}

#[test]
fn a_modem_at_1200_finds_the_signal_again_by_itself() {
    // At 1200 bit/s there is no retrain. The line changes so that the
    // equaliser is wrong for it; the receivers find the signal again
    // without a word to each other, and the data after is whole.
    let mut call = Call::new(V22, V22, &clean());
    assert!(call.run_handshake(600));
    let worse = ChannelConfig {
        seed: 9,
        ..ChannelConfig::symmetric(Impairments {
            gain_db: -8.0,
            loop_slope_db: Some(7.0),
            frequency_offset_hz: 6.0,
            ..Impairments::default()
        })
    };
    change_line(&mut call, &worse);
    call.run(400);
    for party in [&call.a, &call.b] {
        assert_eq!(party.events.len(), 1, "{:?}", party.events);
        assert!(party.modem.data());
    }
    let [a, b] = call.carry(20);
    assert_eq!((a.errors, b.errors), (0, 0));
}

#[test]
fn nobody_at_the_other_end_is_waited_for_and_then_given_up() {
    // The answering modem alone goes on sending unscrambled 1: how long
    // it waits is for whoever made it to say.
    let silent = ChannelConfig {
        seed: 1,
        ..ChannelConfig::default()
    };
    let mut answering = Party::new(Role::Answering, BIS, Law::Ulaw, 1);
    let mut channel = Channel::new(&silent);
    for _ in 0..3000 {
        answering.tick(Side::B, &mut channel);
    }
    assert!(answering.events.is_empty());
    assert_eq!(answering.modem.signal(), Signal::UnscrambledOnes);
    // The calling modem alone stays silent.
    let mut channel = Channel::new(&silent);
    let mut calling = Party::new(Role::Calling, BIS, Law::Ulaw, 1);
    for _ in 0..3000 {
        calling.tick(Side::A, &mut channel);
    }
    assert!(calling.events.is_empty());
    assert_eq!(calling.modem.signal(), Signal::Silence);

    // A calling modem that has had its unscrambled 1 and then nothing
    // gives up 5 s after it began to send.
    let mut call = Call::new(BIS, BIS, &clean());
    call.run(70);
    assert_eq!(call.a.modem.signal(), Signal::S1);
    let mut alone = call.a;
    let mut channel = Channel::new(&silent);
    for _ in 0..700 {
        alone.tick(Side::A, &mut channel);
    }
    assert_eq!(alone.failure(), Some(Failure::NoResponse));
    assert_eq!(alone.modem.signal(), Signal::Silence);
}

#[test]
fn the_answer_tone_before_the_handshake_is_not_taken_for_it() {
    // The calling modem hears the 2100 Hz answer tone, which is inside
    // its channel, for three seconds before the answering modem's
    // unscrambled 1 (V.25 clause 2.3; clause 3.3: no response to it).
    // It stays silent through it, and the handshake after is as ever.
    let mut calling = Party::new(Role::Calling, BIS, Law::Ulaw, 1);
    let amplitude = f64::from(dspkit::level::dbm0_to_amplitude(-12.0, Law::Ulaw));
    for n in 0..24_000u32 {
        let sign = if (n / 3600).is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        let tone =
            sign * amplitude * (std::f64::consts::TAU * 2100.0 * f64::from(n) / 8000.0).sin();
        calling.modem.process(tone as f32, &mut calling.data);
        assert_eq!(calling.modem.signal(), Signal::Silence, "at sample {n}");
    }
    // 75 ms of silence, and then the answering modem.
    for _ in 0..600 {
        calling.modem.process(0.0, &mut calling.data);
    }
    let mut call = Call::new(BIS, BIS, &clean());
    call.a = calling;
    assert!(call.run_handshake(600));
    assert_eq!(call.a.connected(), Some(Standard::V22bis));
    assert_eq!(call.b.connected(), Some(Standard::V22bis));
    let [a, b] = call.carry(5);
    assert_eq!((a.errors, b.errors), (0, 0));
    assert_eq!(Rate::B2400, Standard::V22bis.rate());
}

#[test]
fn any_line_at_all_is_borne() {
    // Whatever octets the line brings, from the first sample: nothing
    // panics, what is sent is a sound signal, and no call is claimed
    // out of noise.
    for (seed, role) in [
        (1u64, Role::Calling),
        (2, Role::Answering),
        (3, Role::Calling),
    ] {
        let mut party = Party::new(role, BIS, Law::Ulaw, seed);
        let mut rng = Rng::new(seed);
        for n in 0..400_000u32 {
            // Noise, silence, and stretches of one octet over and over.
            let octet = match (n / 20_000) % 3 {
                0 => rng.next_u64() as u8,
                1 => 0xFF,
                _ => (n / 4000) as u8,
            };
            let sample = party
                .modem
                .process(Law::Ulaw.decode(octet), &mut party.data);
            assert!(sample.is_finite() && sample.abs() < 1.0);
            while let Some(event) = party.modem.poll_event() {
                assert!(matches!(event, Event::Failed(_)), "{event:?} from noise");
            }
        }
        assert!(party.data.received.is_empty());
    }
}
