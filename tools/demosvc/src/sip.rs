// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The `sip` script: SIP on the loopback interface, with test peers
//! placing calls.
//!
//! The demo service runs the real SIP router and the real media threads,
//! as the service does. The far end is a handful of `sippeer` agents in
//! this process: the first is the trunk peer and the PBX the last line
//! registers to, and all of them call the pilot number of a hunt group
//! that holds every line, talk for a while with a pattern as their media,
//! and hang up. The lines answer by themselves. One caller's packets are
//! lost, doubled and delayed now and then, so that there is something to
//! see in the statistics.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use config::{
    Cidr, Config, EngineKind, HuntGroup, PortRange, RegisterConfig, Route, Strategy, Target,
};
use ctlproto::{Event as ControlEvent, LogEvent, LogLevel, SipTrace, TraceDirection};
use ctlsrv::{Clock, Hub};
use media::{CaptureEvent, CaptureWriter, ThreadedRunner};
use msg::{Message, Severity, message};
use pool::{CallRouter, LineId, MediaRunner};
use sippeer::{Agent, AgentConfig, Impairments, MediaKind, PeerEvent};
use svc::sipsock::SipSocket;
use telephony::{
    Direction, Observer, RouterParts, Settings, SipRouter, TelephonyHandle, system_random,
};

use crate::busy::Rng;

/// The tag of the SIP router in log lines.
const SOURCE: &str = "SIP";

/// The tag of the capture writer in log lines.
const MEDIA_SOURCE: &str = "RTP";

/// The name of the capture directory under the demo service's own.
pub const CAPTURES: &str = "captures";

/// The number that calls the hunt group.
const PILOT: &str = "5551000";

/// The name of the hunt group that holds every line.
const GROUP: &str = "dialup";

/// The ports the demo service's RTP uses.
const RTP_PORTS: PortRange = PortRange {
    first: 26_000,
    last: 26_999,
};

/// The most callers there are at once.
const MAX_CALLERS: usize = 4;

fn loopback(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

/// The far end: the agents that call, the first of which is also the
/// trunk peer and the registrar.
pub struct Callers {
    agents: Vec<Agent>,
}

impl Callers {
    /// Binds the agents: as many as half the lines, and at most
    /// [`MAX_CALLERS`], so that some lines are always free.
    pub fn bind(lines: u16) -> std::io::Result<Callers> {
        let count = (usize::from(lines) / 2).clamp(1, MAX_CALLERS);
        let agents = (0..count)
            .map(|index| {
                Agent::bind(AgentConfig {
                    media: MediaKind::Pattern,
                    seed: 0xD1A1 + index as u64,
                    // The second caller's network is a poor one.
                    impairments: if index == 1 {
                        Impairments {
                            loss: 8,
                            duplicate: 4,
                            reorder: 4,
                            jitter_ms: 6,
                            clock_ppm: 40,
                        }
                    } else {
                        Impairments::default()
                    },
                    ..AgentConfig::new(&format!("555010{index}"), loopback(0))
                })
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Callers { agents })
    }

    /// Where the trunk peer, which is also the registrar, is.
    pub fn peer(&self) -> SocketAddrV4 {
        self.agents[0].sip_address()
    }

    /// Plays the far end for ever: each agent calls the pilot number,
    /// talks, hangs up and waits, with timings that are the same at every
    /// run.
    pub fn run(mut self, service: SocketAddrV4) {
        #[derive(Clone, Copy)]
        enum Plan {
            /// Calls at this time.
            Waiting(Instant),
            /// Has called and gives up at this time.
            Calling(Instant),
            /// Is through and hangs up at this time.
            Talking(Instant),
        }
        let uri = format!("sip:{PILOT}@{service}");
        let mut rng = Rng::new(0x51B0_CA11);
        let began = Instant::now();
        // The first call comes at once, the others a few seconds apart.
        let mut plans: Vec<Plan> = (0..self.agents.len())
            .map(|index| Plan::Waiting(began + Duration::from_secs(1 + 4 * index as u64)))
            .collect();
        loop {
            let now = Instant::now();
            for (agent, plan) in self.agents.iter_mut().zip(&mut plans) {
                // A socket error on loopback is not one to stop for.
                let _ = agent.step();
                let events = agent.events();
                let through = events
                    .iter()
                    .any(|event| matches!(event, PeerEvent::Answered { .. }));
                let over = events.iter().any(|event| {
                    matches!(
                        event,
                        PeerEvent::Refused { .. }
                            | PeerEvent::HungUp { .. }
                            | PeerEvent::TimedOut { .. }
                    )
                });
                let pause =
                    |rng: &mut Rng| Plan::Waiting(now + Duration::from_secs(rng.between(4, 15)));
                *plan = match *plan {
                    Plan::Waiting(at) if now >= at => {
                        agent.invite(&uri, service);
                        Plan::Calling(now + Duration::from_secs(40))
                    }
                    Plan::Calling(_) if through => {
                        Plan::Talking(now + Duration::from_secs(rng.between(15, 90)))
                    }
                    Plan::Calling(_) if over => pause(&mut rng),
                    Plan::Calling(give_up) if now >= give_up => {
                        agent.cancel();
                        pause(&mut rng)
                    }
                    Plan::Talking(_) if over => pause(&mut rng),
                    Plan::Talking(hang_at) if now >= hang_at => {
                        agent.hang_up();
                        pause(&mut rng)
                    }
                    other => other,
                };
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Makes a configuration one for the `sip` script: SIP on at `listen`
/// with `peer` as the trunk peer, a hunt group of every line that the
/// pilot number and everything else in 555 is routed to, every line
/// answering by itself, every other line with the echo engine, and the
/// last line an extension of the peer.
pub fn configure(config: &mut Config, listen: SocketAddrV4, peer: SocketAddrV4) {
    config.sip.enabled = true;
    config.sip.listen = listen;
    config.sip.trusted_peers = vec![Cidr {
        address: Ipv4Addr::LOCALHOST,
        prefix: 32,
    }];
    config.sip.outbound_peer = peer.to_string();
    config.media.rtp_ports = RTP_PORTS;
    let last = config.lines.len();
    for (index, line) in config.lines.iter_mut().enumerate() {
        line.auto_answer_rings = 1;
        if index % 2 == 0 {
            line.engine = EngineKind::Echo;
        }
        if index + 1 == last && last > 1 {
            line.register = Some(RegisterConfig {
                server: peer.to_string(),
                user: format!("2{:02}", line.id),
                password: "demo".to_string(),
                auth_user: String::new(),
                expires_s: RegisterConfig::DEFAULT_EXPIRES_S,
            });
        }
    }
    config.hunt_groups = vec![HuntGroup {
        name: GROUP.to_string(),
        pilot: PILOT.to_string(),
        lines: config.lines.iter().map(|line| line.id).collect(),
        strategy: Strategy::RoundRobin,
    }];
    config.routes = vec![Route {
        called: "555*".to_string(),
        target: Target::Group(GROUP.to_string()),
    }];
}

/// Says what a message says as a log event at the demo service's level.
fn log(hub: &Hub, clock: &Clock, source: &str, message: &Message) {
    hub.publish(&ControlEvent::Log(LogEvent {
        time_ms: clock(),
        level: match message.severity {
            Severity::Info => LogLevel::Info,
            Severity::Warning => LogLevel::Warning,
            Severity::Error => LogLevel::Error,
        },
        source: source.to_string(),
        text: message.to_string(),
    }));
}

/// Takes what the router has to say as it happens.
struct Watcher {
    hub: Hub,
    clock: Clock,
}

impl Observer for Watcher {
    fn trace(&mut self, direction: Direction, peer: SocketAddr, text: &str) {
        if !self.hub.wants("sip.trace") {
            return;
        }
        self.hub.publish(&ControlEvent::SipTrace(SipTrace {
            time_ms: (self.clock)(),
            direction: match direction {
                Direction::Sent => TraceDirection::Sent,
                Direction::Received => TraceDirection::Received,
            },
            peer: peer.to_string(),
            text: text.to_string(),
        }));
    }

    fn log(&mut self, line: Option<LineId>, message: &Message) {
        match line {
            Some(line) => log(&self.hub, &self.clock, &format!("Line{line:02}"), message),
            None => log(&self.hub, &self.clock, SOURCE, message),
        }
    }
}

/// The SIP listener, bound before the configuration is written, so that
/// the configuration can say where it is. It is bound on the runtime its
/// task is to run on.
pub fn listen() -> Result<SipSocket, Message> {
    let any = loopback(0);
    SipSocket::bind(any, Arc::new(|| {}))
        .map_err(|error| message("RMD5002", &[&any.to_string(), &error.to_string()]))
}

/// The router and the media runner that carries its calls.
pub type Parts = (Box<dyn CallRouter>, Box<dyn MediaRunner>);

/// The router on `transport` and the media runner, for a pool.
pub fn start(
    config: &Config,
    transport: SipSocket,
    directory: &Path,
    hub: &Hub,
    clock: &Clock,
) -> Result<Parts, Message> {
    let failed = |error: std::io::Error| message("RMD0003", &[&error.to_string()]);
    let listening = message("RMD5001", &[&transport.local().to_string()]);
    let (capture_hub, capture_clock) = (hub.clone(), Arc::clone(clock));
    let writer = CaptureWriter::new(
        directory.join(CAPTURES),
        Box::new(move |event| {
            let said = match event {
                CaptureEvent::Started { line, path } => {
                    message("RMD6006", &[&line.to_string(), &path.display().to_string()])
                }
                CaptureEvent::Failed { line, path, error } => message(
                    "RMD6007",
                    &[
                        &line.to_string(),
                        &path.display().to_string(),
                        &error.to_string(),
                    ],
                ),
            };
            log(&capture_hub, &capture_clock, MEDIA_SOURCE, &said);
        }),
    )
    .map_err(failed)?;
    let runner = ThreadedRunner::with_capture(config.lines.len(), Arc::new(|| {}), writer)
        .map_err(failed)?;
    let router = SipRouter::new(RouterParts {
        settings: Settings::new(config, crate::VERSION),
        transport: Box::new(transport),
        random: system_random(),
        observer: Box::new(Watcher {
            hub: hub.clone(),
            clock: Arc::clone(clock),
        }),
        handle: TelephonyHandle::new(),
        version: crate::VERSION.to_string(),
    });
    log(hub, clock, SOURCE, &listening);
    Ok((Box::new(router), Box::new(runner)))
}
