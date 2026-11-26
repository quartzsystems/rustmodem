// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the router shows of itself, and how it is spoken to from outside
//! the pool's thread.
//!
//! The router lives in the pool, on the pool's thread. The control server
//! is on others. Between them is a [`TelephonyHandle`]: the router keeps a
//! snapshot of its status in it and takes its commands from it. What the
//! router has to say as it happens, registrations changing and the SIP
//! messages that pass, goes to an [`Observer`].

use std::collections::VecDeque;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};

use msg::Message;

use crate::hunt::LineId;

/// The state of a line's registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationState {
    /// Not registered, and not trying.
    Unregistered,
    /// A REGISTER is out and there is no binding yet.
    Registering,
    Registered,
    /// The last try failed; another follows.
    Failed,
}

/// One line's registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationStatus {
    pub line: LineId,
    /// The registrar, as the configuration names it.
    pub server: String,
    /// The extension.
    pub user: String,
    pub state: RegistrationState,
    /// The seconds left of the binding, when registered.
    pub expires_in_s: Option<u64>,
    /// Why the registration failed, when it has.
    pub error: Option<Message>,
}

/// The router as the control API shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SipStatus {
    /// The trunk listener.
    pub listen: SocketAddrV4,
    pub trusted_peers: Vec<String>,
    pub outbound_peer: String,
    pub registrations: Vec<RegistrationStatus>,
    /// Requests dropped because they came from an address that is not
    /// trusted.
    pub dropped_untrusted: u64,
    /// Calls the router has, ringing, connected or being cleared.
    pub calls: usize,
}

/// Which way a traced message went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Sent,
    Received,
}

/// Takes what the router has to say as it happens. Called on the pool's
/// thread: it must not block.
pub trait Observer: Send {
    /// A line's registration changed state.
    fn registration(&mut self, _status: &RegistrationStatus) {}

    /// A SIP message was sent or received. `text` is the message as it
    /// was on the wire.
    fn trace(&mut self, _direction: Direction, _peer: SocketAddr, _text: &str) {}

    /// Something for the service log, about a line when there is one.
    fn log(&mut self, _line: Option<LineId>, _message: &Message) {}
}

/// An observer that looks away.
pub struct Unobserved;

impl Observer for Unobserved {}

/// Something asked of the router from outside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Register again now: one line, or every line that registers.
    Reregister(Option<LineId>),
}

#[derive(Debug, Default)]
struct Shared {
    commands: VecDeque<Command>,
    status: Option<SipStatus>,
}

/// The way to the router from outside the pool's thread. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct TelephonyHandle {
    shared: Arc<Mutex<Shared>>,
}

impl TelephonyHandle {
    pub fn new() -> TelephonyHandle {
        TelephonyHandle::default()
    }

    /// Asks for a line, or every line that registers, to register again
    /// now. It happens at the router's next turn.
    pub fn reregister(&self, line: Option<LineId>) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.commands.push_back(Command::Reregister(line));
        }
    }

    /// The router's status as of its last turn. `None` before its first.
    pub fn status(&self) -> Option<SipStatus> {
        self.shared.lock().ok()?.status.clone()
    }

    pub(crate) fn take_command(&self) -> Option<Command> {
        self.shared.lock().ok()?.commands.pop_front()
    }

    pub(crate) fn publish(&self, status: SipStatus) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.status = Some(status);
        }
    }
}
