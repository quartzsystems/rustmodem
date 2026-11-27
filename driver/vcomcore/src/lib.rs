// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The virtual COM port's state machine.
//!
//! A [`Port`] is one RustModem line's serial port with no framework in it:
//! it is given requests, octets and the time, and gives back completions.
//! The driver is a thin layer that turns what Windows asks into calls here
//! and completes its requests as the port says, so everything a host can
//! observe of the port is decided, and tested, in this crate.
//!
//! The port has two sides. The **host** side is whatever opened the COM
//! port: RRAS through Windows' modem stack, or a terminal. The **control**
//! side is the RustModem service. Octets written by one are read by the
//! other, through two bounded queues, and the control side stands where a
//! modem's lines would: it sets CTS, DSR, RI and DCD, and is told of DTR
//! and RTS.
//!
//! What a serial port is asked for here follows the Win32 documentation of
//! `COMMTIMEOUTS`, `SetCommMask`, `WaitCommEvent`, `PurgeComm` and
//! `GetCommModemStatus`. Stored settings are stored and read back and move
//! no octet differently: there is no baud rate here.

#![forbid(unsafe_code)]

pub mod ioctl;
mod port;
mod settings;

pub use port::{
    Completion, ControlEvent, ControlState, Done, Fault, Millis, ModemLines, Port, RequestId,
};
pub use settings::{
    Chars, CommStatus, HandFlow, LineControl, QueueSize, Stats, Timeouts, events, modem_control,
    modem_status, purge,
};
