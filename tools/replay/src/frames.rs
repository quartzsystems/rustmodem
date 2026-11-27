// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The frames of LAPM as text: what a frame the engine's data link sent
//! or received is, by its address and control fields (V.42 clause 8.2).

use std::fmt::Write as _;

use engine::TappedFrame;
use v42::xid::Parameters;

use crate::Role;

// The address of the answerer, DLCI 0 with the C/R bit set, and of the
// originator (V.42 clause 8.2.1 and Table 6). A command carries the
// address of the end it is sent to.
const ANSWERER: u8 = 0x03;
const ORIGINATOR: u8 = 0x01;

// The control fields of the supervisory frames and, with the P/F bit
// taken out, of the unnumbered ones (V.42 clause 8.2.4 and Table 8).
const RR: u8 = 0x01;
const RNR: u8 = 0x05;
const REJ: u8 = 0x09;
const SREJ: u8 = 0x0D;
const SABME: u8 = 0x6F;
const DM: u8 = 0x0F;
const UI: u8 = 0x03;
const DISC: u8 = 0x43;
const UA: u8 = 0x63;
const FRMR: u8 = 0x87;
const XID: u8 = 0xAF;
const TEST: u8 = 0xE3;
const U_PF: u8 = 0x10;

// The messages of a UI frame (V.42 Table 12), bit 8 being their sequence
// number (clause 8.13.2.1).
const BRK: u8 = 0x40;
const BRKACK: u8 = 0x60;
const BREAK_SEQUENCE: u8 = 0x80;

/// What a frame is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    /// The frame in a line: its kind, whether a command or a response,
    /// its numbers and its poll or final bit, and the length of its
    /// information field.
    pub text: String,
    /// What an XID frame asks for or answers, where the frame is one and
    /// its information field is of V.42's format (clause 12.2).
    pub xid: Option<String>,
}

/// The parameters of an XID frame in a line.
fn parameters(parameters: &Parameters) -> String {
    let mut text = format!(
        "n401 {}/{} k {}/{}",
        parameters.n401_initiator,
        parameters.n401_responder,
        parameters.k_initiator,
        parameters.k_responder
    );
    match parameters.compression {
        None => text.push_str(" compression none"),
        Some(compression) => {
            let ways = match (
                compression.initiator_to_responder,
                compression.responder_to_initiator,
            ) {
                (true, true) => "both",
                (true, false) => "initiator-to-responder",
                (false, true) => "responder-to-initiator",
                (false, false) => "neither",
            };
            let _ = write!(
                text,
                " compression {ways} codewords {} string {}",
                compression.codewords, compression.string_length
            );
        }
    }
    text
}

/// Describes a frame of a call in which the engine was `role`.
pub fn describe(frame: &TappedFrame, role: Role) -> Described {
    let way = if frame.sent { "tx" } else { "rx" };
    let plain = |text: String| Described { text, xid: None };
    let [address, control, rest @ ..] = frame.kept() else {
        return plain(format!("{way} short len={}", frame.len));
    };
    // The end the frame was going to.
    let to_answerer = frame.sent == (role == Role::Originate);
    let kind = match (*address, to_answerer) {
        (ANSWERER, true) | (ORIGINATOR, false) => "command",
        (ANSWERER, false) | (ORIGINATOR, true) => "response",
        _ => {
            return plain(format!("{way} address {address:#04x} len={}", frame.len));
        }
    };
    // The poll bit of a command is the final bit of a response.
    let bit = if kind == "command" { "p" } else { "f" };
    let whole = usize::from(frame.len);
    if control & 1 == 0 {
        // An I frame: N(S), and in the next octet N(R) and P (clause
        // 8.2.3.1).
        let Some(second) = rest.first() else {
            return plain(format!("{way} short len={}", frame.len));
        };
        return plain(format!(
            "{way} I {kind} ns={} nr={} {bit}={} len={}",
            control >> 1,
            second >> 1,
            second & 1,
            whole - 3
        ));
    }
    if control & 3 == 1 {
        let name = match *control {
            RR => "RR",
            RNR => "RNR",
            REJ => "REJ",
            SREJ => "SREJ",
            _ => "S?",
        };
        let Some(second) = rest.first() else {
            return plain(format!("{way} short len={}", frame.len));
        };
        return plain(format!(
            "{way} {name} {kind} nr={} {bit}={}",
            second >> 1,
            second & 1
        ));
    }
    let pf = u8::from(control & U_PF != 0);
    let name = match control & !U_PF {
        SABME => "SABME",
        DM => "DM",
        UI => "UI",
        DISC => "DISC",
        UA => "UA",
        FRMR => "FRMR",
        XID => "XID",
        TEST => "TEST",
        _ => "U?",
    };
    let mut text = format!("{way} {name} {kind} {bit}={pf}");
    if whole > 2 {
        let _ = write!(text, " len={}", whole - 2);
    }
    let mut xid = None;
    match control & !U_PF {
        UI => {
            if let Some(first) = rest.first() {
                let sequence = u8::from(first & BREAK_SEQUENCE != 0);
                match first & !BREAK_SEQUENCE {
                    BRK => {
                        let _ = write!(text, " break n={sequence}");
                    }
                    BRKACK => {
                        let _ = write!(text, " break-acknowledged n={sequence}");
                    }
                    _ => {}
                }
            }
        }
        XID => {
            xid = Parameters::decode(rest)
                .map(|decoded| format!("{way} {kind} {}", parameters(&decoded)));
        }
        _ => {}
    }
    Described { text, xid }
}

#[cfg(test)]
mod tests {
    use engine::TAP_OCTETS;

    use super::*;

    fn frame(sent: bool, octets: &[u8], len: usize) -> TappedFrame {
        let mut kept = [0u8; TAP_OCTETS];
        kept[..octets.len()].copy_from_slice(octets);
        TappedFrame {
            sent,
            len: len as u16,
            octets: kept,
        }
    }

    fn text(sent: bool, octets: &[u8], role: Role) -> String {
        describe(&frame(sent, octets, octets.len()), role).text
    }

    #[test]
    fn a_frame_is_a_command_by_the_address_of_the_end_it_goes_to() {
        // The answerer is sent SABME with its own address, and answers UA
        // with its own.
        assert_eq!(
            text(false, &[0x03, 0x7F], Role::Answer),
            "rx SABME command p=1"
        );
        assert_eq!(
            text(true, &[0x03, 0x73], Role::Answer),
            "tx UA response f=1"
        );
        assert_eq!(
            text(true, &[0x03, 0x7F], Role::Originate),
            "tx SABME command p=1"
        );
        assert_eq!(
            text(false, &[0x03, 0x73], Role::Originate),
            "rx UA response f=1"
        );
        // The answerer's commands carry the originator's address.
        assert_eq!(
            text(true, &[0x01, 0x53], Role::Answer),
            "tx DISC command p=1"
        );
        assert_eq!(
            text(false, &[0x01, 0x1F], Role::Answer),
            "rx DM response f=1"
        );
        assert_eq!(
            text(false, &[0x05, 0x7F], Role::Answer),
            "rx address 0x05 len=2"
        );
        assert_eq!(text(false, &[0x03], Role::Answer), "rx short len=1");
    }

    #[test]
    fn the_numbers_of_i_and_supervisory_frames_are_read() {
        // N(S) 5, N(R) 3, no poll, and seven octets of information of
        // which the tap has them all.
        let octets = [0x03, 5 << 1, 3 << 1, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(
            text(false, &octets, Role::Answer),
            "rx I command ns=5 nr=3 p=0 len=7"
        );
        // A long one, of which the tap has the beginning.
        let long = frame(true, &[0x01, 127 << 1, (9 << 1) | 1], 131);
        assert_eq!(
            describe(&long, Role::Answer).text,
            "tx I command ns=127 nr=9 p=1 len=128"
        );
        assert_eq!(
            text(true, &[0x03, 0x01, (4 << 1) | 1], Role::Answer),
            "tx RR response nr=4 f=1"
        );
        assert_eq!(
            text(false, &[0x03, 0x05, 6 << 1], Role::Answer),
            "rx RNR command nr=6 p=0"
        );
        assert_eq!(
            text(false, &[0x01, 0x09, 2 << 1], Role::Answer),
            "rx REJ response nr=2 f=0"
        );
        assert_eq!(text(false, &[0x03, 0x01], Role::Answer), "rx short len=2");
    }

    #[test]
    fn an_xid_frame_says_what_it_asks_for() {
        let asked = Parameters {
            compression: Some(v42::xid::Compression {
                initiator_to_responder: true,
                responder_to_initiator: true,
                codewords: 2048,
                string_length: 32,
            }),
            ..Parameters::default()
        };
        let mut octets = [0u8; TAP_OCTETS];
        octets[0] = 0x03;
        octets[1] = 0xAF;
        let len = 2 + asked.encode(&mut octets[2..]);
        let described = describe(&frame(false, &octets[..len], len), Role::Answer);
        assert_eq!(
            described.text,
            format!("rx XID command p=0 len={}", len - 2)
        );
        assert_eq!(
            described.xid.as_deref(),
            Some("rx command n401 128/128 k 15/15 compression both codewords 2048 string 32")
        );
        let plain = Parameters::default();
        let len = 2 + plain.encode(&mut octets[2..]);
        octets[0] = 0x03;
        let described = describe(&frame(true, &octets[..len], len), Role::Answer);
        assert_eq!(
            described.xid.as_deref(),
            Some("tx response n401 128/128 k 15/15 compression none")
        );
        // An information field that is not of the format has no reading.
        assert_eq!(
            describe(&frame(true, &[0x03, 0xAF, 0x00, 1], 4), Role::Answer).xid,
            None
        );
    }

    #[test]
    fn a_break_is_named() {
        assert_eq!(
            text(false, &[0x03, 0x03, 0x40, 0x40, 3], Role::Answer),
            "rx UI command p=0 len=3 break n=0"
        );
        assert_eq!(
            text(true, &[0x03, 0x03, 0xE0], Role::Answer),
            "tx UI response f=0 len=1 break-acknowledged n=1"
        );
    }
}
