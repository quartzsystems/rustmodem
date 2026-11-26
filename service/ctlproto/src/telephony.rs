// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the service reports about SIP and media: registrations, the
//! listener, hunt groups, routes, and the RTP of a call.

use serde::{Deserialize, Serialize};

use crate::model::ErrorInfo;

/// How a line is reached over SIP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SipMode {
    /// Not at all: the service does not speak SIP.
    #[default]
    None,
    /// By the trunk: calls come from the trusted peers and go to the
    /// outbound peer.
    Trunk,
    /// As an extension of a PBX the line registers to.
    Registered,
}

impl SipMode {
    /// Every mode.
    pub const ALL: [SipMode; 3] = [SipMode::None, SipMode::Trunk, SipMode::Registered];

    /// The mode as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            SipMode::None => "none",
            SipMode::Trunk => "trunk",
            SipMode::Registered => "registered",
        }
    }
}

/// The state of a line's registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationState {
    /// Not registered, and not trying.
    Unregistered,
    /// A REGISTER is out and there is no binding yet.
    Registering,
    /// The registrar holds a binding for the line.
    Registered,
    /// The last try failed; another follows.
    Failed,
}

impl RegistrationState {
    /// Every state.
    pub const ALL: [RegistrationState; 4] = [
        RegistrationState::Unregistered,
        RegistrationState::Registering,
        RegistrationState::Registered,
        RegistrationState::Failed,
    ];

    /// The state as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            RegistrationState::Unregistered => "unregistered",
            RegistrationState::Registering => "registering",
            RegistrationState::Registered => "registered",
            RegistrationState::Failed => "failed",
        }
    }
}

/// A line's registration as it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    /// The state.
    pub state: RegistrationState,
    /// The seconds left of the binding, when registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_s: Option<u64>,
    /// Why the registration failed, when it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

/// The SIP side of a line.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LineSip {
    /// How the line is reached.
    pub mode: SipMode,
    /// The extension the line registers as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    /// The registrar, as `host` or `host:port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// The user name the line authenticates with, when it is not the
    /// extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_user: Option<String>,
    /// The registration time the line asks for, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_s: Option<u32>,
    /// The registration, for a line that registers and is in service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<Registration>,
    /// Whether a password is configured. The password itself is never
    /// reported.
    #[serde(default)]
    pub has_password: bool,
}

/// The RTP of a call: what was agreed and what has happened.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaInfo {
    /// The codec: `PCMU` or `PCMA`.
    pub codec: String,
    /// The packet time in milliseconds.
    pub ptime_ms: u32,
    /// Where the far end receives RTP, as `address:port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Packets delivered from the far end.
    pub rx_packets: u64,
    /// Payload octets delivered from the far end.
    pub rx_octets: u64,
    /// Packets sent to the far end.
    pub tx_packets: u64,
    /// Payload octets sent to the far end.
    pub tx_octets: u64,
    /// Packets from the far end that never arrived in time.
    pub lost: u64,
    /// Packets that arrived after their place had been filled.
    pub late: u64,
    /// Packets that had been delivered already.
    pub duplicate: u64,
    /// Times the far end's stream started again.
    pub resync: u64,
    /// Packets of another payload type, or not of whole frames.
    pub ignored: u64,
    /// Frames sent on the local timer before the far end's first packet.
    pub free_run: u64,
    /// Points where the frames sent stopped matching the frames taken.
    /// There should never be one.
    pub slips: u64,
    /// The interarrival jitter, smoothed as RFC 3550 section 6.4.1 has it,
    /// in microseconds.
    pub jitter_us_avg: u32,
    /// The most the jitter has been, in microseconds.
    pub jitter_us_max: u32,
    /// The time a tick takes, in microseconds: the median.
    pub tick_p50_us: u32,
    /// The 99th percentile.
    pub tick_p99_us: u32,
    /// The longest.
    pub tick_max_us: u32,
    /// Ticks that took longer than the packet time.
    pub overruns: u64,
}

/// The state of the SIP listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListenerState {
    /// SIP is off.
    #[default]
    Off,
    /// The listener is bound and takes requests.
    Listening,
    /// SIP is on in the configuration and the listener could not be bound.
    Failed,
}

impl ListenerState {
    /// Every state.
    pub const ALL: [ListenerState; 3] = [
        ListenerState::Off,
        ListenerState::Listening,
        ListenerState::Failed,
    ];

    /// The state as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            ListenerState::Off => "off",
            ListenerState::Listening => "listening",
            ListenerState::Failed => "failed",
        }
    }
}

/// SIP in a line of numbers, for the statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SipSummary {
    /// The state of the listener.
    pub listener: ListenerState,
    /// How many lines are registered.
    pub registered: u32,
    /// How many lines in service have a registration configured.
    pub registering: u32,
    /// Requests dropped because they came from an address that is not
    /// trusted.
    pub dropped_untrusted: u64,
}

/// One line's registration, with where it registers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SipRegistration {
    /// The line.
    pub line: u16,
    /// The registrar, as the configuration names it.
    pub server: String,
    /// The extension.
    pub user: String,
    /// The registration as it stands.
    pub registration: Registration,
}

/// The result of `sip.status`.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SipStatus {
    /// The state of the listener.
    pub listener: ListenerState,
    /// The address the listener is bound to, as `address:port`; empty
    /// when SIP is off.
    pub listen: String,
    /// The addresses that may send requests to the listener.
    pub trusted_peers: Vec<String>,
    /// Where lines that are not registered send the calls they dial;
    /// empty for nowhere.
    pub outbound_peer: String,
    /// Every line that registers, in id order.
    pub registrations: Vec<SipRegistration>,
    /// Requests dropped because they came from an address that is not
    /// trusted.
    pub dropped_untrusted: u64,
    /// Calls the router has: ringing, connected or being cleared.
    pub calls: u32,
}

impl SipStatus {
    /// The status in a line of numbers.
    pub fn summary(&self) -> SipSummary {
        SipSummary {
            listener: self.listener,
            registered: self
                .registrations
                .iter()
                .filter(|line| line.registration.state == RegistrationState::Registered)
                .count() as u32,
            registering: self.registrations.len() as u32,
            dropped_untrusted: self.dropped_untrusted,
        }
    }
}

/// Which way a traced message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceDirection {
    /// The service sent it.
    Sent,
    /// The service received it.
    Received,
}

impl TraceDirection {
    /// Both directions.
    pub const ALL: [TraceDirection; 2] = [TraceDirection::Sent, TraceDirection::Received];

    /// The direction as the control API spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            TraceDirection::Sent => "sent",
            TraceDirection::Received => "received",
        }
    }
}

/// A SIP message that passed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SipTrace {
    /// When, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    /// Which way it went.
    pub direction: TraceDirection,
    /// The other end, as `address:port`.
    pub peer: String,
    /// The message as it was on the wire. The response to a digest
    /// challenge is in it; the password is not.
    pub text: String,
}

/// A hunt group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HuntGroupInfo {
    /// The name, unique.
    pub name: String,
    /// The number that calls the group.
    pub pilot: String,
    /// The lines of the group, in hunting order.
    pub lines: Vec<u16>,
    /// How the group chooses among its idle lines: `first-idle`,
    /// `round-robin` or `least-recent`.
    pub strategy: String,
}

/// The result of `huntgroups.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HuntGroupList {
    /// The hunt groups, in the order of the configuration file.
    pub groups: Vec<HuntGroupInfo>,
}

/// A route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteInfo {
    /// Where the route is in the list, from 0. Routes are tried in order.
    pub index: u32,
    /// The called number the route is for: exact, or a prefix ending in
    /// `*`.
    pub called: String,
    /// Where its calls go: `line:<id>` or `group:<name>`.
    pub target: String,
}

/// The result of `routes.list` and of every change to the routes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteList {
    /// The routes, in the order they are tried.
    pub routes: Vec<RouteInfo>,
}
