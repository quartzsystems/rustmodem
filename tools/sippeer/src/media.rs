// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The peer's RTP: a far end with a clock of its own, and with faults.
//!
//! A [`MediaStream`] makes one packet a packet time, by its own clock,
//! which may run fast or slow by so many parts per million. What it sends
//! comes from a [`Source`]. On the way out a packet may be lost, sent
//! twice, held back past the next one or delayed, as the [`Impairments`]
//! say; the faults come from a seed, so a test sees the same ones every
//! time. What comes back goes through the same receiver the service uses,
//! and is checked against what was sent when the source is a pattern, or
//! given to the engine when the source is one.
//!
//! Like the rest of the crate it is sans-I/O: it is given the time and the
//! datagrams that arrived, and gives back the datagrams to send.

use std::collections::VecDeque;

use engine::ModemEngine;
use rtp::{FRAME, HEADER_LEN, ReceiveStats, RtpPacket, RtpReceiver, RtpSender, Verdict};
use sdp::Codec;

/// The most octets held for the engine: a second.
const RX_LIMIT: usize = 8000;

/// The faults a stream puts in what it sends. The rates are packets in a
/// thousand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Impairments {
    /// Packets never sent.
    pub loss: u16,
    /// Packets sent twice.
    pub duplicate: u16,
    /// Packets held back until after the next one.
    pub reorder: u16,
    /// Each packet is delayed by up to this long.
    pub jitter_ms: u32,
    /// How fast the clock runs, in parts per million: positive is fast.
    pub clock_ppm: i32,
}

/// What a stream sends.
pub enum Source {
    /// Silence in the call's codec.
    Silence,
    /// Octets made from this seed, a different 80 for every 10 ms, so that
    /// what an echoing line sends back can be told from anything else.
    Pattern(u64),
    /// These G.711 octets, then silence.
    File(Vec<u8>),
    /// A modem engine: the far end of a data call. It has been started.
    Engine(Box<dyn ModemEngine>),
}

/// What a stream has done and seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaReport {
    /// Packets made, with those the impairments lost.
    pub made: u64,
    /// Datagrams sent.
    pub sent: u64,
    /// Packets the impairments lost, sent twice and held back.
    pub lost: u64,
    pub duplicated: u64,
    pub reordered: u64,
    /// What came back, as the receiver saw it.
    pub received: ReceiveStats,
    /// With a pattern: frames that came back as they were sent.
    pub echoed_frames: u64,
    /// With a pattern: frames of silence that came back, in the place of
    /// frames that were lost on the way out or before the echo began.
    pub silent_frames: u64,
    /// With a pattern: frames that came back as something else.
    pub mismatched_frames: u64,
    /// With a file: all of it has been sent.
    pub finished: bool,
}

/// xorshift64*: enough for faults and patterns that repeat from a seed.
#[derive(Clone, Copy, Debug)]
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Random {
        // The seeds of a test are small numbers that differ in a bit or
        // two: they are stirred, so that the streams they make differ
        // from the first octet.
        let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        state = (state ^ (state >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        state = (state ^ (state >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        state ^= state >> 31;
        // A zero state never leaves zero.
        Random(if state == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            state
        })
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// True `per_mille` times in a thousand.
    fn chance(&mut self, per_mille: u16) -> bool {
        per_mille > 0 && self.next() % 1000 < u64::from(per_mille)
    }
}

/// The frame `index` of the pattern of `seed`. No octet of it is silence
/// in either codec, so no frame of it reads as a fill frame.
pub fn pattern_frame(seed: u64, index: u64) -> [u8; FRAME] {
    let mut random = Random::new(Random::new(seed).next() ^ index);
    let mut frame = [0u8; FRAME];
    for chunk in frame.chunks_mut(8) {
        let word = random.next().to_le_bytes();
        for (octet, made) in chunk.iter_mut().zip(word) {
            *octet = match made {
                0xFF | 0xD5 => made ^ 1,
                other => other,
            };
        }
    }
    frame
}

/// One call's RTP at the peer.
pub struct MediaStream {
    silence: u8,
    /// Frames in a packet.
    frames: usize,
    sender: RtpSender,
    receiver: RtpReceiver,
    source: Source,
    impairments: Impairments,
    random: Random,
    start_us: u64,
    /// Microseconds between packets by our clock, times a million.
    interval: u128,
    /// Frames made so far.
    frames_made: u64,
    /// Octets of a file sent so far.
    file_at: usize,
    /// Datagrams waiting for their time, each with it.
    pending: Vec<(u64, Vec<u8>)>,
    /// What came back, for the engine.
    rx: VecDeque<u8>,
    /// The frame of the pattern that should come back next, once the
    /// first has been found.
    expect: Option<u64>,
    report: MediaReport,
}

impl MediaStream {
    /// A stream that begins at `now_us`. `seed` makes its faults, its
    /// source identifier and where its numbering begins.
    pub fn new(
        now_us: u64,
        codec: Codec,
        ptime_ms: u32,
        seed: u64,
        source: Source,
        impairments: Impairments,
    ) -> MediaStream {
        let mut random = Random::new(seed);
        let ssrc = random.next() as u32;
        let sequence = random.next() as u16;
        let timestamp = random.next() as u32;
        let ptime_us = u128::from(ptime_ms.clamp(10, 30)) * 1000;
        // A clock that is fast by so many parts per million has that much
        // less time between its packets.
        let rate = (1_000_000 + i64::from(impairments.clock_ppm)).max(1) as u128;
        MediaStream {
            silence: codec.silence(),
            frames: (ptime_ms.clamp(10, 30) / 10) as usize,
            sender: RtpSender::new(codec.payload_type(), ssrc, sequence, timestamp),
            receiver: RtpReceiver::new(codec.payload_type()),
            source,
            impairments,
            random,
            start_us: now_us,
            interval: ptime_us * 1_000_000 * 1_000_000 / rate,
            frames_made: 0,
            file_at: 0,
            pending: Vec::new(),
            rx: VecDeque::new(),
            expect: None,
            report: MediaReport::default(),
        }
    }

    /// When packet `n` is made.
    fn made_at(&self, n: u64) -> u64 {
        self.start_us + (u128::from(n) * self.interval / 1_000_000) as u64
    }

    /// When the stream next has something to do.
    pub fn next_due_us(&self) -> u64 {
        let made = self.made_at(self.report.made);
        self.pending
            .iter()
            .map(|(at, _)| *at)
            .min()
            .map_or(made, |held| held.min(made))
    }

    /// What the stream has done and seen.
    pub fn report(&self) -> MediaReport {
        let mut report = self.report;
        report.received = self.receiver.stats();
        report
    }

    /// The engine, when the source is one: where the test writes the data
    /// to send and reads what arrived.
    pub fn engine(&mut self) -> Option<&mut (dyn ModemEngine + 'static)> {
        match &mut self.source {
            Source::Engine(engine) => Some(engine.as_mut()),
            _ => None,
        }
    }

    /// The payload of the next packet.
    fn payload(&mut self) -> Vec<u8> {
        let len = self.frames * FRAME;
        let mut payload = vec![self.silence; len];
        match &mut self.source {
            Source::Silence => {}
            Source::Pattern(seed) => {
                for (n, frame) in payload.chunks_mut(FRAME).enumerate() {
                    frame.copy_from_slice(&pattern_frame(*seed, self.frames_made + n as u64));
                }
            }
            Source::File(octets) => {
                let rest = &octets[self.file_at.min(octets.len())..];
                let take = rest.len().min(len);
                payload[..take].copy_from_slice(&rest[..take]);
                self.file_at += take;
                self.report.finished = self.file_at >= octets.len();
            }
            Source::Engine(engine) => {
                let mut heard = [0u8; FRAME];
                for frame in payload.chunks_mut(FRAME) {
                    // What has not come yet is silence to the engine: its
                    // clock, not the arrivals, makes the ticks.
                    if self.rx.len() >= FRAME {
                        for (slot, octet) in heard.iter_mut().zip(self.rx.drain(..FRAME)) {
                            *slot = octet;
                        }
                    } else {
                        heard.fill(self.silence);
                    }
                    engine.process(&heard, frame);
                }
            }
        }
        self.frames_made += self.frames as u64;
        payload
    }

    /// Makes the packets whose time has come and gives the datagrams to
    /// send now, in the order to send them.
    pub fn poll(&mut self, now_us: u64) -> Vec<Vec<u8>> {
        while self.made_at(self.report.made) <= now_us {
            let at = self.made_at(self.report.made);
            self.report.made += 1;
            let payload = self.payload();
            let header = self.sender.next(payload.len());
            if self.random.chance(self.impairments.loss) {
                self.report.lost += 1;
                continue;
            }
            let mut datagram = vec![0u8; HEADER_LEN + payload.len()];
            let _ = header.write_packet(&payload, &mut datagram);
            let mut release = at;
            if self.impairments.jitter_ms > 0 {
                release += self.random.next() % (u64::from(self.impairments.jitter_ms) * 1000 + 1);
            }
            if self.random.chance(self.impairments.reorder) {
                // Past the next packet and its jitter.
                release = release.max(at)
                    + (self.interval / 1_000_000) as u64 * 3 / 2
                    + u64::from(self.impairments.jitter_ms) * 1000;
                self.report.reordered += 1;
            }
            if self.random.chance(self.impairments.duplicate) {
                self.report.duplicated += 1;
                self.pending.push((release, datagram.clone()));
            }
            self.pending.push((release, datagram));
        }
        let mut due = Vec::new();
        let mut kept = Vec::new();
        for held in self.pending.drain(..) {
            if held.0 <= now_us {
                due.push(held);
            } else {
                kept.push(held);
            }
        }
        self.pending = kept;
        // A stable sort: packets of one time go in the order made.
        due.sort_by_key(|(at, _)| *at);
        self.report.sent += due.len() as u64;
        due.into_iter().map(|(_, datagram)| datagram).collect()
    }

    /// Takes a datagram that arrived on the RTP port.
    pub fn receive(&mut self, now_us: u64, datagram: &[u8]) {
        let Ok(packet) = RtpPacket::parse(datagram) else {
            return;
        };
        let arrival = (u128::from(now_us) * 8 / 1000) as u32;
        let Verdict::Deliver {
            fill_frames,
            frames,
            resync,
        } = self
            .receiver
            .receive(&packet.header, packet.payload.len(), arrival)
        else {
            return;
        };
        match &self.source {
            Source::Engine(_) => {
                let fill = fill_frames as usize * FRAME;
                self.rx.extend(std::iter::repeat_n(self.silence, fill));
                self.rx.extend(packet.payload);
                // An engine that is not keeping up loses the oldest.
                let over = self.rx.len().saturating_sub(RX_LIMIT);
                self.rx.drain(..over);
            }
            Source::Pattern(seed) => {
                let seed = *seed;
                if resync {
                    self.expect = None;
                }
                // Frames that were lost on the way back.
                if let Some(expect) = &mut self.expect {
                    *expect += u64::from(fill_frames);
                }
                for frame in packet.payload.chunks(FRAME).take(frames as usize) {
                    self.compare(seed, frame);
                }
            }
            Source::Silence | Source::File(_) => {}
        }
    }

    /// Holds a frame that came back against the pattern.
    fn compare(&mut self, seed: u64, frame: &[u8]) {
        let silent = frame.iter().all(|octet| *octet == 0xFF || *octet == 0xD5);
        let Some(expect) = self.expect else {
            if silent {
                self.report.silent_frames += 1;
                return;
            }
            // The first frame of ours to come back says where the echo
            // is in what was sent.
            match (0..self.frames_made).find(|index| pattern_frame(seed, *index) == frame) {
                Some(index) => {
                    self.expect = Some(index + 1);
                    self.report.echoed_frames += 1;
                }
                None => self.report.mismatched_frames += 1,
            }
            return;
        };
        if pattern_frame(seed, expect) == frame {
            self.report.echoed_frames += 1;
        } else if silent {
            self.report.silent_frames += 1;
        } else {
            self.report.mismatched_frames += 1;
        }
        self.expect = Some(expect + 1);
    }
}
