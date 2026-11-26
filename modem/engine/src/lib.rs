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
//! Phase 0 has one implementation, [`NullEngine`], which is a stand-in and
//! not a modem: it frames the data bytes straight into the octet stream.

#![forbid(unsafe_code)]

mod info;
mod null;

pub use info::ConnectInfo;
pub use null::{IDLE_OCTET, NullConfig, NullEngine, SILENCE_OCTET};

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
}
