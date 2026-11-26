// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control API message types.
//!
//! These are the serde types of the control API and nothing else: no I/O,
//! no threads. The service, the command-line client and the manager all
//! use them, so they agree on the wire format by construction.
//!
//! - `model` is what the service reports: lines, calls and events.
//! - `wire` is the framing: hello, requests, responses, and the methods
//!   with their parameters and results.
//! - [`doc`] describes the API for people and renders
//!   `docs/CONTROL_API.md`.

#![forbid(unsafe_code)]

pub mod doc;
mod model;
mod wire;

pub use model::{
    CallEvent, CallInfo, CallRecord, ConnectInfo, ConnectTally, Counters, Direction, EndReason,
    ErrorInfo, Event, LineInfo, LineState, LogEvent, LogLevel, PortInfo, PortKind, ServiceMode,
    Stats, Tally,
};
pub use wire::{
    Call, CallList, CallsQuery, ClientFrame, ControlChange, ControlSettings, Done, EVENT_NAMES,
    EventSelection, Hello, LineChange, LineList, LineRef, LineSpec, LogTail, LogTailQuery,
    MAX_MESSAGE_BYTES, PROTOCOL, PortList, Reloaded, RequestFrame, ServerFrame, ServiceChange,
    ServiceInfo, ServiceSettings, Settings, SettingsChange, SettingsResult, Subscription,
    decode_client, decode_server, encode,
};
