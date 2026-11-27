// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The media seam: the pool does not touch audio.
//!
//! When a call connects, the router hands the pool a [`MediaEndpoint`] for
//! it. The pool gives the endpoint and the line's engine to a
//! [`MediaRunner`] and gets back a [`MediaCall`]. The engine belongs to the
//! runner until the call ends, then returns to the line.
//!
//! ```text
//!   pool thread                         the runner
//!   -----------                         ----------
//!   MediaCall                           MediaPort + the engine
//!     write() ----- ring of bytes ----->  feed():  engine.dte_write
//!     read()  <---- ring of bytes ------  drain(): engine.dte_read
//!     poll_event() <-- ring of events --           engine.poll_event
//!     stats(), set_capture()  <- MediaShared (atomics) ->
//! ```
//!
//! Everything that crosses is lock-free: two rings of bytes, a ring of
//! events and a block of atomics. Nothing on the runner's side takes a
//! lock, allocates or makes a system call to use them.
//!
//! There are two runners. [`InlineRunner`] ticks its calls from the pool's
//! own clock, as the pool itself did before there was a seam; it serves
//! the null switch. The threaded runner of `service/media` ticks each call
//! from the arrival of its RTP packets, on a thread of its own.

use std::any::Any;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::{
    Diagnostics, EngineState, FRAME_20MS, ModemEngine, ModemEvent, SCOPE_SAMPLES, SILENCE_OCTET,
    STATE_HISTORY, ScopeKind, StateChange,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::router::LineId;

/// The bytes each ring of data holds.
pub const RING_BYTES: usize = 8192;

/// The events the ring of events holds. An engine reports a handful in a
/// call.
pub const EVENT_QUEUE: usize = 32;

/// How much data moves between a ring and an engine at a time.
const CHUNK: usize = 512;

/// The far end of a call's media, as a router hands it to the pool. The
/// pool does not look inside: a router and the runner it is used with
/// agree on what it is.
pub struct MediaEndpoint(Box<dyn Any + Send>);

impl MediaEndpoint {
    pub fn new<T: Any + Send>(endpoint: T) -> MediaEndpoint {
        MediaEndpoint(Box::new(endpoint))
    }

    /// The endpoint as a `T`, or the endpoint back if it is not one.
    pub fn downcast<T: Any>(self) -> Result<T, MediaEndpoint> {
        match self.0.downcast::<T>() {
            Ok(endpoint) => Ok(*endpoint),
            Err(other) => Err(MediaEndpoint(other)),
        }
    }
}

impl fmt::Debug for MediaEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MediaEndpoint")
    }
}

/// Something a runner tells the pool about a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaEvent {
    /// The engine reported something.
    Modem(ModemEvent),
    /// Nothing has arrived from the far end for the time allowed: the call
    /// is gone.
    Stalled,
}

/// The statistics of a call's media.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaStats {
    /// The RTP payload type: 0 for PCMU, 8 for PCMA.
    pub payload_type: u8,
    /// The packet time in milliseconds.
    pub ptime_ms: u32,
    /// Where the far end receives.
    pub remote: Option<SocketAddrV4>,
    pub rx_packets: u64,
    pub rx_octets: u64,
    pub tx_packets: u64,
    pub tx_octets: u64,
    /// Packets that never arrived in time.
    pub lost: u64,
    pub late: u64,
    pub duplicate: u64,
    pub resync: u64,
    /// Packets of another payload type.
    pub ignored: u64,
    /// Packets from an address that is not the far end's.
    pub stray: u64,
    /// Frames ticked from the local timer before the far end's first
    /// packet.
    pub free_run: u64,
    /// The interarrival jitter now, in samples.
    pub jitter: u32,
    /// The most the jitter has been, in samples.
    pub jitter_max: u32,
    /// The time a tick takes, in microseconds: the median.
    pub tick_p50_us: u32,
    /// The 99th percentile.
    pub tick_p99_us: u32,
    /// The longest.
    pub tick_max_us: u32,
    /// Ticks that took longer than the packet time.
    pub overruns: u64,
    /// Capture records dropped because the writer fell behind.
    pub capture_dropped: u64,
    /// Arrivals for which the frames ticked were not the frames the
    /// receiver said to deliver. There should never be one.
    pub slips: u64,
}

impl MediaStats {
    /// RTP octets both ways, the fixed headers with the payloads.
    pub fn wire_octets(&self) -> u64 {
        self.rx_octets + self.tx_octets + 12 * (self.rx_packets + self.tx_packets)
    }

    /// The statistics as the control API reports them.
    pub fn info(&self) -> ctlproto::MediaInfo {
        // The jitter is kept in samples of 125 microseconds.
        let micros = |samples: u32| samples.saturating_mul(125);
        ctlproto::MediaInfo {
            codec: if self.payload_type == 8 {
                "PCMA"
            } else {
                "PCMU"
            }
            .to_string(),
            ptime_ms: self.ptime_ms,
            remote: self.remote.map(|remote| remote.to_string()),
            rx_packets: self.rx_packets,
            rx_octets: self.rx_octets,
            tx_packets: self.tx_packets,
            tx_octets: self.tx_octets,
            lost: self.lost,
            late: self.late,
            duplicate: self.duplicate,
            resync: self.resync,
            ignored: self.ignored,
            free_run: self.free_run,
            slips: self.slips,
            jitter_us_avg: micros(self.jitter),
            jitter_us_max: micros(self.jitter_max),
            tick_p50_us: self.tick_p50_us,
            tick_p99_us: self.tick_p99_us,
            tick_max_us: self.tick_max_us,
            overruns: self.overruns,
        }
    }
}

/// What the pool and a runner share about a call, all of it atomic.
#[derive(Debug, Default)]
pub struct MediaShared {
    /// The pool wants the call's packets captured.
    capture: AtomicBool,
    /// The runner publishes statistics.
    has_stats: AtomicBool,
    stats: [AtomicU64; 21],
    /// The runner publishes the engine's snapshot of itself.
    has_diagnostics: AtomicBool,
    diagnostics: [AtomicU64; DIAGNOSTIC_WORDS],
}

/// The words a snapshot is carried in: four of numbers, one for each
/// change of state it remembers, and one for every eight samples of its
/// scope.
const DIAGNOSTIC_WORDS: usize = 4 + STATE_HISTORY + SCOPE_SAMPLES / 8;

impl MediaShared {
    /// Whether the pool wants the call's packets captured.
    pub fn capture(&self) -> bool {
        self.capture.load(Ordering::Relaxed)
    }

    /// Publishes the statistics. Each number is stored by itself, so a
    /// reader may see one tick's packets with the next tick's octets;
    /// nothing is torn.
    pub fn publish(&self, stats: &MediaStats) {
        let remote = stats.remote.map_or(0, |remote| {
            (1 << 48) | (u64::from(u32::from(*remote.ip())) << 16) | u64::from(remote.port())
        });
        let values = [
            u64::from(stats.payload_type),
            u64::from(stats.ptime_ms),
            remote,
            stats.rx_packets,
            stats.rx_octets,
            stats.tx_packets,
            stats.tx_octets,
            stats.lost,
            stats.late,
            stats.duplicate,
            stats.resync,
            stats.ignored,
            stats.stray,
            stats.free_run,
            (u64::from(stats.jitter_max) << 32) | u64::from(stats.jitter),
            u64::from(stats.tick_p50_us),
            u64::from(stats.tick_p99_us),
            u64::from(stats.tick_max_us),
            stats.overruns,
            stats.capture_dropped,
            stats.slips,
        ];
        for (slot, value) in self.stats.iter().zip(values) {
            slot.store(value, Ordering::Relaxed);
        }
        self.has_stats.store(true, Ordering::Relaxed);
    }

    /// Publishes the engine's snapshot of itself. Like the statistics,
    /// each word is stored by itself: a reader may see the levels of one
    /// tick with the scope of the next, and nothing is torn.
    pub fn publish_diagnostics(&self, snapshot: &Diagnostics) {
        let mut words = [0u64; DIAGNOSTIC_WORDS];
        words[0] = u64::from(snapshot.state as u8)
            | (u64::from(snapshot.carrier) << 8)
            | (u64::from(snapshot.changes) << 16)
            | (u64::from(snapshot.scope_kind as u8) << 24)
            | (u64::from(snapshot.framing_errors) << 32);
        words[1] = u64::from(snapshot.rx_level_dbm0.to_bits())
            | (u64::from(snapshot.tx_level_dbm0.to_bits()) << 32);
        words[2] = u64::from(snapshot.frequency_error_hz.to_bits())
            | (u64::from(snapshot.quality.to_bits()) << 32);
        words[3] = u64::from(snapshot.retrains);
        for (word, change) in words[4..].iter_mut().zip(&snapshot.history) {
            *word = u64::from(change.state as u8) | (u64::from(change.at_ms) << 8);
        }
        let scope = &mut words[4 + STATE_HISTORY..];
        for (word, samples) in scope.iter_mut().zip(snapshot.scope.chunks(8)) {
            for (n, sample) in samples.iter().enumerate() {
                *word |= u64::from(*sample as u8) << (8 * n);
            }
        }
        for (slot, word) in self.diagnostics.iter().zip(words) {
            slot.store(word, Ordering::Relaxed);
        }
        self.has_diagnostics.store(true, Ordering::Relaxed);
    }

    /// The snapshot last published, if the call's engine has one to
    /// show.
    pub fn diagnostics(&self) -> Option<Diagnostics> {
        if !self.has_diagnostics.load(Ordering::Relaxed) {
            return None;
        }
        let get = |index: usize| self.diagnostics[index].load(Ordering::Relaxed);
        let mut snapshot = Diagnostics {
            state: EngineState::from_number(get(0) as u8),
            carrier: get(0) >> 8 & 1 != 0,
            changes: ((get(0) >> 16) as u8).min(STATE_HISTORY as u8),
            scope_kind: if (get(0) >> 24) as u8 == ScopeKind::Discriminator as u8 {
                ScopeKind::Discriminator
            } else {
                ScopeKind::None
            },
            framing_errors: (get(0) >> 32) as u32,
            rx_level_dbm0: f32::from_bits(get(1) as u32),
            tx_level_dbm0: f32::from_bits((get(1) >> 32) as u32),
            frequency_error_hz: f32::from_bits(get(2) as u32),
            quality: f32::from_bits((get(2) >> 32) as u32),
            retrains: get(3) as u32,
            ..Diagnostics::default()
        };
        for (n, change) in snapshot.history.iter_mut().enumerate() {
            let word = get(4 + n);
            *change = StateChange {
                state: EngineState::from_number(word as u8),
                at_ms: (word >> 8) as u32,
            };
        }
        for (n, sample) in snapshot.scope.iter_mut().enumerate() {
            let word = get(4 + STATE_HISTORY + n / 8);
            *sample = (word >> (8 * (n % 8))) as u8 as i8;
        }
        Some(snapshot)
    }

    /// The statistics last published, if the runner publishes any.
    pub fn stats(&self) -> Option<MediaStats> {
        if !self.has_stats.load(Ordering::Relaxed) {
            return None;
        }
        let get = |index: usize| self.stats[index].load(Ordering::Relaxed);
        let remote = get(2);
        Some(MediaStats {
            payload_type: get(0) as u8,
            ptime_ms: get(1) as u32,
            remote: (remote >> 48 != 0)
                .then(|| SocketAddrV4::new(Ipv4Addr::from((remote >> 16) as u32), remote as u16)),
            rx_packets: get(3),
            rx_octets: get(4),
            tx_packets: get(5),
            tx_octets: get(6),
            lost: get(7),
            late: get(8),
            duplicate: get(9),
            resync: get(10),
            ignored: get(11),
            stray: get(12),
            free_run: get(13),
            jitter: get(14) as u32,
            jitter_max: (get(14) >> 32) as u32,
            tick_p50_us: get(15) as u32,
            tick_p99_us: get(16) as u32,
            tick_max_us: get(17) as u32,
            overruns: get(18),
            capture_dropped: get(19),
            slips: get(20),
        })
    }
}

/// The pool's end of a call's media.
pub struct MediaCall {
    to_line: Producer<u8>,
    from_line: Consumer<u8>,
    events: Consumer<MediaEvent>,
    shared: Arc<MediaShared>,
}

/// The runner's end of a call's media.
pub struct MediaPort {
    from_host: Consumer<u8>,
    to_host: Producer<u8>,
    events: Producer<MediaEvent>,
    shared: Arc<MediaShared>,
}

/// The two ends of a call's media. Everything a call will ever need to
/// cross between them is allocated here.
pub fn media_pair() -> (MediaCall, MediaPort) {
    let (to_line, from_host) = RingBuffer::new(RING_BYTES);
    let (to_host, from_line) = RingBuffer::new(RING_BYTES);
    let (event_in, event_out) = RingBuffer::new(EVENT_QUEUE);
    let shared = Arc::new(MediaShared::default());
    (
        MediaCall {
            to_line,
            from_line,
            events: event_out,
            shared: Arc::clone(&shared),
        },
        MediaPort {
            from_host,
            to_host,
            events: event_in,
            shared,
        },
    )
}

impl MediaCall {
    /// Gives data from the host to the far end. Returns how many bytes
    /// were taken, which is fewer than offered when the ring is full.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let room = self.to_line.slots().min(data.len());
        for byte in &data[..room] {
            if self.to_line.push(*byte).is_err() {
                break;
            }
        }
        room
    }

    /// Takes data from the far end for the host. Returns how many bytes
    /// were put in `buffer`.
    pub fn read(&mut self, buffer: &mut [u8]) -> usize {
        let mut count = 0;
        while count < buffer.len() {
            match self.from_line.pop() {
                Ok(byte) => {
                    buffer[count] = byte;
                    count += 1;
                }
                Err(_) => break,
            }
        }
        count
    }

    /// The next thing the runner has to say.
    pub fn poll_event(&mut self) -> Option<MediaEvent> {
        self.events.pop().ok()
    }

    /// Turns the capture of the call's packets on or off.
    pub fn set_capture(&self, on: bool) {
        self.shared.capture.store(on, Ordering::Relaxed);
    }

    /// The statistics of the call, if its runner keeps any.
    pub fn stats(&self) -> Option<MediaStats> {
        self.shared.stats()
    }

    /// The snapshot the call's engine last gave of itself, if it gives
    /// any.
    pub fn diagnostics(&self) -> Option<Diagnostics> {
        self.shared.diagnostics()
    }

    /// What the pool and the runner share: it outlives the call, so that
    /// the last statistics can be read once the runner has stopped.
    pub fn shared(&self) -> Arc<MediaShared> {
        Arc::clone(&self.shared)
    }
}

impl MediaPort {
    /// What the pool and the runner share.
    pub fn shared(&self) -> &MediaShared {
        &self.shared
    }

    /// Gives the engine the data the host sent, as much as it takes.
    pub fn feed(&mut self, engine: &mut dyn ModemEngine) {
        let waiting = self.from_host.slots().min(CHUNK);
        if waiting == 0 {
            return;
        }
        let Ok(chunk) = self.from_host.read_chunk(waiting) else {
            return;
        };
        let (first, second) = chunk.as_slices();
        let mut taken = engine.dte_write(first);
        if taken == first.len() && !second.is_empty() {
            taken += engine.dte_write(second);
        }
        chunk.commit(taken);
    }

    /// Takes from the engine the data for the host that there is room for,
    /// and what it has to report. Returns whether there is an event for
    /// the pool.
    pub fn drain(&mut self, engine: &mut dyn ModemEngine) -> bool {
        let mut buffer = [0u8; CHUNK];
        loop {
            let room = self.to_host.slots().min(CHUNK);
            if room == 0 {
                break;
            }
            let count = engine.dte_read(&mut buffer[..room]);
            for byte in &buffer[..count] {
                // There is room: it was asked for above, and nobody else
                // writes to this ring.
                let _ = self.to_host.push(*byte);
            }
            if count < room {
                break;
            }
        }
        let mut reported = false;
        while let Some(event) = engine.poll_event() {
            reported |= self.report(MediaEvent::Modem(event));
        }
        reported
    }

    /// Tells the pool something. Returns whether there was room to: an
    /// engine would have to report more than [`EVENT_QUEUE`] things between
    /// two turns of the pool for there not to be.
    pub fn report(&mut self, event: MediaEvent) -> bool {
        self.events.push(event).is_ok()
    }
}

/// Carries the media of calls.
pub trait MediaRunner: Send {
    /// Starts carrying a line's call: the engine is started, as the
    /// originating or the answering modem, and runs against the endpoint
    /// from now on. If the runner cannot use the endpoint the engine comes
    /// straight back.
    fn start(
        &mut self,
        line: LineId,
        engine: Box<dyn ModemEngine>,
        endpoint: MediaEndpoint,
        originate: bool,
    ) -> Result<MediaCall, Box<dyn ModemEngine>>;

    /// Stops carrying a line's call and gives the engine back, stopped.
    /// `None` if the engine was lost with whatever was running it.
    fn stop(&mut self, line: LineId, call: MediaCall) -> Option<Box<dyn ModemEngine>>;

    /// One tick of the pool's clock for a line. A runner that ticks its
    /// calls from the pool's clock does it here; one that has a clock of
    /// its own does nothing.
    fn tick(&mut self, _line: LineId) {}
}

/// The two ends of a call between two lines of this pool, each end's last
/// frame waiting for the other.
#[derive(Debug, Default)]
pub struct LocalLink {
    frames: Mutex<[Option<[u8; FRAME_20MS]>; 2]>,
}

/// One end of a [`LocalLink`]: the media endpoint the null switch hands
/// the pool.
#[derive(Clone, Debug)]
pub struct LocalEndpoint {
    link: Arc<LocalLink>,
    /// 0 for the line that called, 1 for the line that answered.
    side: usize,
}

impl LocalEndpoint {
    /// The two ends of a new link: the caller's, then the callee's.
    pub fn pair() -> (LocalEndpoint, LocalEndpoint) {
        let link = Arc::new(LocalLink::default());
        (
            LocalEndpoint {
                link: Arc::clone(&link),
                side: 0,
            },
            LocalEndpoint { link, side: 1 },
        )
    }

    /// Takes the frame the other end last sent, or silence if it has sent
    /// none since.
    fn receive(&self, rx: &mut [u8; FRAME_20MS]) {
        let frame = self
            .link
            .frames
            .lock()
            .ok()
            .and_then(|mut frames| frames[1 - self.side].take());
        match frame {
            Some(frame) => *rx = frame,
            None => rx.fill(SILENCE_OCTET),
        }
    }

    fn send(&self, tx: &[u8; FRAME_20MS]) {
        if let Ok(mut frames) = self.link.frames.lock() {
            frames[self.side] = Some(*tx);
        }
    }
}

struct InlineCall {
    line: LineId,
    engine: Box<dyn ModemEngine>,
    port: MediaPort,
    endpoint: LocalEndpoint,
}

/// The runner that ticks from the pool's clock: every tick each engine
/// takes and gives 160 octets, exchanged with the other line of its call.
/// It carries calls between lines of the pool, whose endpoints are
/// [`LocalEndpoint`]s.
#[derive(Default)]
pub struct InlineRunner {
    calls: Vec<InlineCall>,
}

impl InlineRunner {
    pub fn new() -> InlineRunner {
        InlineRunner::default()
    }

    /// How many calls the runner is carrying.
    pub fn call_count(&self) -> usize {
        self.calls.len()
    }
}

impl MediaRunner for InlineRunner {
    fn start(
        &mut self,
        line: LineId,
        mut engine: Box<dyn ModemEngine>,
        endpoint: MediaEndpoint,
        originate: bool,
    ) -> Result<MediaCall, Box<dyn ModemEngine>> {
        let Ok(endpoint) = endpoint.downcast::<LocalEndpoint>() else {
            return Err(engine);
        };
        if originate {
            engine.originate();
        } else {
            engine.answer();
        }
        let (call, mut port) = media_pair();
        // An engine may connect the moment it is started.
        port.drain(engine.as_mut());
        self.calls.retain(|call| call.line != line);
        self.calls.push(InlineCall {
            line,
            engine,
            port,
            endpoint,
        });
        Ok(call)
    }

    fn stop(&mut self, line: LineId, _call: MediaCall) -> Option<Box<dyn ModemEngine>> {
        let index = self.calls.iter().position(|call| call.line == line)?;
        let mut call = self.calls.swap_remove(index);
        call.engine.hangup();
        Some(call.engine)
    }

    fn tick(&mut self, line: LineId) {
        let Some(call) = self.calls.iter_mut().find(|call| call.line == line) else {
            return;
        };
        let mut rx = [0u8; FRAME_20MS];
        let mut tx = [0u8; FRAME_20MS];
        call.port.feed(call.engine.as_mut());
        call.endpoint.receive(&mut rx);
        call.engine.process(&rx, &mut tx);
        call.endpoint.send(&tx);
        call.port.drain(call.engine.as_mut());
        if let Some(snapshot) = call.engine.diagnostics() {
            call.port.shared().publish_diagnostics(&snapshot);
        }
    }
}
