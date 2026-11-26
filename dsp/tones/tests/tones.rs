// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The generated tones measured against V.25 and V.8, and the detectors
//! against the generated tones, noise, speech and silence.

use dspkit::level::{dbm0_to_amplitude, dbm0_to_power, power_to_dbm0};
use g711::Law;
use tones::{
    ANS_DURATION_MS, ANSAM_DURATION_MS, ANSWER_SILENCE_MS, ANSWER_TONE_RECOGNITION_MS, AnswerTone,
    AnswerToneDetector, AnswerToneKind, CALLING_TONE_OFF_MS, CALLING_TONE_ON_MS, CallingTone,
    CallingToneDetector, POST_TONE_SILENCE_MS, REVERSAL_INTERVAL_MS, REVERSAL_TOLERANCE_MS,
    ToneDetector, V8_ANSWER_SILENCE_MIN_MS, samples_of_ms,
};

const FS: f64 = 8000.0;
const TAU: f64 = std::f64::consts::TAU;
const LAW: Law = Law::Ulaw;

/// The amplitude and the phase, as the two parts of a phasor, of the sine
/// wave of a frequency in some samples. The samples are a whole number of
/// cycles of every frequency asked about, so one does not leak into
/// another.
fn component(samples: &[f32], frequency: f64) -> (f64, f64) {
    let (mut s, mut c) = (0.0, 0.0);
    for (n, &x) in samples.iter().enumerate() {
        let (sin, cos) = (TAU * frequency * n as f64 / FS).sin_cos();
        s += f64::from(x) * sin;
        c += f64::from(x) * cos;
    }
    let scale = 2.0 / samples.len() as f64;
    (s * scale, c * scale)
}

fn amplitude(samples: &[f32], frequency: f64) -> f64 {
    let (s, c) = component(samples, frequency);
    s.hypot(c)
}

fn mean_square(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / samples.len() as f64
}

fn generate(kind: AnswerToneKind, level: f32, len: usize) -> Vec<f32> {
    let mut tone = AnswerTone::new(kind, level, LAW);
    let mut samples = vec![0.0; len];
    tone.fill(&mut samples);
    samples
}

/// Noise from a seed: the same seed gives the same noise.
struct Noise {
    state: u64,
    sigma: f64,
}

impl Noise {
    /// White noise at a level in dBm0.
    fn new(seed: u64, level_dbm0: f32) -> Noise {
        Noise {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            sigma: f64::from(dbm0_to_power(level_dbm0, LAW)).sqrt(),
        }
    }

    fn uniform(&mut self) -> f64 {
        // xorshift64*.
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let bits = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        (bits as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn next(&mut self) -> f32 {
        // Box and Muller's normal deviate.
        let (u, v) = (self.uniform(), self.uniform());
        (self.sigma * (-2.0 * u.ln()).sqrt() * (TAU * v).cos()) as f32
    }
}

// The generators.

#[test]
fn the_timings_are_inside_what_the_recommendations_allow() {
    // V.25 clause 2.3, and V.8 clause 7.2: 425 to 475 ms.
    assert!((425..=475).contains(&REVERSAL_INTERVAL_MS));
    assert_eq!(REVERSAL_INTERVAL_MS - REVERSAL_TOLERANCE_MS, 425);
    // V.25 clause 2.2: 3.3 s, 0.7 s either way.
    assert!((2600..=4000).contains(&ANS_DURATION_MS));
    // V.8 clause 8.2.2: 5 s, 1 s either way.
    assert!((4000..=6000).contains(&ANSAM_DURATION_MS));
    // V.25 clause 4.3: 1.8 to 2.5 s. V.8 clause 8.2: at least 0.2 s.
    assert!((1800..=2500).contains(&ANSWER_SILENCE_MS));
    assert_eq!(V8_ANSWER_SILENCE_MIN_MS, 200);
    // V.8 clause 8.2.2: 75 ms, 5 ms either way; V.25 clause 4.4 allows 20.
    assert!((70..=80).contains(&POST_TONE_SILENCE_MS));
    // V.25 clause 2.1: on for 0.5 to 0.7 s and off for 1.5 to 2.0 s.
    assert!((500..=700).contains(&CALLING_TONE_ON_MS));
    assert!((1500..=2000).contains(&CALLING_TONE_OFF_MS));
    // V.25 clause 3.9: recognised in 100 to 600 ms.
    assert!((100..=600).contains(&ANSWER_TONE_RECOGNITION_MS));
    assert_eq!(AnswerToneKind::Ans.duration_ms(), 3300);
    assert_eq!(AnswerToneKind::AnsReversing.duration_ms(), 3300);
    assert_eq!(AnswerToneKind::Ansam.duration_ms(), 5000);
    assert_eq!(AnswerToneKind::AnsamReversing.duration_ms(), 5000);
}

#[test]
fn ans_is_2100_hz_at_the_level_asked_for() {
    for level in [-9.0f32, -12.0, -20.0] {
        let samples = generate(AnswerToneKind::Ans, level, 8000);
        let stated = f64::from(dbm0_to_amplitude(level, LAW));
        // All of it is at 2100 Hz: V.25 clause 2.2 allows 15 Hz either
        // way, and this is exact to the oscillator's step.
        assert!((amplitude(&samples, 2100.0) / stated - 1.0).abs() < 1.0e-5);
        for other in [2085.0, 2099.0, 2101.0, 2115.0, 1300.0, 900.0] {
            assert!(amplitude(&samples, other) < stated * 1.0e-4, "{other}");
        }
        let measured = power_to_dbm0(mean_square(&samples) as f32, LAW);
        assert!((measured - level).abs() < 0.01, "{measured}");
    }
}

/// The samples at which a tone's phase against a 2100 Hz reference turns
/// over, found block by block: 2100 Hz is a whole number of cycles in 80
/// samples, and so is the time between two reversals, so a reversal is
/// always between two blocks.
fn reversals(samples: &[f32]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut last: Option<(f64, f64)> = None;
    for (block, chunk) in samples.as_chunks::<80>().0.iter().enumerate() {
        let now = component(chunk, 2100.0);
        if let Some(before) = last {
            let along = now.0 * before.0 + now.1 * before.1;
            let size = now.0.hypot(now.1) * before.0.hypot(before.1);
            // The same direction or the opposite one, and nothing between.
            assert!(along.abs() > 0.999 * size, "block {block}");
            if along < 0.0 {
                found.push(block * 80);
            }
        }
        last = Some(now);
    }
    found
}

#[test]
fn the_phase_reverses_every_450_ms() {
    for kind in [AnswerToneKind::AnsReversing, AnswerToneKind::AnsamReversing] {
        let samples = generate(kind, -12.0, 40_000);
        let found = reversals(&samples);
        // In 5 s: at 450 ms, 900 ms, and so on.
        assert_eq!(found.len(), 11, "{kind:?}: {found:?}");
        for (n, &at) in found.iter().enumerate() {
            assert_eq!(at, (n + 1) * 3600, "{kind:?}");
        }
        // The reversal is a step between two samples: the samples either
        // side of it are those of a sine wave that carries on, negated.
        let amplitude = dbm0_to_amplitude(-12.0, LAW);
        if kind == AnswerToneKind::AnsReversing {
            for (n, &sample) in samples.iter().enumerate().take(3610).skip(3590) {
                let sign = if n < 3600 { 1.0 } else { -1.0 };
                let ideal = sign * f64::from(amplitude) * (TAU * 2100.0 * n as f64 / FS).sin();
                assert!((f64::from(sample) - ideal).abs() < 1.0e-5, "{n}");
            }
        }
    }
    for kind in [AnswerToneKind::Ans, AnswerToneKind::Ansam] {
        assert!(reversals(&generate(kind, -12.0, 40_000)).is_empty());
    }
}

#[test]
fn ansam_is_modulated_at_15_hz_to_the_depth_v8_gives() {
    let level = -12.0f32;
    let samples = generate(AnswerToneKind::Ansam, level, 16_000);
    // A carrier modulated by a sine wave is the carrier and a sideband
    // either side at the modulating frequency, each of half the depth.
    let carrier = amplitude(&samples, 2100.0);
    let lower = amplitude(&samples, 2085.0);
    let upper = amplitude(&samples, 2115.0);
    let depth = (lower + upper) / carrier;
    // V.8 clause 7.2: the envelope is between 0.8 and 1.2 of its average,
    // each to 0.01.
    assert!((depth - 0.2).abs() < 0.001, "{depth}");
    assert!((lower - upper).abs() < carrier * 1.0e-4);
    // And nothing else: the modulation is a sine wave at 15 Hz (to 0.1 Hz)
    // and the carrier is at 2100 Hz (to 1 Hz).
    // Two seconds of samples tell frequencies half a hertz apart.
    for other in [
        2070.0, 2084.5, 2085.5, 2099.0, 2101.0, 2114.5, 2115.5, 2130.0,
    ] {
        assert!(amplitude(&samples, other) < carrier * 1.0e-4, "{other}");
    }
    // The envelope itself, at the carrier's peaks.
    let peak = samples.iter().fold(0f32, |m, &x| m.max(x.abs()));
    assert!((f64::from(peak) / carrier - 1.2).abs() < 0.002, "{peak}");
    // The average power is the level asked for (V.8 clause 7.2).
    let measured = power_to_dbm0(mean_square(&samples) as f32, LAW);
    assert!((measured - level).abs() < 0.01, "{measured}");
    // Outside 2100 Hz give or take 200 Hz there is to be 24 dB less than
    // inside; here there is nothing outside at all.
    assert!(amplitude(&samples, 1800.0) < carrier * 1.0e-4);
    assert!(amplitude(&samples, 2400.0) < carrier * 1.0e-4);
}

#[test]
fn calling_tone_is_bursts_of_1300_hz() {
    let mut tone = CallingTone::new(-12.0, LAW);
    assert!(tone.is_on());
    let mut samples = vec![0.0; 3 * 18_800];
    tone.fill(&mut samples);
    let stated = f64::from(dbm0_to_amplitude(-12.0, LAW));
    for burst in 0..3 {
        let start = burst * 18_800;
        // 600 ms on.
        let on = &samples[start..start + 4800];
        assert!(
            (amplitude(on, 1300.0) / stated - 1.0).abs() < 1.0e-5,
            "{burst}"
        );
        assert!(amplitude(on, 2100.0) < stated * 1.0e-4);
        // 1750 ms off.
        assert!(
            samples[start + 4800..start + 18_800]
                .iter()
                .all(|&x| x == 0.0)
        );
    }
}

// The detectors.

/// What a detector made of a run of samples.
#[derive(Debug, Default)]
struct Heard {
    /// The sample at which the tone was first present.
    detected_at: Option<usize>,
    /// The sample at which it was last present.
    last_present: Option<usize>,
    /// Whether it was ever absent after it had been present.
    dropped: bool,
    /// The sample at which the kind was known, and what it was.
    kind_at: Option<(usize, bool)>,
    /// Whether the kind, once known, ever changed.
    kind_changed: bool,
    reversals: Vec<usize>,
    frequency_error: f32,
}

fn listen(samples: impl IntoIterator<Item = f32>) -> Heard {
    let mut detector = AnswerToneDetector::new(LAW);
    let mut heard = Heard::default();
    for (n, x) in samples.into_iter().enumerate() {
        let status = detector.process(x);
        if status.present {
            heard.detected_at.get_or_insert(n);
            heard.last_present = Some(n);
            heard.frequency_error = detector.frequency_error_hz();
        } else if heard.detected_at.is_some() && heard.last_present == Some(n - 1) {
            heard.dropped = true;
        }
        if let Some(modulated) = status.modulated {
            match heard.kind_at {
                None => heard.kind_at = Some((n, modulated)),
                Some((_, first)) if first != modulated => heard.kind_changed = true,
                _ => {}
            }
        }
        if status.reversal {
            heard.reversals.push(n);
        }
    }
    heard
}

/// A second of silence, and then a tone.
fn after_silence(kind: AnswerToneKind, level: f32, len: usize) -> Vec<f32> {
    let mut samples = vec![0.0; 8000];
    samples.extend(generate(kind, level, len));
    samples
}

#[test]
fn answer_tone_is_recognised_in_the_time_v25_gives() {
    for kind in [
        AnswerToneKind::Ans,
        AnswerToneKind::AnsReversing,
        AnswerToneKind::Ansam,
        AnswerToneKind::AnsamReversing,
    ] {
        for level in [-10.0f32, -12.0, -20.0, -30.0, -40.0] {
            let heard = listen(after_silence(kind, level, 32_000));
            let at = heard.detected_at.expect("detected") - 8000;
            // V.25 clause 3.9: 100 to 600 ms after it arrives.
            assert!((800..1000).contains(&at), "{kind:?} at {level}: {at}");
            // It stays, through every reversal, to the end.
            assert!(!heard.dropped, "{kind:?} at {level}");
            assert_eq!(heard.last_present, Some(8000 + 32_000 - 1));
            // And its kind is known 400 ms in.
            let (kind_at, modulated) = heard.kind_at.expect("kind known");
            assert_eq!(modulated, kind.is_modulated(), "{kind:?} at {level}");
            assert!(!heard.kind_changed, "{kind:?} at {level}");
            assert!(kind_at - 8000 < 3600, "{kind:?} at {level}: {kind_at}");
            // The reversals are seen where they are, and only there.
            if kind.is_reversing() {
                assert_eq!(heard.reversals.len(), 8, "{kind:?} at {level}");
                for pair in heard.reversals.windows(2) {
                    assert!(
                        (pair[1] - pair[0]).abs_diff(3600) <= 8,
                        "{kind:?}: {pair:?}"
                    );
                }
                // Each is known within 20 ms of happening.
                let first = heard.reversals[0] - 8000;
                assert!((3600..3760).contains(&first), "{kind:?}: {first}");
            } else {
                assert!(heard.reversals.is_empty(), "{kind:?} at {level}");
            }
            assert!(heard.frequency_error.abs() < 0.5, "{kind:?}");
        }
    }
}

#[test]
fn a_tone_below_the_threshold_is_not_heard() {
    let heard = listen(after_silence(AnswerToneKind::Ansam, -46.0, 32_000));
    assert_eq!(heard.detected_at, None);
    let heard = listen(after_silence(AnswerToneKind::Ans, -50.0, 32_000));
    assert_eq!(heard.detected_at, None);
}

#[test]
fn the_kind_is_reported_with_its_reversals() {
    for kind in [
        AnswerToneKind::Ans,
        AnswerToneKind::AnsReversing,
        AnswerToneKind::Ansam,
        AnswerToneKind::AnsamReversing,
    ] {
        let mut detector = AnswerToneDetector::new(LAW);
        assert_eq!(detector.kind(), None);
        for x in generate(kind, -12.0, 8000) {
            detector.process(x);
        }
        assert_eq!(detector.kind(), Some(kind));
        assert!(detector.present());
        assert!((power_to_dbm0(detector.power(), LAW) + 12.0).abs() < 2.0);
        detector.reset();
        assert_eq!(detector.kind(), None);
        assert_eq!(detector.reversals(), 0);
        assert!(!detector.present());
    }
}

#[test]
fn the_end_of_the_tone_is_known_within_45_ms() {
    for kind in [AnswerToneKind::Ans, AnswerToneKind::AnsamReversing] {
        for level in [-10.0f32, -30.0] {
            let mut samples = generate(kind, level, 16_000);
            samples.extend(std::iter::repeat_n(0.0, 8000));
            let heard = listen(samples);
            let gone = heard.last_present.unwrap() + 1 - 16_000;
            // The calling modem is to recognise the end of the tone in 75 ms,
            // 20 ms either way (V.25 clause 3.10): this leaves it time.
            assert!((240..=360).contains(&gone), "{kind:?} at {level}: {gone}");
        }
    }
}

#[test]
fn a_tone_and_then_another_is_heard_afresh() {
    // ANSam, silence, and then ANS: the second is not taken for the first.
    let mut samples = generate(AnswerToneKind::AnsamReversing, -12.0, 16_000);
    samples.extend(std::iter::repeat_n(0.0, 4000));
    let second = samples.len();
    samples.extend(generate(AnswerToneKind::Ans, -12.0, 16_000));
    let mut detector = AnswerToneDetector::new(LAW);
    let mut kinds = Vec::new();
    for (n, x) in samples.into_iter().enumerate() {
        let status = detector.process(x);
        if n == second + 400 {
            assert!(!status.present);
            assert_eq!(status.modulated, None);
        }
        if kinds.last() != Some(&status.modulated) {
            kinds.push(status.modulated);
        }
    }
    assert_eq!(kinds, [None, Some(true), None, Some(false)]);
}

#[test]
fn the_frequency_is_held_to_what_v25_allows_and_a_little_more() {
    let tone = |frequency: f64| {
        let amplitude = f64::from(dbm0_to_amplitude(-12.0, LAW));
        (0..16_000).map(move |n| (amplitude * (TAU * frequency * n as f64 / FS).sin()) as f32)
    };
    // V.25 clause 2.2 allows the tone 15 Hz, and V.21 clause 3 supposes
    // a line may move it 6 Hz more.
    for offset in [-21.0, -15.0, -6.0, 0.0, 1.0, 7.0, 15.0, 21.0] {
        let heard = listen(tone(2100.0 + offset));
        assert!(heard.detected_at.is_some(), "{offset}");
        assert!(!heard.dropped, "{offset}");
        assert!(
            (f64::from(heard.frequency_error) - offset).abs() < 0.5,
            "{offset}: {}",
            heard.frequency_error
        );
        assert_eq!(heard.kind_at.map(|k| k.1), Some(false), "{offset}");
        assert!(heard.reversals.is_empty(), "{offset}");
    }
    // Further off it is another tone.
    for offset in [
        -300.0, -100.0, -60.0, -45.0, -35.0, 35.0, 45.0, 60.0, 100.0, 300.0,
    ] {
        let heard = listen(tone(2100.0 + offset));
        assert_eq!(heard.detected_at, None, "{offset}");
    }
}

#[test]
fn answer_tone_is_recognised_through_noise() {
    // White noise across the whole band, 10 dB below the tone. Every one
    // of 40 noises: the tone is recognised in time, stays, is of the right
    // kind, and its reversals are counted right.
    let mut latest = 0;
    for seed in 1..=40u64 {
        for kind in [AnswerToneKind::AnsReversing, AnswerToneKind::AnsamReversing] {
            let mut noise = Noise::new(seed, -30.0);
            let samples: Vec<f32> = after_silence(kind, -20.0, 32_000)
                .into_iter()
                .map(|x| x + noise.next())
                .collect();
            let heard = listen(samples);
            let at = heard.detected_at.expect("detected") - 8000;
            assert!((800..1200).contains(&at), "seed {seed} {kind:?}: {at}");
            latest = latest.max(at);
            assert!(!heard.dropped, "seed {seed} {kind:?}");
            let (_, modulated) = heard.kind_at.expect("kind known");
            assert_eq!(modulated, kind.is_modulated(), "seed {seed} {kind:?}");
            assert!(!heard.kind_changed, "seed {seed} {kind:?}");
            assert_eq!(heard.reversals.len(), 8, "seed {seed} {kind:?}");
            assert!(heard.frequency_error.abs() < 2.0, "seed {seed} {kind:?}");
        }
    }
    println!(
        "answer tone at 10 dB above noise: recognised within {} ms",
        latest / 8
    );
}

#[test]
fn noise_alone_is_never_answer_tone() {
    // Ten minutes of white noise at -15 dBm0, which puts more power at
    // 2100 Hz than the threshold asks of a tone.
    let mut noise = Noise::new(7, -15.0);
    let mut answer = AnswerToneDetector::new(LAW);
    let mut calling = CallingToneDetector::new(LAW);
    for _ in 0..4_800_000 {
        let x = noise.next();
        let status = answer.process(x);
        assert!(!status.present && !status.reversal && status.modulated.is_none());
        assert!(!calling.process(x));
    }
    assert_eq!(calling.bursts(), 0);
    assert_eq!(answer.reversals(), 0);
}

/// Something like speech: a voiced sound with a moving pitch and many
/// harmonics, in syllables, with noise for the sounds that are not voiced.
fn speech(seed: u64, len: usize) -> Vec<f32> {
    let mut noise = Noise::new(seed, -25.0);
    let amplitude = f64::from(dbm0_to_amplitude(-12.0, LAW));
    let mut phase = 0.0f64;
    (0..len)
        .map(|n| {
            let t = n as f64 / FS;
            // The pitch wanders from 90 to 250 Hz, through every value
            // that has a harmonic on 2100 Hz or 1300 Hz.
            let pitch = 170.0 + 80.0 * (TAU * 0.37 * t).sin() + 0.5 * (TAU * 5.1 * t).sin();
            phase += pitch / FS;
            let mut voiced = 0.0;
            for harmonic in 1..=14 {
                let weight = 1.0 / (1.0 + ((harmonic as f64 * pitch - 900.0) / 700.0).powi(2));
                voiced += weight * (TAU * harmonic as f64 * phase).sin();
            }
            // Three syllables a second.
            let syllable = (TAU * 3.0 * t).sin().max(0.0);
            (amplitude * 0.4 * syllable * voiced) as f32 + noise.next()
        })
        .collect()
}

#[test]
fn speech_and_silence_are_never_a_tone() {
    for seed in 1..=4u64 {
        let mut answer = AnswerToneDetector::new(LAW);
        let mut calling = CallingToneDetector::new(LAW);
        // Two minutes each.
        for x in speech(seed, 960_000) {
            assert!(!answer.process(x).present, "seed {seed}");
            assert!(!calling.process(x), "seed {seed}");
        }
    }
    let mut answer = AnswerToneDetector::new(LAW);
    let mut calling = CallingToneDetector::new(LAW);
    for octet in [0xFFu8, 0xD5] {
        // What a lost packet is filled with, in either law, decoded in
        // either law.
        for law in [Law::Ulaw, Law::Alaw] {
            for _ in 0..80_000 {
                assert!(!answer.process(law.decode(octet)).present);
                assert!(!calling.process(law.decode(octet)));
            }
        }
    }
}

#[test]
fn answer_tone_with_another_signal_as_loud_is_not_recognised() {
    // V.25 clause 11: not when the 2100 Hz is accompanied by another
    // signal of comparable level between 350 and 1800 Hz or between 2500
    // and 3400 Hz.
    for other in [350.0, 1000.0, 1300.0, 1800.0, 2500.0, 3400.0] {
        let amplitude = f64::from(dbm0_to_amplitude(-15.0, LAW));
        let samples = generate(AnswerToneKind::Ans, -15.0, 32_000)
            .into_iter()
            .enumerate()
            .map(|(n, x)| x + (amplitude * (TAU * other * n as f64 / FS).sin()) as f32);
        assert_eq!(listen(samples).detected_at, None, "{other}");
    }
    // One 20 dB down does not prevent it.
    let amplitude = f64::from(dbm0_to_amplitude(-35.0, LAW));
    let samples = generate(AnswerToneKind::Ansam, -15.0, 32_000)
        .into_iter()
        .enumerate()
        .map(|(n, x)| x + (amplitude * (TAU * 1000.0 * n as f64 / FS).sin()) as f32);
    let heard = listen(samples);
    assert!(heard.detected_at.is_some() && !heard.dropped);
    assert_eq!(heard.kind_at.map(|k| k.1), Some(true));
}

#[test]
fn the_detectors_work_on_what_g711_carries() {
    for law in [Law::Ulaw, Law::Alaw] {
        for level in [-12.0f32, -40.0] {
            let mut tone = AnswerTone::new(AnswerToneKind::AnsamReversing, level, law);
            let mut detector = AnswerToneDetector::new(law);
            for _ in 0..24_000 {
                let octet = law.encode(tone.next_sample());
                detector.process(law.decode(octet));
            }
            assert_eq!(
                detector.kind(),
                Some(AnswerToneKind::AnsamReversing),
                "{law:?} {level}"
            );
            assert_eq!(detector.reversals(), 6, "{law:?} {level}");
        }
    }
}

#[test]
fn calling_tone_is_heard_burst_by_burst() {
    let mut tone = CallingTone::new(-15.0, LAW);
    let mut detector = CallingToneDetector::new(LAW);
    let mut noise = Noise::new(3, -35.0);
    let mut on_for = 0;
    for n in 0..3 * 18_800 {
        let present = detector.process(tone.next_sample() + noise.next());
        if present {
            on_for += 1;
        }
        // Present from 100 ms into a burst, and not once the burst has
        // been over for 45 ms.
        let into = n % 18_800;
        if (850..4800).contains(&into) {
            assert!(present, "{n}");
        }
        if !(0..4800 + 360).contains(&into) {
            assert!(!present, "{n}");
        }
    }
    assert_eq!(detector.bursts(), 3);
    assert!(on_for > 3 * 3900);
    detector.reset();
    assert_eq!(detector.bursts(), 0);

    // Answer tone is not calling tone, and calling tone is not answer
    // tone.
    let mut calling = CallingToneDetector::new(LAW);
    for x in generate(AnswerToneKind::AnsamReversing, -12.0, 40_000) {
        assert!(!calling.process(x));
    }
    let mut tone = CallingTone::new(-12.0, LAW);
    let mut answer = AnswerToneDetector::new(LAW);
    for _ in 0..3 * 18_800 {
        assert!(!answer.process(tone.next_sample()).present);
    }
}

#[test]
fn a_tone_detector_has_hysteresis() {
    let mut detector = ToneDetector::new(2100.0, 100, LAW);
    let feed = |level: f32, len: usize, detector: &mut ToneDetector| {
        let amplitude = dbm0_to_amplitude(level, LAW);
        let mut oscillator = dspkit::Oscillator::new(2100.0);
        let mut present = false;
        for _ in 0..len {
            present = detector.process(amplitude * oscillator.next_sine());
        }
        present
    };
    // V.21 clause 8.3's levels: on above -43, off below -48, and between
    // the two as it was.
    assert!(!feed(-45.0, 8000, &mut detector));
    assert!(feed(-42.0, 8000, &mut detector));
    assert!(feed(-46.0, 8000, &mut detector));
    assert!(!feed(-50.0, 8000, &mut detector));
    assert!(!feed(-46.0, 8000, &mut detector));
    assert!(feed(-20.0, 8000, &mut detector));
    assert!((power_to_dbm0(detector.power(), LAW) + 20.0).abs() < 0.1);
    detector.reset();
    assert!(!detector.present());
    assert_eq!(samples_of_ms(450), 3600);
}
