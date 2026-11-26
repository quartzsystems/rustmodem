// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the tests of the service over real sockets share: the runtime the
//! SIP sockets' tasks run on.

#![allow(dead_code)]

use std::io;
use std::net::SocketAddrV4;
use std::sync::OnceLock;

use svc::sipsock::{SipSocket, Waker};
use tokio::runtime::Runtime;

/// The runtime of the test program, built as the service builds its own.
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| svc::runtime::build().unwrap())
}

/// A SIP socket on `listen`, read by a task on the runtime of the tests.
pub fn sip_socket(listen: SocketAddrV4, waker: Waker) -> io::Result<SipSocket> {
    let _on_runtime = runtime().enter();
    SipSocket::bind(listen, waker)
}
