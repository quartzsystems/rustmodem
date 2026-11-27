// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The script format: one step a line, run from the top.
//!
//! Blank lines and lines that begin with `#` are skipped. The settings
//! come first, then `listen`, which binds the sockets, then what the peer
//! does. A step that cannot be done, or an `expect` or `check` that does
//! not hold, ends the script with the number of its line. [`HELP`] lists
//! the steps.

use std::fmt;
use std::io::Write;
use std::net::SocketAddrV4;
use std::time::Duration;

use sdp::Codec;
use sip::Credentials;
use sip::digest::Algorithm;

use crate::agent::{Agent, AgentConfig, MediaKind, loopback};
use crate::media::{Impairments, MediaReport};
use crate::peer::{Challenge, PeerEvent, RegistrarPolicy};

/// The steps, for `--help`.
pub const HELP: &str = "\
Settings, before listen:
  user <name>                      the user in From and Contact (default peer)
  codec <pcmu|pcma>...             the codecs, most preferred first
  ptime <10|20|30>                 the packet time in milliseconds
  seed <number>                    makes tags, faults and patterns
  media silence                    what a call sends: silence (the default),
  media pattern                    a pattern checked against its echo,
  media file <path>                the G.711 octets of a file, or
  media null                       the null engine, for a data call
  media modem [v21] [bell103] [no-v8]
                                   the modem, for a data call with a line
                                   that has the modem engine: with the
                                   modulations named, the first by choice,
                                   or both; with V.8 unless no-v8
  impair [loss=<n>] [duplicate=<n>] [reorder=<n>] [jitter=<ms>] [ppm=<n>]
                                   faults in what is sent: packets in a
                                   thousand lost, sent twice and held back,
                                   delay up to so long, and a clock fast by
                                   so many parts per million

Then:
  listen <address>:<port>          binds SIP there and RTP beside it
  registrar accept [<seconds>]     what a REGISTER gets (default accept)
  registrar challenge <realm> <user> <password>
  registrar reject <status>
  registrar silent
  call <uri> <address>:<port>      sends an INVITE with an offer
  call <uri> <address>:<port> as modem originate [v21] [bell103] [no-v8]
                                   the same, and the call's media is the
                                   modem, as `media modem` has it
  ring                             180 to their INVITE
  answer                           200 with an answer; the media begins
  answer as modem answer [v21] [bell103] [no-v8]
                                   the same, with the modem as its media
  reject <status>                  refuses their INVITE
  cancel                           cancels our INVITE
  hangup                           BYE
  expect <event> [<seconds>]       waits for it, 10 s unless said:
                                   incoming, ringing, answer, refused,
                                   cancelled, ack, hangup, register
  talk <seconds>                   lets the call run
  send <text>                      data to the engine; \\r \\n \\\\ \\xNN
  receive <text> [<seconds>]       waits for that data from the engine
  expect connect [<seconds>]       waits for the modem to connect, 30 s
                                   unless said, and prints its rate and
                                   carrier
  check echo                       the pattern came back and nothing else did
  check clean                      nothing was lost or resynchronised coming in
  report                           prints what the media did and saw
";

/// Why a script stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    /// The line, from 1.
    pub line: usize,
    pub reason: String,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.reason)
    }
}

impl std::error::Error for ScriptError {}

/// An event a script waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    Incoming,
    Ringing,
    Answer,
    Refused,
    Cancelled,
    Ack,
    HangUp,
    Register,
}

/// One line of a script.
#[derive(Clone, Debug)]
pub enum Step {
    User(String),
    Codecs(Vec<Codec>),
    Ptime(u32),
    Seed(u64),
    Media(MediaKind),
    MediaFile(String),
    Impair(Impairments),
    Listen(SocketAddrV4),
    Registrar(RegistrarPolicy),
    Call {
        uri: String,
        to: SocketAddrV4,
        /// The media of this call and those after it, when it is said.
        media: Option<MediaKind>,
    },
    Ring,
    Answer(Option<MediaKind>),
    Reject(u16),
    Cancel,
    HangUp,
    Expect {
        event: Expected,
        within: Duration,
    },
    /// Waits for the call's modem to connect.
    ExpectConnect(Duration),
    Talk(Duration),
    Send(Vec<u8>),
    Receive {
        data: Vec<u8>,
        within: Duration,
    },
    CheckEcho,
    CheckClean,
    Report,
}

fn seconds(word: &str) -> Result<Duration, String> {
    word.parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && (0.0..=86_400.0).contains(seconds))
        .map(Duration::from_secs_f64)
        .ok_or_else(|| format!("{word} is not a number of seconds"))
}

fn number<T: std::str::FromStr>(word: &str) -> Result<T, String> {
    word.parse()
        .map_err(|_| format!("{word} is not a number that fits"))
}

fn address(word: &str) -> Result<SocketAddrV4, String> {
    word.parse()
        .map_err(|_| format!("{word} is not an IPv4 address and port"))
}

/// The octets of a `send` or `receive` text.
fn text(written: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut octets = written.bytes();
    while let Some(octet) = octets.next() {
        if octet != b'\\' {
            out.push(octet);
            continue;
        }
        match octets.next() {
            Some(b'r') => out.push(b'\r'),
            Some(b'n') => out.push(b'\n'),
            Some(b'\\') => out.push(b'\\'),
            Some(b'x') => {
                let digits = [octets.next(), octets.next()];
                let value = match digits {
                    [Some(high), Some(low)] => std::str::from_utf8(&[high, low])
                        .ok()
                        .and_then(|pair| u8::from_str_radix(pair, 16).ok()),
                    _ => None,
                };
                out.push(value.ok_or("\\x takes two hexadecimal digits")?);
            }
            _ => return Err("\\ is followed by r, n, \\ or xNN".to_owned()),
        }
    }
    Ok(out)
}

fn step(line: &str) -> Result<Step, String> {
    let (name, rest) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(name, rest)| (name, rest.trim()));
    let words: Vec<&str> = rest.split_whitespace().collect();
    let within = |word: Option<&&str>| word.map_or(Ok(Duration::from_secs(10)), |w| seconds(w));
    Ok(match (name, words.as_slice()) {
        ("user", [user]) => Step::User((*user).to_owned()),
        ("codec", codecs @ [_, ..]) => Step::Codecs(
            codecs
                .iter()
                .map(|codec| match *codec {
                    "pcmu" => Ok(Codec::Pcmu),
                    "pcma" => Ok(Codec::Pcma),
                    other => Err(format!("{other} is not pcmu or pcma")),
                })
                .collect::<Result<_, _>>()?,
        ),
        ("ptime", [ptime @ ("10" | "20" | "30")]) => Step::Ptime(number(ptime)?),
        ("seed", [seed]) => Step::Seed(number(seed)?),
        ("media", ["silence"]) => Step::Media(MediaKind::Silence),
        ("media", ["pattern"]) => Step::Media(MediaKind::Pattern),
        ("media", ["null"]) => Step::Media(MediaKind::Null),
        ("media", ["modem", modem @ ..]) => Step::Media(modem_media(modem)?),
        ("media", ["file", ..]) => {
            // A path may have spaces in it.
            let path = rest["file".len()..].trim();
            if path.is_empty() {
                return Err("media file takes a path".to_owned());
            }
            Step::MediaFile(path.to_owned())
        }
        ("impair", faults) => {
            let mut impairments = Impairments::default();
            for fault in faults {
                let (key, value) = fault
                    .split_once('=')
                    .ok_or_else(|| format!("{fault} is not name=value"))?;
                match key {
                    "loss" => impairments.loss = number(value)?,
                    "duplicate" => impairments.duplicate = number(value)?,
                    "reorder" => impairments.reorder = number(value)?,
                    "jitter" => impairments.jitter_ms = number(value)?,
                    "ppm" => impairments.clock_ppm = number(value)?,
                    other => return Err(format!("{other} is not a fault")),
                }
            }
            if impairments.loss > 1000 || impairments.duplicate > 1000 || impairments.reorder > 1000
            {
                return Err("a rate is packets in a thousand".to_owned());
            }
            Step::Impair(impairments)
        }
        ("listen", [at]) => Step::Listen(address(at)?),
        ("registrar", ["accept"]) => Step::Registrar(RegistrarPolicy::Accept { expires: None }),
        ("registrar", ["accept", expires]) => Step::Registrar(RegistrarPolicy::Accept {
            expires: Some(number(expires)?),
        }),
        ("registrar", ["challenge", realm, user, password]) => {
            Step::Registrar(RegistrarPolicy::Challenge {
                challenge: Challenge {
                    realm: (*realm).to_owned(),
                    credentials: Credentials {
                        username: (*user).to_owned(),
                        password: (*password).to_owned(),
                    },
                    algorithm: Algorithm::Md5,
                    qop: true,
                },
                expires: None,
            })
        }
        ("registrar", ["reject", status]) => {
            Step::Registrar(RegistrarPolicy::Reject(number(status)?))
        }
        ("registrar", ["silent"]) => Step::Registrar(RegistrarPolicy::Silent),
        ("call", [uri, to]) => Step::Call {
            uri: (*uri).to_owned(),
            to: address(to)?,
            media: None,
        },
        ("call", [uri, to, "as", "modem", "originate", modem @ ..]) => Step::Call {
            uri: (*uri).to_owned(),
            to: address(to)?,
            media: Some(modem_media(modem)?),
        },
        ("ring", []) => Step::Ring,
        ("answer", []) => Step::Answer(None),
        ("answer", ["as", "modem", "answer", modem @ ..]) => {
            Step::Answer(Some(modem_media(modem)?))
        }
        ("reject", [status]) => Step::Reject(number(status)?),
        ("cancel", []) => Step::Cancel,
        ("hangup", []) => Step::HangUp,
        ("expect", ["connect", time @ ..]) if time.len() <= 1 => Step::ExpectConnect(
            time.first()
                .map_or(Ok(Duration::from_secs(30)), |time| seconds(time))?,
        ),
        ("expect", [event, time @ ..]) if time.len() <= 1 => Step::Expect {
            event: match *event {
                "incoming" => Expected::Incoming,
                "ringing" => Expected::Ringing,
                "answer" => Expected::Answer,
                "refused" => Expected::Refused,
                "cancelled" => Expected::Cancelled,
                "ack" => Expected::Ack,
                "hangup" => Expected::HangUp,
                "register" => Expected::Register,
                other => return Err(format!("{other} is not an event")),
            },
            within: within(time.first())?,
        },
        ("talk", [time]) => Step::Talk(seconds(time)?),
        ("send", [_, ..]) => Step::Send(text(rest)?),
        ("receive", [data, time @ ..]) if time.len() <= 1 => Step::Receive {
            data: text(data)?,
            within: within(time.first())?,
        },
        ("check", ["echo"]) => Step::CheckEcho,
        ("check", ["clean"]) => Step::CheckClean,
        ("report", []) => Step::Report,
        _ => return Err(format!("not a step: {line}")),
    })
}

/// The modem a `media modem` or an `as modem` asks for: the modulations
/// named, the first by choice, or both; with V.8 unless `no-v8`.
fn modem_media(words: &[&str]) -> Result<MediaKind, String> {
    let mut modulations = [None; 2];
    let mut count = 0;
    let mut v8 = true;
    for word in words {
        let standard = match *word {
            "v21" => engine::Standard::V21,
            "bell103" => engine::Standard::Bell103,
            "no-v8" => {
                v8 = false;
                continue;
            }
            other => return Err(format!("{other} is not v21, bell103 or no-v8")),
        };
        if count == modulations.len() || modulations.contains(&Some(standard)) {
            return Err(format!("{word} is there twice"));
        }
        modulations[count] = Some(standard);
        count += 1;
    }
    if count == 0 {
        modulations = [Some(engine::Standard::V21), Some(engine::Standard::Bell103)];
    }
    Ok(MediaKind::Modem { modulations, v8 })
}

/// Reads a script: its steps, each with the number of its line.
pub fn parse(script: &str) -> Result<Vec<(usize, Step)>, ScriptError> {
    let mut steps = Vec::new();
    for (index, line) in script.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parsed = step(line).map_err(|reason| ScriptError {
            line: index + 1,
            reason,
        })?;
        steps.push((index + 1, parsed));
    }
    Ok(steps)
}

/// A media report as text.
pub fn describe(report: &MediaReport) -> String {
    let seen = &report.received;
    format!(
        "sent {} of {} made (lost {}, duplicated {}, reordered {})\n\
         received {} (lost {}, late {}, duplicate {}, resync {}, ignored {}, jitter {} samples at most)\n\
         echo: {} frames came back, {} silent, {} mismatched\n",
        report.sent,
        report.made,
        report.lost,
        report.duplicated,
        report.reordered,
        seen.packets,
        seen.lost,
        seen.late,
        seen.duplicate,
        seen.resync,
        seen.ignored,
        seen.jitter_max,
        report.echoed_frames,
        report.silent_frames,
        report.mismatched_frames,
    )
}

fn matches(expected: Expected, event: &PeerEvent) -> bool {
    match (expected, event) {
        (Expected::Incoming, PeerEvent::Incoming { .. })
        | (Expected::Answer, PeerEvent::Answered { .. })
        | (Expected::Refused, PeerEvent::Refused { .. })
        | (Expected::Cancelled, PeerEvent::Cancelled { .. })
        | (Expected::Ack, PeerEvent::Acknowledged { .. })
        | (Expected::HangUp, PeerEvent::HungUp { .. })
        | (Expected::Register, PeerEvent::Registered { .. }) => true,
        (Expected::Ringing, PeerEvent::Progress { response, .. }) => response.status >= 180,
        _ => false,
    }
}

/// What a script has set up.
struct Run {
    config: AgentConfig,
    agent: Option<Agent>,
    /// What the null engine has given and no `receive` has taken.
    heard: Vec<u8>,
}

impl Run {
    fn agent(&mut self) -> Result<&mut Agent, String> {
        self.agent
            .as_mut()
            .ok_or_else(|| "listen comes first".to_owned())
    }

    fn setting(&mut self) -> Result<&mut AgentConfig, String> {
        if self.agent.is_some() {
            return Err("a setting comes before listen".to_owned());
        }
        Ok(&mut self.config)
    }

    fn report(&mut self) -> Result<MediaReport, String> {
        self.agent()?
            .report()
            .ok_or_else(|| "there has been no media".to_owned())
    }

    fn step(&mut self, step: Step, out: &mut dyn Write) -> Result<(), String> {
        let io = |error: std::io::Error| error.to_string();
        match step {
            Step::User(user) => self.setting()?.user = user,
            Step::Codecs(codecs) => self.setting()?.codecs = codecs,
            Step::Ptime(ptime) => self.setting()?.ptime_ms = ptime,
            Step::Seed(seed) => self.setting()?.seed = seed,
            Step::Media(media) => self.setting()?.media = media,
            Step::MediaFile(path) => {
                let octets = std::fs::read(&path).map_err(|error| format!("{path}: {error}"))?;
                self.setting()?.media = MediaKind::File(octets);
            }
            Step::Impair(impairments) => self.setting()?.impairments = impairments,
            Step::Listen(at) => {
                let config = self.setting()?;
                config.listen = at;
                let agent =
                    Agent::bind(config.clone()).map_err(|error| format!("{at}: {error}"))?;
                let _ = writeln!(
                    out,
                    "SIP on {}, RTP on {}",
                    agent.sip_address(),
                    agent.rtp_address()
                );
                self.agent = Some(agent);
            }
            Step::Registrar(policy) => self.agent()?.peer.registrar = policy,
            Step::Call { uri, to, media } => {
                let agent = self.agent()?;
                if let Some(media) = media {
                    agent.set_media(media);
                }
                agent.invite(&uri, to);
            }
            Step::Ring => self.agent()?.ring(),
            Step::Answer(media) => {
                let agent = self.agent()?;
                if let Some(media) = media {
                    agent.set_media(media);
                }
                if !agent.answer() {
                    return Err("there is no INVITE with an offer that can be answered".to_owned());
                }
            }
            Step::Reject(status) => self.agent()?.reject(status),
            Step::Cancel => {
                if !self.agent()?.cancel() {
                    return Err("there is no INVITE of ours to cancel".to_owned());
                }
            }
            Step::HangUp => self.agent()?.hang_up(),
            Step::Expect { event, within } => {
                let mut refusal = None;
                let found = self
                    .agent()?
                    .wait(within, |seen| {
                        // A call that is refused is not going to do
                        // anything else that was waited for.
                        if let PeerEvent::Refused { response, .. } = seen
                            && event != Expected::Refused
                        {
                            refusal = Some(response.status);
                            return true;
                        }
                        matches(event, seen)
                    })
                    .map_err(io)?;
                match (found, refusal) {
                    (_, Some(status)) => return Err(format!("the call was refused with {status}")),
                    (None, _) => return Err("it did not happen in time".to_owned()),
                    (Some(PeerEvent::Refused { response, .. }), _) => {
                        let _ = writeln!(out, "refused with {}", response.status);
                    }
                    (Some(PeerEvent::Registered { request, status }), _) => {
                        let who = request.headers.get("To").unwrap_or_default();
                        let _ = writeln!(out, "REGISTER for {who} answered {status}");
                    }
                    _ => {}
                }
            }
            Step::ExpectConnect(within) => {
                let mut connected = None;
                self.agent()?
                    .until(within, |agent| {
                        while let Some(event) = agent.engine().and_then(|e| e.poll_event()) {
                            if let engine::ModemEvent::Connect(info) = event {
                                connected = Some(info);
                            }
                        }
                        connected.is_some()
                    })
                    .map_err(io)?;
                let Some(info) = connected else {
                    return Err("the modem did not connect in time".to_owned());
                };
                let _ = writeln!(
                    out,
                    "connected at {} bit/s with {}",
                    info.rate,
                    info.carrier.as_deref().unwrap_or("no carrier named")
                );
            }
            Step::Talk(time) => self.agent()?.run(time).map_err(io)?,
            Step::Send(data) => {
                let mut at = 0;
                let sent = self
                    .agent()?
                    .until(Duration::from_secs(10), |agent| {
                        if let Some(engine) = agent.engine() {
                            at += engine.dte_write(&data[at..]);
                        }
                        at == data.len()
                    })
                    .map_err(io)?;
                if !sent {
                    return Err("the engine did not take the data".to_owned());
                }
            }
            Step::Receive { data, within } => {
                let mut heard = std::mem::take(&mut self.heard);
                let mut found = None;
                self.agent()?
                    .until(within, |agent| {
                        let mut buffer = [0u8; 1024];
                        while let Some(engine) = agent.engine() {
                            let len = engine.dte_read(&mut buffer);
                            if len == 0 {
                                break;
                            }
                            heard.extend_from_slice(&buffer[..len]);
                        }
                        found = heard
                            .windows(data.len().max(1))
                            .position(|window| window == data);
                        found.is_some()
                    })
                    .map_err(io)?;
                match found {
                    // What follows is for the next receive.
                    Some(at) => self.heard = heard.split_off(at + data.len()),
                    None => {
                        let got = String::from_utf8_lossy(&heard).into_owned();
                        self.heard = heard;
                        return Err(format!("the data did not come; what came: {got:?}"));
                    }
                }
            }
            Step::CheckEcho => {
                let report = self.report()?;
                if report.echoed_frames == 0 || report.mismatched_frames > 0 {
                    return Err(format!(
                        "the echo is not clean: {} frames came back, {} mismatched",
                        report.echoed_frames, report.mismatched_frames
                    ));
                }
            }
            Step::CheckClean => {
                let seen = self.report()?.received;
                if seen.packets == 0 || seen.lost > 0 || seen.resync > 0 {
                    return Err(format!(
                        "what came in is not clean: {} packets, {} lost, {} resync",
                        seen.packets, seen.lost, seen.resync
                    ));
                }
            }
            Step::Report => {
                let report = self.report()?;
                let _ = out.write_all(describe(&report).as_bytes());
            }
        }
        Ok(())
    }
}

/// Runs a script. What it prints goes to `out`.
pub fn run(script: &str, out: &mut dyn Write) -> Result<(), ScriptError> {
    let steps = parse(script)?;
    let mut run = Run {
        config: AgentConfig::new("peer", loopback()),
        agent: None,
        heard: Vec::new(),
    };
    for (line, step) in steps {
        run.step(step, out)
            .map_err(|reason| ScriptError { line, reason })?;
    }
    // A BYE that was the last step still has to go and be answered.
    if let Some(agent) = &mut run.agent {
        let _ = agent.run(Duration::from_millis(100));
    }
    Ok(())
}
