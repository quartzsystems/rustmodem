// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Two modem engines calling each other through the channel simulator.

#![allow(dead_code)]

use chansim::{ChannelConfig, Duplex, Rng};
use engine::{CallSettings, ConnectInfo, Modem, ModemConfig, ModemEngine, ModemEvent, Modulation};

/// A modem that allows these modulations, in this order.
pub fn config(modulations: &[Modulation], v8: bool) -> ModemConfig {
    ModemConfig {
        modulations: engine::modulations(modulations),
        v8,
        ..ModemConfig::default()
    }
}

/// The 300 bit/s modem: V.21 and then Bell 103, with V.8.
pub fn default_config() -> ModemConfig {
    config(&[Modulation::V21, Modulation::Bell103], true)
}

/// A call between two engines: end A originates and end B answers.
pub struct Call {
    pub duplex: Duplex,
}

/// Ticks in a second.
pub const SECOND: u64 = 100;

impl Call {
    /// Places the call. Each engine is told the law of its end of the
    /// channel and the host's settings, as a media runner and the pool
    /// tell it, and then to originate or to answer.
    pub fn place(
        originator: ModemConfig,
        answerer: ModemConfig,
        channel: &ChannelConfig,
        settings: CallSettings,
    ) -> Call {
        let mut a: Box<dyn ModemEngine> = Box::new(Modem::new(originator));
        let mut b: Box<dyn ModemEngine> = Box::new(Modem::new(answerer));
        a.set_law(channel.law_a);
        b.set_law(channel.law_b);
        a.set_call(&settings);
        b.set_call(&settings);
        a.originate();
        b.answer();
        Call {
            duplex: Duplex::new(a, b, channel),
        }
    }

    /// Runs until both ends have connected, or failed, or `seconds` have
    /// passed. Says whether both connected.
    pub fn connect(&mut self, seconds: u64) -> bool {
        let limit = self.duplex.b.ticks + seconds * SECOND;
        self.duplex.run_until(limit, |d| {
            let over = |end: &chansim::End| end.connected() || end.carrier_lost();
            over(&d.a) && over(&d.b)
        });
        self.duplex.a.connected() && self.duplex.b.connected()
    }

    /// The connection each end reported: the originator's, then the
    /// answerer's.
    pub fn connections(&self) -> (Option<ConnectInfo>, Option<ConnectInfo>) {
        let of = |end: &chansim::End| {
            end.events.iter().find_map(|event| match &event.what {
                ModemEvent::Connect(info) => Some(info.clone()),
                _ => None,
            })
        };
        (of(&self.duplex.a), of(&self.duplex.b))
    }

    /// When each end connected, in milliseconds of its own clock.
    pub fn connected_at_ms(&self) -> (u64, u64) {
        let at = |end: &chansim::End| {
            end.first(|event| matches!(event, ModemEvent::Connect(_)))
                .map_or(0, |event| event.tick * 10)
        };
        (at(&self.duplex.a), at(&self.duplex.b))
    }

    /// Sends data both ways and runs until all of it has arrived or it
    /// has had the time it needs twice over. Returns what each end
    /// received: the originator's, then the answerer's.
    pub fn exchange(&mut self, from_a: &[u8], from_b: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let already_a = self.duplex.a.received.len();
        let already_b = self.duplex.b.received.len();
        self.duplex.a.send(from_a);
        self.duplex.b.send(from_b);
        // Thirty characters a second, and a few seconds over.
        let longest = from_a.len().max(from_b.len()) as u64;
        let limit = self.duplex.b.ticks + 2 * (longest * SECOND / 30) + 5 * SECOND;
        let (want_a, want_b) = (already_a + from_b.len(), already_b + from_a.len());
        self.duplex.run_until(limit, |d| {
            d.a.received.len() >= want_a && d.b.received.len() >= want_b
        });
        // A little longer, to see that nothing more arrives than was
        // sent.
        self.duplex.run(SECOND / 2);
        (
            self.duplex.a.received[already_a..].to_vec(),
            self.duplex.b.received[already_b..].to_vec(),
        )
    }
}

/// Data from a seed.
pub fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

/// The connection an engine reports on a modem.
pub fn connection(standard: Modulation) -> ConnectInfo {
    ConnectInfo {
        rate: standard.rate(),
        carrier: Some(standard.carrier_name().to_string()),
        protocol: None,
        compression: None,
    }
}
