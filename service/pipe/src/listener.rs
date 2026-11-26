// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The service's end of the control pipe.
//!
//! [`PipeListener`] is a `ctlsrv::Listener` on `\\.\pipe\<name>`, on
//! Tokio's named pipe server. The service creates the pipe with a security
//! descriptor that gives full access to SYSTEM and Administrators and
//! nothing to anyone else. The pipe is a byte stream, takes any number of
//! clients, and refuses remote ones.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;

use ctlsrv::{Connection, Halves, Listener};
use msg::{Message, message};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows::core::w;

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

/// A client connected to the pipe.
pub struct PipeConnection {
    server: NamedPipeServer,
}

impl Connection for PipeConnection {
    fn peer(&self) -> String {
        let mut process = 0;
        let handle = HANDLE(self.server.as_raw_handle());
        // SAFETY: the handle is a connected pipe instance that `server`
        // keeps open, and `process` is valid for the call to fill.
        match unsafe { GetNamedPipeClientProcessId(handle, &mut process) } {
            Ok(()) => format!("process {process}"),
            Err(_) => "pipe".to_string(),
        }
    }

    fn split(self) -> Halves {
        // The instance is closed when both halves are dropped, and not
        // disconnected first: what has been written and not yet read stays
        // readable by the client, which then sees the end of the stream.
        // Disconnecting would discard it, and with it the error that
        // explains why the service hung up.
        let (reader, writer) = tokio::io::split(self.server);
        Halves {
            reader: Box::new(reader),
            writer: Box::new(writer),
        }
    }
}

/// Listens on a named pipe.
pub struct PipeListener {
    name: String,
    path: String,
    security: PipeSecurity,
    /// The instance the next client will connect to.
    next: Option<NamedPipeServer>,
}

impl PipeListener {
    /// Creates the pipe `\\.\pipe\<name>`. Fails with message `RMD2007` if
    /// the pipe exists already, which means another copy of the service is
    /// running with the same pipe name.
    ///
    /// This must be called on the runtime the listener will be used on:
    /// the pipe is registered with its I/O driver.
    pub fn bind(name: &str, security: PipeSecurity) -> Result<PipeListener, Message> {
        let mut listener = PipeListener {
            name: name.to_string(),
            path: format!(r"\\.\pipe\{name}"),
            security,
            next: None,
        };
        // The first instance is made now, so that a name already in use is
        // found at start and clients can connect as soon as this returns.
        let first = listener
            .create_instance(true)
            .map_err(|e| message("RMD2007", &[name, &e.to_string()]))?;
        listener.next = Some(first);
        Ok(listener)
    }

    /// The name of the pipe, without the `\\.\pipe\` prefix.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn create_instance(&self, first: bool) -> io::Result<NamedPipeServer> {
        // A duplex pipe of bytes, with as many instances as there are
        // clients, which is what the options start from.
        let mut options = ServerOptions::new();
        options
            // Refuse to share the name with a pipe someone else made first.
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .pipe_mode(PipeMode::Byte)
            .in_buffer_size(BUFFER_SIZE)
            .out_buffer_size(BUFFER_SIZE);
        match self.security {
            PipeSecurity::Creator => options.create(&self.path),
            PipeSecurity::AdministratorsOnly => {
                let mut descriptor = PSECURITY_DESCRIPTOR::default();
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
                let mut attributes = SECURITY_ATTRIBUTES {
                    nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor.0,
                    bInheritHandle: false.into(),
                };
                // SAFETY: `attributes` is a SECURITY_ATTRIBUTES that points
                // at a descriptor, and both live until after the call.
                let created = unsafe {
                    options.create_with_security_attributes_raw(
                        &self.path,
                        (&raw mut attributes).cast::<c_void>(),
                    )
                };
                // SAFETY: the descriptor was allocated by the conversion
                // above, which documents LocalFree as the way to release
                // it, and the pipe has taken its own copy.
                unsafe {
                    LocalFree(Some(HLOCAL(descriptor.0)));
                }
                created
            }
        }
    }
}

impl Listener for PipeListener {
    type Connection = PipeConnection;

    async fn accept(&mut self) -> io::Result<Option<PipeConnection>> {
        let instance = match self.next.take() {
            Some(instance) => instance,
            None => self.create_instance(false)?,
        };
        // The instance stays in `next` while it waits, so that a wait
        // given up loses neither it nor a client on its way to it.
        let connected = self.next.insert(instance).connect().await;
        let Some(server) = self.next.take() else {
            return Err(io::ErrorKind::NotConnected.into());
        };
        connected?;
        // Make the instance for the client after this one before handing
        // this one over, so that there is always one to connect to. If it
        // cannot be made now, the next accept tries again.
        self.next = self.create_instance(false).ok();
        Ok(Some(PipeConnection { server }))
    }
}
