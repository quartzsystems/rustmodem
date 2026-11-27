// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What a host stores in a serial port and reads back, and the bit values
//! of the serial interface. The values are those of the Win32 headers,
//! where a host's program gets them.

/// The events of `SetCommMask` and `WaitCommEvent`.
pub mod events {
    /// A character was received.
    pub const RXCHAR: u32 = 0x0001;
    /// The event character was received.
    pub const RXFLAG: u32 = 0x0002;
    /// The last character of the output queue was sent.
    pub const TXEMPTY: u32 = 0x0004;
    /// CTS changed.
    pub const CTS: u32 = 0x0008;
    /// DSR changed.
    pub const DSR: u32 = 0x0010;
    /// DCD changed.
    pub const RLSD: u32 = 0x0020;
    /// A break was received.
    pub const BREAK: u32 = 0x0040;
    /// A line error. This port has no line and never reports one.
    pub const ERR: u32 = 0x0080;
    /// The ring indicator came on.
    pub const RING: u32 = 0x0100;
    /// A printer error. Never reported.
    pub const PERR: u32 = 0x0200;
    /// The input queue is four fifths full.
    pub const RX80FULL: u32 = 0x0400;
    /// Provider events. Never reported.
    pub const EVENT1: u32 = 0x0800;
    /// Provider events. Never reported.
    pub const EVENT2: u32 = 0x1000;
    /// Every bit a wait mask may have.
    pub const ALL: u32 = 0x1FFF;
}

/// The bits of `GetCommModemStatus`.
pub mod modem_status {
    /// CTS is on.
    pub const CTS: u32 = 0x0010;
    /// DSR is on.
    pub const DSR: u32 = 0x0020;
    /// The ring indicator is on.
    pub const RING: u32 = 0x0040;
    /// DCD is on.
    pub const RLSD: u32 = 0x0080;
}

/// The bits of the modem control register, as a host reads and writes it.
pub mod modem_control {
    /// DTR.
    pub const DTR: u32 = 0x01;
    /// RTS.
    pub const RTS: u32 = 0x02;
    /// OUT1. Stored and read back.
    pub const OUT1: u32 = 0x04;
    /// OUT2. Stored and read back.
    pub const OUT2: u32 = 0x08;
    /// Loopback. Stored and read back; nothing is looped.
    pub const LOOP: u32 = 0x10;
    /// Every bit the register has.
    pub const ALL: u32 = 0x1F;
}

/// The flags of `PurgeComm`.
pub mod purge {
    /// End every pending write.
    pub const TXABORT: u32 = 0x0001;
    /// End every pending read.
    pub const RXABORT: u32 = 0x0002;
    /// Empty the output queue.
    pub const TXCLEAR: u32 = 0x0004;
    /// Empty the input queue.
    pub const RXCLEAR: u32 = 0x0008;
    /// Every flag there is.
    pub const ALL: u32 = 0x000F;
}

/// `COMMTIMEOUTS`, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Timeouts {
    /// The longest time between two octets of one read.
    pub read_interval: u32,
    /// Multiplied by the octets a read asks for.
    pub read_total_multiplier: u32,
    /// Added to that product.
    pub read_total_constant: u32,
    /// Multiplied by the octets a write offers.
    pub write_total_multiplier: u32,
    /// Added to that product.
    pub write_total_constant: u32,
}

/// Stop bits, parity and word length. Stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineControl {
    /// 0 is one stop bit, 1 is one and a half, 2 is two.
    pub stop_bits: u8,
    /// 0 none, 1 odd, 2 even, 3 mark, 4 space.
    pub parity: u8,
    /// Five to eight.
    pub word_length: u8,
}

impl Default for LineControl {
    fn default() -> LineControl {
        LineControl {
            stop_bits: 0,
            parity: 0,
            word_length: 8,
        }
    }
}

/// The special characters. Only the event character does anything: it is
/// what `EV_RXFLAG` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chars {
    /// End of file.
    pub eof: u8,
    /// Put in place of a character with an error.
    pub error: u8,
    /// Put in at a break.
    pub break_char: u8,
    /// The event character.
    pub event: u8,
    /// XON.
    pub xon: u8,
    /// XOFF.
    pub xoff: u8,
}

impl Default for Chars {
    fn default() -> Chars {
        Chars {
            eof: 0,
            error: 0,
            break_char: 0,
            event: 0,
            xon: 0x11,
            xoff: 0x13,
        }
    }
}

/// The flow control settings. Stored; of all their bits only the modes of
/// DTR and RTS do anything, which is to put the line on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandFlow {
    /// `ControlHandShake`.
    pub control_handshake: u32,
    /// `FlowReplace`.
    pub flow_replace: u32,
    /// `XonLimit`.
    pub xon_limit: i32,
    /// `XoffLimit`.
    pub xoff_limit: i32,
}

impl HandFlow {
    /// The bits of `ControlHandShake` that say what DTR does.
    pub const DTR_MASK: u32 = 0x03;
    /// DTR is on while the port is open.
    pub const DTR_CONTROL: u32 = 0x01;
    /// DTR is input flow control.
    pub const DTR_HANDSHAKE: u32 = 0x02;
    /// The bits of `FlowReplace` that say what RTS does.
    pub const RTS_MASK: u32 = 0xC0;
    /// RTS is on while the port is open.
    pub const RTS_CONTROL: u32 = 0x40;
    /// RTS is input flow control.
    pub const RTS_HANDSHAKE: u32 = 0x80;
    /// RTS is on while there is something to send.
    pub const TRANSMIT_TOGGLE: u32 = 0xC0;
}

impl Default for HandFlow {
    /// A port opens with DTR and RTS on, as a serial port of Windows does.
    fn default() -> HandFlow {
        HandFlow {
            control_handshake: HandFlow::DTR_CONTROL,
            flow_replace: HandFlow::RTS_CONTROL,
            xon_limit: 0,
            xoff_limit: 0,
        }
    }
}

/// The sizes a host asked for its queues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QueueSize {
    /// Toward the host.
    pub input: u32,
    /// From the host.
    pub output: u32,
}

/// What `ClearCommError` reports of the queues. There are no errors and
/// nothing ever holds transmission.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CommStatus {
    /// Octets the host has not read.
    pub in_queue: u32,
    /// Octets the host wrote that the control side has not taken.
    pub out_queue: u32,
}

/// The counts a host can read and clear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Stats {
    /// Octets that arrived for the host.
    pub received: u32,
    /// Octets the host sent.
    pub transmitted: u32,
}
