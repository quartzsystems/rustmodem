// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A channel simulator: what a telephone connection does to a modem's
//! signal, for tests.
//!
//! A [`Channel`] joins two ends, A and B. Each end gives the G.711 octets
//! it sends and takes the octets it receives, exactly as an engine does on
//! a media thread, and between the two the channel can do any of the
//! things in [`Impairments`], each switched on by itself. With none on,
//! and the same law at both ends, every octet arrives as it was sent.
//!
//! Everything is deterministic: a [`ChannelConfig`] and the octets sent
//! give the same octets received on every run and every machine. What is
//! random, the noise and the packets lost by chance, comes from the seed.
//!
//! The order of a path, from the end that sends to the end that receives:
//!
//! 1. The octets are decoded in the sender's law.
//! 2. A digital pad: attenuation, and coding again in the same law.
//! 3. Gain or loss.
//! 4. The loop filter: the voice band, and a slope across it.
//! 5. A second pass through a codec, in either law.
//! 6. A frequency offset.
//! 7. The clock offset between the two ends: resampling.
//! 8. Noise.
//! 9. The delay of the path, and sample slips.
//! 10. The receiver's own echo.
//! 11. Coding in the receiver's law.
//! 12. Packets: a lost one is filled with the idle octet, as `voip/rtp`
//!     fills it.
//!
//! [`Duplex`] runs two modem engines against each other through a channel.
//!
//! Not modelled: robbed-bit signalling (`BLOCKED(phase8)`: it matters to
//! the PCM modems only), and a loop good enough for line probing, with
//! group delay and nonlinear distortion (`BLOCKED(phase7)`: V.34).

#![forbid(unsafe_code)]

mod duplex;
mod path;
mod rng;

pub use duplex::{Duplex, End, Timed};
pub use path::PathStats;
pub use rng::Rng;

use g711::Law;
use path::Path;

/// Additive white noise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Noise {
    /// Noise at this level in dBm0, across the whole 4 kHz band.
    Level(f32),
    /// Noise this many dB below a signal whose level is stated.
    Snr {
        /// The level of the signal, in dBm0, as it reaches the noise.
        signal_dbm0: f32,
        /// The signal-to-noise ratio in dB.
        snr_db: f32,
    },
}

impl Noise {
    /// The level of the noise in dBm0.
    pub fn level_dbm0(self) -> f32 {
        match self {
            Noise::Level(level) => level,
            Noise::Snr {
                signal_dbm0,
                snr_db,
            } => signal_dbm0 - snr_db,
        }
    }
}

/// The receiver's own transmission coming back to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Echo {
    /// How long after it was sent the echo arrives, in samples. It is held
    /// to at least one tick: what an end sends in a tick cannot reach it
    /// in the same tick.
    pub delay_samples: u32,
    /// How far below the transmission the echo is, in dB.
    pub loss_db: f32,
}

/// One sample dropped or one repeated, at intervals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slips {
    /// Samples between two slips.
    pub every_samples: u32,
    /// Whether a sample is dropped or repeated.
    pub kind: SlipKind,
}

/// What a slip does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlipKind {
    /// A sample is left out: the stream is one sample early from there.
    Drop,
    /// A sample is given twice: the stream is one sample late from there.
    Repeat,
}

/// The loss of whole packets.
#[derive(Clone, Debug, PartialEq)]
pub struct Packets {
    /// The packet time in milliseconds.
    pub ptime_ms: u32,
    /// The packets that are lost, numbered from 0 as the receiver takes
    /// them.
    pub lost: Vec<u64>,
    /// The chance that any other packet is lost, from 0 to 1.
    pub loss_probability: f64,
}

/// What a path does to a signal. The default does nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Impairments {
    /// A fixed digital pad in dB: the signal is attenuated and coded again
    /// in the sender's law. 0 is no pad.
    pub pad_db: f32,
    /// Gain in dB, negative for loss. 0 is neither.
    pub gain_db: f32,
    /// The loop filter: the band is limited to the voice band, about
    /// 180 Hz to 3500 Hz, and the response slopes down across it by this
    /// many dB from 1000 Hz to 3000 Hz. The gain at 1000 Hz is one. The
    /// slope may be from 0 to 7 dB: it is made by one pole, and at this
    /// sample rate one pole cannot make more than 7.6 dB.
    pub loop_slope_db: Option<f32>,
    /// A second pass through a codec, in this law.
    pub second_codec: Option<Law>,
    /// A frequency offset in hertz: every frequency moves by this much. 0
    /// is none.
    pub frequency_offset_hz: f32,
    /// Noise.
    pub noise: Option<Noise>,
    /// The receiver's own echo.
    pub echo: Option<Echo>,
    /// Sample slips.
    pub slips: Option<Slips>,
    /// Packet loss.
    pub packets: Option<Packets>,
}

impl Default for Impairments {
    fn default() -> Self {
        Impairments {
            pad_db: 0.0,
            gain_db: 0.0,
            loop_slope_db: None,
            second_codec: None,
            frequency_offset_hz: 0.0,
            noise: None,
            echo: None,
            slips: None,
            packets: None,
        }
    }
}

/// A channel's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelConfig {
    /// The seed of everything random.
    pub seed: u64,
    /// The law of end A's octets.
    pub law_a: Law,
    /// The law of end B's octets.
    pub law_b: Law,
    /// The delay of each path in samples. A [`Duplex`] needs one tick of
    /// it at the least, which is the default: what one end sends in a tick
    /// the other receives in its next.
    pub delay_samples: u32,
    /// How fast end A's sample clock is against end B's, in parts in a
    /// million. With an offset the two ends must be run on their own
    /// clocks, as [`Duplex`] runs them: one end then ticks more often than
    /// the other.
    pub clock_offset_ppm: f64,
    /// What happens to A's signal on its way to B.
    pub a_to_b: Impairments,
    /// What happens to B's signal on its way to A.
    pub b_to_a: Impairments,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        ChannelConfig {
            seed: 0,
            law_a: Law::Ulaw,
            law_b: Law::Ulaw,
            delay_samples: 80,
            clock_offset_ppm: 0.0,
            a_to_b: Impairments::default(),
            b_to_a: Impairments::default(),
        }
    }
}

impl ChannelConfig {
    /// The same impairments both ways.
    pub fn symmetric(impairments: Impairments) -> ChannelConfig {
        ChannelConfig {
            a_to_b: impairments.clone(),
            b_to_a: impairments,
            ..ChannelConfig::default()
        }
    }
}

/// One of the two ends of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// End A.
    A,
    /// End B.
    B,
}

/// A channel between two ends.
#[derive(Clone, Debug)]
pub struct Channel {
    a_to_b: Path,
    b_to_a: Path,
    clock_offset_ppm: f64,
}

impl Channel {
    /// A channel with these settings and nothing in it yet but the silence
    /// of its delay.
    pub fn new(config: &ChannelConfig) -> Channel {
        // A is fast by the offset, so there are fewer of B's samples than
        // of A's in any stretch of time.
        let a_per_b = 1.0 + config.clock_offset_ppm * 1.0e-6;
        let resample = config.clock_offset_ppm != 0.0;
        Channel {
            a_to_b: Path::new(
                &config.a_to_b,
                config.law_a,
                config.law_b,
                config.delay_samples,
                resample.then_some(1.0 / a_per_b),
                config.seed,
            ),
            b_to_a: Path::new(
                &config.b_to_a,
                config.law_b,
                config.law_a,
                config.delay_samples,
                resample.then_some(a_per_b),
                config.seed ^ 0x5DEE_CE66_D1CE_4E5B,
            ),
            clock_offset_ppm: config.clock_offset_ppm,
        }
    }

    /// Takes the octets an end sends.
    pub fn send(&mut self, from: Side, tx: &[u8]) {
        match from {
            Side::A => {
                self.a_to_b.send(tx);
                self.b_to_a.note_receiver_sent(tx);
            }
            Side::B => {
                self.b_to_a.send(tx);
                self.a_to_b.note_receiver_sent(tx);
            }
        }
    }

    /// Gives the octets an end receives. If the other end has not sent
    /// enough, the rest is silence, and is counted as an underrun.
    pub fn receive(&mut self, at: Side, rx: &mut [u8]) {
        match at {
            Side::A => self.b_to_a.receive(rx),
            Side::B => self.a_to_b.receive(rx),
        }
    }

    /// One tick of both ends on one clock: each receives, and then each
    /// sends, as two engines ticked together do.
    pub fn tick(&mut self, a_tx: &[u8], b_tx: &[u8], a_rx: &mut [u8], b_rx: &mut [u8]) {
        self.receive(Side::A, a_rx);
        self.receive(Side::B, b_rx);
        self.send(Side::A, a_tx);
        self.send(Side::B, b_tx);
    }

    /// The counts of the path towards an end.
    pub fn stats(&self, towards: Side) -> PathStats {
        match towards {
            Side::A => self.b_to_a.stats(),
            Side::B => self.a_to_b.stats(),
        }
    }

    /// How fast end A's clock is against end B's, in parts in a million.
    pub fn clock_offset_ppm(&self) -> f64 {
        self.clock_offset_ppm
    }
}
