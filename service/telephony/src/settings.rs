// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the router takes from the configuration.

use std::net::SocketAddrV4;

use config::{Cidr, CodecName, Config, HuntGroup, PortRange, RegisterConfig, Route};
use sdp::{AudioPrefs, Codec};

use crate::hunt::LineId;

/// The settings of the router: `[sip]`, `[media]`, the registrations of
/// the lines, the routes and the hunt groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The UDP address of the trunk listener.
    pub listen: SocketAddrV4,
    /// The address to advertise; empty for the one that routes to the
    /// peer.
    pub address: String,
    /// The User-Agent and Server header.
    pub user_agent: String,
    pub trusted_peers: Vec<Cidr>,
    /// Where lines that are not registered send the calls they dial;
    /// empty for nowhere.
    pub outbound_peer: String,
    pub ring_timeout_ms: u64,
    /// The codecs and the packet time we offer and accept.
    pub prefs: AudioPrefs,
    pub rtp_ports: PortRange,
    pub rtp_timeout_ms: u64,
    /// The lines that are extensions of a PBX, with how they register. A
    /// line that is out of service does not register.
    pub registrations: Vec<(LineId, RegisterConfig)>,
    pub routes: Vec<Route>,
    pub hunt_groups: Vec<HuntGroup>,
}

impl Settings {
    /// The settings a configuration asks for. `version` is the service's,
    /// for the User-Agent when the configuration names none.
    pub fn new(config: &Config, version: &str) -> Settings {
        Settings {
            listen: config.sip.listen,
            address: config.sip.address.clone(),
            user_agent: if config.sip.user_agent.is_empty() {
                format!("rustmodem/{version}")
            } else {
                config.sip.user_agent.clone()
            },
            trusted_peers: config.sip.trusted_peers.clone(),
            outbound_peer: config.sip.outbound_peer.clone(),
            ring_timeout_ms: u64::from(config.sip.ring_timeout_s) * 1000,
            prefs: AudioPrefs {
                codecs: config
                    .media
                    .codecs
                    .iter()
                    .map(|codec| match codec {
                        CodecName::Pcmu => Codec::Pcmu,
                        CodecName::Pcma => Codec::Pcma,
                    })
                    .collect(),
                ptime_ms: config.media.ptime_ms,
            },
            rtp_ports: config.media.rtp_ports,
            rtp_timeout_ms: u64::from(config.media.rtp_timeout_s) * 1000,
            registrations: config
                .lines
                .iter()
                .filter(|line| line.enabled)
                .filter_map(|line| Some((line.id, line.register.clone()?)))
                .collect(),
            routes: config.routes.clone(),
            hunt_groups: config.hunt_groups.clone(),
        }
    }
}

/// Splits `host` or `host:port`; a port that is left out is 5060 (RFC 3261
/// section 19.1.2).
pub fn host_and_port(text: &str) -> (&str, u16) {
    match text.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().unwrap_or(5060)),
        None => (text, 5060),
    }
}
