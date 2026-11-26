// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The tick loop of one call, without its thread: what happens for each
//! datagram that arrives and for each wait that ends with none.
//!
//! The loop is written against a [`Transport`] and a [`Clock`], so that a
//! test can run the real loop with scripted arrivals and a clock it moves.
//! Nothing here allocates, locks or logs.

#![forbid(unsafe_code)]

use std::net::SocketAddrV4;
use std::sync::Arc;

use engine::ModemEngine;
use pool::{MediaEvent, MediaPort, MediaShared, MediaStats};
use rtp::{FRAME, HEADER_LEN, RtpPacket, RtpReceiver, RtpSender, Verdict};

use crate::endpoint::RtpParams;

/// The longest datagram a call's socket reads. An RTP packet of G.711 is
/// 252 octets at most.
pub const MAX_DATAGRAM: usize = 2048;

/// The most frames a packet we send holds: 30 ms.
const MAX_FRAMES: usize = 3;

/// A frame takes this long: 10 ms, in microseconds.
const FRAME_US: u64 = 10_000;

/// A sample takes this long, in microseconds.
const SAMPLE_US: u64 = 125;

/// The statistics are published after this many ticks: about twice a
/// second.
const PUBLISH_EVERY: u32 = 50;

/// When the loop has fallen behind its free-run timer, the most packets it
/// makes up in one turn.
const CATCH_UP: u32 = 5;

/// The buckets of the tick-time histogram: sixteen to a power of two, up
/// to about two seconds.
const BUCKETS: usize = 16 * 18;

/// What a wait on the transport ended with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    /// A datagram of `len` octets from `from` is in the buffer.
    Datagram { len: usize, from: SocketAddrV4 },
    /// The wait ended with nothing.
    Nothing,
}

/// Where a call's datagrams come from and go to.
pub trait Transport {
    /// Waits a short time for a datagram and puts it in `buffer`.
    fn receive(&mut self, buffer: &mut [u8]) -> Received;

    /// Sends a datagram.
    fn send(&mut self, datagram: &[u8], to: SocketAddrV4);
}

/// A clock that only goes forward.
pub trait Clock {
    /// Microseconds since some fixed moment.
    fn now_us(&mut self) -> u64;
}

/// The bucket of the tick-time histogram a time in microseconds falls in.
/// The first sixteen buckets are a microsecond wide; after that each power
/// of two is divided in sixteen.
pub fn histogram_bucket(us: u32) -> usize {
    if us < 16 {
        return us as usize;
    }
    let exponent = 31 - us.leading_zeros();
    let index = (exponent - 3) * 16 + ((us >> (exponent - 4)) & 15);
    (index as usize).min(BUCKETS - 1)
}

/// The longest time that falls in a bucket.
fn bucket_ceiling(index: usize) -> u32 {
    if index < 16 {
        return index as u32;
    }
    let exponent = (index / 16 + 3) as u32;
    let step = (index % 16) as u32;
    ((17 + step) << (exponent - 4)) - 1
}

/// The tick loop of one call.
pub struct Ticker {
    params: Arc<RtpParams>,
    /// The negotiation word last applied.
    applied: u64,
    remote: SocketAddrV4,
    frames_per_packet: usize,
    /// A frame of silence in the call's codec: what stands in for a frame
    /// that was lost, and what is received before anything is.
    fill: [u8; FRAME],
    receiver: RtpReceiver,
    sender: RtpSender,
    /// The packet being filled: the header, then the frames so far.
    packet: [u8; HEADER_LEN + MAX_FRAMES * FRAME],
    filled: usize,
    /// A packet from the far end has been delivered: the far end's clock
    /// is the clock from now on.
    synced: bool,
    /// When the next packet is due from the local timer, while not synced.
    next_free_us: u64,
    /// When the far end was last heard, or the call began.
    heard_us: u64,
    timeout_us: u64,
    stalled: bool,
    free_run: u64,
    stray: u64,
    overruns: u64,
    tick_max_us: u32,
    histogram: [u32; BUCKETS],
    ticks: u64,
    since_publish: u32,
}

impl Ticker {
    /// The loop of a call that begins at `now_us`. The SSRC and the first
    /// sequence number and timestamp are random, from the caller.
    pub fn new(
        now_us: u64,
        params: Arc<RtpParams>,
        timeout_ms: u64,
        ssrc: u32,
        sequence: u16,
        timestamp: u32,
    ) -> Ticker {
        let applied = params.raw();
        let (remote, payload_type, frames_per_packet) = RtpParams::unpack(applied);
        Ticker {
            params,
            applied,
            remote,
            frames_per_packet,
            fill: [silence(payload_type); FRAME],
            receiver: RtpReceiver::new(payload_type),
            sender: RtpSender::new(payload_type, ssrc, sequence, timestamp),
            packet: [0; HEADER_LEN + MAX_FRAMES * FRAME],
            filled: 0,
            synced: false,
            next_free_us: now_us + frames_per_packet as u64 * FRAME_US,
            heard_us: now_us,
            timeout_us: timeout_ms * 1000,
            stalled: false,
            free_run: 0,
            stray: 0,
            overruns: 0,
            tick_max_us: 0,
            histogram: [0; BUCKETS],
            ticks: 0,
            since_publish: 0,
        }
    }

    /// Whether the call has been given up: nothing was heard from the far
    /// end for the time allowed.
    pub fn is_stalled(&self) -> bool {
        self.stalled
    }

    /// Takes a renegotiation, if the router has made one.
    fn apply_params(&mut self) {
        let raw = self.params.raw();
        if raw == self.applied {
            return;
        }
        self.applied = raw;
        let (remote, payload_type, frames_per_packet) = RtpParams::unpack(raw);
        self.remote = remote;
        self.frames_per_packet = frames_per_packet;
        self.fill = [silence(payload_type); FRAME];
        self.receiver.set_payload_type(payload_type);
        self.sender.set_payload_type(payload_type);
    }

    /// One turn of the loop: what a wait on the transport ended with has
    /// been put in `buffer`. Returns whether the pool has to be woken: the
    /// engine reported something, or the call stalled.
    pub fn turn<T: Transport, C: Clock>(
        &mut self,
        received: Received,
        buffer: &[u8],
        engine: &mut dyn ModemEngine,
        transport: &mut T,
        clock: &mut C,
        port: &mut MediaPort,
    ) -> bool {
        if self.stalled {
            return false;
        }
        let now = clock.now_us();
        self.apply_params();

        if let Received::Datagram { len, from } = received {
            if from != self.remote {
                // Not the far end of this call.
                self.stray += 1;
            } else if let Ok(packet) = RtpPacket::parse(&buffer[..len.min(buffer.len())]) {
                self.heard_us = now;
                let arrival = (now / SAMPLE_US) as u32;
                if let Verdict::Deliver {
                    fill_frames,
                    frames: _,
                    resync: _,
                } = self
                    .receiver
                    .receive(&packet.header, packet.payload.len(), arrival)
                {
                    self.synced = true;
                    let fill = self.fill;
                    for _ in 0..fill_frames {
                        self.tick(&fill, engine, transport, clock);
                    }
                    for frame in packet.payload.as_chunks::<FRAME>().0 {
                        self.tick(frame, engine, transport, clock);
                    }
                }
            } else {
                // From the far end, and not RTP.
                self.stray += 1;
            }
        }

        if !self.synced {
            // Nothing has come yet: the local timer stands in for the far
            // end's clock, a packet's worth of frames at a time.
            let packet_us = self.frames_per_packet as u64 * FRAME_US;
            let fill = self.fill;
            let mut made_up = 0;
            while now >= self.next_free_us && made_up < CATCH_UP {
                for _ in 0..self.frames_per_packet {
                    self.tick(&fill, engine, transport, clock);
                    self.free_run += 1;
                }
                self.next_free_us += packet_us;
                made_up += 1;
            }
            if now >= self.next_free_us {
                // Too far behind to make up: start again from now.
                self.next_free_us = now + packet_us;
            }
        }

        port.feed(engine);
        let mut wake = port.drain(engine);

        if now.saturating_sub(self.heard_us) > self.timeout_us {
            self.stalled = true;
            port.report(MediaEvent::Stalled);
            wake = true;
            self.publish(port.shared());
        } else if self.since_publish >= PUBLISH_EVERY {
            self.publish(port.shared());
        }
        wake
    }

    /// One tick: one frame in, one frame out.
    fn tick<T: Transport, C: Clock>(
        &mut self,
        rx: &[u8],
        engine: &mut dyn ModemEngine,
        transport: &mut T,
        clock: &mut C,
    ) {
        let began = clock.now_us();
        let at = HEADER_LEN + self.filled * FRAME;
        engine.process(rx, &mut self.packet[at..at + FRAME]);
        self.filled += 1;
        if self.filled >= self.frames_per_packet {
            let len = self.filled * FRAME;
            let header = self.sender.next(len);
            if let Some((head, _)) = self.packet.split_first_chunk_mut::<HEADER_LEN>() {
                header.write(head);
            }
            // A far end on hold has told us to send nowhere.
            if !self.remote.ip().is_unspecified() {
                transport.send(&self.packet[..HEADER_LEN + len], self.remote);
            }
            self.filled = 0;
        }
        let took = clock.now_us().saturating_sub(began);
        let took_us = u32::try_from(took).unwrap_or(u32::MAX);
        let bucket = &mut self.histogram[histogram_bucket(took_us)];
        *bucket = bucket.saturating_add(1);
        self.tick_max_us = self.tick_max_us.max(took_us);
        if took > self.frames_per_packet as u64 * FRAME_US {
            self.overruns += 1;
        }
        self.ticks += 1;
        self.since_publish += 1;
    }

    /// The time that `per_mille` of the ticks were no longer than.
    fn percentile(&self, per_mille: u64) -> u32 {
        let target = (self.ticks * per_mille).div_ceil(1000).max(1);
        let mut seen = 0u64;
        for (index, count) in self.histogram.iter().enumerate() {
            seen += u64::from(*count);
            if seen >= target {
                return bucket_ceiling(index).min(self.tick_max_us);
            }
        }
        self.tick_max_us
    }

    /// The statistics of the call so far.
    pub fn stats(&self) -> MediaStats {
        let received = self.receiver.stats();
        let sent = self.sender.stats();
        let (_, payload_type, frames_per_packet) = RtpParams::unpack(self.applied);
        MediaStats {
            payload_type,
            ptime_ms: (frames_per_packet as u64 * FRAME_US / 1000) as u32,
            remote: Some(self.remote),
            rx_packets: received.packets,
            rx_octets: received.octets,
            tx_packets: sent.packets,
            tx_octets: sent.octets,
            lost: received.lost,
            late: received.late,
            duplicate: received.duplicate,
            resync: received.resync,
            ignored: received.ignored,
            stray: self.stray,
            free_run: self.free_run,
            jitter: received.jitter,
            jitter_max: received.jitter_max,
            tick_p50_us: if self.ticks == 0 {
                0
            } else {
                self.percentile(500)
            },
            tick_p99_us: if self.ticks == 0 {
                0
            } else {
                self.percentile(990)
            },
            tick_max_us: self.tick_max_us,
            overruns: self.overruns,
            capture_dropped: 0,
        }
    }

    /// Publishes the statistics for the pool to read.
    pub fn publish(&mut self, shared: &MediaShared) {
        self.since_publish = 0;
        shared.publish(&self.stats());
    }
}

/// The octet that is silence in a codec, by its payload type (RFC 3551
/// section 4.5.14): 0xD5 for PCMA, 0xFF for PCMU.
fn silence(payload_type: u8) -> u8 {
    if payload_type == 8 { 0xD5 } else { 0xFF }
}
