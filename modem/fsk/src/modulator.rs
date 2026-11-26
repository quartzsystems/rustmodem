// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modulator: bits to a line signal.

use dspkit::SAMPLE_RATE;
use dspkit::fir::{self, Fir, Window};
use dspkit::level::dbm0_to_amplitude;
use dspkit::osc::{Oscillator, phase_step};
use g711::Law;

use crate::{BAND_BETA, BAND_MARGIN_HZ, BAND_TAPS, BIT_RATE, Tones};

/// How long the transmit filter delays the signal, in samples: 8 ms.
pub const TRANSMIT_DELAY_SAMPLES: u32 = (BAND_TAPS as u32 - 1) / 2;

/// A frequency-shift keying modulator with a continuous phase.
///
/// One oscillator makes the signal, and a bit changes its frequency and
/// nothing else, so the signal has no step where the bit changes.
///
/// A bit is 26 and two thirds samples long. The modulator counts in 300ths
/// of a sample, so its bits are of 26 and 27 samples in turn and exactly
/// 300 of them fill 8000 samples: its bit rate is its sample clock's, to
/// the last bit.
///
/// Keying spreads the signal beyond its two frequencies, and what it
/// spreads into the other band the modem's own receiver would hear, as
/// loud as its echo is. So the signal leaves through a band-pass filter,
/// the same one the far end's demodulator has, which takes 60 dB off
/// everything in the other band. It delays the signal by 8 ms.
#[derive(Clone, Debug)]
pub struct Modulator {
    band: Fir<BAND_TAPS>,
    oscillator: Oscillator,
    mark_step: u32,
    space_step: u32,
    amplitude: f32,
    // The time into the bit, in units of which SAMPLE_RATE make a bit.
    into_bit: u32,
    bit: bool,
}

impl Modulator {
    /// A modulator for a band's two frequencies, at a level in dBm0. It
    /// begins at mark and asks for its first bit with its first sample.
    pub fn new(tones: Tones, level_dbm0: f32, law: Law) -> Modulator {
        let mark_step = phase_step(tones.mark_hz);
        let mut oscillator = Oscillator::new(tones.mark_hz);
        oscillator.set_step(mark_step);
        let low = tones.mark_hz.min(tones.space_hz) - BAND_MARGIN_HZ;
        let high = tones.mark_hz.max(tones.space_hz) + BAND_MARGIN_HZ;
        Modulator {
            band: Fir::new(fir::bandpass(low, high, Window::Kaiser(BAND_BETA))),
            oscillator,
            mark_step,
            space_step: phase_step(tones.space_hz),
            amplitude: dbm0_to_amplitude(level_dbm0, law),
            into_bit: SAMPLE_RATE,
            bit: true,
        }
    }

    /// The next sample. When a bit ends, `next_bit` is asked for the
    /// next one: `true` is mark, binary 1.
    #[inline]
    pub fn next_sample(&mut self, next_bit: impl FnOnce() -> bool) -> f32 {
        if self.into_bit >= SAMPLE_RATE {
            self.into_bit -= SAMPLE_RATE;
            self.bit = next_bit();
            self.oscillator.set_step(if self.bit {
                self.mark_step
            } else {
                self.space_step
            });
        }
        self.into_bit += BIT_RATE;
        self.band
            .process(self.amplitude * self.oscillator.next_sine())
    }

    /// A sample with nothing new going in: what is left in the transmit
    /// filter of the bits already sent. The filter delays the signal by
    /// [`TRANSMIT_DELAY_SAMPLES`], so a modem that is to stop sending
    /// calls this, and not `next_sample`, for twice that many samples
    /// after its last bit has ended. Until it has, the end of its signal
    /// has not gone out.
    #[inline]
    pub fn flush_sample(&mut self) -> f32 {
        self.band.process(0.0)
    }

    /// The bit being sent.
    pub fn bit(&self) -> bool {
        self.bit
    }

    /// Starts again at mark, at the beginning of a bit. The phase runs
    /// on and the filter keeps what is in it, so a modulator that is
    /// started again makes no click.
    pub fn restart(&mut self) {
        self.into_bit = SAMPLE_RATE;
        self.bit = true;
        self.oscillator.set_step(self.mark_step);
    }
}
