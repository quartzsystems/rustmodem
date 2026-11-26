// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What is known about a connection once the modems have connected.

/// What a modem reports when it connects. Every part is optional so that
/// each later modem fills in what it knows.
#[derive(Clone, Debug, PartialEq, Eq, Default, Hash)]
pub struct ConnectInfo {
    /// The line rate in bit/s; 0 means the rate is not known.
    pub rate: u32,
    /// The carrier (modulation) name, for example `V.34`.
    pub carrier: Option<String>,
    /// The error-control protocol name, for example `LAPM`.
    pub protocol: Option<String>,
    /// The data compression name, for example `V.42bis`.
    pub compression: Option<String>,
}

impl ConnectInfo {
    /// A connection of which nothing is known.
    pub fn unknown() -> ConnectInfo {
        ConnectInfo::default()
    }

    /// A connection of which only the rate is known.
    pub fn with_rate(rate: u32) -> ConnectInfo {
        ConnectInfo {
            rate,
            ..ConnectInfo::default()
        }
    }
}
