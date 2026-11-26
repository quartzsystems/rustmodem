// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! How numbers, lengths of time and times of day are written.

/// What an empty cell shows: an em dash.
pub const EMPTY: &str = "\u{2014}";

/// A number with its thousands separated: `1,234,567`.
pub fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A length of time as `M:SS`, or `H:MM:SS` from an hour up. Parts of a
/// second are dropped: a call of 59.9 seconds has lasted `0:59`.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// How long a service has been up: `H:MM:SS`, with the days in front once
/// there are any, as `3d 04:12:07`.
pub fn uptime(ms: u64) -> String {
    let seconds = ms / 1000;
    let days = seconds / 86_400;
    let (hours, minutes, seconds) = (seconds / 3600 % 24, seconds / 60 % 60, seconds % 60);
    if days > 0 {
        format!("{days}d {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

/// A date and a time of day, as a clock on the wall shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WallTime {
    /// The year.
    pub year: u16,
    /// The month, 1 to 12.
    pub month: u8,
    /// The day of the month.
    pub day: u8,
    /// The hour, 0 to 23.
    pub hour: u8,
    /// The minute.
    pub minute: u8,
    /// The second.
    pub second: u8,
}

impl WallTime {
    /// `HH:MM:SS`.
    pub fn time(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    /// `HH:MM`, for a clock.
    pub fn clock(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// `YYYY-MM-DD`.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `YYYY-MM-DD HH:MM:SS`.
    pub fn date_time(&self) -> String {
        format!("{} {}", self.date(), self.time())
    }
}

/// Turns the service's times, in milliseconds since the Unix epoch, into
/// what a clock on the wall shows. The manager gives the time zone of the
/// machine; tests give [`Utc`].
pub trait TimeZone {
    /// The wall time of a moment.
    fn wall(&self, unix_ms: u64) -> WallTime;
}

/// Coordinated Universal Time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Utc;

impl TimeZone for Utc {
    fn wall(&self, unix_ms: u64) -> WallTime {
        let seconds = unix_ms / 1000;
        // The Gregorian calendar repeats every 400 years (146 097 days).
        // Counting years from 1 March puts the leap day last, which makes
        // the day of the year a simple function of the month.
        let days = seconds / 86_400 + 719_468;
        let (era, day_of_era) = (days / 146_097, days % 146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        let year = year_of_era + era * 400 + u64::from(month <= 2);
        WallTime {
            year: year.min(u64::from(u16::MAX)) as u16,
            month: month as u8,
            day: day as u8,
            hour: (seconds / 3600 % 24) as u8,
            minute: (seconds / 60 % 60) as u8,
            second: (seconds % 60) as u8,
        }
    }
}

/// Whole percentages of the parts of a total that add up to exactly 100,
/// by the largest-remainder method: each part gets its percentage rounded
/// down, and the points left over go one each to the parts that lost the
/// most by the rounding, the larger part first among equals and then the
/// earlier one. With nothing to share, every part is 0.
pub fn percentages(parts: &[u64]) -> Vec<u32> {
    let total: u128 = parts.iter().map(|part| u128::from(*part)).sum();
    if total == 0 {
        return vec![0; parts.len()];
    }
    let mut shares: Vec<u32> = Vec::with_capacity(parts.len());
    let mut remainders: Vec<(u128, u64, usize)> = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        let scaled = u128::from(*part) * 100;
        shares.push((scaled / total) as u32);
        remainders.push((scaled % total, *part, index));
    }
    let left = 100 - shares.iter().sum::<u32>();
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    for (_, _, index) in remainders.iter().take(left as usize) {
        shares[*index] += 1;
    }
    shares
}

/// A field of a CSV file: in quotes if it holds a comma, a quote or a line
/// break, with its quotes doubled (RFC 4180).
pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// A row of a CSV file, ended as RFC 4180 ends one.
pub fn csv_row(fields: &[&str]) -> String {
    let mut row = fields
        .iter()
        .map(|field| csv_field(field))
        .collect::<Vec<_>>()
        .join(",");
    row.push_str("\r\n");
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_grouped_in_thousands() {
        for (value, text) in [
            (0, "0"),
            (7, "7"),
            (999, "999"),
            (1000, "1,000"),
            (15_200, "15,200"),
            (122_400, "122,400"),
            (1_234_567, "1,234,567"),
            (1_000_000_000, "1,000,000,000"),
            (u64::MAX, "18,446,744,073,709,551,615"),
        ] {
            assert_eq!(grouped(value), text);
        }
    }

    #[test]
    fn durations_show_hours_only_when_there_are_any() {
        for (ms, text) in [
            (0, "0:00"),
            (999, "0:00"),
            (1000, "0:01"),
            (59_999, "0:59"),
            (60_000, "1:00"),
            (205_000, "3:25"),
            (3_599_999, "59:59"),
            (3_600_000, "1:00:00"),
            (3_727_000, "1:02:07"),
            (360_000_000, "100:00:00"),
        ] {
            assert_eq!(duration(ms), text, "{ms}");
        }
    }

    #[test]
    fn uptime_shows_days_once_there_are_any() {
        assert_eq!(uptime(0), "0:00:00");
        assert_eq!(uptime(1_497_170), "0:24:57");
        assert_eq!(uptime(86_399_000), "23:59:59");
        assert_eq!(uptime(86_400_000), "1d 00:00:00");
        assert_eq!(
            uptime(3 * 86_400_000 + 4 * 3_600_000 + 12 * 60_000 + 7000),
            "3d 04:12:07"
        );
    }

    #[test]
    fn utc_is_the_calendar() {
        let wall = Utc.wall(0);
        assert_eq!(wall.date_time(), "1970-01-01 00:00:00");
        assert_eq!(
            Utc.wall(1_800_000_000_999).date_time(),
            "2027-01-15 08:00:00"
        );
        assert_eq!(Utc.wall(951_782_400_000).date(), "2000-02-29");
        assert_eq!(Utc.wall(951_868_799_999).date_time(), "2000-02-29 23:59:59");
        assert_eq!(Utc.wall(4_107_542_400_000).date(), "2100-03-01");
        assert_eq!(
            Utc.wall(1_735_689_599_000).date_time(),
            "2024-12-31 23:59:59"
        );
        let wall = Utc.wall(1_800_000_000_000 + 1_497_170);
        assert_eq!(
            (wall.time().as_str(), wall.clock().as_str()),
            ("08:24:57", "08:24")
        );
    }

    #[test]
    fn percentages_add_up_to_exactly_one_hundred() {
        // The connects of the frozen demo: three, two and one.
        assert_eq!(percentages(&[3, 2, 1]), [50, 33, 17]);
        assert_eq!(percentages(&[1, 1, 1]), [34, 33, 33]);
        assert_eq!(percentages(&[2, 1]), [67, 33]);
        assert_eq!(percentages(&[1, 2]), [33, 67]);
        assert_eq!(percentages(&[5]), [100]);
        assert_eq!(percentages(&[1, 1]), [50, 50]);
        assert_eq!(percentages(&[1, 1, 1, 1, 1, 1]), [17, 17, 17, 17, 16, 16]);
        assert_eq!(percentages(&[999, 1]), [100, 0]);
        // Equal remainders: the larger part takes the point.
        assert_eq!(percentages(&[199, 1]), [100, 0]);
        assert_eq!(percentages(&[7, 0, 0]), [100, 0, 0]);
        // Nothing to share.
        assert_eq!(percentages(&[0, 0]), [0, 0]);
        assert_eq!(percentages(&[]), Vec::<u32>::new());
        // Large counts do not overflow.
        assert_eq!(percentages(&[u64::MAX, u64::MAX]), [50, 50]);
    }

    #[test]
    fn percentages_are_within_one_of_the_true_share_whatever_the_counts() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..2000 {
            let mut parts = Vec::new();
            for _ in 0..1 + (state % 7) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                parts.push(state % 1000);
            }
            let shares = percentages(&parts);
            let total: u64 = parts.iter().sum();
            if total == 0 {
                continue;
            }
            assert_eq!(shares.iter().sum::<u32>(), 100, "{parts:?}");
            for (part, share) in parts.iter().zip(&shares) {
                let exact = *part as f64 * 100.0 / total as f64;
                assert!(
                    (f64::from(*share) - exact).abs() < 1.0,
                    "{parts:?} {shares:?}"
                );
            }
        }
    }

    #[test]
    fn csv_fields_are_quoted_when_they_must_be() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field(""), "");
        assert_eq!(
            csv_row(&["Rate", "V.22bis, 2400", "3"]),
            "Rate,\"V.22bis, 2400\",3\r\n"
        );
    }
}
