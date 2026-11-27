// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The port.
//!
//! Every call that can move an octet or end a request is given the time,
//! in milliseconds on any clock that does not go back. The port first ends
//! what that time has timed out, then does what it was asked, then moves
//! every octet that can move. Requests that wait are named by a
//! [`RequestId`] of the caller's choosing and end as a [`Completion`],
//! which the caller takes with [`Port::take_completion`]; a request that
//! can be answered at once ends that way too, so the caller has one path.
//! [`Port::next_deadline`] says when the port next wants to be called with
//! nothing else to say, which is [`Port::advance`].

use std::collections::VecDeque;

use crate::settings::{
    Chars, CommStatus, HandFlow, LineControl, QueueSize, Stats, Timeouts, events, modem_control,
    modem_status, purge,
};

/// Milliseconds on the caller's clock.
pub type Millis = u64;

/// The value of a timeout that Win32 gives a meaning of its own.
const MAXDWORD: u32 = u32::MAX;

/// The size of each queue until the host asks for another.
const DEFAULT_QUEUE: usize = 4096;
/// The least and the greatest size a host can have for a queue.
const MIN_QUEUE: usize = 64;
const MAX_QUEUE: usize = 65536;
/// How many events can wait for the control side before it is told that
/// it has missed some and must read the state.
const MAX_EVENTS: usize = 256;

/// The caller's name for a request that may wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// How a request that waited ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Done {
    /// It was carried out.
    Ok,
    /// Its time ran out. What it has is what was done by then, and to a
    /// host's program this is a success with fewer octets.
    TimedOut,
    /// It was cancelled, by its caller, by a purge or by a close.
    Cancelled,
}

/// Why a request was refused at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fault {
    /// What a serial port answers to a value it cannot have, and to a
    /// second wait while one is pending.
    InvalidParameter,
    /// The side is already open.
    Busy,
    /// The side is not open.
    NotOpen,
}

/// The lines a modem drives toward its host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModemLines {
    /// Clear to send.
    pub cts: bool,
    /// Data set ready.
    pub dsr: bool,
    /// Ring indicator.
    pub ri: bool,
    /// Data carrier detect.
    pub dcd: bool,
}

/// Something the host did, for the control side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlEvent {
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
    /// More happened than was kept. Read the state.
    Lost,
}

/// What the control side can ask of the port at any time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ControlState {
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

/// The end of a request that was given to the port with a [`RequestId`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    /// A host's read.
    HostRead {
        /// The request.
        id: RequestId,
        /// How it ended.
        done: Done,
        /// What it read.
        data: Vec<u8>,
    },
    /// A host's write.
    HostWrite {
        /// The request.
        id: RequestId,
        /// How it ended.
        done: Done,
        /// How many of its octets were sent.
        written: usize,
    },
    /// A host's wait for an event.
    HostWait {
        /// The request.
        id: RequestId,
        /// How it ended.
        done: Done,
        /// The events, or zero where a new mask ended the wait.
        events: u32,
    },
    /// The control side's read of what the host wrote.
    ControlRead {
        /// The request.
        id: RequestId,
        /// How it ended.
        done: Done,
        /// What it read.
        data: Vec<u8>,
    },
    /// The control side's wait for what the host does.
    ControlWait {
        /// The request.
        id: RequestId,
        /// How it ended.
        done: Done,
        /// What the host did, if the wait was not cancelled.
        event: Option<ControlEvent>,
    },
}

struct Read {
    id: RequestId,
    want: usize,
    got: Vec<u8>,
    started: bool,
    /// Ends with whatever is there, at once.
    immediate: bool,
    /// Ends as soon as it has anything.
    first_arrival: bool,
    total: Option<Millis>,
    interval: Option<u64>,
    idle_until: Option<Millis>,
}

struct Write {
    id: RequestId,
    data: Vec<u8>,
    sent: usize,
    started: bool,
    deadline: Option<Millis>,
}

/// One virtual COM port.
pub struct Port {
    host_open: bool,
    control_open: bool,

    /// Toward the host.
    rx: VecDeque<u8>,
    rx_cap: usize,
    /// From the host.
    tx: VecDeque<u8>,
    tx_cap: usize,
    /// Something was sent since the output queue was last reported empty.
    tx_active: bool,

    reads: VecDeque<Read>,
    writes: VecDeque<Write>,
    wait: Option<RequestId>,
    mask: u32,
    history: u32,

    control_reads: VecDeque<(RequestId, usize)>,
    control_wait: Option<RequestId>,
    control_events: VecDeque<ControlEvent>,

    lines: ModemLines,
    dtr: bool,
    rts: bool,
    out_bits: u32,
    break_on: bool,

    baud: u32,
    line_control: LineControl,
    chars: Chars,
    handflow: HandFlow,
    timeouts: Timeouts,
    queue_size: QueueSize,
    stats: Stats,

    completions: VecDeque<Completion>,
}

impl Default for Port {
    fn default() -> Port {
        Port::new()
    }
}

impl Port {
    /// A port with neither side open.
    pub fn new() -> Port {
        Port {
            host_open: false,
            control_open: false,
            rx: VecDeque::new(),
            rx_cap: DEFAULT_QUEUE,
            tx: VecDeque::new(),
            tx_cap: DEFAULT_QUEUE,
            tx_active: false,
            reads: VecDeque::new(),
            writes: VecDeque::new(),
            wait: None,
            mask: 0,
            history: 0,
            control_reads: VecDeque::new(),
            control_wait: None,
            control_events: VecDeque::new(),
            lines: ModemLines::default(),
            dtr: false,
            rts: false,
            out_bits: 0,
            break_on: false,
            baud: 115_200,
            line_control: LineControl::default(),
            chars: Chars::default(),
            handflow: HandFlow::default(),
            timeouts: Timeouts::default(),
            queue_size: QueueSize {
                input: DEFAULT_QUEUE as u32,
                output: DEFAULT_QUEUE as u32,
            },
            stats: Stats::default(),
            completions: VecDeque::new(),
        }
    }

    /// The next request that ended, if any. Call until it gives `None`
    /// after every other call.
    pub fn take_completion(&mut self) -> Option<Completion> {
        self.completions.pop_front()
    }

    /// When the port next has a timeout to act on. Call [`Port::advance`]
    /// then.
    pub fn next_deadline(&self) -> Option<Millis> {
        let read =
            self.reads
                .front()
                .filter(|r| r.started)
                .and_then(|r| match (r.total, r.idle_until) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                });
        let write = self
            .writes
            .front()
            .filter(|w| w.started)
            .and_then(|w| w.deadline);
        match (read, write) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Tells the port the time, with nothing else to say.
    pub fn advance(&mut self, now: Millis) {
        self.expire(now);
        self.run(now);
    }

    // ----- The host's side: opening and closing -----

    /// The host opens the port. It is one host at a time.
    pub fn host_open(&mut self, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if self.host_open {
            return Err(Fault::Busy);
        }
        self.host_open = true;
        self.mask = 0;
        self.history = 0;
        self.push_event(ControlEvent::HostOpened);
        // A serial port of Windows puts DTR and RTS on at the open where
        // the flow control settings say they are the host's to hold.
        let (dtr, rts) = lines_of(&self.handflow);
        self.change_dtr(dtr);
        self.change_rts(rts);
        self.run(now);
        Ok(())
    }

    /// The host closes the port: what it had waiting is cancelled, both
    /// queues are emptied, and DTR and RTS go off.
    pub fn host_close(&mut self, now: Millis) {
        self.expire(now);
        if !self.host_open {
            return;
        }
        while let Some(read) = self.reads.pop_front() {
            self.completions.push_back(Completion::HostRead {
                id: read.id,
                done: Done::Cancelled,
                data: Vec::new(),
            });
        }
        self.abort_writes();
        if let Some(id) = self.wait.take() {
            self.completions.push_back(Completion::HostWait {
                id,
                done: Done::Cancelled,
                events: 0,
            });
        }
        self.rx.clear();
        self.tx.clear();
        self.tx_active = false;
        self.mask = 0;
        self.history = 0;
        self.change_dtr(false);
        self.change_rts(false);
        if self.break_on {
            self.break_on = false;
            self.push_event(ControlEvent::Break(false));
        }
        self.host_open = false;
        self.push_event(ControlEvent::HostClosed);
        self.run(now);
    }

    /// Whether the host has the port open.
    pub fn is_host_open(&self) -> bool {
        self.host_open
    }

    // ----- The host's side: octets -----

    /// A host's read of up to `want` octets. It ends as the port's
    /// timeouts, as they are when the read comes to the head of the
    /// queue of reads, say it does.
    pub fn host_read(&mut self, id: RequestId, want: usize, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.host_open {
            return Err(Fault::NotOpen);
        }
        self.reads.push_back(Read {
            id,
            want,
            got: Vec::new(),
            started: false,
            immediate: false,
            first_arrival: false,
            total: None,
            interval: None,
            idle_until: None,
        });
        self.run(now);
        Ok(())
    }

    /// A host's write. It ends when every octet has gone into the queue
    /// the control side reads, or at its timeout with those that have.
    pub fn host_write(&mut self, id: RequestId, data: &[u8], now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.host_open {
            return Err(Fault::NotOpen);
        }
        self.writes.push_back(Write {
            id,
            data: data.to_vec(),
            sent: 0,
            started: false,
            deadline: None,
        });
        self.run(now);
        Ok(())
    }

    /// One octet that goes ahead of everything waiting to be sent.
    pub fn transmit_immediate(&mut self, octet: u8, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.host_open {
            return Err(Fault::NotOpen);
        }
        if self.control_open {
            self.tx.push_front(octet);
        }
        self.stats.transmitted = self.stats.transmitted.wrapping_add(1);
        self.tx_active = true;
        self.run(now);
        Ok(())
    }

    /// Cancels a request of either side that is still waiting. A read
    /// that is cancelled gives back what it had taken, so no octet is
    /// lost with it.
    pub fn cancel(&mut self, id: RequestId, now: Millis) {
        self.expire(now);
        if let Some(at) = self.reads.iter().position(|r| r.id == id) {
            if let Some(read) = self.reads.remove(at) {
                self.give_back(read);
            }
        } else if let Some(at) = self.writes.iter().position(|w| w.id == id) {
            if let Some(write) = self.writes.remove(at) {
                self.completions.push_back(Completion::HostWrite {
                    id,
                    done: Done::Cancelled,
                    written: write.sent,
                });
            }
        } else if self.wait == Some(id) {
            self.wait = None;
            self.completions.push_back(Completion::HostWait {
                id,
                done: Done::Cancelled,
                events: 0,
            });
        } else if let Some(at) = self.control_reads.iter().position(|r| r.0 == id) {
            self.control_reads.remove(at);
            self.completions.push_back(Completion::ControlRead {
                id,
                done: Done::Cancelled,
                data: Vec::new(),
            });
        } else if self.control_wait == Some(id) {
            self.control_wait = None;
            self.completions.push_back(Completion::ControlWait {
                id,
                done: Done::Cancelled,
                event: None,
            });
        }
        self.run(now);
    }

    /// `PurgeComm`.
    pub fn purge(&mut self, flags: u32, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if flags == 0 || flags & !purge::ALL != 0 {
            return Err(Fault::InvalidParameter);
        }
        if flags & purge::TXABORT != 0 {
            self.abort_writes();
        }
        if flags & purge::RXABORT != 0 {
            // Last first, so that what the head had taken is at the front
            // of the queue again.
            while let Some(read) = self.reads.pop_back() {
                self.give_back(read);
            }
        }
        let output = flags & purge::TXCLEAR != 0;
        let input = flags & purge::RXCLEAR != 0;
        if output {
            self.tx.clear();
            self.tx_active = false;
        }
        if input {
            self.rx.clear();
        }
        if input || output {
            self.push_event(ControlEvent::Purge { input, output });
        }
        self.run(now);
        Ok(())
    }

    // ----- The host's side: events -----

    /// `SetCommMask`. A wait that is pending ends with no events, as a
    /// serial port ends it.
    pub fn set_wait_mask(&mut self, mask: u32, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if mask & !events::ALL != 0 {
            return Err(Fault::InvalidParameter);
        }
        if let Some(id) = self.wait.take() {
            self.completions.push_back(Completion::HostWait {
                id,
                done: Done::Ok,
                events: 0,
            });
        }
        self.mask = mask;
        self.history &= mask;
        self.run(now);
        Ok(())
    }

    /// The mask of [`Port::set_wait_mask`].
    pub fn wait_mask(&self) -> u32 {
        self.mask
    }

    /// `WaitCommEvent`. One wait at a time, and none with an empty mask.
    pub fn host_wait(&mut self, id: RequestId, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.host_open {
            return Err(Fault::NotOpen);
        }
        if self.wait.is_some() || self.mask == 0 {
            return Err(Fault::InvalidParameter);
        }
        if self.history != 0 {
            let events = std::mem::take(&mut self.history);
            self.completions.push_back(Completion::HostWait {
                id,
                done: Done::Ok,
                events,
            });
        } else {
            self.wait = Some(id);
        }
        self.run(now);
        Ok(())
    }

    // ----- The host's side: lines -----

    /// `GetCommModemStatus`: the lines the control side holds.
    pub fn modem_status(&self) -> u32 {
        let mut bits = 0;
        if self.lines.cts {
            bits |= modem_status::CTS;
        }
        if self.lines.dsr {
            bits |= modem_status::DSR;
        }
        if self.lines.ri {
            bits |= modem_status::RING;
        }
        if self.lines.dcd {
            bits |= modem_status::RLSD;
        }
        bits
    }

    /// The host sets or clears DTR. Refused while DTR is flow control.
    pub fn set_dtr(&mut self, on: bool, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if self.handflow.control_handshake & HandFlow::DTR_MASK == HandFlow::DTR_HANDSHAKE {
            return Err(Fault::InvalidParameter);
        }
        self.change_dtr(on);
        self.run(now);
        Ok(())
    }

    /// The host sets or clears RTS. Refused while RTS is flow control.
    pub fn set_rts(&mut self, on: bool, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        let mode = self.handflow.flow_replace & HandFlow::RTS_MASK;
        if mode == HandFlow::RTS_HANDSHAKE || mode == HandFlow::TRANSMIT_TOGGLE {
            return Err(Fault::InvalidParameter);
        }
        self.change_rts(on);
        self.run(now);
        Ok(())
    }

    /// DTR and RTS as the host has them.
    pub fn dtr_rts(&self) -> (bool, bool) {
        (self.dtr, self.rts)
    }

    /// The modem control register.
    pub fn modem_control(&self) -> u32 {
        let mut bits = self.out_bits;
        if self.dtr {
            bits |= modem_control::DTR;
        }
        if self.rts {
            bits |= modem_control::RTS;
        }
        bits
    }

    /// Writes the modem control register: DTR and RTS, and three bits
    /// that are kept.
    pub fn set_modem_control(&mut self, bits: u32, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if bits & !modem_control::ALL != 0 {
            return Err(Fault::InvalidParameter);
        }
        self.out_bits = bits & !(modem_control::DTR | modem_control::RTS);
        self.change_dtr(bits & modem_control::DTR != 0);
        self.change_rts(bits & modem_control::RTS != 0);
        self.run(now);
        Ok(())
    }

    /// The host begins or ends a break.
    pub fn set_break(&mut self, on: bool, now: Millis) {
        self.expire(now);
        if self.break_on != on {
            self.break_on = on;
            self.push_event(ControlEvent::Break(on));
        }
        self.run(now);
    }

    // ----- The host's side: what is stored -----

    /// The baud rate. Stored.
    pub fn set_baud(&mut self, baud: u32, now: Millis) -> Result<(), Fault> {
        if baud == 0 {
            return Err(Fault::InvalidParameter);
        }
        if self.baud != baud {
            self.baud = baud;
            self.settings_changed(now);
        }
        Ok(())
    }

    /// The baud rate that was stored.
    pub fn baud(&self) -> u32 {
        self.baud
    }

    /// Stop bits, parity and word length. Stored.
    pub fn set_line_control(&mut self, value: LineControl, now: Millis) -> Result<(), Fault> {
        if value.stop_bits > 2 || value.parity > 4 || !(5..=8).contains(&value.word_length) {
            return Err(Fault::InvalidParameter);
        }
        if self.line_control != value {
            self.line_control = value;
            self.settings_changed(now);
        }
        Ok(())
    }

    /// What [`Port::set_line_control`] stored.
    pub fn line_control(&self) -> LineControl {
        self.line_control
    }

    /// The special characters. XON and XOFF may not be the same.
    pub fn set_chars(&mut self, value: Chars, now: Millis) -> Result<(), Fault> {
        if value.xon == value.xoff {
            return Err(Fault::InvalidParameter);
        }
        if self.chars != value {
            self.chars = value;
            self.settings_changed(now);
        }
        Ok(())
    }

    /// What [`Port::set_chars`] stored.
    pub fn chars(&self) -> Chars {
        self.chars
    }

    /// The flow control settings. A change of what DTR or RTS is for puts
    /// the line where the new setting has it.
    pub fn set_handflow(&mut self, value: HandFlow, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        let old = self.handflow;
        if old == value {
            return Ok(());
        }
        self.handflow = value;
        let (dtr, rts) = lines_of(&value);
        if old.control_handshake & HandFlow::DTR_MASK
            != value.control_handshake & HandFlow::DTR_MASK
        {
            self.change_dtr(dtr);
        }
        if old.flow_replace & HandFlow::RTS_MASK != value.flow_replace & HandFlow::RTS_MASK {
            self.change_rts(rts);
        }
        self.push_event(ControlEvent::Settings);
        self.run(now);
        Ok(())
    }

    /// What [`Port::set_handflow`] stored.
    pub fn handflow(&self) -> HandFlow {
        self.handflow
    }

    /// `SetCommTimeouts`. They govern the reads and writes that start
    /// after this.
    pub fn set_timeouts(&mut self, value: Timeouts) -> Result<(), Fault> {
        // As a serial port of Windows refuses it: an interval of MAXDWORD
        // asks for one of the immediate kinds of read, and none of them
        // has a total constant of MAXDWORD.
        if value.read_interval == MAXDWORD && value.read_total_constant == MAXDWORD {
            return Err(Fault::InvalidParameter);
        }
        self.timeouts = value;
        Ok(())
    }

    /// What [`Port::set_timeouts`] stored.
    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// `SetupComm`. Each queue is as large as asked, within the port's
    /// bounds; nothing in a queue is lost by making it smaller.
    pub fn set_queue_size(&mut self, value: QueueSize) {
        self.queue_size = value;
        self.rx_cap = (value.input as usize).clamp(MIN_QUEUE, MAX_QUEUE);
        self.tx_cap = (value.output as usize).clamp(MIN_QUEUE, MAX_QUEUE);
    }

    /// The sizes of the queues as they are: toward the host, and from it.
    pub fn queue_capacity(&self) -> (u32, u32) {
        (self.rx_cap as u32, self.tx_cap as u32)
    }

    /// What `ClearCommError` reports of the queues.
    pub fn comm_status(&self) -> CommStatus {
        let unsent: usize = self.writes.iter().map(|w| w.data.len() - w.sent).sum();
        CommStatus {
            in_queue: self.rx.len() as u32,
            out_queue: (self.tx.len() + unsent) as u32,
        }
    }

    /// The counts.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Clears the counts.
    pub fn clear_stats(&mut self) {
        self.stats = Stats::default();
    }

    // ----- The control side -----

    /// The service opens the port's control side. One client at a time.
    pub fn control_open(&mut self, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if self.control_open {
            return Err(Fault::Busy);
        }
        self.control_open = true;
        self.run(now);
        Ok(())
    }

    /// The service lets go. The port stays, with its lines low: a host
    /// that has it open sees a modem that is not there, and what it
    /// writes goes nowhere.
    pub fn control_close(&mut self, now: Millis) {
        self.expire(now);
        if !self.control_open {
            return;
        }
        while let Some((id, _)) = self.control_reads.pop_front() {
            self.completions.push_back(Completion::ControlRead {
                id,
                done: Done::Cancelled,
                data: Vec::new(),
            });
        }
        if let Some(id) = self.control_wait.take() {
            self.completions.push_back(Completion::ControlWait {
                id,
                done: Done::Cancelled,
                event: None,
            });
        }
        self.control_events.clear();
        self.tx.clear();
        self.apply_lines(ModemLines::default());
        self.control_open = false;
        self.run(now);
    }

    /// Whether the service has the control side open.
    pub fn is_control_open(&self) -> bool {
        self.control_open
    }

    /// Reads what the host wrote, up to `want` octets. Waits until there
    /// is something.
    pub fn control_read(&mut self, id: RequestId, want: usize, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.control_open {
            return Err(Fault::NotOpen);
        }
        self.control_reads.push_back((id, want));
        self.run(now);
        Ok(())
    }

    /// Octets for the host. Gives how many were taken, which is fewer
    /// than offered when the host's queue is full. With no host they are
    /// taken and go nowhere, as with a modem nobody listens to.
    pub fn control_write(&mut self, data: &[u8], now: Millis) -> Result<usize, Fault> {
        self.expire(now);
        if !self.control_open {
            return Err(Fault::NotOpen);
        }
        if !self.host_open {
            return Ok(data.len());
        }
        let before = self.rx.len();
        let take = data.len().min(self.rx_cap.saturating_sub(before));
        let taken = &data[..take];
        self.rx.extend(taken);
        self.stats.received = self.stats.received.wrapping_add(take as u32);
        if take > 0 {
            let mut raised = events::RXCHAR;
            if taken.contains(&self.chars.event) {
                raised |= events::RXFLAG;
            }
            let mark = self.rx_cap / 5 * 4;
            if before < mark && self.rx.len() >= mark {
                raised |= events::RX80FULL;
            }
            self.raise(raised);
        }
        self.run(now);
        Ok(take)
    }

    /// Sets CTS, DSR, RI and DCD toward the host.
    pub fn control_set_lines(&mut self, lines: ModemLines, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.control_open {
            return Err(Fault::NotOpen);
        }
        self.apply_lines(lines);
        self.run(now);
        Ok(())
    }

    /// A break toward the host.
    pub fn control_break(&mut self, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.control_open {
            return Err(Fault::NotOpen);
        }
        self.raise(events::BREAK);
        self.run(now);
        Ok(())
    }

    /// Waits for the next thing the host does. One wait at a time.
    pub fn control_wait(&mut self, id: RequestId, now: Millis) -> Result<(), Fault> {
        self.expire(now);
        if !self.control_open {
            return Err(Fault::NotOpen);
        }
        if self.control_wait.is_some() {
            return Err(Fault::InvalidParameter);
        }
        self.control_wait = Some(id);
        self.run(now);
        Ok(())
    }

    /// The state of the host's side, for the control side.
    pub fn control_state(&self) -> ControlState {
        ControlState {
            host_open: self.host_open,
            dtr: self.dtr,
            rts: self.rts,
            break_on: self.break_on,
            baud: self.baud,
            input_free: self.rx_cap.saturating_sub(self.rx.len()) as u32,
            output_waiting: self.tx.len() as u32,
        }
    }

    // ----- Inside -----

    fn settings_changed(&mut self, now: Millis) {
        self.expire(now);
        self.push_event(ControlEvent::Settings);
        self.run(now);
    }

    fn change_dtr(&mut self, on: bool) {
        if self.dtr != on {
            self.dtr = on;
            self.push_event(ControlEvent::Dtr(on));
        }
    }

    fn change_rts(&mut self, on: bool) {
        if self.rts != on {
            self.rts = on;
            self.push_event(ControlEvent::Rts(on));
        }
    }

    fn apply_lines(&mut self, lines: ModemLines) {
        let old = self.lines;
        self.lines = lines;
        let mut raised = 0;
        if old.cts != lines.cts {
            raised |= events::CTS;
        }
        if old.dsr != lines.dsr {
            raised |= events::DSR;
        }
        if old.dcd != lines.dcd {
            raised |= events::RLSD;
        }
        if !old.ri && lines.ri {
            raised |= events::RING;
        }
        self.raise(raised);
    }

    /// Reports events to the host: to its wait if it has one pending, and
    /// otherwise kept for its next. Only what its mask has is reported.
    fn raise(&mut self, raised: u32) {
        let raised = raised & self.mask;
        if raised == 0 {
            return;
        }
        match self.wait.take() {
            Some(id) => self.completions.push_back(Completion::HostWait {
                id,
                done: Done::Ok,
                events: raised,
            }),
            None => self.history |= raised,
        }
    }

    fn push_event(&mut self, event: ControlEvent) {
        if !self.control_open {
            return;
        }
        if self.control_events.len() >= MAX_EVENTS {
            self.control_events.clear();
            self.control_events.push_back(ControlEvent::Lost);
        } else {
            self.control_events.push_back(event);
        }
    }

    fn abort_writes(&mut self) {
        while let Some(write) = self.writes.pop_front() {
            self.completions.push_back(Completion::HostWrite {
                id: write.id,
                done: Done::Cancelled,
                written: write.sent,
            });
        }
    }

    /// Ends a read as cancelled and puts what it had taken back at the
    /// front of the queue it came from.
    fn give_back(&mut self, read: Read) {
        for &octet in read.got.iter().rev() {
            self.rx.push_front(octet);
        }
        self.completions.push_back(Completion::HostRead {
            id: read.id,
            done: Done::Cancelled,
            data: Vec::new(),
        });
    }

    /// Ends the read and the write at the head of their queues if their
    /// time has run out.
    fn expire(&mut self, now: Millis) {
        let read_out = self.reads.front().is_some_and(|r| {
            r.started
                && (r.total.is_some_and(|d| d <= now) || r.idle_until.is_some_and(|d| d <= now))
        });
        if read_out && let Some(read) = self.reads.pop_front() {
            self.completions.push_back(Completion::HostRead {
                id: read.id,
                done: Done::TimedOut,
                data: read.got,
            });
        }
        let write_out = self
            .writes
            .front()
            .is_some_and(|w| w.started && w.deadline.is_some_and(|d| d <= now));
        if write_out && let Some(write) = self.writes.pop_front() {
            self.completions.push_back(Completion::HostWrite {
                id: write.id,
                done: Done::TimedOut,
                written: write.sent,
            });
        }
    }

    /// Moves every octet that can move and ends every request that can
    /// end.
    fn run(&mut self, now: Millis) {
        loop {
            let mut progress = self.pump_writes(now);
            progress |= self.pump_control_reads();
            progress |= self.pump_reads(now);
            progress |= self.pump_control_wait();
            if !progress {
                break;
            }
        }
        if self.tx_active && self.tx.is_empty() && self.writes.is_empty() {
            self.tx_active = false;
            self.raise(events::TXEMPTY);
        }
    }

    fn pump_writes(&mut self, now: Millis) -> bool {
        let mut progress = false;
        while let Some(mut write) = self.writes.pop_front() {
            if !write.started {
                write.started = true;
                let t = &self.timeouts;
                if t.write_total_multiplier != 0 || t.write_total_constant != 0 {
                    let span = u64::from(t.write_total_multiplier)
                        .saturating_mul(write.data.len() as u64)
                        .saturating_add(u64::from(t.write_total_constant));
                    write.deadline = Some(now.saturating_add(span));
                }
            }
            let left = write.data.len() - write.sent;
            // With no control side the octets go nowhere, as down a line
            // with nothing on it: the write is not held.
            let take = if self.control_open {
                left.min(self.tx_cap.saturating_sub(self.tx.len()))
            } else {
                left
            };
            if take > 0 {
                if self.control_open {
                    self.tx.extend(&write.data[write.sent..write.sent + take]);
                }
                write.sent += take;
                self.stats.transmitted = self.stats.transmitted.wrapping_add(take as u32);
                self.tx_active = true;
                progress = true;
            }
            if write.sent == write.data.len() {
                self.completions.push_back(Completion::HostWrite {
                    id: write.id,
                    done: Done::Ok,
                    written: write.sent,
                });
                progress = true;
            } else {
                self.writes.push_front(write);
                break;
            }
        }
        progress
    }

    fn pump_control_reads(&mut self) -> bool {
        let mut progress = false;
        while let Some(&(id, want)) = self.control_reads.front() {
            if self.tx.is_empty() && want != 0 {
                break;
            }
            self.control_reads.pop_front();
            let take = want.min(self.tx.len());
            let data: Vec<u8> = self.tx.drain(..take).collect();
            self.completions.push_back(Completion::ControlRead {
                id,
                done: Done::Ok,
                data,
            });
            progress = true;
        }
        progress
    }

    fn pump_reads(&mut self, now: Millis) -> bool {
        let mut progress = false;
        while let Some(mut read) = self.reads.pop_front() {
            if !read.started {
                read.started = true;
                self.start_read(&mut read, now);
            }
            let take = (read.want - read.got.len()).min(self.rx.len());
            if take > 0 {
                read.got.extend(self.rx.drain(..take));
                if let Some(interval) = read.interval {
                    read.idle_until = Some(now.saturating_add(interval));
                }
                progress = true;
            }
            let full = read.got.len() == read.want;
            if full || read.immediate || (read.first_arrival && !read.got.is_empty()) {
                self.completions.push_back(Completion::HostRead {
                    id: read.id,
                    done: Done::Ok,
                    data: read.got,
                });
                progress = true;
            } else if read.total.is_some_and(|d| d <= now) {
                // A total timeout of no time at all.
                self.completions.push_back(Completion::HostRead {
                    id: read.id,
                    done: Done::TimedOut,
                    data: read.got,
                });
                progress = true;
            } else {
                self.reads.push_front(read);
                break;
            }
        }
        progress
    }

    /// Decides, from the timeouts as they are now, how a read ends. The
    /// cases are those of the documentation of `COMMTIMEOUTS`.
    fn start_read(&self, read: &mut Read, now: Millis) {
        let t = &self.timeouts;
        let total = |multiplier: u32| {
            let span = u64::from(multiplier)
                .saturating_mul(read.want as u64)
                .saturating_add(u64::from(t.read_total_constant));
            Some(now.saturating_add(span))
        };
        if t.read_interval == MAXDWORD {
            if t.read_total_multiplier == 0 && t.read_total_constant == 0 {
                // "The read operation is to return immediately with the
                // bytes that have already been received, even if no bytes
                // have been received."
                read.immediate = true;
            } else if t.read_total_multiplier == MAXDWORD {
                // "If there are no bytes in the input buffer, ReadFile
                // waits until a byte arrives and then returns
                // immediately. If no bytes arrive within the time
                // specified by ReadTotalTimeoutConstant, ReadFile times
                // out."
                read.first_arrival = true;
                read.total = total(0);
            } else {
                // The documentation has no words for an interval of
                // MAXDWORD with any other total. A serial port of Windows
                // returns with what is there, or waits out the total
                // timeout for the first of anything.
                read.first_arrival = true;
                read.total = total(t.read_total_multiplier);
            }
        } else {
            if t.read_total_multiplier != 0 || t.read_total_constant != 0 {
                read.total = total(t.read_total_multiplier);
            }
            if t.read_interval != 0 {
                // The interval is timed from the first octet of the read,
                // which pump_reads does by setting it at each arrival.
                read.interval = Some(u64::from(t.read_interval));
            }
        }
    }

    fn pump_control_wait(&mut self) -> bool {
        if self.control_wait.is_none() {
            return false;
        }
        let Some(event) = self.control_events.pop_front() else {
            return false;
        };
        if let Some(id) = self.control_wait.take() {
            self.completions.push_back(Completion::ControlWait {
                id,
                done: Done::Ok,
                event: Some(event),
            });
        }
        true
    }
}

/// Where the flow control settings put DTR and RTS.
fn lines_of(handflow: &HandFlow) -> (bool, bool) {
    let dtr = handflow.control_handshake & HandFlow::DTR_MASK != 0;
    let rts = matches!(
        handflow.flow_replace & HandFlow::RTS_MASK,
        HandFlow::RTS_CONTROL | HandFlow::RTS_HANDSHAKE
    );
    (dtr, rts)
}
