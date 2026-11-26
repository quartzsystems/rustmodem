// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The null engine: a test stand-in with the shape of a modem.
//!
//! Two null engines are joined by feeding the `tx` octets of each into the
//! `rx` of the other. The octet stream is not audio. It is the data itself,
//! framed like this:
//!
//! - [`SILENCE_OCTET`] is what an engine sends while it is stopped. It is
//!   the absence of a carrier.
//! - [`IDLE_OCTET`] is the carrier with nothing to say.
//! - Any other octet is a data byte, except that a data byte equal to one of
//!   the three reserved octets is sent as the escape octet followed by the
//!   byte with one bit flipped.
//!
//! A started engine that has seen the carrier and then receives nothing but
//! silence for a configurable number of ticks reports the carrier lost.

use std::collections::VecDeque;

use crate::{ConnectInfo, ModemEngine, ModemEvent};

/// What a stopped engine sends: digital silence in G.711 mu-law.
pub const SILENCE_OCTET: u8 = 0xFF;

/// What a started engine sends when it has no data: the idle pattern.
pub const IDLE_OCTET: u8 = 0x7E;

/// Introduces a data byte that would otherwise read as a reserved octet.
const ESCAPE_OCTET: u8 = 0x7D;

/// The bit flipped in an escaped data byte.
const ESCAPE_FLIP: u8 = 0x20;

/// The most data from the host the engine holds waiting to be sent.
const TX_CAPACITY: usize = 16 * 1024;

/// The settings of a null engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NullConfig {
    /// Ticks between being started and reporting the connection. With 0
    /// the connection is reported at once.
    pub connect_delay_ticks: u32,
    /// Ticks of silence, after the carrier has been seen, that mean the
    /// carrier is lost.
    pub carrier_loss_ticks: u32,
}

impl Default for NullConfig {
    fn default() -> Self {
        NullConfig {
            connect_delay_ticks: 0,
            carrier_loss_ticks: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Not started: sends silence, ignores what it receives.
    Stopped,
    /// Started; the connection is reported when the ticks run out.
    Connecting { ticks_left: u32 },
    /// Connected: carries data.
    Connected,
}

/// The stand-in engine of phase 0.
#[derive(Clone, Debug)]
pub struct NullEngine {
    config: NullConfig,
    phase: Phase,
    /// Data from the host, waiting to be framed into `tx` octets.
    tx: VecDeque<u8>,
    /// The second octet of an escaped byte that did not fit in the tick.
    tx_spill: Option<u8>,
    /// Data taken out of `rx` octets, waiting for the host.
    rx: VecDeque<u8>,
    /// The last octet received was the escape octet.
    rx_escaped: bool,
    carrier_seen: bool,
    silent_ticks: u32,
    events: VecDeque<ModemEvent>,
}

impl NullEngine {
    /// A stopped engine.
    pub fn new(config: NullConfig) -> NullEngine {
        NullEngine {
            config,
            phase: Phase::Stopped,
            tx: VecDeque::new(),
            tx_spill: None,
            rx: VecDeque::new(),
            rx_escaped: false,
            carrier_seen: false,
            silent_ticks: 0,
            events: VecDeque::new(),
        }
    }

    /// Whether the engine has reported a connection that is still up.
    pub fn is_connected(&self) -> bool {
        self.phase == Phase::Connected
    }

    fn start(&mut self) {
        if self.phase != Phase::Stopped {
            return;
        }
        self.clear();
        if self.config.connect_delay_ticks == 0 {
            self.connected();
        } else {
            self.phase = Phase::Connecting {
                ticks_left: self.config.connect_delay_ticks,
            };
        }
    }

    fn connected(&mut self) {
        self.phase = Phase::Connected;
        // Rate 0: the null engine has no line rate.
        self.events
            .push_back(ModemEvent::Connect(ConnectInfo::unknown()));
    }

    fn clear(&mut self) {
        self.tx.clear();
        self.tx_spill = None;
        self.rx.clear();
        self.rx_escaped = false;
        self.carrier_seen = false;
        self.silent_ticks = 0;
    }

    fn receive(&mut self, octets: &[u8]) {
        let mut carrier = false;
        for octet in octets {
            match *octet {
                SILENCE_OCTET => self.rx_escaped = false,
                IDLE_OCTET => {
                    carrier = true;
                    self.rx_escaped = false;
                }
                ESCAPE_OCTET => {
                    carrier = true;
                    self.rx_escaped = true;
                }
                data => {
                    carrier = true;
                    let escaped = std::mem::take(&mut self.rx_escaped);
                    self.rx
                        .push_back(if escaped { data ^ ESCAPE_FLIP } else { data });
                }
            }
        }
        if carrier {
            self.carrier_seen = true;
            self.silent_ticks = 0;
        } else if self.carrier_seen && self.phase == Phase::Connected {
            self.silent_ticks += 1;
            if self.silent_ticks >= self.config.carrier_loss_ticks {
                self.phase = Phase::Stopped;
                self.clear();
                self.events.push_back(ModemEvent::CarrierLost);
            }
        }
    }

    fn transmit(&mut self, octets: &mut [u8]) {
        let mut at = 0;
        if at < octets.len()
            && let Some(spill) = self.tx_spill.take()
        {
            octets[at] = spill;
            at += 1;
        }
        while at < octets.len() && self.phase == Phase::Connected {
            let Some(byte) = self.tx.pop_front() else {
                break;
            };
            if matches!(byte, SILENCE_OCTET | IDLE_OCTET | ESCAPE_OCTET) {
                octets[at] = ESCAPE_OCTET;
                at += 1;
                let flipped = byte ^ ESCAPE_FLIP;
                if at < octets.len() {
                    octets[at] = flipped;
                    at += 1;
                } else {
                    self.tx_spill = Some(flipped);
                }
            } else {
                octets[at] = byte;
                at += 1;
            }
        }
        octets[at..].fill(IDLE_OCTET);
    }
}

impl ModemEngine for NullEngine {
    fn process(&mut self, rx: &[u8], tx: &mut [u8]) {
        debug_assert_eq!(rx.len(), tx.len(), "rx and tx are of equal length");
        if self.phase == Phase::Stopped {
            tx.fill(SILENCE_OCTET);
            return;
        }
        self.receive(rx);
        match self.phase {
            // The carrier was lost in this tick.
            Phase::Stopped => {
                tx.fill(SILENCE_OCTET);
                return;
            }
            Phase::Connecting { ticks_left } => {
                if ticks_left <= 1 {
                    self.connected();
                } else {
                    self.phase = Phase::Connecting {
                        ticks_left: ticks_left - 1,
                    };
                }
            }
            Phase::Connected => {}
        }
        self.transmit(tx);
    }

    fn dte_write(&mut self, data: &[u8]) -> usize {
        if self.phase != Phase::Connected {
            return 0;
        }
        let room = TX_CAPACITY.saturating_sub(self.tx.len());
        let take = data.len().min(room);
        self.tx.extend(&data[..take]);
        take
    }

    fn dte_read(&mut self, buf: &mut [u8]) -> usize {
        let take = buf.len().min(self.rx.len());
        for (slot, byte) in buf.iter_mut().zip(self.rx.drain(..take)) {
            *slot = byte;
        }
        take
    }

    fn poll_event(&mut self) -> Option<ModemEvent> {
        self.events.pop_front()
    }

    fn answer(&mut self) {
        self.start();
    }

    fn originate(&mut self) {
        self.start();
    }

    fn hangup(&mut self) {
        self.phase = Phase::Stopped;
        self.clear();
        self.events.clear();
    }
}
