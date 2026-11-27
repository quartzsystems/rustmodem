// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the driver asks of Win32 itself, which is little: who a caller
//! is, and a trace. A user-mode driver is a DLL in a process and may call
//! these as any program may.

use core::ffi::c_void;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

type Handle = *mut c_void;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenThreadToken(thread: Handle, access: u32, as_self: i32, token: *mut Handle) -> i32;
    fn CheckTokenMembership(token: Handle, sid: *const c_void, member: *mut i32) -> i32;
    fn CreateWellKnownSid(
        kind: i32,
        domain: *const c_void,
        sid: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn LookupAccountNameW(
        system: *const u16,
        account: *const u16,
        sid: *mut c_void,
        sid_size: *mut u32,
        domain: *mut u16,
        domain_size: *mut u32,
        kind: *mut i32,
    ) -> i32;
    fn EventRegister(
        provider: *const Guid,
        callback: *const c_void,
        context: *const c_void,
        handle: *mut u64,
    ) -> u32;
    fn EventWriteString(handle: u64, level: u8, keyword: u64, text: *const u16) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThread() -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

/// `TOKEN_QUERY`.
const TOKEN_QUERY: u32 = 0x0008;
/// `WinLocalSystemSid` and `WinBuiltinAdministratorsSid`.
const LOCAL_SYSTEM: i32 = 22;
const ADMINISTRATORS: i32 = 26;
/// The account a Windows service of this name runs under when it has an
/// account of its own.
const SERVICE_ACCOUNT: &str = "NT SERVICE\\rustmodem";

/// A SID in a buffer long enough for any.
type SidBuffer = [u64; 9];

/// Those who may have the control side of a port: the system, an
/// administrator, and the RustModem service's own account.
pub struct Allowed(Vec<SidBuffer>);

/// Finds out who may have the control side of a port.
///
/// Call it before impersonating a caller, not while: the service's
/// account is looked up by name, and a thread that is impersonating a
/// caller only to identify it may not ask Windows that.
pub fn allowed() -> Allowed {
    let mut sids = Vec::new();
    // SAFETY: plain calls with local buffers, each given its true size.
    unsafe {
        for kind in [LOCAL_SYSTEM, ADMINISTRATORS] {
            let mut sid: SidBuffer = [0; 9];
            let mut size = size_of::<SidBuffer>() as u32;
            if CreateWellKnownSid(kind, core::ptr::null(), sid.as_mut_ptr().cast(), &mut size) != 0
            {
                sids.push(sid);
            }
        }
        let account: Vec<u16> = SERVICE_ACCOUNT.encode_utf16().chain([0]).collect();
        let mut sid: SidBuffer = [0; 9];
        let mut size = size_of::<SidBuffer>() as u32;
        let mut domain = [0u16; 256];
        let mut domain_size = domain.len() as u32;
        let mut kind = 0i32;
        // With no such service there is no such account, and nobody is
        // it.
        if LookupAccountNameW(
            core::ptr::null(),
            account.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        ) != 0
        {
            sids.push(sid);
        }
    }
    Allowed(sids)
}

/// Whether the thread, which is impersonating a caller, is acting for
/// one of those allowed.
///
/// Call it only from the callback of `WdfRequestImpersonate`.
pub fn caller_is_allowed(allowed: &Allowed) -> bool {
    let mut token: Handle = core::ptr::null_mut();
    // SAFETY: plain calls on this thread's own token.
    unsafe {
        if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) == 0 {
            return false;
        }
        let is_one = allowed.0.iter().any(|sid| is_member(token, sid));
        CloseHandle(token);
        is_one
    }
}

/// SAFETY: `token` is an open impersonation token.
unsafe fn is_member(token: Handle, sid: &SidBuffer) -> bool {
    let mut member = 0i32;
    // SAFETY: the SID is a whole one, made by the system in this buffer.
    unsafe { CheckTokenMembership(token, sid.as_ptr().cast(), &mut member) != 0 && member != 0 }
}

/// The trace provider: `{B7C1D5E2-3F4A-4C6B-9D8E-524D54524143}`.
/// `docs/DRIVER.md` says how to listen to it.
const PROVIDER: Guid = Guid(
    0xB7C1_D5E2,
    0x3F4A,
    0x4C6B,
    [0x9D, 0x8E, 0x52, 0x4D, 0x54, 0x52, 0x41, 0x43],
);

static PROVIDER_HANDLE: AtomicU64 = AtomicU64::new(0);
/// 0 is off, which is how a port is installed. 2 is errors, 4 is what is
/// opened and closed, 5 is every request.
static LEVEL: AtomicU8 = AtomicU8::new(0);

/// The levels a trace line can have.
pub const ERROR: u8 = 2;
pub const INFO: u8 = 4;
pub const VERBOSE: u8 = 5;

/// Registers the provider. Once, when the driver is loaded.
pub fn trace_start() {
    let mut handle = 0u64;
    // SAFETY: a static provider id and a local for the handle.
    if unsafe { EventRegister(&PROVIDER, core::ptr::null(), core::ptr::null(), &mut handle) } == 0 {
        PROVIDER_HANDLE.store(handle, Ordering::Relaxed);
    }
}

/// Sets how much is traced, from a port's `TraceLevel` value. The
/// greatest any port asks for is what the driver does.
pub fn trace_level(level: u32) {
    LEVEL.fetch_max(level.min(5) as u8, Ordering::Relaxed);
}

/// Whether a line at this level would be written.
pub fn tracing(level: u8) -> bool {
    level <= LEVEL.load(Ordering::Relaxed)
}

/// Writes a line, if tracing is on at its level.
pub fn trace(level: u8, text: &str) {
    if !tracing(level) {
        return;
    }
    let handle = PROVIDER_HANDLE.load(Ordering::Relaxed);
    if handle == 0 {
        return;
    }
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // SAFETY: a registered handle and a string that ends in a NUL.
    unsafe {
        EventWriteString(handle, level, 0, wide.as_ptr());
    }
}
