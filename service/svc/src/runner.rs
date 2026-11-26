// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Starting and stopping what the service runs: the log, and on the
//! runtime the task that owns the pool, the control server on the named
//! pipe and the refresh of the lines' bindings.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use config::{Backend, ConfigFile, LineConfig};
use ctlproto::{Event as ControlEvent, LogLevel, ServiceMode};
use ctlsrv::{Hub, Limits, Server, ServerParts};
use msg::{Message, message};
use pipe::{PipeListener, PipeSecurity};
use pool::{
    CallRouter, EventSink, InlineRunner, MediaRunner, Now, NullSwitch, Pool, PoolParts,
    SerialFactory,
};
use serial::{Com0comBackend, SerialBackend};
use svc::bridge::BridgedPorts;
use svc::pooltask::{LINGER, PoolClock, PoolTask};
use svc::shutdown::{Shutdown, Stopping};
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::logger::{self, DEFAULT_MAX_BYTES, Logger};
use crate::ports::SystemPorts;
use crate::profiles::ProfileFile;
use crate::{VERSION, cpu, eventlog, telephony};

/// The tag of the service itself in log lines.
const SOURCE: &str = "SVC";

/// The tag of the control server in log lines.
const CONTROL_SOURCE: &str = "CTL";

/// How often the serial ports are looked up again for the lines' bindings.
const BINDING_REFRESH: Duration = Duration::from_secs(30);

/// The name of the call records file in the log directory.
const CALLS_FILE: &str = "calls.jsonl";

/// Opens the serial ports of the lines.
struct Ports;

impl SerialFactory for Ports {
    fn open(&mut self, line: &LineConfig) -> Result<Box<dyn SerialBackend>, Message> {
        match line.serial.backend {
            Backend::Com0com => Ok(Box::new(Com0comBackend::open(&line.serial.port)?)),
        }
    }
}

/// Takes the pool's events: log lines go to the log, which passes on those
/// its level lets through; everything else goes to control clients.
struct Sink {
    logger: Logger,
    hub: Hub,
}

impl EventSink for Sink {
    fn publish(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::Log(line) => self.logger.event(line),
            other => self.hub.publish(&other),
        }
    }
}

/// How the service is being run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunMode {
    /// In the foreground: log lines also go to standard output. Nothing is
    /// written to the Event Log; the console is where its operator looks.
    Console,
    /// Under the service control manager: start, stop and fatal errors
    /// also go to the Windows Event Log.
    Service,
}

impl RunMode {
    /// Writes to the Event Log, in service mode.
    fn report(self, message: &Message) {
        if self == RunMode::Service {
            eventlog::report(message);
        }
    }
}

/// The service, running.
pub struct Running {
    mode: RunMode,
    runtime: Handle,
    pool: JoinHandle<()>,
    control: JoinHandle<()>,
    bindings: JoinHandle<()>,
    logger: Logger,
}

/// Starts the service: its tasks on `runtime`, which `shutdown` ends. The
/// caller waits for that signal and then calls [`Running::stop`].
///
/// A configuration that cannot be used is reported as `RMD` messages to the
/// log, to the Event Log in service mode, and to the caller, who in console
/// mode prints them to standard error.
pub fn start(
    config_path: Option<PathBuf>,
    mode: RunMode,
    runtime: &Handle,
    shutdown: &Shutdown,
) -> Result<Running, Vec<Message>> {
    start_inner(config_path, mode, runtime, shutdown).inspect_err(|problems| {
        // The log directory comes from the configuration, which may be
        // what is wrong; the default one is the best place left.
        let fallback = Logger::open(
            &config::default_log_dir(),
            LogLevel::Info,
            false,
            None,
            DEFAULT_MAX_BYTES,
        );
        for problem in problems {
            mode.report(problem);
            if let Ok(log) = &fallback {
                log.message(SOURCE, problem);
            }
        }
        if let Ok(log) = &fallback {
            log.flush();
        }
    })
}

fn start_inner(
    config_path: Option<PathBuf>,
    mode: RunMode,
    runtime: &Handle,
    shutdown: &Shutdown,
) -> Result<Running, Vec<Message>> {
    let console = mode == RunMode::Console;
    // What is made here is the runtime's: the pipe and the SIP socket are
    // registered with its I/O driver, and their tasks run on it.
    let _on_runtime = runtime.enter();
    let config_path = config_path.unwrap_or_else(config::default_path);
    let file = ConfigFile::load(&config_path)?;
    let config = file.config().clone();

    let (log_dir, level) = log_settings(&config.service);
    let hub = Hub::new();
    let logger = Logger::open(
        &log_dir,
        level,
        console,
        Some(hub.clone()),
        DEFAULT_MAX_BYTES,
    )
    .map_err(|problem| vec![problem])?;
    // From here on a failure can be logged where the configuration says.
    let fail = |problem: Message| {
        logger.message(SOURCE, &problem);
        logger.flush();
        vec![problem]
    };

    let call_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join(CALLS_FILE))
        .map_err(|e| {
            fail(message(
                "RMD0022",
                &[&log_dir.display().to_string(), &e.to_string()],
            ))
        })?;

    // What is wrong with the stored profiles does not stop the service:
    // the lines concerned start from their factory settings.
    let (profiles, profile_problems) = ProfileFile::open(&ProfileFile::path_beside(&config_path));
    for problem in &profile_problems {
        logger.message(SOURCE, problem);
    }

    let listener =
        PipeListener::bind(&config.control.pipe, PipeSecurity::AdministratorsOnly).map_err(fail)?;

    // What wakes the pool's task between its ticks: a serial port's
    // thread with something from its host, or a SIP message that has
    // arrived. A media thread with something to report has a channel of
    // its own.
    let wake = Arc::new(Notify::new());
    let (media_waker, media_events) = svc::mediawake::channel();
    let mut sip_failed = None;
    let (router, media): (Box<dyn CallRouter>, Box<dyn MediaRunner>) = if config.sip.enabled {
        match telephony::start(&config, &log_dir, &logger, &hub, &wake, media_waker) {
            Ok(parts) => parts,
            Err(problem) => {
                // The modems are still of use to whoever dials between
                // them: the service runs, without SIP, and says so.
                logger.message(SOURCE, &problem);
                mode.report(&problem);
                sip_failed = Some(config.sip.listen.to_string());
                (Box::new(NullSwitch::new()), Box::new(InlineRunner::new()))
            }
        }
    } else {
        (Box::new(NullSwitch::new()), Box::new(InlineRunner::new()))
    };

    let mut pool = Pool::new(PoolParts {
        router,
        media,
        // Each port has a thread, which the pool does not see.
        serial: Box::new(BridgedPorts::new(Box::new(Ports), Arc::clone(&wake))),
        engines: Box::new(pool::engine_for),
        sink: Box::new(Sink {
            logger: logger.clone(),
            hub: hub.clone(),
        }),
        profiles: Box::new(profiles),
        call_log: Some(Box::new(call_log)),
    });
    logger.message(
        SOURCE,
        &message(
            "RMD0021",
            &[
                &config_path.display().to_string(),
                &config.lines.len().to_string(),
            ],
        ),
    );
    // The lines are added before the pool has a task and a clock of its
    // own; they are logged with the time now.
    pool.set_now(Now {
        mono_ms: 0,
        unix_ms: logger::unix_ms(),
    });
    if let Some(listen) = sip_failed {
        pool.set_sip_failed(listen);
    }
    pool.apply_config(&config);

    let (handle, requests) = ctlsrv::pool_link();
    let control_log = logger.clone();
    let server = Server::new(ServerParts {
        pool: handle,
        config: file,
        config_path,
        version: VERSION.to_string(),
        pipe: config.control.pipe.clone(),
        hub,
        log: Arc::new(move |level, text| control_log.log(level, CONTROL_SOURCE, &text)),
        limits: Limits::default(),
        mode: match mode {
            RunMode::Console => ServiceMode::Console,
            RunMode::Service => ServiceMode::Service,
        },
        ports: Arc::new(SystemPorts),
        clock: ctlsrv::system_clock(),
        // A new log level or log directory is in effect as soon as it is
        // set. The call records stay in the directory the service
        // started with until it starts again.
        apply_service: {
            let logger = logger.clone();
            Arc::new(move |service| {
                let (directory, level) = log_settings(service);
                logger.reconfigure(&directory, level).map(|()| true)
            })
        },
        cpu: cpu::meter(),
        capture_dir: config
            .sip
            .enabled
            .then(|| telephony::capture_dir(&config, &log_dir)),
    });
    let binding_server = server.clone();

    let pool_task = PoolTask {
        pool,
        requests,
        wake,
        media_events: Some(media_events),
        clock: PoolClock::Running(Arc::new(logger::unix_ms)),
        linger: LINGER,
    };
    let pool = runtime.spawn(pool_task.run(shutdown.subscribe()));
    let control = runtime.spawn(accept_clients(
        server,
        listener,
        shutdown.subscribe(),
        logger.clone(),
    ));
    let bindings = runtime.spawn(refresh_bindings(binding_server, shutdown.subscribe()));

    let started = message("RMD0001", &[VERSION]);
    logger.message(SOURCE, &started);
    mode.report(&started);
    Ok(Running {
        mode,
        runtime: runtime.clone(),
        pool,
        control,
        bindings,
        logger,
    })
}

impl Running {
    /// Stops the service cleanly: every call is hung up, every port is
    /// released, every registration is given up, and the log is flushed.
    /// The pool does this well inside the service stop timeout. The
    /// shutdown signal has been sent by now, and the tasks are ending.
    /// This waits, so it is not called on one of the runtime's threads.
    pub fn stop(self) {
        let (pool, control, bindings) = (self.pool, self.control, self.bindings);
        self.runtime.block_on(async {
            let _ = pool.await;
            let _ = control.await;
            let _ = bindings.await;
        });
        let stopped = message("RMD0002", &[]);
        self.logger.message(SOURCE, &stopped);
        self.logger.flush();
        self.mode.report(&stopped);
    }
}

/// Looks the serial ports up when the service starts and every
/// [`BINDING_REFRESH`] after, and tells the pool what each is bound to.
/// The server does the same itself when the configuration is reloaded.
/// The lines' bindings are a nicety: the service runs without them.
async fn refresh_bindings(server: Server, mut stopping: Stopping) {
    let mut refresh = tokio::time::interval(BINDING_REFRESH);
    loop {
        tokio::select! {
            _ = refresh.tick() => {
                // The only failure is the pool having stopped, which the
                // stop signal says too.
                let _ = server.refresh_bindings().await;
            }
            () = stopping.wait() => break,
        }
    }
}

/// The control server's accepting task: each client is served on tasks of
/// its own.
async fn accept_clients(
    server: Server,
    mut listener: PipeListener,
    mut stopping: Stopping,
    logger: Logger,
) {
    loop {
        match server.serve(&mut listener, stopping.receiver()).await {
            Ok(()) => break,
            Err(error) => {
                logger.message(
                    CONTROL_SOURCE,
                    &message("RMD2007", &[listener.name(), &error.to_string()]),
                );
                // Do not spin on an error that persists.
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    () = stopping.wait() => break,
                }
            }
        }
    }
}

/// Where the log goes and how much of it there is, as `[service]` says.
fn log_settings(service: &config::ServiceConfig) -> (PathBuf, LogLevel) {
    let directory = if service.log_dir.is_empty() {
        config::default_log_dir()
    } else {
        PathBuf::from(&service.log_dir)
    };
    let level = match service.log_level {
        config::LogLevel::Error => LogLevel::Error,
        config::LogLevel::Warning => LogLevel::Warning,
        config::LogLevel::Info => LogLevel::Info,
        config::LogLevel::Debug => LogLevel::Debug,
    };
    (directory, level)
}
