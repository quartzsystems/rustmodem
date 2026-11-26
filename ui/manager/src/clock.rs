// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What time it is, and what a clock on the wall of this machine shows.

use std::time::{SystemTime, UNIX_EPOCH};

use model::{TimeZone, Utc, WallTime};

/// The manager's clock.
///
/// The screenshot tests stop it: with a fixed time the durations, the
/// uptime and the clock in the status bar are the same on every run, and
/// times are shown in UTC, so that the pictures do not depend on where
/// they were taken.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    fixed: Option<u64>,
}

impl Clock {
    /// The clock of this machine.
    pub fn system() -> Clock {
        Clock { fixed: None }
    }

    /// A clock that always shows one moment, in Unix milliseconds.
    pub fn fixed(unix_ms: u64) -> Clock {
        Clock {
            fixed: Some(unix_ms),
        }
    }

    /// Now, in Unix milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.fixed.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_millis() as u64)
                .unwrap_or(0)
        })
    }
}

impl TimeZone for Clock {
    fn wall(&self, unix_ms: u64) -> WallTime {
        if self.fixed.is_some() {
            return Utc.wall(unix_ms);
        }
        match win32::time::local(unix_ms) {
            Some(time) => WallTime {
                year: time.year,
                month: time.month,
                day: time.day,
                hour: time.hour,
                minute: time.minute,
                second: time.second,
            },
            // A time Windows cannot express.
            None => Utc.wall(unix_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_clock_shows_one_moment_in_utc() {
        let clock = Clock::fixed(1_800_000_000_000);
        assert_eq!(clock.now_ms(), 1_800_000_000_000);
        assert_eq!(
            clock.wall(clock.now_ms()).date_time(),
            "2027-01-15 08:00:00"
        );
    }

    #[test]
    fn the_system_clock_runs_and_is_within_a_day_of_utc() {
        let clock = Clock::system();
        let now = clock.now_ms();
        // After this was written, and the same second everywhere.
        assert!(now > 1_780_000_000_000);
        assert_eq!(clock.wall(now).second, Utc.wall(now).second);
    }
}
