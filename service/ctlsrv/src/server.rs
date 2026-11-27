// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The server: the hello exchange, requests and their answers.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use config::{
    Backend, Config, ConfigFile, ControlConfig, EngineKind, HuntGroup, HuntGroupEdit, LineConfig,
    LineEdit, LineModem, LineModemEdit, MediaEdit, ModemEdit, ModulationName, RegisterConfig,
    Route, RouteEdit, ServiceConfig, SettingsEdit, SipEdit, Strategy, Target, UnknownCommand,
};
use ctlproto::{
    Call, CallList, ClientFrame, ControlSettings, Done, EVENT_NAMES, EventSelection, Hello,
    HuntGroupInfo, HuntGroupList, LineChange, LineList, LineSpec, LogLevel, LogTail,
    MAX_MESSAGE_BYTES, MediaSettings, ModemSettings, PROTOCOL, PortInfo, PortList, Reloaded,
    RouteInfo, RouteList, SelfTestResult, ServerFrame, ServiceInfo, ServiceMode, ServiceSettings,
    Settings, SettingsChange, SettingsResult, SipSettings, Subscription, decode_client, encode,
};
use msg::{Message, Severity, message};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::sync::{Mutex, MutexGuard, watch};

use crate::link::PoolLink;
use crate::queue::{ClientQueue, Hub, Limits};
use crate::{Connection, Halves, Listener, PortInventory};

/// Tells the server's tasks that the service is stopping: the value goes
/// to `true`, once. A sender that goes away without saying so never stops
/// them.
pub type Stop = watch::Receiver<bool>;

/// A [`Stop`] that never says to stop, for a server that runs until its
/// process ends.
pub fn never_stops() -> Stop {
    watch::channel(false).1
}

/// Waits until the service is stopping.
async fn stopped(stop: &mut Stop) {
    if stop.wait_for(|stopping| *stopping).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// How long a client's writing task is given, when the service stops, to
/// write what is queued for it.
const STOP_GRACE: Duration = Duration::from_secs(1);

/// Tells the server the time, in milliseconds since the Unix epoch. The
/// service gives [`system_clock`]; the demo service, for screenshots, gives
/// one that stands still.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The clock of the machine.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    })
}

/// Takes the server's own log lines. The source is always `CTL`.
pub type LogFn = Arc<dyn Fn(LogLevel, String) + Send + Sync>;

/// Puts changed `[service]` settings into effect while the service runs.
/// It answers `true` when they are in effect, and `false` when this
/// service can only take them up when it next starts. An error is a
/// setting that could not be used, such as a log directory that cannot be
/// made: nothing has changed then, and the change is not kept.
pub type ApplyService = Arc<dyn Fn(&ServiceConfig) -> Result<bool, Message> + Send + Sync>;

/// For a service that takes up its `[service]` settings only when it
/// starts.
pub fn restart_to_apply() -> ApplyService {
    Arc::new(|_| Ok(false))
}

/// Tells how much of one processor the service process has used over the
/// last second or so, in a hundred, for `stats.get`.
pub type CpuMeter = Arc<dyn Fn() -> f64 + Send + Sync>;

/// For a service that does not measure its processor time.
pub fn no_cpu_meter() -> CpuMeter {
    Arc::new(|| 0.0)
}

/// The settings of `[sip]` and `[media]` that are taken up only when the
/// service starts: whether there is SIP at all, where its listener is,
/// the ports of RTP and where captures go.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AtStart {
    enabled: bool,
    listen: std::net::SocketAddrV4,
    rtp_ports: config::PortRange,
    capture_dir: String,
}

impl AtStart {
    fn of(config: &Config) -> AtStart {
        AtStart {
            enabled: config.sip.enabled,
            listen: config.sip.listen,
            rtp_ports: config.media.rtp_ports,
            capture_dir: config.media.capture_dir.clone(),
        }
    }
}

/// What a server is made from.
pub struct ServerParts {
    /// The way to the pool.
    pub pool: PoolLink,
    /// The configuration the service started with.
    pub config: ConfigFile,
    /// Where the configuration is read from and written to.
    pub config_path: PathBuf,
    /// The service version, for `hello` and `service.info`.
    pub version: String,
    /// The name of the control pipe, for `service.info`.
    pub pipe: String,
    /// Where events are published.
    pub hub: Hub,
    /// Takes the server's own log lines.
    pub log: LogFn,
    /// How much may wait for one client.
    pub limits: Limits,
    /// How the service is being run, for `service.info`.
    pub mode: ServiceMode,
    /// Knows the serial ports of the machine, for `ports.list` and the
    /// lines' bindings.
    pub ports: Arc<dyn PortInventory>,
    /// Tells the time, for `service.info`.
    pub clock: Clock,
    /// Puts changed `[service]` settings into effect, if the service can
    /// without starting again.
    pub apply_service: ApplyService,
    /// Measures the processor time of the service, for `stats.get`.
    pub cpu: CpuMeter,
    /// Where call captures are written, for `service.info`; `None` for a
    /// service that captures nothing.
    pub capture_dir: Option<PathBuf>,
}

struct Shared {
    pool: PoolLink,
    config: Mutex<ConfigFile>,
    config_path: PathBuf,
    version: String,
    pipe: String,
    hub: Hub,
    log: LogFn,
    limits: Limits,
    mode: ServiceMode,
    ports: Arc<dyn PortInventory>,
    /// The `[service]` and `[control]` settings that are in effect: the
    /// ones the service started with, and the changes it could apply
    /// since. A change that is in the file and not here needs a restart.
    running: Mutex<(ServiceConfig, ControlConfig)>,
    /// The `[sip]` and `[media]` settings the service started with that
    /// it cannot change while it runs.
    at_start: AtStart,
    apply_service: ApplyService,
    cpu: CpuMeter,
    capture_dir: Option<PathBuf>,
    clock: Clock,
    started_ms: u64,
    config_loaded_ms: AtomicU64,
    next_client: AtomicU64,
}

/// The control API server.
#[derive(Clone)]
pub struct Server {
    shared: Arc<Shared>,
}

/// One line read from a client.
enum Line {
    /// A line of text.
    Text(String),
    /// A line that is not UTF-8.
    NotText,
    /// A line longer than the limit. What follows cannot be trusted to
    /// start at a message boundary.
    TooLong,
}

impl Server {
    /// A server. Nothing is served until a transport is given to
    /// [`Server::serve`] or a connection to [`Server::handle`].
    pub fn new(parts: ServerParts) -> Server {
        let started_ms = (parts.clock)();
        let running = (
            parts.config.config().service.clone(),
            parts.config.config().control.clone(),
        );
        Server {
            shared: Arc::new(Shared {
                pool: parts.pool,
                mode: parts.mode,
                ports: parts.ports,
                running: Mutex::new(running),
                at_start: AtStart::of(parts.config.config()),
                apply_service: parts.apply_service,
                cpu: parts.cpu,
                capture_dir: parts.capture_dir,
                config_loaded_ms: AtomicU64::new(started_ms),
                config: Mutex::new(parts.config),
                config_path: parts.config_path,
                version: parts.version,
                pipe: parts.pipe,
                hub: parts.hub,
                log: parts.log,
                limits: parts.limits,
                clock: parts.clock,
                started_ms,
                next_client: AtomicU64::new(1),
            }),
        }
    }

    /// Where events are published.
    pub fn hub(&self) -> Hub {
        self.shared.hub.clone()
    }

    /// Looks the serial ports up again and tells the pool what the host
    /// side of each is bound to, so that the lines show it. The service
    /// calls this when it starts and every so often after; the server calls
    /// it itself when the configuration is reloaded.
    pub async fn refresh_bindings(&self) -> Result<(), Message> {
        let bindings = self
            .inventory()
            .await
            .iter()
            .filter_map(|port| Some((port.name.clone(), port.binding()?)))
            .collect();
        self.shared.pool.set_bindings(bindings).await
    }

    /// The serial ports of the machine. Looking them up may wait on the
    /// system, so it is done on a thread for blocking work.
    async fn inventory(&self) -> Vec<PortInfo> {
        let ports = Arc::clone(&self.shared.ports);
        match tokio::task::spawn_blocking(move || ports.ports()).await {
            Ok(ports) => ports,
            Err(failed) => match failed.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                // The runtime is shutting down.
                Err(_) => Vec::new(),
            },
        }
    }

    /// Accepts clients until the listener shuts down or the service
    /// stops, serving each on tasks of its own. An error is the
    /// listener's: the clients already connected are served on, and the
    /// caller may call this again.
    pub async fn serve<L: Listener>(&self, listener: &mut L, mut stop: Stop) -> io::Result<()> {
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted?,
                () = stopped(&mut stop) => return Ok(()),
            };
            let Some(connection) = accepted else {
                return Ok(());
            };
            let server = self.clone();
            let stop = stop.clone();
            tokio::spawn(async move { server.handle(connection, stop).await });
        }
    }

    /// Serves one client until it goes away or the service stops.
    pub async fn handle<C: Connection>(&self, connection: C, mut stop: Stop) {
        let number = self.shared.next_client.fetch_add(1, Ordering::Relaxed);
        let client = format!("{number} ({})", connection.peer());
        let Halves { reader, mut writer } = connection.split();
        let queue = Arc::new(ClientQueue::new(self.shared.limits));

        // One task writes what is queued, so that a client that is slow
        // to read holds up nobody but itself.
        let writing = Arc::clone(&queue);
        let mut writer_task = tokio::spawn(async move {
            while let Some(line) = writing.pop().await {
                let written = tokio::select! {
                    written = write_line(&mut writer, &line) => written,
                    // The client fell too far behind while this waited
                    // for it to read.
                    () = writing.closed() => break,
                };
                if written.is_err() {
                    writing.close();
                    break;
                }
            }
        });

        let mut reader = BufReader::new(reader);
        let greeted = tokio::select! {
            greeted = self.greet(&mut reader, &queue) => greeted,
            () = queue.closed() => false,
            () = stopped(&mut stop) => false,
        };
        if greeted {
            self.log(&message("RMD2011", &[&client]));
            self.shared.hub.add(&queue);
            self.converse(&mut reader, &queue, &mut stop).await;
            self.shared.hub.remove(&queue);
        }

        // Let what is queued go out. The connection closes when its two
        // halves are dropped, the writer's with its task and the reader's
        // here; it is not closed by force, which on some transports would
        // discard what the client has not read yet. A service that is
        // stopping does not wait long for a client that does not read.
        queue.finish();
        tokio::select! {
            _ = &mut writer_task => {}
            () = stopped(&mut stop) => {
                if tokio::time::timeout(STOP_GRACE, &mut writer_task).await.is_err() {
                    writer_task.abort();
                    let _ = writer_task.await;
                }
            }
        }
        drop(reader);
        if queue.overflowed() {
            self.log(&message("RMD2006", &[&client]));
        } else if greeted {
            self.log(&message("RMD2012", &[&client]));
        }
    }

    /// The hello exchange. Returns whether the client may go on.
    async fn greet(
        &self,
        reader: &mut (impl AsyncBufRead + Unpin + Send),
        queue: &ClientQueue,
    ) -> bool {
        let refuse = |problem: Message| {
            queue.push_reply(encode(&ServerFrame::Error {
                id: None,
                error: problem.into(),
            }));
            false
        };
        match read_line(reader).await {
            Ok(Some(Line::Text(line))) => match decode_client(&line) {
                Ok(ClientFrame::Hello { hello }) if hello.protocol == PROTOCOL => {
                    queue.push_reply(encode(&ServerFrame::Hello {
                        hello: Hello {
                            protocol: PROTOCOL,
                            version: self.shared.version.clone(),
                        },
                    }));
                    true
                }
                Ok(ClientFrame::Hello { hello }) => refuse(message(
                    "RMD2001",
                    &[&hello.protocol.to_string(), &PROTOCOL.to_string()],
                )),
                _ => refuse(message("RMD2005", &[])),
            },
            Ok(Some(Line::NotText)) => refuse(message("RMD2005", &[])),
            Ok(Some(Line::TooLong)) => {
                refuse(message("RMD2014", &[&MAX_MESSAGE_BYTES.to_string()]))
            }
            Ok(None) | Err(_) => false,
        }
    }

    /// Requests and their answers, until the client goes away, is cut
    /// off, or the service stops. Only the wait for a request is given
    /// up: a request that has been read is carried out whole.
    async fn converse(
        &self,
        reader: &mut (impl AsyncBufRead + Unpin + Send),
        queue: &Arc<ClientQueue>,
        stop: &mut Stop,
    ) {
        loop {
            let read = tokio::select! {
                read = read_line(reader) => read,
                () = queue.closed() => return,
                () = stopped(stop) => return,
            };
            let line = match read {
                Ok(Some(Line::Text(line))) => line,
                Ok(Some(Line::NotText)) => {
                    let problem = message("RMD2004", &["the message is not UTF-8"]);
                    queue.push_reply(encode(&ServerFrame::Error {
                        id: None,
                        error: problem.into(),
                    }));
                    continue;
                }
                Ok(Some(Line::TooLong)) => {
                    let problem = message("RMD2014", &[&MAX_MESSAGE_BYTES.to_string()]);
                    queue.push_reply(encode(&ServerFrame::Error {
                        id: None,
                        error: problem.into(),
                    }));
                    return;
                }
                Ok(None) | Err(_) => return,
            };
            queue.push_reply(encode(&self.answer(&line, queue).await));
        }
    }

    /// The answer to one line from the client.
    async fn answer(&self, line: &str, queue: &ClientQueue) -> ServerFrame {
        let frame = match decode_client(line) {
            Ok(ClientFrame::Request(frame)) => frame,
            Ok(ClientFrame::Hello { .. }) => {
                return ServerFrame::Error {
                    id: None,
                    error: message("RMD2004", &["hello was already exchanged"]).into(),
                };
            }
            Err(problem) => {
                // If the message is JSON with an id, say which request the
                // error is for.
                let id = serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|value| value.get("id").and_then(Value::as_u64));
                return ServerFrame::Error {
                    id,
                    error: problem.into(),
                };
            }
        };
        let id = frame.id;
        let result = match Call::from_parts(&frame.method, frame.params) {
            Ok(call) => self.dispatch(call, queue).await,
            Err(problem) => Err(problem),
        };
        match result {
            Ok(result) => ServerFrame::Result { id, result },
            Err(problem) => ServerFrame::Error {
                id: Some(id),
                error: problem.into(),
            },
        }
    }

    async fn dispatch(&self, call: Call, queue: &ClientQueue) -> Result<Value, Message> {
        let shared = &self.shared;
        match call {
            Call::ServiceInfo => {
                let lines = shared.pool.lines().await?.len() as u32;
                value(&ServiceInfo {
                    version: shared.version.clone(),
                    protocol: PROTOCOL,
                    pipe: shared.pipe.clone(),
                    config_path: shared.config_path.display().to_string(),
                    lines,
                    started_ms: shared.started_ms,
                    uptime_ms: (shared.clock)().saturating_sub(shared.started_ms),
                    config_loaded_ms: shared.config_loaded_ms.load(Ordering::Relaxed),
                    mode: shared.mode,
                    capture_dir: shared
                        .capture_dir
                        .as_ref()
                        .map(|directory| directory.display().to_string()),
                })
            }
            Call::LinesList => value(&LineList {
                lines: shared.pool.lines().await?,
            }),
            Call::LinesGet(line) => value(&shared.pool.line(line.id).await?),
            Call::LinesAnswer(line) => {
                shared.pool.answer(line.id).await?;
                value(&shared.pool.line(line.id).await?)
            }
            Call::LinesHangup(line) => {
                shared.pool.hangup(line.id).await?;
                value(&shared.pool.line(line.id).await?)
            }
            Call::LinesReset(line) => {
                shared.pool.reset(line.id).await?;
                value(&shared.pool.line(line.id).await?)
            }
            Call::LinesEnable(line) => {
                let edit = LineEdit {
                    enabled: Some(true),
                    ..LineEdit::default()
                };
                self.change_config(|file| file.edit_line(line.id, &edit))
                    .await?;
                value(&shared.pool.line(line.id).await?)
            }
            Call::LinesDisable(line) => {
                let edit = LineEdit {
                    enabled: Some(false),
                    ..LineEdit::default()
                };
                self.change_config(|file| file.edit_line(line.id, &edit))
                    .await?;
                value(&shared.pool.line(line.id).await?)
            }
            Call::LinesAdd(spec) => {
                let line = new_line(&spec)?;
                self.change_config(|file| file.add_line(&line)).await?;
                value(&shared.pool.line(spec.id).await?)
            }
            Call::LinesEdit(change) => {
                self.change_config(|file| {
                    // The registration is changed from what the line has.
                    let edit = line_edit(&change, file.config().line(change.id))
                        .map_err(|problem| vec![problem])?;
                    file.edit_line(change.id, &edit)
                })
                .await?;
                value(&shared.pool.line(change.id).await?)
            }
            Call::LinesCapture(switch) => value(&shared.pool.capture(switch.id, switch.on).await?),
            Call::LinesDiag(line) => {
                let id = line.id;
                value(&shared.pool.with(move |pool| pool.line_diag(id)).await??)
            }
            Call::SelfTestRun(test) => value(&self.self_test(test.a, test.b).await?),
            Call::SipStatus => value(&shared.pool.sip_status().await?),
            Call::SipReregister(query) => {
                shared.pool.reregister(query.id).await?;
                value(&Done {})
            }
            Call::HuntGroupsList => value(&HuntGroupList {
                groups: self
                    .config()
                    .await
                    .config()
                    .hunt_groups
                    .iter()
                    .map(hunt_group_info)
                    .collect(),
            }),
            Call::HuntGroupsAdd(spec) => {
                let group = HuntGroup {
                    strategy: match &spec.strategy {
                        Some(name) => Strategy::parse(name).ok_or_else(|| {
                            message(
                                "RMD2003",
                                &[
                                    "huntgroups.add",
                                    "strategy is not one of: first-idle, round-robin, least-recent",
                                ],
                            )
                        })?,
                        None => Strategy::default(),
                    },
                    name: spec.name,
                    pilot: spec.pilot,
                    lines: spec.lines,
                };
                self.change_config(|file| file.add_hunt_group(&group))
                    .await?;
                value(&hunt_group_info(&group))
            }
            Call::HuntGroupsEdit(change) => {
                let edit = HuntGroupEdit {
                    name: change.new_name.clone(),
                    pilot: change.pilot,
                    lines: change.lines,
                    strategy: change.strategy,
                };
                self.change_config(|file| file.edit_hunt_group(&change.name, &edit))
                    .await?;
                let name = change.new_name.unwrap_or(change.name);
                let config = self.config().await;
                let group = config
                    .config()
                    .hunt_group(&name)
                    .ok_or_else(|| message("RMD0032", &[&name]))?;
                value(&hunt_group_info(group))
            }
            Call::HuntGroupsRemove(group) => {
                self.change_config(|file| file.remove_hunt_group(&group.name))
                    .await?;
                value(&Done {})
            }
            Call::RoutesList => value(&self.routes().await),
            Call::RoutesAdd(spec) => {
                let route = Route {
                    target: Target::parse(&spec.target).ok_or_else(|| {
                        message(
                            "RMD2003",
                            &["routes.add", "target is not line:<id> or group:<name>"],
                        )
                    })?,
                    called: spec.called,
                };
                let index = spec.index.map(|index| index as usize);
                self.change_config(|file| file.add_route(&route, index))
                    .await?;
                value(&self.routes().await)
            }
            Call::RoutesEdit(change) => {
                let edit = RouteEdit {
                    called: change.called,
                    target: change.target,
                };
                self.change_config(|file| file.edit_route(change.index as usize, &edit))
                    .await?;
                value(&self.routes().await)
            }
            Call::RoutesRemove(route) => {
                self.change_config(|file| file.remove_route(route.index as usize))
                    .await?;
                value(&self.routes().await)
            }
            Call::RoutesMove(moved) => {
                self.change_config(|file| file.move_route(moved.index as usize, moved.to as usize))
                    .await?;
                value(&self.routes().await)
            }
            Call::LinesRemove(line) => {
                self.change_config(|file| file.remove_line(line.id)).await?;
                value(&Done {})
            }
            Call::CallsList(query) => value(&CallList {
                calls: shared
                    .pool
                    .calls(query.last.map(|last| last as usize))
                    .await?,
            }),
            Call::ConfigReload => {
                let mut current = self.config().await;
                let loaded = ConfigFile::load(&shared.config_path).map_err(first)?;
                shared.pool.apply_config(loaded.config().clone()).await?;
                let lines = loaded.config().lines.len() as u32;
                *current = loaded;
                drop(current);
                shared
                    .config_loaded_ms
                    .store((shared.clock)(), Ordering::Relaxed);
                self.refresh_bindings().await?;
                value(&Reloaded { lines })
            }
            Call::ConfigGet => value(&self.settings(self.config().await.config())),
            Call::ConfigSet(change) => value(&self.change_settings(&change).await?),
            Call::StatsGet => {
                let mut stats = shared.pool.stats().await?;
                stats.cpu_percent = (shared.cpu)();
                value(&stats)
            }
            Call::LogTail(query) => value(&LogTail {
                lines: shared.hub.log_tail(query.last.map(|last| last as usize)),
            }),
            Call::PortsList => value(&PortList {
                ports: self.ports().await?,
            }),
            Call::Subscribe(selection) => {
                let names = event_names("subscribe", &selection)?;
                // The diagnostics are one line's: which, has to be said,
                // and the line has to be there.
                let diagnostics = names
                    .as_deref()
                    .is_some_and(|names| names.contains(&"line.diag"));
                match (diagnostics, selection.line) {
                    (true, Some(id)) => {
                        shared.pool.line(id).await?;
                    }
                    (true, None) => {
                        return Err(message(
                            "RMD2003",
                            &["subscribe", "line.diag needs the line it is for"],
                        ));
                    }
                    (false, Some(_)) => {
                        return Err(message(
                            "RMD2003",
                            &["subscribe", "line is for line.diag, which is not asked for"],
                        ));
                    }
                    (false, None) => {}
                }
                let events = queue.subscribe(names.as_deref());
                if diagnostics {
                    queue.watch_line(selection.line);
                    self.watch_diagnostics().await?;
                }
                value(&Subscription {
                    events,
                    line: queue.watched_line(),
                })
            }
            Call::Unsubscribe(selection) => {
                let names = event_names("unsubscribe", &selection)?;
                let events = queue.unsubscribe(names.as_deref());
                if !events.iter().any(|name| name == "line.diag") && queue.watched_line().is_some()
                {
                    queue.watch_line(None);
                    self.watch_diagnostics().await?;
                }
                value(&Subscription {
                    events,
                    line: queue.watched_line(),
                })
            }
        }
    }

    async fn config(&self) -> MutexGuard<'_, ConfigFile> {
        self.shared.config.lock().await
    }

    /// Changes the configuration: the change is made to a copy, checked,
    /// written to the file and given to the pool, and only then kept. If
    /// any step fails, nothing has changed.
    async fn change_config(
        &self,
        change: impl FnOnce(&mut ConfigFile) -> Result<(), Vec<Message>>,
    ) -> Result<(), Message> {
        let mut current = self.config().await;
        let mut next = current.clone();
        change(&mut next).map_err(first)?;
        next.save(&self.shared.config_path)?;
        self.shared.pool.apply_config(next.config().clone()).await?;
        *current = next;
        Ok(())
    }

    /// The settings of a configuration that are not lines.
    fn settings(&self, config: &Config) -> Settings {
        Settings {
            service: ServiceSettings {
                log_level: match config.service.log_level {
                    config::LogLevel::Error => LogLevel::Error,
                    config::LogLevel::Warning => LogLevel::Warning,
                    config::LogLevel::Info => LogLevel::Info,
                    config::LogLevel::Debug => LogLevel::Debug,
                },
                log_dir: config.service.log_dir.clone(),
            },
            control: ControlSettings {
                pipe: config.control.pipe.clone(),
            },
            sip: SipSettings {
                enabled: config.sip.enabled,
                listen: config.sip.listen.to_string(),
                address: config.sip.address.clone(),
                user_agent: config.sip.user_agent.clone(),
                trusted_peers: config
                    .sip
                    .trusted_peers
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                outbound_peer: config.sip.outbound_peer.clone(),
                ring_timeout_s: config.sip.ring_timeout_s,
            },
            media: MediaSettings {
                codecs: config
                    .media
                    .codecs
                    .iter()
                    .map(|codec| codec.as_str().to_string())
                    .collect(),
                ptime_ms: config.media.ptime_ms,
                rtp_ports: config.media.rtp_ports.to_string(),
                rtp_timeout_s: config.media.rtp_timeout_s,
                capture_dir: config.media.capture_dir.clone(),
            },
            modem: ModemSettings {
                modulations: config
                    .modem
                    .modulations
                    .iter()
                    .map(|name| name.as_str().to_string())
                    .collect(),
                v8: config.modem.v8,
                tx_level_dbm0: i32::from(config.modem.tx_level_dbm0),
                answer_delay_ms: config.modem.answer_delay_ms,
                fallback_ms: config.modem.fallback_ms,
            },
            path: self.shared.config_path.display().to_string(),
        }
    }

    /// The routes of the configuration, in the order they are tried.
    async fn routes(&self) -> RouteList {
        RouteList {
            routes: self
                .config()
                .await
                .config()
                .routes
                .iter()
                .enumerate()
                .map(|(index, route)| RouteInfo {
                    index: index as u32,
                    called: route.called.clone(),
                    target: route.target.to_string(),
                })
                .collect(),
        }
    }

    /// `config.set`: the change is made to a copy, checked, put into
    /// effect where the service can do that while it runs, written to the
    /// file, and only then kept. The lines are not touched. What could
    /// not be put into effect waits for the service to start again, and
    /// the result says so.
    async fn change_settings(&self, change: &SettingsChange) -> Result<SettingsResult, Message> {
        let service = change.service.as_ref();
        let edit = SettingsEdit {
            log_level: service
                .and_then(|service| service.log_level)
                .map(|level| match level {
                    LogLevel::Error => config::LogLevel::Error,
                    LogLevel::Warning => config::LogLevel::Warning,
                    LogLevel::Info => config::LogLevel::Info,
                    LogLevel::Debug => config::LogLevel::Debug,
                }),
            log_dir: service.and_then(|service| service.log_dir.clone()),
            pipe: change
                .control
                .as_ref()
                .and_then(|control| control.pipe.clone()),
            sip: change
                .sip
                .clone()
                .map_or_else(SipEdit::default, |sip| SipEdit {
                    enabled: sip.enabled,
                    listen: sip.listen,
                    address: sip.address,
                    user_agent: sip.user_agent,
                    trusted_peers: sip.trusted_peers,
                    outbound_peer: sip.outbound_peer,
                    ring_timeout_s: sip.ring_timeout_s,
                }),
            media: change
                .media
                .clone()
                .map_or_else(MediaEdit::default, |media| MediaEdit {
                    codecs: media.codecs,
                    ptime_ms: media.ptime_ms,
                    rtp_ports: media.rtp_ports,
                    rtp_timeout_s: media.rtp_timeout_s,
                    capture_dir: media.capture_dir,
                }),
            modem: change
                .modem
                .clone()
                .map_or_else(ModemEdit::default, |modem| ModemEdit {
                    modulations: modem.modulations,
                    v8: modem.v8,
                    tx_level_dbm0: modem.tx_level_dbm0.map(i64::from),
                    answer_delay_ms: modem.answer_delay_ms,
                    fallback_ms: modem.fallback_ms,
                    ..ModemEdit::default()
                }),
        };
        // The pool takes the modem settings as it takes the telephony's:
        // for the calls that follow.
        let telephony = !edit.sip.is_empty() || !edit.media.is_empty() || !edit.modem.is_empty();
        let mut current = self.config().await;
        let mut next = current.clone();
        next.edit_settings(&edit).map_err(first)?;
        let mut running = self.shared.running.lock().await;
        // The settings of the service itself go into effect now, if this
        // service can do that; the control pipe is made when it starts.
        let wanted = next.config().service.clone();
        let applied = wanted != running.0 && (self.shared.apply_service)(&wanted)?;
        if let Err(problem) = next.save(&self.shared.config_path) {
            if applied {
                // What is in effect is what the file says, or it would
                // change at the next start with nobody having asked.
                let _ = (self.shared.apply_service)(&running.0);
            }
            return Err(problem);
        }
        if applied {
            running.0 = wanted;
        }
        *current = next;
        let config = current.config();
        if telephony {
            // The router takes what it can take while it runs: the peers,
            // the timeouts, the codecs and the packet time, for the calls
            // that follow.
            self.shared.pool.apply_config(config.clone()).await?;
        }
        Ok(SettingsResult {
            restart_required: config.service != running.0
                || config.control != running.1
                || AtStart::of(config) != self.shared.at_start,
            config: self.settings(config),
        })
    }

    /// Tells the pool which lines' diagnostics the clients there are
    /// have asked for.
    async fn watch_diagnostics(&self) -> Result<(), Message> {
        let lines = self.hub().watched_lines();
        self.shared
            .pool
            .with(move |pool| pool.watch_diagnostics(&lines))
            .await
    }

    /// `selftest.run`: the two lines are held busy, their modems are run
    /// against each other on a thread of their own, and the lines are let
    /// go.
    async fn self_test(&self, a: u16, b: u16) -> Result<SelfTestResult, Message> {
        let pool = &self.shared.pool;
        let job = pool.with(move |pool| pool.selftest_begin(a, b)).await??;
        let began = std::time::Instant::now();
        let ran = tokio::task::spawn_blocking(move || job.run()).await;
        let elapsed_ms = began.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        // The lines are let go whatever became of the test.
        pool.with(move |pool| pool.selftest_end(a, b)).await?;
        let mut result = ran.map_err(|_| message("RMD2010", &[]))?;
        result.elapsed_ms = elapsed_ms;
        if let Some(error) = &result.error {
            (self.shared.log)(LogLevel::Error, error.to_string());
        }
        Ok(result)
    }

    /// `ports.list`: the ports of the machine in name order, each with the
    /// line that uses it.
    async fn ports(&self) -> Result<Vec<PortInfo>, Message> {
        let lines = self.shared.pool.lines().await?;
        let mut ports = self.inventory().await;
        for port in &mut ports {
            // Windows port names do not distinguish case.
            port.line = lines
                .iter()
                .find(|line| line.port.eq_ignore_ascii_case(&port.name))
                .map(|line| line.id);
        }
        ports.sort_by_cached_key(|port| name_order(&port.name));
        Ok(ports)
    }

    fn log(&self, message: &Message) {
        let level = match message.severity {
            Severity::Info => LogLevel::Info,
            Severity::Warning => LogLevel::Warning,
            Severity::Error => LogLevel::Error,
        };
        (self.shared.log)(level, message.to_string());
    }
}

/// The key port names are ordered by: the letters, then the number after
/// them as a number, so that `COM9` comes before `COM10`.
fn name_order(name: &str) -> (String, u64, String) {
    let folded = name.to_ascii_uppercase();
    let digits = folded.bytes().rev().take_while(u8::is_ascii_digit).count();
    let (letters, number) = folded.split_at(folded.len() - digits);
    (
        letters.to_string(),
        number.parse().unwrap_or(u64::MAX),
        folded.clone(),
    )
}

/// The first of the problems a configuration change found. The API answers
/// a request with one error.
fn first(mut problems: Vec<Message>) -> Message {
    if problems.is_empty() {
        // Validation that fails always says why.
        message("RMD2003", &["configuration", "the change is not valid"])
    } else {
        problems.swap_remove(0)
    }
}

fn value<T: Serialize>(result: &T) -> Result<Value, Message> {
    serde_json::to_value(result).map_err(|e| message("RMD2013", &[&e.to_string()]))
}

fn backend(method: &str, name: &str) -> Result<Backend, Message> {
    Backend::parse(name)
        .ok_or_else(|| message("RMD2003", &[method, "backend is not one of: com0com"]))
}

fn unknown_command(method: &str, name: &str) -> Result<UnknownCommand, Message> {
    UnknownCommand::parse(name).ok_or_else(|| {
        message(
            "RMD2003",
            &[method, "unknown_command is not one of: ok, error"],
        )
    })
}

fn engine(method: &str, name: &str) -> Result<EngineKind, Message> {
    EngineKind::parse(name).ok_or_else(|| {
        message(
            "RMD2003",
            &[method, "engine is not one of: modem, null, echo"],
        )
    })
}

fn modulation(method: &str, name: &str) -> Result<ModulationName, Message> {
    ModulationName::parse(name).ok_or_else(|| {
        message(
            "RMD2003",
            &[
                method,
                "a modulation is not one of: v22bis, v22, bell212a, v21, bell103",
            ],
        )
    })
}

/// A level for a new line. Its range is checked by the configuration; here
/// it has only to be a number the configuration can hold.
fn tx_level(method: &str, level: i32) -> Result<i8, Message> {
    i8::try_from(level)
        .map_err(|_| message("RMD2003", &[method, "tx_level_dbm0 is not a level in dBm0"]))
}

fn hunt_group_info(group: &HuntGroup) -> HuntGroupInfo {
    HuntGroupInfo {
        name: group.name.clone(),
        pilot: group.pilot.clone(),
        lines: group.lines.clone(),
        strategy: group.strategy.as_str().to_string(),
    }
}

/// The line `lines.add` describes. Ranges are checked by the configuration
/// when the line is added.
fn new_line(spec: &LineSpec) -> Result<LineConfig, Message> {
    let mut line = LineConfig::new(spec.id, &spec.port);
    line.name = spec.name.clone().filter(|name| !name.is_empty());
    if let Some(name) = &spec.backend {
        line.serial.backend = backend("lines.add", name)?;
    }
    if let Some(enabled) = spec.enabled {
        line.enabled = enabled;
    }
    if let Some(rings) = spec.auto_answer_rings {
        line.auto_answer_rings = rings;
    }
    if let Some(name) = &spec.unknown_command {
        line.unknown_command = unknown_command("lines.add", name)?;
    }
    if let Some(name) = &spec.engine {
        line.engine = engine("lines.add", name)?;
    }
    if let Some(modem) = &spec.modem {
        line.modem = LineModem {
            modulations: modem
                .modulations
                .as_ref()
                .map(|names| {
                    names
                        .iter()
                        .map(|name| modulation("lines.add", name))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            v8: modem.v8,
            tx_level_dbm0: modem
                .tx_level_dbm0
                .map(|level| tx_level("lines.add", level))
                .transpose()?,
            ..LineModem::default()
        };
    }
    if let Some(register) = &spec.register {
        line.register = Some(RegisterConfig {
            server: register.server.clone(),
            user: register.user.clone(),
            password: register.password.clone(),
            auth_user: register.auth_user.clone().unwrap_or_default(),
            expires_s: register
                .expires_s
                .unwrap_or(RegisterConfig::DEFAULT_EXPIRES_S),
        });
    }
    Ok(line)
}

/// The registration `lines.edit` asks for: none, when the server is
/// given as an empty string, or what the line has with the keys that are
/// given changed. A line that has none needs the server, the user and the
/// password together.
fn register_edit(
    id: u16,
    change: &ctlproto::RegisterChange,
    current: Option<&RegisterConfig>,
) -> Result<Option<RegisterConfig>, Message> {
    if change.server.as_deref() == Some("") {
        return Ok(None);
    }
    let incomplete = || message("RMD0035", &[&id.to_string()]);
    let mut register = match current {
        Some(current) => current.clone(),
        None => RegisterConfig {
            server: change.server.clone().ok_or_else(incomplete)?,
            user: change.user.clone().ok_or_else(incomplete)?,
            password: change.password.clone().ok_or_else(incomplete)?,
            auth_user: String::new(),
            expires_s: RegisterConfig::DEFAULT_EXPIRES_S,
        },
    };
    if let Some(server) = &change.server {
        register.server.clone_from(server);
    }
    if let Some(user) = &change.user {
        register.user.clone_from(user);
    }
    if let Some(password) = &change.password {
        register.password.clone_from(password);
    }
    if let Some(auth_user) = &change.auth_user {
        register.auth_user.clone_from(auth_user);
    }
    if let Some(expires_s) = change.expires_s {
        register.expires_s = expires_s;
    }
    Ok(Some(register))
}

/// The change `lines.edit` describes, to the line as it is now.
fn line_edit(change: &LineChange, current: Option<&LineConfig>) -> Result<LineEdit, Message> {
    Ok(LineEdit {
        modem: change.modem.as_ref().map(|modem| {
            // The change names the line's own modem settings, all of
            // them that the control API has. What it has not, the line's
            // policy for error control and compression and its guard
            // tone, stays as the line has it.
            let own = current.map(|line| &line.modem);
            LineModemEdit {
                modulations: modem.modulations.clone(),
                v8: modem.v8,
                tx_level_dbm0: modem.tx_level_dbm0.map(i64::from),
                error_control: own
                    .and_then(|own| own.error_control)
                    .map(|policy| policy.as_str().to_string()),
                compression: own.and_then(|own| own.compression),
                guard_tone: own
                    .and_then(|own| own.guard_tone)
                    .map(|tone| tone.as_str().to_string()),
            }
        }),
        engine: change
            .engine
            .as_deref()
            .map(|name| engine("lines.edit", name))
            .transpose()?,
        register: change
            .register
            .as_ref()
            .map(|register| {
                register_edit(
                    change.id,
                    register,
                    current.and_then(|line| line.register.as_ref()),
                )
            })
            .transpose()?,
        // An empty name removes the name.
        name: change
            .name
            .as_ref()
            .map(|name| Some(name.clone()).filter(|name| !name.is_empty())),
        backend: change
            .backend
            .as_deref()
            .map(|name| backend("lines.edit", name))
            .transpose()?,
        port: change.port.clone(),
        enabled: change.enabled,
        auto_answer_rings: change.auto_answer_rings,
        unknown_command: change
            .unknown_command
            .as_deref()
            .map(|name| unknown_command("lines.edit", name))
            .transpose()?,
    })
}

/// The event names of a selection, checked. `None` means every event.
fn event_names(
    method: &str,
    selection: &EventSelection,
) -> Result<Option<Vec<&'static str>>, Message> {
    let Some(names) = &selection.events else {
        return Ok(None);
    };
    names
        .iter()
        .map(|name| {
            EVENT_NAMES
                .iter()
                .find(|known| *known == name)
                .copied()
                .ok_or_else(|| message("RMD2003", &[method, &format!("there is no event {name}")]))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Writes one message and its line feed.
async fn write_line(writer: &mut (impl AsyncWrite + Unpin), line: &str) -> io::Result<()> {
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

/// Reads one message: the bytes up to a line feed. Blank lines are skipped.
/// `None` means the client has gone.
async fn read_line(reader: &mut (impl AsyncBufRead + Unpin + Send)) -> io::Result<Option<Line>> {
    loop {
        let mut bytes = Vec::new();
        // One byte past the limit is enough to know the line is too long.
        let read = (&mut *reader)
            .take(MAX_MESSAGE_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)
            .await?;
        if read == 0 {
            return Ok(None);
        }
        if bytes.last() != Some(&b'\n') {
            if bytes.len() > MAX_MESSAGE_BYTES {
                return Ok(Some(Line::TooLong));
            }
            // The client went away in the middle of a message.
            return Ok(None);
        }
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        return Ok(Some(match String::from_utf8(bytes) {
            Ok(text) => Line::Text(text),
            Err(_) => Line::NotText,
        }));
    }
}
