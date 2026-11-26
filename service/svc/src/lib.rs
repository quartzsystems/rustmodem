// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The runner of the rustmodem service: the Tokio runtime its I/O shell
//! runs on, the task that owns the pool, the threads that own the serial
//! ports, the task that owns the SIP socket, and the signal that stops
//! them all.
//!
//! This is the part of the service that `rustmodem-svc.exe` and the demo
//! service share. It needs nothing of Windows, so it builds and is tested
//! on Linux as the pure crates are; the one thing it takes from Windows
//! where it has it is the wait on a serial port's handle. What is of
//! Windows only, the service control manager, the event log, the console
//! and the registry, is in the program beside it.
//!
//! `docs/RUNTIME.md` says what runs on the runtime and what must not.

#![forbid(unsafe_code)]

pub mod bridge;
pub mod mediawake;
pub mod pooltask;
pub mod runtime;
pub mod shutdown;
pub mod sipsock;
