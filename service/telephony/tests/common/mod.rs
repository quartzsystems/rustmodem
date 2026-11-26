// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The router with two scripted peers, a gateway and a PBX, joined by a
//! network in memory, and a clock the test moves.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};

use config::ConfigFile;
use ctlproto::LineState;
use media::RtpEndpoint;
use pool::{CallRouter, LineDirectory, LineId, RouterEvent};
use sdp::{AudioPrefs, Codec, LocalEndpoint, SessionDescription};
use sip::Message;
use sippeer::{Peer, PeerEvent};
use telephony::{
    Direction, Observer, RegistrationStatus, RouterParts, Settings, SipRouter, SipTransport,
    TelephonyHandle,
};

/// Where the router is, as its peers see it.
pub const ROUTER: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 5), 5060));
/// The gateway: a trusted peer, and the outbound peer.
pub const GATEWAY: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 11), 5060));
/// The PBX: the registrar of the lines that register. Not a trusted peer.
pub const PBX: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 10, 20, 1), 5060));
/// Somebody else.
pub const STRANGER: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 9), 5060));

/// The configuration most tests use: three lines, the third registered to
/// the PBX, a hunt group of the first two, and a route to the third.
pub const CONFIG: &str = r#"
[sip]
enabled = true
listen = "127.0.0.1:5060"
trusted_peers = ["10.10.20.11"]
outbound_peer = "10.10.20.11"
ring_timeout_s = 60

[media]
codecs = ["pcmu", "pcma"]
ptime_ms = 20
rtp_ports = "PORTS"
rtp_timeout_s = 10

[[line]]
id = 1
serial = { backend = "com0com", port = "CNCB0" }

[[line]]
id = 2
serial = { backend = "com0com", port = "CNCB1" }

[[line]]
id = 3
serial = { backend = "com0com", port = "CNCB2" }
register = { server = "pbx.example.lan", user = "7003", password = "secret" }

[[hunt_group]]
name = "dialin"
pilot = "5551000"
lines = [1, 2]

[[route]]
called = "5552*"
target = "line:2"
"#;

/// A stretch of ports for the calls of this test program: the tests of
/// one program run side by side, and the programs do too.
pub fn rtp_ports() -> String {
    let first = 32_000 + (std::process::id() % 100) as u16 * 200;
    format!("{first}-{}", first + 199)
}

type Queue = Arc<Mutex<VecDeque<(SocketAddr, Vec<u8>)>>>;

struct MemoryTransport {
    inbox: Queue,
    sent: Queue,
}

impl SipTransport for MemoryTransport {
    fn receive(&mut self) -> Option<(SocketAddr, Vec<u8>)> {
        self.inbox.lock().unwrap().pop_front()
    }

    fn send(&mut self, to: SocketAddr, datagram: &[u8]) {
        self.sent.lock().unwrap().push_back((to, datagram.to_vec()));
    }

    fn local_port(&self) -> u16 {
        5060
    }

    fn local_address(&mut self, _peer: SocketAddrV4) -> Ipv4Addr {
        Ipv4Addr::new(10, 10, 20, 5)
    }

    fn resolve(&mut self, host: &str, port: u16) -> Option<SocketAddrV4> {
        let address = match host {
            "pbx.example.lan" => Ipv4Addr::new(10, 10, 20, 1),
            "gw.example.lan" => Ipv4Addr::new(10, 10, 20, 11),
            other => other.parse().ok()?,
        };
        Some(SocketAddrV4::new(address, port))
    }
}

/// What the router told its observer.
#[derive(Default)]
pub struct Seen {
    pub registrations: Vec<RegistrationStatus>,
    /// The ids of the messages logged, with the line they were about.
    pub logs: Vec<(Option<LineId>, String)>,
    /// The first line of every message traced, with its direction and
    /// the other end.
    pub trace: Vec<(Direction, SocketAddr, String)>,
    /// Everything traced, whole.
    pub wire: String,
}

struct Recorder(Arc<Mutex<Seen>>);

impl Observer for Recorder {
    fn registration(&mut self, status: &RegistrationStatus) {
        self.0.lock().unwrap().registrations.push(status.clone());
    }

    fn trace(&mut self, direction: Direction, peer: SocketAddr, text: &str) {
        let mut seen = self.0.lock().unwrap();
        let first = text.lines().next().unwrap_or_default().to_owned();
        seen.trace.push((direction, peer, first));
        seen.wire.push_str(text);
    }

    fn log(&mut self, line: Option<LineId>, message: &msg::Message) {
        self.0.lock().unwrap().logs.push((line, message.id.clone()));
    }
}

struct Lines<'a>(&'a BTreeMap<LineId, LineState>);

impl LineDirectory for Lines<'_> {
    fn state(&self, line: LineId) -> Option<LineState> {
        self.0.get(&line).copied()
    }
}

/// Decides what happens to a message on its way: whether it is lost.
pub type Filter = Box<dyn FnMut(&Message) -> bool>;

pub struct World {
    pub router: SipRouter,
    pub gateway: Peer,
    pub pbx: Peer,
    pub now: u64,
    /// The state of the lines, as the pool would give it.
    pub lines: BTreeMap<LineId, LineState>,
    /// What the router said to the lines, summarised.
    pub said: Vec<String>,
    /// The media endpoint of each call that connected.
    pub media: Vec<(LineId, RtpEndpoint)>,
    pub seen: Arc<Mutex<Seen>>,
    pub handle: TelephonyHandle,
    inbox: Queue,
    sent: Queue,
    /// Messages from the router for which this says true are lost.
    pub lose_from_router: Option<Filter>,
    /// Messages to the router for which this says true are lost.
    pub lose_to_router: Option<Filter>,
    /// Every message arrives twice, both ways.
    pub duplicate: bool,
    /// The ids of the messages logged before the test began: the lines
    /// registering.
    pub startup_logs: Vec<String>,
}

impl World {
    pub fn new() -> World {
        World::with_config(CONFIG)
    }

    pub fn with_config(text: &str) -> World {
        let mut world = World::unsettled(text);
        world.settle();
        // What was logged while the lines registered is kept apart, so
        // that a test sees what its own doing logged.
        world.startup_logs = world.logged();
        world
    }

    /// A world in which nothing has happened yet: the router has not had
    /// its first turn, so a test can say first how the peers behave.
    pub fn unsettled(text: &str) -> World {
        let text = text.replace("PORTS", &rtp_ports());
        let file = ConfigFile::parse(&text).unwrap_or_else(|problems| panic!("{problems:?}"));
        let inbox = Queue::default();
        let sent = Queue::default();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let handle = TelephonyHandle::new();
        let mut next = 0x1000u64;
        let router = SipRouter::new(RouterParts {
            settings: Settings::new(file.config(), "0.0.3"),
            transport: Box::new(MemoryTransport {
                inbox: Arc::clone(&inbox),
                sent: Arc::clone(&sent),
            }),
            random: Box::new(move || {
                next = next.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                next
            }),
            observer: Box::new(Recorder(Arc::clone(&seen))),
            handle: handle.clone(),
            version: "0.0.3".to_string(),
        });
        let lines = file
            .config()
            .lines
            .iter()
            .map(|line| (line.id, LineState::Idle))
            .collect();
        World {
            router,
            gateway: Peer::new("201", "10.10.20.11", 5060, 0xA),
            pbx: Peer::new("pbx", "10.10.20.1", 5060, 0xB),
            now: 1000,
            lines,
            said: Vec::new(),
            media: Vec::new(),
            seen,
            handle,
            inbox,
            sent,
            lose_from_router: None,
            lose_to_router: None,
            duplicate: false,
            startup_logs: Vec::new(),
        }
    }

    /// Gives the router another configuration, as the pool does when the
    /// file is read again or changed through the control API.
    pub fn configure(&mut self, text: &str) {
        let text = text.replace("PORTS", &rtp_ports());
        let file = ConfigFile::parse(&text).unwrap_or_else(|problems| panic!("{problems:?}"));
        self.router.configure(self.now, file.config());
        self.settle();
    }

    fn deliver_to_router(&mut self, from: SocketAddr, datagram: Vec<u8>) {
        if let (Some(lose), Ok(message)) = (&mut self.lose_to_router, Message::parse(&datagram))
            && lose(&message)
        {
            return;
        }
        let mut inbox = self.inbox.lock().unwrap();
        if self.duplicate {
            inbox.push_back((from, datagram.clone()));
        }
        inbox.push_back((from, datagram));
    }

    /// A datagram for the router from an address of the test's choosing.
    pub fn inject(&mut self, from: SocketAddr, datagram: &[u8]) {
        self.inbox
            .lock()
            .unwrap()
            .push_back((from, datagram.to_vec()));
    }

    /// Moves what is on the network and runs every end, at the present
    /// time, until nothing moves.
    pub fn settle(&mut self) {
        for _ in 0..200 {
            let mut moved = false;
            // The peers' timers first, so that what they send again at
            // this moment arrives at this moment.
            self.gateway.poll(self.now);
            self.pbx.poll(self.now);
            for (to, datagram) in self.gateway.take_sent() {
                moved = true;
                if to == ROUTER {
                    self.deliver_to_router(GATEWAY, datagram);
                }
            }
            for (to, datagram) in self.pbx.take_sent() {
                moved = true;
                if to == ROUTER {
                    self.deliver_to_router(PBX, datagram);
                }
            }
            while let Some(event) = self.router.poll(self.now, &Lines(&self.lines)) {
                moved = true;
                self.hear(event);
            }
            loop {
                let next = self.sent.lock().unwrap().pop_front();
                let Some((to, datagram)) = next else {
                    break;
                };
                moved = true;
                if let (Some(lose), Ok(message)) =
                    (&mut self.lose_from_router, Message::parse(&datagram))
                    && lose(&message)
                {
                    continue;
                }
                let times = if self.duplicate { 2 } else { 1 };
                for _ in 0..times {
                    if to == GATEWAY {
                        self.gateway.receive(self.now, ROUTER, &datagram);
                    } else if to == PBX {
                        self.pbx.receive(self.now, ROUTER, &datagram);
                    }
                }
            }
            if !moved {
                return;
            }
        }
        panic!("the network never went quiet");
    }

    /// Moves the clock on, ten milliseconds at a time.
    pub fn advance(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            self.now = (self.now + 10).min(end);
            self.settle();
        }
    }

    fn hear(&mut self, event: RouterEvent) {
        let said = match event {
            RouterEvent::Incoming { line, from } => format!("incoming {line} from {from}"),
            RouterEvent::Ring(line) => format!("ring {line}"),
            RouterEvent::RingEnd(line) => format!("ring end {line}"),
            RouterEvent::Abandoned(line) => format!("abandoned {line}"),
            RouterEvent::Connected {
                line,
                originate,
                media,
            } => {
                let endpoint = media
                    .downcast::<RtpEndpoint>()
                    .expect("the SIP router gives RTP endpoints");
                self.media.push((line, endpoint));
                format!(
                    "connected {line} {}",
                    if originate { "originate" } else { "answer" }
                )
            }
            RouterEvent::Busy(line) => format!("busy {line}"),
            RouterEvent::NoAnswer(line) => format!("no answer {line}"),
            RouterEvent::NoDialtone(line) => format!("no dialtone {line}"),
            RouterEvent::Failed(line) => format!("failed {line}"),
            RouterEvent::Cleared(line) => format!("cleared {line}"),
        };
        self.said.push(said);
    }

    /// What the router has said to the lines since the last look.
    pub fn said(&mut self) -> Vec<String> {
        std::mem::take(&mut self.said)
    }

    // The lines, as the pool would drive the router for them.

    pub fn answer(&mut self, line: LineId) {
        self.router.answer(self.now, line);
        self.settle();
    }

    pub fn hangup(&mut self, line: LineId) {
        self.router.hangup(self.now, line);
        self.settle();
    }

    pub fn dial(&mut self, line: LineId, number: &str) {
        self.router
            .dial(self.now, line, number, &Lines(&self.lines));
        self.settle();
    }

    /// The gateway calls a number with an ordinary offer.
    pub fn call(&mut self, number: &str) -> sippeer::Call {
        let uri = format!("sip:{number}@10.10.20.5");
        let call = self
            .gateway
            .invite(self.now, ROUTER, &uri, Some(&offer(16_482)));
        self.settle();
        call
    }

    /// The ids of the messages the router logged since the last look.
    pub fn logged(&mut self) -> Vec<String> {
        std::mem::take(&mut self.seen.lock().unwrap().logs)
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    }
}

/// An offer as a gateway makes it: PCMU and PCMA and telephone-event, 20
/// ms, from 10.10.20.11 at `port`.
pub fn offer(port: u16) -> SessionDescription {
    let text = format!(
        "v=0\r\n\
         o=- 16264 16264 IN IP4 10.10.20.11\r\n\
         s=-\r\n\
         c=IN IP4 10.10.20.11\r\n\
         t=0 0\r\n\
         m=audio {port} RTP/AVP 0 8 101\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=rtpmap:101 telephone-event/8000\r\n\
         a=ptime:20\r\n\
         a=sendrecv\r\n"
    );
    SessionDescription::parse(text.as_bytes()).unwrap()
}

/// A description with these lines after `t=`, from 10.10.20.11.
pub fn description(lines: &[&str]) -> SessionDescription {
    let mut text =
        "v=0\r\no=- 5 5 IN IP4 10.10.20.11\r\ns=-\r\nc=IN IP4 10.10.20.11\r\nt=0 0\r\n".to_owned();
    for line in lines {
        text.push_str(line);
        text.push_str("\r\n");
    }
    SessionDescription::parse(text.as_bytes()).unwrap()
}

/// An answer as a gateway makes it to an offer of ours: one codec.
pub fn answer_sdp(port: u16, codec: Codec) -> SessionDescription {
    sdp::offer(
        &AudioPrefs {
            codecs: vec![codec],
            ptime_ms: 20,
        },
        &LocalEndpoint {
            address: Ipv4Addr::new(10, 10, 20, 11),
            port,
            session_id: 9,
            session_version: 9,
        },
    )
}

/// The statuses of the responses in a peer's events, in order, with what
/// kind of event each was.
pub fn statuses(events: &[PeerEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            PeerEvent::Incoming { request, .. } => format!("incoming {}", request.uri),
            PeerEvent::Progress { response, .. } => format!("progress {}", response.status),
            PeerEvent::Answered { response, .. } => format!("answered {}", response.status),
            PeerEvent::Refused { response, .. } => format!("refused {}", response.status),
            PeerEvent::TimedOut { method, .. } => format!("timed out {method}"),
            PeerEvent::Acknowledged { .. } => "acknowledged".to_owned(),
            PeerEvent::NotAcknowledged { .. } => "not acknowledged".to_owned(),
            PeerEvent::Cancelled { .. } => "cancelled".to_owned(),
            PeerEvent::HungUp { .. } => "hung up".to_owned(),
            PeerEvent::InCall { request, .. } => format!("in call {}", request.method),
            PeerEvent::InCallAnswered {
                method, response, ..
            } => format!("{method} answered {}", response.status),
            PeerEvent::Registered { status, .. } => format!("registered {status}"),
            PeerEvent::Request { request, .. } => format!("request {}", request.method),
            PeerEvent::Response { response, .. } => format!("response {}", response.status),
        })
        .collect()
}

/// The session description a message carries.
pub fn body_sdp(body: &[u8]) -> SessionDescription {
    SessionDescription::parse(body).expect("the body is a session description")
}
