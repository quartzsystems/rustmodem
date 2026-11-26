// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The service's end of the control pipe.
//!
//! [`PipeListener`] is a `ctlsrv::Listener` on `\\.\pipe\<name>`. The
//! service creates the pipe with a security descriptor that gives full
//! access to SYSTEM and Administrators and nothing to anyone else. The pipe
//! refuses remote clients.
//!
//! Every handle is opened for overlapped I/O, so that a connection can be
//! read on one thread while it is written on another, and so that waiting
//! for a client can be interrupted when the service stops.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ctlsrv::{Closer, Connection, Halves, Listener};
use msg::{Message, message};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows::core::{HSTRING, w};

use crate::event::{Event, wait_any};

/// The size asked for each of the pipe's buffers, in bytes.
const BUFFER_SIZE: u32 = 64 * 1024;

/// Who may open the pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeSecurity {
    /// SYSTEM and Administrators, with full access; nobody else. This is
    /// what the service uses.
    AdministratorsOnly,
    /// The default security of a named pipe, which lets in the user who
    /// created it. For the demo service and for tests, neither of which
    /// runs elevated.
    Creator,
}

/// One end of one instance of the pipe: a handle that is closed when the
/// last user of it goes.
pub(crate) struct Instance {
    pub(crate) handle: HANDLE,
    /// The handle is the service's end, which can disconnect the client.
    pub(crate) server: bool,
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
        // Neither call blocks: disconnecting fails whatever I/O is started
        // from now on, and cancelling ends whatever is already waiting. A
        // client's end has nothing to disconnect; the flag above is what
        // stops its I/O from starting again.
        unsafe {
            if self.server {
                let _ = DisconnectNamedPipe(self.handle);
            }
            let _ = CancelIoEx(self.handle, None);
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateNamedPipeW or, for a
        // client, by CreateFileW, and is closed nowhere else. The reader and the writer each hold an `Arc` of
        // this object, so no I/O on the handle is outstanding by now.
        //
        // The pipe is closed without being disconnected first: what has
        // been written and not yet read stays readable by the client, which
        // then sees the end of the stream. Disconnecting would discard it,
        // and with it the error that explains why the service hung up.
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
            // The client has gone, or the connection was closed from this
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
        // the client to read, which is exactly what a writer must not do.
        Ok(())
    }
}

/// A client connected to the pipe.
pub struct PipeConnection {
    instance: Arc<Instance>,
    reader_event: Event,
    writer_event: Event,
}

impl Connection for PipeConnection {
    fn peer(&self) -> String {
        let mut process = 0;
        // SAFETY: the handle is a connected pipe instance and `process` is
        // valid for the call to fill.
        match unsafe { GetNamedPipeClientProcessId(self.instance.handle, &mut process) } {
            Ok(()) => format!("process {process}"),
            Err(_) => "pipe".to_string(),
        }
    }

    fn split(self: Box<Self>) -> Halves {
        let reader = Half {
            instance: Arc::clone(&self.instance),
            event: self.reader_event,
            overlapped: Box::new(OVERLAPPED::default()),
        };
        let writer = Half {
            instance: Arc::clone(&self.instance),
            event: self.writer_event,
            overlapped: Box::new(OVERLAPPED::default()),
        };
        Halves {
            reader: Box::new(reader),
            writer: Box::new(writer),
            closer: self.instance,
        }
    }
}

/// Listens on a named pipe.
pub struct PipeListener {
    name: String,
    path: HSTRING,
    security: PipeSecurity,
    /// Set when the service is stopping; ends a wait for a client.
    stop: Arc<Event>,
    /// The instance the next client will connect to.
    next: Option<HANDLE>,
    connect_event: Event,
    overlapped: Box<OVERLAPPED>,
}

// SAFETY: the listener owns its handles and its OVERLAPPED outright and is
// used by one thread at a time.
unsafe impl Send for PipeListener {}

impl PipeListener {
    /// Creates the pipe `\\.\pipe\<name>`. Fails with message `RMD2007` if
    /// the pipe exists already, which means another copy of the service is
    /// running with the same pipe name.
    pub fn bind(
        name: &str,
        security: PipeSecurity,
        stop: Arc<Event>,
    ) -> Result<PipeListener, Message> {
        let refused = |e: io::Error| message("RMD2007", &[name, &e.to_string()]);
        let mut listener = PipeListener {
            name: name.to_string(),
            path: HSTRING::from(format!(r"\\.\pipe\{name}")),
            security,
            stop,
            next: None,
            connect_event: Event::new().map_err(refused)?,
            overlapped: Box::new(OVERLAPPED::default()),
        };
        // The first instance is made now, so that a name already in use is
        // found at start and clients can connect as soon as this returns.
        listener.next = Some(listener.create_instance(true).map_err(refused)?);
        Ok(listener)
    }

    /// The name of the pipe, without the `\\.\pipe\` prefix.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn create_instance(&self, first: bool) -> io::Result<HANDLE> {
        let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            // Refuse to share the name with a pipe someone else made first.
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let pipe_mode =
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;

        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let attributes = match self.security {
            PipeSecurity::AdministratorsOnly => {
                // A protected DACL with two entries: generic all for SYSTEM
                // (SY) and for the built-in Administrators group (BA).
                // SAFETY: the string is a NUL-terminated wide literal and
                // `descriptor` is valid for the call to fill.
                unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)"),
                        SDDL_REVISION_1,
                        &mut descriptor,
                        None,
                    )
                }?;
                Some(SECURITY_ATTRIBUTES {
                    nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor.0,
                    bInheritHandle: false.into(),
                })
            }
            PipeSecurity::Creator => None,
        };

        // SAFETY: `path` is a NUL-terminated wide string and `attributes`,
        // if any, points at a descriptor that lives until after the call.
        let handle = unsafe {
            CreateNamedPipeW(
                &self.path,
                open_mode,
                pipe_mode,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER_SIZE,
                BUFFER_SIZE,
                0,
                attributes.as_ref().map(|a| a as *const SECURITY_ATTRIBUTES),
            )
        };
        // Read the error before anything else can replace it.
        let result = if handle.is_invalid() {
            Err(io::Error::last_os_error())
        } else {
            Ok(handle)
        };
        if !descriptor.0.is_null() {
            // SAFETY: the descriptor was allocated by the conversion above,
            // which documents LocalFree as the way to release it, and the
            // pipe has taken its own copy.
            unsafe {
                LocalFree(Some(HLOCAL(descriptor.0)));
            }
        }
        result
    }

    /// Waits for a client to connect to `handle`, or for the stop signal.
    /// Returns whether a client connected.
    fn wait_for_client(&mut self, handle: HANDLE) -> io::Result<bool> {
        *self.overlapped = OVERLAPPED {
            hEvent: self.connect_event.handle(),
            ..OVERLAPPED::default()
        };
        // SAFETY: `handle` is a pipe instance this listener owns, and the
        // OVERLAPPED is boxed and outlives the operation: every path below
        // waits for the operation to end before returning.
        let started = unsafe { ConnectNamedPipe(handle, Some(&mut *self.overlapped)) };
        match started {
            Ok(()) => return Ok(true),
            // A client connected between the instance being created and
            // this call.
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => return Ok(true),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(e) => return Err(e.into()),
        }
        let handles = [self.connect_event.raw(), self.stop.raw()];
        let stopping = loop {
            match wait_any(&handles, Duration::from_secs(3600)) {
                Some(0) => break false,
                Some(_) => break true,
                None => {}
            }
        };
        let mut moved = 0;
        // SAFETY: as above; cancelling and then waiting leaves the kernel
        // with no reference to the OVERLAPPED.
        let connected = unsafe {
            if stopping {
                let _ = CancelIoEx(handle, Some(&*self.overlapped));
            }
            GetOverlappedResult(handle, &*self.overlapped, &mut moved, true)
        };
        if stopping {
            return Ok(false);
        }
        connected?;
        Ok(true)
    }
}

impl Listener for PipeListener {
    fn accept(&mut self) -> io::Result<Option<Box<dyn Connection>>> {
        if self.stop.is_set() {
            return Ok(None);
        }
        let handle = match self.next.take() {
            Some(handle) => handle,
            None => self.create_instance(false)?,
        };
        // From here the instance closes the handle when it is dropped.
        let instance = Arc::new(Instance {
            handle,
            server: true,
            closed: AtomicBool::new(false),
        });
        if !self.wait_for_client(handle)? {
            return Ok(None);
        }
        // Make the instance for the client after this one before handing
        // this one over, so that there is always one to connect to. If it
        // cannot be made now, the next accept tries again.
        self.next = self.create_instance(false).ok();
        Ok(Some(Box::new(PipeConnection {
            instance,
            reader_event: Event::new()?,
            writer_event: Event::new()?,
        })))
    }
}

impl Drop for PipeListener {
    fn drop(&mut self) {
        if let Some(handle) = self.next.take() {
            // SAFETY: the handle is an instance nobody has connected to and
            // nothing else refers to.
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }
}
