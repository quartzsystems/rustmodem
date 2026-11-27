// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the client prints: tables by default, for people.

use ctlproto::{
    CallInfo, CallRecord, ConnectInfo, ErrorInfo, Event, HuntGroupInfo, LineDiag, LineInfo,
    LineModem, LinkInfo, ListenerState, LogEvent, LogLevel, MediaInfo, ModemInfo, PortInfo,
    Registration, RouteInfo, SelfTestResult, ServiceInfo, SipMode, SipStatus, Stats,
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
    if line.engine == "modem" {
        pairs.push(("Modem", modem_settings(&line.modem)));
    }
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
        if let Some(modem) = &call.modem {
            pairs.push(("Modem state", modem_summary(modem)));
        }
    }
    if let Some(error) = &line.error {
        pairs.push(("Port error", error.to_string()));
    }
    fields(&pairs)
}

/// A line's modem settings in a line: a setting of the line's own is
/// marked, the others are those of `[modem]`.
fn modem_settings(modem: &LineModem) -> String {
    let own = |key: &str| {
        if modem.own.iter().any(|name| name == key) {
            " (line)"
        } else {
            ""
        }
    };
    format!(
        "{}{}, V.8 {}{}, {} dBm0{}, error control {}{}, compression {}{}, guard tone {}{}",
        modem.modulations.join(" then "),
        own("modulations"),
        if modem.v8 { "on" } else { "off" },
        own("v8"),
        modem.tx_level_dbm0,
        own("tx_level_dbm0"),
        modem.error_control,
        own("error_control"),
        if modem.compression { "on" } else { "off" },
        own("compression"),
        modem.guard_tone,
        own("guard_tone"),
    )
}

/// What a connection has over its modulation: `, LAPM, V.42bis`, or
/// nothing.
fn over_the_modem(connect: &ConnectInfo) -> String {
    let mut text = String::new();
    for part in [&connect.protocol, &connect.compression]
        .into_iter()
        .flatten()
    {
        text.push_str(", ");
        text.push_str(part);
    }
    text
}

/// How many octets went into a compressor for each that came out.
fn ratio(plain: u32, coded: u32) -> String {
    if plain == 0 || coded == 0 {
        return "no data yet".to_string();
    }
    format!("{:.1}:1", f64::from(plain) / f64::from(coded))
}

/// A call's data link in a line.
fn link_summary(link: &LinkInfo) -> String {
    let what = match (link.stage.as_str(), link.end.as_deref()) {
        ("down", Some(end)) => format!("ended ({end})"),
        (stage, _) => stage.to_string(),
    };
    if link.window_size == 0 && link.frames_tx == 0 && link.frames_rx == 0 {
        return what;
    }
    let direction = |mode: &str, plain: u32, coded: u32| match mode {
        "none" => "none".to_string(),
        mode => format!("{mode} {}", ratio(plain, coded)),
    };
    format!(
        "{what}, frames {} out and {} in, {} bad, {} sent again, window {} of {}{}{}, \
         compression out {}, in {}",
        link.frames_tx,
        link.frames_rx,
        link.frame_errors,
        link.retransmissions,
        link.window,
        link.window_size,
        if link.own_busy { ", host busy" } else { "" },
        if link.peer_busy { ", far end busy" } else { "" },
        direction(&link.compression_tx, link.tx_octets_in, link.tx_octets_out),
        direction(&link.compression_rx, link.rx_octets_out, link.rx_octets_in),
    )
}

/// The constellation in a line: how many points, of which constellation,
/// and how far the farthest is from the ideal point nearest it, in units
/// of the grid, where a unit is the distance to a wrong decision at 2400
/// bit/s.
fn constellation_summary(diag: &LineDiag) -> String {
    let unit = f64::from(diag.constellation_unit.max(1));
    // The ideal points are at 1 and 3 units each way: the sixteen of
    // them, or at 1200 bit/s the four at (3, 1) turned through the
    // quadrants.
    let ideal: Vec<(f64, f64)> = match diag.constellation_bits {
        4 => [-3.0, -1.0, 1.0, 3.0]
            .iter()
            .flat_map(|x| [-3.0, -1.0, 1.0, 3.0].map(|y| (*x, y)))
            .collect(),
        _ => vec![(3.0, 1.0), (-1.0, 3.0), (-3.0, -1.0), (1.0, -3.0)],
    };
    let mut farthest = 0f64;
    let mut count = 0;
    for [re, im] in diag.constellation.as_chunks::<2>().0 {
        let (x, y) = (f64::from(*re) / unit, f64::from(*im) / unit);
        let nearest = ideal
            .iter()
            .map(|(ix, iy)| ((x - ix).powi(2) + (y - iy).powi(2)).sqrt())
            .fold(f64::INFINITY, f64::min);
        farthest = farthest.max(nearest);
        count += 1;
    }
    format!(
        "{count} points of {}, the farthest {farthest:.2} of a unit from its place",
        if diag.constellation_bits == 4 {
            "sixteen"
        } else {
            "four"
        }
    )
}

/// A level in tenths of a dBm0, or what stands for none.
fn dbm0(tenths: Option<i32>) -> String {
    match tenths {
        Some(tenths) => format!("{:.1} dBm0", f64::from(tenths) / 10.0),
        None => "none".to_string(),
    }
}

/// The modem of a call in a line.
fn modem_summary(modem: &ModemInfo) -> String {
    format!(
        "{}, receiving {}, sending {}, carrier {}, {:+.1} Hz, quality {}%, {} framing errors, \
         {} retrains",
        modem.state,
        dbm0(modem.rx_level_tenth_dbm0),
        dbm0(modem.tx_level_tenth_dbm0),
        if modem.carrier { "detected" } else { "absent" },
        f64::from(modem.frequency_error_mhz) / 1000.0,
        modem.quality_percent,
        modem.framing_errors,
        modem.retrains,
    )
}

/// The scope as one line of text: a character for each sample, from the
/// lowest to the highest.
fn scope_text(scope: &[i8]) -> String {
    const MARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    scope
        .iter()
        .map(|sample| MARKS[((i32::from(*sample) + 128) * 8 / 256).clamp(0, 7) as usize])
        .collect()
}

/// The diagnostics of a line, as `diag` prints them.
pub fn diag(diag: &LineDiag) -> String {
    let Some(modem) = &diag.modem else {
        return format!(
            "Line {} has no modem to show: it has no call, or its engine is not the modem.\n",
            diag.line
        );
    };
    let mut pairs = vec![
        ("Line", diag.line.to_string()),
        ("State", modem.state.clone()),
        ("Receive level", dbm0(modem.rx_level_tenth_dbm0)),
        ("Transmit level", dbm0(modem.tx_level_tenth_dbm0)),
        (
            "Carrier",
            if modem.carrier { "detected" } else { "absent" }.to_string(),
        ),
        (
            "Frequency error",
            format!("{:+.1} Hz", f64::from(modem.frequency_error_mhz) / 1000.0),
        ),
        ("Quality", format!("{}%", modem.quality_percent)),
        ("Framing errors", modem.framing_errors.to_string()),
        ("Retrains", modem.retrains.to_string()),
    ];
    let decibels = |tenths: i32| format!("{:.1} dB", f64::from(tenths) / 10.0);
    if let Some(snr) = modem.snr_tenth_db {
        pairs.push(("Signal to noise", decibels(snr)));
    }
    if let Some(error) = modem.equalizer_error_tenth_db {
        pairs.push(("Equaliser error", decibels(error)));
    }
    if let Some(ppm) = modem.timing_offset_ppm {
        pairs.push(("Timing offset", format!("{ppm:+} ppm")));
    }
    if let Some(link) = &modem.link {
        pairs.push(("Link", link_summary(link)));
    }
    if !diag.constellation.is_empty() {
        pairs.push(("Constellation", constellation_summary(diag)));
    }
    if !diag.history.is_empty() {
        let history: Vec<String> = diag
            .history
            .iter()
            .map(|change| {
                format!(
                    "{} at {}.{:03} s",
                    change.state,
                    change.at_ms / 1000,
                    change.at_ms % 1000
                )
            })
            .collect();
        pairs.push(("History", history.join(", ")));
    }
    if !diag.scope.is_empty() {
        pairs.push((
            "Scope",
            format!("{} ({})", scope_text(&diag.scope), diag.scope_kind),
        ));
    }
    fields(&pairs)
}

/// The result of a self-test, as `selftest` prints it.
pub fn selftest(result: &SelfTestResult) -> String {
    let mut pairs = vec![
        (
            "Self-test",
            format!("line {} calls line {}", result.a, result.b),
        ),
        (
            "Connect",
            match (&result.connect, result.connect_ms) {
                (Some(connect), Some(ms)) => format!(
                    "{}{}{} after {}.{} s of the call",
                    rate(connect.rate),
                    match &connect.carrier {
                        Some(carrier) => format!(" {carrier}"),
                        None => String::new(),
                    },
                    over_the_modem(connect),
                    ms / 1000,
                    ms % 1000 / 100
                ),
                _ => "none".to_string(),
            },
        ),
        (
            "Data",
            format!(
                "{} of {} octets whole from {} to {}, {} of {} from {} to {}",
                result.octets_a_to_b,
                result.octets,
                result.a,
                result.b,
                result.octets_b_to_a,
                result.octets,
                result.b,
                result.a
            ),
        ),
        (
            "Time",
            format!(
                "{} of call in {}.{} s",
                duration(u64::from(result.call_ms)),
                result.elapsed_ms / 1000,
                result.elapsed_ms % 1000 / 100
            ),
        ),
        (
            "Result",
            if result.passed { "passed" } else { "FAILED" }.to_string(),
        ),
    ];
    if let Some(error) = &result.error {
        pairs.push(("Reason", error.to_string()));
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
                    Some(connect) => format!("{}{}", rate(connect.rate), over_the_modem(connect)),
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
        Event::LineDiag(diag) => match &diag.modem {
            Some(modem) => format!("line {} modem: {}", diag.line, modem_summary(modem)),
            None => format!("line {} modem: none", diag.line),
        },
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
