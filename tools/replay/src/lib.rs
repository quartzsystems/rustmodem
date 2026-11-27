// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Replays a call capture through the modem engine.
//!
//! A capture (`docs/CAPTURE.md`) has every frame a call received. Given to
//! a new engine one frame at a time, as the call's media thread gave them,
//! those frames make the engine do again what it did in the call: the same
//! states at the same times, the same events, the same data received.
//! That is what makes a capture of a call that went wrong a test: the
//! engine is changed, the capture replayed, and what it does now is read
//! or compared with what is expected of it ([`render`], [`compare`]).
//!
//! # What a replay cannot do
//!
//! The far end of the captured call answered what the engine of that day
//! sent. A replay is exact for as long as the engine of today sends the
//! same: once it sends something else, a different signal or the same
//! signal at another time, the frames that follow in the capture are the
//! far end's answer to a call that is no longer the one being made. The
//! replay goes on, and what it shows past that point is how the engine
//! takes those frames, which is still worth seeing and is not what a call
//! would be. [`Outcome::sent_as_captured`] says where that point is.
//!
//! The data the host sent is not in a capture, so a replayed engine sends
//! none: in a capture of a call that carried data from the host, the
//! transmission differs from the first character on. What was received
//! does not depend on it.

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::io::{self, Read, Write};

use engine::{
    CallSettings, EngineState, FRAME_10MS as FRAME, Law, Modem, ModemConfig, ModemEngine,
    ModemEvent, Standard,
};
use rtp::capture::{CaptureError, Reader, channels};

/// Which end of the call the engine is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The engine answered the call.
    Answer,
    /// The engine made the call.
    Originate,
}

impl Role {
    /// The role as the options and the report spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Answer => "answer",
            Role::Originate => "originate",
        }
    }
}

/// How a capture is replayed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub role: Role,
    /// The law of the call's G.711; `None` is the law of the capture's
    /// payload type.
    pub law: Option<Law>,
    /// The modem's settings.
    pub modem: ModemConfig,
    /// What the host had asked of the call.
    pub call: CallSettings,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            role: Role::Answer,
            law: None,
            modem: ModemConfig::default(),
            call: CallSettings::default(),
        }
    }
}

/// Something the engine did in a replay, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// Milliseconds of the call's own samples since the engine started.
    pub at_ms: u64,
    pub what: What,
}

/// What a [`Step`] is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    /// The engine entered a state.
    State(EngineState),
    /// The engine connected.
    Connect { rate: u32, carrier: Option<String> },
    /// The engine gave up, or lost the carrier.
    CarrierLost,
    /// The engine retrained, or changed its rate.
    Retrain,
}

/// What a replay did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub role: Role,
    /// The law the engine was told.
    pub alaw: bool,
    /// Frames of 10 ms the engine was given.
    pub frames: u64,
    /// What the engine did, in order.
    pub steps: Vec<Step>,
    /// The data the engine received.
    pub data: Vec<u8>,
    /// Characters lost to a wrong stop bit.
    pub framing_errors: u32,
    /// What the engine sent, a frame for every frame it was given.
    pub sent: Vec<u8>,
    /// How many frames from the start the engine sent exactly what the
    /// capture says was sent. The replay is exact that far.
    pub sent_as_captured: u64,
    /// Frames the capture has of what was sent.
    pub captured_sent: u64,
}

/// The law of a capture's payload type: 8 is PCMA (RFC 3551 section 6).
fn law_of(payload_type: u8) -> Law {
    if payload_type == 8 {
        Law::Alaw
    } else {
        Law::Ulaw
    }
}

/// Replays a capture.
pub fn replay<R: Read>(reader: &mut Reader<R>, options: &Options) -> Result<Outcome, CaptureError> {
    let law = options
        .law
        .unwrap_or_else(|| law_of(reader.header().payload_type));
    let (received, captured) = channels(reader)?;
    Ok(replay_frames(&received, &captured, law, options))
}

/// Replays the frames a call received, `received`, a whole number of
/// frames of G.711 in `law`. `captured` is what the call sent, to say how
/// far the replay is exact; it may be empty.
pub fn replay_frames(received: &[u8], captured: &[u8], law: Law, options: &Options) -> Outcome {
    let mut engine = Modem::new(options.modem);
    engine.set_law(law);
    engine.set_call(&options.call);
    match options.role {
        Role::Answer => engine.answer(),
        Role::Originate => engine.originate(),
    }
    let mut outcome = Outcome {
        role: options.role,
        alaw: law == Law::Alaw,
        frames: 0,
        steps: Vec::new(),
        data: Vec::new(),
        framing_errors: 0,
        sent: Vec::with_capacity(received.len()),
        sent_as_captured: 0,
        captured_sent: (captured.len() / FRAME) as u64,
    };
    let mut state = EngineState::Idle;
    let mut exact = true;
    let mut tx = [0u8; FRAME];
    let mut buffer = [0u8; 512];
    for (n, rx) in received.as_chunks::<FRAME>().0.iter().enumerate() {
        // What the engine reports of a frame it reports at the end of it.
        let at_ms = (n as u64 + 1) * 10;
        engine.process(rx, &mut tx);
        outcome.frames += 1;
        outcome.sent.extend_from_slice(&tx);
        if exact && captured.get(n * FRAME..(n + 1) * FRAME) == Some(&tx[..]) {
            outcome.sent_as_captured += 1;
        } else {
            exact = false;
        }
        if let Some(snapshot) = engine.diagnostics() {
            if snapshot.state != state {
                state = snapshot.state;
                outcome.steps.push(Step {
                    at_ms,
                    what: What::State(state),
                });
            }
            outcome.framing_errors = snapshot.framing_errors;
        }
        while let Some(event) = engine.poll_event() {
            let what = match event {
                ModemEvent::Connect(info) => What::Connect {
                    rate: info.rate,
                    carrier: info.carrier,
                },
                ModemEvent::CarrierLost => What::CarrierLost,
                ModemEvent::Retrain | ModemEvent::RateChange(_) => What::Retrain,
            };
            outcome.steps.push(Step { at_ms, what });
        }
        loop {
            let count = engine.dte_read(&mut buffer);
            if count == 0 {
                break;
            }
            outcome.data.extend_from_slice(&buffer[..count]);
        }
    }
    outcome
}

/// The 64-bit FNV-1a hash of the data: enough to say whether two replays
/// received the same.
fn digest(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, octet| {
        (hash ^ u64::from(*octet)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// What a replay did, as text: a line for each thing, in the form an
/// expectation file has.
pub fn render(outcome: &Outcome) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "role {}", outcome.role.as_str());
    let _ = writeln!(text, "law {}", if outcome.alaw { "alaw" } else { "ulaw" });
    let _ = writeln!(text, "frames {}", outcome.frames);
    for step in &outcome.steps {
        let at = step.at_ms;
        let _ = match &step.what {
            What::State(state) => writeln!(text, "{at} state {}", state.name()),
            What::Connect { rate, carrier } => writeln!(
                text,
                "{at} connect {rate} {}",
                carrier.as_deref().unwrap_or("-")
            ),
            What::CarrierLost => writeln!(text, "{at} carrier-lost"),
            What::Retrain => writeln!(text, "{at} retrain"),
        };
    }
    let _ = writeln!(
        text,
        "data {} {:016x}",
        outcome.data.len(),
        digest(&outcome.data)
    );
    let _ = writeln!(text, "framing-errors {}", outcome.framing_errors);
    let _ = writeln!(
        text,
        "sent-as-captured {} of {}",
        outcome.sent_as_captured, outcome.captured_sent
    );
    text
}

/// The lines of a report or an expectation that say something: comments,
/// which begin with `#`, and empty lines are not among them.
fn telling(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(n, line)| (n + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// Where a replay and what was expected of it part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    /// The line of the expectation file, or the line after its last.
    pub line: usize,
    /// What the file expects there; `None` at its end.
    pub expected: Option<String>,
    /// What the replay did; `None` when it did nothing more.
    pub found: Option<String>,
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let nothing = "nothing more";
        write!(
            f,
            "line {}: expected `{}`, and the replay has `{}`",
            self.line,
            self.expected.as_deref().unwrap_or(nothing),
            self.found.as_deref().unwrap_or(nothing)
        )
    }
}

/// Compares what a replay did, as [`render`] gives it, with an
/// expectation file. Comments and empty lines of the file do not count.
pub fn compare(rendered: &str, expected: &str) -> Result<(), Difference> {
    let mut found = telling(rendered);
    let mut wanted = telling(expected);
    let mut last = 0;
    loop {
        match (wanted.next(), found.next()) {
            (None, None) => return Ok(()),
            (Some((line, want)), Some((_, have))) if want == have => last = line,
            (want, have) => {
                return Err(Difference {
                    line: want.map_or(last + 1, |(line, _)| line),
                    expected: want.map(|(_, text)| text.to_string()),
                    found: have.map(|(_, text)| text.to_string()),
                });
            }
        }
    }
}

/// Writes G.711 as a WAV file of one channel: format tag 7 for the u-law
/// and 6 for the A-law, one octet a sample, 8000 samples a second.
pub fn write_wav<W: Write>(out: &mut W, octets: &[u8], alaw: bool) -> io::Result<()> {
    let format: u16 = if alaw { 6 } else { 7 };
    let data_len = octets.len() as u32;
    // A data chunk of an odd length is padded to an even one.
    let pad = data_len % 2;
    out.write_all(b"RIFF")?;
    out.write_all(&(4 + 26 + 12 + 8 + data_len + pad).to_le_bytes())?;
    out.write_all(b"WAVE")?;
    out.write_all(b"fmt ")?;
    out.write_all(&18u32.to_le_bytes())?;
    out.write_all(&format.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // channels
    out.write_all(&8000u32.to_le_bytes())?; // samples a second
    out.write_all(&8000u32.to_le_bytes())?; // octets a second
    out.write_all(&1u16.to_le_bytes())?; // block align
    out.write_all(&8u16.to_le_bytes())?; // bits a sample
    out.write_all(&0u16.to_le_bytes())?; // no extension
    out.write_all(b"fact")?;
    out.write_all(&4u32.to_le_bytes())?;
    out.write_all(&data_len.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    out.write_all(octets)?;
    if pad == 1 {
        out.write_all(&[0])?;
    }
    out.flush()
}

/// A modulation as the options spell it.
pub fn modulation(name: &str) -> Option<Standard> {
    match name {
        "v21" => Some(Standard::V21),
        "bell103" => Some(Standard::Bell103),
        _ => None,
    }
}
