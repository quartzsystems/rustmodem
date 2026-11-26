// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Times as the user's clock shows them.
//!
//! The service reports times in milliseconds since the Unix epoch. The
//! manager shows them in the time zone of the machine, with daylight
//! saving as it was at that time, which is what Windows works out here.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

/// A date and a time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateTime {
    /// The year.
    pub year: u16,
    /// The month, 1 to 12.
    pub month: u8,
    /// The day of the month, 1 to 31.
    pub day: u8,
    /// The hour, 0 to 23.
    pub hour: u8,
    /// The minute.
    pub minute: u8,
    /// The second.
    pub second: u8,
    /// The millisecond.
    pub millisecond: u16,
}

/// From the Windows epoch, 1601, to the Unix epoch, 1970, in the hundreds
/// of nanoseconds a FILETIME counts.
const EPOCH_DIFFERENCE: u64 = 116_444_736_000_000_000;

fn from_system(time: &SYSTEMTIME) -> DateTime {
    DateTime {
        year: time.wYear,
        month: time.wMonth as u8,
        day: time.wDay as u8,
        hour: time.wHour as u8,
        minute: time.wMinute as u8,
        second: time.wSecond as u8,
        millisecond: time.wMilliseconds,
    }
}

fn utc_system(unix_ms: u64) -> Option<SYSTEMTIME> {
    let ticks = unix_ms.checked_mul(10_000)?.checked_add(EPOCH_DIFFERENCE)?;
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    // SAFETY: both structures are valid for the call to read and fill.
    unsafe { FileTimeToSystemTime(&file_time, &mut utc) }.ok()?;
    Some(utc)
}

/// A time in UTC. `None` for a time Windows cannot express.
pub fn utc(unix_ms: u64) -> Option<DateTime> {
    utc_system(unix_ms).map(|time| from_system(&time))
}

/// A time on the clock of this machine. `None` for a time Windows cannot
/// express.
pub fn local(unix_ms: u64) -> Option<DateTime> {
    let utc = utc_system(unix_ms)?;
    let mut local = SYSTEMTIME::default();
    // SAFETY: both structures are valid for the call to read and fill;
    // with no time zone given the one of this machine is used.
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }.ok()?;
    Some(from_system(&local))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_time_is_a_date_and_a_time_of_day() {
        let epoch = utc(0).unwrap();
        assert_eq!((epoch.year, epoch.month, epoch.day), (1970, 1, 1));
        assert_eq!((epoch.hour, epoch.minute, epoch.second), (0, 0, 0));
        let time = utc(1_800_000_000_123).unwrap();
        assert_eq!((time.year, time.month, time.day), (2027, 1, 15));
        assert_eq!((time.hour, time.minute, time.second), (8, 0, 0));
        assert_eq!(time.millisecond, 123);
        let leap = utc(951_782_400_000).unwrap();
        assert_eq!((leap.year, leap.month, leap.day), (2000, 2, 29));
        assert_eq!(utc(u64::MAX), None);
    }

    #[test]
    fn local_time_is_within_a_day_of_utc_and_keeps_the_seconds() {
        for unix_ms in [0u64, 1_800_000_000_000, 1_790_000_000_500] {
            let there = utc(unix_ms).unwrap();
            let here = local(unix_ms).unwrap();
            // Time zones are whole minutes from UTC.
            assert_eq!(
                (here.second, here.millisecond),
                (there.second, there.millisecond)
            );
            let days = |time: &DateTime| {
                i64::from(time.year) * 372 + i64::from(time.month) * 31 + i64::from(time.day)
            };
            assert!((days(&here) - days(&there)).abs() <= 1);
        }
    }
}
