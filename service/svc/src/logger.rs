// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The service log: rotating files in the log directory, the console in
//! console mode, and `log` events for control clients.
//!
//! A log line carries a timestamp, a level, a source tag and the text:
//!
//! ```text
//! 2026-11-25T21:57:03.123Z INFO  [Line01] RMD1005: Line 1 state is now idle.
//! ```
//!
//! The current file is `rustmodem.log`. When it reaches the size limit it
//! becomes `rustmodem.1.log`, the one before becomes `rustmodem.2.log`, and
//! so on; ten files are kept in all.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use ctlproto::{Event, LogEvent, LogLevel};
use ctlsrv::Hub;
use msg::{Message, Severity, message};

/// The name of the current log file.
pub const LOG_FILE: &str = "rustmodem.log";

/// How many log files are kept, the current one included.
pub const KEEP_FILES: u32 = 10;

/// How large a log file may grow before it is rotated, in bytes.
pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;

struct Files {
    directory: PathBuf,
    current: Option<File>,
    size: u64,
    max_bytes: u64,
}

struct Inner {
    level: LogLevel,
    console: bool,
    hub: Option<Hub>,
    files: Mutex<Files>,
}

/// The service log. Cloning it gives another handle on the same log.
#[derive(Clone)]
pub struct Logger {
    inner: Arc<Inner>,
}

impl Logger {
    /// Opens the log in `directory`, creating the directory if need be.
    /// Lines more detailed than `level` are left out. With `console`, lines
    /// also go to standard output. With a hub, lines also go to control
    /// clients as `log` events.
    pub fn open(
        directory: &Path,
        level: LogLevel,
        console: bool,
        hub: Option<Hub>,
        max_bytes: u64,
    ) -> Result<Logger, Message> {
        let unusable = |e: std::io::Error| {
            message(
                "RMD0022",
                &[&directory.display().to_string(), &e.to_string()],
            )
        };
        std::fs::create_dir_all(directory).map_err(unusable)?;
        let current = open_append(&directory.join(LOG_FILE)).map_err(unusable)?;
        let size = current.metadata().map_err(unusable)?.len();
        Ok(Logger {
            inner: Arc::new(Inner {
                level,
                console,
                hub,
                files: Mutex::new(Files {
                    directory: directory.to_path_buf(),
                    current: Some(current),
                    size,
                    max_bytes,
                }),
            }),
        })
    }

    /// Writes a line, if the log's level lets it through.
    pub fn log(&self, level: LogLevel, source: &str, text: &str) {
        self.event(LogEvent {
            time_ms: unix_ms(),
            level,
            source: source.to_string(),
            text: text.to_string(),
        });
    }

    /// Writes an `RMD` message at the level its severity gives it.
    pub fn message(&self, source: &str, message: &Message) {
        let level = match message.severity {
            Severity::Info => LogLevel::Info,
            Severity::Warning => LogLevel::Warning,
            Severity::Error => LogLevel::Error,
        };
        self.log(level, source, &message.to_string());
    }

    /// Writes a line that already has its time, as the pool's do.
    pub fn event(&self, event: LogEvent) {
        if event.level > self.inner.level {
            return;
        }
        let line = format_line(&event);
        if self.inner.console {
            println!("{line}");
        }
        self.files().write(&line);
        if let Some(hub) = &self.inner.hub {
            hub.publish(&Event::Log(event));
        }
    }

    /// Makes sure everything written has reached the file.
    pub fn flush(&self) {
        if let Some(file) = &mut self.files().current {
            let _ = file.flush();
            let _ = file.sync_data();
        }
    }

    fn files(&self) -> MutexGuard<'_, Files> {
        self.inner.files.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Files {
    fn write(&mut self, line: &str) {
        let bytes = line.len() as u64 + 1;
        if self.size > 0 && self.size + bytes > self.max_bytes {
            self.rotate();
        }
        // A log that cannot be written has nowhere to say so; the line
        // still went to the console and to control clients.
        if let Some(file) = &mut self.current
            && writeln!(file, "{line}").is_ok()
        {
            self.size += bytes;
        }
    }

    /// Moves every file along by one and starts a new current file.
    fn rotate(&mut self) {
        self.current = None;
        let name = |number: u32| {
            if number == 0 {
                self.directory.join(LOG_FILE)
            } else {
                self.directory.join(format!("rustmodem.{number}.log"))
            }
        };
        let _ = std::fs::remove_file(name(KEEP_FILES - 1));
        for number in (0..KEEP_FILES - 1).rev() {
            let _ = std::fs::rename(name(number), name(number + 1));
        }
        self.current = open_append(&name(0)).ok();
        self.size = self
            .current
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |metadata| metadata.len());
    }
}

fn open_append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Now, in milliseconds since the Unix epoch.
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// One log line, without its line ending.
pub fn format_line(event: &LogEvent) -> String {
    let level = match event.level {
        LogLevel::Error => "ERROR",
        LogLevel::Warning => "WARN ",
        LogLevel::Info => "INFO ",
        LogLevel::Debug => "DEBUG",
    };
    format!(
        "{} {level} [{}] {}",
        timestamp(event.time_ms),
        event.source,
        event.text
    )
}

/// A time in milliseconds since the Unix epoch as UTC in ISO 8601 form.
pub fn timestamp(unix_ms: u64) -> String {
    let ms = unix_ms % 1000;
    let seconds = unix_ms / 1000;
    let (second, minute, hour) = (seconds % 60, seconds / 60 % 60, seconds / 3600 % 24);
    let (year, month, day) = civil_from_days(seconds / 86_400);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

/// The Gregorian date of a day counted from 1970-01-01.
///
/// The calendar repeats every 400 years (146 097 days). Counting years from
/// 1 March puts the leap day at the end of the year, which makes the day of
/// the year a simple function of the month.
fn civil_from_days(days_since_epoch: u64) -> (u64, u64, u64) {
    // Days from 0000-03-01 to 1970-01-01.
    let days = days_since_epoch + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Months counted from March: 0 is March, 11 is February.
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("rustmodem-logger-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn timestamps_are_utc_iso_8601() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(timestamp(951_868_799_999), "2000-02-29T23:59:59.999Z");
        assert_eq!(timestamp(951_868_800_000), "2000-03-01T00:00:00.000Z");
        assert_eq!(timestamp(1_800_000_000_123), "2027-01-15T08:00:00.123Z");
        assert_eq!(timestamp(4_102_444_800_000), "2100-01-01T00:00:00.000Z");
        // 2100 is not a leap year.
        assert_eq!(timestamp(4_107_542_400_000), "2100-03-01T00:00:00.000Z");
        assert_eq!(timestamp(1_735_689_599_000), "2024-12-31T23:59:59.000Z");
    }

    #[test]
    fn a_line_has_time_level_source_and_text() {
        let line = format_line(&LogEvent {
            time_ms: 1_800_000_000_123,
            level: LogLevel::Info,
            source: "Line01".to_string(),
            text: "RMD1005: Line 1 state is now idle.".to_string(),
        });
        assert_eq!(
            line,
            "2027-01-15T08:00:00.123Z INFO  [Line01] RMD1005: Line 1 state is now idle."
        );
        for (level, word) in [
            (LogLevel::Error, "ERROR"),
            (LogLevel::Warning, "WARN "),
            (LogLevel::Debug, "DEBUG"),
        ] {
            let line = format_line(&LogEvent {
                time_ms: 0,
                level,
                source: "SVC".to_string(),
                text: "x".to_string(),
            });
            assert_eq!(line, format!("1970-01-01T00:00:00.000Z {word} [SVC] x"));
        }
    }

    #[test]
    fn lines_above_the_level_are_left_out() {
        let directory = temp_dir("level");
        let logger = Logger::open(
            &directory,
            LogLevel::Warning,
            false,
            None,
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        logger.log(LogLevel::Error, "SVC", "an error");
        logger.log(LogLevel::Warning, "SVC", "a warning");
        logger.log(LogLevel::Info, "SVC", "some information");
        logger.log(LogLevel::Debug, "SVC", "some detail");
        logger.message("CTL", &message("RMD2010", &[]));
        logger.message("CTL", &message("RMD2011", &["1"]));
        logger.flush();
        let text = std::fs::read_to_string(directory.join(LOG_FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].ends_with("ERROR [SVC] an error"));
        assert!(lines[1].ends_with("WARN  [SVC] a warning"));
        assert!(lines[2].ends_with("ERROR [CTL] RMD2010: The service is stopping."));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_log_is_appended_to_across_runs() {
        let directory = temp_dir("append");
        for run in 0..3 {
            let logger =
                Logger::open(&directory, LogLevel::Info, false, None, DEFAULT_MAX_BYTES).unwrap();
            logger.log(LogLevel::Info, "SVC", &format!("run {run}"));
        }
        let text = std::fs::read_to_string(directory.join(LOG_FILE)).unwrap();
        assert_eq!(text.lines().count(), 3);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn files_rotate_by_size_and_ten_are_kept() {
        let directory = temp_dir("rotate");
        // Each line is 58 bytes with its line feed, so four fit in a file.
        let logger = Logger::open(&directory, LogLevel::Info, false, None, 250).unwrap();
        for number in 0..100 {
            logger.log(LogLevel::Info, "SVC", &format!("line number {number:08}"));
        }
        logger.flush();

        let mut names: Vec<String> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut expected: Vec<String> = (1..KEEP_FILES)
            .map(|number| format!("rustmodem.{number}.log"))
            .collect();
        expected.push(LOG_FILE.to_string());
        expected.sort();
        assert_eq!(names, expected, "ten files, no more");

        // The newest lines are in the current file, the ones before in .1.
        let current = std::fs::read_to_string(directory.join(LOG_FILE)).unwrap();
        assert!(
            current
                .lines()
                .last()
                .unwrap()
                .ends_with("line number 00000099")
        );
        let previous = std::fs::read_to_string(directory.join("rustmodem.1.log")).unwrap();
        assert_eq!(previous.lines().count(), 4);
        assert!(previous.len() as u64 <= 250);
        let oldest = std::fs::read_to_string(directory.join("rustmodem.9.log")).unwrap();
        let first_kept: u32 = oldest
            .lines()
            .next()
            .unwrap()
            .rsplit(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            first_kept,
            100 - 4 * KEEP_FILES,
            "the oldest lines are gone"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_directory_that_cannot_be_used_is_message_0022() {
        let directory = temp_dir("blocked");
        // A file where the directory should be.
        std::fs::write(&directory, b"in the way").unwrap();
        let Err(problem) = Logger::open(&directory, LogLevel::Info, false, None, 1000) else {
            panic!("a file is not a directory");
        };
        assert_eq!(problem.id, "RMD0022");
        std::fs::remove_file(&directory).unwrap();
    }

    #[test]
    fn lines_reach_control_clients_through_the_hub() {
        // With no clients the hub takes the event and drops it; this checks
        // only that publishing through a logger does not disturb the file.
        let directory = temp_dir("hub");
        let hub = Hub::new();
        let logger = Logger::open(
            &directory,
            LogLevel::Debug,
            false,
            Some(hub),
            DEFAULT_MAX_BYTES,
        )
        .unwrap();
        logger.log(LogLevel::Debug, "Line01", "> AT");
        logger.flush();
        let text = std::fs::read_to_string(directory.join(LOG_FILE)).unwrap();
        assert!(text.trim_end().ends_with("DEBUG [Line01] > AT"));
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
