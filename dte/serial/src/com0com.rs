// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The com0com backend: the modem's side of a com0com null-modem pair.
//!
//! com0com gives two ports joined back to back. The host opens one (the
//! one with a COM name); this backend opens the other. A null-modem pair
//! has no DCE end: each side drives its own output lines (DTR, RTS, OUT1,
//! OUT2) and the driver wires them to the other side's input lines (DSR,
//! CTS, DCD, RI) as the pair is configured.
//!
//! The backend therefore expects the pair to be set up as
//! `docs/COM0COM.md` describes, which gives this mapping:
//!
//! | The modem wants to set | The backend drives | The host sees |
//! |---|---|---|
//! | DSR | DTR | DSR |
//! | CTS | RTS | CTS |
//! | DCD | OUT1 | DCD |
//! | RI | OUT2 | RI |
//!
//! | The host does | The backend sees on its | Reported as |
//! |---|---|---|
//! | drives DTR | DSR | `Dtr` |
//! | drives RTS | CTS | `Rts` |
//! | opens or closes the port | DCD | `HostOpened`, `HostClosed` |
//!
//! NEEDS-CODY: verify this mapping on a real pair. It is taken from the
//! com0com documentation, not from a test; no pair was available when this
//! was written.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io;

use msg::Message;
use windows::Win32::Devices::Communication::{
    COMM_EVENT_MASK, COMMTIMEOUTS, DCB, DCB_PARITY, DCB_STOP_BITS, EV_CTS, EV_DSR, EV_RLSD,
    EV_RXCHAR, GetCommModemStatus, GetCommState, MODEM_STATUS_FLAGS, MS_CTS_ON, MS_DSR_ON,
    MS_RLSD_ON, PURGE_RXCLEAR, PURGE_TXCLEAR, PurgeComm, SetCommMask, SetCommState,
    SetCommTimeouts, SetupComm, WaitCommEvent,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::core::{HSTRING, PCWSTR};

use crate::{ModemSignals, SerialBackend, SerialEvent, SerialOperation, WaitHandle, error_message};

/// `IOCTL_SERIAL_SET_MODEM_CONTROL`: sets the modem control register, and
/// with it OUT1 and OUT2, which the Win32 comm API cannot reach. The value
/// is `CTL_CODE(FILE_DEVICE_SERIAL_PORT, 37, METHOD_BUFFERED,
/// FILE_ANY_ACCESS)` from the Windows driver kit's `ntddser.h`.
const IOCTL_SERIAL_SET_MODEM_CONTROL: u32 = 0x001B_0094;

/// The bits of the modem control register (`SERIAL_MCR_*` in `ntddser.h`).
const MCR_DTR: u32 = 0x01;
const MCR_RTS: u32 = 0x02;
const MCR_OUT1: u32 = 0x04;
const MCR_OUT2: u32 = 0x08;

/// The most bytes handed to the driver in one write.
const WRITE_CHUNK: usize = 4096;

/// The size asked for the driver's queues.
const QUEUE_SIZE: u32 = 65536;

/// The modem's side of a com0com pair.
pub struct Com0comBackend {
    port: String,
    handle: HANDLE,
    /// For operations that are started and waited for in one call: reads,
    /// which the port's timeouts make return at once, and the IOCTL.
    sync_overlapped: Box<OVERLAPPED>,
    sync_event: HANDLE,
    /// For the one write that may be in flight between calls.
    write_overlapped: Box<OVERLAPPED>,
    write_event: HANDLE,
    write_buffer: Box<[u8]>,
    write_pending: bool,
    /// For the wait on the host sending data or changing a line.
    comm_overlapped: Box<OVERLAPPED>,
    comm_event: HANDLE,
    comm_mask: Box<COMM_EVENT_MASK>,
    comm_armed: bool,
    /// The input lines as last seen.
    status: u32,
    events: VecDeque<SerialEvent>,
}

// SAFETY: the backend owns its handles and buffers outright and shares them
// with no other object, and Windows file and event handles may be used from
// any thread. The raw pointers inside `HANDLE` and `OVERLAPPED` are what
// stop the compiler from deriving this.
unsafe impl Send for Com0comBackend {}

impl Com0comBackend {
    /// Opens `port` (for example `CNCB0`) as `\\.\<port>` and sets it up.
    pub fn open(port: &str) -> Result<Com0comBackend, Message> {
        let path = HSTRING::from(format!(r"\\.\{port}"));
        // SAFETY: `path` is a valid, NUL-terminated wide string that lives
        // through the call; the other arguments are plain values.
        let handle = unsafe {
            CreateFileW(
                &path,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|e| error_message(port, SerialOperation::Open, &e.into()))?;

        let configure = |e: io::Error| error_message(port, SerialOperation::Configure, &e);
        let events =
            (|| -> io::Result<[HANDLE; 3]> { Ok([new_event()?, new_event()?, new_event()?]) })();
        let [sync_event, write_event, comm_event] = match events {
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
        let mut backend = Com0comBackend {
            port: port.to_string(),
            handle,
            sync_overlapped: Box::new(OVERLAPPED::default()),
            sync_event,
            write_overlapped: Box::new(OVERLAPPED::default()),
            write_event,
            write_buffer: vec![0; WRITE_CHUNK].into_boxed_slice(),
            write_pending: false,
            comm_overlapped: Box::new(OVERLAPPED::default()),
            comm_event,
            comm_mask: Box::new(COMM_EVENT_MASK(0)),
            comm_armed: false,
            status: 0,
            events: VecDeque::new(),
        };
        backend.configure().map_err(configure)?;
        Ok(backend)
    }

    /// The port name this backend was opened with.
    pub fn port(&self) -> &str {
        &self.port
    }

    fn configure(&mut self) -> io::Result<()> {
        let mut dcb = DCB::default();
        // SAFETY: `self.handle` is the open port; `dcb` is a valid DCB for
        // the call to fill.
        unsafe { GetCommState(self.handle, &mut dcb) }?;
        dcb.DCBlength = size_of::<DCB>() as u32;
        dcb.BaudRate = 115_200;
        dcb.ByteSize = 8;
        dcb.Parity = DCB_PARITY(0);
        dcb.StopBits = DCB_STOP_BITS(0);
        // The flag bit field: binary mode (bit 0) and nothing else. That
        // leaves parity checking, every kind of flow control and the
        // driver's own DTR and RTS handling off; the backend drives the
        // output lines itself.
        dcb._bitfield = 1;
        let timeouts = COMMTIMEOUTS {
            // A read returns at once with whatever has arrived.
            ReadIntervalTimeout: u32::MAX,
            ReadTotalTimeoutMultiplier: 0,
            ReadTotalTimeoutConstant: 0,
            // A write has no timeout: it completes when the driver has
            // taken every byte.
            WriteTotalTimeoutMultiplier: 0,
            WriteTotalTimeoutConstant: 0,
        };
        let mask = COMM_EVENT_MASK(EV_RXCHAR.0 | EV_CTS.0 | EV_DSR.0 | EV_RLSD.0);
        // SAFETY: `self.handle` is the open port, and `dcb` and `timeouts`
        // are valid for the duration of each call.
        unsafe {
            SetCommState(self.handle, &dcb)?;
            SetCommTimeouts(self.handle, &timeouts)?;
            SetupComm(self.handle, QUEUE_SIZE, QUEUE_SIZE)?;
            PurgeComm(self.handle, PURGE_RXCLEAR | PURGE_TXCLEAR)?;
            SetCommMask(self.handle, mask)?;
        }
        // Until the modem says otherwise, every line toward the host is off.
        self.set_signals(ModemSignals::default())
    }

    /// Waits for an operation started on `sync_overlapped` and returns how
    /// many bytes it moved.
    fn finish_sync(&mut self, started: windows::core::Result<()>) -> io::Result<u32> {
        match started {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(e) => return Err(e.into()),
        }
        let mut moved = 0;
        // SAFETY: `sync_overlapped` is the OVERLAPPED the operation was
        // started with, boxed so it has not moved, and `moved` is valid for
        // the call. Waiting here means the operation no longer uses the
        // OVERLAPPED or the caller's buffer once this returns.
        unsafe { GetOverlappedResult(self.handle, &*self.sync_overlapped, &mut moved, true) }?;
        Ok(moved)
    }

    /// Checks on the write in flight. Returns whether it has finished.
    fn finish_write(&mut self, wait: bool) -> io::Result<bool> {
        if !self.write_pending {
            return Ok(true);
        }
        let mut moved = 0;
        // SAFETY: `write_overlapped` is the OVERLAPPED the write was started
        // with, boxed so it has not moved, and `moved` is valid for the
        // call.
        let result =
            unsafe { GetOverlappedResult(self.handle, &*self.write_overlapped, &mut moved, wait) };
        match result {
            Ok(()) => {
                self.write_pending = false;
                Ok(true)
            }
            Err(e) if e.code() == ERROR_IO_INCOMPLETE.to_hresult() => Ok(false),
            Err(e) => {
                self.write_pending = false;
                Err(e.into())
            }
        }
    }

    /// Starts waiting for the host to send data or change a line.
    fn arm(&mut self) -> io::Result<()> {
        *self.comm_overlapped = OVERLAPPED {
            hEvent: self.comm_event,
            ..OVERLAPPED::default()
        };
        // SAFETY: `comm_mask` and `comm_overlapped` are boxed, so they stay
        // where they are for as long as the wait is outstanding; Drop
        // cancels the wait and waits for it to end before freeing them.
        let started = unsafe {
            WaitCommEvent(
                self.handle,
                &mut *self.comm_mask,
                Some(&mut *self.comm_overlapped),
            )
        };
        match started {
            Ok(()) => {
                // Something had already happened. Leave the wait unarmed
                // and wake the pool so that it polls again.
                // SAFETY: `comm_event` is an event handle this backend owns.
                unsafe { SetEvent(self.comm_event) }?;
                Ok(())
            }
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                self.comm_armed = true;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Notes whether the armed wait has completed.
    fn check_armed(&mut self) -> io::Result<()> {
        if !self.comm_armed {
            return Ok(());
        }
        let mut moved = 0;
        // SAFETY: `comm_overlapped` is the OVERLAPPED the wait was started
        // with, boxed so it has not moved, and `moved` is valid for the
        // call.
        let result =
            unsafe { GetOverlappedResult(self.handle, &*self.comm_overlapped, &mut moved, false) };
        match result {
            Ok(()) => {
                self.comm_armed = false;
                Ok(())
            }
            Err(e) if e.code() == ERROR_IO_INCOMPLETE.to_hresult() => Ok(()),
            Err(e) => {
                self.comm_armed = false;
                Err(e.into())
            }
        }
    }
}

impl SerialBackend for Com0comBackend {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        *self.sync_overlapped = OVERLAPPED {
            hEvent: self.sync_event,
            ..OVERLAPPED::default()
        };
        // SAFETY: `buf` and `sync_overlapped` are valid now, and
        // `finish_sync` waits for the read to end before this function
        // returns, so neither is used after the borrow of `buf` ends.
        let started = unsafe {
            ReadFile(
                self.handle,
                Some(buf),
                None,
                Some(&mut *self.sync_overlapped),
            )
        };
        Ok(self.finish_sync(started)? as usize)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        // One write at a time: until the driver has taken the last chunk,
        // nothing more is accepted.
        if !self.finish_write(false)? || data.is_empty() {
            return Ok(0);
        }
        let take = data.len().min(WRITE_CHUNK);
        self.write_buffer[..take].copy_from_slice(&data[..take]);
        *self.write_overlapped = OVERLAPPED {
            hEvent: self.write_event,
            ..OVERLAPPED::default()
        };
        // SAFETY: `write_buffer` and `write_overlapped` are boxed and owned
        // by the backend, so they stay where they are while the write is
        // outstanding; `write_pending` stops the buffer being reused, and
        // Drop cancels the write and waits for it to end before freeing
        // them.
        let started = unsafe {
            WriteFile(
                self.handle,
                Some(&self.write_buffer[..take]),
                None,
                Some(&mut *self.write_overlapped),
            )
        };
        match started {
            Ok(()) => Ok(take),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                self.write_pending = true;
                Ok(take)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn set_signals(&mut self, signals: ModemSignals) -> io::Result<()> {
        // NEEDS-CODY: verify on a real pair. This is the mapping in the
        // module documentation: it holds only when the host's side of the
        // pair is set up with dsr=rdtr, cts=rrts, dcd=rout1 and ri=rout2.
        let mut control: u32 = 0;
        if signals.dsr {
            control |= MCR_DTR;
        }
        if signals.cts {
            control |= MCR_RTS;
        }
        if signals.dcd {
            control |= MCR_OUT1;
        }
        if signals.ri {
            control |= MCR_OUT2;
        }
        *self.sync_overlapped = OVERLAPPED {
            hEvent: self.sync_event,
            ..OVERLAPPED::default()
        };
        // SAFETY: the input buffer is `control`, four bytes that live until
        // `finish_sync` has waited for the request to end; there is no
        // output buffer.
        let started = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_SERIAL_SET_MODEM_CONTROL,
                Some((&raw const control).cast::<c_void>()),
                size_of::<u32>() as u32,
                None,
                0,
                None,
                Some(&mut *self.sync_overlapped),
            )
        };
        self.finish_sync(started).map(|_| ())
    }

    fn poll(&mut self) -> io::Result<Option<SerialEvent>> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        self.check_armed()?;

        let mut status = MODEM_STATUS_FLAGS(0);
        // SAFETY: `self.handle` is the open port and `status` is valid for
        // the call to fill.
        unsafe { GetCommModemStatus(self.handle, &mut status) }?;
        let changed = status.0 ^ self.status;
        self.status = status.0;
        // NEEDS-CODY: verify on a real pair. The host's DTR arrives on this
        // side's DSR and its RTS on this side's CTS with the pair's default
        // wiring. Whether the host has the port open arrives on this side's
        // DCD when this side is set up with dcd=ropen; with the default
        // wiring DCD follows the host's DTR instead, which is the next best
        // thing.
        if changed & MS_RLSD_ON.0 != 0 && status.0 & MS_RLSD_ON.0 != 0 {
            self.events.push_back(SerialEvent::HostOpened);
        }
        if changed & MS_DSR_ON.0 != 0 {
            self.events
                .push_back(SerialEvent::Dtr(status.0 & MS_DSR_ON.0 != 0));
        }
        if changed & MS_CTS_ON.0 != 0 {
            self.events
                .push_back(SerialEvent::Rts(status.0 & MS_CTS_ON.0 != 0));
        }
        if changed & MS_RLSD_ON.0 != 0 && status.0 & MS_RLSD_ON.0 == 0 {
            self.events.push_back(SerialEvent::HostClosed);
        }

        if !self.comm_armed {
            self.arm()?;
        }
        Ok(self.events.pop_front())
    }

    fn wait_handle(&self) -> WaitHandle {
        WaitHandle::from_raw(self.comm_event.0 as isize)
    }
}

impl Drop for Com0comBackend {
    fn drop(&mut self) {
        // SAFETY: every handle here was created by `open` and is closed
        // nowhere else. Outstanding operations are cancelled and then
        // waited for, so the kernel has stopped using the boxed OVERLAPPED
        // structures, the event mask and the write buffer before they are
        // freed with the rest of the backend.
        unsafe {
            if self.write_pending || self.comm_armed {
                let _ = CancelIoEx(self.handle, None);
            }
            let mut moved = 0;
            if self.write_pending {
                let _ = GetOverlappedResult(self.handle, &*self.write_overlapped, &mut moved, true);
            }
            if self.comm_armed {
                let _ = GetOverlappedResult(self.handle, &*self.comm_overlapped, &mut moved, true);
            }
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.sync_event);
            let _ = CloseHandle(self.write_event);
            let _ = CloseHandle(self.comm_event);
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
