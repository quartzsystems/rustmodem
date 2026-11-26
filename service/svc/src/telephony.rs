// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the service adds when SIP is on: the router on its socket, the
//! media threads, the writer of call captures, and what carries their
//! news to the log and to control clients.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use config::Config;
use ctlproto::{Event as ControlEvent, SipTrace, TraceDirection};
use ctlsrv::Hub;
use media::{CaptureEvent, CaptureWriter, ThreadedRunner};
use msg::{Message, message};
use pool::{CallRouter, LineId, MediaRunner};
use svc::sipsock::SipSocket;
use telephony::{
    Direction, Observer, RouterParts, Settings, SipRouter, TelephonyHandle, system_random,
};
use tokio::sync::Notify;

use crate::VERSION;
use crate::logger::{self, Logger};

/// The tag of the SIP router in log lines.
const SOURCE: &str = "SIP";

/// The tag of the media threads and the capture writer in log lines.
const MEDIA_SOURCE: &str = "RTP";

/// The name of the capture directory under the log directory.
const CAPTURES: &str = "captures";

/// Takes what the router has to say as it happens.
struct Watcher {
    logger: Logger,
    hub: Hub,
}

impl Observer for Watcher {
    fn trace(&mut self, direction: Direction, peer: SocketAddr, text: &str) {
        // A trace is made only while somebody has the trace window open.
        if !self.hub.wants("sip.trace") {
            return;
        }
        self.hub.publish(&ControlEvent::SipTrace(SipTrace {
            time_ms: logger::unix_ms(),
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
            Some(line) => self.logger.message(&format!("Line{line:02}"), message),
            None => self.logger.message(SOURCE, message),
        }
    }
}

/// Where call captures go: `media.capture_dir`, or `captures` under the
/// log directory.
pub fn capture_dir(config: &Config, log_dir: &Path) -> PathBuf {
    if config.media.capture_dir.is_empty() {
        log_dir.join(CAPTURES)
    } else {
        PathBuf::from(&config.media.capture_dir)
    }
}

/// The router and the media runner that carries its calls.
pub type Parts = (Box<dyn CallRouter>, Box<dyn MediaRunner>);

/// Binds the SIP listener and makes the router and the media runner. An
/// error is why there can be no SIP: the listener could not be bound, or
/// a thread could not be started. This is called on the runtime, which
/// the listener's task is started on.
pub fn start(
    config: &Config,
    log_dir: &Path,
    logger: &Logger,
    hub: &Hub,
    wake: &Arc<Notify>,
    media_waker: media::Waker,
) -> Result<Parts, Message> {
    let listen = config.sip.listen.to_string();
    let waker = {
        let wake = Arc::clone(wake);
        Arc::new(move || wake.notify_one())
    };
    let transport = SipSocket::bind(config.sip.listen, waker)
        .map_err(|error| message("RMD5002", &[&listen, &error.to_string()]))?;
    let listening = message("RMD5001", &[&transport.local().to_string()]);

    let capture_log = logger.clone();
    let writer = CaptureWriter::new(
        capture_dir(config, log_dir),
        Box::new(move |event| match event {
            CaptureEvent::Started { line, path } => capture_log.message(
                MEDIA_SOURCE,
                &message("RMD6006", &[&line.to_string(), &path.display().to_string()]),
            ),
            CaptureEvent::Failed { line, path, error } => capture_log.message(
                MEDIA_SOURCE,
                &message(
                    "RMD6007",
                    &[
                        &line.to_string(),
                        &path.display().to_string(),
                        &error.to_string(),
                    ],
                ),
            ),
        }),
    )
    .map_err(|error| message("RMD0003", &[&error.to_string()]))?;
    // A thread for each line there is now; more are made if lines are
    // added.
    let runner = ThreadedRunner::with_capture(config.lines.len(), media_waker, writer)
        .map_err(|error| message("RMD0003", &[&error.to_string()]))?;

    let router = SipRouter::new(RouterParts {
        settings: Settings::new(config, VERSION),
        transport: Box::new(transport),
        random: system_random(),
        observer: Box::new(Watcher {
            logger: logger.clone(),
            hub: hub.clone(),
        }),
        handle: TelephonyHandle::new(),
        version: VERSION.to_string(),
    });
    logger.message(SOURCE, &listening);
    Ok((Box::new(router), Box::new(runner)))
}
