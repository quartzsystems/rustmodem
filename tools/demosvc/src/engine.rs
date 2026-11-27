// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The demo engine: the null engine, made to say it negotiated something.
//!
//! The null engine reports a connection of which nothing is known, which
//! is the truth, and leaves the manager's connect rates with one bar. The
//! demo engine carries the data the same way and reports a rate and a
//! carrier from a fixed rota instead, so that the demo has something to
//! show. Nothing is modulated; the names are labels.

use std::sync::{Arc, Mutex};

use engine::{
    ConnectInfo, Diagnostics, EngineState, ModemEngine, ModemEvent, NullConfig, NullEngine,
    SCOPE_SAMPLES, ScopeKind, StateChange,
};

/// How long the demo engine takes to connect, in 20 ms ticks: a second and
/// a half of handshaking for the manager to show.
pub const CONNECT_DELAY_TICKS: u32 = 75;

/// What the calls say they negotiated, in turn: rate in bit/s and carrier.
/// Mostly the fastest, as a pool of real modems would be.
const ROTA: [(u32, &str); 8] = [
    (2400, "V.22bis"),
    (2400, "V.22bis"),
    (1200, "V.22"),
    (2400, "V.22bis"),
    (300, "V.21"),
    (2400, "V.22bis"),
    (1200, "V.22"),
    (2400, "V.22bis"),
];

/// What the engines of one pool share, so that the two ends of a call say
/// the same thing.
#[derive(Debug, Default)]
pub struct Negotiation {
    /// How many calls have been given a turn of the rota.
    calls: usize,
    /// What the originating end of the call being connected chose, until
    /// the answering end has taken it.
    offered: Option<ConnectInfo>,
}

impl Negotiation {
    /// The originating end starts: the call takes the next turn.
    fn originate(&mut self) -> ConnectInfo {
        let (rate, carrier) = ROTA[self.calls % ROTA.len()];
        self.calls += 1;
        let info = ConnectInfo {
            rate,
            carrier: Some(carrier.to_string()),
            protocol: None,
            compression: None,
        };
        self.offered = Some(info.clone());
        info
    }

    /// The answering end starts: it agrees with the originating end, which
    /// the switch starts just before it.
    fn answer(&mut self) -> ConnectInfo {
        match self.offered.take() {
            Some(info) => info,
            None => {
                let info = self.originate();
                self.offered = None;
                info
            }
        }
    }
}

/// The null engine with a connection report from the rota.
pub struct DemoEngine {
    inner: NullEngine,
    shared: Arc<Mutex<Negotiation>>,
    /// What this engine reports when the null engine connects.
    info: ConnectInfo,
    /// Ticks since the call began, and at which of them it connected.
    ticks: u32,
    connected_at: Option<u32>,
    /// Whether there is a call.
    running: bool,
}

impl DemoEngine {
    /// A stopped engine that negotiates with the others that share
    /// `shared`.
    pub fn new(shared: Arc<Mutex<Negotiation>>) -> DemoEngine {
        DemoEngine {
            inner: NullEngine::new(NullConfig {
                connect_delay_ticks: CONNECT_DELAY_TICKS,
                ..NullConfig::default()
            }),
            shared,
            info: ConnectInfo::unknown(),
            ticks: 0,
            connected_at: None,
            running: false,
        }
    }

    fn begin(&mut self) {
        self.ticks = 0;
        self.connected_at = None;
        self.running = true;
    }

    fn negotiate(&mut self, choose: impl FnOnce(&mut Negotiation) -> ConnectInfo) {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        self.info = choose(&mut shared);
    }
}

impl ModemEngine for DemoEngine {
    fn process(&mut self, rx: &[u8], tx: &mut [u8]) {
        self.inner.process(rx, tx);
        self.ticks = self.ticks.saturating_add(1);
    }

    fn dte_write(&mut self, data: &[u8]) -> usize {
        self.inner.dte_write(data)
    }

    fn dte_read(&mut self, buf: &mut [u8]) -> usize {
        self.inner.dte_read(buf)
    }

    fn poll_event(&mut self) -> Option<ModemEvent> {
        let connected_at = self.ticks;
        self.inner.poll_event().map(|event| match event {
            ModemEvent::Connect(_) => {
                self.connected_at = Some(connected_at);
                ModemEvent::Connect(self.info.clone())
            }
            other => other,
        })
    }

    fn answer(&mut self) {
        self.negotiate(Negotiation::answer);
        self.begin();
        self.inner.answer();
    }

    fn originate(&mut self) {
        self.negotiate(Negotiation::originate);
        self.begin();
        self.inner.originate();
    }

    fn hangup(&mut self) {
        self.running = false;
        self.inner.hangup();
    }

    /// What a modem would show of itself, made up as the rest is: the
    /// same for the same tick of a call, so that a scripted call looks
    /// the same on every run.
    fn diagnostics(&self) -> Option<Diagnostics> {
        if !self.running {
            return None;
        }
        let mut snapshot = Diagnostics {
            state: EngineState::Training,
            tx_level_dbm0: -12.0,
            ..Diagnostics::default()
        };
        snapshot.history[0] = StateChange {
            state: EngineState::Training,
            at_ms: 0,
        };
        snapshot.changes = 1;
        if let Some(at) = self.connected_at {
            snapshot.state = EngineState::Data;
            snapshot.rx_level_dbm0 = -14.5;
            snapshot.carrier = true;
            snapshot.frequency_error_hz = 0.4;
            snapshot.quality = 0.7;
            snapshot.history[1] = StateChange {
                state: EngineState::Data,
                at_ms: at * TICK_MS,
            };
            snapshot.changes = 2;
            // A square wave that moves along: marks and spaces of a few
            // samples each.
            snapshot.scope_kind = ScopeKind::Discriminator;
            for (n, sample) in snapshot.scope.iter_mut().enumerate() {
                let place = (n as u32 + self.ticks) % 16;
                *sample = if place < 9 { 90 } else { -90 };
            }
        }
        Some(snapshot)
    }
}

/// The length of a tick, in milliseconds.
const TICK_MS: u32 = 20;

// The scope the snapshot fills is the size the engine crate gives it.
const _: () = assert!(SCOPE_SAMPLES >= 16);

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs two engines against each other until both have connected, and
    /// returns what each reported.
    fn connect(caller: &mut DemoEngine, callee: &mut DemoEngine) -> (ConnectInfo, ConnectInfo) {
        caller.originate();
        callee.answer();
        let mut a = [0u8; 160];
        let mut b = [0u8; 160];
        let mut reports = (None, None);
        for _ in 0..=CONNECT_DELAY_TICKS {
            let from_callee = b;
            caller.process(&from_callee, &mut a);
            callee.process(&a, &mut b);
            if let Some(ModemEvent::Connect(info)) = caller.poll_event() {
                reports.0 = Some(info);
            }
            if let Some(ModemEvent::Connect(info)) = callee.poll_event() {
                reports.1 = Some(info);
            }
        }
        (reports.0.unwrap(), reports.1.unwrap())
    }

    #[test]
    fn both_ends_of_a_call_report_the_same_and_calls_take_turns() {
        let shared = Arc::new(Mutex::new(Negotiation::default()));
        let mut one = DemoEngine::new(Arc::clone(&shared));
        let mut two = DemoEngine::new(Arc::clone(&shared));
        let mut seen = Vec::new();
        for _ in 0..ROTA.len() + 1 {
            let (caller, callee) = connect(&mut one, &mut two);
            assert_eq!(caller, callee);
            seen.push((caller.rate, caller.carrier.clone().unwrap()));
            one.hangup();
            two.hangup();
        }
        let expected: Vec<(u32, String)> = ROTA
            .iter()
            .chain(&ROTA[..1])
            .map(|(rate, carrier)| (*rate, carrier.to_string()))
            .collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn data_goes_through_as_through_the_null_engine() {
        let shared = Arc::new(Mutex::new(Negotiation::default()));
        let mut one = DemoEngine::new(Arc::clone(&shared));
        let mut two = DemoEngine::new(shared);
        connect(&mut one, &mut two);
        assert_eq!(one.dte_write(b"hello"), 5);
        let mut a = [0u8; 160];
        let mut b = [0u8; 160];
        one.process(&b.clone(), &mut a);
        two.process(&a, &mut b);
        let mut received = [0u8; 16];
        let count = two.dte_read(&mut received);
        assert_eq!(&received[..count], b"hello");
    }

    #[test]
    fn an_answer_with_nothing_offered_still_reports_something() {
        let mut shared = Negotiation::default();
        let alone = shared.answer();
        assert_eq!(alone.rate, ROTA[0].0);
        // What it took is not left over for the next call.
        let next = shared.originate();
        assert_eq!(next.rate, ROTA[1].0);
        assert_eq!(shared.answer(), next);
    }
}
