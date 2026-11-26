// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The router's way to the network.
//!
//! The router itself is sans-I/O: it is given datagrams and gives
//! datagrams back. A [`SipTransport`] is where they come from and go to.
//! The real one is the service's, which has the socket; the tests of the
//! router give it a network in memory. Nothing in this crate opens a
//! socket.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

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
