// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The SIP socket.
//!
//! The router is sans-I/O: it is given datagrams and gives datagrams back,
//! through a `telephony::SipTransport`. [`SipSocket`] is the real one. A
//! task owns the reading of a `tokio::net::UdpSocket` and passes each
//! datagram, with where it came from, to the pool's task through a bounded
//! channel, waking it; the router, on the pool's task, takes them from the
//! channel and sends on the same socket directly.
//!
//! A channel that is full drops the datagram and counts it: a sender that
//! is not answered sends again. A datagram that cannot be sent at once is
//! a datagram lost, and its transaction sends it again.
//!
//! SIP over TCP or TLS is one more transport here, when there is one.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use telephony::SipTransport;
use tokio::net::UdpSocket;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// Wakes the pool's task: a datagram has arrived.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The datagrams that may wait for the router. More than that are dropped:
/// a sender that is not answered sends again.
pub const QUEUE: usize = 1024;

/// The largest datagram there can be.
const MAX_DATAGRAM: usize = 65_536;

/// How long a name stays resolved.
const RESOLVE_FOR: Duration = Duration::from_secs(300);

/// The SIP listener on a UDP socket.
pub struct SipSocket {
    socket: Arc<UdpSocket>,
    local: SocketAddrV4,
    inbox: mpsc::Receiver<(SocketAddr, Vec<u8>)>,
    dropped: Arc<AtomicU64>,
    routes: HashMap<Ipv4Addr, Ipv4Addr>,
    names: HashMap<String, (Instant, Option<Ipv4Addr>)>,
}

impl SipSocket {
    /// Binds the listener and starts the task that reads it. `waker` is
    /// called from that task for every datagram that is queued.
    ///
    /// This must be called on the runtime the task is to run on. The task
    /// ends when the socket is dropped, and not before: a service that is
    /// stopping still hears the answers to what it sends to say so.
    pub fn bind(listen: SocketAddrV4, waker: Waker) -> io::Result<SipSocket> {
        let runtime = Handle::try_current().map_err(io::Error::other)?;
        let socket = std::net::UdpSocket::bind(listen)?;
        let SocketAddr::V4(local) = socket.local_addr()? else {
            return Err(io::Error::other("the SIP socket is not IPv4"));
        };
        socket.set_nonblocking(true)?;
        let socket = {
            // The socket is registered with the runtime's I/O driver.
            let _on_runtime = runtime.enter();
            Arc::new(UdpSocket::from_std(socket)?)
        };
        let (queue, inbox) = mpsc::channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        runtime.spawn(read(
            Arc::clone(&socket),
            queue,
            Arc::clone(&dropped),
            waker,
        ));
        Ok(SipSocket {
            socket,
            local,
            inbox,
            dropped,
            routes: HashMap::new(),
            names: HashMap::new(),
        })
    }

    /// The address the listener is bound to.
    pub fn local(&self) -> SocketAddrV4 {
        self.local
    }

    /// How many datagrams have been dropped because the router was
    /// behind. The count can be read after the socket has gone to the
    /// router.
    pub fn dropped(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.dropped)
    }
}

/// The reading task: every datagram goes into the queue, and the pool is
/// woken.
async fn read(
    socket: Arc<UdpSocket>,
    queue: mpsc::Sender<(SocketAddr, Vec<u8>)>,
    dropped: Arc<AtomicU64>,
    waker: Waker,
) {
    let mut buffer = vec![0u8; MAX_DATAGRAM];
    loop {
        let received = tokio::select! {
            received = socket.recv_from(&mut buffer) => received,
            // The router has let go of the socket.
            () = queue.closed() => return,
        };
        // An error is the system passing on an ICMP error for something
        // we sent: it is not a datagram.
        let Ok((len, from)) = received else {
            tokio::task::yield_now().await;
            continue;
        };
        match queue.try_send((from, buffer[..len].to_vec())) {
            Ok(()) => waker(),
            // The router is behind: the datagram is dropped.
            Err(TrySendError::Full(_)) => {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Closed(_)) => return,
        }
    }
}

impl SipTransport for SipSocket {
    fn receive(&mut self) -> Option<(SocketAddr, Vec<u8>)> {
        self.inbox.try_recv().ok()
    }

    fn send(&mut self, to: SocketAddr, datagram: &[u8]) {
        // A datagram that cannot be sent is a datagram lost: the
        // transaction sends it again.
        let _ = self.socket.try_send_to(datagram, to);
    }

    fn local_port(&self) -> u16 {
        self.local.port()
    }

    fn local_address(&mut self, peer: SocketAddrV4) -> Ipv4Addr {
        // A listener on one address sends from that address.
        if !self.local.ip().is_unspecified() {
            return *self.local.ip();
        }
        if let Some(address) = self.routes.get(peer.ip()) {
            return *address;
        }
        // Connecting a UDP socket sends nothing: it only has the system
        // choose the route, and with it the address to send from.
        let routed = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .and_then(|socket| {
                socket.connect(peer)?;
                socket.local_addr()
            })
            .ok()
            .and_then(|address| match address {
                SocketAddr::V4(address) => Some(*address.ip()),
                SocketAddr::V6(_) => None,
            })
            .unwrap_or(Ipv4Addr::LOCALHOST);
        self.routes.insert(*peer.ip(), routed);
        routed
    }

    fn resolve(&mut self, host: &str, port: u16) -> Option<SocketAddrV4> {
        if let Ok(address) = host.parse::<Ipv4Addr>() {
            return Some(SocketAddrV4::new(address, port));
        }
        let now = Instant::now();
        if let Some((at, address)) = self.names.get(host)
            && now.duration_since(*at) < RESOLVE_FOR
        {
            return address.map(|address| SocketAddrV4::new(address, port));
        }
        // The pool's task waits for the resolver, as its thread did.
        let address = (host, port).to_socket_addrs().ok().and_then(|addresses| {
            addresses.into_iter().find_map(|address| match address {
                SocketAddr::V4(address) => Some(*address.ip()),
                SocketAddr::V6(_) => None,
            })
        });
        self.names.insert(host.to_owned(), (now, address));
        address.map(|address| SocketAddrV4::new(address, port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_router_that_is_behind_loses_datagrams_and_they_are_counted() {
        let runtime = crate::runtime::build().unwrap();
        let mut socket = {
            let _on_runtime = runtime.enter();
            SipSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), Arc::new(|| {})).unwrap()
        };
        let dropped = socket.dropped();
        let peer = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        // More than the queue holds, a little at a time so that the
        // socket's own buffer is not what loses them.
        const SENT: usize = QUEUE + 200;
        for number in 0..SENT {
            peer.send_to(&(number as u32).to_be_bytes(), socket.local())
                .unwrap();
            if number % 64 == 63 {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while (dropped.load(Ordering::Relaxed) as usize) < SENT - QUEUE {
            assert!(
                Instant::now() < deadline,
                "{} datagrams were counted as dropped",
                dropped.load(Ordering::Relaxed)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(dropped.load(Ordering::Relaxed) as usize, SENT - QUEUE);

        // What was queued is the first of them, in the order they came.
        for number in 0..QUEUE {
            let (_, datagram) = socket.receive().expect("a queued datagram");
            assert_eq!(datagram, (number as u32).to_be_bytes());
        }
        assert_eq!(socket.receive(), None);
    }
}
