// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The ModemEngine trait, modem events and the null engine.
//!
//! A modem engine sits between a line's audio and its data. Once per tick
//! it is given the G.711 octets received from the far end and fills in the
//! octets to send; on its other side it takes bytes from the host and gives
//! back the bytes it has demodulated. Every softmodem of the later phases
//! implements [`ModemEngine`].
//!
//! There are three implementations. [`Modem`] is the softmodem: it
//! answers and originates at 2400 bit/s (V.22bis), 1200 bit/s (V.22 and
//! Bell 212A) and 300 bit/s (V.21 and Bell 103), with V.8, and above 300
//! bit/s with the error control of V.42 and the compression of V.42bis.
//! [`NullEngine`] is not a modem: it frames the data bytes straight into
//! the octet stream, so that two of them joined by a path that alters no
//! octet carry data, and one altered octet shows as wrong data or a lost
//! carrier. [`EchoEngine`] sends back what it receives, for trying a
//! telephone path by ear.

#![forbid(unsafe_code)]

mod diag;
mod echo;
mod info;
mod link;
mod modem;
mod modulation;
mod null;

pub use diag::{
    CONSTELLATION_POINTS, CONSTELLATION_UNIT, Diagnostics, EngineState, SCOPE_SAMPLES,
    STATE_HISTORY, ScopeKind, StateChange,
};
pub use echo::{ECHO_CARRIER, EchoEngine};
pub use g711::Law;
pub use info::ConnectInfo;
pub use link::{
    CODEWORDS, Compression, ErrorControl, LinkEnd, LinkStage, LinkStats, STRING_LENGTH,
};
pub use modem::{
    COMPRESSION_V42BIS, COMPRESSION_V42BIS_RECEIVE, COMPRESSION_V42BIS_TRANSMIT, CallSettings,
    Modem, ModemConfig, PROTOCOL_LAPM,
};
pub use modulation::{MODULATIONS, Modulation, modulations};
pub use null::{IDLE_OCTET, NullConfig, NullEngine, SILENCE_OCTET};
pub use v14::Format;
pub use v22::GuardTone;

/// The octets in a 20 ms tick at 8000 octets a second.
pub const FRAME_20MS: usize = 160;

/// The octets in a 10 ms tick at 8000 octets a second.
pub const FRAME_10MS: usize = 80;

/// Something a modem reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModemEvent {
    /// The modems connected.
    Connect(ConnectInfo),
    /// The modems are retraining; data stops until they have.
    Retrain,
    /// The line rate changed to this many bit/s.
    RateChange(u32),
    /// The carrier was lost; the connection is over.
    CarrierLost,
}

/// One softmodem on one call.
pub trait ModemEngine: Send {
    /// One tick. `rx` and `tx` are G.711 octets of equal length (80 or 160).
    fn process(&mut self, rx: &[u8], tx: &mut [u8]);

    /// Takes data from the host to send to the line. Returns how many bytes
    /// were accepted, which may be fewer than offered.
    fn dte_write(&mut self, data: &[u8]) -> usize;

    /// Gives data received from the line to the host. Returns how many
    /// bytes were put in `buf`.
    fn dte_read(&mut self, buf: &mut [u8]) -> usize;

    /// The next event, if there is one.
    fn poll_event(&mut self) -> Option<ModemEvent>;

    /// Start as the answering modem.
    fn answer(&mut self);

    /// Start as the originating modem.
    fn originate(&mut self);

    /// Stop. The engine sends silence until it is started again.
    fn hangup(&mut self);

    /// Tells the engine the G.711 law of the call it is about to be
    /// started on. An engine that does not look inside the octets takes
    /// no notice.
    fn set_law(&mut self, _law: Law) {}

    /// Tells the engine the host's settings for the call it is about to
    /// be started on. An engine that has nothing to set takes no notice.
    fn set_call(&mut self, _call: &CallSettings) {}

    /// The engine's snapshot of itself, if it has anything to show. It is
    /// taken on the media thread every tick, so it allocates nothing.
    fn diagnostics(&self) -> Option<Diagnostics> {
        None
    }
}
