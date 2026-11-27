// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem INF against the modem: every line the AT interpreter can
//! answer a host with has its entry in the Responses, every command the
//! INF has Windows send is one the interpreter takes, and the rates are
//! all there.

use std::collections::HashMap;

use at::{AtConfig, AtInterpreter, ConnectInfo, Input, LineEvent, Output, UnknownCommand};
use engine::Modulation;
use modeminf::{Response, State, options};

fn table() -> HashMap<String, Response> {
    let mut table = HashMap::new();
    for response in modeminf::responses() {
        let text = response.text.clone();
        assert!(
            table.insert(text.clone(), response).is_none(),
            "{text:?} is listed twice"
        );
    }
    table
}

/// A modem on a line that refuses commands it does not know, so that a
/// command the INF sends and the modem has not got is seen.
fn modem() -> AtInterpreter {
    AtInterpreter::new(AtConfig {
        unknown_command: UnknownCommand::Error,
        ..AtConfig::default()
    })
}

/// Gives the modem an input and collects what it says to the host.
fn said(modem: &mut AtInterpreter, input: Input<'_>) -> String {
    let mut out = Vec::new();
    modem.step(input, &mut out);
    let mut text = Vec::new();
    for output in out {
        if let Output::DteBytes(bytes) = output {
            text.extend(bytes);
        }
    }
    String::from_utf8(text).expect("the modem speaks ASCII")
}

fn command(modem: &mut AtInterpreter, line: &str) -> String {
    said(modem, Input::DteBytes(format!("{line}\r").as_bytes()))
}

/// The lines of what a modem said in words: each is between two pairs of
/// carriage return and line feed.
fn lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let body = rest
            .strip_prefix("\r\n")
            .unwrap_or_else(|| panic!("no header in {text:?}"));
        let end = body
            .find("\r\n")
            .unwrap_or_else(|| panic!("no trailer in {text:?}"));
        lines.push(format!("\r\n{}\r\n", &body[..end]));
        rest = &body[end + 2..];
    }
    lines
}

/// The INF's initialisation, as Windows sends it.
fn initialise(modem: &mut AtInterpreter) {
    for init in [modeminf::INIT_1, modeminf::INIT_2] {
        let answer = command(modem, init);
        // The first is echoed: echo is on until the second turns it off.
        assert!(answer.ends_with("\r\nOK\r\n"), "{init}: {answer:?}");
    }
}

#[test]
fn every_command_the_inf_sends_is_one_the_modem_takes() {
    let mut modem = modem();
    initialise(&mut modem);
    let ok = "\r\nOK\r\n";
    for line in ["AT&F", "ATS0=0", "ATH"] {
        assert_eq!(command(&mut modem, line), ok, "{line}");
        initialise(&mut modem);
    }
    for (name, fragment) in modeminf::SETTINGS {
        let line = match *name {
            "Prefix" | "Terminator" | "DialSuffix" => continue,
            // Dialling fragments go in a dial command, which is tried
            // below.
            "DialPrefix" | "Tone" | "Pulse" => continue,
            "CallSetupFailTimer" => "ATS7=60".to_string(),
            _ => format!("AT{fragment}"),
        };
        assert_eq!(command(&mut modem, &line), ok, "{name}: {line}");
    }
    // All of them at once, as Windows builds its line.
    assert_eq!(command(&mut modem, "ATX4S7=60&K3\\N3%C3B0"), ok);
    // A dial, tone and pulse, is accepted: it gets no answer until the
    // line does something.
    for dial in ["ATDT5551234", "ATDP5551234"] {
        let mut modem = self::modem();
        initialise(&mut modem);
        assert_eq!(command(&mut modem, dial), "", "{dial}");
    }
}

#[test]
fn every_line_of_every_connection_is_in_the_responses() {
    let table = table();
    let protocols = [None, Some(engine::PROTOCOL_LAPM)];
    let compressions = [
        None,
        Some(engine::COMPRESSION_V42BIS),
        Some(engine::COMPRESSION_V42BIS_TRANSMIT),
        Some(engine::COMPRESSION_V42BIS_RECEIVE),
    ];
    let mut connections = 0;
    for modulation in Modulation::ALL {
        for protocol in protocols {
            for compression in compressions {
                // With the reports the INF asks for, and with every
                // report there is.
                for extra in ["", "AT+ILRR=1"] {
                    let mut modem = modem();
                    initialise(&mut modem);
                    if !extra.is_empty() {
                        command(&mut modem, extra);
                    }
                    assert_eq!(command(&mut modem, "ATA"), "");
                    let info = ConnectInfo {
                        rate: modulation.rate(),
                        carrier: Some(modulation.carrier_name().to_string()),
                        protocol: protocol.map(str::to_string),
                        compression: compression.map(str::to_string),
                    };
                    let text = said(&mut modem, Input::Event(LineEvent::Connected(info)));
                    let lines = lines(&text);
                    let what = format!("{modulation:?} {protocol:?} {compression:?} {extra}");

                    for line in &lines {
                        assert!(
                            table.contains_key(line),
                            "{what}: {line:?} is not in the INF"
                        );
                    }
                    // Windows learns of the connection what the modem
                    // said of it: the rate, and whether there is error
                    // control and compression.
                    let last = &table[lines.last().expect("a CONNECT")];
                    assert_eq!(last.state, State::Connect, "{what}");
                    assert_eq!(last.dce_rate, modulation.rate(), "{what}");
                    let said_options = lines.iter().fold(0, |all, line| all | table[line].options);
                    let mut expected = 0;
                    if protocol.is_some() {
                        expected |= options::ERROR_CONTROL;
                    }
                    if compression.is_some() {
                        expected |= options::COMPRESSION;
                    }
                    assert_eq!(said_options, expected, "{what}");
                    connections += 1;
                }
            }
        }
    }
    assert_eq!(connections, 5 * 2 * 4 * 2);
}

#[test]
fn every_other_result_is_in_the_responses_with_its_meaning() {
    let table = table();
    let cases = [
        (LineEvent::Ring, State::Ring),
        (LineEvent::Busy, State::Busy),
        (LineEvent::NoAnswer, State::NoAnswer),
        (LineEvent::NoDialtone, State::NoDialtone),
        (LineEvent::CarrierLost, State::NoCarrier),
    ];
    for (event, state) in cases {
        let mut modem = modem();
        initialise(&mut modem);
        if event != LineEvent::Ring {
            assert_eq!(command(&mut modem, "ATD5551234"), "");
        }
        let text = said(&mut modem, Input::Event(event.clone()));
        let lines = lines(&text);
        assert_eq!(lines.len(), 1, "{event:?}: {text:?}");
        assert_eq!(table[&lines[0]].state, state, "{event:?}");
    }
    let mut modem = modem();
    initialise(&mut modem);
    assert_eq!(table[&command(&mut modem, "AT")].state, State::Ok);
    assert_eq!(table[&command(&mut modem, "ATS999=1")].state, State::Error);
    // A connection of which the rate is not known is a bare CONNECT.
    command(&mut modem, "ATA");
    let text = said(
        &mut modem,
        Input::Event(LineEvent::Connected(ConnectInfo::unknown())),
    );
    let last = lines(&text).pop().unwrap();
    assert_eq!(last, "\r\nCONNECT\r\n");
    assert_eq!(table[&last].state, State::Connect);
    assert_eq!(table[&last].dce_rate, 0);

    // In numbers, for a host that has asked for them.
    let mut modem = self::modem();
    command(&mut modem, "ATE0V0");
    for (line, state) in [("AT", State::Ok), ("ATS999=1", State::Error)] {
        assert_eq!(table[&command(&mut modem, line)].state, state, "{line}");
    }
}

#[test]
fn the_rates_are_every_one_from_300_to_v34_and_v90() {
    let rates = modeminf::rates();
    assert_eq!(rates.first(), Some(&300));
    assert_eq!(rates.last(), Some(&56_000));
    assert!(
        rates.windows(2).all(|pair| pair[0] < pair[1]),
        "in order, each once"
    );
    for modulation in Modulation::ALL {
        assert!(rates.contains(&modulation.rate()), "{modulation:?}");
    }
    // V.34: every 2400 bit/s to 33600.
    for rate in (2400..=33_600).step_by(2400) {
        assert!(rates.contains(&rate), "V.34 at {rate}");
    }
    // V.32bis.
    for rate in [4800, 7200, 9600, 12_000, 14_400] {
        assert!(rates.contains(&rate), "V.32bis at {rate}");
    }
    // V.90: 28000 and every 1333 1/3 bit/s to 56000, twenty-two rates.
    let v90 = [
        28_000, 29_333, 30_666, 32_000, 33_333, 34_666, 36_000, 37_333, 38_666, 40_000, 41_333,
        42_666, 44_000, 45_333, 46_666, 48_000, 49_333, 50_666, 52_000, 53_333, 54_666, 56_000,
    ];
    for rate in v90 {
        assert!(rates.contains(&rate), "V.90 at {rate}");
    }
    let table = table();
    for rate in rates {
        let connect = &table[&format!("\r\nCONNECT {rate}\r\n")];
        assert_eq!((connect.state, connect.dce_rate), (State::Connect, rate));
    }
    assert_eq!(
        modeminf::max_dce_rate(),
        2400,
        "the fastest the engine has today"
    );
}

#[test]
fn an_entry_is_ten_octets_with_the_rate_low_octet_first() {
    let table = table();
    assert_eq!(
        table["\r\nCONNECT 2400\r\n"].octets(),
        [0x02, 0x00, 0x60, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        table["\r\nCONNECT 56000\r\n"].octets(),
        [0x02, 0x00, 0xC0, 0xDA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        table["\r\n+ER: LAPM\r\n"].octets(),
        [0x01, 0x02, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(table["\r\nRING\r\n"].key(), "<cr><lf>RING<cr><lf>");
    assert_eq!(table["2\r"].key(), "2<cr>");
}

#[test]
fn the_inf_is_a_modem_inf_with_every_response_in_it() {
    let inf = modeminf::generate();
    for needed in [
        "[Version]",
        "Class       = Modem",
        "{4D36E96D-E325-11CE-BFC1-08002BE10318}",
        "CatalogFile = rmvcom.cat",
        "%Modem% = Modem, RUSTMODEM\\SOFTMODEM",
        "Modem    = \"RustModem Softmodem\"",
        "Provider = \"Quartz Systems\"",
        "HKR,Init,2,,\"AT&FE0V1X4&C1&D2S0=0+MR=1;+ER=1;+DR=1<cr>\"",
        "HKR,Settings,ErrorControl_Forced,,\"\\N2\"",
        "HKR,Settings,Compression_On,,\"%%C3\"",
        "HKR,Responses,\"<cr><lf>CONNECT 2400<cr><lf>\",1,02,00,60,09,00,00,00,00,00,00",
        // Properties: the port at 115200 and the line at 2400.
        "00,c2,01,00,60,09,00,00\r\n",
    ] {
        assert!(inf.contains(needed), "the INF has no {needed:?}");
    }
    let entries = inf
        .lines()
        .filter(|line| line.starts_with("HKR,Responses,"))
        .count();
    assert_eq!(entries, modeminf::responses().len());
    // An INF is ASCII here, with the line endings Windows' tools expect,
    // and no line is longer than they take.
    assert!(inf.is_ascii());
    assert!(inf.lines().all(|line| line.len() < 512));
    assert!(!inf.replace("\r\n", "").contains(['\r', '\n']));
}
