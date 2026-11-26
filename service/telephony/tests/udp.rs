// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The router on a real SIP socket: its reading thread, the queue, the
//! waker, and a peer over UDP on the loopback interface.

mod common;

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{offer, statuses};
use config::ConfigFile;
use ctlproto::LineState;
use pool::{CallRouter, LineDirectory, LineId, RouterEvent};
use sippeer::Peer;
use telephony::{
    RouterParts, Settings, SipRouter, SipTransport, TelephonyHandle, UdpTransport, Unobserved,
    Waker, system_random,
};

struct Lines(BTreeMap<LineId, LineState>);

impl LineDirectory for Lines {
    fn state(&self, line: LineId) -> Option<LineState> {
        self.0.get(&line).copied()
    }
}

/// A router on a loopback socket and a peer on another, with the wall
/// clock.
struct Loopback {
    router: SipRouter,
    router_at: SocketAddr,
    peer: Peer,
    socket: UdpSocket,
    lines: Lines,
    started: Instant,
    wakes: Arc<AtomicUsize>,
    said: Vec<String>,
}

impl Loopback {
    fn new() -> Loopback {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let peer_port = socket.local_addr().unwrap().port();

        let wakes = Arc::new(AtomicUsize::new(0));
        let waker: Waker = {
            let wakes = Arc::clone(&wakes);
            Arc::new(move || {
                wakes.fetch_add(1, Ordering::Relaxed);
            })
        };
        let transport =
            UdpTransport::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), waker).unwrap();
        let router_at = SocketAddr::V4(transport.local());

        let text = common::CONFIG
            .replace("PORTS", &common::rtp_ports())
            .replace("[\"10.10.20.11\"]", "[\"127.0.0.1\"]")
            .replace(
                "register = { server = \"pbx.example.lan\", user = \"7003\", password = \"secret\" }",
                "",
            );
        let file = ConfigFile::parse(&text).unwrap();
        let mut settings = Settings::new(file.config(), "0.0.3");
        settings.listen = transport.local();
        let lines = Lines(
            file.config()
                .lines
                .iter()
                .map(|line| (line.id, LineState::Idle))
                .collect(),
        );
        let router = SipRouter::new(RouterParts {
            settings,
            transport: Box::new(transport),
            random: system_random(),
            observer: Box::new(Unobserved),
            handle: TelephonyHandle::new(),
            version: "0.0.3".to_string(),
        });
        Loopback {
            router,
            router_at,
            peer: Peer::new("201", "127.0.0.1", peer_port, 7),
            socket,
            lines,
            started: Instant::now(),
            wakes,
            said: Vec::new(),
        }
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Runs both ends for a while, or until `done` says so.
    fn run(&mut self, at_most: Duration, mut done: impl FnMut(&mut Loopback) -> bool) -> bool {
        let deadline = Instant::now() + at_most;
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline {
            let now = self.now();
            self.peer.poll(now);
            for (to, datagram) in self.peer.take_sent() {
                self.socket.send_to(&datagram, to).unwrap();
            }
            if let Ok((len, from)) = self.socket.recv_from(&mut buffer) {
                self.peer.receive(now, from, &buffer[..len]);
            }
            while let Some(event) = self.router.poll(now, &self.lines) {
                self.said.push(match event {
                    RouterEvent::Incoming { line, .. } => format!("incoming {line}"),
                    RouterEvent::Ring(line) => format!("ring {line}"),
                    RouterEvent::RingEnd(line) => format!("ring end {line}"),
                    RouterEvent::Connected { line, .. } => format!("connected {line}"),
                    RouterEvent::Cleared(line) => format!("cleared {line}"),
                    other => format!("{other:?}"),
                });
            }
            if done(self) {
                return true;
            }
        }
        false
    }
}

#[test]
fn a_call_over_a_real_socket() {
    let mut net = Loopback::new();
    let uri = format!("sip:5551000@{}", net.router_at);
    let call = net
        .peer
        .invite(net.now(), net.router_at, &uri, Some(&offer(7000)));
    assert!(
        net.run(Duration::from_secs(5), |net| net.said.len() == 2),
        "{:?}",
        net.said
    );
    assert_eq!(net.said, ["incoming 1", "ring 1"]);
    // A moment more, for what the router sent to reach the peer.
    net.run(Duration::from_millis(200), |_| false);
    // The reading thread woke the pool for what arrived.
    assert!(net.wakes.load(Ordering::Relaxed) >= 1);
    let events = net.peer.events();
    assert_eq!(statuses(&events), ["progress 100", "progress 180"]);
    // With no address to advertise, the Contact is the address the system
    // routes to the peer from, and the port of the listener.
    let sippeer::PeerEvent::Progress { response, .. } = &events[1] else {
        panic!("not a response");
    };
    assert_eq!(
        response.headers.get("Contact"),
        Some(format!("<sip:5551000@{}>", net.router_at).as_str())
    );

    net.router.answer(net.now(), 1);
    assert!(net.run(Duration::from_secs(5), |net| net.said.len() == 4));
    assert_eq!(net.said[2..], ["ring end 1", "connected 1"]);
    net.run(Duration::from_millis(200), |_| false);
    assert_eq!(statuses(&net.peer.events()), ["answered 200"]);

    net.peer.bye(net.now(), call);
    assert!(net.run(Duration::from_secs(5), |net| net.said.len() == 5));
    assert_eq!(net.said[4], "cleared 1");
    assert!(net.run(Duration::from_secs(5), |net| !net.router.busy()));
}

#[test]
fn the_transport_by_itself() {
    let (waker, wakes): (Waker, _) = {
        let wakes = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&wakes);
        (
            Arc::new(move || {
                seen.fetch_add(1, Ordering::Relaxed);
            }),
            wakes,
        )
    };
    let mut transport =
        UdpTransport::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), waker).unwrap();
    let at = transport.local();
    assert_eq!(transport.local_port(), at.port());
    assert_eq!(transport.receive(), None);

    // What arrives is queued, with where it came from, and the pool is
    // woken once for each.
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    peer.send_to(b"one", at).unwrap();
    peer.send_to(b"two", at).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while wakes.load(Ordering::Relaxed) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let from = peer.local_addr().unwrap();
    assert_eq!(transport.receive(), Some((from, b"one".to_vec())));
    assert_eq!(transport.receive(), Some((from, b"two".to_vec())));
    assert_eq!(transport.receive(), None);

    // What is sent arrives.
    transport.send(from, b"back");
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buffer = [0u8; 16];
    let (len, source) = peer.recv_from(&mut buffer).unwrap();
    assert_eq!((&buffer[..len], source), (&b"back"[..], SocketAddr::V4(at)));

    // A listener on one address sends from that address; a name that is
    // an address needs no looking up, and one that is nothing resolves to
    // nothing.
    let loopback = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5060);
    assert_eq!(transport.local_address(loopback), Ipv4Addr::LOCALHOST);
    assert_eq!(
        transport.resolve("10.10.20.11", 5062),
        Some("10.10.20.11:5062".parse().unwrap())
    );
    assert_eq!(transport.resolve("no-such-host.invalid", 5060), None);
    assert_eq!(
        transport.resolve("localhost", 5060),
        Some("127.0.0.1:5060".parse().unwrap())
    );

    // Any address: the address is the one the system routes from.
    let any = UdpTransport::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0), Arc::new(|| {}));
    let mut any = any.unwrap();
    assert_eq!(any.local_address(loopback), Ipv4Addr::LOCALHOST);
    // Dropping the transport ends its thread.
    drop(any);
    drop(transport);
}
