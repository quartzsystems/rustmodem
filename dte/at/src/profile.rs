// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A profile: the settings and S-registers the commands change. The
//! interpreter has an active profile, and may have a stored one, which
//! `&W` writes and `Z` returns to.

use engine::{Compression, ErrorControl, Modulation};

use crate::AtConfig;

/// The S-registers the interpreter has. The index into the table is the
/// position in this list, not the register number.
pub(crate) const REGISTERS: [u8; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 12, 36, 46, 48];

/// The factory values, in the order of [`REGISTERS`]. S0 is replaced by the
/// line's configured value. S36, S46 and S48 are not in V.250: they are
/// the registers of those numbers that modems of the Rockwell kind have
/// for error control, and their factory values there, 7, 138 and 7, are
/// what `+ES=3,0,2` and `+DS=3` are.
const FACTORY: [u8; 14] = [0, 0, 43, 13, 10, 8, 2, 50, 6, 14, 50, 7, 138, 7];

/// The factory value of `+MS`: carrier, automode, and the four rate limits
/// (V.250 clause 6.4.1). V.22bis first, with automode: any modulation the
/// line allows.
pub(crate) const FACTORY_MS: &str = "V22B,1,0,0,0,0";
/// The factory value of `+ES` (V.250 clause 6.5.1) on a line whose policy
/// is error control if the far end has it: the Recommendation's
/// defaults.
pub(crate) const FACTORY_ES: &str = "3,0,2";
/// The factory value of `+DS` (V.250 clause 6.6.1) on a line that allows
/// compression. The dictionary and the string length are the
/// manufacturer's to choose (V.42bis Appendix II); the Recommendation's
/// default of 6 for the string length gives up most of what compression
/// is for.
pub(crate) const FACTORY_DS: &str = "3,0,2048,32";

/// What `+MS=?` reports (V.250 clause 6.4.1): the carriers, automode,
/// and the rates. `V21`, `V22` and `V22B` are the carrier strings of
/// Table 13/V.250. The Bell modems are not ITU modulations and have none
/// there: `B103` and `B212A` are this modem's own, which the table's note
/// allows as long as they do not begin with `V`.
pub(crate) const RANGE_MS: &str = "(V21,V22,V22B,B103,B212A),(0,1),(0,300,1200,2400),\
(0,300,1200,2400),(0,300,1200,2400),(0,300,1200,2400)";
/// What `+ES=?` reports: every mode of Table 20/V.250 but those that ask
/// for the alternative protocol of V.42 Annex A, which this modem has
/// not got, and the synchronous access mode.
pub(crate) const RANGE_ES: &str = "(0-3),(0-3),(0-5)";
/// What `+DS=?` reports: every direction, no disconnecting when
/// compression is not agreed, and the dictionaries and strings of
/// V.42bis that this modem has.
pub(crate) const RANGE_DS: &str = "(0-3),(0),(512-4096),(6-250)";

/// The factory value of `+ICF` (V.250 clause 6.2.11: the recommended
/// defaults, format 3, eight data bits and one stop bit, and parity 3).
pub(crate) const FACTORY_ICF: &str = "3,3";
/// What `+ICF=?` reports: the formats this modem has, 3 and 5 of Table
/// 5/V.250, and the parities the command takes.
pub(crate) const RANGE_ICF: &str = "(3,5),(0-3)";

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
    /// `+MS`: the modulation selection.
    pub ms: String,
    /// `+ES`: the error control selection.
    pub es: String,
    /// `+DS`: the data compression selection.
    pub ds: String,
    /// `+ICF`: the format and the parity.
    pub icf: String,
    /// `+MR`: report the modulation when a call connects.
    pub mr: bool,
    /// `+ER`: report the error control.
    pub er: bool,
    /// `+DR`: report the data compression.
    pub dr: bool,
    /// `+ILRR`: report the rate.
    pub ilrr: bool,
    registers: [u8; 14],
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
            icf: FACTORY_ICF.to_string(),
            mr: false,
            er: false,
            dr: false,
            ilrr: false,
            registers,
        }
    }

    /// The factory profile of a line: its configured S0, and its policy
    /// for error control and compression as the factory values of `+ES`
    /// and `+DS`, which the host's commands then change.
    pub fn for_line(config: &AtConfig) -> Profile {
        let mut profile = Profile::factory(config.auto_answer_rings);
        profile.es = match config.error_control {
            ErrorControl::Auto => FACTORY_ES,
            // LAPM or nothing, calling and answering.
            ErrorControl::Required => "3,2,4",
            // Buffered mode only, and error control disabled.
            ErrorControl::Off => "1,0,1",
        }
        .to_string();
        if !config.compression {
            profile.set_compression_direction(0);
        }
        profile
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
        if ErrorControlSelection::parse(self.es.as_bytes(), None).is_none() {
            return Err("+ES");
        }
        if CompressionSelection::parse(self.ds.as_bytes(), None).is_none() {
            return Err("+DS");
        }
        if !matches!(self.register(36), Some(0 | 1 | 3 | 4 | 5 | 7)) {
            return Err("S36");
        }
        if !matches!(self.register(46), Some(136 | 138)) {
            return Err("S46");
        }
        if !matches!(self.register(48), Some(0 | 7 | 128)) {
            return Err("S48");
        }
        if CharacterFraming::parse(self.icf.as_bytes(), None).is_none() {
            return Err("+ICF");
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

    /// What `+ICF` selects: the character format of the line where the
    /// host's characters go as start-stop characters.
    pub fn character_format(&self) -> CharacterFormat {
        CharacterFraming::parse(self.icf.as_bytes(), None)
            .map_or(CharacterFormat::N81, CharacterFraming::format)
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

    /// Sets the carrier `+MS` prefers and leaves the rest of it as it is.
    pub fn set_carrier(&mut self, carrier: Carrier) {
        let mut selection = self.modulation();
        selection.carrier = carrier;
        self.ms = selection.to_string();
    }

    /// `B0` and `B1`: the ITU's modems at 300 and 1200 bit/s, or Bell's.
    /// The carrier of `+MS` becomes the one of the same rate in the other
    /// family. At 2400 bit/s there is one modem, V.22bis, which `B0`
    /// leaves; `B1` there asks for Bell 212A.
    pub fn set_family(&mut self, bell: bool) {
        let carrier = match (self.modulation().carrier, bell) {
            (Carrier::V21, true) => Carrier::Bell103,
            (Carrier::V22 | Carrier::V22bis, true) => Carrier::Bell212A,
            (Carrier::Bell103, false) => Carrier::V21,
            (Carrier::Bell212A, false) => Carrier::V22,
            (carrier, _) => carrier,
        };
        self.set_carrier(carrier);
    }

    /// What `+ES` selects.
    pub fn error_control_selection(&self) -> ErrorControlSelection {
        ErrorControlSelection::parse(self.es.as_bytes(), None).unwrap_or_default()
    }

    /// The error control of a call this modem originates, or answers.
    pub fn error_control(&self, originate: bool) -> ErrorControl {
        let selection = self.error_control_selection();
        if originate {
            selection.originating()
        } else {
            selection.answering()
        }
    }

    /// Whether an originating modem runs the detection phase of V.42.
    pub fn detection(&self) -> bool {
        self.error_control_selection().orig_rqst != 2
    }

    /// What `+DS` selects.
    pub fn compression_selection(&self) -> CompressionSelection {
        CompressionSelection::parse(self.ds.as_bytes(), None).unwrap_or_default()
    }

    /// The data compression of a call.
    pub fn compression(&self) -> Compression {
        self.compression_selection().compression()
    }

    fn set_compression_direction(&mut self, direction: u8) {
        let mut selection = self.compression_selection();
        selection.direction = direction;
        self.ds = selection.to_string();
    }

    /// `\N<n>`, the command modems of the Microcom and Rockwell kinds
    /// have for error control, as the `+ES` it is: 0 is normal mode, with
    /// no error control; 1 direct mode; 2 reliable mode, error control or
    /// no call; 3 auto-reliable, error control if the far end has it; 4
    /// LAPM or no call; 5 MNP or no call, which here, with no MNP, is
    /// error control or no call.
    pub fn set_reliability(&mut self, n: u8) {
        self.es = match n {
            0 => "1,0,1",
            1 => "0,0,0",
            3 => "3,0,2",
            4 => "3,3,5",
            _ => "3,2,4",
        }
        .to_string();
    }

    /// `%C<n>`, the command of the same modems for compression, as the
    /// `+DS` it is: 0 is none; 1 is MNP 5, which this modem has not got,
    /// and so none; 2 is V.42bis; 3 is either, and so V.42bis.
    pub fn set_compression_mode(&mut self, n: u8) {
        self.set_compression_direction(if n >= 2 { 3 } else { 0 });
    }

    /// `&Q<n>`: 0 is direct asynchronous mode, 5 error control, 6
    /// asynchronous mode with buffering. The `+ES` each is.
    pub fn set_communications_mode(&mut self, n: u8) {
        self.es = match n {
            0 => "0,0,0",
            5 => "3,0,2",
            _ => "1,0,1",
        }
        .to_string();
    }

    /// What writing S36, S46 or S48 does to `+ES` and `+DS`. The
    /// register keeps the value written.
    ///
    /// - S36, what follows a failure to agree on error control: 0 and 4
    ///   hang up, which is error control required; 1, 3, 5 and 7 go on
    ///   without it. An `+ES` that has error control off stays off.
    /// - S46: 136 is LAPM without compression, 138 with it.
    /// - S48: 7 negotiates, with the detection phase; 0 goes to LAPM
    ///   without it; 128 does not negotiate, which is no error control.
    pub(crate) fn apply_register(&mut self, number: u32, value: u8) {
        let mut es = self.error_control_selection();
        match (number, value) {
            (36, 0 | 4) => {
                es.orig_fbk = 2;
                if es.ans_fbk >= 2 {
                    es.ans_fbk = 4;
                }
            }
            (36, _) => {
                es.orig_fbk = 0;
                if es.ans_fbk >= 2 {
                    es.ans_fbk = 2;
                }
            }
            (46, value) => self.set_compression_direction(if value == 138 { 3 } else { 0 }),
            (48, 0) if es.orig_rqst == 3 => es.orig_rqst = 2,
            (48, 7) if es.orig_rqst == 2 => es.orig_rqst = 3,
            (48, 128) => {
                es.orig_rqst = 1;
                es.ans_fbk = 1;
            }
            _ => {}
        }
        self.es = es.to_string();
    }
}

/// A carrier `+MS` can name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Carrier {
    /// `V21`: ITU-T V.21 (Table 13/V.250).
    V21,
    /// `V22`: ITU-T V.22.
    V22,
    /// `V22B`: ITU-T V.22bis.
    #[default]
    V22bis,
    /// `B103`: Bell 103.
    Bell103,
    /// `B212A`: Bell 212A.
    Bell212A,
}

impl Carrier {
    /// Every carrier.
    pub const ALL: [Carrier; 5] = [
        Carrier::V21,
        Carrier::V22,
        Carrier::V22bis,
        Carrier::Bell103,
        Carrier::Bell212A,
    ];

    /// The carrier as `+MS` spells it, and `+MCR` reports it.
    pub fn as_str(self) -> &'static str {
        match self {
            Carrier::V21 => "V21",
            Carrier::V22 => "V22",
            Carrier::V22bis => "V22B",
            Carrier::Bell103 => "B103",
            Carrier::Bell212A => "B212A",
        }
    }

    fn parse(text: &[u8]) -> Option<Carrier> {
        Carrier::ALL
            .into_iter()
            .find(|carrier| carrier.as_str().as_bytes() == text)
    }

    /// The modulation of the engine that the carrier is.
    pub fn modulation(self) -> Modulation {
        match self {
            Carrier::V21 => Modulation::V21,
            Carrier::V22 => Modulation::V22,
            Carrier::V22bis => Modulation::V22bis,
            Carrier::Bell103 => Modulation::Bell103,
            Carrier::Bell212A => Modulation::Bell212A,
        }
    }

    /// The carrier that a modulation of the engine is.
    pub fn of(modulation: Modulation) -> Carrier {
        match modulation {
            Modulation::V21 => Carrier::V21,
            Modulation::V22 => Carrier::V22,
            Modulation::V22bis => Carrier::V22bis,
            Modulation::Bell103 => Carrier::Bell103,
            Modulation::Bell212A => Carrier::Bell212A,
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
    /// each 0, for whatever the carrier has, or 300, 1200 or 2400.
    pub rates: [u16; 4],
}

impl Default for ModulationSelection {
    fn default() -> Self {
        ModulationSelection {
            carrier: Carrier::V22bis,
            automode: true,
            rates: [0; 4],
        }
    }
}

impl ModulationSelection {
    /// Whether a modulation is one the selection allows, as far as the
    /// selection goes; the line has its own say. Without automode it is
    /// the carrier and nothing else. With it, it is any whose rate is
    /// within the limits, which are the stricter of those for the two
    /// directions: the modems this engine has are the same rate both
    /// ways.
    pub fn allows(&self, modulation: Modulation) -> bool {
        if !self.automode {
            return modulation == self.carrier.modulation();
        }
        let [min, max, min_rx, max_rx] = self.rates.map(u32::from);
        let rate = modulation.rate();
        let over = |limit: u32| limit != 0 && rate > limit;
        rate >= min.max(min_rx) && !over(max) && !over(max_rx)
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
                Some(b"1200") => *rate = 1200,
                Some(b"2400") => *rate = 2400,
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

/// Reads the numeric subparameters of a compound value: those that are
/// there go into `fields` in their order, and those left out stay as
/// they are (V.250 clause 5.4.2.1). `None` if one is not a number, or
/// there are more than there are fields.
fn numbers<const N: usize>(value: &[u8], mut fields: [u32; N]) -> Option<[u32; N]> {
    let text = std::str::from_utf8(value).ok()?;
    for (n, field) in text.split(',').enumerate() {
        if field.is_empty() {
            continue;
        }
        *fields.get_mut(n)? = field.parse().ok()?;
    }
    Some(fields)
}

/// The value of `+ES` (V.250 clause 6.5.1, Table 20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ErrorControlSelection {
    /// `<orig_rqst>`, what an originating modem asks for: 0 direct mode,
    /// 1 buffered mode only, 2 V.42 without the detection phase, 3 V.42
    /// with it.
    pub orig_rqst: u8,
    /// `<orig_fbk>`, what an originating modem does if there is no error
    /// control: 0 and 1 go on without, 2 and 3 disconnect.
    pub orig_fbk: u8,
    /// `<ans_fbk>`, for an answering modem: 0 direct mode and 1 buffered
    /// mode, with error control disabled; 2 and 3 error control if the
    /// far end has it; 4 and 5 error control or disconnect.
    pub ans_fbk: u8,
}

impl Default for ErrorControlSelection {
    fn default() -> Self {
        // Table 20/V.250, the recommended defaults.
        ErrorControlSelection {
            orig_rqst: 3,
            orig_fbk: 0,
            ans_fbk: 2,
        }
    }
}

impl ErrorControlSelection {
    /// Reads a value. A subparameter left out is taken from `current`,
    /// or is the default if there is none. `None` if a subparameter is
    /// not a number or is one this modem has not got: the alternative
    /// protocol and the synchronous access mode are not.
    pub fn parse(
        value: &[u8],
        current: Option<ErrorControlSelection>,
    ) -> Option<ErrorControlSelection> {
        let current = current.unwrap_or_default();
        let fields = [current.orig_rqst, current.orig_fbk, current.ans_fbk].map(u32::from);
        let [orig_rqst, orig_fbk, ans_fbk] = numbers(value, fields)?;
        (orig_rqst <= 3 && orig_fbk <= 3 && ans_fbk <= 5).then_some(ErrorControlSelection {
            orig_rqst: orig_rqst as u8,
            orig_fbk: orig_fbk as u8,
            ans_fbk: ans_fbk as u8,
        })
    }

    /// The error control of a call this modem originates.
    pub fn originating(self) -> ErrorControl {
        match (self.orig_rqst, self.orig_fbk) {
            (0 | 1, _) => ErrorControl::Off,
            (_, 2 | 3) => ErrorControl::Required,
            _ => ErrorControl::Auto,
        }
    }

    /// The error control of a call this modem answers.
    pub fn answering(self) -> ErrorControl {
        match self.ans_fbk {
            0 | 1 => ErrorControl::Off,
            4 | 5 => ErrorControl::Required,
            _ => ErrorControl::Auto,
        }
    }
}

impl std::fmt::Display for ErrorControlSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{},{}", self.orig_rqst, self.orig_fbk, self.ans_fbk)
    }
}

/// The value of `+DS` (V.250 clause 6.6.1, Table 27).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompressionSelection {
    /// `<direction>`, from the host's point of view: 0 none, 1 what it
    /// sends, 2 what it receives, 3 both.
    pub direction: u8,
    /// `<compression_negotiation>`: 0, do not disconnect if compression
    /// is not agreed. The modem has no other.
    pub negotiation: u8,
    /// `<max_dict>`: the most codewords, 512 to 4096.
    pub max_dict: u16,
    /// `<max_string>`: the longest string, 6 to 250.
    pub max_string: u8,
}

impl Default for CompressionSelection {
    fn default() -> Self {
        CompressionSelection {
            direction: 3,
            negotiation: 0,
            max_dict: engine::CODEWORDS,
            max_string: engine::STRING_LENGTH,
        }
    }
}

impl CompressionSelection {
    /// Reads a value. A subparameter left out is taken from `current`,
    /// or is the default if there is none. `None` if a subparameter is
    /// not a number or is one this modem has not got.
    pub fn parse(
        value: &[u8],
        current: Option<CompressionSelection>,
    ) -> Option<CompressionSelection> {
        let current = current.unwrap_or_default();
        let fields = [
            u32::from(current.direction),
            u32::from(current.negotiation),
            u32::from(current.max_dict),
            u32::from(current.max_string),
        ];
        let [direction, negotiation, max_dict, max_string] = numbers(value, fields)?;
        let sound = direction <= 3
            && negotiation == 0
            && (512..=4096).contains(&max_dict)
            && (6..=250).contains(&max_string);
        sound.then_some(CompressionSelection {
            direction: direction as u8,
            negotiation: negotiation as u8,
            max_dict: max_dict as u16,
            max_string: max_string as u8,
        })
    }

    /// What the engine is asked for.
    pub fn compression(self) -> Compression {
        Compression {
            transmit: self.direction & 1 != 0,
            receive: self.direction & 2 != 0,
            codewords: self.max_dict,
            string_length: self.max_string,
        }
    }
}

impl std::fmt::Display for CompressionSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{},{},{},{}",
            self.direction, self.negotiation, self.max_dict, self.max_string
        )
    }
}

/// A start-stop character format the modem has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CharacterFormat {
    /// Eight data bits, no parity, one stop bit.
    N81,
    /// Seven data bits, even parity, one stop bit.
    E71,
    /// Seven data bits, odd parity, one stop bit.
    O71,
}

/// The value of `+ICF` (V.250 clause 6.2.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharacterFraming {
    /// `<format>` of Table 5/V.250: 3 is eight data bits and one stop
    /// bit, 5 is seven data bits, a parity bit and one stop bit. The
    /// modem has no others.
    pub format: u8,
    /// `<parity>`: 0 odd, 1 even, 2 mark, 3 space. With format 5 it is
    /// odd or even; with format 3 there is no parity bit and it is kept
    /// and means nothing.
    pub parity: u8,
}

impl CharacterFraming {
    /// Reads a value. A subparameter left out is taken from `current`,
    /// or is the default if there is none. `None` if a subparameter is
    /// not a number, is one the modem has not got, or there are too many.
    pub fn parse(value: &[u8], current: Option<CharacterFraming>) -> Option<CharacterFraming> {
        let mut framing = current.unwrap_or(CharacterFraming {
            format: 3,
            parity: 3,
        });
        let text = std::str::from_utf8(value).ok()?;
        for (n, field) in text.split(',').enumerate() {
            if field.is_empty() {
                continue;
            }
            let number: u8 = field.parse().ok()?;
            match n {
                0 => framing.format = number,
                1 => framing.parity = number,
                _ => return None,
            }
        }
        let sound = match framing.format {
            3 => framing.parity <= 3,
            5 => framing.parity <= 1,
            _ => false,
        };
        sound.then_some(framing)
    }

    /// The character format this selects.
    pub fn format(self) -> CharacterFormat {
        match (self.format, self.parity) {
            (5, 0) => CharacterFormat::O71,
            (5, _) => CharacterFormat::E71,
            _ => CharacterFormat::N81,
        }
    }
}

impl std::fmt::Display for CharacterFraming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.format, self.parity)
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
