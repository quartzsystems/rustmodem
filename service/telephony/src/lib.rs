// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The telephone side of the service: which line a call goes to.
//!
//! This is the pure part: [`hunt`] decides, from the routes, the hunt
//! groups and the state of the lines, which line an incoming call is
//! offered to, or why it is turned away.

#![forbid(unsafe_code)]

pub mod hunt;

pub use hunt::{Decision, Hunter};
