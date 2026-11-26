// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The tones of call establishment: answer tone, calling tone, and the
//! detectors of both.
//!
//! The answering modem's first sound is the answer tone, 2100 Hz. It comes
//! in four kinds: plain (ANS, ITU-T V.25 clause 2.2), with its phase
//! reversed every 450 ms to disable echo cancellers (V.25 clause 2.3), and
//! each of those with its amplitude modulated at 15 Hz (ANSam, ITU-T V.8
//! clause 7.2), which tells a calling modem that the answerer has V.8. The
//! calling modem may send calling tone, bursts of 1300 Hz (V.25 clause
//! 2.1).
//!
//! The editions are V.25 (10/96) with its Corrigendum 1 (07/2001), and V.8
//! (11/2000). Every figure here is from one of them, with the clause
//! beside it.

#![forbid(unsafe_code)]

mod detect;
mod generate;

pub use detect::{AnswerToneDetector, AnswerToneStatus, CallingToneDetector, ToneDetector};
pub use generate::{AnswerTone, CallingTone};

/// The frequency of answer tone in hertz (V.25 clause 2.2, Note 1; V.8
/// clause 7.2).
pub const ANSWER_TONE_HZ: f32 = 2100.0;

/// How far from 2100 Hz plain answer tone may be, in hertz (V.25 clause
/// 2.2, Note 1). ANSam is held to 1 Hz (V.8 clause 7.2).
pub const ANSWER_TONE_TOLERANCE_HZ: f32 = 15.0;

/// The time between two phase reversals of answer tone, in milliseconds:
/// 450, and anywhere from 425 to 475 is in tolerance (V.25 clause 2.3 and
/// Figure 3; V.8 clause 7.2).
pub const REVERSAL_INTERVAL_MS: u32 = 450;

/// The tolerance of the reversal interval, either way, in milliseconds
/// (V.25 clause 2.3; V.8 clause 7.2).
pub const REVERSAL_TOLERANCE_MS: u32 = 25;

/// The frequency of the sine wave that modulates the amplitude of ANSam,
/// in hertz. It is held to 0.1 Hz (V.8 clause 7.2).
pub const ANSAM_MODULATION_HZ: f32 = 15.0;

/// The depth of ANSam's modulation: its envelope swings between 0.8 and
/// 1.2 times its average amplitude, each held to 0.01 (V.8 clause 7.2).
pub const ANSAM_MODULATION_DEPTH: f32 = 0.2;

/// How long plain answer tone lasts if nothing cuts it short, in
/// milliseconds: 3.3 s, and 0.7 s either way is in tolerance (V.25 clause
/// 2.2, Note 1, and clause 4.3).
pub const ANS_DURATION_MS: u32 = 3300;

/// How long ANSam lasts if neither CM nor a calling modem's signal ends
/// it, in milliseconds: 5 s, and 1 s either way is in tolerance (V.8
/// clause 8.2.2).
pub const ANSAM_DURATION_MS: u32 = 5000;

/// The silence an answering modem keeps after it is connected to the
/// line and before its answer tone, in milliseconds. V.25 clause 4.3 asks
/// for 1.8 to 2.5 s; this is the middle. V.8 clause 8.2 asks only for at
/// least 0.2 s.
pub const ANSWER_SILENCE_MS: u32 = 2150;

/// The least silence V.8 allows before the answer tone, in milliseconds
/// (V.8 clause 8.2).
pub const V8_ANSWER_SILENCE_MIN_MS: u32 = 200;

/// The silence after the answer tone and before the answering modem's own
/// signal, in milliseconds. V.25 clause 4.4 allows 20 ms either way, and
/// V.8 clauses 8.2.2 and 8.2.3 allow 5 ms.
pub const POST_TONE_SILENCE_MS: u32 = 75;

/// The frequency of calling tone in hertz, held to 15 Hz (V.25 clause
/// 2.1).
pub const CALLING_TONE_HZ: f32 = 1300.0;

/// How long a burst of calling tone lasts, in milliseconds. V.25 clause
/// 2.1 allows 0.5 to 0.7 s; this is the middle.
pub const CALLING_TONE_ON_MS: u32 = 600;

/// The silence between two bursts of calling tone, in milliseconds. V.25
/// clause 2.1 allows 1.5 to 2.0 s; this is the middle.
pub const CALLING_TONE_OFF_MS: u32 = 1750;

/// How long answer tone is heard before it is taken as recognised, in
/// milliseconds. V.25 clause 3.9 allows 100 to 600 ms.
pub const ANSWER_TONE_RECOGNITION_MS: u32 = 100;

/// The kinds of answer tone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnswerToneKind {
    /// ANS: plain 2100 Hz (V.25 clause 2.2).
    Ans,
    /// ANS with phase reversals (V.25 clause 2.3), which disables echo
    /// cancellers as well as echo suppressors.
    AnsReversing,
    /// ANSam: 2100 Hz with its amplitude modulated at 15 Hz, and no phase
    /// reversals (V.8 clause 7.2, for when echo canceller disabling is not
    /// required).
    Ansam,
    /// ANSam with phase reversals (V.8 clause 7.2).
    AnsamReversing,
}

impl AnswerToneKind {
    /// Whether the amplitude is modulated: whether this is ANSam.
    pub const fn is_modulated(self) -> bool {
        matches!(self, AnswerToneKind::Ansam | AnswerToneKind::AnsamReversing)
    }

    /// Whether the phase is reversed every 450 ms.
    pub const fn is_reversing(self) -> bool {
        matches!(
            self,
            AnswerToneKind::AnsReversing | AnswerToneKind::AnsamReversing
        )
    }

    /// How long the tone lasts if nothing ends it, in milliseconds.
    pub const fn duration_ms(self) -> u32 {
        if self.is_modulated() {
            ANSAM_DURATION_MS
        } else {
            ANS_DURATION_MS
        }
    }
}

/// Samples in this many milliseconds, at 8000 samples a second.
pub const fn samples_of_ms(ms: u32) -> u32 {
    ms * 8
}
