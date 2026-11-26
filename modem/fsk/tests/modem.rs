// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The 300 bit/s modems: the modulator measured, the modulator into the
//! demodulator through the channel simulator, and the bit error rate
//! against theory.

use chansim::{Channel, ChannelConfig, Echo, Impairments, Noise, Rng, Side};
use dspkit::fir::{self, Fir, Window};
use dspkit::level::{dbm0_to_amplitude, power_to_dbm0};
use fsk::{
    BIT_RATE, Band, CARRIER_OFF_DBM0, CARRIER_ON_DBM0, Demodulator, Modulator, Receiver, Standard,
    StartStopTransmitter, Tones,
};
use g711::Law;

const FS: f64 = 8000.0;
const TAU: f64 = std::f64::consts::TAU;
const LAW: Law = Law::Ulaw;

/// Every modem and band there is.
const ALL: [(Standard, Band); 4] = [
    (Standard::V21, Band::Caller),
    (Standard::V21, Band::Answerer),
    (Standard::Bell103, Band::Caller),
    (Standard::Bell103, Band::Answerer),
];

fn amplitude(samples: &[f32], frequency: f64) -> f64 {
    let (mut s, mut c) = (0.0, 0.0);
    for (n, &x) in samples.iter().enumerate() {
        let (sin, cos) = (TAU * frequency * n as f64 / FS).sin_cos();
        s += f64::from(x) * sin;
        c += f64::from(x) * cos;
    }
    2.0 * s.hypot(c) / samples.len() as f64
}

fn mean_square(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / samples.len() as f64
}

fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

// The frequencies.

#[test]
fn the_frequencies_are_those_of_v21_and_bell_103() {
    // V.21 clause 3: 1080 Hz and 1750 Hz, 100 Hz either way, and the
    // higher frequency is binary 0.
    let low = Tones::of(Standard::V21, Band::Caller);
    let high = Tones::of(Standard::V21, Band::Answerer);
    assert_eq!((low.mark_hz, low.space_hz), (980.0, 1180.0));
    assert_eq!((high.mark_hz, high.space_hz), (1650.0, 1850.0));
    assert_eq!((low.centre_hz(), high.centre_hz()), (1080.0, 1750.0));
    // Bell 103: the higher frequency is mark.
    let low = Tones::of(Standard::Bell103, Band::Caller);
    let high = Tones::of(Standard::Bell103, Band::Answerer);
    assert_eq!((low.mark_hz, low.space_hz), (1270.0, 1070.0));
    assert_eq!((high.mark_hz, high.space_hz), (2225.0, 2025.0));
    assert_eq!(Band::Caller.other(), Band::Answerer);
    assert_eq!(Band::Answerer.other(), Band::Caller);
    assert_eq!(Standard::V21.carrier_name(), "V.21");
    assert_eq!(Standard::Bell103.carrier_name(), "BELL103");
    assert_eq!(BIT_RATE, 300);
}

// The modulator.

#[test]
fn a_steady_bit_is_its_frequency_at_the_level_asked_for() {
    for (standard, band) in ALL {
        let tones = Tones::of(standard, band);
        for (bit, frequency) in [(true, tones.mark_hz), (false, tones.space_hz)] {
            let mut modulator = Modulator::new(tones, -12.0, LAW);
            let samples: Vec<f32> = (0..8800).map(|_| modulator.next_sample(|| bit)).collect();
            // After the transmit filter has filled.
            let samples = &samples[800..];
            let stated = f64::from(dbm0_to_amplitude(-12.0, LAW));
            // V.21 clause 3 allows the modulator 6 Hz; this is exact. The
            // transmit filter is flat to a part in a hundred at the two
            // frequencies.
            let measured = amplitude(samples, f64::from(frequency));
            assert!(
                (measured / stated - 1.0).abs() < 0.01,
                "{standard:?} {band:?}: {measured}"
            );
            for wrong in [frequency - 6.0, frequency + 6.0] {
                assert!(amplitude(samples, f64::from(wrong)) < stated * 1.0e-3);
            }
            assert_eq!(modulator.bit(), bit);
        }
    }
}

#[test]
fn the_bit_rate_is_exactly_300_and_the_phase_is_continuous() {
    let tones = Tones::of(Standard::V21, Band::Caller);
    let mut modulator = Modulator::new(tones, -12.0, LAW);
    let mut asked = 0u32;
    let mut bit = false;
    let mut last = 0.0f32;
    let mut worst = 0.0f32;
    let mut lengths = Vec::new();
    let mut since = 0;
    for _ in 0..80_000 {
        let before = asked;
        let sample = modulator.next_sample(|| {
            asked += 1;
            bit = !bit;
            bit
        });
        if asked != before {
            lengths.push(since);
            since = 0;
        }
        since += 1;
        worst = worst.max((sample - last).abs());
        last = sample;
    }
    // Ten seconds: 3000 bits, each of 26 or 27 samples.
    assert_eq!(asked, 3000);
    assert!(lengths[1..].iter().all(|&len| len == 26 || len == 27));
    // No step at a change of bit: the largest change in a sample is that
    // of a sine wave at the higher frequency, and the few percent the
    // transmit filter may add where the frequency changes.
    let amplitude = f64::from(dbm0_to_amplitude(-12.0, LAW));
    let limit = amplitude * 2.0 * (TAU * 1180.0 / FS / 2.0).sin();
    assert!(f64::from(worst) <= limit * 1.05, "{worst} against {limit}");

    // Started again, it asks for a bit at once and is at mark until then.
    modulator.restart();
    assert!(modulator.bit());
    let mut asked = false;
    modulator.next_sample(|| {
        asked = true;
        false
    });
    assert!(asked);
}

/// The mean square of what a band-pass filter lets through of some
/// samples: the power between two frequencies. The filter is 90 dB down
/// outside its band, so that a strong signal next to the band does not
/// show in it.
fn power_between(samples: &[f32], low: f32, high: f32) -> f64 {
    let mut filter = Fir::<255>::new(fir::bandpass(low, high, Window::Kaiser(8.6)));
    let out: Vec<f32> = samples.iter().map(|&x| filter.process(x)).collect();
    mean_square(&out[255..])
}

#[test]
fn the_signal_stays_in_its_band() {
    for (standard, band) in ALL {
        let tones = Tones::of(standard, band);
        let other = Tones::of(standard, band.other());
        let mut modulator = Modulator::new(tones, -12.0, LAW);
        let mut rng = Rng::new(11);
        // Eight seconds of random bits.
        let samples: Vec<f32> = (0..64_000)
            .map(|_| modulator.next_sample(|| rng.next_u64() & 1 == 1))
            .collect();
        let total = mean_square(&samples[255..]);
        let level = power_to_dbm0(total as f32, LAW);
        // The transmit filter takes the little that is outside the band.
        assert!(
            (level + 12.0).abs() < 0.15,
            "{standard:?} {band:?}: {level}"
        );
        // Nearly all of it is within 350 Hz of the middle of its band,
        // and within 270 Hz of the middle of the other band, which is all
        // that this end's own receiver lets in, there is 50 dB less.
        let own = power_between(
            &samples,
            tones.centre_hz() - 350.0,
            tones.centre_hz() + 350.0,
        );
        let others = power_between(
            &samples,
            other.centre_hz() - 270.0,
            other.centre_hz() + 270.0,
        );
        assert!(own / total > 0.98, "{standard:?} {band:?}: {}", own / total);
        assert!(
            10.0 * (others / total).log10() < -50.0,
            "{standard:?} {band:?}: {}",
            10.0 * (others / total).log10()
        );
    }
}

// The modulator into the demodulator.

/// One end sending characters to the other through a channel.
struct Link {
    transmitter: StartStopTransmitter,
    modulator: Modulator,
    channel: Channel,
    receiver: Receiver,
    law: Law,
    received: Vec<u8>,
    carrier_changes: Vec<(usize, bool)>,
    breaks: u32,
    samples: usize,
    carrier: bool,
    // The time of each end, in the receiving end's samples, and what a
    // tick of the sending end is in them: the two ends are on their own
    // clocks, as in a `chansim::Duplex`.
    a_time: f64,
    b_time: f64,
    a_tick: f64,
}

impl Link {
    fn new(standard: Standard, band: Band, level: f32, config: &ChannelConfig) -> Link {
        let law = config.law_a;
        Link {
            transmitter: StartStopTransmitter::new(),
            modulator: Modulator::new(Tones::of(standard, band), level, law),
            channel: Channel::new(config),
            receiver: Receiver::new(standard, band, 100, config.law_b),
            law,
            received: Vec::new(),
            carrier_changes: Vec::new(),
            breaks: 0,
            samples: 0,
            carrier: false,
            a_time: 0.0,
            b_time: 0.0,
            a_tick: 80.0 / (1.0 + config.clock_offset_ppm * 1.0e-6),
        }
    }

    /// Runs for some ticks of the receiving end, sending `data` as fast
    /// as it goes. The transmitter is on when `on` says so.
    fn run(&mut self, ticks: usize, data: &mut &[u8], on: bool) {
        let mut done = 0;
        while done < ticks {
            if self.a_time <= self.b_time {
                self.a_time += self.a_tick;
                self.send_tick(data, on, None);
            } else {
                self.b_time += 80.0;
                self.receive_tick();
                done += 1;
            }
        }
    }

    /// One tick of the sending end. With `held`, the line is held at that
    /// bit and no character is sent.
    fn send_tick(&mut self, data: &mut &[u8], on: bool, held: Option<bool>) {
        let taken = self.transmitter.write(data);
        *data = &data[taken..];
        let mut tx = [self.law.silence(); 80];
        if on {
            for octet in &mut tx {
                let transmitter = &mut self.transmitter;
                let sample = self.modulator.next_sample(|| match held {
                    Some(bit) => bit,
                    None => transmitter.next_bit(),
                });
                *octet = self.law.encode(sample);
            }
        }
        self.channel.send(Side::A, &tx);
    }

    fn receive_tick(&mut self) {
        let mut rx = [0u8; 80];
        self.channel.receive(Side::B, &mut rx);
        for octet in rx {
            let out = self.receiver.process(LAW.decode(octet));
            if out.carrier != self.carrier {
                self.carrier = out.carrier;
                self.carrier_changes.push((self.samples, out.carrier));
            }
            self.received.extend(out.framed.character);
            if out.framed.line_break {
                self.breaks += 1;
            }
            self.samples += 1;
        }
    }

    /// Sends all of `data` and waits for it to arrive.
    fn carry(&mut self, data: &[u8]) {
        // Carrier first, for the detector, and then the data: ten bits a
        // character and a little over.
        self.run(40, &mut &[][..], true);
        let mut rest = data;
        let ticks = data.len() * 10 * 8000 / 300 / 80 + 60;
        self.run(ticks, &mut rest, true);
        assert!(rest.is_empty());
    }
}

fn clean() -> ChannelConfig {
    ChannelConfig::default()
}

#[test]
fn every_modem_and_band_carries_data_on_a_clean_line() {
    for (standard, band) in ALL {
        for law in [Law::Ulaw, Law::Alaw] {
            let config = ChannelConfig {
                law_a: law,
                law_b: Law::Ulaw,
                ..clean()
            };
            let mut link = Link::new(standard, band, -12.0, &config);
            // Every character there is, and then a thousand at random.
            let mut data: Vec<u8> = (0..=255).collect();
            data.extend(random_bytes(1, 1000));
            link.carry(&data);
            assert_eq!(link.received, data, "{standard:?} {band:?} {law:?}");
            assert_eq!(link.receiver.framing_errors(), 0);
            // As open as it gets: a steady frequency gives 0.72.
            let eye = link.receiver.eye_opening();
            assert!(eye > 0.55, "{standard:?} {band:?}: {eye}");
        }
    }
}

#[test]
fn data_is_carried_from_minus_10_to_minus_43_dbm0() {
    // V.21 clause 8.3 puts the line signal detector's threshold at
    // -43 dBm: down to there the modem is to work.
    for (standard, band) in ALL {
        for level in [-10.0f32, -20.0, -30.0, -40.0, -42.5] {
            let config = ChannelConfig {
                a_to_b: Impairments {
                    gain_db: level + 10.0,
                    ..Impairments::default()
                },
                ..clean()
            };
            let mut link = Link::new(standard, band, -10.0, &config);
            let data = random_bytes(2, 600);
            link.carry(&data);
            assert_eq!(link.received, data, "{standard:?} {band:?} at {level}");
            assert_eq!(link.receiver.framing_errors(), 0);
            let measured = link.receiver.demodulator().level_dbm0();
            // At the bottom G.711 has few steps left and the reading is
            // rougher.
            assert!((measured - level).abs() < 1.0, "{level}: {measured}");
        }
        // Below the lower threshold there is no carrier and no data.
        let config = ChannelConfig {
            a_to_b: Impairments {
                gain_db: -39.0,
                ..Impairments::default()
            },
            ..clean()
        };
        let mut link = Link::new(standard, band, -10.0, &config);
        link.carry(&random_bytes(2, 100));
        assert!(link.received.is_empty() && link.carrier_changes.is_empty());
    }
}

#[test]
fn a_frequency_offset_is_borne_and_measured() {
    // V.21 clause 3: the demodulator must tolerate 12 Hz. 20 Hz is borne
    // too.
    for (standard, band) in ALL {
        for offset in [-20.0f32, -12.0, -7.0, 7.0, 12.0, 20.0] {
            let config = ChannelConfig {
                a_to_b: Impairments {
                    frequency_offset_hz: offset,
                    gain_db: -15.0,
                    ..Impairments::default()
                },
                ..clean()
            };
            let mut link = Link::new(standard, band, -12.0, &config);
            let data = random_bytes(3, 600);
            link.carry(&data);
            assert_eq!(link.received, data, "{standard:?} {band:?} {offset} Hz");
            let measured = link.receiver.demodulator().frequency_error_hz();
            assert!(
                (measured - offset).abs() < 2.0,
                "{standard:?} {band:?} {offset}: {measured}"
            );
        }
        let mut link = Link::new(standard, band, -12.0, &clean());
        link.carry(&random_bytes(3, 300));
        let measured = link.receiver.demodulator().frequency_error_hz();
        assert!(measured.abs() < 1.0, "{standard:?} {band:?}: {measured}");
    }
}

#[test]
fn a_far_end_on_another_clock_is_followed() {
    // 0.01 percent is what a crystal is out by. A far end's sample clock
    // moves its frequencies as well as its bits, so half a percent is as
    // far as this goes: that is 11 Hz at 2225 Hz, which is the 12 Hz
    // V.21 clause 3 asks a demodulator to bear. A host whose characters
    // are slow or fast by percents is another matter, and is in the tests
    // of the framing.
    for (standard, band) in ALL {
        for ppm in [-5000.0, -1000.0, -100.0, 100.0, 1000.0, 5000.0] {
            let config = ChannelConfig {
                clock_offset_ppm: ppm,
                ..clean()
            };
            let mut link = Link::new(standard, band, -15.0, &config);
            let data = random_bytes(4, 600);
            link.carry(&data);
            assert_eq!(link.received, data, "{standard:?} {band:?} {ppm} ppm");
            assert_eq!(link.receiver.framing_errors(), 0, "{ppm}");
            assert_eq!(link.channel.stats(Side::B).underruns, 0, "{ppm}");
        }
    }
}

#[test]
fn both_directions_at_once_with_each_end_hearing_its_own_echo() {
    for standard in [Standard::V21, Standard::Bell103] {
        // The far end arrives 25 dB down, at -37 dBm0, and each end's own
        // transmission comes back to it only 10 dB down, at -22 dBm0: the
        // echo is 15 dB louder than what is to be received.
        let impairments = Impairments {
            gain_db: -25.0,
            echo: Some(Echo {
                delay_samples: 160,
                loss_db: 10.0,
            }),
            ..Impairments::default()
        };
        let config = ChannelConfig::symmetric(impairments);
        let mut channel = Channel::new(&config);
        let mut a_tx = StartStopTransmitter::new();
        let mut b_tx = StartStopTransmitter::new();
        let mut a_mod = Modulator::new(Tones::of(standard, Band::Caller), -12.0, LAW);
        let mut b_mod = Modulator::new(Tones::of(standard, Band::Answerer), -12.0, LAW);
        let mut a_rx = Receiver::new(standard, Band::Answerer, 100, LAW);
        let mut b_rx = Receiver::new(standard, Band::Caller, 100, LAW);
        let from_a = random_bytes(5, 1500);
        let from_b = random_bytes(6, 1500);
        let (mut rest_a, mut rest_b) = (&from_a[..], &from_b[..]);
        let (mut at_a, mut at_b) = (Vec::new(), Vec::new());
        for tick in 0..5200 {
            if tick > 40 {
                let taken = a_tx.write(rest_a);
                rest_a = &rest_a[taken..];
                let taken = b_tx.write(rest_b);
                rest_b = &rest_b[taken..];
            }
            let (mut a_out, mut b_out) = ([0u8; 80], [0u8; 80]);
            for (a, b) in a_out.iter_mut().zip(&mut b_out) {
                *a = LAW.encode(a_mod.next_sample(|| a_tx.next_bit()));
                *b = LAW.encode(b_mod.next_sample(|| b_tx.next_bit()));
            }
            let (mut a_in, mut b_in) = ([0u8; 80], [0u8; 80]);
            channel.tick(&a_out, &b_out, &mut a_in, &mut b_in);
            for (&a, &b) in a_in.iter().zip(&b_in) {
                at_a.extend(a_rx.process(LAW.decode(a)).framed.character);
                at_b.extend(b_rx.process(LAW.decode(b)).framed.character);
            }
        }
        assert_eq!(at_a, from_b, "{standard:?}");
        assert_eq!(at_b, from_a, "{standard:?}");
        assert_eq!(a_rx.framing_errors() + b_rx.framing_errors(), 0);
        // And an end hears nothing of itself: with the far end silent
        // there is no carrier, however loud the echo.
        let mut alone = Receiver::new(standard, Band::Answerer, 100, LAW);
        let mut modulator = Modulator::new(Tones::of(standard, Band::Caller), -9.0, LAW);
        let mut rng = Rng::new(8);
        for _ in 0..80_000 {
            let own = modulator.next_sample(|| rng.next_u64() & 1 == 1);
            assert!(!alone.process(own).carrier, "{standard:?}");
        }
    }
}

#[test]
fn the_line_signal_detector_has_the_thresholds_and_times_of_v21() {
    assert_eq!((CARRIER_ON_DBM0, CARRIER_OFF_DBM0), (-43.0, -48.0));
    for (standard, band) in ALL {
        let tones = Tones::of(standard, band);
        // A steady mark at a level, straight into the demodulator, from a
        // modulator that has been running: what changes is the level.
        let feed = |demodulator: &mut Demodulator, level: f32, samples: usize| {
            let mut modulator = Modulator::new(tones, level, LAW);
            for _ in 0..400 {
                modulator.next_sample(|| true);
            }
            let mut changes = Vec::new();
            let mut carrier = demodulator.carrier();
            for n in 0..samples {
                let out = demodulator.process(modulator.next_sample(|| true));
                if out.carrier != carrier {
                    carrier = out.carrier;
                    changes.push(n);
                }
            }
            changes
        };
        // V.21 clause 8.3: on above -43 dBm, off below -48 dBm, and in
        // between as it was.
        let mut demodulator = Demodulator::new(tones, 500, LAW);
        assert!(feed(&mut demodulator, -44.0, 16_000).is_empty());
        assert!(!demodulator.carrier());
        let on = feed(&mut demodulator, -42.0, 16_000);
        assert_eq!(on.len(), 1, "{standard:?} {band:?}");
        assert!(demodulator.carrier());
        assert!(feed(&mut demodulator, -47.0, 16_000).is_empty());
        let off = feed(&mut demodulator, -49.0, 16_000);
        assert_eq!(off.len(), 1);
        assert!(feed(&mut demodulator, -44.0, 16_000).is_empty());
        assert!(!demodulator.carrier());

        // V.21 clause 8.2.2, Table 2, for the switched network: on in 300
        // to 700 ms, off in 20 to 80 ms. The test tone is to be from 1 dB
        // above the threshold to the highest level there is.
        for level in [-42.0f32, -20.0, 3.0] {
            let mut demodulator = Demodulator::new(tones, 500, LAW);
            let on = feed(&mut demodulator, level, 16_000)[0];
            assert!((2400..=5600).contains(&on), "{level}: on after {on}");
            let mut off = None;
            for n in 0..8000 {
                if !demodulator.process(0.0).carrier {
                    off = Some(n);
                    break;
                }
            }
            let off = off.expect("the carrier goes");
            assert!(
                (160..=640).contains(&off),
                "{standard:?} {band:?} at {level}: off after {off}"
            );
        }
    }
}

#[test]
fn a_break_is_reported_and_is_not_data() {
    let mut link = Link::new(Standard::V21, Band::Caller, -15.0, &clean());
    link.carry(b"before");
    // Half a second of space.
    for _ in 0..50 {
        link.send_tick(&mut &[][..], true, Some(false));
        link.receive_tick();
    }
    link.carry(b"after");
    assert_eq!(link.received, b"beforeafter");
    assert_eq!(link.breaks, 1);
    assert_eq!(link.receiver.framing_errors(), 0);
}

#[test]
fn when_the_carrier_goes_the_data_stops() {
    let mut link = Link::new(Standard::Bell103, Band::Answerer, -15.0, &clean());
    link.carry(b"some data");
    link.run(100, &mut &[][..], false);
    assert_eq!(link.received, b"some data");
    assert_eq!(link.carrier_changes.len(), 2);
    assert!(link.carrier_changes[0].1 && !link.carrier_changes[1].1);
    // Silence, and the idle octets of a lost packet in either law, are
    // not a carrier.
    let mut receiver = Receiver::new(Standard::V21, Band::Caller, 100, LAW);
    for octet in [0xFFu8, 0x7F, 0xD5] {
        for _ in 0..16_000 {
            let out = receiver.process(LAW.decode(octet));
            assert!(!out.carrier && out.framed.character.is_none());
        }
    }
    receiver.reset();
    assert_eq!(receiver.framing_errors(), 0);
}

// The bit error rate.

/// The bit error rate of a demodulator at a ratio of energy per bit to
/// noise density, with the bits sampled where they are known to be: the
/// demodulator alone, without the start-stop receiver's timing.
fn bit_error_rate(standard: Standard, band: Band, eb_n0_db: f64, bits: usize, seed: u64) -> f64 {
    let level = -20.0f32;
    // The noise is white over the 4000 Hz to half the sample rate, and a
    // bit lasts 1/300 s: Eb/N0 is the signal-to-noise ratio times
    // 4000/300.
    let snr_db = eb_n0_db - 10.0 * (4000.0f64 / 300.0).log10();
    let config = ChannelConfig {
        seed,
        delay_samples: 0,
        a_to_b: Impairments {
            noise: Some(Noise::Snr {
                signal_dbm0: level,
                snr_db: snr_db as f32,
            }),
            ..Impairments::default()
        },
        ..clean()
    };
    let tones = Tones::of(standard, band);
    let mut modulator = Modulator::new(tones, level, LAW);
    let mut demodulator = Demodulator::new(tones, 100, LAW);
    let mut channel = Channel::new(&config);
    let mut rng = Rng::new(seed ^ 0xB175);
    let mut sent = Vec::with_capacity(bits + 8);
    let mut soft = Vec::with_capacity(bits * 27 + 80);
    let mut starts = Vec::with_capacity(bits + 8);
    let mut n = 0usize;
    while sent.len() < bits + 12 {
        let mut tx = [0u8; 80];
        for octet in &mut tx {
            let before = sent.len();
            let sample = modulator.next_sample(|| {
                let bit = rng.next_u64() & 1 == 1;
                sent.push(bit);
                bit
            });
            if sent.len() != before {
                starts.push(n);
            }
            *octet = LAW.encode(sample);
            n += 1;
        }
        let mut rx = [0u8; 80];
        channel.send(Side::A, &tx);
        channel.receive(Side::B, &mut rx);
        soft.extend(
            rx.iter()
                .map(|&octet| demodulator.process(LAW.decode(octet)).soft),
        );
    }
    // Where in the output a bit is: so many samples after it began to be
    // sent, for the two band filters and the correlators. It is the same
    // whatever the noise: 64 samples for each filter and 26 for the
    // correlators.
    let delay = 64 + 64 + 26;
    let mut errors = 0u32;
    let mut counted = 0u32;
    for (k, &start) in starts.iter().enumerate().skip(40).take(bits - 40) {
        let at = start + delay;
        if at >= soft.len() {
            break;
        }
        counted += 1;
        if (soft[at] > 0.0) != sent[k] {
            errors += 1;
        }
    }
    f64::from(errors) / f64::from(counted)
}

/// The Eb/N0, in dB, at which the non-coherent detector of two orthogonal
/// frequencies has a bit error rate: the textbook curve, on which the
/// rate is exp(-Eb / 2 N0) / 2.
fn orthogonal_eb_n0_db(bit_error_rate: f64) -> f64 {
    10.0 * (-2.0 * (2.0 * bit_error_rate).ln()).log10()
}

/// The bit error rate of the best coherent detector there can be of two
/// frequencies this far apart: Q(sqrt((1 - r) Eb / N0)), with r the
/// correlation of the two over a bit, sin(2 pi h) / (2 pi h) for
/// frequencies h bit rates apart. No detector that takes a bit at a time
/// does better.
fn coherent_bound(eb_n0_db: f64) -> f64 {
    let h = 200.0 / 300.0;
    let r = (TAU * h).sin() / (TAU * h);
    let x = ((1.0 - r) * 10f64.powf(eb_n0_db / 10.0)).sqrt();
    // Q(x), by Simpson's rule on the normal density out to x + 12.
    let steps = 4000;
    let width = 12.0 / f64::from(steps);
    let density = |t: f64| (-t * t / 2.0).exp() / TAU.sqrt();
    let mut sum = density(x) + density(x + 12.0);
    for n in 1..steps {
        let weight = if n % 2 == 1 { 4.0 } else { 2.0 };
        sum += weight * density(x + f64::from(n) * width);
    }
    sum * width / 3.0
}

/// The least by which the demodulator is to better the non-coherent
/// curve of orthogonal frequencies, in dB of Eb/N0, at bit error rates
/// from 1 in 100 to 1 in 10,000. A negative number here would be an
/// implementation loss.
const MARGIN_DB: f64 = 0.3;

#[test]
fn the_bit_error_rate_is_no_worse_than_the_non_coherent_curve() {
    // With no noise there are no errors at all.
    for (standard, band) in ALL {
        assert_eq!(bit_error_rate(standard, band, 60.0, 20_000, 1), 0.0);
    }
    println!("modem               Eb/N0   BER        curve at     margin    coherent bound");
    let mut least = f64::MAX;
    for (standard, band) in ALL {
        for eb_n0 in [8.0, 10.0, 12.0] {
            let ber = bit_error_rate(standard, band, eb_n0, 400_000, 21);
            let curve = orthogonal_eb_n0_db(ber);
            // How much less Eb/N0 this demodulator needs than the curve
            // does for the same error rate.
            let margin = curve - eb_n0;
            let bound = coherent_bound(eb_n0);
            println!(
                "{:<8} {:<9}  {eb_n0:>4.1}   {ber:.3e}  {curve:>6.2} dB    {margin:+.2} dB  {bound:.3e}",
                format!("{standard:?}"),
                format!("{band:?}"),
            );
            least = least.min(margin);
            // The curve is that of orthogonal frequencies detected a bit
            // at a time with no memory. These frequencies are closer than
            // orthogonal and their phase is continuous, and a filter
            // before the detector makes use of both: the demodulator does
            // better than the curve, by the margin asserted here.
            assert!(
                margin > MARGIN_DB,
                "{standard:?} {band:?} at {eb_n0}: {margin}"
            );
            // It cannot do better than the coherent bound. If it seems
            // to, the noise is not at the level the test thinks.
            assert!(
                ber > bound,
                "{standard:?} {band:?} at {eb_n0}: {ber} under {bound}"
            );
        }
    }
    println!("least margin over the non-coherent curve {least:.2} dB");
}

#[test]
fn characters_are_carried_through_noise_with_no_errors_above_a_stated_ratio() {
    // At Eb/N0 of 20 dB, which is a signal-to-noise ratio of 8.8 dB over
    // the whole band, 3000 characters arrive without one error.
    for (standard, band) in ALL {
        let config = ChannelConfig {
            seed: 31,
            a_to_b: Impairments {
                noise: Some(Noise::Snr {
                    signal_dbm0: -20.0,
                    snr_db: 8.75,
                }),
                ..Impairments::default()
            },
            ..clean()
        };
        let mut link = Link::new(standard, band, -20.0, &config);
        let data = random_bytes(7, 3000);
        link.carry(&data);
        assert_eq!(link.received, data, "{standard:?} {band:?}");
        assert_eq!(link.receiver.framing_errors(), 0);
    }
}
