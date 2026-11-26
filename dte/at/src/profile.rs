// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A profile: the settings and S-registers the commands change. The
//! interpreter has an active profile, and may have a stored one, which
//! `&W` writes and `Z` returns to.

/// The S-registers the interpreter has. The index into the table is the
/// position in this list, not the register number.
pub(crate) const REGISTERS: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 10, 12];

/// The factory values, in the order of [`REGISTERS`]. S0 is replaced by the
/// line's configured value.
const FACTORY: [u8; 10] = [0, 0, 43, 13, 10, 8, 2, 50, 14, 50];

/// The factory value of `+MS`: carrier, automode, and the four rate limits
/// (V.250 clause 6.4.1). Phase 0 has no modulation, so this only names the
/// first one planned. Stored, no effect.
pub(crate) const FACTORY_MS: &str = "V21,1,0,0,0,0";
/// The factory value of `+ES` (V.250 clause 6.5.1). Stored, no effect.
pub(crate) const FACTORY_ES: &str = "3,0,2";
/// The factory value of `+DS` (V.250 clause 6.6.1). Stored, no effect.
pub(crate) const FACTORY_DS: &str = "3,0,2048,6";

/// What `+MS=?`, `+ES=?` and `+DS=?` report. Phase 0 has nothing to
/// negotiate; these are the shapes later phases fill in.
pub(crate) const RANGE_MS: &str = "(V21),(0,1),(0),(0),(0),(0)";
pub(crate) const RANGE_ES: &str = "(0-4),(0-4),(0-6)";
pub(crate) const RANGE_DS: &str = "(0-3),(0),(512-2048),(6-250)";

/// The longest value an extended-syntax parameter may be set to.
const MAX_EXTENDED_VALUE: usize = 64;

/// The settings and registers.
///
/// The fields are open so that the service can keep a stored profile in a
/// file and give it back. A profile that did not come from an interpreter
/// must pass [`Profile::check`] before one will take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// `E`: echo command characters.
    pub echo: bool,
    /// `Q`: suppress result codes.
    pub quiet: bool,
    /// `V`: verbose result codes.
    pub verbose: bool,
    /// `X`: result code set, 0 to 4.
    pub x: u8,
    /// `L`: speaker volume, 0 to 3. Stored only.
    pub volume: u8,
    /// `M`: speaker mode, 0 to 3. Stored only.
    pub speaker: u8,
    /// `&C1`: DCD follows the carrier; `&C0`: DCD always on.
    pub dcd_follows_carrier: bool,
    /// `&D`: what a DTR drop does, 0 to 3.
    pub dtr_action: u8,
    /// `&K`: flow control, 0 or 3. Stored only.
    pub flow: u8,
    /// `+MS`: stored as given.
    pub ms: String,
    /// `+ES`: stored as given.
    pub es: String,
    /// `+DS`: stored as given.
    pub ds: String,
    registers: [u8; 10],
}

impl Profile {
    /// The factory profile for a line whose configured S0 is `s0`.
    pub fn factory(s0: u8) -> Profile {
        let mut registers = FACTORY;
        registers[0] = s0;
        Profile {
            echo: true,
            quiet: false,
            verbose: true,
            x: 4,
            volume: 0,
            speaker: 0,
            dcd_follows_carrier: true,
            dtr_action: 2,
            flow: 3,
            ms: FACTORY_MS.to_string(),
            es: FACTORY_ES.to_string(),
            ds: FACTORY_DS.to_string(),
            registers,
        }
    }

    /// The value of register `number`, or `None` if there is no such
    /// register.
    pub fn register(&self, number: u32) -> Option<u8> {
        index(number).map(|i| self.registers[i])
    }

    /// Sets register `number`. Returns `false` if there is no such register.
    pub fn set_register(&mut self, number: u32, value: u8) -> bool {
        match index(number) {
            Some(i) => {
                self.registers[i] = value;
                true
            }
            None => false,
        }
    }

    /// Every register the profile has: its number and its value.
    pub fn registers(&self) -> impl Iterator<Item = (u8, u8)> + '_ {
        REGISTERS.iter().copied().zip(self.registers)
    }

    /// Whether every value is one the commands could have set. On failure
    /// the answer names what is wrong, as the commands spell it.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.x > 4 {
            return Err("X");
        }
        if self.volume > 3 {
            return Err("L");
        }
        if self.speaker > 3 {
            return Err("M");
        }
        if self.dtr_action > 3 {
            return Err("&D");
        }
        if self.flow != 0 && self.flow != 3 {
            return Err("&K");
        }
        // V.250 clause 6.2.1 to 6.2.3: S3, S4 and S5 are 0 to 127.
        if self.s3() > 127 {
            return Err("S3");
        }
        if self.s4() > 127 {
            return Err("S4");
        }
        if self.s5() > 127 {
            return Err("S5");
        }
        if !acceptable_parameter(self.ms.as_bytes()) {
            return Err("+MS");
        }
        if !acceptable_parameter(self.es.as_bytes()) {
            return Err("+ES");
        }
        if !acceptable_parameter(self.ds.as_bytes()) {
            return Err("+DS");
        }
        Ok(())
    }

    /// S0: rings before auto-answer.
    pub fn s0(&self) -> u8 {
        self.registers[0]
    }

    /// S1: the ring count.
    pub fn s1(&self) -> u8 {
        self.registers[1]
    }

    pub fn set_s1(&mut self, value: u8) {
        self.registers[1] = value;
    }

    /// S2: the escape character.
    pub fn s2(&self) -> u8 {
        self.registers[2]
    }

    /// S3: the command line terminator.
    pub fn s3(&self) -> u8 {
        self.registers[3]
    }

    /// S4: the response formatting character.
    pub fn s4(&self) -> u8 {
        self.registers[4]
    }

    /// S5: the command line editing character.
    pub fn s5(&self) -> u8 {
        self.registers[5]
    }

    /// S7: how long to wait for a connection, in milliseconds.
    pub fn connect_wait_ms(&self) -> u32 {
        u32::from(self.registers[7]) * 1000
    }

    /// S12: the escape guard time, in milliseconds.
    pub fn guard_ms(&self) -> u32 {
        u32::from(self.registers[9]) * 20
    }
}

/// Whether a `+MS`, `+ES` or `+DS` value is one a command line may set:
/// letters, digits, commas and full stops, within the length limit.
pub(crate) fn acceptable_parameter(value: &[u8]) -> bool {
    value.len() <= MAX_EXTENDED_VALUE
        && value
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b',' | b'.'))
}

fn index(number: u32) -> Option<usize> {
    REGISTERS.iter().position(|r| u32::from(*r) == number)
}
