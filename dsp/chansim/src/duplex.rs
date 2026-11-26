// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Two modem engines against each other through a channel.

use std::collections::VecDeque;

use engine::{FRAME_10MS, ModemEngine, ModemEvent};

use crate::{Channel, ChannelConfig, Side};

/// Something that happened at an end, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed<T> {
    /// The tick of that end at which it happened, from 0. A tick is 10 ms
    /// on the end's own clock.
    pub tick: u64,
    /// What happened.
    pub what: T,
}

/// One end of a [`Duplex`]: an engine, the data waiting to be sent through
/// it, and what has come out of it.
pub struct End {
    /// The engine.
    pub engine: Box<dyn ModemEngine>,
    /// Data from the host that the engine has not taken yet.
    pub to_send: VecDeque<u8>,
    /// The data the engine has given the host.
    pub received: Vec<u8>,
    /// The events the engine has reported.
    pub events: Vec<Timed<ModemEvent>>,
    /// The ticks this end has run.
    pub ticks: u64,
    // The end's own time, in samples of the other end's clock.
    time: f64,
}

impl End {
    fn new(engine: Box<dyn ModemEngine>) -> End {
        End {
            engine,
            to_send: VecDeque::new(),
            received: Vec::new(),
            events: Vec::new(),
            ticks: 0,
            time: 0.0,
        }
    }

    /// Queues data for the engine to send.
    pub fn send(&mut self, data: &[u8]) {
        self.to_send.extend(data);
    }

    /// The first event of a kind, if there has been one.
    pub fn first(&self, wanted: impl Fn(&ModemEvent) -> bool) -> Option<&Timed<ModemEvent>> {
        self.events.iter().find(|event| wanted(&event.what))
    }

    /// Whether the engine has reported that it connected.
    pub fn connected(&self) -> bool {
        self.first(|event| matches!(event, ModemEvent::Connect(_)))
            .is_some()
    }

    /// Whether the engine has reported that it lost the carrier.
    pub fn carrier_lost(&self) -> bool {
        self.first(|event| matches!(event, ModemEvent::CarrierLost))
            .is_some()
    }

    fn tick(&mut self, side: Side, channel: &mut Channel) {
        let mut rx = [0u8; FRAME_10MS];
        let mut tx = [0u8; FRAME_10MS];
        channel.receive(side, &mut rx);

        // The host's side, as the pool does it between ticks: offer what
        // is waiting, and take what has arrived.
        let (front, _) = self.to_send.as_slices();
        if !front.is_empty() {
            let taken = self.engine.dte_write(front);
            self.to_send.drain(..taken);
        }
        self.engine.process(&rx, &mut tx);
        let mut buf = [0u8; 256];
        loop {
            let n = self.engine.dte_read(&mut buf);
            if n == 0 {
                break;
            }
            self.received.extend_from_slice(&buf[..n]);
        }
        while let Some(event) = self.engine.poll_event() {
            self.events.push(Timed {
                tick: self.ticks,
                what: event,
            });
        }
        channel.send(side, &tx);
        self.ticks += 1;
    }
}

/// Two engines joined through a channel, each on its own clock.
///
/// Each end ticks 10 ms at a time: it receives 80 octets, runs its engine,
/// and sends 80. With no clock offset the two tick in turn, A first. With
/// one, the end whose clock is fast ticks a little more often, as it would
/// in the world: after a million ticks of the slow end, an end 100 parts
/// in a million fast has made 1,000,100.
pub struct Duplex {
    /// End A.
    pub a: End,
    /// End B.
    pub b: End,
    channel: Channel,
    // What a tick of A is in B's samples.
    a_tick: f64,
}

impl Duplex {
    /// Joins two engines. Neither is started: the caller tells one to
    /// answer and the other to originate.
    pub fn new(a: Box<dyn ModemEngine>, b: Box<dyn ModemEngine>, config: &ChannelConfig) -> Duplex {
        let a_per_b = 1.0 + config.clock_offset_ppm * 1.0e-6;
        Duplex {
            a: End::new(a),
            b: End::new(b),
            channel: Channel::new(config),
            a_tick: FRAME_10MS as f64 / a_per_b,
        }
    }

    /// The channel, for its counts.
    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    /// Runs until end B has made this many more ticks.
    pub fn run(&mut self, ticks: u64) {
        let until = self.b.ticks + ticks;
        self.run_until(until, |_| false);
    }

    /// Runs until `done` says so, looking after every tick of end B, or
    /// until end B has made `limit` ticks in all. Says whether `done` did.
    pub fn run_until(&mut self, limit: u64, mut done: impl FnMut(&Duplex) -> bool) -> bool {
        while self.b.ticks < limit {
            // Whichever end is behind in time goes next; A when they are
            // level.
            if self.a.time <= self.b.time {
                self.a.tick(Side::A, &mut self.channel);
                self.a.time += self.a_tick;
            } else {
                self.b.tick(Side::B, &mut self.channel);
                self.b.time += FRAME_10MS as f64;
                if done(self) {
                    return true;
                }
            }
        }
        false
    }
}
