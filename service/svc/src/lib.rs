// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The runner of the rustmodem service: the Tokio runtime its I/O shell
//! runs on, and the signal that stops it.
//!
//! This is the part of the service that `rustmodem-svc.exe` and the demo
//! service share. It uses nothing of Windows, so it builds and is tested
//! wherever the pure crates are. What is of Windows only, the service
//! control manager, the event log, the console and the registry, is in
//! the program beside it.
//!
//! `docs/RUNTIME.md` says what runs on the runtime and what must not.

#![forbid(unsafe_code)]

pub mod runtime;
pub mod shutdown;
