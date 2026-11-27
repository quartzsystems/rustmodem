// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modulations the engine has.

/// A modulation: one of the modems the engine can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Modulation {
    /// ITU-T V.22bis: 2400 bit/s.
    V22bis = 0,
    /// ITU-T V.22: 1200 bit/s.
    V22 = 1,
    /// Bell 212A: 1200 bit/s.
    Bell212A = 2,
    /// ITU-T V.21: 300 bit/s.
    V21 = 3,
    /// Bell 103: 300 bit/s.
    Bell103 = 4,
}

/// How many modulations there are.
pub const MODULATIONS: usize = 5;

/// A list of allowed modulations, in order, as a modem's settings have
/// it: what there is no room for is left out.
pub fn modulations(list: &[Modulation]) -> [Option<Modulation>; MODULATIONS] {
    let mut allowed = [None; MODULATIONS];
    for (slot, &modulation) in allowed.iter_mut().zip(list) {
        *slot = Some(modulation);
    }
    allowed
}

impl Modulation {
    /// Every modulation, the fastest first: the order of the default
    /// configuration, and the order in which an answering modem tries
    /// them on a caller that has no V.8.
    pub const ALL: [Modulation; MODULATIONS] = [
        Modulation::V22bis,
        Modulation::V22,
        Modulation::Bell212A,
        Modulation::V21,
        Modulation::Bell103,
    ];

    /// The name of the modulation, as the configuration has it.
    pub const fn name(self) -> &'static str {
        match self {
            Modulation::V22bis => "v22bis",
            Modulation::V22 => "v22",
            Modulation::Bell212A => "bell212a",
            Modulation::V21 => "v21",
            Modulation::Bell103 => "bell103",
        }
    }

    /// The modulation of a name of the configuration's.
    pub fn from_name(name: &str) -> Option<Modulation> {
        Modulation::ALL
            .into_iter()
            .find(|modulation| modulation.name() == name)
    }

    /// The name a connection on this modulation is reported by.
    pub const fn carrier_name(self) -> &'static str {
        match self {
            Modulation::V22bis => v22::Standard::V22bis.carrier_name(),
            Modulation::V22 => v22::Standard::V22.carrier_name(),
            Modulation::Bell212A => v22::Standard::Bell212A.carrier_name(),
            Modulation::V21 => fsk::Standard::V21.carrier_name(),
            Modulation::Bell103 => fsk::Standard::Bell103.carrier_name(),
        }
    }

    /// The line rate of a connection on this modulation, in bit/s.
    pub const fn rate(self) -> u32 {
        match self {
            Modulation::V22bis => 2400,
            Modulation::V22 | Modulation::Bell212A => 1200,
            Modulation::V21 | Modulation::Bell103 => fsk::BIT_RATE,
        }
    }

    /// Whether the line is synchronous: a modem of `modem/v22`, with a
    /// data link over it. The others are the 300 bit/s modems, whose
    /// line carries start-stop characters as they are.
    pub const fn synchronous(self) -> bool {
        self.fsk().is_none()
    }

    /// The 300 bit/s modem this is, if it is one.
    pub(crate) const fn fsk(self) -> Option<fsk::Standard> {
        match self {
            Modulation::V21 => Some(fsk::Standard::V21),
            Modulation::Bell103 => Some(fsk::Standard::Bell103),
            _ => None,
        }
    }

    pub(crate) const fn of_fsk(standard: fsk::Standard) -> Modulation {
        match standard {
            fsk::Standard::V21 => Modulation::V21,
            fsk::Standard::Bell103 => Modulation::Bell103,
        }
    }

    pub(crate) const fn of_v22(standard: v22::Standard) -> Modulation {
        match standard {
            v22::Standard::V22bis => Modulation::V22bis,
            v22::Standard::V22 => Modulation::V22,
            v22::Standard::Bell212A => Modulation::Bell212A,
        }
    }
}
