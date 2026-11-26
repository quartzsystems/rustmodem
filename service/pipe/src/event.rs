// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Win32 events and waiting on them.

use std::ffi::c_void;
use std::io;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// The most handles one wait can take (`MAXIMUM_WAIT_OBJECTS`).
const MAX_WAIT_HANDLES: usize = 64;

/// An event. One made with [`Event::new`] is manual-reset: once set it
/// stays set. Overlapped I/O on a pipe signals one when it is done. One
/// made with [`Event::auto`] is reset by the wait it ends: the service
/// wakes the thread of a serial port with one.
pub struct Event(HANDLE);

// SAFETY: an event handle may be used from any thread at once; the kernel
// object does its own locking. The raw pointer inside `HANDLE` is what
// stops the compiler from deriving these.
unsafe impl Send for Event {}
// SAFETY: as above.
unsafe impl Sync for Event {}

impl Event {
    /// A new event, not set.
    pub fn new() -> io::Result<Event> {
        // SAFETY: no security attributes and no name; the arguments are
        // plain values.
        let handle = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?;
        Ok(Event(handle))
    }

    /// A new auto-reset event, not set: setting it ends one wait, and the
    /// event is not set again after it.
    pub fn auto() -> io::Result<Event> {
        // SAFETY: no security attributes and no name; the arguments are
        // plain values.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }?;
        Ok(Event(handle))
    }

    /// Sets the event.
    pub fn set(&self) {
        // SAFETY: `self.0` is an event handle this object owns.
        let _ = unsafe { SetEvent(self.0) };
    }

    /// Waits for the event. Returns whether it is set.
    pub fn wait(&self, timeout: Duration) -> bool {
        // SAFETY: `self.0` is an event handle this object owns.
        unsafe { WaitForSingleObject(self.0, millis(timeout)) == WAIT_OBJECT_0 }
    }

    /// The handle, for Win32 calls that take one.
    pub fn handle(&self) -> HANDLE {
        self.0
    }

    /// The handle as a plain number, for [`wait_any`].
    pub fn raw(&self) -> isize {
        self.0.0 as isize
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `new` and is closed nowhere
        // else.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn millis(timeout: Duration) -> u32 {
    // u32::MAX means "no limit" to Win32, so stop one short of it.
    timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32
}

/// Waits until one of the handles is signalled or the time runs out.
/// Returns the index of a signalled handle.
///
/// The handles are raw values of waitable kernel objects that their owners
/// keep open for the length of the call. At most [`MAX_WAIT_HANDLES`] are
/// waited on; any more are ignored, which costs only promptness.
pub fn wait_any(handles: &[isize], timeout: Duration) -> Option<usize> {
    let count = handles.len().min(MAX_WAIT_HANDLES);
    if count == 0 {
        std::thread::sleep(timeout);
        return None;
    }
    let mut array = [HANDLE::default(); MAX_WAIT_HANDLES];
    for (slot, raw) in array.iter_mut().zip(handles) {
        *slot = HANDLE(*raw as *mut c_void);
    }
    // SAFETY: `array[..count]` holds handle values; a value that is not a
    // live waitable handle makes the call fail, which is handled below, and
    // cannot make it touch memory.
    let woken = unsafe { WaitForMultipleObjects(&array[..count], false, millis(timeout)) };
    let index = woken.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
    if index < count {
        Some(index)
    } else {
        if woken.0 == u32::MAX {
            // The wait itself failed, for example because a port was closed
            // between collecting its handle and waiting. Do not spin.
            std::thread::sleep(timeout.min(Duration::from_millis(20)));
        }
        None
    }
}
