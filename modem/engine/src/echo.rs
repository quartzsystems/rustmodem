// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The echo engine: a stand-in that sends back what it receives.
//!
//! It is for trying a telephone path by ear: a caller who reaches a line
//! with the echo engine hears themselves, a packet time later. It is not a
//! modem and carries no data: bytes from the host are taken and dropped,
//! and the host is given none.

use std::collections::VecDeque;

use crate::{ConnectInfo, ModemEngine, ModemEvent, SILENCE_OCTET};

/// The carrier name the echo engine reports.
pub const ECHO_CARRIER: &str = "ECHO";

/// The engine that echoes.
#[derive(Clone, Debug, Default)]
pub struct EchoEngine {
    running: bool,
    events: VecDeque<ModemEvent>,
}

impl EchoEngine {
    /// A stopped engine.
    pub fn new() -> EchoEngine {
        EchoEngine::default()
    }

    fn start(&mut self) {
        if self.running {
            return;
        }
        self.running = true;
        // There is nothing to negotiate: it is connected at once, at no
        // rate, on a carrier of its own name.
        self.events.push_back(ModemEvent::Connect(ConnectInfo {
            rate: 0,
            carrier: Some(ECHO_CARRIER.to_owned()),
            protocol: None,
            compression: None,
        }));
    }
}

impl ModemEngine for EchoEngine {
    fn process(&mut self, rx: &[u8], tx: &mut [u8]) {
        debug_assert_eq!(rx.len(), tx.len(), "rx and tx are of equal length");
        if self.running {
            tx.copy_from_slice(rx);
        } else {
            tx.fill(SILENCE_OCTET);
        }
    }

    fn dte_write(&mut self, data: &[u8]) -> usize {
        // Taken and dropped, so that a host that types does not fill its
        // line's buffers.
        if self.running { data.len() } else { 0 }
    }

    fn dte_read(&mut self, _buf: &mut [u8]) -> usize {
        0
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
        self.running = false;
        self.events.clear();
    }
}
