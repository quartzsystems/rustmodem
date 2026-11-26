// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The two things a media thread needs that `std` cannot do: socket
//! buffers of a set size, and the multimedia scheduling class.
//!
//! On Windows both are Win32 calls. Elsewhere, where only the tests run,
//! neither is done: the buffers stay as the system made them and the
//! thread keeps its priority.
//!
//! BLOCKED(phase5): the packets are not marked with a DSCP. On Windows an
//! application cannot set one on its own sockets; it is set by a QoS
//! policy, which the installer creates.

use std::net::UdpSocket;

/// The size asked for each of a call's socket buffers. A second of the
/// smallest packets, a hundred of 120 octets with their headers, is 12 kB;
/// this holds several seconds of the largest.
pub const SOCKET_BUFFER: i32 = 256 * 1024;

/// Asks for receive and send buffers of [`SOCKET_BUFFER`] octets. A
/// system that gives less gives what it gives: the call still works.
#[cfg(windows)]
pub fn size_buffers(socket: &UdpSocket) {
    use std::os::windows::io::AsRawSocket;

    use windows::Win32::Networking::WinSock::{
        SO_RCVBUF, SO_SNDBUF, SOCKET, SOL_SOCKET, setsockopt,
    };

    let raw = SOCKET(socket.as_raw_socket() as usize);
    let size = SOCKET_BUFFER.to_ne_bytes();
    for option in [SO_RCVBUF, SO_SNDBUF] {
        // SAFETY: `raw` is the handle of `socket`, which is borrowed for
        // the whole call and so is open; the option value is a four-octet
        // integer that lives until the call returns, as SO_RCVBUF and
        // SO_SNDBUF take.
        let _ = unsafe { setsockopt(raw, SOL_SOCKET, option, Some(&size)) };
    }
}

#[cfg(not(windows))]
pub fn size_buffers(_socket: &UdpSocket) {}

/// The thread is registered with the Multimedia Class Scheduler Service
/// under the `Pro Audio` task for as long as this lives.
#[cfg(windows)]
pub struct ProAudio(Option<windows::Win32::Foundation::HANDLE>);

#[cfg(windows)]
impl ProAudio {
    /// Registers the calling thread. If the service refuses, the thread
    /// runs at its ordinary priority.
    pub fn enter() -> ProAudio {
        use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        use windows::core::w;

        let mut index = 0u32;
        // SAFETY: the task name is a constant NUL-terminated wide string
        // and `index` is a local the call writes a task index to.
        let handle = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut index) };
        ProAudio(handle.ok())
    }
}

#[cfg(windows)]
impl Drop for ProAudio {
    fn drop(&mut self) {
        use windows::Win32::System::Threading::AvRevertMmThreadCharacteristics;

        if let Some(handle) = self.0.take() {
            // SAFETY: `handle` came from AvSetMmThreadCharacteristicsW on
            // this thread, which is the thread that drops it, and is
            // reverted nowhere else.
            let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
        }
    }
}

#[cfg(not(windows))]
pub struct ProAudio;

#[cfg(not(windows))]
impl ProAudio {
    pub fn enter() -> ProAudio {
        ProAudio
    }
}
