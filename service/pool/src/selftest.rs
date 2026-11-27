// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The loopback self-test, and what the pool shows of a call's modem.
//!
//! The self-test joins the modems of two lines sample for sample, with
//! nothing between them: the first originates, the second answers, and
//! once they are connected each sends the other [`SELFTEST_OCTETS`]
//! octets, which must arrive as they were sent. It takes no socket and no
//! port, so it says whether the modems of this build, with these two
//! lines' settings, work; it says nothing of the network or the hosts.
//!
//! A test is a [`SelfTestJob`], which the pool makes
//! ([`crate::Pool::selftest_begin`]) and whoever asked runs on a thread of
//! its own: the modems run as fast as the machine carries them, which for
//! half an hour of a 300 bit/s call is some seconds, and the pool is not
//! kept waiting.

use ctlproto::{
    ConnectInfo, ErrorInfo, LineDiag, LinkInfo, ModemInfo, ModemStateChange, SelfTestResult,
};
use engine::{
    CONSTELLATION_UNIT, CallSettings, Diagnostics, EngineState, FRAME_10MS, Law, LinkEnd,
    LinkStage, LinkStats, Modem, ModemConfig, ModemEngine, ModemEvent, SILENCE_OCTET, ScopeKind,
};
use msg::message;

use crate::router::LineId;

/// The octets a self-test sends each way.
pub const SELFTEST_OCTETS: usize = 64 * 1024;

/// How long the modems of a self-test are given to connect, in
/// milliseconds of the call.
const CONNECT_LIMIT_MS: u64 = 60_000;

/// The least a level is, in dBm0, to be one: below it there is nothing.
const NOTHING_DBM0: f32 = -99.0;

fn level(dbm0: f32) -> Option<i32> {
    (dbm0 >= NOTHING_DBM0).then(|| (dbm0 * 10.0).round() as i32)
}

/// A data link's counts as the control API has them; none for a modem
/// that has no link, or has not started it.
fn link_info(link: &LinkStats) -> Option<LinkInfo> {
    if link.stage == LinkStage::Idle {
        return None;
    }
    let compression = |agreed: bool, compressed: bool| {
        match (agreed, compressed) {
            (false, _) => "none",
            (true, false) => "transparent",
            (true, true) => "compressed",
        }
        .to_string()
    };
    Some(LinkInfo {
        stage: link.stage.name().to_string(),
        end: (link.end != LinkEnd::None).then(|| link.end.name().to_string()),
        frames_tx: link.frames_sent,
        frames_rx: link.frames_received,
        retransmissions: link.retransmissions,
        frame_errors: link.frame_errors,
        window: u32::from(link.window),
        window_size: u32::from(link.window_size),
        peer_busy: link.peer_busy,
        own_busy: link.own_busy,
        compression_tx: compression(link.compression.0, link.compressed_mode.0),
        compression_rx: compression(link.compression.1, link.compressed_mode.1),
        tx_octets_in: link.compress_in,
        tx_octets_out: link.compress_out,
        rx_octets_in: link.decompress_in,
        rx_octets_out: link.decompress_out,
        breaks: link.breaks,
    })
}

/// A modem's snapshot of itself as the control API has it.
pub(crate) fn modem_info(snapshot: &Diagnostics) -> ModemInfo {
    // The receiver's figures are those of a modem at 1200 or 2400 bit/s
    // that has connected, which is when its link is started.
    let link = link_info(&snapshot.link).map(Box::new);
    let receiver = link.is_some() && snapshot.equalizer_mse > 0.0;
    let tenths = |value: f32| (value * 10.0).round() as i32;
    ModemInfo {
        snr_tenth_db: receiver.then(|| tenths(snapshot.snr_db)),
        equalizer_error_tenth_db: receiver
            .then(|| tenths(10.0 * snapshot.equalizer_mse.max(1e-12).log10())),
        timing_offset_ppm: receiver.then(|| snapshot.timing_offset_ppm.round() as i32),
        link,
        state: snapshot.state.name().to_string(),
        rx_level_tenth_dbm0: level(snapshot.rx_level_dbm0),
        tx_level_tenth_dbm0: level(snapshot.tx_level_dbm0),
        carrier: snapshot.carrier,
        frequency_error_mhz: (snapshot.frequency_error_hz * 1000.0).round() as i32,
        quality_percent: (snapshot.quality.clamp(0.0, 1.0) * 100.0).round() as u32,
        framing_errors: snapshot.framing_errors,
        retrains: snapshot.retrains,
    }
}

/// The diagnostics of a line whose call's modem last gave `snapshot`.
pub(crate) fn line_diag(line: LineId, snapshot: Option<&Diagnostics>) -> LineDiag {
    let Some(snapshot) = snapshot else {
        return LineDiag {
            line,
            modem: None,
            scope_kind: "none".to_string(),
            ..LineDiag::default()
        };
    };
    let (scope_kind, scope) = match snapshot.scope_kind {
        ScopeKind::None => ("none", Vec::new()),
        ScopeKind::Discriminator => ("discriminator", snapshot.scope.to_vec()),
    };
    LineDiag {
        line,
        modem: Some(modem_info(snapshot)),
        history: snapshot
            .history()
            .iter()
            .map(|change| ModemStateChange {
                state: change.state.name().to_string(),
                at_ms: change.at_ms,
            })
            .collect(),
        scope_kind: scope_kind.to_string(),
        scope,
        constellation_bits: u32::from(snapshot.constellation_bits),
        constellation_unit: if snapshot.constellation_bits == 0 {
            0
        } else {
            i32::from(CONSTELLATION_UNIT)
        },
        constellation: snapshot.constellation().iter().flatten().copied().collect(),
    }
}

/// A self-test that is ready to run: two lines, held busy, and the
/// settings of their modems.
#[derive(Clone, Debug)]
pub struct SelfTestJob {
    /// The line whose modem originates.
    pub a: LineId,
    /// The line whose modem answers.
    pub b: LineId,
    pub(crate) modem_a: ModemConfig,
    pub(crate) modem_b: ModemConfig,
    pub(crate) call_a: CallSettings,
    pub(crate) call_b: CallSettings,
    /// The octets to send each way: [`SELFTEST_OCTETS`], but for a test
    /// of the test.
    pub octets: usize,
}

/// Octet `index` of the data one end sends: it does not repeat for a long
/// time, and the two ends send different data.
fn octet(seed: u32, index: usize) -> u8 {
    let mixed = (index as u32)
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(seed)
        .rotate_left(11);
    (mixed ^ (mixed >> 15)) as u8
}

/// One end of the test.
struct End {
    modem: Modem,
    seed: u32,
    /// What it last sent: the other end's next frame.
    line: [u8; FRAME_10MS],
    connect: Option<(ConnectInfo, u64)>,
    lost: bool,
    written: usize,
    /// Octets received that are the ones the other end sent in their
    /// places.
    good: usize,
    received: usize,
    state: EngineState,
}

impl End {
    fn new(config: ModemConfig, call: &CallSettings, seed: u32) -> End {
        let mut modem = Modem::new(config);
        modem.set_law(Law::Ulaw);
        modem.set_call(call);
        End {
            modem,
            seed,
            line: [SILENCE_OCTET; FRAME_10MS],
            connect: None,
            lost: false,
            written: 0,
            good: 0,
            received: 0,
            state: EngineState::Idle,
        }
    }

    /// One frame: what the other end sent in, what this end sends out.
    fn tick(&mut self, heard: &[u8; FRAME_10MS], at_ms: u64, other_seed: u32, octets: usize) {
        self.modem.process(heard, &mut self.line);
        while let Some(event) = self.modem.poll_event() {
            match event {
                ModemEvent::Connect(info) => {
                    let connect = ConnectInfo {
                        rate: info.rate,
                        carrier: info.carrier,
                        protocol: info.protocol,
                        compression: info.compression,
                    };
                    self.connect = Some((connect, at_ms));
                }
                ModemEvent::CarrierLost => self.lost = true,
                ModemEvent::Retrain | ModemEvent::RateChange(_) => {}
            }
        }
        if let Some(snapshot) = self.modem.diagnostics()
            && snapshot.state != EngineState::Ended
        {
            self.state = snapshot.state;
        }
        if self.connect.is_none() {
            return;
        }
        // What the modem will take of the data that is left, and what it
        // has received.
        let mut chunk = [0u8; 64];
        while self.written < octets {
            let count = chunk.len().min(octets - self.written);
            for (n, slot) in chunk[..count].iter_mut().enumerate() {
                *slot = octet(self.seed, self.written + n);
            }
            let taken = self.modem.dte_write(&chunk[..count]);
            self.written += taken;
            if taken < count {
                break;
            }
        }
        loop {
            let count = self.modem.dte_read(&mut chunk);
            if count == 0 {
                break;
            }
            for got in &chunk[..count] {
                if self.received < octets && *got == octet(other_seed, self.received) {
                    self.good += 1;
                }
                self.received += 1;
            }
        }
    }
}

impl SelfTestJob {
    /// Runs the test. It reads no clock: `elapsed_ms` of the result is
    /// left at 0 for whoever timed the run to fill in.
    pub fn run(&self) -> SelfTestResult {
        const SEED_A: u32 = 0x00A1_1CE5;
        const SEED_B: u32 = 0x0B0B_5EED;
        let octets = self.octets;
        let mut a = End::new(self.modem_a, &self.call_a, SEED_A);
        let mut b = End::new(self.modem_b, &self.call_b, SEED_B);
        a.modem.originate();
        b.modem.answer();

        // Thirty characters a second at the most, and half as long again
        // to spare, once the modems are connected.
        let data_limit_ms = (octets as u64 * 1000 / 30) * 3 / 2 + 5000;
        let mut at_ms = 0u64;
        let mut connected_at = None;
        loop {
            let (to_a, to_b) = (b.line, a.line);
            at_ms += 10;
            a.tick(&to_a, at_ms, SEED_B, octets);
            b.tick(&to_b, at_ms, SEED_A, octets);
            if connected_at.is_none() && a.connect.is_some() && b.connect.is_some() {
                connected_at = Some(at_ms);
            }
            let done = a.received >= octets && b.received >= octets;
            let late = match connected_at {
                None => at_ms >= CONNECT_LIMIT_MS,
                Some(since) => at_ms - since >= data_limit_ms,
            };
            if done || late || a.lost || b.lost {
                break;
            }
        }
        a.modem.hangup();
        b.modem.hangup();

        let (line_a, line_b) = (self.a.to_string(), self.b.to_string());
        let whole = a.good == octets && b.good == octets;
        let error = match connected_at {
            None => {
                // The end that got less far is the one to name.
                let (line, state) = if a.connect.is_none() {
                    (&line_a, a.state)
                } else {
                    (&line_b, b.state)
                };
                Some(message("RMD7005", &[&line_a, &line_b, line, state.name()]))
            }
            Some(_) if !whole => Some(message(
                "RMD7006",
                &[
                    &line_a,
                    &line_b,
                    &b.good.to_string(),
                    &a.good.to_string(),
                    &octets.to_string(),
                ],
            )),
            Some(_) => None,
        };
        SelfTestResult {
            a: self.a,
            b: self.b,
            passed: error.is_none(),
            // The answerer names the carrier it was called with.
            connect: b.connect.or(a.connect).map(|(connect, _)| connect),
            connect_ms: connected_at.map(|at| at as u32),
            octets: octets as u32,
            octets_a_to_b: b.good as u32,
            octets_b_to_a: a.good as u32,
            call_ms: at_ms as u32,
            elapsed_ms: 0,
            error: error.as_ref().map(ErrorInfo::from),
        }
    }
}
