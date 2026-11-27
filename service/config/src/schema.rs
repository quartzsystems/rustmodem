// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The configuration as the rest of the service uses it.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};

/// The highest line id.
pub const MAX_LINE_ID: u16 = 999;

/// How much the service logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Errors and warnings.
    Warning,
    /// Errors, warnings and information.
    #[default]
    Info,
    /// Everything, including every AT command line and result code.
    Debug,
}

impl LogLevel {
    /// Every level, least verbose first.
    pub const ALL: [LogLevel; 4] = [
        LogLevel::Error,
        LogLevel::Warning,
        LogLevel::Info,
        LogLevel::Debug,
    ];

    /// The level as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warning => "warning",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    /// Reads a level as the configuration spells it.
    pub fn parse(text: &str) -> Option<LogLevel> {
        LogLevel::ALL.into_iter().find(|l| l.as_str() == text)
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which serial backend a line uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Backend {
    /// One side of a com0com null-modem pair.
    #[default]
    Com0com,
    /// A RustModem virtual COM port, of the driver in `driver/`.
    Vcom,
}

impl Backend {
    /// The backend as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Com0com => "com0com",
            Backend::Vcom => "vcom",
        }
    }

    /// Reads a backend as the configuration spells it.
    pub fn parse(text: &str) -> Option<Backend> {
        match text {
            "com0com" => Some(Backend::Com0com),
            "vcom" => Some(Backend::Vcom),
            _ => None,
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a line answers to an AT command it does not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum UnknownCommand {
    /// Skip the command and carry on, as if it had worked.
    #[default]
    Ok,
    /// Stop the command line with `ERROR`.
    Error,
}

impl UnknownCommand {
    /// The setting as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            UnknownCommand::Ok => "ok",
            UnknownCommand::Error => "error",
        }
    }

    /// Reads the setting as the configuration spells it.
    pub fn parse(text: &str) -> Option<UnknownCommand> {
        match text {
            "ok" => Some(UnknownCommand::Ok),
            "error" => Some(UnknownCommand::Error),
            _ => None,
        }
    }
}

impl fmt::Display for UnknownCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `[service]` table.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ServiceConfig {
    /// How much the service logs.
    pub log_level: LogLevel,
    /// The log directory; empty means the default one.
    pub log_dir: String,
}

/// The `[control]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlConfig {
    /// The control pipe is `\\.\pipe\<pipe>`.
    pub pipe: String,
}

impl ControlConfig {
    /// The default pipe name.
    pub const DEFAULT_PIPE: &'static str = "rustmodem-control";
}

impl Default for ControlConfig {
    fn default() -> Self {
        ControlConfig {
            pipe: ControlConfig::DEFAULT_PIPE.to_string(),
        }
    }
}

/// The `serial` table of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialConfig {
    /// Which backend opens the port.
    pub backend: Backend,
    /// The port name as the backend knows it, for example `CNCB0`.
    pub port: String,
}

/// One `[[line]]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineConfig {
    /// The line id, 1 to [`MAX_LINE_ID`], unique.
    pub id: u16,
    /// A name for people; optional.
    pub name: Option<String>,
    /// The serial port of the line.
    pub serial: SerialConfig,
    /// Whether the line is in service.
    pub enabled: bool,
    /// The initial value of S0: rings before auto-answer, 0 for off.
    pub auto_answer_rings: u8,
    /// What the line answers to an AT command it does not know.
    pub unknown_command: UnknownCommand,
    /// Which modem engine the line runs.
    pub engine: EngineKind,
    /// The settings of the modem engine that the line has its own value
    /// for.
    pub modem: LineModem,
    /// The line's registration to a PBX, if it is an extension of one.
    pub register: Option<RegisterConfig>,
}

impl LineConfig {
    /// A line with the given id and port and every other key at its default.
    pub fn new(id: u16, port: &str) -> LineConfig {
        LineConfig {
            id,
            name: None,
            serial: SerialConfig {
                backend: Backend::default(),
                port: port.to_string(),
            },
            enabled: true,
            auto_answer_rings: 0,
            unknown_command: UnknownCommand::default(),
            engine: EngineKind::default(),
            modem: LineModem::default(),
            register: None,
        }
    }
}

/// A change to one line: each `Some` replaces that key, each `None` leaves
/// it alone. For `name`, `Some(None)` removes the key.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineEdit {
    /// A new name, or `Some(None)` to remove the name.
    pub name: Option<Option<String>>,
    /// A new serial backend.
    pub backend: Option<Backend>,
    /// A new serial port.
    pub port: Option<String>,
    /// Whether the line is in service.
    pub enabled: Option<bool>,
    /// A new initial S0.
    pub auto_answer_rings: Option<u8>,
    /// A new unknown-command setting.
    pub unknown_command: Option<UnknownCommand>,
    /// A new modem engine.
    pub engine: Option<EngineKind>,
    /// A new registration, whole, or `Some(None)` to remove the
    /// registration.
    pub register: Option<Option<RegisterConfig>>,
    /// The line's own modem settings, whole: a key that is `None` is
    /// taken from `[modem]` from now on.
    pub modem: Option<LineModemEdit>,
}

/// A line's own modem settings as they are to be written: checked when
/// the change is made.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineModemEdit {
    /// The modulations as the configuration spells them.
    pub modulations: Option<Vec<String>>,
    pub v8: Option<bool>,
    pub tx_level_dbm0: Option<i64>,
    /// The policy for error control as the configuration spells it.
    pub error_control: Option<String>,
    pub compression: Option<bool>,
    /// The guard tone as the configuration spells it.
    pub guard_tone: Option<String>,
}

/// A change to `[modem]`: each `Some` replaces that key. The values are
/// checked when the change is made.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ModemEdit {
    /// The modulations as the configuration spells them.
    pub modulations: Option<Vec<String>>,
    pub v8: Option<bool>,
    pub tx_level_dbm0: Option<i64>,
    pub answer_delay_ms: Option<u32>,
    pub fallback_ms: Option<u32>,
    /// The policy for error control as the configuration spells it.
    pub error_control: Option<String>,
    pub compression: Option<bool>,
    /// The guard tone as the configuration spells it.
    pub guard_tone: Option<String>,
}

impl ModemEdit {
    /// Whether the edit changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == ModemEdit::default()
    }
}

/// A change to the settings that are not lines: each `Some` replaces that
/// key, each `None` leaves it alone.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SettingsEdit {
    /// A new `service.log_level`.
    pub log_level: Option<LogLevel>,
    /// A new `service.log_dir`; empty means the default directory.
    pub log_dir: Option<String>,
    /// A new `control.pipe`.
    pub pipe: Option<String>,
    /// Changes to `[sip]`.
    pub sip: SipEdit,
    /// Changes to `[media]`.
    pub media: MediaEdit,
    /// Changes to `[modem]`.
    pub modem: ModemEdit,
}

/// A change to `[sip]`: each `Some` replaces that key.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SipEdit {
    pub enabled: Option<bool>,
    /// As `address:port`; checked when the change is made.
    pub listen: Option<String>,
    pub address: Option<String>,
    pub user_agent: Option<String>,
    /// Addresses or CIDR blocks; checked when the change is made.
    pub trusted_peers: Option<Vec<String>>,
    pub outbound_peer: Option<String>,
    pub ring_timeout_s: Option<u32>,
}

impl SipEdit {
    /// Whether the edit changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == SipEdit::default()
    }
}

/// A change to `[media]`: each `Some` replaces that key.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MediaEdit {
    /// The codecs as the configuration spells them; checked when the
    /// change is made.
    pub codecs: Option<Vec<String>>,
    pub ptime_ms: Option<u32>,
    /// As `first-last`; checked when the change is made.
    pub rtp_ports: Option<String>,
    pub rtp_timeout_s: Option<u32>,
    pub capture_dir: Option<String>,
}

impl MediaEdit {
    /// Whether the edit changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == MediaEdit::default()
    }
}

/// A change to a hunt group: each `Some` replaces that key.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct HuntGroupEdit {
    /// A new name. The routes to the group follow it.
    pub name: Option<String>,
    pub pilot: Option<String>,
    pub lines: Option<Vec<u16>>,
    /// The strategy as the configuration spells it; checked when the
    /// change is made.
    pub strategy: Option<String>,
}

/// A change to a route: each `Some` replaces that key.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RouteEdit {
    pub called: Option<String>,
    /// As `line:<id>` or `group:<name>`; checked when the change is made.
    pub target: Option<String>,
}

/// A whole, valid configuration.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Config {
    /// The `[service]` table.
    pub service: ServiceConfig,
    /// The `[control]` table.
    pub control: ControlConfig,
    /// The `[[line]]` tables, in file order.
    pub lines: Vec<LineConfig>,
    /// The `[sip]` table.
    pub sip: SipConfig,
    /// The `[media]` table.
    pub media: MediaConfig,
    /// The `[modem]` table.
    pub modem: ModemConfig,
    /// The `[[hunt_group]]` tables, in file order.
    pub hunt_groups: Vec<HuntGroup>,
    /// The `[[route]]` tables, in file order: the order they are tried in.
    pub routes: Vec<Route>,
}

impl Config {
    /// The line with the given id.
    pub fn line(&self, id: u16) -> Option<&LineConfig> {
        self.lines.iter().find(|l| l.id == id)
    }

    /// The hunt group with the given name.
    pub fn hunt_group(&self, name: &str) -> Option<&HuntGroup> {
        self.hunt_groups.iter().find(|g| g.name == name)
    }

    /// The names of the hunt groups a line is in, in file order.
    pub fn hunt_groups_of(&self, line: u16) -> Vec<&str> {
        self.hunt_groups
            .iter()
            .filter(|g| g.lines.contains(&line))
            .map(|g| g.name.as_str())
            .collect()
    }
}

/// Which modem engine a line runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum EngineKind {
    /// The modem engine: a softmodem.
    #[default]
    Modem,
    /// The null engine: data framed straight into the octet stream.
    Null,
    /// The echo engine: what is received is sent back.
    Echo,
}

impl EngineKind {
    /// Every engine of this phase.
    pub const ALL: [EngineKind; 3] = [EngineKind::Modem, EngineKind::Null, EngineKind::Echo];

    /// The engine as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::Modem => "modem",
            EngineKind::Null => "null",
            EngineKind::Echo => "echo",
        }
    }

    /// Reads an engine as the configuration spells it.
    pub fn parse(text: &str) -> Option<EngineKind> {
        EngineKind::ALL.into_iter().find(|e| e.as_str() == text)
    }
}

impl fmt::Display for EngineKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A codec of `media.codecs`. Both are G.711.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodecName {
    /// G.711 mu-law.
    Pcmu,
    /// G.711 A-law.
    Pcma,
}

impl CodecName {
    /// Every codec, in the default order of preference.
    pub const ALL: [CodecName; 2] = [CodecName::Pcmu, CodecName::Pcma];

    /// The codec as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            CodecName::Pcmu => "pcmu",
            CodecName::Pcma => "pcma",
        }
    }

    /// Reads a codec as the configuration spells it.
    pub fn parse(text: &str) -> Option<CodecName> {
        CodecName::ALL.into_iter().find(|c| c.as_str() == text)
    }
}

impl fmt::Display for CodecName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a hunt group chooses among its idle lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Strategy {
    /// The first idle line in the order the group lists them.
    #[default]
    FirstIdle,
    /// The first idle line after the one that took the group's last call.
    RoundRobin,
    /// The line that has been idle longest.
    LeastRecent,
}

impl Strategy {
    /// Every strategy.
    pub const ALL: [Strategy; 3] = [
        Strategy::FirstIdle,
        Strategy::RoundRobin,
        Strategy::LeastRecent,
    ];

    /// The strategy as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::FirstIdle => "first-idle",
            Strategy::RoundRobin => "round-robin",
            Strategy::LeastRecent => "least-recent",
        }
    }

    /// Reads a strategy as the configuration spells it.
    pub fn parse(text: &str) -> Option<Strategy> {
        Strategy::ALL.into_iter().find(|s| s.as_str() == text)
    }
}

impl fmt::Display for Strategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An address or a block of addresses, as `sip.trusted_peers` lists them:
/// `10.10.20.11` or `10.10.20.0/24`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cidr {
    /// The address as written.
    pub address: Ipv4Addr,
    /// How many leading bits must match: 32 for one address.
    pub prefix: u8,
}

impl Cidr {
    /// Reads an address or a CIDR block.
    pub fn parse(text: &str) -> Option<Cidr> {
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => {
                if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                (address, prefix.parse().ok().filter(|p| *p <= 32)?)
            }
            None => (text, 32),
        };
        Some(Cidr {
            address: address.parse().ok()?,
            prefix,
        })
    }

    /// Whether `address` is in the block.
    pub fn contains(&self, address: Ipv4Addr) -> bool {
        let mask = match self.prefix {
            0 => 0,
            bits => u32::MAX << (32 - u32::from(bits)),
        };
        u32::from(self.address) & mask == u32::from(address) & mask
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.prefix == 32 {
            write!(f, "{}", self.address)
        } else {
            write!(f, "{}/{}", self.address, self.prefix)
        }
    }
}

/// The `[sip]` table. With no such table SIP is off and the service is as
/// it was before it had SIP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SipConfig {
    /// Whether the service speaks SIP at all.
    pub enabled: bool,
    /// The UDP address the trunk listener binds.
    pub listen: SocketAddrV4,
    /// The address to advertise in Contact and in session descriptions;
    /// empty for the local address that routes to the peer.
    pub address: String,
    /// The User-Agent header; empty for `rustmodem/<version>`.
    pub user_agent: String,
    /// The addresses that may send requests to the trunk listener.
    pub trusted_peers: Vec<Cidr>,
    /// Where lines that are not registered send the calls they dial, as
    /// `host` or `host:port`; empty for nowhere.
    pub outbound_peer: String,
    /// How long a line may ring before the caller is turned away.
    pub ring_timeout_s: u32,
}

impl SipConfig {
    /// The default trunk listener.
    pub const DEFAULT_LISTEN: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 5060);
    /// The default `ring_timeout_s`.
    pub const DEFAULT_RING_TIMEOUT_S: u32 = 60;
}

impl Default for SipConfig {
    fn default() -> Self {
        SipConfig {
            enabled: false,
            listen: SipConfig::DEFAULT_LISTEN,
            address: String::new(),
            user_agent: String::new(),
            trusted_peers: Vec::new(),
            outbound_peer: String::new(),
            ring_timeout_s: SipConfig::DEFAULT_RING_TIMEOUT_S,
        }
    }
}

/// The ports RTP may use, both ends included. Only the even ones are used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortRange {
    /// The lowest port.
    pub first: u16,
    /// The highest port.
    pub last: u16,
}

impl PortRange {
    /// Reads `first-last`. The range must hold an even port and no port
    /// below 1024.
    pub fn parse(text: &str) -> Option<PortRange> {
        let (first, last) = text.split_once('-')?;
        let number = |text: &str| -> Option<u16> {
            (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
                .then(|| text.parse().ok())
                .flatten()
        };
        let range = PortRange {
            first: number(first)?,
            last: number(last)?,
        };
        (range.first >= 1024 && range.first <= range.last && range.even_ports().next().is_some())
            .then_some(range)
    }

    /// The even ports of the range, lowest first.
    pub fn even_ports(&self) -> impl Iterator<Item = u16> + use<> {
        (self.first..=self.last).filter(|port| port % 2 == 0)
    }
}

impl fmt::Display for PortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.first, self.last)
    }
}

/// The `[media]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaConfig {
    /// The codecs, most preferred first.
    pub codecs: Vec<CodecName>,
    /// The packet time in milliseconds: 10, 20 or 30.
    pub ptime_ms: u32,
    /// The ports RTP may use.
    pub rtp_ports: PortRange,
    /// How long a call may go without a packet before it is cleared.
    pub rtp_timeout_s: u32,
    /// Where call captures are written; empty for `captures` under the log
    /// directory.
    pub capture_dir: String,
}

impl MediaConfig {
    /// The packet times a configuration may name, in milliseconds.
    pub const PTIMES: [u32; 3] = [10, 20, 30];
}

impl Default for MediaConfig {
    fn default() -> Self {
        MediaConfig {
            codecs: CodecName::ALL.to_vec(),
            ptime_ms: 20,
            rtp_ports: PortRange {
                first: 20000,
                last: 20999,
            },
            rtp_timeout_s: 10,
            capture_dir: String::new(),
        }
    }
}

/// A modulation the modem engine has, as the configuration spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModulationName {
    /// ITU-T V.22bis: 2400 bit/s.
    V22bis,
    /// ITU-T V.22: 1200 bit/s.
    V22,
    /// Bell 212A: 1200 bit/s.
    Bell212A,
    /// ITU-T V.21: 300 bit/s.
    V21,
    /// Bell 103: 300 bit/s.
    Bell103,
}

impl ModulationName {
    /// Every modulation, in the default fallback order: the fastest
    /// first.
    pub const ALL: [ModulationName; 5] = [
        ModulationName::V22bis,
        ModulationName::V22,
        ModulationName::Bell212A,
        ModulationName::V21,
        ModulationName::Bell103,
    ];

    /// What a message says the modulations are.
    pub const LIST: &'static str = "v22bis, v22, bell212a, v21 and bell103";

    /// The modulation as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            ModulationName::V22bis => "v22bis",
            ModulationName::V22 => "v22",
            ModulationName::Bell212A => "bell212a",
            ModulationName::V21 => "v21",
            ModulationName::Bell103 => "bell103",
        }
    }

    /// Reads a modulation as the configuration spells it.
    pub fn parse(text: &str) -> Option<ModulationName> {
        ModulationName::ALL.into_iter().find(|m| m.as_str() == text)
    }
}

impl fmt::Display for ModulationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A line's policy for error control, on a modulation that can have it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ErrorControlName {
    /// LAPM if the far end has it, and none if not.
    #[default]
    Auto,
    /// LAPM, or the call is cleared.
    Required,
    /// None.
    Off,
}

impl ErrorControlName {
    /// Every policy.
    pub const ALL: [ErrorControlName; 3] = [
        ErrorControlName::Auto,
        ErrorControlName::Required,
        ErrorControlName::Off,
    ];

    /// What a message says the policies are.
    pub const LIST: &'static str = "auto, required or off";

    /// The policy as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorControlName::Auto => "auto",
            ErrorControlName::Required => "required",
            ErrorControlName::Off => "off",
        }
    }

    /// Reads a policy as the configuration spells it.
    pub fn parse(text: &str) -> Option<ErrorControlName> {
        ErrorControlName::ALL
            .into_iter()
            .find(|p| p.as_str() == text)
    }
}

impl fmt::Display for ErrorControlName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The guard tone an answering modem sends with its data at 1200 and
/// 2400 bit/s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GuardToneName {
    /// None.
    #[default]
    Off,
    /// 1800 Hz.
    Hz1800,
    /// 550 Hz.
    Hz550,
}

impl GuardToneName {
    /// Every guard tone.
    pub const ALL: [GuardToneName; 3] = [
        GuardToneName::Off,
        GuardToneName::Hz1800,
        GuardToneName::Hz550,
    ];

    /// What a message says the guard tones are.
    pub const LIST: &'static str = "off, 1800 or 550";

    /// The guard tone as the configuration spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            GuardToneName::Off => "off",
            GuardToneName::Hz1800 => "1800",
            GuardToneName::Hz550 => "550",
        }
    }

    /// Reads a guard tone as the configuration spells it.
    pub fn parse(text: &str) -> Option<GuardToneName> {
        GuardToneName::ALL.into_iter().find(|g| g.as_str() == text)
    }
}

impl fmt::Display for GuardToneName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `[modem]` table: the settings of the modem engine, for every line
/// that does not say otherwise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModemConfig {
    /// The modulations allowed, in the order the fallback tries them.
    pub modulations: Vec<ModulationName>,
    /// Whether V.8 is used.
    pub v8: bool,
    /// The level sent, in dBm0: -20 to -9.
    pub tx_level_dbm0: i8,
    /// The silence before the answer tone, in milliseconds; 0 for the
    /// value of the Recommendation.
    pub answer_delay_ms: u32,
    /// How long each step of the fallback lasts, in milliseconds: the
    /// answer sequence of V.22bis, and each 300 bit/s modem's answer
    /// carrier.
    pub fallback_ms: u32,
    /// The policy for error control at 1200 and 2400 bit/s.
    pub error_control: ErrorControlName,
    /// Whether V.42bis is asked for and agreed to.
    pub compression: bool,
    /// The guard tone sent when answering at 1200 and 2400 bit/s.
    pub guard_tone: GuardToneName,
}

impl ModemConfig {
    /// The transmit levels a configuration may name, in dBm0.
    pub const TX_LEVELS: std::ops::RangeInclusive<i64> = -20..=-9;
    /// The answer delays a configuration may name, in milliseconds.
    pub const ANSWER_DELAYS: std::ops::RangeInclusive<i64> = 0..=10_000;
    /// The fallback times a configuration may name, in milliseconds.
    pub const FALLBACK_TIMES: std::ops::RangeInclusive<i64> = 500..=20_000;

    /// These settings with a line's own in place of them where it has
    /// any.
    pub fn for_line(&self, line: &LineModem) -> ModemConfig {
        ModemConfig {
            modulations: line
                .modulations
                .clone()
                .unwrap_or_else(|| self.modulations.clone()),
            v8: line.v8.unwrap_or(self.v8),
            tx_level_dbm0: line.tx_level_dbm0.unwrap_or(self.tx_level_dbm0),
            answer_delay_ms: self.answer_delay_ms,
            fallback_ms: self.fallback_ms,
            error_control: line.error_control.unwrap_or(self.error_control),
            compression: line.compression.unwrap_or(self.compression),
            guard_tone: line.guard_tone.unwrap_or(self.guard_tone),
        }
    }
}

impl Default for ModemConfig {
    fn default() -> Self {
        ModemConfig {
            modulations: ModulationName::ALL.to_vec(),
            v8: true,
            tx_level_dbm0: -12,
            answer_delay_ms: 0,
            fallback_ms: 3000,
            error_control: ErrorControlName::Auto,
            compression: true,
            guard_tone: GuardToneName::Off,
        }
    }
}

/// The `modem` table of a line: the settings of `[modem]` that this line
/// has its own value for. A `None` is the value of `[modem]`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineModem {
    /// The modulations allowed, in fallback order.
    pub modulations: Option<Vec<ModulationName>>,
    /// Whether V.8 is used.
    pub v8: Option<bool>,
    /// The level sent, in dBm0.
    pub tx_level_dbm0: Option<i8>,
    /// The policy for error control.
    pub error_control: Option<ErrorControlName>,
    /// Whether V.42bis is asked for and agreed to.
    pub compression: Option<bool>,
    /// The guard tone sent when answering at 1200 and 2400 bit/s.
    pub guard_tone: Option<GuardToneName>,
}

impl LineModem {
    /// Whether the line has no setting of its own.
    pub fn is_empty(&self) -> bool {
        *self == LineModem::default()
    }
}

/// The `register` table of a line: the line is an extension of a PBX.
#[derive(Clone, PartialEq, Eq)]
pub struct RegisterConfig {
    /// The registrar, as `host` or `host:port`.
    pub server: String,
    /// The extension: the user of the address of record.
    pub user: String,
    /// The password. Never shown: not by the control API, not in a log.
    pub password: String,
    /// The user name to authenticate with; empty for `user`.
    pub auth_user: String,
    /// The registration time to ask for, in seconds.
    pub expires_s: u32,
}

impl RegisterConfig {
    /// The default `expires_s`.
    pub const DEFAULT_EXPIRES_S: u32 = 300;
}

impl fmt::Debug for RegisterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterConfig")
            .field("server", &self.server)
            .field("user", &self.user)
            .field("has_password", &!self.password.is_empty())
            .field("auth_user", &self.auth_user)
            .field("expires_s", &self.expires_s)
            .finish()
    }
}

/// One `[[hunt_group]]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HuntGroup {
    /// The name, unique.
    pub name: String,
    /// The number that calls the group, unique.
    pub pilot: String,
    /// The lines of the group, in hunting order.
    pub lines: Vec<u16>,
    /// How the group chooses among its idle lines.
    pub strategy: Strategy,
}

/// Where a route sends a call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// `line:<id>`.
    Line(u16),
    /// `group:<name>`.
    Group(String),
}

impl Target {
    /// Reads `line:<id>` or `group:<name>`.
    pub fn parse(text: &str) -> Option<Target> {
        match text.split_once(':')? {
            ("line", id) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => {
                id.parse().ok().map(Target::Line)
            }
            ("group", name) if !name.is_empty() => Some(Target::Group(name.to_string())),
            _ => None,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Line(id) => write!(f, "line:{id}"),
            Target::Group(name) => write!(f, "group:{name}"),
        }
    }
}

/// One `[[route]]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// The called number the route is for: exact, or a prefix ending in
    /// `*`.
    pub called: String,
    /// Where its calls go.
    pub target: Target,
}

impl Route {
    /// Whether the route is for this called number.
    pub fn matches(&self, number: &str) -> bool {
        pattern_matches(&self.called, number)
    }
}

/// Whether a called-number pattern, exact or a prefix ending in `*`,
/// matches a number.
pub fn pattern_matches(pattern: &str, number: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => number.starts_with(prefix),
        None => pattern == number,
    }
}

/// Whether every number `later` matches is matched by `earlier` too, so
/// that a route for `later` after one for `earlier` never gets a call.
pub fn pattern_covers(earlier: &str, later: &str) -> bool {
    match (earlier.strip_suffix('*'), later.strip_suffix('*')) {
        (Some(prefix), Some(longer)) => longer.starts_with(prefix),
        (Some(prefix), None) => later.starts_with(prefix),
        (None, None) => earlier == later,
        (None, Some(_)) => false,
    }
}
