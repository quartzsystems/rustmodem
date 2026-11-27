// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A profile: the settings and S-registers the commands change. The
//! interpreter has an active profile, and may have a stored one, which
//! `&W` writes and `Z` returns to.

/// The S-registers the interpreter has. The index into the table is the
/// position in this list, not the register number.
pub(crate) const REGISTERS: [u8; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 12];

/// The factory values, in the order of [`REGISTERS`]. S0 is replaced by the
/// line's configured value.
const FACTORY: [u8; 11] = [0, 0, 43, 13, 10, 8, 2, 50, 6, 14, 50];

/// The factory value of `+MS`: carrier, automode, and the four rate limits
/// (V.250 clause 6.4.1). V.21 first, with automode: any modulation the
/// line allows.
pub(crate) const FACTORY_MS: &str = "V21,1,0,0,0,0";
/// The factory value of `+ES` (V.250 clause 6.5.1). Stored, no effect.
pub(crate) const FACTORY_ES: &str = "3,0,2";
/// The factory value of `+DS` (V.250 clause 6.6.1). Stored, no effect.
pub(crate) const FACTORY_DS: &str = "3,0,2048,6";

/// What `+MS=?` reports (V.250 clause 6.4.1): the carriers, automode,
/// and the rates, which for both carriers are 300 bit/s and nothing else.
/// `V21` is the carrier string of Table 13/V.250. Bell 103 is not an ITU
/// modulation and has none there: `B103` is this modem's own, which the
/// table's note allows as long as it does not begin with `V`.
pub(crate) const RANGE_MS: &str = "(V21,B103),(0,1),(0,300),(0,300),(0,300),(0,300)";
/// What `+ES=?` and `+DS=?` report. Nothing is negotiated yet; these are
/// the shapes later phases fill in.
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
    registers: [u8; 11],
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
        if ModulationSelection::parse(self.ms.as_bytes(), None).is_none() {
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

    /// S9: how long the far end's carrier must be there before the
    /// connection is reported, in milliseconds. The register is in tenths
    /// of a second. It is not in V.250: it is the register of that number
    /// and meaning that Hayes-compatible modems have.
    pub fn carrier_response_ms(&self) -> u32 {
        u32::from(self.register(9).unwrap_or(6)) * 100
    }

    /// S10: how long the far end's carrier may be gone before the call is
    /// ended, in milliseconds. The register is in tenths of a second
    /// (V.250 clause 6.3.12).
    pub fn carrier_loss_ms(&self) -> u32 {
        u32::from(self.register(10).unwrap_or(14)) * 100
    }

    /// S12: the escape guard time, in milliseconds.
    pub fn guard_ms(&self) -> u32 {
        u32::from(self.register(12).unwrap_or(50)) * 20
    }

    /// What `+MS` selects: the carrier preferred, and whether any other
    /// may be used.
    pub fn modulation(&self) -> ModulationSelection {
        ModulationSelection::parse(self.ms.as_bytes(), None).unwrap_or_default()
    }

    /// Sets the carrier `+MS` prefers and leaves the rest of it as it is:
    /// what `B0` and `B1` do.
    pub fn set_carrier(&mut self, carrier: Carrier) {
        let mut selection = self.modulation();
        selection.carrier = carrier;
        self.ms = selection.to_string();
    }
}

/// A carrier `+MS` can name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Carrier {
    /// `V21`: ITU-T V.21 (Table 13/V.250).
    #[default]
    V21,
    /// `B103`: Bell 103.
    Bell103,
}

impl Carrier {
    /// The carrier as `+MS` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Carrier::V21 => "V21",
            Carrier::Bell103 => "B103",
        }
    }

    fn parse(text: &[u8]) -> Option<Carrier> {
        match text {
            b"V21" => Some(Carrier::V21),
            b"B103" => Some(Carrier::Bell103),
            _ => None,
        }
    }
}

/// The value of `+MS` (V.250 clause 6.4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModulationSelection {
    /// `<carrier>`: the modulation to prefer.
    pub carrier: Carrier,
    /// `<automode>`: whether another modulation may be used, and whether
    /// one may be negotiated by V.8.
    pub automode: bool,
    /// `<min_rate>`, `<max_rate>`, `<min_rx_rate>` and `<max_rx_rate>`:
    /// each 0, for whatever the carrier has, or 300.
    pub rates: [u16; 4],
}

impl Default for ModulationSelection {
    fn default() -> Self {
        ModulationSelection {
            carrier: Carrier::V21,
            automode: true,
            rates: [0; 4],
        }
    }
}

impl ModulationSelection {
    /// Reads a value as a command line or a stored profile gives it. A
    /// subparameter that is left out keeps the value it has in `current`,
    /// or its default if there is no `current` (V.250 clause 5.4.2.1).
    /// `None` if any subparameter is not one this modem has.
    pub fn parse(
        value: &[u8],
        current: Option<ModulationSelection>,
    ) -> Option<ModulationSelection> {
        let mut selection = current.unwrap_or_default();
        let mut parts = value.split(|b| *b == b',');
        if let Some(carrier) = parts.next()
            && !carrier.is_empty()
        {
            selection.carrier = Carrier::parse(carrier)?;
        }
        if let Some(automode) = parts.next()
            && !automode.is_empty()
        {
            selection.automode = match automode {
                b"0" => false,
                b"1" => true,
                _ => return None,
            };
        }
        for rate in &mut selection.rates {
            match parts.next() {
                None | Some(b"") => {}
                Some(b"0") => *rate = 0,
                Some(b"300") => *rate = 300,
                Some(_) => return None,
            }
        }
        // Six subparameters and no more.
        parts.next().is_none().then_some(selection)
    }
}

impl std::fmt::Display for ModulationSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d] = self.rates;
        write!(
            f,
            "{},{},{a},{b},{c},{d}",
            self.carrier.as_str(),
            u8::from(self.automode)
        )
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
