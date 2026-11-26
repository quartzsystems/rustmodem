// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The server: the hello exchange, requests and their answers.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use config::{
    Backend, Config, ConfigFile, ControlConfig, LineConfig, LineEdit, ServiceConfig, SettingsEdit,
    UnknownCommand,
};
use ctlproto::{
    Call, CallList, ClientFrame, ControlSettings, Done, EVENT_NAMES, EventSelection, Hello,
    LineChange, LineList, LineSpec, LogLevel, LogTail, MAX_MESSAGE_BYTES, PROTOCOL, PortInfo,
    PortList, Reloaded, ServerFrame, ServiceInfo, ServiceMode, ServiceSettings, Settings,
    SettingsChange, SettingsResult, Subscription, decode_client, encode,
};
use msg::{Message, Severity, message};
use pool::PoolHandle;
use serde::Serialize;
use serde_json::Value;

use crate::queue::{ClientQueue, Hub, Limits};
use crate::{Connection, Halves, Listener, PortInventory};

/// Takes the server's own log lines. The source is always `CTL`.
pub type LogFn = Arc<dyn Fn(LogLevel, String) + Send + Sync>;

/// What a server is made from.
pub struct ServerParts {
    /// The way to the pool.
    pub pool: PoolHandle,
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
}

struct Shared {
    pool: PoolHandle,
    config: Mutex<ConfigFile>,
    config_path: PathBuf,
    version: String,
    pipe: String,
    hub: Hub,
    log: LogFn,
    limits: Limits,
    mode: ServiceMode,
    ports: Arc<dyn PortInventory>,
    /// The `[service]` and `[control]` settings the service started with.
    /// A change to them is in effect only after a restart.
    running: (ServiceConfig, ControlConfig),
    started: Instant,
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
        let started_ms = unix_ms();
        let running = (
            parts.config.config().service.clone(),
            parts.config.config().control.clone(),
        );
        Server {
            shared: Arc::new(Shared {
                pool: parts.pool,
                mode: parts.mode,
                ports: parts.ports,
                running,
                config_loaded_ms: AtomicU64::new(started_ms),
                config: Mutex::new(parts.config),
                config_path: parts.config_path,
                version: parts.version,
                pipe: parts.pipe,
                hub: parts.hub,
                log: parts.log,
                limits: parts.limits,
                started: Instant::now(),
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
    pub fn refresh_bindings(&self) -> Result<(), Message> {
        let bindings = self
            .shared
            .ports
            .ports()
            .iter()
            .filter_map(|port| Some((port.name.clone(), port.binding()?)))
            .collect();
        self.shared.pool.set_bindings(bindings)
    }

    /// Accepts clients until the listener shuts down, serving each on a
    /// thread of its own.
    pub fn serve(&self, mut listener: Box<dyn Listener>) -> io::Result<()> {
        while let Some(connection) = listener.accept()? {
            let server = self.clone();
            std::thread::Builder::new()
                .name("ctl-client".to_string())
                .spawn(move || server.handle(connection))?;
        }
        Ok(())
    }

    /// Serves one client until it goes away.
    pub fn handle(&self, connection: Box<dyn Connection>) {
        let number = self.shared.next_client.fetch_add(1, Ordering::Relaxed);
        let client = format!("{number} ({})", connection.peer());
        let Halves {
            reader,
            mut writer,
            closer,
        } = connection.split();
        let queue = Arc::new(ClientQueue::new(Arc::clone(&closer), self.shared.limits));

        // One thread writes what is queued, so that a client that is slow
        // to read holds up nobody but itself.
        let writing = Arc::clone(&queue);
        let writer_thread = std::thread::Builder::new()
            .name("ctl-writer".to_string())
            .spawn(move || {
                while let Some(line) = writing.pop() {
                    let written = writer
                        .write_all(line.as_bytes())
                        .and_then(|()| writer.write_all(b"\n"))
                        .and_then(|()| writer.flush());
                    if written.is_err() {
                        writing.close();
                        break;
                    }
                }
            });

        let mut reader = BufReader::new(reader);
        let greeted = self.greet(&mut reader, &queue);
        if greeted {
            self.log(&message("RMD2011", &[&client]));
            self.shared.hub.add(&queue);
            self.converse(&mut reader, &queue);
            self.shared.hub.remove(&queue);
        }

        // Let what is queued go out. The connection closes when its two
        // halves are dropped, the writer's with its thread and the reader's
        // here; it is not closed by force, which on some transports would
        // discard what the client has not read yet.
        queue.finish();
        if let Ok(thread) = writer_thread {
            let _ = thread.join();
        }
        drop(reader);
        if queue.overflowed() {
            self.log(&message("RMD2006", &[&client]));
        } else if greeted {
            self.log(&message("RMD2012", &[&client]));
        }
    }

    /// The hello exchange. Returns whether the client may go on.
    fn greet(&self, reader: &mut impl BufRead, queue: &ClientQueue) -> bool {
        let refuse = |problem: Message| {
            queue.push_reply(encode(&ServerFrame::Error {
                id: None,
                error: problem.into(),
            }));
            false
        };
        match read_line(reader) {
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

    /// Requests and their answers, until the client goes away.
    fn converse(&self, reader: &mut impl BufRead, queue: &Arc<ClientQueue>) {
        loop {
            let line = match read_line(reader) {
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
            queue.push_reply(encode(&self.answer(&line, queue)));
        }
    }

    /// The answer to one line from the client.
    fn answer(&self, line: &str, queue: &ClientQueue) -> ServerFrame {
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
        let result = Call::from_parts(&frame.method, frame.params)
            .and_then(|call| self.dispatch(call, queue));
        match result {
            Ok(result) => ServerFrame::Result { id, result },
            Err(problem) => ServerFrame::Error {
                id: Some(id),
                error: problem.into(),
            },
        }
    }

    fn dispatch(&self, call: Call, queue: &ClientQueue) -> Result<Value, Message> {
        let shared = &self.shared;
        match call {
            Call::ServiceInfo => {
                let lines = shared.pool.lines()?.len() as u32;
                value(&ServiceInfo {
                    version: shared.version.clone(),
                    protocol: PROTOCOL,
                    pipe: shared.pipe.clone(),
                    config_path: shared.config_path.display().to_string(),
                    lines,
                    started_ms: shared.started_ms,
                    uptime_ms: shared.started.elapsed().as_millis() as u64,
                    config_loaded_ms: shared.config_loaded_ms.load(Ordering::Relaxed),
                    mode: shared.mode,
                })
            }
            Call::LinesList => value(&LineList {
                lines: shared.pool.lines()?,
            }),
            Call::LinesGet(line) => value(&shared.pool.line(line.id)?),
            Call::LinesAnswer(line) => {
                shared.pool.answer(line.id)?;
                value(&shared.pool.line(line.id)?)
            }
            Call::LinesHangup(line) => {
                shared.pool.hangup(line.id)?;
                value(&shared.pool.line(line.id)?)
            }
            Call::LinesReset(line) => {
                shared.pool.reset(line.id)?;
                value(&shared.pool.line(line.id)?)
            }
            Call::LinesEnable(line) => {
                let edit = LineEdit {
                    enabled: Some(true),
                    ..LineEdit::default()
                };
                self.change_config(|file| file.edit_line(line.id, &edit))?;
                value(&shared.pool.line(line.id)?)
            }
            Call::LinesDisable(line) => {
                let edit = LineEdit {
                    enabled: Some(false),
                    ..LineEdit::default()
                };
                self.change_config(|file| file.edit_line(line.id, &edit))?;
                value(&shared.pool.line(line.id)?)
            }
            Call::LinesAdd(spec) => {
                let line = new_line(&spec)?;
                self.change_config(|file| file.add_line(&line))?;
                value(&shared.pool.line(spec.id)?)
            }
            Call::LinesEdit(change) => {
                let edit = line_edit(&change)?;
                self.change_config(|file| file.edit_line(change.id, &edit))?;
                value(&shared.pool.line(change.id)?)
            }
            Call::LinesRemove(line) => {
                self.change_config(|file| file.remove_line(line.id))?;
                value(&Done {})
            }
            Call::CallsList(query) => value(&CallList {
                calls: shared.pool.calls(query.last.map(|last| last as usize))?,
            }),
            Call::ConfigReload => {
                let mut current = self.config();
                let loaded = ConfigFile::load(&shared.config_path).map_err(first)?;
                shared.pool.apply_config(loaded.config().clone())?;
                let lines = loaded.config().lines.len() as u32;
                *current = loaded;
                drop(current);
                shared.config_loaded_ms.store(unix_ms(), Ordering::Relaxed);
                self.refresh_bindings()?;
                value(&Reloaded { lines })
            }
            Call::ConfigGet => value(&self.settings(self.config().config())),
            Call::ConfigSet(change) => value(&self.change_settings(&change)?),
            Call::StatsGet => value(&shared.pool.stats()?),
            Call::LogTail(query) => value(&LogTail {
                lines: shared.hub.log_tail(query.last.map(|last| last as usize)),
            }),
            Call::PortsList => value(&PortList {
                ports: self.ports()?,
            }),
            Call::Subscribe(selection) => {
                let names = event_names("subscribe", &selection)?;
                value(&Subscription {
                    events: queue.subscribe(names.as_deref()),
                })
            }
            Call::Unsubscribe(selection) => {
                let names = event_names("unsubscribe", &selection)?;
                value(&Subscription {
                    events: queue.unsubscribe(names.as_deref()),
                })
            }
        }
    }

    fn config(&self) -> MutexGuard<'_, ConfigFile> {
        self.shared.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Changes the configuration: the change is made to a copy, checked,
    /// written to the file and given to the pool, and only then kept. If
    /// any step fails, nothing has changed.
    fn change_config(
        &self,
        change: impl FnOnce(&mut ConfigFile) -> Result<(), Vec<Message>>,
    ) -> Result<(), Message> {
        let mut current = self.config();
        let mut next = current.clone();
        change(&mut next).map_err(first)?;
        next.save(&self.shared.config_path)?;
        self.shared.pool.apply_config(next.config().clone())?;
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
            path: self.shared.config_path.display().to_string(),
        }
    }

    /// `config.set`: the change is made to a copy, checked and written to
    /// the file, and only then kept. The lines are not touched. What the
    /// service is doing does not change until it starts again.
    fn change_settings(&self, change: &SettingsChange) -> Result<SettingsResult, Message> {
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
        };
        let mut current = self.config();
        let mut next = current.clone();
        next.edit_settings(&edit).map_err(first)?;
        next.save(&self.shared.config_path)?;
        *current = next;
        let config = current.config();
        let (service, control) = &self.shared.running;
        Ok(SettingsResult {
            restart_required: config.service != *service || config.control != *control,
            config: self.settings(config),
        })
    }

    /// `ports.list`: the ports of the machine in name order, each with the
    /// line that uses it.
    fn ports(&self) -> Result<Vec<PortInfo>, Message> {
        let lines = self.shared.pool.lines()?;
        let mut ports = self.shared.ports.ports();
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

/// Now, in milliseconds since the Unix epoch.
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
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
    Ok(line)
}

/// The change `lines.edit` describes.
fn line_edit(change: &LineChange) -> Result<LineEdit, Message> {
    Ok(LineEdit {
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

/// Reads one message: the bytes up to a line feed. Blank lines are skipped.
/// `None` means the client has gone.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Line>> {
    loop {
        let mut bytes = Vec::new();
        // One byte past the limit is enough to know the line is too long.
        let read = reader
            .by_ref()
            .take(MAX_MESSAGE_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
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
