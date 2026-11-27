// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The vcom backend: the modem's side of a RustModem virtual COM port.
//!
//! A RustModem port is one device with two sides (`docs/DRIVER.md`). The
//! host opens it by its COM name. This backend opens the same device with
//! `\control` after the name and speaks the control interface of
//! `vcomapi`, which is made for it: octets each way, the four lines a
//! modem drives, and an event for everything the host does. So, unlike a
//! com0com pair, nothing is inferred and nothing depends on how a pair is
//! wired: the host opening and closing the port are events of their own,
//! and each line is itself.
//!
//! Two requests are kept waiting in the driver, a read of what the host
//! writes and a wait for what the host does, each on an `OVERLAPPED` of
//! its own; both signal one event, which is what the port's thread sleeps
//! on. Writes and the setting of lines are answered by the driver at
//! once.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io;

use msg::{Message, message};
use vcomapi::{Event, State, lines};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_NAME,
    ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_PATH_NOT_FOUND, GENERIC_READ, GENERIC_WRITE,
    HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, OPEN_EXISTING,
};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::core::{HSTRING, PCWSTR};

use crate::{ModemSignals, SerialBackend, SerialEvent, SerialOperation, WaitHandle, error_message};

/// The most octets asked of the driver in one read, and given it in one
/// write.
const CHUNK: usize = 4096;

/// A request that waits in the driver between calls.
struct Waiting {
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u8]>,
    pending: bool,
}

impl Waiting {
    fn new(size: usize) -> Waiting {
        Waiting {
            overlapped: Box::new(OVERLAPPED::default()),
            buffer: vec![0; size].into_boxed_slice(),
            pending: false,
        }
    }
}

/// The modem's side of a RustModem port.
pub struct VcomBackend {
    port: String,
    handle: HANDLE,
    /// For requests the driver answers at once.
    sync_overlapped: Box<OVERLAPPED>,
    sync_event: HANDLE,
    /// Signalled when either waiting request ends.
    wake_event: HANDLE,
    read: Waiting,
    wait: Waiting,
    /// What the host wrote and the pool has not taken.
    received: VecDeque<u8>,
    events: VecDeque<SerialEvent>,
    /// The host's side as last known, for when events were missed.
    host_open: bool,
    dtr: bool,
    rts: bool,
}

// SAFETY: the backend owns its handles and buffers outright and shares them
// with no other object, and Windows file and event handles may be used from
// any thread. The raw pointers inside `HANDLE` and `OVERLAPPED` are what
// stop the compiler from deriving this.
unsafe impl Send for VcomBackend {}

impl VcomBackend {
    /// Opens the control side of `port` (for example `COM10`).
    pub fn open(port: &str) -> Result<VcomBackend, Message> {
        let path = HSTRING::from(format!(r"\\.\{port}{}", vcomapi::CONTROL_NAME));
        // SAFETY: `path` is a valid, NUL-terminated wide string that lives
        // through the call; the other arguments are plain values.
        let opened = unsafe {
            CreateFileW(
                &path,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        };
        let handle = match opened {
            Ok(handle) => handle,
            Err(e) => {
                let missing = [
                    ERROR_FILE_NOT_FOUND,
                    ERROR_PATH_NOT_FOUND,
                    ERROR_INVALID_NAME,
                ]
                .iter()
                .any(|code| e.code() == code.to_hresult());
                return Err(if missing {
                    // There is no such port, or it is a port of another
                    // driver, which has no control side.
                    message("RMD9001", &[port])
                } else if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
                    message("RMD9002", &[port])
                } else {
                    error_message(port, SerialOperation::Open, &e.into())
                });
            }
        };

        let configure = |e: io::Error| error_message(port, SerialOperation::Configure, &e);
        let events = (|| -> io::Result<[HANDLE; 2]> { Ok([new_event()?, new_event()?]) })();
        let [sync_event, wake_event] = match events {
            Ok(events) => events,
            Err(e) => {
                // SAFETY: `handle` was returned by CreateFileW above and is
                // closed nowhere else.
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(configure(e));
            }
        };

        // From here the backend owns every handle and its Drop closes them.
        let mut backend = VcomBackend {
            port: port.to_string(),
            handle,
            sync_overlapped: Box::new(OVERLAPPED::default()),
            sync_event,
            wake_event,
            read: Waiting::new(CHUNK),
            wait: Waiting::new(Event::SIZE),
            received: VecDeque::new(),
            events: VecDeque::new(),
            host_open: false,
            dtr: false,
            rts: false,
        };
        let state = backend.state().map_err(configure)?;
        if state.version != vcomapi::VERSION {
            return Err(message(
                "RMD9003",
                &[
                    port,
                    &state.version.to_string(),
                    &vcomapi::VERSION.to_string(),
                ],
            ));
        }
        // Until the modem says otherwise, every line toward the host is off.
        backend
            .set_signals(ModemSignals::default())
            .map_err(configure)?;
        // What the host has done already is told as if it had just done it.
        backend.follow(state);
        backend.arm_read().map_err(configure)?;
        backend.arm_wait().map_err(configure)?;
        Ok(backend)
    }

    /// The port name this backend was opened with.
    pub fn port(&self) -> &str {
        &self.port
    }

    /// A request the driver answers at once. Gives how many octets it put
    /// in `output`.
    fn control(&mut self, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
        *self.sync_overlapped = OVERLAPPED {
            hEvent: self.sync_event,
            ..OVERLAPPED::default()
        };
        let mut moved = 0;
        // SAFETY: the buffers are the caller's slices, with their true
        // lengths, and live until the request has ended, which the wait
        // below is for; `sync_overlapped` is boxed and has not moved.
        unsafe {
            let started = DeviceIoControl(
                self.handle,
                code,
                (!input.is_empty()).then_some(input.as_ptr().cast::<c_void>()),
                input.len() as u32,
                (!output.is_empty()).then_some(output.as_mut_ptr().cast::<c_void>()),
                output.len() as u32,
                None,
                Some(&mut *self.sync_overlapped),
            );
            match started {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
                Err(e) => return Err(e.into()),
            }
            GetOverlappedResult(self.handle, &*self.sync_overlapped, &mut moved, true)?;
        }
        Ok(moved as usize)
    }

    fn state(&mut self) -> io::Result<State> {
        let mut wire = [0u8; State::SIZE];
        let length = self.control(vcomapi::IOCTL_GET_STATE, &[], &mut wire)?;
        State::decode(&wire[..length]).ok_or_else(|| io::ErrorKind::InvalidData.into())
    }

    /// Makes the events that take the host's side from what was known of
    /// it to `state`.
    fn follow(&mut self, state: State) {
        if state.host_open && !self.host_open {
            self.events.push_back(SerialEvent::HostOpened);
        }
        if state.dtr != self.dtr {
            self.events.push_back(SerialEvent::Dtr(state.dtr));
        }
        if state.rts != self.rts {
            self.events.push_back(SerialEvent::Rts(state.rts));
        }
        if !state.host_open && self.host_open {
            self.events.push_back(SerialEvent::HostClosed);
        }
        self.host_open = state.host_open;
        self.dtr = state.dtr;
        self.rts = state.rts;
    }

    /// Starts a request that waits in the driver.
    fn arm(handle: HANDLE, wake: HANDLE, code: u32, waiting: &mut Waiting) -> io::Result<()> {
        *waiting.overlapped = OVERLAPPED {
            hEvent: wake,
            ..OVERLAPPED::default()
        };
        // SAFETY: the buffer and the OVERLAPPED are boxed and owned by the
        // backend, so they stay where they are while the request is
        // outstanding; `pending` stops them being reused, and Drop cancels
        // the request and waits for it to end before freeing them.
        let started = unsafe {
            DeviceIoControl(
                handle,
                code,
                None,
                0,
                Some(waiting.buffer.as_mut_ptr().cast::<c_void>()),
                waiting.buffer.len() as u32,
                None,
                Some(&mut *waiting.overlapped),
            )
        };
        match started {
            // It ended at once. It is taken as any other that has ended,
            // and the port's thread is woken to do so.
            Ok(()) => {
                waiting.pending = true;
                // SAFETY: an event handle the backend owns.
                unsafe { SetEvent(wake) }?;
                Ok(())
            }
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                waiting.pending = true;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    fn arm_read(&mut self) -> io::Result<()> {
        VcomBackend::arm(
            self.handle,
            self.wake_event,
            vcomapi::IOCTL_READ,
            &mut self.read,
        )
    }

    fn arm_wait(&mut self) -> io::Result<()> {
        VcomBackend::arm(
            self.handle,
            self.wake_event,
            vcomapi::IOCTL_WAIT_EVENT,
            &mut self.wait,
        )
    }

    /// How many octets a waiting request ended with, if it has ended.
    fn taken(handle: HANDLE, waiting: &mut Waiting) -> io::Result<Option<usize>> {
        if !waiting.pending {
            return Ok(None);
        }
        let mut moved = 0;
        // SAFETY: `overlapped` is the OVERLAPPED the request was started
        // with, boxed so it has not moved, and `moved` is valid for the
        // call.
        let result =
            unsafe { GetOverlappedResult(handle, &*waiting.overlapped, &mut moved, false) };
        match result {
            Ok(()) => {
                waiting.pending = false;
                Ok(Some(moved as usize))
            }
            Err(e) if e.code() == ERROR_IO_INCOMPLETE.to_hresult() => Ok(None),
            Err(e) => {
                waiting.pending = false;
                Err(e.into())
            }
        }
    }
}

impl SerialBackend for VcomBackend {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Take what the driver has given, and ask for more, until what is
        // held is enough or the driver has no more yet.
        while self.received.len() < buf.len() {
            match VcomBackend::taken(self.handle, &mut self.read)? {
                Some(length) => {
                    self.received.extend(&self.read.buffer[..length]);
                    self.arm_read()?;
                }
                None => break,
            }
        }
        let take = buf.len().min(self.received.len());
        for (to, octet) in buf.iter_mut().zip(self.received.drain(..take)) {
            *to = octet;
        }
        Ok(take)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut taken = [0u8; 4];
        let chunk = &data[..data.len().min(CHUNK)];
        self.control(vcomapi::IOCTL_WRITE, chunk, &mut taken)?;
        // The driver says how many it took: fewer than given when the
        // host is not reading.
        Ok((u32::from_le_bytes(taken) as usize).min(chunk.len()))
    }

    fn set_signals(&mut self, signals: ModemSignals) -> io::Result<()> {
        let mut bits = 0u32;
        if signals.cts {
            bits |= lines::CTS;
        }
        if signals.dsr {
            bits |= lines::DSR;
        }
        if signals.ri {
            bits |= lines::RI;
        }
        if signals.dcd {
            bits |= lines::DCD;
        }
        self.control(vcomapi::IOCTL_SET_LINES, &bits.to_le_bytes(), &mut [])
            .map(|_| ())
    }

    fn poll(&mut self) -> io::Result<Option<SerialEvent>> {
        while self.events.is_empty() {
            let Some(length) = VcomBackend::taken(self.handle, &mut self.wait)? else {
                break;
            };
            match Event::decode(&self.wait.buffer[..length]) {
                Some(Event::HostOpened) => {
                    self.host_open = true;
                    self.events.push_back(SerialEvent::HostOpened);
                }
                Some(Event::HostClosed) => {
                    self.host_open = false;
                    self.events.push_back(SerialEvent::HostClosed);
                }
                Some(Event::Dtr(on)) => {
                    self.dtr = on;
                    self.events.push_back(SerialEvent::Dtr(on));
                }
                Some(Event::Rts(on)) => {
                    self.rts = on;
                    self.events.push_back(SerialEvent::Rts(on));
                }
                // More happened than the driver kept: what the host's
                // side is now is read, and told as the change it is.
                Some(Event::Lost) => {
                    let state = self.state()?;
                    self.follow(state);
                }
                // A modem has no use for the host's settings, purges and
                // breaks: there is no baud rate in the port.
                Some(Event::Settings | Event::Purge { .. } | Event::Break(_)) => {}
                None => return Err(io::ErrorKind::InvalidData.into()),
            }
            self.arm_wait()?;
        }
        Ok(self.events.pop_front())
    }

    fn wait_handle(&self) -> WaitHandle {
        WaitHandle::from_raw(self.wake_event.0 as isize)
    }
}

impl Drop for VcomBackend {
    fn drop(&mut self) {
        // SAFETY: every handle here was created by `open` and is closed
        // nowhere else. Outstanding requests are cancelled and then waited
        // for, so the kernel has stopped using the boxed OVERLAPPED
        // structures and buffers before they are freed with the rest of
        // the backend.
        unsafe {
            if self.read.pending || self.wait.pending {
                let _ = CancelIoEx(self.handle, None);
            }
            let mut moved = 0;
            if self.read.pending {
                let _ = GetOverlappedResult(self.handle, &*self.read.overlapped, &mut moved, true);
            }
            if self.wait.pending {
                let _ = GetOverlappedResult(self.handle, &*self.wait.overlapped, &mut moved, true);
            }
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.sync_event);
            let _ = CloseHandle(self.wake_event);
        }
    }
}

/// A manual-reset event, not signalled, with no name.
fn new_event() -> io::Result<HANDLE> {
    // SAFETY: no security attributes and no name; the arguments are plain
    // values.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?;
    Ok(event)
}
