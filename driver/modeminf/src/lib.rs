// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem INF of "RustModem Softmodem".
//!
//! Each RustModem line is a COM port with a modem attached to it
//! (`docs/DRIVER_SPIKE.md`). The modem has no driver of its own: its stack
//! is Windows' modem driver, and this INF is what tells that stack how to
//! talk to it. It says which commands initialise, dial, answer and hang
//! up, which command fragments set flow control, error control and
//! compression, and, in its Responses, every line the modem can answer
//! with and what each means: above all the rate of a connection, which is
//! what makes RRAS show the line's rate and not the port's.
//!
//! The INF is generated, and from the AT interpreter's own names and the
//! engine's own rates, so that the two cannot drift: the tests run the
//! interpreter through every kind of connection and fail on a line the
//! INF does not list.
//!
//! The grammar of a modem INF, and the ten octets of a response, are from
//! Microsoft's modem INF reference, "Modem INF File Sections" and
//! "Responses", which is archived under `previous-versions` on
//! learn.microsoft.com. Nothing of another INF is copied.
//!
//! The file made is an `.inx`: `stampinf` puts in the architecture, the
//! date and the version.

#![forbid(unsafe_code)]

use std::fmt::Write as _;

use at::{Carrier, MANUFACTURER, MODEL};
use engine::Modulation;

/// The hardware id the modem's device is made with and the INF matches.
pub const HARDWARE_ID: &str = "RUSTMODEM\\SOFTMODEM";

/// The rate of the port, which is no wire and has none: what the modem
/// stack is told to open it at.
pub const MAX_DTE_RATE: u32 = 115_200;

/// What a response means to Windows' modem stack: the first octet of a
/// Responses entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// `OK`.
    Ok = 0x00,
    /// A step of a connection being made, with what it says of it.
    Progress = 0x01,
    /// Connected.
    Connect = 0x02,
    /// `ERROR`.
    Error = 0x03,
    /// `NO CARRIER`.
    NoCarrier = 0x04,
    /// `NO DIALTONE`.
    NoDialtone = 0x05,
    /// `BUSY`.
    Busy = 0x06,
    /// `NO ANSWER`.
    NoAnswer = 0x07,
    /// `RING`.
    Ring = 0x08,
}

/// The second octet: what a progress or connect response says of the
/// connection.
pub mod options {
    /// Data compression was negotiated.
    pub const COMPRESSION: u8 = 0x01;
    /// Error control was negotiated.
    pub const ERROR_CONTROL: u8 = 0x02;
}

/// One line the modem can answer with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// The line as the modem sends it, with its carriage returns and line
    /// feeds.
    pub text: String,
    pub state: State,
    pub options: u8,
    /// The rate of the line, where the response says it; 0 where not.
    pub dce_rate: u32,
}

impl Response {
    /// The entry's ten octets: state, options, the line's rate, and the
    /// rate Windows should change the port to, which is never.
    pub fn octets(&self) -> [u8; 10] {
        let mut octets = [0; 10];
        octets[0] = self.state as u8;
        octets[1] = self.options;
        octets[2..6].copy_from_slice(&self.dce_rate.to_le_bytes());
        octets
    }

    /// The text as an INF spells it: `<cr>` and `<lf>` for the two
    /// characters, and `<<` for a `<`.
    pub fn key(&self) -> String {
        let mut key = String::new();
        for character in self.text.chars() {
            match character {
                '\r' => key.push_str("<cr>"),
                '\n' => key.push_str("<lf>"),
                '<' => key.push_str("<<"),
                other => key.push(other),
            }
        }
        key
    }
}

/// The rates of the modulations of later phases: V.32 and V.32bis, then
/// V.34, then V.90 downstream (ITU-T V.90 clause 5.4: 28000 bit/s and
/// every 8000/6 bit/s to 56000, reported in whole bits a second).
fn later_rates() -> Vec<u32> {
    let mut rates = vec![4800, 7200, 9600, 12_000, 14_400];
    // ITU-T V.34 clause 5: 2400 bit/s and every 2400 to 33600.
    rates.extend((1..=14).map(|n| n * 2400));
    rates.extend((0..=21).map(|n| 28_000 + n * 8000 / 6));
    rates
}

/// Every rate a connection may be reported at, in order: those of the
/// modulations the engine has, and those of the ones to come.
pub fn rates() -> Vec<u32> {
    let mut rates: Vec<u32> = Modulation::ALL.into_iter().map(Modulation::rate).collect();
    rates.extend(later_rates());
    rates.sort_unstable();
    rates.dedup();
    rates
}

/// The fastest rate the engine has today, which is what the modem says
/// it can do.
pub fn max_dce_rate() -> u32 {
    Modulation::ALL
        .into_iter()
        .map(Modulation::rate)
        .max()
        .unwrap_or(0)
}

/// Every line the modem can answer with.
pub fn responses() -> Vec<Response> {
    let mut all = Vec::new();
    let mut add = |text: String, state: State, options: u8, dce_rate: u32| {
        all.push(Response {
            text,
            state,
            options,
            dce_rate,
        });
    };
    // A result code in words is between two pairs of carriage return and
    // line feed; in numbers it is the number and a carriage return (V.250
    // clause 5.7.1). The INF's commands ask for words, and the numbers
    // are here for a host that has asked the modem for them.
    let words = |text: &str| format!("\r\n{text}\r\n");
    let basic = [
        (at::ResultCode::Ok, State::Ok),
        (at::ResultCode::Connect, State::Connect),
        (at::ResultCode::Ring, State::Ring),
        (at::ResultCode::NoCarrier, State::NoCarrier),
        (at::ResultCode::Error, State::Error),
        (at::ResultCode::NoDialtone, State::NoDialtone),
        (at::ResultCode::Busy, State::Busy),
        (at::ResultCode::NoAnswer, State::NoAnswer),
    ];
    for (code, state) in basic {
        add(words(code.text()), state, 0, 0);
        add(format!("{}\r", code.number()), state, 0, 0);
    }
    for rate in rates() {
        add(words(&format!("CONNECT {rate}")), State::Connect, 0, rate);
        // What +MR and +ILRR report before it.
        add(words(&format!("+MRR: {rate}")), State::Progress, 0, rate);
        add(words(&format!("+ILRR: {rate}")), State::Progress, 0, 0);
    }
    for carrier in Carrier::ALL {
        add(
            words(&format!("+MCR: {}", carrier.as_str())),
            State::Progress,
            0,
            0,
        );
    }
    // What +ER reports (V.250 clause 6.5.5) and what +DR does (6.6.2).
    add(
        words("+ER: LAPM"),
        State::Progress,
        options::ERROR_CONTROL,
        0,
    );
    add(words("+ER: NONE"), State::Progress, 0, 0);
    for compression in ["V42B", "V42B TD", "V42B RD"] {
        add(
            words(&format!("+DR: {compression}")),
            State::Progress,
            options::COMPRESSION,
            0,
        );
    }
    add(words("+DR: NONE"), State::Progress, 0, 0);
    all
}

/// The first initialisation command, which only finds the modem.
pub const INIT_1: &str = "AT";
/// The second: the factory profile, no echo, results in words, DCD
/// following the carrier, DTR hanging up, no auto-answer, every call
/// progress code, and the reports of a connection's rate, error control
/// and compression, which the Responses read.
pub const INIT_2: &str = "AT&FE0V1X4&C1&D2S0=0+MR=1;+ER=1;+DR=1";

/// The command fragments Windows puts after `AT` for what its dialogs
/// set. Each is a command the AT interpreter has, in the basic syntax,
/// which needs nothing between one command and the next.
pub const SETTINGS: &[(&str, &str)] = &[
    ("Prefix", "AT"),
    ("Terminator", "<cr>"),
    ("DialPrefix", "D"),
    ("DialSuffix", ""),
    ("Tone", "T"),
    ("Pulse", "P"),
    ("Blind_Off", "X4"),
    ("Blind_On", "X3"),
    ("CallSetupFailTimer", "S7=<#>"),
    ("FlowControl_Off", "&K0"),
    ("FlowControl_Hard", "&K3"),
    ("ErrorControl_On", "\\N3"),
    ("ErrorControl_Off", "\\N0"),
    ("ErrorControl_Forced", "\\N2"),
    ("Compression_On", "%C3"),
    ("Compression_Off", "%C0"),
    ("Modulation_CCITT", "B0"),
    ("Modulation_Bell", "B1"),
];

/// The modem's Properties: eight numbers of four octets.
pub fn properties() -> [u32; 8] {
    [
        // Dial options: none of wait-for-bong, quiet answer or dial tone.
        0,
        // The longest call set-up timer, which is S7's: 255 seconds.
        255,
        // No inactivity timer.
        0,
        // No speaker: its volumes, and its modes.
        0,
        0,
        // Modem options: compression, error control, forced error
        // control, hardware flow control, the choice of ITU or Bell
        // modulations, tone dialling and blind dialling.
        0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0040 | 0x0100 | 0x0200,
        MAX_DTE_RATE,
        max_dce_rate(),
    ]
}

fn octets_of(values: &[u8]) -> String {
    values
        .iter()
        .map(|octet| format!("{octet:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The INF, as an `.inx` for `stampinf`.
pub fn generate() -> String {
    let mut inf = String::new();
    let mut line = |text: &str| {
        inf.push_str(text);
        inf.push_str("\r\n");
    };
    line("; Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is");
    line("; licensed under the terms of the MIT license which can be found in the");
    line("; root directory of this project.");
    line(";");
    line("; RustModem Softmodem: the modem on a RustModem port. Generated by");
    line("; driver/modeminf from what the modem answers. Do not edit: change the");
    line("; generator.");
    line("");
    line("[Version]");
    line("Signature   = \"$Windows NT$\"");
    line("Class       = Modem");
    line("ClassGuid   = {4D36E96D-E325-11CE-BFC1-08002BE10318}");
    line("Provider    = %Provider%");
    line("CatalogFile = rmvcom.cat");
    line("DriverVer   = 01/01/2026,0.0.0.0");
    line("PnpLockdown = 1");
    line("");
    line("[Manufacturer]");
    line("%Provider% = Models,NT$ARCH$.10.0...17134");
    line("");
    line("[Models.NT$ARCH$.10.0...17134]");
    line(&format!("%Modem% = Modem, {HARDWARE_ID}"));
    line("");
    line("[Modem.NT]");
    line("AddReg = Modem.Identity, Modem.Commands, Modem.Responses");
    line("");
    line("; What Windows' modem stack is and where its dialogs are.");
    line("[Modem.Identity]");
    line("HKR,,FriendlyDriver,,unimodem.vxd");
    line("HKR,,DevLoader,,*vcomm");
    line("HKR,,ConfigDialog,,modemui.dll");
    line("HKR,,EnumPropPages,,\"modemui.dll,EnumPropPages\"");
    line("HKR,,PortSubClass,1,02");
    line("; An external modem: one on a COM port.");
    line("HKR,,DeviceType,1,01");
    line("HKR,,InactivityScale,1,0a,00,00,00");
    let properties: Vec<u8> = properties()
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    line(&format!("HKR,,Properties,1,{}", octets_of(&properties)));
    line("");
    line("[Modem.Commands]");
    line("HKR,,Reset,,\"AT&F<cr>\"");
    line(&format!("HKR,Init,1,,\"{INIT_1}<cr>\""));
    line(&format!("HKR,Init,2,,\"{INIT_2}<cr>\""));
    line("HKR,Monitor,1,,\"ATS0=0<cr>\"");
    line("HKR,Monitor,2,,\"None\"");
    line("HKR,Answer,1,,\"ATA<cr>\"");
    line("HKR,Hangup,1,,\"ATH<cr>\"");
    for (name, value) in SETTINGS {
        // A per cent sign begins a string's name in an INF, and is
        // itself when written twice.
        let value = value.replace('%', "%%");
        line(&format!("HKR,Settings,{name},,\"{value}\""));
    }
    line("");
    line("; State, options, the line's rate, the port's rate.");
    line("[Modem.Responses]");
    for response in responses() {
        let mut entry = String::new();
        let _ = write!(
            entry,
            "HKR,Responses,\"{}\",1,{}",
            response.key(),
            octets_of(&response.octets())
        );
        line(&entry);
    }
    line("");
    line("[Strings]");
    line(&format!("Provider = \"{MANUFACTURER}\""));
    line(&format!("Modem    = \"{MODEL}\""));
    inf
}
