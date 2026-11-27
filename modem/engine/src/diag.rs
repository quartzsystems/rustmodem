// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What an engine shows of itself while a call is up: a small snapshot of
//! fixed size that the media thread can take every tick without
//! allocating, and that the pool hands to whoever is watching the line.
//!
//! The snapshot is the same shape for every modulation. The 300 bit/s
//! modems fill the scope, which is a run of signed samples of their
//! demodulator's output; the modems at 1200 and 2400 bit/s fill the
//! constellation and the equaliser's figures, and what their data link
//! counts.

use crate::link::LinkStats;

/// The samples of the scope a snapshot carries.
pub const SCOPE_SAMPLES: usize = 64;

/// The state changes a snapshot remembers.
pub const STATE_HISTORY: usize = 8;

/// The received points a snapshot carries: a little over a tenth of a
/// second of them at 600 baud.
pub const CONSTELLATION_POINTS: usize = 64;

/// What a unit of the constellation's grid is in a point of a snapshot.
/// The points of V.22bis are at 1 and 3 units each way (Figure 2/V.22
/// bis), which is 30 and 90; a point further out than 127 is held there.
pub const CONSTELLATION_UNIT: i8 = 30;

/// What an engine is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EngineState {
    /// Stopped.
    #[default]
    Idle = 0,
    /// Answering: the silence before the answer tone.
    AnswerSilence = 1,
    /// Answering: sending answer tone, and with V.8 listening for CM.
    AnswerTone = 2,
    /// Answering with V.8: CM received, sending JM.
    JointMenu = 3,
    /// The silence between the V.8 signals, or the answer tone, and what
    /// follows.
    Gap = 4,
    /// Answering without V.8, or after it came to nothing: sending a
    /// carrier and listening for the caller's.
    Fallback = 5,
    /// Originating: listening for answer tone or a carrier.
    Listening = 6,
    /// Originating with V.8: ANSam detected, silent before CM.
    CallSilence = 7,
    /// Originating with V.8: sending CM, listening for JM.
    CallMenu = 8,
    /// Originating with V.8: JM received, sending CJ.
    Terminator = 9,
    /// The modulation is chosen and its carrier is being sent; waiting for
    /// the far end's to have been there long enough.
    Training = 10,
    /// Connected: carrying data.
    Data = 11,
    /// The attempt failed, or the carrier was lost: sending nothing.
    Ended = 12,
    /// The modems have connected at 1200 or 2400 bit/s and the data link
    /// is being made: the detection phase of V.42, and the set-up of
    /// LAPM.
    Link = 13,
    /// The modems are retraining: no data passes until they have.
    Retrain = 14,
}

impl EngineState {
    /// Every state.
    pub const ALL: [EngineState; 15] = [
        EngineState::Idle,
        EngineState::AnswerSilence,
        EngineState::AnswerTone,
        EngineState::JointMenu,
        EngineState::Gap,
        EngineState::Fallback,
        EngineState::Listening,
        EngineState::CallSilence,
        EngineState::CallMenu,
        EngineState::Terminator,
        EngineState::Training,
        EngineState::Data,
        EngineState::Ended,
        EngineState::Link,
        EngineState::Retrain,
    ];

    /// The name of the state, as the control API and the manager show it.
    pub const fn name(self) -> &'static str {
        match self {
            EngineState::Idle => "idle",
            EngineState::AnswerSilence => "answer-silence",
            EngineState::AnswerTone => "answer-tone",
            EngineState::JointMenu => "v8-jm",
            EngineState::Gap => "gap",
            EngineState::Fallback => "fallback",
            EngineState::Listening => "listening",
            EngineState::CallSilence => "v8-te",
            EngineState::CallMenu => "v8-cm",
            EngineState::Terminator => "v8-cj",
            EngineState::Training => "training",
            EngineState::Data => "data",
            EngineState::Ended => "ended",
            EngineState::Link => "link",
            EngineState::Retrain => "retrain",
        }
    }

    /// The state of a number, as [`EngineState`] is numbered; a number
    /// that is no state is `Idle`.
    pub fn from_number(number: u8) -> EngineState {
        EngineState::ALL
            .into_iter()
            .find(|state| *state as u8 == number)
            .unwrap_or(EngineState::Idle)
    }
}

/// A change of state, and when it happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateChange {
    /// The state that was entered.
    pub state: EngineState,
    /// When, in milliseconds of the call's own samples since the engine
    /// was started.
    pub at_ms: u32,
}

/// What the scope's samples are samples of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ScopeKind {
    /// Nothing: the scope is empty.
    #[default]
    None = 0,
    /// The output of a frequency-shift keying demodulator: 127 is all
    /// mark and -127 all space, one sample for every four of the line's.
    Discriminator = 1,
}

/// An engine's snapshot of itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diagnostics {
    /// What the engine is doing.
    pub state: EngineState,
    /// The level received in the band being listened to, in dBm0. Below
    /// -99 is nothing.
    pub rx_level_dbm0: f32,
    /// The level being sent, in dBm0. Below -99 is nothing.
    pub tx_level_dbm0: f32,
    /// Whether the far end's carrier is detected.
    pub carrier: bool,
    /// How far the far end's carrier is from its nominal frequency, in
    /// hertz.
    pub frequency_error_hz: f32,
    /// How good the received signal is, from 0 to 1. For the 300 bit/s
    /// modems it is the opening of the eye where the bits are sampled,
    /// which on a clean line is about 0.7. For the others it is one less
    /// the error of the decisions as a part of the distance to a wrong
    /// one at 2400 bit/s, which on a clean line is about 0.9.
    pub quality: f32,
    /// Start-stop characters lost to a wrong stop element or a wrong
    /// parity in this call.
    pub framing_errors: u32,
    /// Retrains in this call. The 300 bit/s modems have none.
    pub retrains: u32,
    /// The last changes of state, the oldest first; `changes` of them are
    /// meant.
    pub history: [StateChange; STATE_HISTORY],
    /// How many entries of `history` are meant.
    pub changes: u8,
    /// What the scope's samples are.
    pub scope_kind: ScopeKind,
    /// The scope: the most recent samples, the oldest first.
    pub scope: [i8; SCOPE_SAMPLES],
    /// The signal-to-noise ratio of the decisions at 1200 and 2400 bit/s,
    /// in decibels; 0 when there is none to give.
    pub snr_db: f32,
    /// The mean-squared error of the equaliser's output against the
    /// decisions, for a constellation of mean power one.
    pub equalizer_mse: f32,
    /// The far end's symbol clock against this end's, in parts in a
    /// million.
    pub timing_offset_ppm: f32,
    /// The bits a symbol carries, which says what the ideal points are:
    /// 2 for the four points of 1200 bit/s, 4 for the sixteen of 2400
    /// bit/s, and 0 when there is no constellation.
    pub constellation_bits: u8,
    /// How many entries of `constellation` are meant.
    pub constellation_len: u8,
    /// The most recent received points, the oldest first, each its real
    /// and its imaginary part in [`CONSTELLATION_UNIT`]s of the grid.
    pub constellation: [[i8; 2]; CONSTELLATION_POINTS],
    /// What the data link counts.
    pub link: LinkStats,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Diagnostics {
            state: EngineState::Idle,
            rx_level_dbm0: -120.0,
            tx_level_dbm0: -120.0,
            carrier: false,
            frequency_error_hz: 0.0,
            quality: 0.0,
            framing_errors: 0,
            retrains: 0,
            history: [StateChange::default(); STATE_HISTORY],
            changes: 0,
            scope_kind: ScopeKind::None,
            scope: [0; SCOPE_SAMPLES],
            snr_db: 0.0,
            equalizer_mse: 0.0,
            timing_offset_ppm: 0.0,
            constellation_bits: 0,
            constellation_len: 0,
            constellation: [[0; 2]; CONSTELLATION_POINTS],
            link: LinkStats::default(),
        }
    }
}

impl Diagnostics {
    /// The changes of state that are meant, the oldest first.
    pub fn history(&self) -> &[StateChange] {
        &self.history[..usize::from(self.changes).min(STATE_HISTORY)]
    }

    /// The received points that are meant, the oldest first.
    pub fn constellation(&self) -> &[[i8; 2]] {
        &self.constellation[..usize::from(self.constellation_len).min(CONSTELLATION_POINTS)]
    }
}
