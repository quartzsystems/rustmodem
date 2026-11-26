// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A pipe handle read and written with blocking calls, for a client.
//!
//! The handle is opened for overlapped I/O, so that a connection can be
//! read on one thread while it is written on another, and closed from a
//! third. Each call waits for its operation to finish: to its caller it
//! is `std::io`.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_NOT_CONNECTED, HANDLE,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use crate::event::Event;

/// Closes a connection from another thread, so that a reader or writer
/// blocked on it returns. `close` does not block.
pub trait Closer: Send + Sync {
    /// Closes the connection. Closing twice is harmless.
    fn close(&self);
}

/// The two directions of a connection, and the means to close it.
pub struct Halves {
    /// Bytes from the service.
    pub reader: Box<dyn Read + Send>,
    /// Bytes to the service.
    pub writer: Box<dyn Write + Send>,
    /// Closes both directions.
    pub closer: Arc<dyn Closer>,
}

/// A client's end of one instance of the pipe: a handle that is closed
/// when the last user of it goes.
pub(crate) struct Instance {
    pub(crate) handle: HANDLE,
    pub(crate) closed: AtomicBool,
}

// SAFETY: a pipe handle opened for overlapped I/O may be used from several
// threads at once, each operation with its own OVERLAPPED; that is how the
// reader, the writer and the closer use it.
unsafe impl Send for Instance {}
// SAFETY: as above.
unsafe impl Sync for Instance {}

impl Closer for Instance {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        // SAFETY: the handle is open for as long as this object lives.
        // The call does not block: it ends whatever is already waiting.
        // The flag above is what stops I/O from starting again.
        unsafe {
            let _ = CancelIoEx(self.handle, None);
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateFileW and is closed
        // nowhere else. The reader and the writer each hold an `Arc` of
        // this object, so no I/O on the handle is outstanding by now.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// One direction of a connection: its own event and OVERLAPPED, so that it
/// does not disturb the other direction.
pub(crate) struct Half {
    pub(crate) instance: Arc<Instance>,
    pub(crate) event: Event,
    pub(crate) overlapped: Box<OVERLAPPED>,
}

// SAFETY: the OVERLAPPED is used only by the thread that owns this half,
// and only during a call that waits for the operation to finish.
unsafe impl Send for Half {}

impl Half {
    fn prepare(&mut self) -> io::Result<()> {
        if self.instance.closed.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        *self.overlapped = OVERLAPPED {
            hEvent: self.event.handle(),
            ..OVERLAPPED::default()
        };
        Ok(())
    }

    /// Waits for the operation just started and returns how many bytes it
    /// moved.
    fn finish(&mut self, started: windows::core::Result<()>) -> windows::core::Result<u32> {
        match started {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(e) => return Err(e),
        }
        let mut moved = 0;
        // SAFETY: `overlapped` is the OVERLAPPED the operation was started
        // with, boxed so it has not moved. Waiting here means the kernel
        // has finished with it, and with the caller's buffer, before this
        // returns, whether the operation succeeded, failed or was
        // cancelled.
        unsafe { GetOverlappedResult(self.instance.handle, &*self.overlapped, &mut moved, true) }?;
        Ok(moved)
    }
}

impl Read for Half {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.prepare().is_err() {
            return Ok(0);
        }
        // SAFETY: `buf` and the OVERLAPPED are valid now, and `finish`
        // waits for the read to end before this returns.
        let started = unsafe {
            ReadFile(
                self.instance.handle,
                Some(buf),
                None,
                Some(&mut *self.overlapped),
            )
        };
        match self.finish(started) {
            Ok(moved) => Ok(moved as usize),
            // The service has gone, or the connection was closed from this
            // side: the end of the stream.
            Err(e)
                if [
                    ERROR_BROKEN_PIPE,
                    ERROR_PIPE_NOT_CONNECTED,
                    ERROR_OPERATION_ABORTED,
                    ERROR_NO_DATA,
                ]
                .iter()
                .any(|code| e.code() == code.to_hresult()) =>
            {
                Ok(0)
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Write for Half {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        self.prepare()?;
        // SAFETY: `data` and the OVERLAPPED are valid now, and `finish`
        // waits for the write to end before this returns.
        let started = unsafe {
            WriteFile(
                self.instance.handle,
                Some(data),
                None,
                Some(&mut *self.overlapped),
            )
        };
        Ok(self.finish(started)? as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        // Nothing is buffered on this side. FlushFileBuffers would wait for
        // the service to read, which is exactly what a writer must not do.
        Ok(())
    }
}
