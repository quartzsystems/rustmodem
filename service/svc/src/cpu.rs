// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! How much of a processor the service process uses, for `stats.get`.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use ctlsrv::CpuMeter;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

/// How long a measurement of the processor time stands before another is
/// made.
const CPU_EVERY_MS: u128 = 1000;

/// The processor time this process has used, kernel and user, in units of
/// 100 nanoseconds.
fn process_time() -> Option<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the pseudo-handle of the current process is always valid and
    // needs no closing, and the four pointers are to FILETIMEs that live
    // for the length of the call.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Some(ticks(kernel) + ticks(user))
}

/// Measures how much of one processor the service process uses, for
/// `stats.get`: the processor time used since the measurement before, over
/// the time that passed. A measurement stands for a second, so that asking
/// often does not make the number jump.
pub fn meter() -> CpuMeter {
    // When the last measurement was made, the processor time by then, and
    // what it came to.
    let last = Mutex::new((Instant::now(), process_time().unwrap_or(0), 0.0f64));
    Arc::new(move || {
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed = last.0.elapsed();
        if elapsed.as_millis() >= CPU_EVERY_MS
            && let Some(used) = process_time()
        {
            // Both in units of 100 nanoseconds.
            let wall = elapsed.as_nanos() as f64 / 100.0;
            let percent = used.saturating_sub(last.1) as f64 * 100.0 / wall;
            *last = (Instant::now(), used, (percent * 10.0).round() / 10.0);
        }
        last.2
    })
}
