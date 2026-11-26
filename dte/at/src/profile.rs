// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The active profile: the settings and S-registers the commands change.

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

/// The settings and registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Profile {
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
    /// `+MS`, `+ES`, `+DS`: stored as given.
    pub ms: String,
    pub es: String,
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

fn index(number: u32) -> Option<usize> {
    REGISTERS.iter().position(|r| u32::from(*r) == number)
}
