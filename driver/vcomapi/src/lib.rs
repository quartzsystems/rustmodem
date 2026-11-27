// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The control interface of the virtual COM port.
//!
//! A RustModem port is one device with two sides. A host opens it by its
//! COM name and uses it as a serial port. The service opens the same
//! device with [`CONTROL_NAME`] after the name, `\\.\COM10\control`, and
//! is then the modem's side of the port: it speaks the I/O control codes
//! here and no others. The driver and the service's backend both take the
//! codes and the layouts from this crate, so they cannot differ.
//!
//! Every code is buffered. Numbers are little-endian.

#![forbid(unsafe_code)]

/// What follows the port's name when the service opens it.
pub const CONTROL_NAME: &str = "\\control";

/// The device interface every RustModem port has, by which setup and the
/// manager find the ports:
/// `{5F0D2A0B-91C4-4E57-A31E-524D434F4D31}`.
pub const INTERFACE_GUID: (u32, u16, u16, [u8; 8]) = (
    0x5F0D_2A0B,
    0x91C4,
    0x4E57,
    [0xA3, 0x1E, 0x52, 0x4D, 0x43, 0x4F, 0x4D, 0x31],
);

/// The version of this interface, which [`State`] reports. A backend
/// refuses a driver whose version it does not know.
pub const VERSION: u32 = 1;

/// A device type in the range Windows leaves to vendors.
const DEVICE_TYPE: u32 = 0x8A51;

const fn code(function: u32) -> u32 {
    // CTL_CODE(DEVICE_TYPE, function, METHOD_BUFFERED, FILE_ANY_ACCESS).
    (DEVICE_TYPE << 16) | (function << 2)
}

/// Reads what the host wrote. Output: the octets, as many as there are up
/// to the buffer's length. Waits until there is at least one.
pub const IOCTL_READ: u32 = code(0x800);
/// Writes octets for the host. Input: the octets. Output: a `u32`, how
/// many were taken, which is fewer than given when the host's queue is
/// full.
pub const IOCTL_WRITE: u32 = code(0x801);
/// Sets the lines toward the host. Input: a `u32` of [`lines`] bits.
pub const IOCTL_SET_LINES: u32 = code(0x802);
/// Waits for the next thing the host does. Output: an [`Event`]. One wait
/// at a time.
pub const IOCTL_WAIT_EVENT: u32 = code(0x803);
/// Reads the state of the host's side. Output: a [`State`].
pub const IOCTL_GET_STATE: u32 = code(0x804);
/// Sends a break toward the host.
pub const IOCTL_BREAK: u32 = code(0x805);

/// The bits of [`IOCTL_SET_LINES`].
pub mod lines {
    /// Clear to send.
    pub const CTS: u32 = 0x1;
    /// Data set ready.
    pub const DSR: u32 = 0x2;
    /// Ring indicator.
    pub const RI: u32 = 0x4;
    /// Data carrier detect.
    pub const DCD: u32 = 0x8;
    /// Every line.
    pub const ALL: u32 = 0xF;
}

/// Something the host did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// The host opened the port.
    HostOpened,
    /// The host closed the port.
    HostClosed,
    /// The host changed DTR.
    Dtr(bool),
    /// The host changed RTS.
    Rts(bool),
    /// The host changed a stored setting.
    Settings,
    /// The host purged a queue.
    Purge {
        /// The queue toward the host.
        input: bool,
        /// The queue from the host.
        output: bool,
    },
    /// The host began or ended a break.
    Break(bool),
    /// More happened than was kept: read the state.
    Lost,
}

impl Event {
    /// The length of an event on the wire: a kind and a value.
    pub const SIZE: usize = 8;

    /// The event as [`IOCTL_WAIT_EVENT`] gives it.
    pub fn encode(self) -> [u8; Event::SIZE] {
        let (kind, value): (u32, u32) = match self {
            Event::HostOpened => (1, 0),
            Event::HostClosed => (2, 0),
            Event::Dtr(on) => (3, u32::from(on)),
            Event::Rts(on) => (4, u32::from(on)),
            Event::Settings => (5, 0),
            Event::Purge { input, output } => (6, u32::from(input) | u32::from(output) << 1),
            Event::Break(on) => (7, u32::from(on)),
            Event::Lost => (8, 0),
        };
        let mut wire = [0; Event::SIZE];
        wire[..4].copy_from_slice(&kind.to_le_bytes());
        wire[4..].copy_from_slice(&value.to_le_bytes());
        wire
    }

    /// The event in what [`IOCTL_WAIT_EVENT`] gave, if it is one.
    pub fn decode(wire: &[u8]) -> Option<Event> {
        let kind = u32::from_le_bytes(wire.get(..4)?.try_into().ok()?);
        let value = u32::from_le_bytes(wire.get(4..8)?.try_into().ok()?);
        Some(match kind {
            1 => Event::HostOpened,
            2 => Event::HostClosed,
            3 => Event::Dtr(value != 0),
            4 => Event::Rts(value != 0),
            5 => Event::Settings,
            6 => Event::Purge {
                input: value & 1 != 0,
                output: value & 2 != 0,
            },
            7 => Event::Break(value != 0),
            8 => Event::Lost,
            _ => return None,
        })
    }
}

/// The state of the host's side of a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct State {
    /// The driver's [`VERSION`].
    pub version: u32,
    /// The host has the port open.
    pub host_open: bool,
    /// DTR from the host.
    pub dtr: bool,
    /// RTS from the host.
    pub rts: bool,
    /// The host is sending a break.
    pub break_on: bool,
    /// The baud rate the host stored.
    pub baud: u32,
    /// How many octets the port would take for the host now.
    pub input_free: u32,
    /// How many octets from the host wait to be read.
    pub output_waiting: u32,
}

impl State {
    /// The length of a state on the wire.
    pub const SIZE: usize = 20;

    /// The state as [`IOCTL_GET_STATE`] gives it.
    pub fn encode(&self) -> [u8; State::SIZE] {
        let flags = u32::from(self.host_open)
            | u32::from(self.dtr) << 1
            | u32::from(self.rts) << 2
            | u32::from(self.break_on) << 3;
        let mut wire = [0; State::SIZE];
        for (at, value) in [
            self.version,
            flags,
            self.baud,
            self.input_free,
            self.output_waiting,
        ]
        .into_iter()
        .enumerate()
        {
            wire[at * 4..at * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        wire
    }

    /// The state in what [`IOCTL_GET_STATE`] gave, if it is one.
    pub fn decode(wire: &[u8]) -> Option<State> {
        let word = |at: usize| {
            Some(u32::from_le_bytes(
                wire.get(at * 4..at * 4 + 4)?.try_into().ok()?,
            ))
        };
        let flags = word(1)?;
        Some(State {
            version: word(0)?,
            host_open: flags & 1 != 0,
            dtr: flags & 2 != 0,
            rts: flags & 4 != 0,
            break_on: flags & 8 != 0,
            baud: word(2)?,
            input_free: word(3)?,
            output_waiting: word(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_survives_the_wire() {
        let events = [
            Event::HostOpened,
            Event::HostClosed,
            Event::Dtr(true),
            Event::Dtr(false),
            Event::Rts(true),
            Event::Rts(false),
            Event::Settings,
            Event::Purge {
                input: true,
                output: false,
            },
            Event::Purge {
                input: false,
                output: true,
            },
            Event::Purge {
                input: true,
                output: true,
            },
            Event::Break(true),
            Event::Break(false),
            Event::Lost,
        ];
        for event in events {
            assert_eq!(Event::decode(&event.encode()), Some(event));
        }
        assert_eq!(Event::decode(&[0; 8]), None);
        assert_eq!(Event::decode(&[9, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(Event::decode(&[1, 0, 0]), None);
    }

    #[test]
    fn a_state_survives_the_wire() {
        for bits in 0..16u32 {
            let state = State {
                version: VERSION,
                host_open: bits & 1 != 0,
                dtr: bits & 2 != 0,
                rts: bits & 4 != 0,
                break_on: bits & 8 != 0,
                baud: 115_200 + bits,
                input_free: 4096 - bits,
                output_waiting: bits * 3,
            };
            assert_eq!(State::decode(&state.encode()), Some(state));
        }
        assert_eq!(State::decode(&[0; 19]), None);
    }

    #[test]
    fn the_codes_are_buffered_codes_of_one_vendor_device_type() {
        let codes = [
            IOCTL_READ,
            IOCTL_WRITE,
            IOCTL_SET_LINES,
            IOCTL_WAIT_EVENT,
            IOCTL_GET_STATE,
            IOCTL_BREAK,
        ];
        for (at, code) in codes.into_iter().enumerate() {
            assert_eq!(code >> 16, 0x8A51);
            assert_eq!(code & 3, 0, "METHOD_BUFFERED");
            assert_eq!((code >> 14) & 3, 0, "FILE_ANY_ACCESS");
            assert_eq!((code >> 2) & 0xFFF, 0x800 + at as u32);
        }
    }
}
