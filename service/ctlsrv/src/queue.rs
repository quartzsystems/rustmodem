// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What is waiting to be written to each client, and the hub that puts
//! events there.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use ctlproto::{EVENT_NAMES, Event, encode};

use crate::Closer;

/// How much may wait for one client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most messages queued for a client. A state event that would go
    /// past this disconnects the client instead.
    pub max_queued: usize,
    /// The most `log` events queued for a client. One more drops the
    /// oldest.
    pub max_logs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_queued: 4096,
            max_logs: 512,
        }
    }
}

struct Item {
    line: Arc<str>,
    is_log: bool,
}

#[derive(Default)]
struct State {
    items: VecDeque<Item>,
    logs: usize,
    /// The events the client has subscribed to.
    subscribed: BTreeSet<&'static str>,
    /// Nothing more is accepted; the writer stops once the queue is empty.
    finishing: bool,
    /// The connection is gone or going; the writer stops at once.
    closed: bool,
    /// The client was disconnected for falling too far behind.
    overflowed: bool,
}

/// The messages waiting to be written to one client.
pub(crate) struct ClientQueue {
    state: Mutex<State>,
    ready: Condvar,
    closer: Arc<dyn Closer>,
    limits: Limits,
}

impl ClientQueue {
    pub fn new(closer: Arc<dyn Closer>, limits: Limits) -> ClientQueue {
        ClientQueue {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
            closer,
            limits,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The state is a queue and some flags; a thread that panicked while
        // holding the lock cannot have left it half-changed in a way that
        // matters more than the panic.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues the answer to a request, or a hello. Answers are never
    /// dropped: a client gets one for each request it sends, so it cannot
    /// make more of them than it reads requests' worth.
    pub fn push_reply(&self, line: String) {
        let mut state = self.lock();
        if state.closed || state.finishing {
            return;
        }
        state.items.push_back(Item {
            line: line.into(),
            is_log: false,
        });
        self.ready.notify_one();
    }

    /// Queues an event, if the client has subscribed to it. Never blocks.
    fn offer(&self, name: &str, is_state: bool, line: &Arc<str>) {
        let mut state = self.lock();
        if state.closed || state.finishing || !state.subscribed.contains(name) {
            return;
        }
        if is_state {
            if state.items.len() >= self.limits.max_queued {
                // The client is too far behind to be told the state it has
                // missed. Disconnect it; it can connect again and ask.
                state.closed = true;
                state.overflowed = true;
                state.items.clear();
                state.logs = 0;
                drop(state);
                self.ready.notify_all();
                self.closer.close();
                return;
            }
        } else {
            if state.logs >= self.limits.max_logs
                && let Some(oldest) = state.items.iter().position(|item| item.is_log)
            {
                state.items.remove(oldest);
                state.logs -= 1;
            }
            if state.items.len() >= self.limits.max_queued {
                // Full of state events and answers: this log line is the
                // one to lose.
                return;
            }
            state.logs += 1;
        }
        state.items.push_back(Item {
            line: Arc::clone(line),
            is_log: !is_state,
        });
        self.ready.notify_one();
    }

    /// The next message to write. Waits for one; `None` means the writer
    /// should stop.
    pub fn pop(&self) -> Option<Arc<str>> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(item) = state.items.pop_front() {
                if item.is_log {
                    state.logs -= 1;
                }
                return Some(item.line);
            }
            if state.finishing {
                return None;
            }
            state = self.ready.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Lets the writer write what is queued and then stop.
    pub fn finish(&self) {
        self.lock().finishing = true;
        self.ready.notify_all();
    }

    /// Stops the writer at once.
    pub fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    pub fn overflowed(&self) -> bool {
        self.lock().overflowed
    }

    /// Adds to the client's subscription; `None` means every event.
    /// Returns what the client is now subscribed to.
    pub fn subscribe(&self, events: Option<&[&'static str]>) -> Vec<String> {
        let mut state = self.lock();
        match events {
            Some(names) => state.subscribed.extend(names),
            None => state.subscribed.extend(EVENT_NAMES),
        }
        subscription(&state)
    }

    /// Takes from the client's subscription; `None` means every event.
    /// Returns what the client is still subscribed to.
    pub fn unsubscribe(&self, events: Option<&[&'static str]>) -> Vec<String> {
        let mut state = self.lock();
        match events {
            Some(names) => {
                for name in names {
                    state.subscribed.remove(name);
                }
            }
            None => state.subscribed.clear(),
        }
        subscription(&state)
    }

    fn wants(&self, name: &str) -> bool {
        let state = self.lock();
        !state.closed && !state.finishing && state.subscribed.contains(name)
    }
}

/// The subscription in the order the documentation lists the events.
fn subscription(state: &State) -> Vec<String> {
    EVENT_NAMES
        .iter()
        .filter(|name| state.subscribed.contains(*name))
        .map(|name| name.to_string())
        .collect()
}

/// Where events are published. Every connected client's queue is known to
/// the hub; each event goes to the clients that have subscribed to it.
#[derive(Clone, Default)]
pub struct Hub {
    clients: Arc<Mutex<Vec<Arc<ClientQueue>>>>,
}

impl Hub {
    /// A hub with no clients.
    pub fn new() -> Hub {
        Hub::default()
    }

    fn clients(&self) -> MutexGuard<'_, Vec<Arc<ClientQueue>>> {
        self.clients.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sends an event to every client that has subscribed to it. Never
    /// blocks on a client: it only queues, and a client whose queue is full
    /// loses log lines or is disconnected.
    pub fn publish(&self, event: &Event) {
        let name = event.name();
        let clients = self.clients();
        if !clients.iter().any(|client| client.wants(name)) {
            return;
        }
        let line: Arc<str> = encode(event).into();
        for client in clients.iter() {
            client.offer(name, event.is_state(), &line);
        }
    }

    /// How many clients are connected.
    pub fn client_count(&self) -> usize {
        self.clients().len()
    }

    pub(crate) fn add(&self, client: &Arc<ClientQueue>) {
        self.clients().push(Arc::clone(client));
    }

    pub(crate) fn remove(&self, client: &Arc<ClientQueue>) {
        self.clients().retain(|c| !Arc::ptr_eq(c, client));
    }
}
