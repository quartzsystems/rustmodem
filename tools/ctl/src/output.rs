// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the client prints: tables by default, for people.

use ctlproto::{
    CallInfo, CallRecord, ErrorInfo, Event, HuntGroupInfo, LineInfo, ListenerState, LogEvent,
    LogLevel, MediaInfo, PortInfo, Registration, RouteInfo, ServiceInfo, SipMode, SipStatus, Stats,
};

/// A column of a table.
struct Column {
    title: &'static str,
    /// Numbers are set to the right.
    right: bool,
}

const fn left(title: &'static str) -> Column {
    Column {
        title,
        right: false,
    }
}

const fn right(title: &'static str) -> Column {
    Column { title, right: true }
}

/// Lays rows out under their column titles, each column as wide as its
/// widest cell, two spaces apart, with nothing trailing.
fn table(columns: &[Column], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = columns.iter().map(|c| c.title.chars().count()).collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    let titles: Vec<String> = columns.iter().map(|c| c.title.to_string()).collect();
    for row in std::iter::once(&titles).chain(rows) {
        let mut line = String::new();
        for (index, cell) in row.iter().enumerate() {
            let width = widths[index];
            if index > 0 {
                line.push_str("  ");
            }
            if columns[index].right {
                line.push_str(&format!("{cell:>width$}"));
            } else {
                line.push_str(&format!("{cell:<width$}"));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Label and value lines, the values aligned.
pub fn fields(pairs: &[(&str, String)]) -> String {
    let width = pairs
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    pairs
        .iter()
        .map(|(label, value)| {
            format!(
                "{:<width$}  {value}\n",
                format!("{label}:"),
                width = width + 1
            )
        })
        .collect()
}

/// A time in milliseconds since the Unix epoch as `YYYY-MM-DD HH:MM:SS`,
/// in UTC.
pub fn timestamp(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000;
    let (second, minute, hour) = (seconds % 60, seconds / 60 % 60, seconds / 3600 % 24);
    // The Gregorian calendar repeats every 400 years (146 097 days).
    // Counting years from 1 March puts the leap day last, which makes the
    // day of the year a simple function of the month.
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
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// A length of time in milliseconds as `H:MM:SS`.
pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// The rate of a connection for people: 0 means it is not known.
fn rate(rate: u32) -> String {
    if rate == 0 {
        "unknown".to_string()
    } else {
        format!("{rate} bit/s")
    }
}

/// `info`
pub fn info(info: &ServiceInfo) -> String {
    fields(&[
        ("Version", info.version.clone()),
        ("Protocol", info.protocol.to_string()),
        ("Pipe", info.pipe.clone()),
        ("Configuration", info.config_path.clone()),
        ("Lines", info.lines.to_string()),
        ("Started", format!("{} UTC", timestamp(info.started_ms))),
        ("Uptime", duration(info.uptime_ms)),
        ("Mode", info.mode.as_str().to_string()),
        (
            "Config loaded",
            format!("{} UTC", timestamp(info.config_loaded_ms)),
        ),
    ])
}

/// `stats`
pub fn stats(stats: &Stats) -> String {
    let mut out = fields(&[
        ("Calls answered", stats.calls_answered.to_string()),
        ("Connected", stats.calls_connected.to_string()),
        ("Failed to connect", stats.calls_failed.to_string()),
        ("Rejected", stats.calls_rejected.to_string()),
        ("Abandoned", stats.calls_abandoned.to_string()),
        ("Average call", duration(stats.avg_call_ms)),
        ("Longest call", duration(stats.longest_call_ms)),
        (
            "Lines in use",
            format!("{} (peak {})", stats.lines_in_use, stats.peak_lines_in_use),
        ),
        ("Bytes in", stats.bytes_in.to_string()),
        ("Bytes out", stats.bytes_out.to_string()),
        ("AT commands", stats.at_commands.to_string()),
        ("Retrains", stats.retrains.to_string()),
    ]);
    // A service with no SIP has none of what follows to show.
    if stats.sip.listener != ListenerState::Off {
        out.push_str(&fields(&[
            (
                "SIP",
                format!(
                    "{}, {} of {} lines registered, {} requests dropped",
                    stats.sip.listener.as_str(),
                    stats.sip.registered,
                    stats.sip.registering,
                    stats.sip.dropped_untrusted
                ),
            ),
            (
                "RTP",
                format!(
                    "{} kbit/s, {} at {} ms",
                    stats.rtp_kbps, stats.codec, stats.ptime_ms
                ),
            ),
            (
                "Jitter",
                format!(
                    "{:.1} ms average, {:.1} ms at most",
                    stats.jitter_ms_avg, stats.jitter_ms_max
                ),
            ),
            ("Packet loss", format!("{:.2} %", stats.packet_loss_percent)),
            ("CPU", format!("{:.1} %", stats.cpu_percent)),
        ]));
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    for connect in &stats.connects {
        rows.push(vec![
            "connect".to_string(),
            match &connect.carrier {
                Some(carrier) => format!("{}, {carrier}", rate(connect.rate)),
                None => rate(connect.rate),
            },
            connect.count.to_string(),
        ]);
    }
    let named = [
        ("protocol", &stats.protocols),
        ("compression", &stats.compressions),
    ];
    for (what, tallies) in named {
        for tally in tallies {
            rows.push(vec![
                what.to_string(),
                tally.name.clone(),
                tally.count.to_string(),
            ]);
        }
    }
    if !rows.is_empty() {
        out.push('\n');
        out.push_str(&table(
            &[left("NEGOTIATED"), left("WHAT"), right("CALLS")],
            &rows,
        ));
    }
    out
}

/// `ports`
pub fn ports(ports: &[PortInfo]) -> String {
    let columns = [
        left("PORT"),
        left("KIND"),
        left("PAIR"),
        left("HOST PORT"),
        left("MODEM"),
        right("LINE"),
    ];
    let or_dash = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_string());
    let rows: Vec<Vec<String>> = ports
        .iter()
        .map(|port| {
            vec![
                port.name.clone(),
                port.kind.as_str().to_string(),
                or_dash(&port.pair),
                or_dash(&port.host_port),
                or_dash(&port.modem),
                port.line
                    .map_or_else(|| "-".to_string(), |id| id.to_string()),
            ]
        })
        .collect();
    table(&columns, &rows)
}

fn level(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "ERROR",
        LogLevel::Warning => "WARN",
        LogLevel::Info => "INFO",
        LogLevel::Debug => "DEBUG",
    }
}

/// `log`: the lines as the service log has them, the time in UTC.
pub fn log(lines: &[LogEvent]) -> String {
    lines
        .iter()
        .map(|line| {
            format!(
                "{}.{:03} {:<5} [{}] {}\n",
                timestamp(line.time_ms),
                line.time_ms % 1000,
                level(line.level),
                line.source,
                line.text
            )
        })
        .collect()
}

/// `lines`
pub fn lines(lines: &[LineInfo]) -> String {
    let columns = [
        right("ID"),
        left("NAME"),
        left("PORT"),
        left("STATE"),
        right("IN"),
        right("OUT"),
        right("CONNECTS"),
        right("TX"),
        right("RX"),
    ];
    let rows: Vec<Vec<String>> = lines
        .iter()
        .map(|line| {
            vec![
                line.id.to_string(),
                line.name.clone().unwrap_or_else(|| "-".to_string()),
                line.port.clone(),
                line.state.as_str().to_string(),
                line.counters.calls_in.to_string(),
                line.counters.calls_out.to_string(),
                line.counters.connects.to_string(),
                line.counters.bytes_tx.to_string(),
                line.counters.bytes_rx.to_string(),
            ]
        })
        .collect();
    table(&columns, &rows)
}

fn call_summary(call: &CallInfo) -> String {
    let connection = match &call.connect {
        Some(connect) => format!("connected, {}", rate(connect.rate)),
        None => "not connected".to_string(),
    };
    format!(
        "{} {} ({connection}), {} bytes sent, {} received",
        call.direction.as_str(),
        call.number,
        call.bytes_tx,
        call.bytes_rx
    )
}

/// `line <id>`
pub fn line(line: &LineInfo) -> String {
    let mut pairs = vec![
        ("Line", line.id.to_string()),
        ("Name", line.name.clone().unwrap_or_else(|| "-".to_string())),
        ("Backend", line.backend.clone()),
        ("Port", line.port.clone()),
        (
            "Enabled",
            if line.enabled { "yes" } else { "no" }.to_string(),
        ),
        ("State", line.state.as_str().to_string()),
        ("Auto-answer rings", line.auto_answer_rings.to_string()),
        ("Unknown commands", line.unknown_command.clone()),
        ("Calls in", line.counters.calls_in.to_string()),
        ("Calls out", line.counters.calls_out.to_string()),
        ("Connects", line.counters.connects.to_string()),
        ("Bytes sent", line.counters.bytes_tx.to_string()),
        ("Bytes received", line.counters.bytes_rx.to_string()),
        ("AT commands", line.counters.at_commands.to_string()),
    ];
    if let Some(binding) = &line.binding {
        pairs.push(("Binding", binding.clone()));
    }
    pairs.push(("Engine", line.engine.clone()));
    pairs.push((
        "SIP",
        match line.sip.mode {
            SipMode::None => "none".to_string(),
            SipMode::Trunk => "trunk".to_string(),
            SipMode::Registered => format!(
                "extension {} of {}{}",
                line.sip.extension.as_deref().unwrap_or("-"),
                line.sip.server.as_deref().unwrap_or("-"),
                match &line.sip.registration {
                    Some(registration) => format!(": {}", registration_text(registration)),
                    None => String::new(),
                }
            ),
        },
    ));
    if !line.hunt_groups.is_empty() {
        pairs.push(("Hunt groups", line.hunt_groups.join(", ")));
    }
    if line.capture {
        pairs.push(("Capture", "on".to_string()));
    }
    if let Some(call) = &line.call {
        pairs.push(("Call", call_summary(call)));
        if let Some(media) = &call.media {
            pairs.push(("RTP", media_summary(media)));
        }
    }
    if let Some(error) = &line.error {
        pairs.push(("Port error", error.to_string()));
    }
    fields(&pairs)
}

/// A registration in a few words.
fn registration_text(registration: &Registration) -> String {
    match (&registration.error, registration.expires_in_s) {
        (Some(error), _) => format!("{}, {error}", registration.state.as_str()),
        (None, Some(seconds)) => {
            format!("{} for {seconds} s more", registration.state.as_str())
        }
        (None, None) => registration.state.as_str().to_string(),
    }
}

/// The RTP of a call in a line.
fn media_summary(media: &MediaInfo) -> String {
    format!(
        "{} at {} ms{}, {} packets in, {} out, {} lost, {} late, {} duplicate, {} resync, \
         {} slips, jitter {:.1} ms",
        media.codec,
        media.ptime_ms,
        match &media.remote {
            Some(remote) => format!(" with {remote}"),
            None => String::new(),
        },
        media.rx_packets,
        media.tx_packets,
        media.lost,
        media.late,
        media.duplicate,
        media.resync,
        media.slips,
        f64::from(media.jitter_us_avg) / 1000.0
    )
}

/// `sip`
pub fn sip(status: &SipStatus) -> String {
    if status.listener == ListenerState::Off {
        return "SIP is off.\n".to_string();
    }
    let list = |values: &[String]| {
        if values.is_empty() {
            "none".to_string()
        } else {
            values.join(", ")
        }
    };
    let mut out = fields(&[
        (
            "Listener",
            format!("{} on {}", status.listener.as_str(), status.listen),
        ),
        ("Trusted peers", list(&status.trusted_peers)),
        (
            "Outbound peer",
            if status.outbound_peer.is_empty() {
                "none".to_string()
            } else {
                status.outbound_peer.clone()
            },
        ),
        ("Calls", status.calls.to_string()),
        ("Requests dropped", status.dropped_untrusted.to_string()),
    ]);
    if !status.registrations.is_empty() {
        let rows: Vec<Vec<String>> = status
            .registrations
            .iter()
            .map(|line| {
                vec![
                    line.line.to_string(),
                    line.user.clone(),
                    line.server.clone(),
                    registration_text(&line.registration),
                ]
            })
            .collect();
        out.push('\n');
        out.push_str(&table(
            &[
                right("LINE"),
                left("EXTENSION"),
                left("REGISTRAR"),
                left("REGISTRATION"),
            ],
            &rows,
        ));
    }
    out
}

/// `groups`
pub fn groups(groups: &[HuntGroupInfo]) -> String {
    let rows: Vec<Vec<String>> = groups
        .iter()
        .map(|group| {
            vec![
                group.name.clone(),
                group.pilot.clone(),
                group.strategy.clone(),
                group
                    .lines
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            ]
        })
        .collect();
    table(
        &[
            left("GROUP"),
            left("PILOT"),
            left("STRATEGY"),
            left("LINES"),
        ],
        &rows,
    )
}

/// `routes`
pub fn routes(routes: &[RouteInfo]) -> String {
    let rows: Vec<Vec<String>> = routes
        .iter()
        .map(|route| {
            vec![
                route.index.to_string(),
                route.called.clone(),
                route.target.clone(),
            ]
        })
        .collect();
    table(&[right("#"), left("CALLED"), left("TARGET")], &rows)
}

/// What `answer`, `hangup`, `reset`, `enable` and `disable` print: the
/// line's state afterwards.
pub fn acted(line: &LineInfo) -> String {
    format!("Line {}: {}\n", line.id, line.state.as_str())
}

/// `calls`
pub fn calls(calls: &[CallRecord]) -> String {
    let columns = [
        right("CALL"),
        right("LINE"),
        left("DIRECTION"),
        left("NUMBER"),
        left("STARTED (UTC)"),
        right("LENGTH"),
        left("RATE"),
        left("ENDED"),
        right("TX"),
        right("RX"),
    ];
    let rows: Vec<Vec<String>> = calls
        .iter()
        .map(|call| {
            vec![
                call.id.to_string(),
                call.line.to_string(),
                call.direction.as_str().to_string(),
                call.number.clone(),
                timestamp(call.started_ms),
                duration(call.ended_ms.saturating_sub(call.started_ms)),
                match &call.connect {
                    Some(connect) => rate(connect.rate),
                    None => "-".to_string(),
                },
                call.reason.as_str().to_string(),
                call.bytes_tx.to_string(),
                call.bytes_rx.to_string(),
            ]
        })
        .collect();
    table(&columns, &rows)
}

/// One event, as `watch` prints it.
pub fn event(event: &Event) -> String {
    match event {
        Event::LineState(line) => format!("line {} is {}", line.id, line.state.as_str()),
        Event::CallStarted(e) => format!(
            "line {} call {} started: {} {}",
            e.line,
            e.call.id,
            e.call.direction.as_str(),
            e.call.number
        ),
        Event::CallConnected(e) => format!(
            "line {} call {} connected: {}",
            e.line,
            e.call.id,
            rate(e.call.connect.as_ref().map_or(0, |c| c.rate))
        ),
        Event::CallEnded(record) => format!(
            "line {} call {} ended: {}, {} bytes sent, {} received",
            record.line,
            record.id,
            record.reason.as_str(),
            record.bytes_tx,
            record.bytes_rx
        ),
        Event::Log(log) => format!("log {} [{}] {}", level(log.level), log.source, log.text),
        Event::SipRegistration(e) => format!(
            "line {} registration to {} as {} is {}{}",
            e.line,
            e.server,
            e.user,
            e.registration.state.as_str(),
            match &e.registration.error {
                Some(error) => format!(": {error}"),
                None => String::new(),
            }
        ),
        // The message itself, set off from what is around it.
        Event::SipTrace(trace) => format!(
            "sip {} {}
{}",
            trace.direction.as_str(),
            trace.peer,
            trace.text.trim_end()
        ),
    }
}

/// An error: its id and text, and with `verbose` its cause and recovery.
pub fn error(error: &ErrorInfo, verbose: bool) -> String {
    if verbose {
        format!(
            "{error}\nCause: {}\nRecovery: {}\n",
            error.cause, error.recovery
        )
    } else {
        format!("{error}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_and_durations() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(timestamp(951_782_400_000), "2000-02-29 00:00:00");
        assert_eq!(timestamp(1_800_000_000_999), "2027-01-15 08:00:00");
        assert_eq!(timestamp(4_107_542_400_000), "2100-03-01 00:00:00");
        assert_eq!(duration(0), "0:00:00");
        assert_eq!(duration(61_999), "0:01:01");
        assert_eq!(duration(3_600_000 * 30 + 5000), "30:00:05");
    }

    #[test]
    fn tables_align_and_have_no_trailing_spaces() {
        let text = table(
            &[right("ID"), left("NAME"), right("N")],
            &[
                vec!["1".to_string(), "first".to_string(), "10".to_string()],
                vec!["200".to_string(), "x".to_string(), "5".to_string()],
            ],
        );
        assert_eq!(text, " ID  NAME    N\n  1  first  10\n200  x       5\n");
        assert!(text.lines().all(|line| line == line.trim_end()));
    }

    #[test]
    fn an_error_is_id_and_text_and_verbosely_cause_and_recovery() {
        let info = ErrorInfo::from(msg::message("RMD1001", &["9"]));
        assert_eq!(error(&info, false), "RMD1001: Line 9 does not exist.\n");
        let verbose = error(&info, true);
        assert!(verbose.starts_with("RMD1001: Line 9 does not exist.\nCause: "));
        assert!(verbose.contains("\nRecovery: "));
    }
}
