// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The data link of a synchronous modem: what is between the host's
//! octets and the bits of the line.
//!
//! When the modems have connected, the line is a stream of bits each
//! way. This makes of it what the two modems can agree on:
//!
//! 1. **Detection** (V.42 clause 7.2.1): each finds out whether the
//!    other does error control.
//! 2. **LAPM** (V.42 clause 8), if both do: the parameters by XID, the
//!    connection by SABME and UA, and then the host's octets in frames
//!    that are sent again until they arrive, compressed by V.42bis if
//!    that was agreed.
//! 3. **Start-stop characters** (V.14), if not and the line's policy
//!    allows it: the host's octets as they are, with no protection.
//!
//! It is the [`v22::BitIo`] of the modem: asked for a bit at every bit
//! time, given every bit received. The host's side is two queues of
//! octets, and the engine tells it the time.
//!
//! A link is made with its engine, which is not on the media thread, and
//! started when the modems connect, which is. Starting it allocates
//! nothing, and neither does anything after: it is built of `link/v14`,
//! `link/v42` and `link/v42bis`, which are held to that, and of fixed
//! queues. What the far end sends cannot make it panic.

use v14::Format;
use v42::lapm::{CompressionWanted, Config, Event as LapmEvent, Lapm, Negotiated, Release, State};
use v42::{Deframed, Deframer, Detection, Fill, Framer, Outcome, Role};
use v42bis::{Decoder, Encoder, Mode};

/// What a line's policy says of error control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ErrorControl {
    /// LAPM if the far end does it, and start-stop characters if not.
    #[default]
    Auto,
    /// LAPM, or the call is cleared.
    Required,
    /// Start-stop characters.
    Off,
}

impl ErrorControl {
    /// Every policy.
    pub const ALL: [ErrorControl; 3] = [
        ErrorControl::Auto,
        ErrorControl::Required,
        ErrorControl::Off,
    ];

    /// The name of the policy, as the configuration has it.
    pub const fn name(self) -> &'static str {
        match self {
            ErrorControl::Auto => "auto",
            ErrorControl::Required => "required",
            ErrorControl::Off => "off",
        }
    }
}

/// The data compression a modem asks for and agrees to: V.42bis, in
/// LAPM's frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Compression {
    /// Compression of what this modem sends.
    pub transmit: bool,
    /// Compression of what it receives.
    pub receive: bool,
    /// The most codewords, P1 of V.42bis: 512 to 4096.
    pub codewords: u16,
    /// The longest string, P2 of V.42bis: 6 to 250.
    pub string_length: u8,
}

impl Compression {
    /// None.
    pub const OFF: Compression = Compression {
        transmit: false,
        receive: false,
        codewords: CODEWORDS,
        string_length: STRING_LENGTH,
    };

    /// Both ways, with a dictionary of [`CODEWORDS`] and strings of
    /// [`STRING_LENGTH`]: what a line that allows compression asks for
    /// when its host has said nothing.
    pub const BOTH: Compression = Compression {
        transmit: true,
        receive: true,
        codewords: CODEWORDS,
        string_length: STRING_LENGTH,
    };

    /// Whether either direction is asked for.
    pub const fn any(self) -> bool {
        self.transmit || self.receive
    }
}

/// What a link is started with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkSettings {
    /// Which end of the call.
    pub role: Role,
    /// The policy.
    pub error_control: ErrorControl,
    /// Whether an originator runs the detection phase before it begins
    /// the protocol (V.42 clause 7.2.1); without, it begins the protocol
    /// at once, which an answerer that has V.42 follows and one that has
    /// not takes for data.
    pub detection: bool,
    /// The V.42bis that is asked for and agreed to.
    pub compression: Compression,
    /// The character format of the start-stop characters.
    pub format: Format,
    /// The line rate in bit/s.
    pub rate: u32,
}

/// The codewords and the string length asked for in V.42bis where the
/// host has not said. 2048 codewords is what V.42bis Appendix II.1 finds
/// most of the gain at, and a string of 32 characters costs the far end
/// little and loses little.
pub const CODEWORDS: u16 = 2048;
pub const STRING_LENGTH: u8 = 32;

/// The flags an originator sends before its first frame (V.42 clauses
/// 8.3.2.1 and 8.10.2, the notes: at least 16).
const OPENING_FLAGS: u64 = 16;

/// How long the answerer waits for the originator's first frame once
/// the detection phase has said there will be one, in samples: 10 s.
const PROTOCOL_WAIT: u32 = 80_000;

/// The octets of the host's that wait for the link, and of the link's
/// that wait for the host.
const TO_LINE: usize = 512;
const TO_HOST: usize = 4096;

/// The ticks without anything from the host after which what the
/// compressor holds is sent (V.42bis clause 5.7). The pool gives the
/// host's octets tick by tick, so one tick with nothing is the host
/// having stopped.
const FLUSH_AFTER_TICKS: u8 = 2;

/// The characters given to the compressor in a tick, at the most, and
/// the octets taken from LAPM for the decompressor at a time and how
/// many times a tick: bounded work, and far more than a line carries.
const ENCODE_STEP: usize = 64;
const DECODE_STEP: usize = 2;
const DECODE_STEPS: usize = 64;

/// Why a link came to nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Down {
    /// The policy asks for error control and the far end has none.
    Required,
    /// The far end said it had error control and did not go on with it.
    NoProtocol,
    /// The error-corrected connection ended.
    Released(Release),
    /// The far end's compressor sent what it may not (V.42bis clause
    /// 5.8).
    Compression(v42bis::Error),
}

/// How a link ended, as a display and a log name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LinkEnd {
    /// It has not ended, or was never started.
    #[default]
    None = 0,
    /// The policy asks for error control and the far end has none.
    Required = 1,
    /// The far end said it had error control and did not go on with it.
    NoProtocol = 2,
    /// The far end did not answer the set-up of the connection.
    NoResponse = 3,
    /// The far end refused the connection.
    Refused = 4,
    /// The far end released the connection.
    Remote = 5,
    /// This end released it.
    Local = 6,
    /// The far end stopped answering.
    TimedOut = 7,
    /// The far end sent what the protocol does not allow.
    Protocol = 8,
    /// The far end's compressor sent what it may not.
    Compression = 9,
}

impl LinkEnd {
    /// Every way.
    pub const ALL: [LinkEnd; 10] = [
        LinkEnd::None,
        LinkEnd::Required,
        LinkEnd::NoProtocol,
        LinkEnd::NoResponse,
        LinkEnd::Refused,
        LinkEnd::Remote,
        LinkEnd::Local,
        LinkEnd::TimedOut,
        LinkEnd::Protocol,
        LinkEnd::Compression,
    ];

    /// The name of the way, as the control API has it.
    pub const fn name(self) -> &'static str {
        match self {
            LinkEnd::None => "none",
            LinkEnd::Required => "required",
            LinkEnd::NoProtocol => "no-protocol",
            LinkEnd::NoResponse => "no-response",
            LinkEnd::Refused => "refused",
            LinkEnd::Remote => "remote",
            LinkEnd::Local => "local",
            LinkEnd::TimedOut => "timed-out",
            LinkEnd::Protocol => "protocol",
            LinkEnd::Compression => "compression",
        }
    }

    /// The way of a number; a number that is none of them is `None`.
    pub fn from_number(number: u8) -> LinkEnd {
        LinkEnd::ALL
            .into_iter()
            .find(|end| *end as u8 == number)
            .unwrap_or(LinkEnd::None)
    }

    const fn of(why: Down) -> LinkEnd {
        match why {
            Down::Required => LinkEnd::Required,
            Down::NoProtocol => LinkEnd::NoProtocol,
            Down::Released(Release::NoResponse) => LinkEnd::NoResponse,
            Down::Released(Release::Refused) => LinkEnd::Refused,
            Down::Released(Release::Remote) => LinkEnd::Remote,
            Down::Released(Release::Local) => LinkEnd::Local,
            Down::Released(Release::TimedOut) => LinkEnd::TimedOut,
            Down::Released(Release::Protocol) => LinkEnd::Protocol,
            Down::Compression(_) => LinkEnd::Compression,
        }
    }
}

/// What a link has to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    /// Data passes.
    Up {
        /// Whether by LAPM.
        lapm: bool,
        /// Whether what this end sends, and what it receives, is
        /// compressed by V.42bis.
        compression: (bool, bool),
    },
    /// No data passes any more, and the call is to be cleared.
    Down(Down),
    /// The far end sent a break.
    Break,
}

/// Where a link is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LinkStage {
    /// Not started: the modems have not connected.
    #[default]
    Idle = 0,
    /// The detection phase.
    Detecting = 1,
    /// The protocol is being set up: flags, XID, SABME.
    Protocol = 2,
    /// The error-corrected connection is up.
    Lapm = 3,
    /// Start-stop characters, with no error control.
    Async = 4,
    /// Over.
    Down = 5,
}

impl LinkStage {
    /// Every stage.
    pub const ALL: [LinkStage; 6] = [
        LinkStage::Idle,
        LinkStage::Detecting,
        LinkStage::Protocol,
        LinkStage::Lapm,
        LinkStage::Async,
        LinkStage::Down,
    ];

    /// The name of the stage, as the control API and the manager show
    /// it.
    pub const fn name(self) -> &'static str {
        match self {
            LinkStage::Idle => "idle",
            LinkStage::Detecting => "detection",
            LinkStage::Protocol => "setup",
            LinkStage::Lapm => "lapm",
            LinkStage::Async => "async",
            LinkStage::Down => "down",
        }
    }

    /// The stage of a number; a number that is no stage is `Idle`.
    pub fn from_number(number: u8) -> LinkStage {
        LinkStage::ALL
            .into_iter()
            .find(|stage| *stage as u8 == number)
            .unwrap_or(LinkStage::Idle)
    }
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

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    fn free(&self) -> usize {
        N - self.len
    }

    fn push(&mut self, octets: &[u8]) -> usize {
        let taken = octets.len().min(self.free());
        for &octet in &octets[..taken] {
            self.octets[(self.head + self.len) % N] = octet;
            self.len += 1;
        }
        taken
    }

    fn pop(&mut self, out: &mut [u8]) -> usize {
        let taken = out.len().min(self.len);
        for place in &mut out[..taken] {
            *place = self.octets[self.head];
            self.head = (self.head + 1) % N;
        }
        self.len -= taken;
        taken
    }

    /// The octets at the front, without taking them.
    fn peek(&self, out: &mut [u8]) -> usize {
        let taken = out.len().min(self.len);
        for (n, place) in out[..taken].iter_mut().enumerate() {
            *place = self.octets[(self.head + n) % N];
        }
        taken
    }

    fn drop_front(&mut self, count: usize) {
        let count = count.min(self.len);
        self.head = (self.head + count) % N;
        self.len -= count;
    }
}

/// How much of a frame the tap keeps: the whole of every frame but an I
/// frame with a long information field, of which it is the beginning.
pub const TAP_OCTETS: usize = 48;

/// The frames the tap holds until they are taken. A frame takes longer
/// on the line than a tick, so a reader that takes them every tick loses
/// none.
const TAP_FRAMES: usize = 8;

/// A frame the link sent or received, as a tool that shows frames is
/// given it: its octets from the address to the end of the information
/// field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TappedFrame {
    /// Whether this end sent it; otherwise it received it, whole.
    pub sent: bool,
    /// How long the frame was, in octets.
    pub len: u16,
    /// Its first octets.
    pub octets: [u8; TAP_OCTETS],
}

impl TappedFrame {
    const EMPTY: TappedFrame = TappedFrame {
        sent: false,
        len: 0,
        octets: [0; TAP_OCTETS],
    };

    /// The octets of the frame that were kept.
    pub fn kept(&self) -> &[u8] {
        &self.octets[..usize::from(self.len).min(TAP_OCTETS)]
    }
}

/// The tap: the last frames, for whoever asked for them. It is off
/// unless asked for, and a call of the service never asks.
#[derive(Clone, Debug)]
struct Tap {
    on: bool,
    frames: [TappedFrame; TAP_FRAMES],
    head: usize,
    len: usize,
}

impl Tap {
    const fn new() -> Tap {
        Tap {
            on: false,
            frames: [TappedFrame::EMPTY; TAP_FRAMES],
            head: 0,
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// Keeps a frame, if the tap is on and has room: a reader that does
    /// not take them loses the newest.
    #[inline]
    fn keep(&mut self, sent: bool, frame: &[u8]) {
        if !self.on || self.len == TAP_FRAMES {
            return;
        }
        let place = &mut self.frames[(self.head + self.len) % TAP_FRAMES];
        let kept = frame.len().min(TAP_OCTETS);
        place.sent = sent;
        place.len = frame.len().min(usize::from(u16::MAX)) as u16;
        place.octets[..kept].copy_from_slice(&frame[..kept]);
        self.len += 1;
    }

    fn take(&mut self) -> Option<TappedFrame> {
        if self.len == 0 {
            return None;
        }
        let frame = self.frames[self.head];
        self.head = (self.head + 1) % TAP_FRAMES;
        self.len -= 1;
        Some(frame)
    }
}

/// The counts a display reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Where the link is.
    pub stage: LinkStage,
    /// How it ended, if it has.
    pub end: LinkEnd,
    /// Frames sent, and frames received whole.
    pub frames_sent: u32,
    /// Frames received whole.
    pub frames_received: u32,
    /// Frames received with a check sequence that failed, too short, too
    /// long or aborted.
    pub frame_errors: u32,
    /// I frames sent again.
    pub retransmissions: u32,
    /// I frames sent and not yet acknowledged.
    pub window: u8,
    /// The window agreed for what this end sends.
    pub window_size: u8,
    /// Whether the far end has said it cannot take more (V.42 clause
    /// 8.4.6), whether this end has (clause 8.4.7), and whether this end
    /// is waiting for an answer to a poll (clause 8.5.3).
    pub peer_busy: bool,
    /// Whether this end has said it cannot take more.
    pub own_busy: bool,
    /// Whether this end is waiting for an answer to a poll.
    pub recovering: bool,
    /// Whether what this end sends is compressed in this call, and
    /// whether what it receives is.
    pub compression: (bool, bool),
    /// Whether the compressor is in compressed mode now, and whether the
    /// decompressor is.
    pub compressed_mode: (bool, bool),
    /// The host's octets into the compressor, and the octets out of it.
    pub compress_in: u32,
    /// The octets out of the compressor.
    pub compress_out: u32,
    /// The octets into the decompressor.
    pub decompress_in: u32,
    /// The octets out of the decompressor, to the host.
    pub decompress_out: u32,
    /// Start-stop characters lost to a wrong stop element or parity.
    pub character_errors: u32,
    /// Breaks received.
    pub breaks: u32,
}

/// The data link of one call.
pub struct DataLink {
    settings: LinkSettings,
    stage: LinkStage,
    detection: Detection,
    framer: Framer,
    deframer: Deframer,
    lapm: Lapm,
    negotiated: Negotiated,
    // The compressor and the decompressor, whose dictionaries are large:
    // made once, with the engine, and set for each call where they are.
    codec: Box<(Encoder, Decoder)>,
    transmitter: v14::Transmitter,
    receiver: v14::Receiver,
    to_line: Ring<TO_LINE>,
    to_host: Ring<TO_HOST>,
    // Samples since the protocol phase began, for an answerer that is
    // never sent a frame.
    waited: u32,
    opened: bool,
    idle_ticks: u8,
    wrote: bool,
    events: [Option<LinkEvent>; 4],
    stats: LinkStats,
    frame: [u8; v42::hdlc::MAX_FRAME],
    tap: Tap,
}

impl DataLink {
    /// A link that is not started. This is where its memory is made.
    pub fn new() -> DataLink {
        let settings = LinkSettings {
            role: Role::Answerer,
            error_control: ErrorControl::Off,
            detection: true,
            compression: Compression::OFF,
            format: Format::N81,
            rate: 1200,
        };
        DataLink {
            settings,
            stage: LinkStage::Idle,
            detection: Detection::new(settings.role, v42::T400_MS * 8, false),
            framer: Framer::new(),
            deframer: Deframer::new(),
            lapm: Lapm::new(settings.role, Config::for_rate(settings.rate)),
            negotiated: Negotiated::default(),
            codec: Box::new((
                Encoder::new(v42bis::MAX_CODEWORDS as u16, v42bis::MAX_STRING),
                Decoder::new(v42bis::MAX_CODEWORDS as u16, v42bis::MAX_STRING),
            )),
            transmitter: v14::Transmitter::new(settings.format),
            receiver: v14::Receiver::new(settings.format),
            to_line: Ring::new(),
            to_host: Ring::new(),
            waited: 0,
            opened: false,
            idle_ticks: 0,
            wrote: false,
            events: [None; 4],
            stats: LinkStats::default(),
            frame: [0; v42::hdlc::MAX_FRAME],
            tap: Tap::new(),
        }
    }

    /// Turns the tap on the link's frames on or off. It stays as it is
    /// set from call to call.
    pub fn set_tap(&mut self, on: bool) {
        self.tap.on = on;
        self.tap.clear();
    }

    /// The next frame the tap has kept.
    pub fn poll_frame(&mut self) -> Option<TappedFrame> {
        self.tap.take()
    }

    /// Stops the link and forgets what it held.
    pub fn stop(&mut self) {
        self.stage = LinkStage::Idle;
        self.to_line.clear();
        self.to_host.clear();
        self.events = [None; 4];
        self.stats = LinkStats::default();
        self.negotiated = Negotiated::default();
        self.tap.clear();
    }

    /// Starts the link, the modems having connected: the detection
    /// phase, or start-stop characters at once where the policy has no
    /// error control.
    pub fn start(&mut self, settings: LinkSettings) {
        self.stop();
        self.settings = settings;
        let mut config = Config::for_rate(settings.rate);
        let compression = settings.compression;
        config.compression = compression.any().then_some(CompressionWanted {
            transmit: compression.transmit,
            receive: compression.receive,
            codewords: compression
                .codewords
                .clamp(v42bis::MIN_CODEWORDS, v42bis::MAX_CODEWORDS as u16),
            string_length: compression
                .string_length
                .clamp(v42bis::MIN_STRING, v42bis::MAX_STRING),
        });
        self.detection = Detection::new(settings.role, v42::T400_MS * 8, true);
        self.framer = Framer::new();
        self.deframer = Deframer::new();
        self.lapm = Lapm::new(settings.role, config);
        self.transmitter = v14::Transmitter::new(settings.format);
        self.receiver = v14::Receiver::new(settings.format);
        self.waited = 0;
        self.opened = false;
        self.idle_ticks = 0;
        self.wrote = false;
        if settings.error_control == ErrorControl::Off {
            // Nothing to find out. An originator that wants none sends
            // no pattern; an answerer that wants none is a modem without
            // error control, to which a pattern is two characters of the
            // host's.
            self.go_async();
        } else if settings.role == Role::Originator && !settings.detection {
            // V.42 clause 7.2.1.1: the originator may go to the protocol
            // without the detection phase.
            self.stage = LinkStage::Protocol;
            self.framer.set_fill(Fill::Flags);
        } else {
            self.stage = LinkStage::Detecting;
        }
    }

    /// Whether data passes.
    pub fn up(&self) -> bool {
        matches!(self.stage, LinkStage::Lapm | LinkStage::Async)
    }

    /// The counts.
    pub fn stats(&self) -> LinkStats {
        if self.stage == LinkStage::Idle {
            return LinkStats::default();
        }
        let lapm = self.lapm.stats();
        let narrow = |count: u64| count.min(u64::from(u32::MAX)) as u32;
        let by_lapm = self.stage == LinkStage::Lapm;
        LinkStats {
            stage: self.stage,
            frames_sent: narrow(self.framer.frames_sent()),
            frames_received: narrow(self.deframer.frames()),
            frame_errors: narrow(self.deframer.invalid_frames() + self.deframer.aborts()),
            retransmissions: narrow(lapm.i_resent),
            window: self.lapm.outstanding(),
            window_size: if by_lapm {
                self.negotiated.window_transmit
            } else {
                0
            },
            peer_busy: by_lapm && self.lapm.busy().0,
            own_busy: by_lapm && self.lapm.busy().1,
            recovering: by_lapm && self.lapm.timer_recovery(),
            compression: (
                by_lapm && self.negotiated.compress_transmit,
                by_lapm && self.negotiated.compress_receive,
            ),
            compressed_mode: (
                by_lapm
                    && self.negotiated.compress_transmit
                    && self.codec.0.mode() == Mode::Compressed,
                by_lapm
                    && self.negotiated.compress_receive
                    && self.codec.1.mode() == Mode::Compressed,
            ),
            character_errors: self.receiver.framing_errors() + self.receiver.parity_errors(),
            breaks: self.stats.breaks + self.receiver.breaks(),
            ..self.stats
        }
    }

    /// The next thing the link has to say.
    pub fn poll_event(&mut self) -> Option<LinkEvent> {
        let event = self.events[0].take()?;
        self.events.rotate_left(1);
        Some(event)
    }

    fn push_event(&mut self, event: LinkEvent) {
        if let Some(slot) = self.events.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(event);
        }
    }

    fn go_async(&mut self) {
        self.stage = LinkStage::Async;
        self.push_event(LinkEvent::Up {
            lapm: false,
            compression: (false, false),
        });
    }

    fn go_down(&mut self, why: Down) {
        if self.stage != LinkStage::Down {
            self.stage = LinkStage::Down;
            self.stats.end = LinkEnd::of(why);
            self.push_event(LinkEvent::Down(why));
        }
    }

    /// Takes octets from the host, as many as there is room for.
    pub fn write(&mut self, octets: &[u8]) -> usize {
        if !self.up() {
            return 0;
        }
        let taken = self.to_line.push(octets);
        self.wrote |= taken > 0;
        taken
    }

    /// Gives the host the octets received.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        self.to_host.pop(out)
    }

    /// A tick: so many samples have passed. The timers run, and the
    /// octets move between the host's queues and the link.
    pub fn tick(&mut self, samples: u32) {
        match self.stage {
            LinkStage::Detecting => {
                self.detection.advance(samples);
                self.detected();
            }
            LinkStage::Protocol | LinkStage::Lapm => {
                self.lapm.advance(samples);
                if self.stage == LinkStage::Protocol {
                    self.waited = self.waited.saturating_add(samples);
                    if self.settings.role == Role::Answerer
                        && self.lapm.state() == State::Disconnected
                        && self.waited >= PROTOCOL_WAIT
                    {
                        self.go_down(Down::NoProtocol);
                    }
                }
                self.lapm_events();
                if self.stage == LinkStage::Lapm {
                    self.send_data();
                    self.deliver_data();
                }
            }
            LinkStage::Async => {
                let mut octets = [0u8; 64];
                let space = self.transmitter.space().min(octets.len());
                let taken = self.to_line.pop(&mut octets[..space]);
                self.transmitter.write(&octets[..taken]);
            }
            LinkStage::Idle | LinkStage::Down => {}
        }
        self.idle_ticks = if std::mem::take(&mut self.wrote) {
            0
        } else {
            self.idle_ticks.saturating_add(1)
        };
    }

    /// What the detection phase came to.
    fn detected(&mut self) {
        match self.detection.outcome() {
            Outcome::Pending => {}
            Outcome::ErrorControl => {
                self.stage = LinkStage::Protocol;
                self.framer.set_fill(Fill::Flags);
                self.waited = 0;
            }
            Outcome::None => {
                if self.settings.error_control == ErrorControl::Required {
                    self.go_down(Down::Required);
                } else {
                    self.go_async();
                }
            }
        }
    }

    fn lapm_events(&mut self) {
        while let Some(event) = self.lapm.poll_event() {
            match event {
                LapmEvent::Established(negotiated) => {
                    self.negotiated = negotiated;
                    // V.42bis clause 5.2: the compression is initialised
                    // with what was agreed.
                    if negotiated.compress_transmit || negotiated.compress_receive {
                        self.codec
                            .0
                            .configure(negotiated.codewords, negotiated.string_length);
                        self.codec
                            .1
                            .configure(negotiated.codewords, negotiated.string_length);
                    }
                    self.stage = LinkStage::Lapm;
                    self.push_event(LinkEvent::Up {
                        lapm: true,
                        compression: (negotiated.compress_transmit, negotiated.compress_receive),
                    });
                }
                LapmEvent::Released(why) => self.go_down(Down::Released(why)),
                LapmEvent::Break { destructive, .. } => {
                    self.stats.breaks += 1;
                    if destructive {
                        // V.42bis clause 5.6 b).
                        self.codec.0.reset();
                        self.codec.1.reset();
                        self.to_line.clear();
                        self.to_host.clear();
                    }
                    self.push_event(LinkEvent::Break);
                }
            }
        }
    }

    /// The host's octets into LAPM, through the compressor if what this
    /// end sends is compressed.
    fn send_data(&mut self) {
        if !self.negotiated.compress_transmit {
            let mut octets = [0u8; 128];
            let space = self.lapm.space().min(octets.len());
            let taken = self.to_line.pop(&mut octets[..space]);
            self.lapm.write(&octets[..taken]);
            return;
        }
        let mut plain = [0u8; ENCODE_STEP];
        let mut coded = [0u8; 256];
        // Only what LAPM has room for is encoded: what the compressor
        // gives cannot be put back.
        let room = self.lapm.space().min(coded.len());
        let have = self.to_line.peek(&mut plain);
        let (taken, written) = self.codec.0.encode(&plain[..have], &mut coded[..room]);
        self.to_line.drop_front(taken);
        self.lapm.write(&coded[..written]);
        self.stats.compress_in = self.stats.compress_in.saturating_add(taken as u32);
        self.stats.compress_out = self.stats.compress_out.saturating_add(written as u32);
        // V.42bis clause 5.7: when the host has stopped, what is held is
        // sent, so that nothing waits for more data.
        if self.to_line.len == 0
            && self.idle_ticks >= FLUSH_AFTER_TICKS
            && self.codec.0.pending()
            && self.lapm.space() >= v42bis::ENCODER_ROOM
        {
            let written = self.codec.0.flush(&mut coded);
            self.lapm.write(&coded[..written]);
            self.stats.compress_out = self.stats.compress_out.saturating_add(written as u32);
        }
    }

    /// LAPM's octets to the host, through the decompressor if what this
    /// end receives is compressed.
    fn deliver_data(&mut self) {
        if !self.negotiated.compress_receive {
            let mut octets = [0u8; 256];
            let space = self.to_host.free().min(octets.len());
            let taken = self.lapm.read(&mut octets[..space]);
            self.to_host.push(&octets[..taken]);
            return;
        }
        let mut coded = [0u8; DECODE_STEP];
        let mut plain = [0u8; DECODE_STEP * v42bis::DECODER_ROOM];
        for _ in 0..DECODE_STEPS {
            // A codeword is a string of up to 250 characters, and there
            // must be room for what the octets taken may make.
            if self.to_host.free() < plain.len() {
                break;
            }
            let taken = self.lapm.read(&mut coded);
            if taken == 0 {
                break;
            }
            match self.codec.1.decode(&coded[..taken], &mut plain) {
                Ok((_, written)) => {
                    self.to_host.push(&plain[..written]);
                    self.stats.decompress_in =
                        self.stats.decompress_in.saturating_add(taken as u32);
                    self.stats.decompress_out =
                        self.stats.decompress_out.saturating_add(written as u32);
                }
                Err(error) => {
                    // V.42bis clause 5.8. The edition of V.42 this is
                    // written from (03/2002) has no re-establishment of
                    // a connection: it is released.
                    self.lapm.release();
                    self.go_down(Down::Compression(error));
                    return;
                }
            }
        }
    }
}

impl Default for DataLink {
    fn default() -> Self {
        DataLink::new()
    }
}

impl v22::BitIo for DataLink {
    #[inline]
    fn tx_bit(&mut self) -> bool {
        match self.stage {
            LinkStage::Detecting => self.detection.next_bit(),
            LinkStage::Protocol | LinkStage::Lapm => {
                if self.framer.ready() {
                    // V.42 clause 8.3.2.1, Note 2: the originator's first
                    // frame follows at least 16 flags.
                    if !self.opened
                        && self.settings.role == Role::Originator
                        && self.framer.flags_sent() >= OPENING_FLAGS
                    {
                        self.opened = true;
                        self.lapm.establish();
                    }
                    let len = self.lapm.poll_transmit(&mut self.frame);
                    if len > 0 {
                        self.framer.send(&self.frame[..len]);
                        self.tap.keep(true, &self.frame[..len]);
                    }
                }
                self.framer.next_bit()
            }
            LinkStage::Async => self.transmitter.next_bit(),
            LinkStage::Idle | LinkStage::Down => true,
        }
    }

    #[inline]
    fn rx_bit(&mut self, bit: bool) {
        match self.stage {
            LinkStage::Detecting => {
                self.detection.receive_bit(bit);
                // The answerer may be sent frames at once, by an
                // originator that skips the detection phase: they are
                // kept for LAPM.
                self.deframer.process(bit);
            }
            LinkStage::Protocol | LinkStage::Lapm => {
                if let Deframed::Frame(_) = self.deframer.process(bit) {
                    self.lapm.receive(self.deframer.frame());
                    self.tap.keep(false, self.deframer.frame());
                }
            }
            LinkStage::Async => {
                let received = self.receiver.process(bit);
                self.to_host.push(received.characters());
            }
            LinkStage::Idle | LinkStage::Down => {}
        }
    }
}
