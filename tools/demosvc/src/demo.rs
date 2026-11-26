// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Putting the demo service together and running it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use config::{Config, ConfigFile, ControlConfig, LineConfig};
use ctlproto::{Event as ControlEvent, LogEvent, LogLevel, ServiceMode};
use ctlsrv::{Clock, Hub, Limits, Listener, Server, ServerParts};
use msg::{Message, Severity, message};
use pipe::{Event, PipeListener, PipeSecurity};
use pool::{
    CallRouter, EngineFactory, EventSink, InlineRunner, MediaRunner, MemoryProfiles, Now,
    NullSwitch, Pool, PoolHandle, PoolInbox, PoolParts, TICK_MS, command_channel,
};

use crate::args::{Options, Script};
use crate::busy::{Driver, STEP_MS};
use crate::engine::{DemoEngine, Negotiation};
use crate::hosts::{Hosts, demo_ports, port_of_line};
use crate::{frozen, sip};

/// The tag of the service itself in log lines.
const SOURCE: &str = "SVC";

/// The tag of the control server in log lines.
const CONTROL_SOURCE: &str = "CTL";

/// The name of the configuration file the demo service makes for itself.
const CONFIG_FILE: &str = "rustmodem-demo.toml";

/// How long the pool's thread sleeps between turns of its loop.
const POOL_SLEEP: Duration = Duration::from_millis(5);

/// Everything the pool publishes goes to the control clients, log lines
/// and all: the demo service keeps no log file.
struct Sink(Hub);

impl EventSink for Sink {
    fn publish(&mut self, event: ControlEvent) {
        self.0.publish(&event);
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// The directory the demo service keeps its configuration file in. Each
/// pipe name has its own, so that two demo services do not share a file,
/// and it is made afresh at every start.
fn data_dir(pipe: &str) -> PathBuf {
    std::env::temp_dir().join("rustmodem-demo").join(pipe)
}

/// The configuration the demo service starts with: lines 1 to `lines` on
/// the service's ends of the pretended com0com pairs.
fn starting_config(options: &Options) -> Config {
    let lines = (1..=options.lines)
        .map(|id| {
            let mut line = LineConfig::new(id, &port_of_line(id));
            if options.script == Script::Frozen && id == frozen::DISABLED_LINE {
                line.enabled = false;
            }
            line
        })
        .collect();
    let mut config = Config {
        control: ControlConfig {
            pipe: options.pipe.clone(),
        },
        lines,
        ..Config::default()
    };
    if options.script == Script::Frozen {
        frozen::arrange(&mut config);
    }
    config
}

/// Runs the demo service until the process is ended. Returns only if it
/// could not start.
pub fn run(options: &Options) -> Result<(), Vec<Message>> {
    let one = |problem: Message| vec![problem];
    let frozen = options.script == Script::Frozen;

    // A configuration file of its own, so that the changes the control API
    // writes have somewhere to go.
    let directory = data_dir(&options.pipe);
    let config_path = directory.join(CONFIG_FILE);
    let unusable = |e: std::io::Error| {
        message(
            "RMD0011",
            &[&config_path.display().to_string(), &e.to_string()],
        )
    };
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).map_err(|e| one(unusable(e)))?;
    let mut config = starting_config(options);
    // The `sip` script binds its sockets first: the configuration says
    // where they are.
    let mut far_end = None;
    if options.script == Script::Sip {
        let transport = sip::listen().map_err(one)?;
        let callers = sip::Callers::bind(options.lines)
            .map_err(|e| one(message("RMD0003", &[&e.to_string()])))?;
        sip::configure(&mut config, transport.local(), callers.peer());
        far_end = Some((transport, callers));
    }
    let file = ConfigFile::from_config(&config)?;
    file.save(&config_path).map_err(one)?;

    let stop = Arc::new(Event::new().map_err(|e| one(message("RMD0003", &[&e.to_string()])))?);
    let mut listener =
        PipeListener::bind(&options.pipe, PipeSecurity::Creator, Arc::clone(&stop)).map_err(one)?;

    let hub = Hub::new();
    let hosts = Hosts::default();
    if frozen {
        hosts.refuse(&port_of_line(frozen::BROKEN_LINE));
    }
    // The frozen script has a clock of its own, which it moves and then
    // stops; the others read the machine's.
    let frozen_clock = Arc::new(AtomicU64::new(frozen::START_MS));
    let clock: Clock = if frozen {
        let clock = Arc::clone(&frozen_clock);
        Arc::new(move || clock.load(Ordering::Relaxed))
    } else {
        ctlsrv::system_clock()
    };
    let started_ms = clock();

    // Over SIP the lines run the engines their configuration names, on
    // the media threads; otherwise they run the demo engine, which shows
    // the rates and protocols of later phases, on the pool's clock.
    let sip_on = far_end.is_some();
    let mut callers = None;
    let (router, media, engines): (Box<dyn CallRouter>, Box<dyn MediaRunner>, EngineFactory) =
        match far_end {
            Some((transport, far_end)) => {
                let service = transport.local();
                let (router, media) =
                    sip::start(&config, transport, &directory, &hub, &clock).map_err(one)?;
                callers = Some((far_end, service));
                (router, media, Box::new(pool::engine_for))
            }
            None => {
                let negotiation = Arc::new(Mutex::new(Negotiation::default()));
                (
                    Box::new(NullSwitch::new()),
                    Box::new(InlineRunner::new()),
                    Box::new(move |_| Box::new(DemoEngine::new(Arc::clone(&negotiation)))),
                )
            }
        };
    let mut pool = Pool::new(PoolParts {
        router,
        media,
        serial: hosts.factory(),
        engines,
        sink: Box::new(Sink(hub.clone())),
        profiles: Box::new(MemoryProfiles::new()),
        call_log: None,
    });

    let log = {
        let hub = hub.clone();
        let clock = Arc::clone(&clock);
        move |source: &str, problem: &Message| {
            let level = match problem.severity {
                Severity::Info => LogLevel::Info,
                Severity::Warning => LogLevel::Warning,
                Severity::Error => LogLevel::Error,
            };
            hub.publish(&ControlEvent::Log(LogEvent {
                time_ms: clock(),
                level,
                source: source.to_string(),
                text: problem.to_string(),
            }));
        }
    };
    log(
        SOURCE,
        &message(
            "RMD0021",
            &[
                &config_path.display().to_string(),
                &config.lines.len().to_string(),
            ],
        ),
    );
    pool.set_now(Now {
        mono_ms: 0,
        unix_ms: started_ms,
    });
    pool.apply_config(&config);

    let ports = demo_ports(options.lines);
    let (handle, inbox) = command_channel();
    let control_log = {
        let hub = hub.clone();
        let clock = Arc::clone(&clock);
        move |level: LogLevel, text: String| {
            // Who connects, and when, is different at every run: left out
            // of the frozen script's log so that it is the same each time.
            if !frozen {
                hub.publish(&ControlEvent::Log(LogEvent {
                    time_ms: clock(),
                    level,
                    source: CONTROL_SOURCE.to_string(),
                    text,
                }));
            }
        }
    };
    let server = Server::new(ServerParts {
        pool: handle.clone(),
        config: file,
        config_path,
        version: crate::VERSION.to_string(),
        pipe: options.pipe.clone(),
        hub,
        log: Arc::new(control_log),
        limits: Limits::default(),
        mode: ServiceMode::Demo,
        ports: Arc::new(ports.clone()),
        clock,
        // The demo service keeps no log file: a new log level or log
        // directory is in effect as soon as it is set.
        apply_service: Arc::new(|_| Ok(true)),
        cpu: ctlsrv::no_cpu_meter(),
        capture_dir: sip_on.then(|| directory.join(sip::CAPTURES)),
    });

    log(SOURCE, &message("RMD0001", &[crate::VERSION]));

    // The lines show what their ports are bound to from the start.
    pool.set_bindings(
        ports
            .iter()
            .filter_map(|port| Some((port.name.clone(), port.binding()?)))
            .collect(),
    );
    let frozen_at = frozen.then(|| frozen::play(&mut pool, &hosts, options.lines, &frozen_clock));

    let spawn_failed = |e: std::io::Error| one(message("RMD0003", &[&e.to_string()]));
    std::thread::Builder::new()
        .name("pool".to_string())
        .spawn(move || run_pool(pool, inbox, frozen_at))
        .map_err(spawn_failed)?;
    if !frozen {
        let active = options.script == Script::Busy;
        std::thread::Builder::new()
            .name("hosts".to_string())
            .spawn(move || run_hosts(Driver::new(hosts, active), handle))
            .map_err(spawn_failed)?;
    }
    if let Some((far_end, service)) = callers {
        std::thread::Builder::new()
            .name("callers".to_string())
            .spawn(move || far_end.run(service))
            .map_err(spawn_failed)?;
    }

    println!(
        "rustmodem-demosvc {}: {} lines, script {}, pipe {}",
        crate::VERSION,
        options.lines,
        options.script.as_str(),
        options.pipe
    );
    loop {
        match listener.accept() {
            Ok(Some(connection)) => {
                let server = server.clone();
                // A client that cannot be given a thread is turned away by
                // its connection closing.
                let _ = std::thread::Builder::new()
                    .name("ctl-client".to_string())
                    .spawn(move || server.handle(connection));
            }
            Ok(None) => return Ok(()),
            Err(error) => {
                return Err(one(message(
                    "RMD2007",
                    &[listener.name(), &error.to_string()],
                )));
            }
        }
    }
}

/// The pool's thread. With `frozen_at` the pool is served at that one
/// time for ever: what the operator asks for is done, and nothing moves on
/// by itself.
fn run_pool(mut pool: Pool, inbox: PoolInbox, frozen_at: Option<Now>) {
    let started = Instant::now();
    while inbox.serve(&mut pool) {
        let now = frozen_at.unwrap_or_else(|| Now {
            mono_ms: started.elapsed().as_millis() as u64,
            unix_ms: unix_ms(),
        });
        pool.service(now);
        std::thread::sleep(POOL_SLEEP.min(Duration::from_millis(TICK_MS)));
    }
}

/// The thread that plays the hosts.
fn run_hosts(mut driver: Driver, pool: PoolHandle) {
    let started = Instant::now();
    // The pool going away ends the demo service.
    while let Ok(lines) = pool.lines() {
        driver.step(started.elapsed().as_millis() as u64, &lines);
        std::thread::sleep(Duration::from_millis(STEP_MS));
    }
}
