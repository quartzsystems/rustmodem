// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The receive session: the heart of the tick-on-receive rule.
//!
//! For each arriving packet the receiver says what the tick loop must do,
//! and the far end's timestamps are the authority on how many samples that
//! is:
//!
//! - **In order:** deliver the payload, as frames of [`FRAME`] octets.
//! - **After a gap:** first deliver fill frames for the samples the far
//!   end's clock says are missing, then the payload.
//! - **A duplicate, or older than the last delivered:** drop. There is no
//!   reorder buffer: a packet that arrives after its successor is lost, and
//!   its place was filled when the successor arrived.
//! - **Another SSRC, or a timestamp more than three seconds from the one
//!   expected:** a new stream. Start again on this packet.
//! - **Another payload type:** drop.
//!
//! The invariant, which the property tests state: between two
//! resynchronisations, the frames delivered are exactly the frames the far
//! end's sample clock produced from the first delivered packet to the last.
//! A sample is never lost and never invented.

use crate::{FRAME, RESYNC_SAMPLES, RtpHeader};

/// What to do with an arriving packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Run `fill_frames` ticks on fill frames, then `frames` ticks on the
    /// payload, 80 octets a tick.
    Deliver {
        fill_frames: u32,
        frames: u32,
        /// The stream was started again on this packet.
        resync: bool,
    },
    /// Do nothing.
    Drop(Dropped),
}

/// Why a packet was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// It was delivered already.
    Duplicate,
    /// It is older than the last delivered and its place was filled.
    Late,
    /// It is not the negotiated payload type, or its payload is not a whole
    /// number of frames.
    Ignored,
}

/// What a receiver has seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceiveStats {
    /// Packets delivered.
    pub packets: u64,
    /// Payload octets delivered.
    pub octets: u64,
    /// Packets that never arrived in time, by sequence number.
    pub lost: u64,
    /// Fill frames delivered in place of what was lost.
    pub fill_frames: u64,
    pub late: u64,
    pub duplicate: u64,
    pub resync: u64,
    pub ignored: u64,
    /// The interarrival jitter now, in samples (RFC 3550 section 6.4.1).
    pub jitter: u32,
    /// The most the jitter has been.
    pub jitter_max: u32,
}

/// How many of the last sequence numbers are remembered, to tell a
/// duplicate from a late packet.
const SEEN: u32 = 64;

#[derive(Clone, Copy, Debug)]
struct Sync {
    ssrc: u32,
    /// The sequence number after the last delivered.
    next_sequence: u16,
    /// The timestamp after the last delivered sample.
    next_timestamp: u32,
    /// Bit `n` is set when the packet `n + 1` before `next_sequence`
    /// arrived, and clear when its place was filled.
    seen: u64,
    /// Arrival time less timestamp of the last delivered packet.
    transit: u32,
}

/// The receive side of one stream.
#[derive(Clone, Debug)]
pub struct RtpReceiver {
    payload_type: u8,
    sync: Option<Sync>,
    /// The jitter estimate times 16, as RFC 3550 appendix A.8 keeps it.
    jitter_q4: u32,
    stats: ReceiveStats,
}

impl RtpReceiver {
    /// A receiver that takes packets of this payload type.
    pub fn new(payload_type: u8) -> RtpReceiver {
        RtpReceiver {
            payload_type,
            sync: None,
            jitter_q4: 0,
            stats: ReceiveStats::default(),
        }
    }

    /// Changes the payload type, for a call whose codec was renegotiated.
    pub fn set_payload_type(&mut self, payload_type: u8) {
        self.payload_type = payload_type;
    }

    pub fn payload_type(&self) -> u8 {
        self.payload_type
    }

    pub fn stats(&self) -> ReceiveStats {
        self.stats
    }

    /// Whether a first packet has been delivered.
    pub fn is_synchronised(&self) -> bool {
        self.sync.is_some()
    }

    /// Forgets the stream: the far end has said, by renegotiating, that
    /// the next packet starts a new one. That packet is delivered as a
    /// first packet is, and is not counted as a resynchronisation. The
    /// statistics are kept.
    pub fn forget_stream(&mut self) {
        self.sync = None;
    }

    /// A packet arrived. `payload_len` is the length of its payload and
    /// `arrival` the time it arrived, in samples of a monotonic clock that
    /// may start anywhere and wraps.
    pub fn receive(&mut self, header: &RtpHeader, payload_len: usize, arrival: u32) -> Verdict {
        if header.payload_type != self.payload_type
            || payload_len == 0
            || !payload_len.is_multiple_of(FRAME)
            || payload_len > usize::from(u16::MAX)
        {
            self.stats.ignored += 1;
            return Verdict::Drop(Dropped::Ignored);
        }
        let samples = payload_len as u32;
        let frames = samples / FRAME as u32;
        let transit = arrival.wrapping_sub(header.timestamp);

        let Some(sync) = &mut self.sync else {
            self.start(header, samples, transit);
            return self.delivered(0, frames, samples, false);
        };
        if sync.ssrc != header.ssrc {
            return self.resync(header, frames, samples, transit);
        }
        // How far the timestamp is from the one expected, as a signed
        // number: RTP timestamps wrap (RFC 3550 section 5.1).
        let ahead = header.timestamp.wrapping_sub(sync.next_timestamp) as i32;
        if ahead.unsigned_abs() > RESYNC_SAMPLES {
            return self.resync(header, frames, samples, transit);
        }
        let skipped = header.sequence.wrapping_sub(sync.next_sequence) as i16;
        if skipped < 0 {
            // Older than the last delivered. 0 is the last delivered.
            let back = u32::from(skipped.unsigned_abs()) - 1;
            return if back < SEEN && sync.seen & (1 << back) != 0 {
                self.stats.duplicate += 1;
                Verdict::Drop(Dropped::Duplicate)
            } else {
                self.stats.late += 1;
                Verdict::Drop(Dropped::Late)
            };
        }
        if ahead < 0 || !ahead.unsigned_abs().is_multiple_of(FRAME as u32) {
            // The sequence went on and the sample clock did not, or it left
            // the frame grid. The count cannot be kept; start again.
            return self.resync(header, frames, samples, transit);
        }

        let skipped = u32::from(skipped.unsigned_abs());
        let fill_frames = ahead.unsigned_abs() / FRAME as u32;
        sync.seen = if skipped + 1 >= SEEN {
            1
        } else {
            (sync.seen << (skipped + 1)) | 1
        };
        sync.next_sequence = header.sequence.wrapping_add(1);
        sync.next_timestamp = header.timestamp.wrapping_add(samples);

        // RFC 3550 section 6.4.1 and appendix A.8: D is the change in
        // transit time, and J moves a sixteenth of the way to |D|.
        let d = (transit.wrapping_sub(sync.transit) as i32).unsigned_abs();
        sync.transit = transit;
        let jitter = self.jitter_q4;
        self.jitter_q4 = jitter.saturating_add(d) - (jitter.saturating_add(8) >> 4);
        self.stats.jitter = self.jitter_q4 >> 4;
        self.stats.jitter_max = self.stats.jitter_max.max(self.stats.jitter);

        self.stats.lost += u64::from(skipped);
        self.stats.fill_frames += u64::from(fill_frames);
        self.delivered(fill_frames, frames, samples, false)
    }

    fn start(&mut self, header: &RtpHeader, samples: u32, transit: u32) {
        self.sync = Some(Sync {
            ssrc: header.ssrc,
            next_sequence: header.sequence.wrapping_add(1),
            next_timestamp: header.timestamp.wrapping_add(samples),
            seen: 1,
            transit,
        });
    }

    fn resync(&mut self, header: &RtpHeader, frames: u32, samples: u32, transit: u32) -> Verdict {
        self.start(header, samples, transit);
        self.stats.resync += 1;
        self.delivered(0, frames, samples, true)
    }

    fn delivered(&mut self, fill_frames: u32, frames: u32, samples: u32, resync: bool) -> Verdict {
        self.stats.packets += 1;
        self.stats.octets += u64::from(samples);
        Verdict::Deliver {
            fill_frames,
            frames,
            resync,
        }
    }
}
