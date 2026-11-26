// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! One direction of a channel: from the octets one end sends to the octets
//! the other receives.

use std::collections::VecDeque;

use dspkit::SAMPLE_RATE;
use dspkit::biquad::{Cascade, Coefficients};
use dspkit::hilbert::FrequencyShifter;
use dspkit::interp::Resampler;
use dspkit::level::dbm0_to_power;
use dspkit::math;
use g711::Law;

use crate::rng::Rng;
use crate::{Impairments, Packets, SlipKind, Slips};

/// The taps of the frequency shifter's Hilbert transformer.
const SHIFTER_TAPS: usize = 127;

/// The corners of the voice band the loop filter keeps, in hertz.
const BAND_LOW_HZ: f32 = 180.0;
const BAND_HIGH_HZ: f32 = 3500.0;

/// How much of the receiver's own transmission is kept for its echo, in
/// samples.
const ECHO_HISTORY: usize = 8192;

/// What a path has counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathStats {
    /// Samples taken from the sender.
    pub sent: u64,
    /// Samples given to the receiver.
    pub received: u64,
    /// Samples the receiver was given as silence because the sender had
    /// not sent enough.
    pub underruns: u64,
    /// Packets lost.
    pub packets_lost: u64,
    /// Packets given to the receiver, lost ones among them.
    pub packets: u64,
    /// Slips made.
    pub slips: u64,
    /// The samples waiting between the two ends.
    pub queued: usize,
}

/// The loop filter: the voice band, and a slope across it.
#[derive(Clone, Debug)]
struct LoopFilter {
    band: Cascade<2>,
    // A one-pole low-pass, y += a (x - y), which makes the slope.
    a: f32,
    y: f32,
    // The gain that makes the whole filter one at 1000 Hz.
    gain: f32,
}

impl LoopFilter {
    fn new(slope_db: f32) -> LoopFilter {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let band = Cascade::new([
            Coefficients::highpass(BAND_LOW_HZ, q),
            Coefficients::lowpass(BAND_HIGH_HZ, q),
        ]);
        let a = one_pole_for_slope(f64::from(slope_db.clamp(0.0, 7.0)));
        let slope_gain = one_pole_magnitude(a, 1000.0);
        let gain = 1.0 / (f64::from(band.magnitude_at(1000.0)) * slope_gain);
        LoopFilter {
            band,
            a: a as f32,
            y: 0.0,
            gain: gain as f32,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let x = self.band.process(x);
        self.y += self.a * (x - self.y);
        self.y * self.gain
    }
}

/// The gain at a frequency of the one-pole low-pass with coefficient `a`.
fn one_pole_magnitude(a: f64, frequency_hz: f64) -> f64 {
    let b = 1.0 - a;
    let cos = math::cos(math::TAU * frequency_hz / f64::from(SAMPLE_RATE));
    a / (1.0 + b * b - 2.0 * b * cos).sqrt()
}

/// The coefficient of the one-pole low-pass that is `slope_db` further
/// down at 3000 Hz than at 1000 Hz.
fn one_pole_for_slope(slope_db: f64) -> f64 {
    if slope_db <= 0.0 {
        return 1.0;
    }
    // With b = 1 - a, the power response is a^2 / (1 + b^2 - 2 b cos w).
    // The ratio r of the response at 1000 Hz to that at 3000 Hz is then
    // (1 + b^2 - 2 b c3) / (1 + b^2 - 2 b c1), which is a quadratic in b:
    // (1 - r) b^2 - 2 (c3 - r c1) b + (1 - r) = 0.
    let r = math::pow10(slope_db / 10.0);
    let c1 = math::cos(math::TAU * 1000.0 / f64::from(SAMPLE_RATE));
    let c3 = math::cos(math::TAU * 3000.0 / f64::from(SAMPLE_RATE));
    let k = (c3 - r * c1) / (1.0 - r);
    // b^2 - 2 k b + 1 = 0: the root inside the unit circle. There is none
    // when the slope asked for is more than one pole can make, and then
    // the steepest there is, is used.
    let inside = k * k - 1.0;
    if inside <= 0.0 {
        return 0.01;
    }
    let b = k - inside.sqrt();
    (1.0 - b).clamp(0.01, 1.0)
}

/// One direction of a channel.
#[derive(Clone, Debug)]
pub(crate) struct Path {
    from: Law,
    to: Law,
    rng: Rng,
    pad: Option<f32>,
    gain: Option<f32>,
    filter: Option<LoopFilter>,
    second_codec: Option<Law>,
    shifter: Option<Box<FrequencyShifter<SHIFTER_TAPS>>>,
    resampler: Option<Box<Resampler>>,
    noise_sigma: Option<f64>,
    queue: VecDeque<f32>,
    // The receiver's own transmission, for its echo: a ring of the last
    // samples it sent, and how many it has sent in all.
    echo: Option<(usize, f32)>,
    echo_line: Vec<f32>,
    receiver_sent: u64,
    slips: Option<Slips>,
    last_sample: f32,
    packets: Option<Packets>,
    packet_samples: u32,
    in_packet: u32,
    packet_lost: bool,
    stats: PathStats,
}

impl Path {
    pub(crate) fn new(
        impairments: &Impairments,
        from: Law,
        to: Law,
        delay_samples: u32,
        resample_ratio: Option<f64>,
        seed: u64,
    ) -> Path {
        let factor = |db: f32| math::pow10(f64::from(db) / 20.0) as f32;
        let resampler = resample_ratio.map(|ratio| Box::new(Resampler::new(ratio)));
        // Two ends on two clocks do not tick together: the one that is
        // ahead needs what the other has not sent yet, so the path starts
        // with two ticks more in it.
        let slack = if resampler.is_some() { 160 } else { 0 };
        let mut queue = VecDeque::new();
        queue.extend(std::iter::repeat_n(0.0, delay_samples as usize + slack));
        Path {
            from,
            to,
            rng: Rng::new(seed),
            pad: (impairments.pad_db != 0.0).then(|| factor(-impairments.pad_db.abs())),
            gain: (impairments.gain_db != 0.0).then(|| factor(impairments.gain_db)),
            filter: impairments.loop_slope_db.map(LoopFilter::new),
            second_codec: impairments.second_codec,
            shifter: (impairments.frequency_offset_hz != 0.0)
                .then(|| Box::new(FrequencyShifter::new(impairments.frequency_offset_hz))),
            resampler,
            noise_sigma: impairments
                .noise
                .map(|noise| f64::from(dbm0_to_power(noise.level_dbm0(), to)).sqrt()),
            queue,
            echo: impairments.echo.map(|echo| {
                (
                    echo.delay_samples.max(80) as usize,
                    factor(-echo.loss_db.abs()),
                )
            }),
            echo_line: vec![0.0; ECHO_HISTORY],
            receiver_sent: 0,
            slips: impairments.slips.filter(|slips| slips.every_samples > 0),
            last_sample: 0.0,
            packets: impairments.packets.clone().filter(|p| p.ptime_ms > 0),
            packet_samples: impairments
                .packets
                .as_ref()
                .map_or(0, |p| p.ptime_ms * SAMPLE_RATE / 1000),
            in_packet: 0,
            packet_lost: false,
            stats: PathStats::default(),
        }
    }

    /// Takes the octets the sending end sends.
    pub(crate) fn send(&mut self, tx: &[u8]) {
        for &octet in tx {
            self.stats.sent += 1;
            let mut sample = self.from.decode(octet);
            if let Some(pad) = self.pad {
                sample = self.from.decode(self.from.encode(sample * pad));
            }
            if let Some(gain) = self.gain {
                sample *= gain;
            }
            if let Some(filter) = &mut self.filter {
                sample = filter.process(sample);
            }
            if let Some(law) = self.second_codec {
                sample = law.decode(law.encode(sample));
            }
            if let Some(shifter) = &mut self.shifter {
                sample = shifter.process(sample);
            }
            match &mut self.resampler {
                Some(resampler) => {
                    let out = resampler.process(sample);
                    for &sample in out.as_slice() {
                        let sample = add_noise(&mut self.rng, self.noise_sigma, sample);
                        self.queue.push_back(sample);
                    }
                }
                None => {
                    let sample = add_noise(&mut self.rng, self.noise_sigma, sample);
                    self.queue.push_back(sample);
                }
            }
        }
    }

    /// Notes what the receiving end itself sends, for its echo.
    pub(crate) fn note_receiver_sent(&mut self, tx: &[u8]) {
        if self.echo.is_none() {
            return;
        }
        for &octet in tx {
            let at = (self.receiver_sent % ECHO_HISTORY as u64) as usize;
            self.echo_line[at] = self.to.decode(octet);
            self.receiver_sent += 1;
        }
    }

    /// Gives the octets the receiving end receives.
    pub(crate) fn receive(&mut self, rx: &mut [u8]) {
        for octet in rx {
            let mut sample = self.next_sample();
            if let Some((delay, factor)) = self.echo {
                // The sample this end sent `delay` samples before the one
                // it is receiving now. It has sent as many as it has
                // received, up to the tick it is in.
                let wanted = self.stats.received.checked_sub(delay as u64);
                if let Some(index) = wanted
                    && index < self.receiver_sent
                    && self.receiver_sent - index <= ECHO_HISTORY as u64
                {
                    sample += factor * self.echo_line[(index % ECHO_HISTORY as u64) as usize];
                }
            }
            self.stats.received += 1;
            *octet = self.to.encode(sample);

            if let Some(packets) = &self.packets {
                if self.in_packet == 0 {
                    let number = self.stats.packets;
                    self.packet_lost = packets.lost.contains(&number)
                        || (packets.loss_probability > 0.0
                            && self.rng.chance(packets.loss_probability));
                    self.stats.packets += 1;
                    if self.packet_lost {
                        self.stats.packets_lost += 1;
                    }
                }
                if self.packet_lost {
                    // What `voip/rtp` fills a lost packet with.
                    *octet = self.to.silence();
                }
                self.in_packet += 1;
                if self.in_packet == self.packet_samples {
                    self.in_packet = 0;
                }
            }
        }
        self.stats.queued = self.queue.len();
    }

    /// The next sample out of the delay, with the slips made.
    fn next_sample(&mut self) -> f32 {
        if let Some(slips) = self.slips {
            let due = (self.stats.received + 1).is_multiple_of(u64::from(slips.every_samples));
            if due {
                self.stats.slips += 1;
                match slips.kind {
                    SlipKind::Drop => {
                        self.queue.pop_front();
                    }
                    SlipKind::Repeat => return self.last_sample,
                }
            }
        }
        match self.queue.pop_front() {
            Some(sample) => {
                self.last_sample = sample;
                sample
            }
            None => {
                self.stats.underruns += 1;
                0.0
            }
        }
    }

    pub(crate) fn stats(&self) -> PathStats {
        PathStats {
            queued: self.queue.len(),
            ..self.stats
        }
    }
}

fn add_noise(rng: &mut Rng, sigma: Option<f64>, sample: f32) -> f32 {
    match sigma {
        Some(sigma) => sample + (sigma * rng.gaussian()) as f32,
        // Nothing is added, not even a zero: a negative zero is the
        // u-law's second octet of silence and must stay one.
        None => sample,
    }
}
