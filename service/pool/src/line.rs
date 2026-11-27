// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! One line: an AT interpreter, a modem engine and a serial port, with the
//! buffers between them and the line's counters. While a call is connected
//! the engine is with the media runner and the line holds the call's
//! [`MediaCall`] in its place.

use std::collections::VecDeque;

use at::{AtConfig, AtInterpreter, Mode, ModemSignals, Profile};
use config::{EngineKind, LineConfig, UnknownCommand};
use ctlproto::{CallInfo, CallRecord, ConnectInfo, Counters, Direction, EndReason, LineInfo};
use ctlproto::{ErrorInfo, LineSip, LineState, MediaInfo, Registration, SipMode};
use ctlproto::{LineModem, ModemInfo};
use engine::ModemEngine;
use msg::Message;
use serial::SerialBackend;

use crate::media::MediaCall;
use crate::router::LineId;
use crate::selftest::modem_info;

/// The call a line is in, from dialling or being offered it until it ends.
#[derive(Clone, Debug)]
pub(crate) struct ActiveCall {
    pub id: u64,
    pub direction: Direction,
    pub number: String,
    pub started_ms: u64,
    pub connected_ms: Option<u64>,
    pub connect: Option<ConnectInfo>,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    /// The line went off hook to answer the call.
    pub answered: bool,
    /// The RTP of the call as it was when its media stopped.
    pub media: Option<MediaInfo>,
    /// The call's modem as it was when its media stopped.
    pub modem: Option<ModemInfo>,
}

impl ActiveCall {
    /// The call as the control API shows it. `media` is the RTP of the
    /// call now, and `modem` its modem, when its media is running.
    pub fn info(&self, media: Option<MediaInfo>, modem: Option<ModemInfo>) -> CallInfo {
        CallInfo {
            id: self.id,
            direction: self.direction,
            number: self.number.clone(),
            started_ms: self.started_ms,
            connected_ms: self.connected_ms,
            connect: self.connect.clone(),
            bytes_tx: self.bytes_tx,
            bytes_rx: self.bytes_rx,
            media: media.or_else(|| self.media.clone()).map(Box::new),
            modem: modem.or_else(|| self.modem.clone()),
        }
    }

    pub fn record(self, line: LineId, ended_ms: u64, reason: EndReason) -> CallRecord {
        CallRecord {
            id: self.id,
            line,
            direction: self.direction,
            number: self.number,
            started_ms: self.started_ms,
            connected_ms: self.connected_ms,
            ended_ms,
            reason,
            connect: self.connect,
            bytes_tx: self.bytes_tx,
            bytes_rx: self.bytes_rx,
            media: self.media.map(Box::new),
        }
    }
}

/// One line.
pub(crate) struct Line {
    pub config: LineConfig,
    pub interpreter: AtInterpreter,
    /// The line's engine, while it has no call connected. During one the
    /// engine is with the media runner.
    pub engine: Option<Box<dyn ModemEngine>>,
    /// The media of the call that is connected: the engine is running.
    pub media: Option<MediaCall>,
    /// The serial port, while it is open.
    pub serial: Option<Box<dyn SerialBackend>>,
    /// When to try next to open the port, on the pool's clock.
    pub retry_at_ms: u64,
    /// Why the port is not open, if it would not open or failed.
    pub error: Option<Message>,
    /// The host has DTR on.
    pub dtr: bool,
    /// A call is being offered to the line and has not been answered.
    pub incoming: bool,
    /// Bytes for the host that the port has not taken yet.
    pub to_host: VecDeque<u8>,
    /// Data for the far end that the engine has not taken yet.
    pub to_line: VecDeque<u8>,
    /// The signals last set on the port.
    pub applied: Option<ModemSignals>,
    /// The state last published.
    pub published: Option<LineState>,
    pub counters: Counters,
    pub call: Option<ActiveCall>,
    /// What the host side of the port is bound to, when the pool has been
    /// told.
    pub binding: Option<String>,
    /// The packets of the line's calls are to be captured.
    pub capture: bool,
    /// The names of the hunt groups the line is in.
    pub hunt_groups: Vec<String>,
    /// The service speaks SIP, by its configuration.
    pub sip: bool,
    /// The modem settings the line's engine was made with: `[modem]`'s
    /// with the line's own over them.
    pub engine_modem: config::ModemConfig,
    /// The line's modem is in a self-test: the line takes no call.
    pub testing: bool,
    /// When the line's diagnostics were last published, on the pool's
    /// monotonic clock, and whether its call had a modem to show then.
    pub diag_sent: Option<(u64, bool)>,
}

impl Line {
    /// A line that starts from its stored profile, if it has one.
    pub fn new(config: LineConfig, engine: Box<dyn ModemEngine>, stored: Option<Profile>) -> Line {
        let interpreter = AtInterpreter::with_profile(at_config(&config), stored);
        Line {
            config,
            interpreter,
            engine: Some(engine),
            media: None,
            serial: None,
            retry_at_ms: 0,
            error: None,
            dtr: false,
            incoming: false,
            to_host: VecDeque::new(),
            to_line: VecDeque::new(),
            applied: None,
            published: None,
            counters: Counters::default(),
            call: None,
            binding: None,
            capture: false,
            hunt_groups: Vec::new(),
            sip: false,
            engine_modem: config::ModemConfig::default(),
            testing: false,
            diag_sent: None,
        }
    }

    /// The RTP of the line's call now, when its media is running and its
    /// runner keeps statistics.
    pub fn media_info(&self) -> Option<MediaInfo> {
        Some(self.media.as_ref()?.stats()?.info())
    }

    /// The modem of the line's call now, when its media is running and
    /// its engine shows itself.
    pub fn modem_info(&self) -> Option<ModemInfo> {
        Some(modem_info(&self.media.as_ref()?.diagnostics()?))
    }

    /// The modem settings the line's calls use, as the control API shows
    /// them: `modem` is the `[modem]` table.
    pub fn modem_settings(&self, modem: &config::ModemConfig) -> LineModem {
        let own = &self.config.modem;
        let resolved = modem.for_line(own);
        let mut keys = Vec::new();
        for (key, set) in [
            ("modulations", own.modulations.is_some()),
            ("v8", own.v8.is_some()),
            ("tx_level_dbm0", own.tx_level_dbm0.is_some()),
        ] {
            if set {
                keys.push(key.to_string());
            }
        }
        LineModem {
            modulations: resolved
                .modulations
                .iter()
                .map(|name| name.as_str().to_string())
                .collect(),
            v8: resolved.v8,
            tx_level_dbm0: i32::from(resolved.tx_level_dbm0),
            own: keys,
        }
    }

    pub fn id(&self) -> LineId {
        self.config.id
    }

    /// The tag of the line in log lines: `Line01`.
    pub fn source(&self) -> String {
        format!("Line{:02}", self.config.id)
    }

    /// The state of the line, from what its parts are doing now.
    pub fn state(&self) -> LineState {
        if !self.config.enabled {
            LineState::Disabled
        } else if self.interpreter.off_hook() {
            match self.interpreter.mode() {
                Mode::OnlineData | Mode::OnlineCommand => LineState::Online,
                _ => LineState::Handshaking,
            }
        } else if self.serial.is_none() || !self.dtr {
            // The port is closed, or the host has DTR off.
            LineState::NotReady
        } else if self.testing {
            // Its modem is in a self-test: the line takes no call.
            LineState::Handshaking
        } else if self.incoming {
            LineState::Ringing
        } else {
            LineState::Idle
        }
    }

    /// The line as the control API shows it. `registration` is what the
    /// router says of the line's registration, and `modem` the `[modem]`
    /// table.
    pub fn info(
        &self,
        registration: Option<Registration>,
        modem: &config::ModemConfig,
    ) -> LineInfo {
        let register = self.config.register.as_ref();
        let sip = LineSip {
            mode: match register {
                Some(_) => SipMode::Registered,
                None if self.sip => SipMode::Trunk,
                None => SipMode::None,
            },
            extension: register.map(|register| register.user.clone()),
            server: register.map(|register| register.server.clone()),
            auth_user: register
                .map(|register| register.auth_user.clone())
                .filter(|user| !user.is_empty()),
            expires_s: register.map(|register| register.expires_s),
            registration,
            has_password: register.is_some_and(|register| !register.password.is_empty()),
        };
        LineInfo {
            id: self.config.id,
            name: self.config.name.clone(),
            backend: self.config.serial.backend.as_str().to_string(),
            port: self.config.serial.port.clone(),
            enabled: self.config.enabled,
            state: self.state(),
            auto_answer_rings: self.config.auto_answer_rings,
            unknown_command: self.config.unknown_command.as_str().to_string(),
            counters: self.counters,
            call: self
                .call
                .as_ref()
                .map(|call| call.info(self.media_info(), self.modem_info())),
            error: self.error.as_ref().map(ErrorInfo::from),
            binding: self.binding.clone(),
            engine: self.config.engine.as_str().to_string(),
            hunt_groups: self.hunt_groups.clone(),
            sip,
            capture: self.capture,
            modem: self.modem_settings(modem),
        }
    }
}

/// The interpreter settings a line's configuration asks for.
pub(crate) fn at_config(config: &LineConfig) -> AtConfig {
    AtConfig {
        unknown_command: match config.unknown_command {
            UnknownCommand::Ok => at::UnknownCommand::Ok,
            UnknownCommand::Error => at::UnknownCommand::Error,
        },
        auto_answer_rings: config.auto_answer_rings,
        engine: match config.engine {
            EngineKind::Modem => "SOFT",
            EngineKind::Null => "NULL",
            EngineKind::Echo => "ECHO",
        },
    }
}

/// What a call that begins now asks of the line's engine: the host's S9
/// and S10, and its `+MS`, within what the line's configuration allows.
pub(crate) fn call_settings(
    profile: &Profile,
    modem: &config::ModemConfig,
) -> engine::CallSettings {
    let allowed: Vec<engine::Standard> = modem
        .modulations
        .iter()
        .map(|name| crate::standard(*name))
        .collect();
    let selection = profile.modulation();
    let preferred = match selection.carrier {
        at::Carrier::V21 => engine::Standard::V21,
        at::Carrier::Bell103 => engine::Standard::Bell103,
    };
    // The configuration says what the line may do. Within that the host
    // says which carrier to try first and, with automode off, that no
    // other is to be used and nothing negotiated. A carrier the line may
    // not use is not the host's to choose.
    let order: Vec<engine::Standard> = if !allowed.contains(&preferred) {
        allowed
    } else if selection.automode {
        let others = allowed.iter().copied().filter(|s| *s != preferred);
        std::iter::once(preferred).chain(others).collect()
    } else {
        vec![preferred]
    };
    let mut modulations = [None; 2];
    for (slot, standard) in modulations.iter_mut().zip(order) {
        *slot = Some(standard);
    }
    engine::CallSettings {
        carrier_response_ms: profile.carrier_response_ms(),
        carrier_loss_ms: profile.carrier_loss_ms(),
        modulations: Some(modulations),
        v8: if selection.automode {
            None
        } else {
            Some(false)
        },
    }
}
