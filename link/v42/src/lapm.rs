// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! LAPM: the link access procedure for modems (V.42 clause 8).
//!
//! [`Lapm`] is one end of the error-corrected connection, as a machine
//! that is given the frames received, the host's octets and the time
//! that has passed, and gives the frames to send and the octets for the
//! host. It does no framing: a frame here is its octets from the address
//! to the end of the information field, as [`crate::hdlc`] gives and
//! takes them, and only frames whose check sequence was good are given
//! to it.
//!
//! # What is built
//!
//! - Negotiation by XID (clauses 8.10 and 12.2): the window, the longest
//!   information field, and the parameters of V.42bis.
//! - Set-up and release: SABME, UA, DM and DISC (clauses 8.3, 8.7 and
//!   8.8), with their retransmission on the timer T401 up to N400 times.
//! - Information transfer (clause 8.4): I frames modulo 128, RR, RNR and
//!   REJ, acknowledgement by I frame and by supervisory frame, the poll
//!   and final bits, the window.
//! - The timer-recovery condition (clauses 8.4.8 and 8.5.3) and the
//!   inactivity timer T403 (clause 8.12).
//! - Flow control both ways: RNR when the host is not reading (clause
//!   8.4.7), and no more taken from the host than the window and a queue
//!   hold.
//! - The break (clause 8.13), by UI frames.
//! - Termination on what the Recommendation ends a connection for
//!   (clause 8.4.9): the retransmissions used up, a frame that cannot be
//!   right, an N(R) that cannot be, an FRMR, a DM, an SABME where there
//!   is a connection.
//!
//! # What is not
//!
//! - Selective reject (clauses 8.2.4.8 and 8.4.5) is optional and is not
//!   negotiated: recovery is by REJ.
//! - The loop-back test (clause 8.11) and the 32-bit check sequence are
//!   optional and are not negotiated.
//! - The edition of 2002 has no re-establishment of a connection: where
//!   earlier ones reset the link, it ends the connection (clause 8.4.9),
//!   and so does this.
//!
//! # Memory and work
//!
//! Everything is in the value itself: the frames that may have to be
//! sent again are in a fixed array of the largest window by the longest
//! information field, and the two queues of octets are fixed rings. A
//! frame received does a bounded amount of work, and nothing received
//! can write past a buffer or make the machine panic.

use crate::Role;
use crate::hdlc::MAX_FRAME;
use crate::xid::{Compression, DEFAULT_K, DEFAULT_N401, Parameters};

/// The largest window this modem uses: the default (clause 9.2.4).
pub const MAX_WINDOW: u8 = DEFAULT_K;

/// The longest information field this modem uses: the default (clause
/// 9.2.3).
pub const MAX_N401: usize = DEFAULT_N401 as usize;

/// The octets from the host that wait to be put in frames.
pub const HOST_QUEUE: usize = 1024;

/// The octets received that wait for the host. When fewer than a frame's
/// worth of places are left the far end is told to stop (clause 8.4.7).
pub const DELIVERY_QUEUE: usize = 2048;

/// The places that must be free in the delivery queue before the far end
/// is told it may send again.
const DELIVERY_RESUME: usize = DELIVERY_QUEUE / 2;

/// The places for frames that may have to be sent again: more than the
/// largest window, and a power of two.
const SLOTS: usize = 16;

// The control field (Table 8 and Table 7 as Corrigendum 1 has it).
const RR: u8 = 0x01;
const RNR: u8 = 0x05;
const REJ: u8 = 0x09;
const SREJ: u8 = 0x0D;
const SABME: u8 = 0x6F;
const DM: u8 = 0x0F;
const UI: u8 = 0x03;
const DISC: u8 = 0x43;
const UA: u8 = 0x63;
const FRMR: u8 = 0x87;
const XID: u8 = 0xAF;
const TEST: u8 = 0xE3;
/// The poll or final bit of an unnumbered frame's control octet.
const U_PF: u8 = 0x10;

// The first octet of a UI frame's information field (Table 12).
const BRK: u8 = 0x40;
const BRKACK: u8 = 0x60;
/// The sequence number of a break message: bit 8 (clause 8.13.2.1).
const BREAK_SEQUENCE: u8 = 0x80;
/// The break handling option, in a BRK message's second octet (clause
/// 12.3.1): D, discard, is bit 8 and S, out of sequence, bit 7.
const BREAK_DISCARD: u8 = 0x80;
const BREAK_EXPEDITED: u8 = 0x40;

/// The timers and the counter of clause 9.2, and what is asked for in the
/// negotiation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The acknowledgement timer T401, in samples (clause 9.2.1).
    pub t401: u32,
    /// The inactivity timer T403, in samples (clause 9.2.6).
    pub t403: u32,
    /// The most retransmissions, N400 (clause 9.2.2).
    pub n400: u8,
    /// Whether the originator negotiates by XID before it sets the link
    /// up. Without, both ends use the defaults and no compression.
    pub negotiate: bool,
    /// The data compression this modem can do and wants: as an
    /// originator what it asks for, as an answerer the most it agrees to.
    /// The directions are those of this modem: to the far end, and from
    /// it.
    pub compression: Option<CompressionWanted>,
}

/// The data compression a modem wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressionWanted {
    /// Compression of what this modem sends.
    pub transmit: bool,
    /// Compression of what it receives.
    pub receive: bool,
    /// The most codewords, P1.
    pub codewords: u16,
    /// The longest string, P2.
    pub string_length: u8,
}

impl Config {
    /// The timers for a line of this many bit/s.
    ///
    /// T401 is the sum of the times of Appendix IV: a frame of the
    /// longest kind may be going out at the far end when the one to be
    /// acknowledged arrives, the acknowledgement is short, and the line
    /// and the two modems' filters take a tenth of a second each way.
    /// That is the time of a longest frame, and as much again for the
    /// frame that was to be acknowledged having been the longest too,
    /// and a third of a second. N400 is 10, by Appendix III.2: tries
    /// over several seconds. T403 is 10 s.
    pub fn for_rate(bits_per_second: u32) -> Config {
        let rate = u64::from(bits_per_second.max(300));
        // The longest frame on the line: its octets, its flag and a 0
        // after every five 1s at the worst.
        let frame_bits = (MAX_FRAME as u64 + 3) * 8 * 6 / 5;
        let frame_samples = frame_bits * 8000 / rate;
        Config {
            t401: (2 * frame_samples + 8000 / 3) as u32,
            t403: 80_000,
            n400: 10,
            negotiate: true,
            compression: None,
        }
    }
}

/// What the two modems agreed, as this modem sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negotiated {
    /// The longest information field this modem sends, in octets.
    pub n401_transmit: u16,
    /// The longest it takes.
    pub n401_receive: u16,
    /// The window for what this modem sends.
    pub window_transmit: u8,
    /// The window for what it receives.
    pub window_receive: u8,
    /// Whether what this modem sends is compressed.
    pub compress_transmit: bool,
    /// Whether what it receives is.
    pub compress_receive: bool,
    /// The number of codewords, if either is.
    pub codewords: u16,
    /// The longest string, if either is.
    pub string_length: u8,
}

impl Default for Negotiated {
    fn default() -> Self {
        Negotiated {
            n401_transmit: DEFAULT_N401,
            n401_receive: DEFAULT_N401,
            window_transmit: DEFAULT_K,
            window_receive: DEFAULT_K,
            compress_transmit: false,
            compress_receive: false,
            codewords: 0,
            string_length: 0,
        }
    }
}

/// Why a connection ended, or was never made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Release {
    /// This modem released it (clause 8.7).
    Local,
    /// The far end released it with DISC (clause 8.7.2).
    Remote,
    /// The far end did not answer the XID or the SABME in N400 tries
    /// (clauses 8.3.2.2 and 8.10.3, and Appendix III.3).
    NoResponse,
    /// The far end refused with DM (clause 8.3.2.1).
    Refused,
    /// The far end stopped answering: N400 polls in the timer-recovery
    /// condition (clause 8.4.8).
    TimedOut,
    /// Something was received that ends a connection (clause 8.4.9.1):
    /// a frame that cannot be right, an invalid N(R), an FRMR, a DM, or
    /// an SABME where there is a connection.
    Protocol,
}

/// What the machine has to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The connection is made (clause 8.3.2.1).
    Established(Negotiated),
    /// The connection is ended, or could not be made.
    Released(Release),
    /// The far end sent a break (clause 8.13).
    Break {
        /// Its length in units of 10 ms; 0 is not said, and 255 more
        /// than 2.54 s (clause 12.3.1).
        length: u8,
        /// Whether data not yet delivered was discarded with it, and the
        /// link's state variables set back: whoever compresses the data
        /// begins again (V.42bis clause 5.6 b).
        destructive: bool,
    },
}

/// The state of the connection (clauses 8.3, 8.7 and 8.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// No connection.
    Disconnected,
    /// The originator has sent XID and waits for the answer.
    Negotiating,
    /// SABME is sent and UA awaited.
    Establishing,
    /// Connected.
    Connected,
    /// DISC is sent and UA awaited.
    Releasing,
}

/// The counts a display or a test reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// I frames sent, those sent again among them.
    pub i_sent: u64,
    /// I frames sent again.
    pub i_resent: u64,
    /// I frames received in sequence and delivered.
    pub i_received: u64,
    /// I frames received out of sequence or while busy, and discarded.
    pub i_discarded: u64,
    /// REJ frames sent, and received.
    pub rej_sent: u64,
    pub rej_received: u64,
    /// RNR frames sent, and received.
    pub rnr_sent: u64,
    pub rnr_received: u64,
    /// Times the acknowledgement timer ran out.
    pub t401_expired: u64,
    /// Frames received that were ignored: of a kind not for the state,
    /// with an address not this link's, or a response not asked for.
    pub ignored: u64,
    /// Octets taken from the host, and given to it.
    pub octets_sent: u64,
    pub octets_received: u64,
}

/// A queue of octets of a fixed size.
#[derive(Clone, Debug)]
struct Ring<const N: usize> {
    octets: [u8; N],
    head: usize,
    len: usize,
}

impl<const N: usize> Ring<N> {
    const fn new() -> Ring<N> {
        Ring {
            octets: [0; N],
            head: 0,
            len: 0,
        }
    }

    fn free(&self) -> usize {
        N - self.len
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Puts as many of the octets in as fit, and says how many.
    fn push(&mut self, octets: &[u8]) -> usize {
        let taken = octets.len().min(self.free());
        for &octet in &octets[..taken] {
            self.octets[(self.head + self.len) % N] = octet;
            self.len += 1;
        }
        taken
    }

    /// Takes as many octets out as there are and as `out` holds.
    fn pop(&mut self, out: &mut [u8]) -> usize {
        let taken = out.len().min(self.len);
        for place in &mut out[..taken] {
            *place = self.octets[self.head];
            self.head = (self.head + 1) % N;
        }
        self.len -= taken;
        taken
    }
}

/// How far `a` is after `b`, modulo 128 (clause 8.2.3.2.1).
#[inline]
const fn after(a: u8, b: u8) -> u8 {
    a.wrapping_sub(b) & 0x7F
}

/// The next sequence number.
#[inline]
const fn next(n: u8) -> u8 {
    (n + 1) & 0x7F
}

/// One end of an error-corrected connection.
#[derive(Clone, Debug)]
pub struct Lapm {
    role: Role,
    config: Config,
    state: State,
    // What this modem asked for or answered with, in the negotiation's
    // own terms, and what came of it.
    asked: Parameters,
    negotiated: Negotiated,
    // Clause 8.2.3.2: the send, acknowledge and receive state variables;
    // and the number of the next I frame that has never been sent, which
    // V(S) falls behind when frames are to be sent again.
    vs: u8,
    va: u8,
    vr: u8,
    vt: u8,
    // The conditions of clauses 8.4.6 to 8.5.3.
    timer_recovery: bool,
    peer_busy: bool,
    own_busy: bool,
    reject: bool,
    discarded_busy: bool,
    // Whether anything numbered has been exchanged on this connection:
    // until it has, a repeated SABME is the originator's not having had
    // the UA (clause 8.4.9.1).
    exchanged: bool,
    // Whether this machine's connection has ended, or could not be made.
    // It makes no other. The edition of V.42 this is written from has no
    // re-establishment of a connection, and a modem clears the call when
    // its connection ends; and an end that made a new connection where
    // the other still had the old one would be given the new one's data
    // as the old one's, with a part of it missing and nothing to say
    // so.
    over: bool,
    // The timers, in samples left; none when stopped.
    t401: Option<u32>,
    t403: Option<u32>,
    retries: u8,
    // Frames owed, in the order they are sent.
    owe_ua: Option<bool>,
    owe_dm: Option<bool>,
    owe_xid: bool,
    owe_command: bool,
    owe_break_ack: Option<bool>,
    owe_break: bool,
    owe_final: bool,
    owe_poll: bool,
    owe_reject: bool,
    owe_status: bool,
    owe_ack: bool,
    // The break (clause 8.13.2).
    vsb: bool,
    vrb: bool,
    break_out: Option<u8>,
    break_timer: Option<u32>,
    break_retries: u8,
    // The I frames that may have to be sent again, by N(S) modulo the
    // number of places.
    slots: [[u8; MAX_N401]; SLOTS],
    slot_len: [u8; SLOTS],
    host: Ring<HOST_QUEUE>,
    delivery: Ring<DELIVERY_QUEUE>,
    events: [Option<Event>; 4],
    stats: Stats,
}

impl Lapm {
    /// A machine with no connection.
    pub fn new(role: Role, config: Config) -> Lapm {
        Lapm {
            role,
            config,
            state: State::Disconnected,
            asked: Parameters::default(),
            negotiated: Negotiated::default(),
            vs: 0,
            va: 0,
            vr: 0,
            vt: 0,
            timer_recovery: false,
            peer_busy: false,
            own_busy: false,
            reject: false,
            discarded_busy: false,
            exchanged: false,
            over: false,
            t401: None,
            t403: None,
            retries: 0,
            owe_ua: None,
            owe_dm: None,
            owe_xid: false,
            owe_command: false,
            owe_break_ack: None,
            owe_break: false,
            owe_final: false,
            owe_poll: false,
            owe_reject: false,
            owe_status: false,
            owe_ack: false,
            vsb: false,
            vrb: false,
            break_out: None,
            break_timer: None,
            break_retries: 0,
            slots: [[0; MAX_N401]; SLOTS],
            slot_len: [0; SLOTS],
            host: Ring::new(),
            delivery: Ring::new(),
            events: [None; 4],
            stats: Stats::default(),
        }
    }

    /// The state of the connection.
    pub fn state(&self) -> State {
        self.state
    }

    /// What was agreed, once the connection is made.
    pub fn negotiated(&self) -> Negotiated {
        self.negotiated
    }

    /// The counts.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The I frames sent and not yet acknowledged.
    pub fn outstanding(&self) -> u8 {
        after(self.vt, self.va)
    }

    /// Whether the far end has said it cannot take more (clause 8.4.6),
    /// and whether this end has (clause 8.4.7).
    pub fn busy(&self) -> (bool, bool) {
        (self.peer_busy, self.own_busy)
    }

    /// Whether the machine is in the timer-recovery condition (clause
    /// 8.5.3).
    pub fn timer_recovery(&self) -> bool {
        self.timer_recovery
    }

    /// The next thing the machine has to say.
    pub fn poll_event(&mut self) -> Option<Event> {
        let event = self.events[0].take()?;
        self.events.rotate_left(1);
        Some(event)
    }

    fn push_event(&mut self, event: Event) {
        match self.events.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(event),
            // The oldest gives way: the last is what matters, a release
            // above all.
            None => {
                self.events.rotate_left(1);
                self.events[3] = Some(event);
            }
        }
    }

    /// The address octet of a frame this end sends (clause 8.2.1 and
    /// Table 6): DLCI 0, the C/R bit, and the EA bit. A command carries
    /// the address of the end it is sent to and a response that of the
    /// end that sends it, and the answerer's is the one with C/R set.
    const fn address(&self, command: bool) -> u8 {
        let answerers = match self.role {
            Role::Originator => command,
            Role::Answerer => !command,
        };
        if answerers { 0x03 } else { 0x01 }
    }

    /// What this modem can do, as parameters in the terms of a
    /// negotiation it begins or answers.
    fn able(&self, initiator: bool) -> Parameters {
        let compression = self.config.compression.map(|wanted| {
            let (out, back) = if initiator {
                (wanted.transmit, wanted.receive)
            } else {
                (wanted.receive, wanted.transmit)
            };
            Compression {
                initiator_to_responder: out,
                responder_to_initiator: back,
                codewords: wanted.codewords,
                string_length: wanted.string_length,
            }
        });
        Parameters {
            compression: compression.filter(|c| c.any()),
            ..Parameters::default()
        }
    }

    /// Takes what was agreed, from the negotiation's terms to this
    /// modem's.
    fn adopt(&mut self, agreed: &Parameters, initiator: bool) {
        let most = |n: u16| n.clamp(1, MAX_N401 as u16);
        let window = |k: u8| k.clamp(1, MAX_WINDOW);
        let (n_out, n_in, k_out, k_in) = if initiator {
            (
                agreed.n401_initiator,
                agreed.n401_responder,
                agreed.k_initiator,
                agreed.k_responder,
            )
        } else {
            (
                agreed.n401_responder,
                agreed.n401_initiator,
                agreed.k_responder,
                agreed.k_initiator,
            )
        };
        let (out, back, codewords, string_length) = match agreed.compression {
            Some(c) if initiator => (
                c.initiator_to_responder,
                c.responder_to_initiator,
                c.codewords,
                c.string_length,
            ),
            Some(c) => (
                c.responder_to_initiator,
                c.initiator_to_responder,
                c.codewords,
                c.string_length,
            ),
            None => (false, false, 0, 0),
        };
        self.negotiated = Negotiated {
            n401_transmit: most(n_out),
            n401_receive: most(n_in),
            window_transmit: window(k_out),
            window_receive: window(k_in),
            compress_transmit: out,
            compress_receive: back,
            codewords,
            string_length,
        };
    }

    /// Begins to make the connection (clause 7.2.2, the L-ESTABLISH
    /// request): the originator negotiates by XID if it is to, and sends
    /// SABME. Nothing happens unless there is no connection and there
    /// has been none: a machine makes one connection, and a call whose
    /// connection has ended is cleared.
    pub fn establish(&mut self) {
        if self.state != State::Disconnected || self.over {
            return;
        }
        self.negotiated = Negotiated::default();
        self.retries = 0;
        self.owe_command = true;
        self.t401 = Some(self.config.t401);
        if self.config.negotiate {
            // Clause 8.10.2.
            self.asked = self.able(true);
            self.state = State::Negotiating;
        } else {
            self.state = State::Establishing;
        }
    }

    /// Releases the connection in an orderly way (clause 8.7.2), or
    /// gives up making it.
    pub fn release(&mut self) {
        match self.state {
            State::Disconnected | State::Releasing => {}
            State::Negotiating | State::Establishing => self.end(Release::Local),
            State::Connected => {
                // Clause 8.7.1: what is queued is discarded.
                self.host.clear();
                self.state = State::Releasing;
                self.retries = 0;
                self.owe_command = true;
                self.t401 = Some(self.config.t401);
                self.t403 = None;
            }
        }
    }

    /// Takes octets from the host for the far end, as many as there is
    /// room for, and says how many. None is taken without a connection.
    pub fn write(&mut self, octets: &[u8]) -> usize {
        if self.state != State::Connected {
            return 0;
        }
        let taken = self.host.push(octets);
        self.stats.octets_sent += taken as u64;
        taken
    }

    /// The room for the host's octets.
    pub fn space(&self) -> usize {
        if self.state == State::Connected {
            self.host.free()
        } else {
            0
        }
    }

    /// Whether everything taken from the host has been sent and
    /// acknowledged.
    pub fn drained(&self) -> bool {
        self.host.len == 0 && self.vt == self.va
    }

    /// Gives the host the octets received, as many as `out` holds, and
    /// says how many.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let taken = self.delivery.pop(out);
        self.stats.octets_received += taken as u64;
        if self.own_busy && self.delivery.free() >= DELIVERY_RESUME {
            // Clause 8.4.7: the busy condition is cleared with RR, or
            // with REJ if frames were discarded for it.
            self.own_busy = false;
            if self.discarded_busy {
                self.discarded_busy = false;
                self.reject = true;
                self.owe_reject = true;
            } else {
                self.owe_status = true;
            }
        }
        taken
    }

    /// The octets received that the host has not taken.
    pub fn readable(&self) -> usize {
        self.delivery.len
    }

    /// Sends a break to the far end (clause 8.13.3.1): not destructive,
    /// and ahead of the data. `length` is in units of 10 ms. Nothing
    /// happens without a connection, or while an earlier break is not
    /// yet acknowledged (clause 7.4).
    pub fn send_break(&mut self, length: u8) -> bool {
        if self.state != State::Connected || self.break_out.is_some() {
            return false;
        }
        self.break_out = Some(length);
        self.owe_break = true;
        self.break_retries = 0;
        self.break_timer = Some(self.config.t401);
        true
    }

    /// The connection is over, or was not made: no connection, and
    /// nothing owed.
    fn end(&mut self, why: Release) {
        self.state = State::Disconnected;
        self.over = true;
        self.t401 = None;
        self.t403 = None;
        self.break_timer = None;
        self.break_out = None;
        self.owe_xid = false;
        self.owe_command = false;
        self.owe_break = false;
        self.owe_break_ack = None;
        self.owe_final = false;
        self.owe_poll = false;
        self.owe_reject = false;
        self.owe_status = false;
        self.owe_ack = false;
        self.host.clear();
        self.push_event(Event::Released(why));
    }

    /// The connection is made: the state variables at nothing (clause
    /// 8.3.2.1).
    fn connect(&mut self) {
        self.state = State::Connected;
        self.reset_variables();
        self.vsb = false;
        self.vrb = false;
        self.host.clear();
        self.delivery.clear();
        self.t401 = None;
        self.t403 = Some(self.config.t403);
        self.owe_command = false;
        self.push_event(Event::Established(self.negotiated));
    }

    fn reset_variables(&mut self) {
        self.vs = 0;
        self.va = 0;
        self.vr = 0;
        self.vt = 0;
        self.timer_recovery = false;
        self.peer_busy = false;
        self.own_busy = false;
        self.reject = false;
        self.discarded_busy = false;
        self.exchanged = false;
        self.retries = 0;
        self.owe_final = false;
        self.owe_poll = false;
        self.owe_reject = false;
        self.owe_status = false;
        self.owe_ack = false;
    }

    /// Time passes: so many samples.
    pub fn advance(&mut self, samples: u32) {
        let ran_out = |timer: &mut Option<u32>| match timer {
            Some(left) if *left > samples => {
                *left -= samples;
                false
            }
            Some(_) => {
                *timer = None;
                true
            }
            None => false,
        };
        if ran_out(&mut self.break_timer) {
            // Clause 8.13.3.4.
            self.break_retries += 1;
            if self.break_retries > self.config.n400 {
                self.break_out = None;
            } else if self.break_out.is_some() {
                self.owe_break = true;
                self.break_timer = Some(self.config.t401);
            }
        }
        if ran_out(&mut self.t403) && self.state == State::Connected && self.t401.is_none() {
            // Clause 8.12.3.3.
            self.timer_recovery = true;
            self.retries = 0;
            self.owe_poll = true;
            self.t401 = Some(self.config.t401);
        }
        if !ran_out(&mut self.t401) {
            return;
        }
        self.stats.t401_expired += 1;
        match self.state {
            State::Disconnected => {}
            State::Negotiating | State::Establishing | State::Releasing => {
                // Clauses 8.3.2.2, 8.7.3 and 8.10.3.
                self.retries += 1;
                if self.retries > self.config.n400 {
                    self.end(if self.state == State::Releasing {
                        Release::Local
                    } else {
                        Release::NoResponse
                    });
                } else {
                    self.owe_command = true;
                    self.t401 = Some(self.config.t401);
                }
            }
            State::Connected => {
                // Clause 8.4.8.
                if self.timer_recovery {
                    self.retries += 1;
                } else {
                    self.timer_recovery = true;
                    self.retries = 0;
                }
                if self.retries >= self.config.n400 {
                    self.end(Release::TimedOut);
                } else {
                    self.owe_poll = true;
                    self.t401 = Some(self.config.t401);
                }
            }
        }
    }

    /// Takes a frame received: its octets from the address to the end of
    /// the information field, the check sequence having been good.
    pub fn receive(&mut self, frame: &[u8]) {
        // Clause 8.1.3 e): an address of one octet, DLCI 0.
        let [address, control, rest @ ..] = frame else {
            self.stats.ignored += 1;
            return;
        };
        let command = if *address == self.address(false) {
            // The far end's commands carry this end's address.
            true
        } else if *address == self.address(true) {
            false
        } else {
            self.stats.ignored += 1;
            return;
        };
        if control & 1 == 0 {
            // An I frame: N(S), and in the next octet N(R) and P.
            let Some((&second, information)) = rest.split_first() else {
                // Clause 8.1.3 b): too short for a frame with sequence
                // numbers.
                self.stats.ignored += 1;
                return;
            };
            if !command {
                // I is a command only (Table 8).
                self.reject_frame();
                return;
            }
            self.information(control >> 1, second >> 1, second & 1 == 1, information);
        } else if control & 3 == 1 {
            let Some((&second, more)) = rest.split_first() else {
                self.stats.ignored += 1;
                return;
            };
            if !more.is_empty() || !matches!(*control, RR | RNR | REJ) {
                // A supervisory frame of the wrong length (clause
                // 8.2.4.12 b), or SREJ, which was not agreed (clause
                // 8.4.5.1), or a kind there is none of.
                let _ = SREJ;
                self.reject_frame();
                return;
            }
            self.supervisory(*control, command, second >> 1, second & 1 == 1);
        } else {
            self.unnumbered(control & !U_PF, command, control & U_PF != 0, rest);
        }
    }

    /// A frame-rejection condition (clause 8.5.5): with a connection it
    /// ends it; without, the frame is discarded.
    fn reject_frame(&mut self) {
        if self.state == State::Connected {
            self.end(Release::Protocol);
        } else {
            self.stats.ignored += 1;
        }
    }

    /// An I frame or a supervisory frame came while SABME was out: the
    /// UA was lost, and this is as good (clause 8.3.2.1).
    fn connect_if_establishing(&mut self) {
        if self.state == State::Establishing {
            self.connect();
        }
    }

    /// Takes an N(R): whether it is one there can be, and if so the
    /// frames it acknowledges (clauses 8.2.3.2.3 and 8.4.3.2).
    fn acknowledge(&mut self, nr: u8, stop_timer: bool) -> bool {
        // Clause 8.5.2: V(A) <= N(R) <= V(S). The V(S) of that is the
        // number of the next frame that has never been sent. A REJ or the
        // answer to a poll takes V(S) back, and a frame that was sent
        // before it did may yet arrive and be acknowledged: at 2400 bit/s
        // a frame is on the line for half a second, and the far end's
        // REJ and its acknowledgement can both come while the next one
        // is still going out. That is no error, and what the far end has
        // is not sent again: V(S) goes on from the acknowledgement.
        if after(nr, self.va) > after(self.vt, self.va) {
            self.end(Release::Protocol);
            return false;
        }
        if after(nr, self.va) > after(self.vs, self.va) {
            self.vs = nr;
        }
        if nr != self.va {
            self.va = nr;
            if stop_timer && !self.timer_recovery {
                // Clause 8.4.3.2: stopped, and started again if I frames
                // are still out.
                self.t401 = (self.va != self.vs).then_some(self.config.t401);
            }
        }
        if self.t401.is_none() {
            // Clause 8.12.3.1.
            self.t403 = Some(self.config.t403);
        }
        true
    }

    /// An I frame (clause 8.4.2).
    fn information(&mut self, ns: u8, nr: u8, poll: bool, information: &[u8]) {
        self.connect_if_establishing();
        if self.state != State::Connected {
            self.stats.ignored += 1;
            return;
        }
        if information.len() > usize::from(self.negotiated.n401_receive) {
            // Clause 8.2.4.12 d).
            self.end(Release::Protocol);
            return;
        }
        self.exchanged = true;
        // Clause 8.4.3.2, Note 2: the timer is not stopped by an I frame
        // while the far end is busy.
        if !self.acknowledge(nr, !self.peer_busy) {
            return;
        }
        if self.own_busy {
            // Clause 8.4.7: discarded, after V(A).
            self.stats.i_discarded += 1;
            self.discarded_busy = true;
            if poll {
                self.owe_final = true;
            }
            return;
        }
        if ns != self.vr {
            // Clause 8.5.1: an N(S) sequence error. One REJ at a time.
            self.stats.i_discarded += 1;
            if !self.reject {
                self.reject = true;
                self.owe_reject = true;
            }
            if poll {
                self.owe_final = true;
            }
            return;
        }
        if information.len() > self.delivery.free() {
            // No room: it is as though it had not come, and the far end
            // is told to stop.
            self.stats.i_discarded += 1;
            self.own_busy = true;
            self.discarded_busy = true;
            self.owe_status = true;
            if poll {
                self.owe_final = true;
            }
            return;
        }
        self.delivery.push(information);
        self.stats.i_received += 1;
        self.vr = next(self.vr);
        self.reject = false;
        self.owe_reject = false;
        if self.delivery.free() < MAX_N401 {
            // Clause 8.4.7: busy before the next frame would be lost.
            self.own_busy = true;
            self.owe_status = true;
        }
        if poll {
            // Clause 8.4.2.1.
            self.owe_final = true;
        } else {
            // Clause 8.4.2.2: an I frame if there is one, or RR.
            self.owe_ack = true;
        }
    }

    /// RR, RNR or REJ (clauses 8.4.3 to 8.4.8).
    fn supervisory(&mut self, kind: u8, command: bool, nr: u8, pf: bool) {
        self.connect_if_establishing();
        if self.state != State::Connected {
            self.stats.ignored += 1;
            return;
        }
        self.exchanged = true;
        let solicited = !command && pf && self.timer_recovery;
        if !command && pf && !self.timer_recovery {
            // Table 9: a response with F set that was not asked for is
            // ignored, and is a protocol violation.
            self.stats.ignored += 1;
            return;
        }
        if command && pf {
            // Clause 8.4.6: answered with F set.
            self.owe_final = true;
        }
        match kind {
            RR => {
                if !self.acknowledge(nr, true) {
                    return;
                }
                self.peer_busy = false;
                if solicited {
                    self.recovered(nr);
                }
            }
            RNR => {
                self.stats.rnr_received += 1;
                if !self.acknowledge(nr, false) {
                    return;
                }
                self.peer_busy = true;
                if solicited {
                    // Clause 8.4.6: V(S) from the N(R), and the enquiry
                    // goes on.
                    self.timer_recovery = false;
                    self.retries = 0;
                    self.vs = nr;
                }
                if !self.timer_recovery {
                    self.t401 = Some(self.config.t401);
                    self.t403 = None;
                }
            }
            _ => {
                self.stats.rej_received += 1;
                if !self.acknowledge(nr, false) {
                    return;
                }
                self.peer_busy = false;
                if !self.timer_recovery || solicited {
                    // Clause 8.4.4 a) and b): V(S) and V(A) to the N(R),
                    // and the frames from there again.
                    self.timer_recovery = false;
                    self.retries = 0;
                    self.vs = nr;
                    self.va = nr;
                    self.t401 = None;
                    self.t403 = Some(self.config.t403);
                }
                // Clause 8.4.4 c): otherwise V(A) alone, which is done.
            }
        }
    }

    /// The timer-recovery condition is cleared by a response with F set
    /// (clause 8.4.8): V(S) to its N(R), and on with the I frames.
    fn recovered(&mut self, nr: u8) {
        self.timer_recovery = false;
        self.retries = 0;
        self.vs = nr;
        self.t401 = None;
        self.t403 = Some(self.config.t403);
    }

    /// An unnumbered frame.
    fn unnumbered(&mut self, kind: u8, command: bool, pf: bool, information: &[u8]) {
        match (kind, command) {
            (SABME, true) => self.sabme(pf, information),
            (DISC, true) => {
                if !information.is_empty() {
                    self.reject_frame();
                    return;
                }
                match self.state {
                    // Clause 8.8.
                    State::Disconnected | State::Negotiating => self.owe_dm = Some(pf),
                    // Clause 8.9.2: different set-mode commands.
                    State::Establishing => self.owe_dm = Some(pf),
                    // Clause 8.7.2.
                    State::Connected => {
                        self.owe_ua = Some(pf);
                        self.end(Release::Remote);
                    }
                    // Clause 8.9.1: the same command both ways.
                    State::Releasing => self.owe_ua = Some(pf),
                }
            }
            (UA, false) => match self.state {
                // Clause 8.3.2.1.
                State::Establishing if pf => self.connect(),
                // Clause 8.7.2.
                State::Releasing if pf => self.end(Release::Local),
                // Table 9: not asked for.
                _ => self.stats.ignored += 1,
            },
            (DM, false) => match self.state {
                // Clause 8.3.2.1: refused. With F clear it is ignored
                // (clause 8.9.3).
                State::Establishing if pf => self.end(Release::Refused),
                State::Releasing if pf => self.end(Release::Local),
                // Clause 8.4.9.1: with F clear where there is a
                // connection, or with F set in the timer-recovery
                // condition.
                State::Connected if !pf || self.timer_recovery => self.end(Release::Protocol),
                _ => self.stats.ignored += 1,
            },
            (FRMR, false) => {
                // Clause 8.5.6.
                if self.state == State::Connected {
                    self.end(Release::Protocol);
                } else {
                    self.stats.ignored += 1;
                }
            }
            (XID, true) => {
                // Clause 8.10.2.
                match self.state {
                    State::Disconnected if !self.over => {
                        let theirs = Parameters::decode(information).unwrap_or_default();
                        let answer = Parameters::answer(&theirs, &self.able(false));
                        self.adopt(&answer, false);
                        self.asked = answer;
                        self.owe_xid = true;
                    }
                    // With a connection the parameters stay as they are,
                    // and the answer says what they are.
                    State::Connected => self.owe_xid = true,
                    _ => self.stats.ignored += 1,
                }
            }
            (XID, false) => {
                if self.state == State::Negotiating {
                    let answer = Parameters::decode(information).unwrap_or_default();
                    let agreed = Parameters::agreed(&self.asked, &answer);
                    self.adopt(&agreed, true);
                    self.state = State::Establishing;
                    self.retries = 0;
                    self.owe_command = true;
                    self.t401 = Some(self.config.t401);
                } else {
                    self.stats.ignored += 1;
                }
            }
            (UI, _) => self.user_information(command, pf, information),
            (TEST, true) => {
                // Clause 8.11: optional, and not agreed to.
                self.stats.ignored += 1;
            }
            // Clause 8.2.4.1: not in Table 8, or a response where there
            // are only commands, or the other way about.
            _ => self.reject_frame(),
        }
    }

    /// SABME (clauses 8.3.2.1 and 8.4.9.1).
    fn sabme(&mut self, poll: bool, information: &[u8]) {
        if !information.is_empty() {
            self.reject_frame();
            return;
        }
        match self.state {
            // Clause 8.3.2.1: unable to enter the connected state, DM.
            State::Disconnected if self.over => self.owe_dm = Some(poll),
            State::Disconnected => {
                self.owe_ua = Some(poll);
                self.connect();
            }
            // Clause 8.9.1: the same command both ways.
            State::Negotiating | State::Establishing => self.owe_ua = Some(poll),
            State::Connected if !self.exchanged => {
                // The originator did not have the UA.
                self.owe_ua = Some(poll);
            }
            State::Connected => self.end(Release::Protocol),
            // Clause 8.9.2: different commands.
            State::Releasing => self.owe_dm = Some(poll),
        }
    }

    /// A UI frame: a break or its acknowledgement (clause 8.13.3).
    fn user_information(&mut self, command: bool, pf: bool, information: &[u8]) {
        if self.state != State::Connected {
            self.stats.ignored += 1;
            return;
        }
        let Some((&first, rest)) = information.split_first() else {
            self.stats.ignored += 1;
            return;
        };
        let sequence = first & BREAK_SEQUENCE != 0;
        match (first & !BREAK_SEQUENCE, command) {
            (BRK, true) => {
                if sequence == self.vrb {
                    // Clause 8.13.3.2.
                    self.vrb = !self.vrb;
                    let options = rest.first().copied().unwrap_or(0);
                    let length = rest.get(1).copied().unwrap_or(0);
                    let destructive = options & BREAK_DISCARD != 0;
                    if destructive {
                        // Tables 4 and 5, note a): the data not delivered
                        // and not sent is discarded, and the state
                        // variables are set back.
                        self.host.clear();
                        self.delivery.clear();
                        self.reset_variables();
                        self.t401 = None;
                        self.t403 = Some(self.config.t403);
                    }
                    self.push_event(Event::Break {
                        length,
                        destructive,
                    });
                }
                // A break had before is acknowledged again and not given
                // again.
                self.owe_break_ack = Some(pf);
            }
            (BRKACK, false) => {
                // Clause 8.13.3.3: N(RB) is V(SB) + 1.
                if self.break_out.is_some() && sequence != self.vsb {
                    self.vsb = !self.vsb;
                    self.break_out = None;
                    self.break_timer = None;
                    self.owe_break = false;
                }
            }
            // Table 9, Note 1: an encoding that is not known.
            _ => self.stats.ignored += 1,
        }
    }

    /// Writes a supervisory frame.
    fn put_supervisory(
        &self,
        out: &mut [u8; MAX_FRAME],
        kind: u8,
        command: bool,
        pf: bool,
    ) -> usize {
        out[0] = self.address(command);
        out[1] = kind;
        // Clause 8.4.3.1: N(R) is V(R).
        out[2] = (self.vr << 1) | u8::from(pf);
        3
    }

    /// Writes an unnumbered frame with no information field.
    fn put_unnumbered(
        &self,
        out: &mut [u8; MAX_FRAME],
        kind: u8,
        command: bool,
        pf: bool,
    ) -> usize {
        out[0] = self.address(command);
        out[1] = kind | if pf { U_PF } else { 0 };
        2
    }

    /// The supervisory frame that says how this end is (clause 8.4.6,
    /// Note 2): RNR if it is busy, and RR otherwise, the REJ of a
    /// sequence error having been sent when the error was seen.
    fn status(&mut self) -> u8 {
        if self.own_busy {
            self.stats.rnr_sent += 1;
            RNR
        } else {
            RR
        }
    }

    /// Gives the next frame to send, if there is one, and its length:
    /// called when the line is free for a frame.
    pub fn poll_transmit(&mut self, out: &mut [u8; MAX_FRAME]) -> usize {
        // Responses to set-mode commands first: "at the earliest
        // possible opportunity" (clause 8.9.1).
        if let Some(f) = self.owe_ua.take() {
            return self.put_unnumbered(out, UA, false, f);
        }
        if let Some(f) = self.owe_dm.take() {
            return self.put_unnumbered(out, DM, false, f);
        }
        if std::mem::take(&mut self.owe_xid) {
            // Clause 8.2.4.13: the P/F bit of an XID frame is 0.
            let head = self.put_unnumbered(out, XID, false, false);
            return head + self.asked.encode(&mut out[head..]);
        }
        if std::mem::take(&mut self.owe_command) {
            return match self.state {
                State::Negotiating => {
                    let head = self.put_unnumbered(out, XID, true, false);
                    head + self.asked.encode(&mut out[head..])
                }
                // Clauses 8.3.2.1 and 8.7.2, the notes: always with P.
                State::Establishing => self.put_unnumbered(out, SABME, true, true),
                State::Releasing => self.put_unnumbered(out, DISC, true, true),
                _ => 0,
            };
        }
        if self.state != State::Connected {
            return 0;
        }
        if let Some(f) = self.owe_break_ack.take() {
            // Clause 8.13.3.2: N(RB) is V(RB).
            let head = self.put_unnumbered(out, UI, false, f);
            out[head] = BRKACK | if self.vrb { BREAK_SEQUENCE } else { 0 };
            return head + 1;
        }
        if std::mem::take(&mut self.owe_break)
            && let Some(length) = self.break_out
        {
            // Clauses 8.13.3.1 and 12.3.1: not destructive, expedited.
            let head = self.put_unnumbered(out, UI, true, false);
            out[head] = BRK | if self.vsb { BREAK_SEQUENCE } else { 0 };
            out[head + 1] = BREAK_EXPEDITED;
            out[head + 2] = length;
            return head + 3;
        }
        if std::mem::take(&mut self.owe_final) {
            self.owe_ack = false;
            self.owe_status = false;
            let kind = self.status();
            return self.put_supervisory(out, kind, false, true);
        }
        if std::mem::take(&mut self.owe_poll) {
            // Clause 8.4.8 a).
            self.owe_ack = false;
            let kind = self.status();
            return self.put_supervisory(out, kind, true, true);
        }
        if std::mem::take(&mut self.owe_reject) {
            self.owe_ack = false;
            self.owe_status = false;
            self.stats.rej_sent += 1;
            return self.put_supervisory(out, REJ, false, false);
        }
        if std::mem::take(&mut self.owe_status) {
            self.owe_ack = false;
            let kind = self.status();
            return self.put_supervisory(out, kind, false, false);
        }
        if let Some(len) = self.put_information(out) {
            return len;
        }
        if std::mem::take(&mut self.owe_ack) {
            let kind = self.status();
            return self.put_supervisory(out, kind, false, false);
        }
        0
    }

    /// Writes the next I frame, one to be sent again or a new one, if
    /// one may be sent (clause 8.4.1).
    fn put_information(&mut self, out: &mut [u8; MAX_FRAME]) -> Option<usize> {
        // Clause 8.4.6: none to a busy far end; clause 8.4.8: none until
        // the timer-recovery condition is cleared.
        if self.peer_busy || self.timer_recovery {
            return None;
        }
        let slot = usize::from(self.vs) % SLOTS;
        if self.vs == self.vt {
            // A new one, if the window is open and the host has given
            // anything.
            if after(self.vt, self.va) >= self.negotiated.window_transmit || self.host.len == 0 {
                return None;
            }
            let most = usize::from(self.negotiated.n401_transmit).min(MAX_N401);
            let len = self.host.pop(&mut self.slots[slot][..most]);
            self.slot_len[slot] = len as u8;
            self.vt = next(self.vt);
        } else {
            self.stats.i_resent += 1;
        }
        let len = usize::from(self.slot_len[slot]);
        out[0] = self.address(true);
        out[1] = self.vs << 1;
        out[2] = self.vr << 1;
        out[3..3 + len].copy_from_slice(&self.slots[slot][..len]);
        self.vs = next(self.vs);
        self.stats.i_sent += 1;
        // It carries the acknowledgement.
        self.owe_ack = false;
        if self.t401.is_none() {
            self.t401 = Some(self.config.t401);
        }
        // Clause 8.12.3.2.
        self.t403 = None;
        Some(3 + len)
    }
}
