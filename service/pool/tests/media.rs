// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The media seam: the pool hands a connected call's engine and the far
//! end the router gave to a media runner, and has nothing to do with the
//! audio. Here the router and the runner are scripted, and the runner has
//! a clock of its own: it works when the test says, not when the pool
//! ticks.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use common::{Harness, SharedProfiles, config_with_lines, summarise};
use ctlproto::LineState;
use engine::{ConnectInfo, ModemEngine, ModemEvent};
use pool::{
    CallRouter, LineDirectory, LineId, MediaCall, MediaEndpoint, MediaEvent, MediaPort,
    MediaRunner, RouterEvent, TICK_MS, media_pair,
};

/// What the far end of a scripted call is: a number the test chose.
#[derive(Debug, PartialEq, Eq)]
struct Token(u32);

/// What the scripted router was asked to do, and what it is to say.
#[derive(Default)]
struct RouterScript {
    asked: Vec<String>,
    say: VecDeque<RouterEvent>,
}

struct ScriptedRouter(Arc<Mutex<RouterScript>>);

impl CallRouter for ScriptedRouter {
    fn dial(&mut self, _now_ms: u64, from: LineId, number: &str, _lines: &dyn LineDirectory) {
        let mut script = self.0.lock().unwrap();
        script.asked.push(format!("dial {from} {number}"));
    }
    fn answer(&mut self, _now_ms: u64, line: LineId) {
        self.0.lock().unwrap().asked.push(format!("answer {line}"));
    }
    fn hangup(&mut self, _now_ms: u64, line: LineId) {
        self.0.lock().unwrap().asked.push(format!("hangup {line}"));
    }
    fn poll(&mut self, _now_ms: u64) -> Option<RouterEvent> {
        self.0.lock().unwrap().say.pop_front()
    }
    fn shutdown(&mut self, _now_ms: u64) {
        self.0.lock().unwrap().asked.push("shutdown".to_string());
    }
}

/// An engine that carries its data straight back to its own host, and
/// writes down what is done to it.
struct LoopEngine {
    id: usize,
    journal: Arc<Mutex<Vec<String>>>,
    data: VecDeque<u8>,
    events: VecDeque<ModemEvent>,
}

impl ModemEngine for LoopEngine {
    fn process(&mut self, _rx: &[u8], _tx: &mut [u8]) {
        self.journal
            .lock()
            .unwrap()
            .push(format!("engine {} process", self.id));
    }
    fn dte_write(&mut self, data: &[u8]) -> usize {
        self.data.extend(data);
        data.len()
    }
    fn dte_read(&mut self, buf: &mut [u8]) -> usize {
        let count = buf.len().min(self.data.len());
        for (slot, byte) in buf.iter_mut().zip(self.data.drain(..count)) {
            *slot = byte;
        }
        count
    }
    fn poll_event(&mut self) -> Option<ModemEvent> {
        self.events.pop_front()
    }
    fn answer(&mut self) {
        self.journal
            .lock()
            .unwrap()
            .push(format!("engine {} answer", self.id));
        self.events
            .push_back(ModemEvent::Connect(ConnectInfo::with_rate(300)));
    }
    fn originate(&mut self) {
        self.journal
            .lock()
            .unwrap()
            .push(format!("engine {} originate", self.id));
        self.events
            .push_back(ModemEvent::Connect(ConnectInfo::with_rate(300)));
    }
    fn hangup(&mut self) {
        self.journal
            .lock()
            .unwrap()
            .push(format!("engine {} hangup", self.id));
        self.data.clear();
    }
}

/// A call the scripted runner is carrying.
struct Carried {
    line: LineId,
    engine: Box<dyn ModemEngine>,
    port: MediaPort,
}

/// What the scripted runner holds and how it is told to behave.
#[derive(Default)]
struct RunnerScript {
    calls: Vec<Carried>,
    /// What was started: the line, the endpoint and the direction.
    started: Vec<(LineId, Token, bool)>,
    stopped: Vec<LineId>,
    /// Refuse the next endpoint.
    refuse: bool,
    /// Lose the engine of the next call that is stopped.
    lose_engine: bool,
    ticks: usize,
}

impl RunnerScript {
    fn call(&mut self, line: LineId) -> &mut Carried {
        self.calls
            .iter_mut()
            .find(|call| call.line == line)
            .expect("the runner is carrying the line's call")
    }

    /// The runner's own clock: moves data and events across the seam.
    fn work(&mut self, line: LineId) {
        let call = self.call(line);
        call.port.feed(call.engine.as_mut());
        call.port.drain(call.engine.as_mut());
    }
}

struct ScriptedRunner(Arc<Mutex<RunnerScript>>);

impl MediaRunner for ScriptedRunner {
    fn start(
        &mut self,
        line: LineId,
        mut engine: Box<dyn ModemEngine>,
        endpoint: MediaEndpoint,
        originate: bool,
    ) -> Result<MediaCall, Box<dyn ModemEngine>> {
        let mut script = self.0.lock().unwrap();
        let token = match endpoint.downcast::<Token>() {
            Ok(token) if !script.refuse => token,
            _ => {
                script.refuse = false;
                return Err(engine);
            }
        };
        if originate {
            engine.originate();
        } else {
            engine.answer();
        }
        let (call, mut port) = media_pair();
        port.drain(engine.as_mut());
        script.started.push((line, token, originate));
        script.calls.push(Carried { line, engine, port });
        Ok(call)
    }

    fn stop(&mut self, line: LineId, _call: MediaCall) -> Option<Box<dyn ModemEngine>> {
        let mut script = self.0.lock().unwrap();
        script.stopped.push(line);
        let index = script.calls.iter().position(|call| call.line == line)?;
        let mut call = script.calls.swap_remove(index);
        if std::mem::take(&mut script.lose_engine) {
            return None;
        }
        call.engine.hangup();
        Some(call.engine)
    }

    fn tick(&mut self, _line: LineId) {
        self.0.lock().unwrap().ticks += 1;
    }
}

struct World {
    harness: Harness,
    router: Arc<Mutex<RouterScript>>,
    runner: Arc<Mutex<RunnerScript>>,
    journal: Arc<Mutex<Vec<String>>>,
}

impl World {
    /// Two idle lines on a scripted router and a scripted runner.
    fn new() -> World {
        let router = Arc::new(Mutex::new(RouterScript::default()));
        let runner = Arc::new(Mutex::new(RunnerScript::default()));
        let journal = Arc::new(Mutex::new(Vec::new()));
        let engines = {
            let journal = Arc::clone(&journal);
            let mut made = 0;
            Box::new(move |_line: LineId| {
                made += 1;
                Box::new(LoopEngine {
                    id: made,
                    journal: Arc::clone(&journal),
                    data: VecDeque::new(),
                    events: VecDeque::new(),
                }) as Box<dyn ModemEngine>
            })
        };
        let mut harness = Harness::with_parts(
            config_with_lines(2),
            SharedProfiles::default(),
            Box::new(ScriptedRouter(Arc::clone(&router))),
            Box::new(ScriptedRunner(Arc::clone(&runner))),
            engines,
        );
        for id in 1..=2 {
            harness.host(id).open();
        }
        harness.advance(TICK_MS);
        harness.take_events();
        harness.quiet(1);
        harness.quiet(2);
        World {
            harness,
            router,
            runner,
            journal,
        }
    }

    fn say(&mut self, event: RouterEvent) {
        self.router.lock().unwrap().say.push_back(event);
        // The same moment: no tick of the pool's clock passes.
        self.harness.service();
    }

    fn asked(&self) -> Vec<String> {
        std::mem::take(&mut self.router.lock().unwrap().asked)
    }

    fn journal(&self) -> Vec<String> {
        std::mem::take(&mut self.journal.lock().unwrap())
    }

    /// Line 1 dials out and the far end, `token`, answers.
    fn dial_and_connect(&mut self, token: u32) {
        assert_eq!(self.harness.command(1, "ATD5551000"), "");
        assert_eq!(self.asked(), ["dial 1 5551000"]);
        self.say(RouterEvent::Connected {
            line: 1,
            originate: true,
            media: MediaEndpoint::new(Token(token)),
        });
        assert_eq!(self.harness.read(1), "\r\nCONNECT 300\r\n");
        assert_eq!(self.harness.state(1), LineState::Online);
    }
}

#[test]
fn the_endpoint_and_the_engine_go_to_the_runner_when_the_call_connects() {
    let mut world = World::new();
    world.dial_and_connect(7);
    {
        let runner = world.runner.lock().unwrap();
        assert_eq!(runner.started, [(1, Token(7), true)]);
        assert!(runner.stopped.is_empty());
    }
    // The engine was started by the runner, not by the pool, and nothing
    // of the pool ever ticks it.
    assert_eq!(world.journal(), ["engine 1 originate"]);
    world.harness.advance(10 * TICK_MS);
    assert_eq!(world.journal(), Vec::<String>::new());
    // The pool does tell the runner its clock, for a runner that goes by
    // it.
    assert!(world.runner.lock().unwrap().ticks >= 10);

    // An incoming call is started as the answering modem.
    world.say(RouterEvent::Incoming {
        line: 2,
        from: "201".to_string(),
    });
    world.say(RouterEvent::Ring(2));
    assert_eq!(world.harness.read(2), "\r\nRING\r\n");
    assert_eq!(world.harness.command(2, "ATA"), "");
    assert_eq!(world.asked(), ["answer 2"]);
    world.say(RouterEvent::Connected {
        line: 2,
        originate: false,
        media: MediaEndpoint::new(Token(8)),
    });
    assert_eq!(world.harness.read(2), "\r\nCONNECT 300\r\n");
    assert_eq!(
        world.runner.lock().unwrap().started[1],
        (2, Token(8), false)
    );
    assert_eq!(world.journal(), ["engine 2 answer"]);
}

#[test]
fn data_crosses_the_seam_on_the_runners_clock_not_the_pools() {
    let mut world = World::new();
    world.dial_and_connect(7);

    world.harness.host(1).write(b"ping");
    world.harness.advance(5 * TICK_MS);
    // The pool has put it in the ring; nothing comes back until the
    // runner works.
    assert_eq!(world.harness.read(1), "");
    world.runner.lock().unwrap().work(1);
    world.harness.service();
    assert_eq!(world.harness.read(1), "ping");

    // More than a ring holds goes through in pieces and arrives whole.
    let data = common::noise(42, 40_000);
    world.harness.host(1).write(&data);
    let mut back = Vec::new();
    for _ in 0..400 {
        world.harness.advance(TICK_MS);
        world.runner.lock().unwrap().work(1);
        world.harness.service();
        back.extend(world.harness.host(1).read());
        if back.len() >= data.len() {
            break;
        }
    }
    assert!(
        back == data,
        "{} of {} bytes came back",
        back.len(),
        data.len()
    );
    let line = world.harness.pool.line(1).unwrap();
    let call = line.call.unwrap();
    assert_eq!((call.bytes_tx, call.bytes_rx), (40_004, 40_004));
}

#[test]
fn what_the_runner_reports_is_taken_between_ticks() {
    for (event, what) in [
        (
            MediaEvent::Modem(ModemEvent::CarrierLost),
            "the carrier is lost",
        ),
        (MediaEvent::Stalled, "the far end goes silent"),
    ] {
        let mut world = World::new();
        world.dial_and_connect(7);
        world.harness.take_events();

        world.runner.lock().unwrap().call(1).port.report(event);
        // A turn of the pool at the same moment: no tick.
        world.harness.service();
        assert_eq!(world.harness.read(1), "\r\nNO CARRIER\r\n", "{what}");
        assert_eq!(world.harness.state(1), LineState::Idle);
        assert_eq!(
            summarise(&world.harness.take_events()),
            ["call 1 ended on 1 remote", "line 1 idle"]
        );
        // The router is told, so that it clears its side.
        assert_eq!(world.asked(), ["hangup 1"]);
        // The engine came back from the runner, stopped.
        assert_eq!(world.runner.lock().unwrap().stopped, [1]);
        assert_eq!(world.journal(), ["engine 1 originate", "engine 1 hangup"]);

        // The next call on the line runs the same engine.
        world.dial_and_connect(9);
        assert_eq!(world.journal(), ["engine 1 originate"]);
    }
}

#[test]
fn the_engine_comes_back_when_the_host_hangs_up() {
    let mut world = World::new();
    world.dial_and_connect(7);
    world.harness.host(1).set_dtr(false);
    world.harness.advance(TICK_MS);
    assert_eq!(world.asked(), ["hangup 1"]);
    assert_eq!(world.runner.lock().unwrap().stopped, [1]);
    assert!(world.runner.lock().unwrap().calls.is_empty());
    assert_eq!(world.journal(), ["engine 1 originate", "engine 1 hangup"]);

    // And when the far end clears the call.
    world.harness.host(1).set_dtr(true);
    world.harness.advance(TICK_MS);
    world.harness.read(1);
    world.dial_and_connect(8);
    world.say(RouterEvent::Cleared(1));
    assert_eq!(world.harness.read(1), "\r\nNO CARRIER\r\n");
    assert_eq!(world.runner.lock().unwrap().stopped, [1, 1]);
    assert_eq!(
        world.journal(),
        ["engine 1 originate", "engine 1 hangup"],
        "the same engine again"
    );
}

#[test]
fn a_call_whose_endpoint_the_runner_cannot_use_fails() {
    let mut world = World::new();
    assert_eq!(world.harness.command(1, "ATD5551000"), "");
    world.asked();
    world.harness.take_events();
    world.runner.lock().unwrap().refuse = true;
    world.say(RouterEvent::Connected {
        line: 1,
        originate: true,
        media: MediaEndpoint::new(Token(7)),
    });
    assert_eq!(world.harness.read(1), "\r\nNO CARRIER\r\n");
    assert_eq!(
        summarise(&world.harness.take_events()),
        ["call 1 ended on 1 failed", "line 1 idle"]
    );
    assert_eq!(world.asked(), ["hangup 1"]);
    // The engine was never started and is still the line's.
    assert_eq!(world.journal(), Vec::<String>::new());
    world.dial_and_connect(8);
    assert_eq!(world.journal(), ["engine 1 originate"]);

    // An endpoint of a kind the runner does not know is refused the same.
    let mut world = World::new();
    assert_eq!(world.harness.command(1, "ATD5551000"), "");
    world.say(RouterEvent::Connected {
        line: 1,
        originate: true,
        media: MediaEndpoint::new("not a token"),
    });
    assert_eq!(world.harness.read(1), "\r\nNO CARRIER\r\n");
}

#[test]
fn a_runner_that_loses_the_engine_costs_the_line_only_that_engine() {
    let mut world = World::new();
    world.dial_and_connect(7);
    world.runner.lock().unwrap().lose_engine = true;
    world.say(RouterEvent::Cleared(1));
    assert_eq!(world.harness.read(1), "\r\nNO CARRIER\r\n");
    assert_eq!(world.journal(), ["engine 1 originate"]);
    // The line's next call gets a new engine. (The two lines' first
    // engines were 1 and 2.)
    world.dial_and_connect(8);
    assert_eq!(world.journal(), ["engine 3 originate"]);
}

#[test]
fn a_line_that_went_on_hook_before_the_call_was_through_starts_no_media() {
    let mut world = World::new();
    assert_eq!(world.harness.command(1, "ATD5551000"), "");
    world.harness.host(1).set_dtr(false);
    world.harness.advance(TICK_MS);
    world.asked();
    world.say(RouterEvent::Connected {
        line: 1,
        originate: true,
        media: MediaEndpoint::new(Token(7)),
    });
    assert!(world.runner.lock().unwrap().started.is_empty());
    assert_eq!(world.asked(), ["hangup 1"]);
}

#[test]
fn shutting_down_stops_the_media_and_then_the_router() {
    let mut world = World::new();
    world.dial_and_connect(7);
    world.asked();
    let now = world.harness.now();
    world.harness.pool.shutdown(now);
    assert_eq!(world.runner.lock().unwrap().stopped, [1]);
    assert_eq!(world.asked(), ["hangup 1", "shutdown"]);
}
