// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A client's end of the control pipe.
//!
//! A pipe opened as an ordinary file does one thing at a time: a write
//! waits for a read that is already waiting. That is enough for a client
//! that asks and then listens, as `rustmodemctl` does. A client that
//! listens for events all the time and asks whenever its user does needs
//! the handle opened for overlapped I/O, which is what [`PipeClient`] is.

use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use ctlsrv::Halves;
use windows::Win32::Foundation::{ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::IO::OVERLAPPED;
use windows::core::HSTRING;

use crate::event::Event;
use crate::listener::{Half, Instance};

/// How long to wait between tries while every instance of the pipe is
/// taken.
const BUSY_RETRY: Duration = Duration::from_millis(20);

/// The Windows error as `std::io` knows it, so that its kind says what
/// went wrong: not found, permission denied. The `windows` crate carries a
/// Win32 error code inside an HRESULT, which `std::io` does not read.
fn win32_error(error: windows::core::Error) -> io::Error {
    /// What an HRESULT made from a Win32 error code starts with.
    const FACILITY_WIN32: u32 = 0x8007;
    let code = error.code().0 as u32;
    if code >> 16 == FACILITY_WIN32 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        error.into()
    }
}

/// A connection to a control pipe, from the client's side.
pub struct PipeClient {
    instance: Arc<Instance>,
    reader_event: Event,
    writer_event: Event,
}

impl PipeClient {
    /// Opens `\\.\pipe\<name>`. The service makes a new instance of the
    /// pipe as soon as a client connects; while every instance is taken
    /// this keeps trying for `busy_wait`.
    ///
    /// The error is the Windows one: not found when no service listens on
    /// the name, permission denied when the pipe does not admit this user.
    pub fn connect(name: &str, busy_wait: Duration) -> io::Result<PipeClient> {
        let path = HSTRING::from(format!(r"\\.\pipe\{name}"));
        let deadline = Instant::now() + busy_wait;
        let handle = loop {
            // SAFETY: `path` is a terminated wide string that outlives the
            // call; the other arguments are plain values.
            let opened = unsafe {
                CreateFileW(
                    &path,
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            };
            match opened {
                Ok(handle) => break handle,
                Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() && Instant::now() < deadline => {
                    std::thread::sleep(BUSY_RETRY);
                }
                Err(e) => return Err(win32_error(e)),
            }
        };
        // From here the instance closes the handle when it is dropped.
        let instance = Arc::new(Instance {
            handle,
            server: false,
            closed: AtomicBool::new(false),
        });
        Ok(PipeClient {
            instance,
            reader_event: Event::new()?,
            writer_event: Event::new()?,
        })
    }

    /// Splits the connection so that one thread can read while another
    /// writes. The closer ends both from any thread: a read that is
    /// waiting returns the end of the stream.
    pub fn split(self) -> Halves {
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
