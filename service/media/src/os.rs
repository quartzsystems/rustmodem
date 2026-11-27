// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The three things a media thread needs that `std` cannot do: socket
//! buffers of a set size, a wait for a datagram that loses none, and the
//! multimedia scheduling class.
//!
//! On Windows all three are Win32 calls. Elsewhere, where only the tests
//! run, the buffers stay as the system made them, the wait is the
//! socket's own read timeout, and the thread keeps its priority.
//!
//! # Waiting for a datagram
//!
//! `std` can wait for a datagram for a limited time only with
//! `set_read_timeout`, which on Windows is `SO_RCVTIMEO`. A receive that
//! runs into that timeout is cancelled, and a datagram that arrives at
//! that instant is lost with it: Winsock says of a socket whose receive
//! timed out that its state is indeterminate. This was measured here:
//! with a 10 ms timeout the first datagram to a waiting socket was lost
//! in 2 of 720 tries, and the transparency test of the phase lost the
//! first packet of a call about once in five runs. A media thread waits
//! every packet time, so that would be a steady loss of packets.
//! [`receive_within`] waits with `WSAPoll`, which only looks, and then
//! receives without waiting.
//!
//! The packets are marked with a DSCP by Windows, not here: on Windows an
//! application cannot set one on its own sockets. The setup program makes
//! a QoS policy that marks what the service sends by UDP as expedited
//! forwarding, 46. `docs/INSTALL.md` says when Windows applies it.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

/// The size asked for each of a call's socket buffers. A second of the
/// smallest packets, a hundred of 120 octets with their headers, is 12 kB;
/// this holds several seconds of the largest.
#[cfg(windows)]
pub const SOCKET_BUFFER: i32 = 256 * 1024;

/// Asks for receive and send buffers of 256 kB each. A system that gives
/// less gives what it gives: the call still works. It is done when the
/// socket is bound, before anything can be waiting in it.
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

/// Makes a socket ready for [`receive_within`] with waits of `wait`.
#[cfg(windows)]
pub fn prepare_socket(socket: &UdpSocket, _wait: Duration) -> io::Result<()> {
    // The wait is WSAPoll's; the receive after it never waits.
    socket.set_nonblocking(true)
}

/// Makes a socket ready for [`receive_within`] with waits of `wait`.
#[cfg(not(windows))]
pub fn prepare_socket(socket: &UdpSocket, wait: Duration) -> io::Result<()> {
    socket.set_read_timeout(Some(wait))
}

/// Waits up to `wait` for a datagram and receives it. `None` when none
/// came, and when the system reports an error in place of one, as Windows
/// does for an ICMP answer to something that was sent. No datagram is
/// ever lost to the wait running out.
#[cfg(windows)]
pub fn receive_within(
    socket: &UdpSocket,
    buffer: &mut [u8],
    wait: Duration,
) -> Option<(usize, SocketAddr)> {
    use std::os::windows::io::AsRawSocket;

    use windows::Win32::Networking::WinSock::{POLLRDNORM, SOCKET, WSAPOLLFD, WSAPoll};

    let mut poll = WSAPOLLFD {
        fd: SOCKET(socket.as_raw_socket() as usize),
        events: POLLRDNORM,
        revents: Default::default(),
    };
    let millis = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: `poll` is one WSAPOLLFD that lives until the call returns,
    // and the count passed is one; its socket is the handle of `socket`,
    // which is borrowed for the whole call and so is open.
    let ready = unsafe { WSAPoll(&mut poll, 1, millis) };
    if ready <= 0 {
        return None;
    }
    // Readable, or in error: either way the receive returns at once.
    socket.recv_from(buffer).ok()
}

/// Waits up to `wait` for a datagram and receives it. `None` when none
/// came. The socket's read timeout, set by [`prepare_socket`], is the
/// wait.
#[cfg(not(windows))]
pub fn receive_within(
    socket: &UdpSocket,
    buffer: &mut [u8],
    _wait: Duration,
) -> Option<(usize, SocketAddr)> {
    socket.recv_from(buffer).ok()
}

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
