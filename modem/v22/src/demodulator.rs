// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The demodulator: a line signal to bits.
//!
//! # The chain
//!
//! 1. **To baseband.** The line is multiplied by the far end's carrier in
//!    both phases.
//! 2. **The matched filter.** The square root of a raised cosine, twelve
//!    symbols long and windowed. It is also the filter that separates the
//!    channels: this end's own transmitter is on the same line and may
//!    come back as an echo louder than the far end, and the filter has it
//!    77 dB down, with the copy of the signal that the first step leaves
//!    at twice the carrier.
//! 3. **Level.** The power after the filter is the far end's and nothing
//!    else's. The line signal detector reads it (clause 3.3), and a gain
//!    brings the signal to the size the decisions want.
//! 4. **Timing.** Gardner's detector and an interpolator give two samples
//!    a symbol on the far end's clock.
//! 5. **The equaliser.** Fractionally spaced, sixteen taps: eight symbols.
//! 6. **The carrier loop** turns the equaliser's output by the phase it
//!    has found, and
//! 7. **the slicer** decides the point: of four at 1200 bit/s, of sixteen
//!    at 2400.
//! 8. **Decoding.** The quadrant the point is in against the last one is
//!    the first two bits (Table 1) and the point in the quadrant the last
//!    two (Figure 2); the bits are descrambled (clause 5.2).
//!
//! The handoff has a band-pass filter and the gain control before the
//! demodulation. Here the matched filter does the band-pass filter's
//! work, at baseband, and the gain follows it, since a gain that came
//! before the filter would be set by the echo.
//!
//! # Finding the signal
//!
//! Nothing is trained on a known sequence: V.22bis has none. The modem
//! begins at 1200 bit/s, where a decision needs only the quadrant, and
//! every loop is directed by decisions from the start. When the carrier
//! comes the gain settles first, then the timing and the carrier loop
//! with a wide bandwidth, then the equaliser begins to adapt and the
//! loops narrow. By the time the far end changes to sixteen points the
//! equaliser has the signal at the right size for them.
//!
//! When the carrier goes, everything is held as it is, so that a signal
//! that comes back is taken up where it was left (clause 6.5).

use dspkit::carrier::{CarrierLoop, CarrierSettings};
use dspkit::equalizer::Equalizer;
use dspkit::fir::Window;
use dspkit::level::{CarrierDetect, CarrierThresholds, power_to_dbm0};
use dspkit::passband;
use dspkit::quality::SignalQuality;
use dspkit::scramble::Descrambler;
use dspkit::shape::{ComplexFir, matched_taps};
use dspkit::slicer::{Slicer4, Slicer16};
use dspkit::timing::{GARDNER_GAIN_75, TimingRecovery, TimingSettings};
use dspkit::{Complex, SAMPLE_RATE};
use g711::Law;

use crate::{
    Band, CARRIER_OFF_DBM0, ELEMENT_1200, PULSE_BETA, ROLLOFF, Rate, SAMPLES_PER_SYMBOL, SCRAMBLER,
    SCRAMBLER_GUARD_ONES, SYMBOL_RATE, UNIT, dibit_of_change, point, quadrant_and_element,
};

/// The taps of the matched filter: twelve symbols.
const MATCHED_TAPS: usize = 161;

/// The taps of the equaliser, half a symbol apart.
const EQUALIZER_TAPS: usize = 16;

/// The part of the far end's power that is left after the matched
/// filter: a signal whose spectrum is the root of a raised cosine,
/// through the same again, keeps 1 less a quarter of the roll-off.
const MATCHED_POWER: f32 = 1.0 - ROLLOFF as f32 / 4.0;

/// The level at which the detector comes on, in dBm0. Clause 3.3 has it
/// on above -43 dBm and off below -48 dBm, leaves what is between to the
/// modem, and asks for 2 dB of hysteresis. A signal at -43 dBm exactly
/// is to be received, and its power from one symbol to the next is not
/// steady, so the detector comes on halfway: 2.5 dB over where it goes
/// off.
const DETECT_ON_DBM0: f32 = -45.5;

/// The time constant of the power the line signal detector reads, in
/// seconds. Short, so that the reading falls as fast from a loud signal
/// as from a quiet one, near enough: the detector's time to go off has a
/// window of 25 ms (clause 3.2).
const DETECT_TIME_S: f32 = 0.0015;

/// The time constant of the power that says whether the signal has
/// faded, in seconds: three symbols, so that a few small points in a row
/// are not a fade.
const FADE_TIME_S: f32 = 0.005;

/// How long a signal may stay that far from its usual power before it is
/// taken for the signal's new level, in samples: 100 ms. And for how long
/// the gain then goes to it before anything is learned again: 200 ms,
/// five of the gain's time constants.
const FAR_SAMPLES: u32 = 800;
const REGAIN_SAMPLES: u32 = 1600;

/// How long the power must be over the threshold before the detector
/// comes on, and under it before it goes off, in seconds. With the time
/// the power takes to rise and fall through the filters, the detector
/// comes on 40 to 205 ms after a signal comes and goes off 40 to 65 ms
/// after it has gone (clause 3.2). The time to come on is also what
/// keeps the detector off through the step of a tone in the other
/// channel, which is in this one for the 10 ms the filter takes to
/// forget it.
const DETECT_ON_S: f32 = 0.030;
const DETECT_OFF_S: f32 = 0.036;

/// The time constants of the power the gain is set from: when the
/// carrier has just come, while the signal is being found, and after.
const GAIN_FAST_S: f32 = 0.005;
const GAIN_MEDIUM_S: f32 = 0.040;
const GAIN_SLOW_S: f32 = 1.0;

/// How far from its usual power the signal may be, under or over,
/// before the loops and the equaliser stop learning from it, as a part
/// of that power: 10 dB. The line signal detector takes 40 ms and more
/// to say that a signal has gone, and what is decided meanwhile is noise.
/// A change of less than this is the equaliser's to follow, and if it
/// cannot, the modem's to retrain on.
const FADE: f32 = 0.1;

/// The power in the equaliser's delay line under which neither it nor the
/// carrier loop learns, as a part of what a signal at its usual size puts
/// there. The delay line is eight symbols, and a signal that goes, as for
/// a lost packet, has it under this in four: sooner than the fade is
/// seen, and before what is in the line is small enough for the step to
/// make much of. Sixteen points at their usual size are under it once in
/// some hundreds of symbols, when six of eight are the small ones.
const EQUALIZER_FLOOR: f32 = 0.5;

/// The symbols after the carrier comes in which the gain is still
/// settling and nothing is decided.
const SETTLE_SYMBOLS: u32 = 12;

/// The symbols after the carrier comes before the equaliser adapts: the
/// timing is found first.
const EQUALIZER_AFTER_SYMBOLS: u32 = 60;

/// The symbols after the carrier comes when the loops narrow and the
/// equaliser's step becomes the one it keeps.
const TRACK_AFTER_SYMBOLS: u32 = 240;

/// The equaliser's step while the signal is being found and after, and
/// its leak.
const EQUALIZER_STEP_FAST: f32 = 0.05;
const EQUALIZER_STEP_SLOW: f32 = 0.01;
const EQUALIZER_LEAK: f32 = 1.0e-6;

/// The noise bandwidth of the timing loop as a part of the symbol rate,
/// while the signal is being found and after: 12 Hz and 1.8 Hz.
const TIMING_BAND_FAST: f32 = 0.02;
const TIMING_BAND_SLOW: f32 = 0.003;

/// The noise bandwidth of the carrier loop likewise: 18 Hz and 9 Hz.
const CARRIER_BAND_FAST: f32 = 0.03;
const CARRIER_BAND_SLOW: f32 = 0.015;

/// The most the carrier loop takes the frequency offset to be, in turns
/// a symbol: 15 Hz, twice the 7 Hz of clause 2.6. An unscrambled signal
/// is a tone 150 Hz from the carrier, and the loop is not to take that
/// for an offset.
const CARRIER_MAX_TURNS: f32 = 15.0 / SYMBOL_RATE as f32;

/// The time constant of the signal quality, in symbols.
const QUALITY_SYMBOLS: f32 = 128.0;

/// One symbol as the demodulator decided it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Symbol {
    /// The received point after the equaliser and the carrier loop, in
    /// units of the constellation's grid: the points are at 1 and 3 each
    /// way.
    pub point: Complex,
    /// How many bits the symbol carries: two or four.
    pub bits: u8,
    /// The bits as they were on the line, the first received in the
    /// highest of the `bits` low bits.
    pub line: u8,
    /// The same bits descrambled.
    pub data: u8,
}

impl Symbol {
    /// The descrambled bits, the first received first.
    pub fn data_bits(self) -> impl Iterator<Item = bool> {
        (0..self.bits).rev().map(move |n| (self.data >> n) & 1 == 1)
    }

    /// The bits as they were on the line, the first received first.
    pub fn line_bits(self) -> impl Iterator<Item = bool> {
        (0..self.bits).rev().map(move |n| (self.line >> n) & 1 == 1)
    }
}

/// What the demodulator has after a sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Demodulated {
    /// Whether the line signal detector is on.
    pub carrier: bool,
    /// The symbol that was decided at this sample, if one was.
    pub symbol: Option<Symbol>,
}

/// The demodulator of one channel.
#[derive(Clone, Debug)]
pub struct Demodulator {
    law: Law,
    mixer: passband::Demodulator,
    matched: ComplexFir<MATCHED_TAPS>,
    // The power after the matched filter, smoothed for the detector and
    // for the gain.
    detect_power: f32,
    detect_coefficient: f32,
    fade_power: f32,
    fade_coefficient: f32,
    // For how many samples the signal has been far from its usual
    // power, and for how many more the gain goes to its new one.
    far_run: u32,
    regain: u32,
    gain_power: f32,
    gain_fast: f32,
    gain_medium: f32,
    gain_slow: f32,
    detector: CarrierDetect,
    timing: TimingRecovery,
    equalizer: Equalizer<EQUALIZER_TAPS>,
    carrier: CarrierLoop,
    four: Slicer4,
    sixteen: Slicer16,
    descrambler: Descrambler,
    quality: SignalQuality,
    rate: Rate,
    // The quadrant of the last decision, 0 to 3.
    quadrant: u8,
    // Symbols since the carrier came, held at the last stage.
    symbols: u32,
    // Whether the signal has been found once: a carrier that comes back
    // is then taken up where it was left.
    found: bool,
    // Whether a carrier that comes back is taken up where it was left.
    resuming: bool,
    // The matched filter's last output.
    baseband: Complex,
    adapting: bool,
    // Whether nothing is being learned because the signal is far from
    // its usual power, and whether the timing loop has been told to hold.
    held: bool,
    frozen: bool,
}

fn coefficient(time_s: f32) -> f32 {
    1.0 / (time_s * SAMPLE_RATE as f32).max(1.0)
}

fn timing_settings(band: f32) -> TimingSettings {
    let mut settings = TimingSettings::with_bandwidth(band, 1.0, GARDNER_GAIN_75);
    // Twice what V.22 allows the two clocks between them, and more.
    settings.max_offset = 1.0e-3;
    settings
}

fn carrier_settings(band: f32) -> CarrierSettings {
    let mut settings = CarrierSettings::with_bandwidth(band, 0.707);
    settings.max_frequency = CARRIER_MAX_TURNS;
    settings
}

impl Demodulator {
    /// A demodulator for the channel the far end sends in. It begins at
    /// 1200 bit/s.
    pub fn new(band: Band, law: Law) -> Demodulator {
        let mut equalizer = Equalizer::new(0.0, EQUALIZER_LEAK);
        equalizer.set_power_floor(EQUALIZER_FLOOR * MATCHED_POWER * EQUALIZER_TAPS as f32);
        Demodulator {
            law,
            mixer: passband::Demodulator::new(band.carrier_hz()),
            matched: ComplexFir::new(matched_taps::<MATCHED_TAPS>(
                ROLLOFF,
                SAMPLES_PER_SYMBOL,
                Window::Kaiser(PULSE_BETA),
            )),
            detect_power: 0.0,
            detect_coefficient: coefficient(DETECT_TIME_S),
            fade_power: 0.0,
            fade_coefficient: coefficient(FADE_TIME_S),
            far_run: 0,
            regain: 0,
            gain_power: 0.0,
            gain_fast: coefficient(GAIN_FAST_S),
            gain_medium: coefficient(GAIN_MEDIUM_S),
            gain_slow: coefficient(GAIN_SLOW_S),
            detector: CarrierDetect::new(
                CarrierThresholds {
                    // The thresholds are of the signal on the line; what
                    // the detector reads is after the matched filter.
                    on_dbm0: DETECT_ON_DBM0 + matched_loss_db(),
                    off_dbm0: CARRIER_OFF_DBM0 + matched_loss_db(),
                    on_time_s: DETECT_ON_S,
                    off_time_s: DETECT_OFF_S,
                },
                law,
            ),
            timing: TimingRecovery::new(SAMPLES_PER_SYMBOL, timing_settings(TIMING_BAND_FAST)),
            equalizer,
            carrier: CarrierLoop::new(carrier_settings(CARRIER_BAND_FAST)),
            four: Slicer4::new(point(0, ELEMENT_1200).scale(UNIT)),
            sixteen: Slicer16::new(UNIT),
            descrambler: Descrambler::with_guard(SCRAMBLER, SCRAMBLER_GUARD_ONES),
            quality: SignalQuality::new(QUALITY_SYMBOLS),
            rate: Rate::B1200,
            quadrant: 0,
            symbols: 0,
            found: false,
            resuming: true,
            baseband: Complex::ZERO,
            adapting: true,
            held: false,
            frozen: false,
        }
    }

    /// The rate the symbols are decided and decoded at, from the next
    /// symbol. The four points of 1200 bit/s are four of the sixteen, so
    /// a receiver may decide sixteen ways before the far end sends them
    /// (clause 6.3.1.1.1 d).
    pub fn set_rate(&mut self, rate: Rate) {
        self.rate = rate;
    }

    /// The rate.
    pub fn rate(&self) -> Rate {
        self.rate
    }

    /// Whether the equaliser and the loops learn from the signal. Off,
    /// they are held as they are while symbols are still decided: for a
    /// signal that is known not to be worth learning from.
    pub fn set_adapting(&mut self, adapting: bool) {
        self.adapting = adapting;
    }

    /// Whether a carrier that goes and comes back is taken up where it
    /// was left (clause 6.5), which is so unless this says otherwise.
    /// Off, every carrier that comes is found from nothing: for a
    /// handshake, where what came before the far modem's signal was not
    /// the far modem.
    pub fn set_resuming(&mut self, resuming: bool) {
        self.resuming = resuming;
    }

    /// The matched filter's output at the last sample: the far end's
    /// channel at baseband, with nothing done to it but the filtering.
    /// The power of a signal here is its power on the line, less what
    /// the filter takes of it.
    pub fn baseband(&self) -> Complex {
        self.baseband
    }

    /// Forgets the signal: the next carrier is found from nothing, as
    /// the first was. The rate and the descrambler are left as they are.
    pub fn retrain(&mut self) {
        self.found = self.detector.present();
        self.acquire();
    }

    /// Everything as it was made, and no carrier.
    pub fn reset(&mut self) {
        self.matched.reset();
        self.detect_power = 0.0;
        self.fade_power = 0.0;
        self.far_run = 0;
        self.regain = 0;
        self.held = false;
        self.gain_power = 0.0;
        self.detector.reset();
        self.descrambler.reset();
        self.rate = Rate::B1200;
        self.quadrant = 0;
        self.found = false;
        self.resuming = true;
        self.baseband = Complex::ZERO;
        self.adapting = true;
        self.acquire();
    }

    /// Begins to find the signal.
    fn acquire(&mut self) {
        self.timing.reset();
        self.frozen = false;
        self.timing.set_settings(timing_settings(TIMING_BAND_FAST));
        self.equalizer.reset();
        self.equalizer.set_step(0.0);
        self.carrier.reset();
        self.carrier
            .set_settings(carrier_settings(CARRIER_BAND_FAST));
        self.quality.reset();
        self.symbols = 0;
        self.gain_power = self.fade_power;
    }

    /// The level of the far end's signal on the line, in dBm0: while
    /// there is a carrier, the level the gain is set from, which is
    /// smoothed over a second once the signal has been found.
    pub fn level_dbm0(&self) -> f32 {
        let power = if self.detector.present() {
            self.gain_power
        } else {
            self.detect_power
        };
        power_to_dbm0(power.max(1.0e-12), self.law) - matched_loss_db()
    }

    /// The signal-to-noise ratio of the decisions, in decibels.
    pub fn snr_db(&self) -> f32 {
        self.quality.snr_db()
    }

    /// The mean-squared error of the decisions, against a constellation
    /// of mean power one.
    pub fn mse(&self) -> f32 {
        self.quality.mse()
    }

    /// The far end's symbol clock against ours, in parts in a million.
    pub fn timing_offset_ppm(&self) -> f32 {
        self.timing.offset_ppm()
    }

    /// The far end's carrier against ours, in hertz.
    pub fn carrier_offset_hz(&self) -> f32 {
        self.carrier.frequency_hz(SYMBOL_RATE as f32)
    }

    /// Whether the line signal detector is on.
    pub fn carrier(&self) -> bool {
        self.detector.present()
    }

    /// Whether the signal has been found and the loops have narrowed.
    pub fn tracking(&self) -> bool {
        self.detector.present() && self.symbols >= TRACK_AFTER_SYMBOLS
    }

    /// Takes one sample.
    #[inline]
    pub fn process(&mut self, sample: f32) -> Demodulated {
        let sample = if sample.is_finite() { sample } else { 0.0 };
        let baseband = self.matched.process(self.mixer.process(sample));
        let power = baseband.norm_sqr();
        self.detect_power += (power - self.detect_power) * self.detect_coefficient;
        self.fade_power += (power - self.fade_power) * self.fade_coefficient;

        let was = self.detector.present();
        let carrier = self.detector.process(self.detect_power);
        self.baseband = baseband;
        if carrier && !was && !(self.found && self.resuming) {
            self.acquire();
            self.found = true;
        }
        // A signal far from its usual power, either way, is going, gone
        // or changed, and nothing is learned from it.
        let far =
            self.fade_power < FADE * self.gain_power || self.fade_power * FADE > self.gain_power;
        if !carrier {
            self.far_run = 0;
            self.regain = 0;
        } else if self.regain > 0 {
            // The gain goes to the level the signal has now.
            self.regain -= 1;
            self.gain_power += (power - self.gain_power) * self.gain_medium;
        } else if far {
            self.far_run += 1;
            if self.far_run > FAR_SAMPLES {
                // Still a carrier, and this far off for this long: it is
                // the signal's level.
                self.far_run = 0;
                self.regain = REGAIN_SAMPLES;
            }
        } else {
            self.far_run = 0;
            let rate = if self.symbols <= SETTLE_SYMBOLS {
                self.gain_fast
            } else if self.symbols < TRACK_AFTER_SYMBOLS {
                self.gain_medium
            } else {
                self.gain_slow
            };
            self.gain_power += (power - self.gain_power) * rate;
        }
        self.held = far || self.regain > 0;
        let frozen = !carrier || !self.adapting || self.held;
        if frozen != self.frozen {
            self.frozen = frozen;
            self.timing.set_frozen(frozen);
        }
        // The gain that makes the power at the symbol instants one.
        let gain = if self.gain_power > 1.0e-12 {
            (MATCHED_POWER / self.gain_power).sqrt()
        } else {
            0.0
        };

        let mut out = Demodulated {
            carrier,
            symbol: None,
        };
        let Some(strobe) = self.timing.process(baseband.scale(gain)) else {
            return out;
        };
        self.equalizer.push(strobe.sample);
        if !strobe.on_symbol {
            return out;
        }
        if !carrier {
            // The phase runs on at the frequency found, for a signal that
            // comes back.
            self.carrier.coast();
            return out;
        }
        self.stage();
        if self.symbols <= SETTLE_SYMBOLS {
            return out;
        }
        out.symbol = Some(self.decide());
        out
    }

    /// Counts a symbol since the carrier came, and at the stages changes
    /// the loops.
    #[inline]
    fn stage(&mut self) {
        if self.symbols > TRACK_AFTER_SYMBOLS {
            return;
        }
        self.symbols += 1;
        if self.symbols == EQUALIZER_AFTER_SYMBOLS {
            self.equalizer.set_step(EQUALIZER_STEP_FAST);
        } else if self.symbols == TRACK_AFTER_SYMBOLS {
            self.equalizer.set_step(EQUALIZER_STEP_SLOW);
            self.timing.set_settings(timing_settings(TIMING_BAND_SLOW));
            self.carrier
                .set_settings(carrier_settings(CARRIER_BAND_SLOW));
        }
    }

    /// Decides the symbol the equaliser has now.
    #[inline]
    fn decide(&mut self) -> Symbol {
        let equalized = self.equalizer.output();
        let received = self.carrier.derotate(equalized);
        let decision = match self.rate {
            Rate::B1200 => self.four.slice(received).point,
            Rate::B2400 => self.sixteen.slice(received).point,
        };
        let error = received - decision;
        if self.adapting && !self.held && self.equalizer.adaptable() {
            // The error where the equaliser works, which is before the
            // carrier loop's turn.
            self.equalizer.update(self.carrier.rotate(error));
            self.carrier
                .update(CarrierLoop::phase_error(received, decision, 1.0));
        } else {
            self.equalizer.observe(error);
            self.carrier.coast();
        }
        self.quality.update(decision, error);

        let (quadrant, element) = quadrant_and_element(decision.scale(1.0 / UNIT));
        let dibit = dibit_of_change(quadrant.wrapping_sub(self.quadrant));
        self.quadrant = quadrant;
        let (bits, line) = match self.rate {
            Rate::B1200 => (2, dibit),
            Rate::B2400 => (4, (dibit << 2) | element),
        };
        let mut data = 0;
        for n in (0..bits).rev() {
            data = (data << 1) | u8::from(self.descrambler.bit((line >> n) & 1 == 1));
        }
        Symbol {
            point: received.scale(1.0 / UNIT),
            bits,
            line,
            data,
        }
    }
}

/// What the matched filter takes from the far end's power, in decibels:
/// a negative number.
fn matched_loss_db() -> f32 {
    // 10 log10 of 1 - 0.75 / 4.
    -0.901_766_3
}
