// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The router's way to the network.
//!
//! The router itself is sans-I/O: it is given datagrams and gives
//! datagrams back. A [`SipTransport`] is where they come from and go to.
//! [`UdpTransport`] is the real one: a thread owns the reading of the SIP
//! socket and passes what arrives through a queue, waking the pool for
//! each datagram.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Wakes the pool's thread: a datagram has arrived.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The datagrams that may wait for the router. More than that are dropped:
/// a sender that is not answered sends again.
const QUEUE: usize = 1024;

/// How long the reading thread waits for a datagram before it looks at
/// whether it is to stop.
const WAIT: Duration = Duration::from_millis(250);

/// How long a name stays resolved.
const RESOLVE_FOR: Duration = Duration::from_secs(300);

/// Where the router's datagrams come from and go to.
pub trait SipTransport: Send {
    /// The next datagram that has arrived, if there is one. Never blocks.
    fn receive(&mut self) -> Option<(SocketAddr, Vec<u8>)>;

    /// Sends a datagram.
    fn send(&mut self, to: SocketAddr, datagram: &[u8]);

    /// The port the listener is on.
    fn local_port(&self) -> u16;

    /// The local address the system would send to `peer` from.
    fn local_address(&mut self, peer: SocketAddrV4) -> Ipv4Addr;

    /// The IPv4 address of a host, which is a name or an address. `None`
    /// when a name does not resolve.
    fn resolve(&mut self, host: &str, port: u16) -> Option<SocketAddrV4>;
}

/// The SIP listener on a UDP socket.
pub struct UdpTransport {
    socket: UdpSocket,
    local: SocketAddrV4,
    inbox: Receiver<(SocketAddr, Vec<u8>)>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    routes: HashMap<Ipv4Addr, Ipv4Addr>,
    names: HashMap<String, (Instant, Option<Ipv4Addr>)>,
}

impl UdpTransport {
    /// Binds the listener and starts its reading thread. `waker` is called
    /// from that thread for every datagram that arrives.
    pub fn bind(listen: SocketAddrV4, waker: Waker) -> io::Result<UdpTransport> {
        let socket = UdpSocket::bind(listen)?;
        let SocketAddr::V4(local) = socket.local_addr()? else {
            return Err(io::Error::other("the SIP socket is not IPv4"));
        };
        let reading = socket.try_clone()?;
        // The wait ends now and then so that the thread sees it is to
        // stop. It is not the socket's read timeout: on Windows a datagram
        // that arrives as that runs out is lost.
        media::prepare_socket(&reading, WAIT)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (queue, inbox) = sync_channel(QUEUE);
        let reader = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("sip".to_string())
                .spawn(move || read(&reading, &queue, &stop, &waker))?
        };
        Ok(UdpTransport {
            socket,
            local,
            inbox,
            stop,
            reader: Some(reader),
            routes: HashMap::new(),
            names: HashMap::new(),
        })
    }

    /// The address the listener is bound to.
    pub fn local(&self) -> SocketAddrV4 {
        self.local
    }
}

/// The reading thread: every datagram goes into the queue, and the pool
/// is woken.
fn read(
    socket: &UdpSocket,
    queue: &SyncSender<(SocketAddr, Vec<u8>)>,
    stop: &AtomicBool,
    waker: &Waker,
) {
    let mut buffer = vec![0u8; 65_536];
    while !stop.load(Ordering::Relaxed) {
        // An error is the wait running out, or the system passing on an
        // ICMP error for something we sent: neither is a datagram.
        let Some((len, from)) = media::receive_within(socket, &mut buffer, WAIT) else {
            continue;
        };
        match queue.try_send((from, buffer[..len].to_vec())) {
            Ok(()) => waker(),
            // The router is behind: the datagram is dropped.
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => return,
        }
    }
}

impl SipTransport for UdpTransport {
    fn receive(&mut self) -> Option<(SocketAddr, Vec<u8>)> {
        self.inbox.try_recv().ok()
    }

    fn send(&mut self, to: SocketAddr, datagram: &[u8]) {
        // A datagram that cannot be sent is a datagram lost: the
        // transaction sends it again.
        let _ = self.socket.send_to(datagram, to);
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
        let routed = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
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

impl Drop for UdpTransport {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
