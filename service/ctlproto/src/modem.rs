// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the service reports about its modems, and what it takes to set
//! them: the settings, the modem of a call, a line's diagnostics and the
//! loopback self-test.
//!
//! Levels are in tenths of a dBm0 and frequencies in millihertz: whole
//! numbers, so that what is reported can be compared for equality.

use serde::{Deserialize, Serialize};

use crate::model::{ConnectInfo, ErrorInfo};

/// The modem settings a line's calls use: those of `[modem]` with the
/// line's own over them.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LineModem {
    /// The modulations the modem may use, the preferred one first: `v21`,
    /// `bell103`.
    pub modulations: Vec<String>,
    /// Whether the modem uses V.8.
    pub v8: bool,
    /// The level the modem sends at, in dBm0.
    pub tx_level_dbm0: i32,
    /// The keys the line sets for itself; the others are `[modem]`'s.
    #[serde(default)]
    pub own: Vec<String>,
}

/// The `modem` part of `lines.add` and `lines.edit`: the line's own modem
/// settings, all of them. A key left out is taken from `[modem]`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineModemSpec {
    /// The modulations the line's modem may use, the preferred one first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulations: Option<Vec<String>>,
    /// Whether the line's modem uses V.8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v8: Option<bool>,
    /// The level the line's modem sends at, in dBm0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_level_dbm0: Option<i32>,
}

/// The `[modem]` settings.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModemSettings {
    /// The modulations a modem may use, the preferred one first.
    pub modulations: Vec<String>,
    /// Whether a modem uses V.8.
    pub v8: bool,
    /// The level a modem sends at, in dBm0.
    pub tx_level_dbm0: i32,
    /// Silence before an answering modem begins, in milliseconds.
    pub answer_delay_ms: u32,
    /// How long an answering modem offers each modulation, in
    /// milliseconds.
    pub fallback_ms: u32,
}

/// The `modem` part of `config.set`: the keys of `[modem]` to change.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModemChange {
    /// The modulations, all of them, the preferred one first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulations: Option<Vec<String>>,
    /// Whether a modem uses V.8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v8: Option<bool>,
    /// A new level to send at, in dBm0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_level_dbm0: Option<i32>,
    /// A new silence before answering, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_delay_ms: Option<u32>,
    /// A new time for each modulation offered, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_ms: Option<u32>,
}

/// The modem of a call in progress, as it last reported itself.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModemInfo {
    /// What the modem is doing: `answer-tone`, `training`, `data` and so
    /// on.
    pub state: String,
    /// The level received in the band listened to, in tenths of a dBm0.
    /// Absent when nothing is received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx_level_tenth_dbm0: Option<i32>,
    /// The level sent, in tenths of a dBm0. Absent when nothing is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_level_tenth_dbm0: Option<i32>,
    /// Whether the far end's carrier is detected.
    pub carrier: bool,
    /// How far the far end's carrier is from its nominal frequency, in
    /// millihertz.
    pub frequency_error_mhz: i32,
    /// How good the received signal is, in a hundred.
    pub quality_percent: u32,
    /// Characters lost to a wrong stop bit in this call.
    pub framing_errors: u32,
    /// Retrains in this call.
    pub retrains: u32,
}

/// A change of a modem's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModemStateChange {
    /// The state that was entered.
    pub state: String,
    /// When, in milliseconds of the call since the modem was started.
    pub at_ms: u32,
}

/// The diagnostics of a line: its call's modem with what the modem
/// remembers and sees. The result of `lines.diag` and the data of
/// `line.diag`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LineDiag {
    /// The line.
    pub line: u16,
    /// The modem of the line's call. Absent when the line has no call, or
    /// its engine is not a modem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modem: Option<ModemInfo>,
    /// The modem's last changes of state, the oldest first.
    #[serde(default)]
    pub history: Vec<ModemStateChange>,
    /// What the scope's samples are: `none` or `discriminator`.
    #[serde(default = "no_scope")]
    pub scope_kind: String,
    /// The scope: the most recent samples, the oldest first, each from
    /// -127 to 127.
    #[serde(default)]
    pub scope: Vec<i8>,
}

fn no_scope() -> String {
    "none".to_string()
}

/// The parameters of `selftest.run`: two lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfTest {
    /// The line whose modem originates.
    pub a: u16,
    /// The line whose modem answers.
    pub b: u16,
}

/// The result of `selftest.run`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfTestResult {
    /// The line whose modem originated.
    pub a: u16,
    /// The line whose modem answered.
    pub b: u16,
    /// Whether the modems connected and every octet arrived, both ways.
    pub passed: bool,
    /// What the modems connected with, if they connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<ConnectInfo>,
    /// How long the modems took to connect, in milliseconds of the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<u32>,
    /// The octets sent each way.
    pub octets: u32,
    /// The octets that arrived as they were sent, from a to b.
    pub octets_a_to_b: u32,
    /// The octets that arrived as they were sent, from b to a.
    pub octets_b_to_a: u32,
    /// How long the call was, in milliseconds of its own samples.
    pub call_ms: u32,
    /// How long the test took, in milliseconds.
    pub elapsed_ms: u32,
    /// Why the test failed, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}
