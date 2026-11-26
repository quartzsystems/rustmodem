// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! How other threads reach the pool.
//!
//! The pool runs on one thread. Anything else that wants something from it,
//! the control server above all, sends a [`Command`] through a
//! [`PoolHandle`] and waits for the answer. The pool's thread takes the
//! commands from the [`PoolInbox`] between its ticks.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use config::Config;
use ctlproto::{CallRecord, LineInfo, Stats};
use msg::{Message, message};

use crate::pool::Pool;
use crate::router::LineId;

/// How long a caller waits for the pool to answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Something asked of the pool, with where to send the answer.
pub enum Command {
    /// Every line.
    Lines(Sender<Vec<LineInfo>>),
    /// One line.
    Line(LineId, Sender<Result<LineInfo, Message>>),
    /// Answer the call ringing on a line.
    Answer(LineId, Sender<Result<(), Message>>),
    /// Hang up a line.
    Hangup(LineId, Sender<Result<(), Message>>),
    /// Reset a line.
    Reset(LineId, Sender<Result<(), Message>>),
    /// The calls that have ended, oldest first; at most the last `n`.
    Calls(Option<usize>, Sender<Vec<CallRecord>>),
    /// The statistics since the pool started.
    Stats(Sender<Stats>),
    /// What the host side of each port is bound to: port name and text.
    SetBindings(Vec<(String, String)>, Sender<()>),
    /// Make the lines match this configuration.
    ApplyConfig(Box<Config>, Sender<()>),
}

/// The pool's end of the command channel.
pub struct PoolInbox {
    commands: Receiver<Command>,
}

/// A handle other threads use to ask things of the pool.
#[derive(Clone)]
pub struct PoolHandle {
    commands: Sender<Command>,
}

/// Makes the channel between the pool's thread and everything else.
pub fn command_channel() -> (PoolHandle, PoolInbox) {
    let (sender, receiver) = mpsc::channel();
    (
        PoolHandle { commands: sender },
        PoolInbox { commands: receiver },
    )
}

impl PoolInbox {
    /// Carries out every command that is waiting. Returns `false` once
    /// every handle is gone and nothing more can arrive.
    pub fn serve(&self, pool: &mut Pool) -> bool {
        loop {
            match self.commands.try_recv() {
                Ok(command) => pool.command(command),
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }
}

impl PoolHandle {
    fn ask<T>(&self, command: impl FnOnce(Sender<T>) -> Command) -> Result<T, Message> {
        let (sender, receiver) = mpsc::channel();
        // The pool's thread has gone, or does not answer: the service is
        // stopping.
        let stopping = || message("RMD2010", &[]);
        self.commands
            .send(command(sender))
            .map_err(|_| stopping())?;
        receiver
            .recv_timeout(ANSWER_TIMEOUT)
            .map_err(|_| stopping())
    }

    /// Every line.
    pub fn lines(&self) -> Result<Vec<LineInfo>, Message> {
        self.ask(Command::Lines)
    }

    /// One line.
    pub fn line(&self, id: LineId) -> Result<LineInfo, Message> {
        self.ask(|answer| Command::Line(id, answer))?
    }

    /// Answer the call ringing on a line.
    pub fn answer(&self, id: LineId) -> Result<(), Message> {
        self.ask(|answer| Command::Answer(id, answer))?
    }

    /// Hang up a line.
    pub fn hangup(&self, id: LineId) -> Result<(), Message> {
        self.ask(|answer| Command::Hangup(id, answer))?
    }

    /// Reset a line.
    pub fn reset(&self, id: LineId) -> Result<(), Message> {
        self.ask(|answer| Command::Reset(id, answer))?
    }

    /// The calls that have ended, oldest first; at most the last `last`.
    pub fn calls(&self, last: Option<usize>) -> Result<Vec<CallRecord>, Message> {
        self.ask(|answer| Command::Calls(last, answer))
    }

    /// The statistics since the pool started.
    pub fn stats(&self) -> Result<Stats, Message> {
        self.ask(Command::Stats)
    }

    /// Tell the pool what the host side of each port is bound to.
    pub fn set_bindings(&self, bindings: Vec<(String, String)>) -> Result<(), Message> {
        self.ask(|answer| Command::SetBindings(bindings, answer))
    }

    /// Make the lines match this configuration.
    pub fn apply_config(&self, config: Config) -> Result<(), Message> {
        self.ask(|answer| Command::ApplyConfig(Box::new(config), answer))
    }
}
