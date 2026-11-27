// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The port's two interfaces as Windows asks for them: an I/O control
//! code, an input buffer and the length of an output buffer, answered
//! with a status and octets. The driver gives every device I/O control
//! request to [`host_ioctl`] or [`control_ioctl`] and does as the
//! [`Outcome`] says, so the codes a port accepts, the layouts it reads and
//! writes and the status each mistake gets are decided here, where they
//! are tested with no framework.
//!
//! The host's codes and layouts are those of the serial interface of
//! Windows (`ntddser.h`), where a host's program gets them through
//! `SetCommState` and its neighbours. A code this port does not have is
//! refused with `STATUS_INVALID_PARAMETER`, which is what the serial
//! driver of Windows answers to a code it does not know.

use vcomapi::{Event, State, lines};

use crate::port::{Completion, ControlEvent, Done, Fault, Millis, ModemLines, Port, RequestId};
use crate::settings::{Chars, HandFlow, LineControl, QueueSize, Timeouts};

/// The statuses a port gives.
pub mod status {
    /// Done.
    pub const SUCCESS: u32 = 0;
    /// A request's time ran out. A success, with what was done.
    pub const TIMEOUT: u32 = 0x0000_0102;
    /// A value the port cannot have, or a code it does not have.
    pub const INVALID_PARAMETER: u32 = 0xC000_000D;
    /// The side is already open, or the caller may not open it.
    pub const ACCESS_DENIED: u32 = 0xC000_0022;
    /// A buffer is shorter than what it is to hold.
    pub const BUFFER_TOO_SMALL: u32 = 0xC000_0023;
    /// The request was cancelled.
    pub const CANCELLED: u32 = 0xC000_0120;
    /// The side the request is for is not open.
    pub const INVALID_DEVICE_STATE: u32 = 0xC000_0184;
}

/// The device type of a serial port, in an I/O control code.
const FILE_DEVICE_SERIAL_PORT: u32 = 0x1b;

/// The functions of the serial I/O control codes.
mod serial {
    pub const SET_BAUD_RATE: u32 = 1;
    pub const SET_QUEUE_SIZE: u32 = 2;
    pub const SET_LINE_CONTROL: u32 = 3;
    pub const SET_BREAK_ON: u32 = 4;
    pub const SET_BREAK_OFF: u32 = 5;
    pub const IMMEDIATE_CHAR: u32 = 6;
    pub const SET_TIMEOUTS: u32 = 7;
    pub const GET_TIMEOUTS: u32 = 8;
    pub const SET_DTR: u32 = 9;
    pub const CLR_DTR: u32 = 10;
    pub const RESET_DEVICE: u32 = 11;
    pub const SET_RTS: u32 = 12;
    pub const CLR_RTS: u32 = 13;
    pub const SET_XOFF: u32 = 14;
    pub const SET_XON: u32 = 15;
    pub const GET_WAIT_MASK: u32 = 16;
    pub const SET_WAIT_MASK: u32 = 17;
    pub const WAIT_ON_MASK: u32 = 18;
    pub const PURGE: u32 = 19;
    pub const GET_BAUD_RATE: u32 = 20;
    pub const GET_LINE_CONTROL: u32 = 21;
    pub const GET_CHARS: u32 = 22;
    pub const SET_CHARS: u32 = 23;
    pub const GET_HANDFLOW: u32 = 24;
    pub const SET_HANDFLOW: u32 = 25;
    pub const GET_MODEMSTATUS: u32 = 26;
    pub const GET_COMMSTATUS: u32 = 27;
    pub const GET_PROPERTIES: u32 = 29;
    pub const GET_DTRRTS: u32 = 30;
    pub const CONFIG_SIZE: u32 = 32;
    pub const GET_STATS: u32 = 35;
    pub const CLEAR_STATS: u32 = 36;
    pub const GET_MODEM_CONTROL: u32 = 37;
    pub const SET_MODEM_CONTROL: u32 = 38;
}

/// The serial I/O control code of a function: buffered, any access.
pub const fn serial_code(function: u32) -> u32 {
    (FILE_DEVICE_SERIAL_PORT << 16) | (function << 2)
}

/// What became of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Complete it now, with this status and these octets.
    Done {
        /// An `NTSTATUS`.
        status: u32,
        /// What goes in the output buffer. Never longer than it.
        data: Vec<u8>,
    },
    /// The port has it, under the id it was given with. It ends as a
    /// [`Completion`].
    Pending,
}

impl Outcome {
    fn ok() -> Outcome {
        Outcome::Done {
            status: status::SUCCESS,
            data: Vec::new(),
        }
    }

    fn fail(status: u32) -> Outcome {
        Outcome::Done {
            status,
            data: Vec::new(),
        }
    }

    /// Success with these octets, if the output buffer holds them.
    fn give(data: Vec<u8>, output: usize) -> Outcome {
        if output < data.len() {
            Outcome::fail(status::BUFFER_TOO_SMALL)
        } else {
            Outcome::Done {
                status: status::SUCCESS,
                data,
            }
        }
    }

    fn of(result: Result<(), Fault>) -> Outcome {
        match result {
            Ok(()) => Outcome::ok(),
            Err(fault) => Outcome::fail(fault_status(fault)),
        }
    }

    fn pending(result: Result<(), Fault>) -> Outcome {
        match result {
            Ok(()) => Outcome::Pending,
            Err(fault) => Outcome::fail(fault_status(fault)),
        }
    }
}

/// The status a refusal has.
pub fn fault_status(fault: Fault) -> u32 {
    match fault {
        Fault::InvalidParameter => status::INVALID_PARAMETER,
        Fault::Busy => status::ACCESS_DENIED,
        Fault::NotOpen => status::INVALID_DEVICE_STATE,
    }
}

/// The end of a request, as the driver completes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// The request.
    pub id: RequestId,
    /// An `NTSTATUS`.
    pub status: u32,
    /// What goes in the request's output buffer.
    pub data: Vec<u8>,
    /// The count the request reports: the length of `data`, or for a
    /// write how many octets were sent.
    pub information: usize,
}

impl Completion {
    /// The completion as the driver completes the request.
    pub fn finish(self) -> Finished {
        let status_of = |done| match done {
            Done::Ok => status::SUCCESS,
            Done::TimedOut => status::TIMEOUT,
            Done::Cancelled => status::CANCELLED,
        };
        let with = |id, done, data: Vec<u8>| Finished {
            id,
            status: status_of(done),
            information: data.len(),
            data,
        };
        match self {
            Completion::HostRead { id, done, data }
            | Completion::ControlRead { id, done, data } => with(id, done, data),
            Completion::HostWrite { id, done, written } => Finished {
                id,
                status: status_of(done),
                data: Vec::new(),
                information: written,
            },
            Completion::HostWait { id, done, events } => {
                let data = if done == Done::Cancelled {
                    Vec::new()
                } else {
                    events.to_le_bytes().to_vec()
                };
                with(id, done, data)
            }
            Completion::ControlWait { id, done, event } => {
                let data = event.map(|e| wire_event(e).encode().to_vec());
                with(id, done, data.unwrap_or_default())
            }
        }
    }
}

fn wire_event(event: ControlEvent) -> Event {
    match event {
        ControlEvent::HostOpened => Event::HostOpened,
        ControlEvent::HostClosed => Event::HostClosed,
        ControlEvent::Dtr(on) => Event::Dtr(on),
        ControlEvent::Rts(on) => Event::Rts(on),
        ControlEvent::Settings => Event::Settings,
        ControlEvent::Purge { input, output } => Event::Purge { input, output },
        ControlEvent::Break(on) => Event::Break(on),
        ControlEvent::Lost => Event::Lost,
    }
}

/// Reads little-endian fields out of an input buffer.
struct Fields<'a>(&'a [u8]);

impl Fields<'_> {
    fn u32(&mut self) -> u32 {
        let (head, rest) = self.0.split_at(4);
        self.0 = rest;
        u32::from_le_bytes([head[0], head[1], head[2], head[3]])
    }

    fn u8(&mut self) -> u8 {
        let (head, rest) = self.0.split_at(1);
        self.0 = rest;
        head[0]
    }
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// `SERIAL_COMMPROP`: what the port says it can do. It can be set to
/// anything and does none of it, so it says yes to everything a serial
/// port may have.
fn properties(port: &Port) -> Vec<u8> {
    let (input, output) = port.queue_capacity();
    let mut data = Vec::with_capacity(64);
    data.extend(64u16.to_le_bytes()); // PacketLength
    data.extend(2u16.to_le_bytes()); // PacketVersion
    data.extend(words(&[
        0x0000_0001, // ServiceMask: SP_SERIALCOMM
        0,           // Reserved1
        0,           // MaxTxQueue: no limit
        0,           // MaxRxQueue: no limit
        0x1000_0000, // MaxBaud: BAUD_USER, any rate
        0x0000_0001, // ProvSubType: PST_RS232
        0x0000_01FF, // ProvCapabilities: every one there is
        0x0000_007F, // SettableParams: every one there is
        0x1007_FFFF, // SettableBaud: every rate named, and any other
    ]));
    data.extend(0x000Fu16.to_le_bytes()); // SettableData: 5 to 8 bits
    data.extend(0x1F07u16.to_le_bytes()); // SettableStopParity: all
    data.extend(words(&[output, input, 0, 0])); // Current queues, ProvSpec
    data.extend([0; 4]); // ProvChar, and padding
    data
}

/// A device I/O control request from the host's side.
pub fn host_ioctl(
    port: &mut Port,
    id: RequestId,
    code: u32,
    input: &[u8],
    output: usize,
    now: Millis,
) -> Outcome {
    // Any device type but a serial port's, and any method but buffered,
    // is a code this port does not have.
    if code >> 16 != FILE_DEVICE_SERIAL_PORT || code & 3 != 0 {
        return Outcome::fail(status::INVALID_PARAMETER);
    }
    let function = (code >> 2) & 0xFFF;
    let needs = match function {
        serial::SET_BAUD_RATE
        | serial::SET_WAIT_MASK
        | serial::PURGE
        | serial::SET_MODEM_CONTROL => 4,
        serial::SET_QUEUE_SIZE => 8,
        serial::SET_LINE_CONTROL => 3,
        serial::IMMEDIATE_CHAR => 1,
        serial::SET_TIMEOUTS => 20,
        serial::SET_CHARS => 6,
        serial::SET_HANDFLOW => 16,
        _ => 0,
    };
    if input.len() < needs {
        return Outcome::fail(status::BUFFER_TOO_SMALL);
    }
    let mut fields = Fields(input);
    match function {
        serial::SET_BAUD_RATE => Outcome::of(port.set_baud(fields.u32(), now)),
        serial::GET_BAUD_RATE => Outcome::give(words(&[port.baud()]), output),
        serial::SET_QUEUE_SIZE => {
            port.set_queue_size(QueueSize {
                input: fields.u32(),
                output: fields.u32(),
            });
            Outcome::ok()
        }
        serial::SET_LINE_CONTROL => {
            let value = LineControl {
                stop_bits: fields.u8(),
                parity: fields.u8(),
                word_length: fields.u8(),
            };
            Outcome::of(port.set_line_control(value, now))
        }
        serial::GET_LINE_CONTROL => {
            let value = port.line_control();
            Outcome::give(
                vec![value.stop_bits, value.parity, value.word_length],
                output,
            )
        }
        serial::SET_BREAK_ON | serial::SET_BREAK_OFF => {
            port.set_break(function == serial::SET_BREAK_ON, now);
            Outcome::ok()
        }
        serial::IMMEDIATE_CHAR => Outcome::of(port.transmit_immediate(fields.u8(), now)),
        serial::SET_TIMEOUTS => {
            let value = Timeouts {
                read_interval: fields.u32(),
                read_total_multiplier: fields.u32(),
                read_total_constant: fields.u32(),
                write_total_multiplier: fields.u32(),
                write_total_constant: fields.u32(),
            };
            Outcome::of(port.set_timeouts(value))
        }
        serial::GET_TIMEOUTS => {
            let t = port.timeouts();
            let data = words(&[
                t.read_interval,
                t.read_total_multiplier,
                t.read_total_constant,
                t.write_total_multiplier,
                t.write_total_constant,
            ]);
            Outcome::give(data, output)
        }
        serial::SET_DTR | serial::CLR_DTR => {
            Outcome::of(port.set_dtr(function == serial::SET_DTR, now))
        }
        serial::SET_RTS | serial::CLR_RTS => {
            Outcome::of(port.set_rts(function == serial::SET_RTS, now))
        }
        serial::GET_DTRRTS => {
            let (dtr, rts) = port.dtr_rts();
            Outcome::give(words(&[u32::from(dtr) | u32::from(rts) << 1]), output)
        }
        // There is no device to reset and no flow to hold or let go.
        serial::RESET_DEVICE | serial::SET_XOFF | serial::SET_XON => Outcome::ok(),
        serial::GET_WAIT_MASK => Outcome::give(words(&[port.wait_mask()]), output),
        serial::SET_WAIT_MASK => Outcome::of(port.set_wait_mask(fields.u32(), now)),
        serial::WAIT_ON_MASK => {
            if output < 4 {
                return Outcome::fail(status::BUFFER_TOO_SMALL);
            }
            Outcome::pending(port.host_wait(id, now))
        }
        serial::PURGE => Outcome::of(port.purge(fields.u32(), now)),
        serial::SET_CHARS => {
            let value = Chars {
                eof: fields.u8(),
                error: fields.u8(),
                break_char: fields.u8(),
                event: fields.u8(),
                xon: fields.u8(),
                xoff: fields.u8(),
            };
            Outcome::of(port.set_chars(value, now))
        }
        serial::GET_CHARS => {
            let c = port.chars();
            let data = vec![c.eof, c.error, c.break_char, c.event, c.xon, c.xoff];
            Outcome::give(data, output)
        }
        serial::SET_HANDFLOW => {
            let value = HandFlow {
                control_handshake: fields.u32(),
                flow_replace: fields.u32(),
                xon_limit: fields.u32() as i32,
                xoff_limit: fields.u32() as i32,
            };
            Outcome::of(port.set_handflow(value, now))
        }
        serial::GET_HANDFLOW => {
            let h = port.handflow();
            let data = words(&[
                h.control_handshake,
                h.flow_replace,
                h.xon_limit as u32,
                h.xoff_limit as u32,
            ]);
            Outcome::give(data, output)
        }
        serial::GET_MODEMSTATUS => Outcome::give(words(&[port.modem_status()]), output),
        serial::GET_COMMSTATUS => {
            // SERIAL_STATUS: errors, hold reasons, the two queues, and
            // two flags with their padding. No error, nothing held.
            let queues = port.comm_status();
            let mut data = words(&[0, 0, queues.in_queue, queues.out_queue]);
            data.extend([0; 4]);
            Outcome::give(data, output)
        }
        serial::GET_PROPERTIES => Outcome::give(properties(port), output),
        // No configuration of a provider's own.
        serial::CONFIG_SIZE => Outcome::give(words(&[0]), output),
        serial::GET_STATS => {
            // SERIALPERF_STATS: received, transmitted, and four counts of
            // errors a port with no line never has.
            let stats = port.stats();
            Outcome::give(
                words(&[stats.received, stats.transmitted, 0, 0, 0, 0]),
                output,
            )
        }
        serial::CLEAR_STATS => {
            port.clear_stats();
            Outcome::ok()
        }
        serial::GET_MODEM_CONTROL => Outcome::give(words(&[port.modem_control()]), output),
        serial::SET_MODEM_CONTROL => Outcome::of(port.set_modem_control(fields.u32(), now)),
        _ => Outcome::fail(status::INVALID_PARAMETER),
    }
}

/// A device I/O control request from the control side.
pub fn control_ioctl(
    port: &mut Port,
    id: RequestId,
    code: u32,
    input: &[u8],
    output: usize,
    now: Millis,
) -> Outcome {
    match code {
        vcomapi::IOCTL_READ => Outcome::pending(port.control_read(id, output, now)),
        vcomapi::IOCTL_WRITE => {
            if output < 4 {
                return Outcome::fail(status::BUFFER_TOO_SMALL);
            }
            match port.control_write(input, now) {
                Ok(taken) => Outcome::give(words(&[taken as u32]), output),
                Err(fault) => Outcome::fail(fault_status(fault)),
            }
        }
        vcomapi::IOCTL_SET_LINES => {
            if input.len() < 4 {
                return Outcome::fail(status::BUFFER_TOO_SMALL);
            }
            let bits = Fields(input).u32();
            if bits & !lines::ALL != 0 {
                return Outcome::fail(status::INVALID_PARAMETER);
            }
            let value = ModemLines {
                cts: bits & lines::CTS != 0,
                dsr: bits & lines::DSR != 0,
                ri: bits & lines::RI != 0,
                dcd: bits & lines::DCD != 0,
            };
            Outcome::of(port.control_set_lines(value, now))
        }
        vcomapi::IOCTL_WAIT_EVENT => {
            if output < Event::SIZE {
                return Outcome::fail(status::BUFFER_TOO_SMALL);
            }
            Outcome::pending(port.control_wait(id, now))
        }
        vcomapi::IOCTL_GET_STATE => {
            let state = port.control_state();
            let wire = State {
                version: vcomapi::VERSION,
                host_open: state.host_open,
                dtr: state.dtr,
                rts: state.rts,
                break_on: state.break_on,
                baud: state.baud,
                input_free: state.input_free,
                output_waiting: state.output_waiting,
            };
            Outcome::give(wire.encode().to_vec(), output)
        }
        vcomapi::IOCTL_BREAK => Outcome::of(port.control_break(now)),
        _ => Outcome::fail(status::INVALID_PARAMETER),
    }
}
