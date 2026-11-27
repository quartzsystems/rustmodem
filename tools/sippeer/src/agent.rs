// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The peer on real sockets: a [`Peer`] and a [`MediaStream`] over UDP.
//!
//! An [`Agent`] holds one call at a time. It owns a SIP socket and an RTP
//! socket, neither of which blocks, and does its work in [`Agent::step`]:
//! whoever drives it calls that, or one of the waits built on it. The
//! media of a call begins by itself when the call is answered and ends
//! when it is hung up; everything else is asked for.

use std::collections::VecDeque;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use engine::{Law, Modem, ModemConfig, ModemEngine, NullConfig, NullEngine, Standard};
use sdp::{AudioPrefs, Codec, LocalEndpoint, Negotiated, SessionDescription};

use crate::media::{Impairments, MediaReport, MediaStream, Source};
use crate::peer::{Call, Peer, PeerEvent};

/// The longest an idle agent sleeps between looks at its sockets.
const IDLE: Duration = Duration::from_millis(1);

/// What a call of the agent's sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    /// Silence.
    #[default]
    Silence,
    /// A pattern, checked against what comes back: for a line with the
    /// echo engine.
    Pattern,
    /// These G.711 octets, then silence.
    File(Vec<u8>),
    /// A null engine: the far end of a line with the null engine.
    Null,
    /// The modem: the far end of a line with the modem engine. It may use
    /// these modulations, the first by choice, and V.8 or not; the rest
    /// of its settings are the modem's own defaults.
    Modem {
        modulations: [Option<Standard>; 2],
        v8: bool,
    },
}

/// How an agent is set up.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// The user name, in From and Contact.
    pub user: String,
    /// Where SIP is bound. The RTP socket is on the same address.
    pub listen: SocketAddrV4,
    /// The codecs offered and accepted, most preferred first.
    pub codecs: Vec<Codec>,
    pub ptime_ms: u32,
    pub media: MediaKind,
    pub impairments: Impairments,
    /// Makes tags, branches, faults and patterns.
    pub seed: u64,
}

impl AgentConfig {
    /// A peer `user` at `listen` that offers PCMU and PCMA at 20 ms,
    /// sends silence and has no faults.
    pub fn new(user: &str, listen: SocketAddrV4) -> AgentConfig {
        AgentConfig {
            user: user.to_owned(),
            listen,
            codecs: vec![Codec::Pcmu, Codec::Pcma],
            ptime_ms: 20,
            media: MediaKind::Silence,
            impairments: Impairments::default(),
            seed: 1,
        }
    }
}

/// A peer on sockets.
pub struct Agent {
    /// The user agent, for what the agent has no method for: the
    /// registrar's policy, challenges, requests made by hand.
    pub peer: Peer,
    config: AgentConfig,
    sip: UdpSocket,
    rtp: UdpSocket,
    sip_at: SocketAddrV4,
    rtp_at: SocketAddrV4,
    epoch: Instant,
    /// What happened and has not been looked at.
    backlog: VecDeque<PeerEvent>,
    /// The call, from its INVITE to its end.
    call: Option<Call>,
    /// The INVITE of theirs that waits for an answer: its body.
    offer: Option<Vec<u8>>,
    /// We made the call.
    originated: bool,
    stream: Option<(MediaStream, SocketAddrV4)>,
    /// The report of the last call's media, once it has ended.
    last: Option<MediaReport>,
    /// What the media agreed on.
    negotiated: Option<Negotiated>,
    calls_made: u64,
    buffer: Vec<u8>,
}

fn v4(address: SocketAddr) -> io::Result<SocketAddrV4> {
    match address {
        SocketAddr::V4(address) => Ok(address),
        SocketAddr::V6(_) => Err(io::Error::other("an IPv4 address is needed")),
    }
}

impl Agent {
    /// Binds the sockets. A port of 0 in `listen` takes any.
    pub fn bind(config: AgentConfig) -> io::Result<Agent> {
        let sip = UdpSocket::bind(config.listen)?;
        let rtp = UdpSocket::bind(SocketAddrV4::new(*config.listen.ip(), 0))?;
        sip.set_nonblocking(true)?;
        rtp.set_nonblocking(true)?;
        let sip_at = v4(sip.local_addr()?)?;
        let rtp_at = v4(rtp.local_addr()?)?;
        let peer = Peer::new(
            &config.user,
            &sip_at.ip().to_string(),
            sip_at.port(),
            config.seed,
        );
        Ok(Agent {
            peer,
            config,
            sip,
            rtp,
            sip_at,
            rtp_at,
            epoch: Instant::now(),
            backlog: VecDeque::new(),
            call: None,
            offer: None,
            originated: false,
            stream: None,
            last: None,
            negotiated: None,
            calls_made: 0,
            buffer: vec![0u8; 65_535],
        })
    }

    /// Where the agent's SIP is.
    pub fn sip_address(&self) -> SocketAddrV4 {
        self.sip_at
    }

    /// Where the agent's RTP is.
    pub fn rtp_address(&self) -> SocketAddrV4 {
        self.rtp_at
    }

    /// What the calls answered or made from now on send.
    pub fn set_media(&mut self, media: MediaKind) {
        self.config.media = media;
    }

    /// The call there is, if there is one.
    pub fn call(&self) -> Option<Call> {
        self.call
    }

    /// What the media of the call agreed on.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.negotiated
    }

    /// The media of the call there is, or of the last there was.
    pub fn report(&self) -> Option<MediaReport> {
        self.stream
            .as_ref()
            .map(|(stream, _)| stream.report())
            .or(self.last)
    }

    /// The engine of the call there is, when its media is one.
    pub fn engine(&mut self) -> Option<&mut (dyn ModemEngine + 'static)> {
        self.stream.as_mut().and_then(|(stream, _)| stream.engine())
    }

    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn prefs(&self) -> AudioPrefs {
        AudioPrefs {
            codecs: self.config.codecs.clone(),
            ptime_ms: self.config.ptime_ms,
        }
    }

    fn local(&self) -> LocalEndpoint {
        LocalEndpoint {
            address: *self.rtp_at.ip(),
            port: self.rtp_at.port(),
            session_id: self.config.seed.wrapping_add(self.calls_made),
            session_version: 1,
        }
    }

    fn start_media(&mut self, negotiated: Negotiated) {
        let source = match &self.config.media {
            MediaKind::Silence => Source::Silence,
            MediaKind::Pattern => Source::Pattern(self.config.seed),
            MediaKind::File(octets) => Source::File(octets.clone()),
            MediaKind::Null => {
                // The peer's clock, not the arrivals, ticks its engine,
                // so a burst of loss or a pause is many ticks of silence
                // to it. It is slow to call that the end of the carrier.
                let mut engine = NullEngine::new(NullConfig {
                    carrier_loss_ticks: 500,
                    ..NullConfig::default()
                });
                if self.originated {
                    engine.originate();
                } else {
                    engine.answer();
                }
                Source::Engine(Box::new(engine))
            }
            MediaKind::Modem { modulations, v8 } => {
                let mut engine = Modem::new(ModemConfig {
                    modulations: *modulations,
                    v8: *v8,
                    ..ModemConfig::default()
                });
                engine.set_law(match negotiated.codec {
                    Codec::Pcma => Law::Alaw,
                    Codec::Pcmu => Law::Ulaw,
                });
                if self.originated {
                    engine.originate();
                } else {
                    engine.answer();
                }
                Source::Engine(Box::new(engine))
            }
        };
        let stream = MediaStream::new(
            self.now_us(),
            negotiated.codec,
            negotiated.ptime_ms,
            self.config.seed.wrapping_add(self.calls_made),
            source,
            self.config.impairments,
        );
        self.negotiated = Some(negotiated);
        self.stream = Some((stream, negotiated.remote));
    }

    fn stop_media(&mut self) {
        if let Some((stream, _)) = self.stream.take() {
            self.last = Some(stream.report());
        }
    }

    fn end_call(&mut self) {
        self.stop_media();
        self.call = None;
        self.offer = None;
    }

    /// What an event means for the call and its media.
    fn follow(&mut self, event: &PeerEvent) {
        match event {
            PeerEvent::Incoming { call, request } if self.call.is_none() => {
                self.call = Some(*call);
                self.originated = false;
                self.offer = Some(request.body.clone());
            }
            PeerEvent::Answered { call, response } if self.call == Some(*call) => {
                let agreed = SessionDescription::parse(&response.body)
                    .ok()
                    .and_then(|answer| sdp::accept_answer(&self.prefs(), &answer).ok());
                if let Some(negotiated) = agreed {
                    self.start_media(negotiated);
                }
            }
            PeerEvent::Refused { call, .. }
            | PeerEvent::Cancelled { call }
            | PeerEvent::HungUp { call }
            | PeerEvent::NotAcknowledged { call }
                if self.call == Some(*call) =>
            {
                self.end_call();
            }
            PeerEvent::TimedOut { call, .. } if self.call == Some(*call) => self.end_call(),
            _ => {}
        }
    }

    /// One look at everything: what arrived is taken, what is due is
    /// sent. Never waits. True when something arrived or was sent.
    pub fn step(&mut self) -> io::Result<bool> {
        let mut busy = false;
        loop {
            match self.sip.recv_from(&mut self.buffer) {
                Ok((len, from)) => {
                    busy = true;
                    let now = self.now_ms();
                    self.peer.receive(now, from, &self.buffer[..len]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                // An ICMP refusal of something sent earlier (Windows
                // reports it on the next read) is not an arrival.
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
                Err(error) => return Err(error),
            }
        }
        let now = self.now_ms();
        self.peer.poll(now);
        for event in self.peer.events() {
            self.follow(&event);
            self.backlog.push_back(event);
        }
        for (to, datagram) in self.peer.take_sent() {
            busy = true;
            self.sip.send_to(&datagram, to)?;
        }
        loop {
            match self.rtp.recv_from(&mut self.buffer) {
                Ok((len, _)) => {
                    busy = true;
                    let now = self.now_us();
                    if let Some((stream, _)) = &mut self.stream {
                        stream.receive(now, &self.buffer[..len]);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
                Err(error) => return Err(error),
            }
        }
        let now = self.now_us();
        if let Some((stream, to)) = &mut self.stream {
            for datagram in stream.poll(now) {
                busy = true;
                // A far end that has gone is found out by SIP, not here.
                let _ = self.rtp.send_to(&datagram, *to);
            }
        }
        Ok(busy)
    }

    /// Sleeps until there may be something to do.
    fn idle(&self) {
        let mut sleep = IDLE;
        if let Some((stream, _)) = &self.stream {
            let until = stream.next_due_us().saturating_sub(self.now_us());
            sleep = sleep.min(Duration::from_micros(until));
        }
        if !sleep.is_zero() {
            std::thread::sleep(sleep);
        }
    }

    /// Runs for this long.
    pub fn run(&mut self, time: Duration) -> io::Result<()> {
        let until = Instant::now() + time;
        while Instant::now() < until {
            if !self.step()? {
                self.idle();
            }
        }
        Ok(())
    }

    /// Runs until an event that `wanted` takes has happened, and gives
    /// it; events before it are dropped. `None` when the time runs out.
    pub fn wait(
        &mut self,
        time: Duration,
        mut wanted: impl FnMut(&PeerEvent) -> bool,
    ) -> io::Result<Option<PeerEvent>> {
        let until = Instant::now() + time;
        loop {
            while let Some(event) = self.backlog.pop_front() {
                if wanted(&event) {
                    return Ok(Some(event));
                }
            }
            if Instant::now() >= until {
                return Ok(None);
            }
            if !self.step()? {
                self.idle();
            }
        }
    }

    /// Runs until `done` says so. False when the time runs out first.
    pub fn until(
        &mut self,
        time: Duration,
        mut done: impl FnMut(&mut Agent) -> bool,
    ) -> io::Result<bool> {
        let until = Instant::now() + time;
        loop {
            if done(self) {
                return Ok(true);
            }
            if Instant::now() >= until {
                return Ok(false);
            }
            if !self.step()? {
                self.idle();
            }
        }
    }

    /// The events that have happened and have not been looked at.
    pub fn events(&mut self) -> Vec<PeerEvent> {
        self.backlog.drain(..).collect()
    }

    /// Calls `uri` at `to` with an offer of the agent's codecs.
    pub fn invite(&mut self, uri: &str, to: SocketAddrV4) -> Call {
        self.calls_made += 1;
        let offer = sdp::offer(&self.prefs(), &self.local());
        let now = self.now_ms();
        let call = self.peer.invite(now, SocketAddr::V4(to), uri, Some(&offer));
        self.call = Some(call);
        self.originated = true;
        self.offer = None;
        call
    }

    /// Sends 180 to their INVITE.
    pub fn ring(&mut self) {
        if let Some(call) = self.call {
            let now = self.now_ms();
            self.peer.ring(now, call, 180);
        }
    }

    /// Answers their INVITE and begins the media. False when there is no
    /// INVITE to answer or its offer cannot be used, in which case it is
    /// refused with 488.
    pub fn answer(&mut self) -> bool {
        let (Some(call), Some(body)) = (self.call, self.offer.take()) else {
            return false;
        };
        self.calls_made += 1;
        let now = self.now_ms();
        let answered = SessionDescription::parse(&body)
            .ok()
            .and_then(|offer| sdp::answer(&self.prefs(), &self.local(), &offer).ok());
        match answered {
            Some((answer, negotiated)) => {
                self.peer.answer(now, call, Some(&answer));
                self.start_media(negotiated);
                true
            }
            None => {
                self.peer.reject(now, call, 488);
                self.end_call();
                false
            }
        }
    }

    /// Refuses their INVITE with this status.
    pub fn reject(&mut self, status: u16) {
        if let Some(call) = self.call {
            let now = self.now_ms();
            self.peer.reject(now, call, status);
            self.end_call();
        }
    }

    /// Cancels the INVITE of ours that has not been answered. The call
    /// ends when its 487 comes.
    pub fn cancel(&mut self) -> bool {
        let now = self.now_ms();
        self.call.is_some_and(|call| self.peer.cancel(now, call))
    }

    /// Hangs up the call there is. Its media ends at once.
    pub fn hang_up(&mut self) {
        if let Some(call) = self.call {
            let now = self.now_ms();
            self.peer.bye(now, call);
            self.end_call();
        }
    }
}

/// Loopback, on any port.
pub fn loopback() -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)
}
