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
//! - `telephony` is what it reports about SIP and media.
//! - `modem` is what it reports about its modems and takes to set them.
//! - `wire` is the framing: hello, requests, responses, and the methods
//!   with their parameters and results.
//! - [`doc`] describes the API for people and renders
//!   `docs/CONTROL_API.md`.

#![forbid(unsafe_code)]

pub mod doc;
mod model;
mod modem;
mod telephony;
mod wire;

pub use model::{
    CallEvent, CallInfo, CallRecord, ConnectInfo, ConnectTally, Counters, Direction, EndReason,
    ErrorInfo, Event, LineInfo, LineState, LogEvent, LogLevel, PortInfo, PortKind, ServiceMode,
    Stats, Tally,
};
pub use modem::{
    LineDiag, LineModem, LineModemSpec, ModemChange, ModemInfo, ModemSettings, ModemStateChange,
    SelfTest, SelfTestResult,
};
pub use telephony::{
    HuntGroupInfo, HuntGroupList, LineSip, ListenerState, MediaInfo, Registration,
    RegistrationState, RouteInfo, RouteList, SipMode, SipRegistration, SipStatus, SipSummary,
    SipTrace, TraceDirection,
};
pub use wire::{
    Call, CallList, CallsQuery, CaptureSwitch, ClientFrame, ControlChange, ControlSettings, Done,
    EVENT_NAMES, EVENTS_BY_NAME_ONLY, EventSelection, Hello, HuntGroupChange, HuntGroupRef,
    HuntGroupSpec, LINE_DIAG_PER_SECOND, LineChange, LineList, LineRef, LineSpec, LogTail,
    LogTailQuery, MAX_MESSAGE_BYTES, MediaChange, MediaSettings, PROTOCOL, PortList,
    RegisterChange, RegisterSpec, Reloaded, RequestFrame, ReregisterQuery, RouteChange, RouteMove,
    RouteRef, RouteSpec, ServerFrame, ServiceChange, ServiceInfo, ServiceSettings, Settings,
    SettingsChange, SettingsResult, SipChange, SipSettings, Subscription, decode_client,
    decode_server, encode,
};
