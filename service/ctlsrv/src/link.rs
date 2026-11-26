// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! How the tasks of the service reach the pool.
//!
//! The pool is owned by one task, and nothing else touches it. Whatever
//! wants something of it, the control server above all, sends a
//! [`Request`] through a [`PoolLink`] and awaits the answer; the pool's
//! task takes the requests from the [`PoolRequests`] and runs each against
//! the pool. A request never waits on anything: it is one call on the
//! pool, and the answer is ready when the call returns.

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use config::Config;
use ctlproto::{CallRecord, LineInfo, SipStatus, Stats};
use msg::{Message, message};
use pool::{Command, LineId, Pool};
use tokio::sync::{mpsc, oneshot};

/// How long a caller waits for the pool to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Something asked of the pool. The pool's task runs it.
pub struct Request(Box<dyn FnOnce(&mut Pool) + Send>);

impl Request {
    /// Carries the request out and sends its answer to whoever asked.
    pub fn run(self, pool: &mut Pool) {
        (self.0)(pool);
    }
}

/// The pool's end of the link: the requests, in the order they were made.
pub type PoolRequests = mpsc::UnboundedReceiver<Request>;

/// A handle the tasks and threads of the service use to ask things of the
/// pool.
#[derive(Clone)]
pub struct PoolLink {
    requests: mpsc::UnboundedSender<Request>,
}

/// Makes the link between the pool's task and everything else.
pub fn pool_link() -> (PoolLink, PoolRequests) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (PoolLink { requests: sender }, receiver)
}

/// The pool's task has gone, or does not answer: the service is stopping.
fn stopping() -> Message {
    message("RMD2010", &[])
}

/// Carries out one of the pool's commands and takes its answer, which the
/// pool gives before `command` returns.
fn command<T>(pool: &mut Pool, make: impl FnOnce(std_mpsc::Sender<T>) -> Command) -> Option<T> {
    let (sender, receiver) = std_mpsc::channel();
    pool.command(make(sender));
    receiver.try_recv().ok()
}

impl PoolLink {
    /// Runs `ask` against the pool, on the pool's task, and awaits what it
    /// returns.
    pub async fn with<T: Send + 'static>(
        &self,
        ask: impl FnOnce(&mut Pool) -> T + Send + 'static,
    ) -> Result<T, Message> {
        let (answer, answered) = oneshot::channel();
        self.requests
            .send(Request(Box::new(move |pool| {
                // A caller that has stopped waiting is not an error.
                let _ = answer.send(ask(pool));
            })))
            .map_err(|_| stopping())?;
        match tokio::time::timeout(ANSWER_TIMEOUT, answered).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) | Err(_) => Err(stopping()),
        }
    }

    /// As [`PoolLink::with`], for a thread that is not one of the
    /// runtime's: it waits where it stands.
    pub fn blocking_with<T: Send + 'static>(
        &self,
        ask: impl FnOnce(&mut Pool) -> T + Send + 'static,
    ) -> Result<T, Message> {
        let (answer, answered) = std_mpsc::channel();
        self.requests
            .send(Request(Box::new(move |pool| {
                let _ = answer.send(ask(pool));
            })))
            .map_err(|_| stopping())?;
        answered
            .recv_timeout(ANSWER_TIMEOUT)
            .map_err(|_| stopping())
    }

    async fn ask<T: Send + 'static>(
        &self,
        make: impl FnOnce(std_mpsc::Sender<T>) -> Command + Send + 'static,
    ) -> Result<T, Message> {
        self.with(move |pool| command(pool, make))
            .await?
            .ok_or_else(stopping)
    }

    /// Every line.
    pub async fn lines(&self) -> Result<Vec<LineInfo>, Message> {
        self.ask(Command::Lines).await
    }

    /// Every line, for a thread that is not one of the runtime's.
    pub fn blocking_lines(&self) -> Result<Vec<LineInfo>, Message> {
        self.blocking_with(|pool| command(pool, Command::Lines))?
            .ok_or_else(stopping)
    }

    /// One line.
    pub async fn line(&self, id: LineId) -> Result<LineInfo, Message> {
        self.ask(move |answer| Command::Line(id, answer)).await?
    }

    /// Answer the call ringing on a line.
    pub async fn answer(&self, id: LineId) -> Result<(), Message> {
        self.ask(move |answer| Command::Answer(id, answer)).await?
    }

    /// Hang up a line.
    pub async fn hangup(&self, id: LineId) -> Result<(), Message> {
        self.ask(move |answer| Command::Hangup(id, answer)).await?
    }

    /// Reset a line.
    pub async fn reset(&self, id: LineId) -> Result<(), Message> {
        self.ask(move |answer| Command::Reset(id, answer)).await?
    }

    /// The calls that have ended, oldest first; at most the last `last`.
    pub async fn calls(&self, last: Option<usize>) -> Result<Vec<CallRecord>, Message> {
        self.ask(move |answer| Command::Calls(last, answer)).await
    }

    /// The statistics since the pool started.
    pub async fn stats(&self) -> Result<Stats, Message> {
        self.ask(Command::Stats).await
    }

    /// Tell the pool what the host side of each port is bound to.
    pub async fn set_bindings(&self, bindings: Vec<(String, String)>) -> Result<(), Message> {
        self.ask(move |answer| Command::SetBindings(bindings, answer))
            .await
    }

    /// Make the lines match this configuration.
    pub async fn apply_config(&self, config: Config) -> Result<(), Message> {
        self.ask(move |answer| Command::ApplyConfig(Box::new(config), answer))
            .await
    }

    /// Turn the capture of a line's calls on or off. The line afterwards.
    pub async fn capture(&self, id: LineId, on: bool) -> Result<LineInfo, Message> {
        self.ask(move |answer| Command::Capture(id, on, answer))
            .await?
    }

    /// The SIP side of the service.
    pub async fn sip_status(&self) -> Result<SipStatus, Message> {
        self.ask(Command::SipStatus).await
    }

    /// Register a line again now, or every line that registers.
    pub async fn reregister(&self, id: Option<LineId>) -> Result<(), Message> {
        self.ask(move |answer| Command::Reregister(id, answer))
            .await?
    }
}
