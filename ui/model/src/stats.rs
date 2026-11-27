// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The two statistics panels: what each row says.

use ctlproto::{ListenerState, ServiceInfo, Stats};

use crate::format::{EMPTY, TimeZone, csv_row, duration, grouped, percentages, uptime};

/// How many connect rates the panel has room for.
pub const RATE_ROWS: usize = 5;

/// One row of the connect rates: a label, how much of the bar is filled,
/// and the same as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateRow {
    /// What was negotiated, for example `2400 V.22bis`.
    pub label: String,
    /// The share of the connected calls, 0 to 100.
    pub percent: u32,
    /// How many calls.
    pub count: u64,
}

/// What the statistics panels show.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StatsView {
    /// The rows of "Inbound call statistics": label and value.
    pub calls: Vec<(&'static str, String)>,
    /// The connect rates of "Negotiation and carrier", at most
    /// [`RATE_ROWS`], largest first. Their percentages add up to 100.
    pub rates: Vec<RateRow>,
    /// The rows under the connect rates: label and value.
    pub carrier: Vec<(&'static str, String)>,
}

/// The labels of the rows of "Inbound call statistics", in order.
pub const CALL_LABELS: [&str; 12] = [
    "Calls answered",
    "Connected successfully",
    "Failed to connect",
    "Rejected (line busy)",
    "Caller hung up early",
    "Avg. call length",
    "Longest call",
    "Peak lines in use",
    "Bytes in / out",
    "AT commands parsed",
    "Service uptime",
    "Config loaded",
];

/// The labels of the rows under the connect rates, in order.
pub const CARRIER_LABELS: [&str; 6] = [
    "Retrains / renegotiations",
    "Error correction",
    "Softmodem engine",
    "RTP codec / ptime",
    "Jitter (avg / max)",
    "Packet loss",
];

/// What a connect rate is called: the rate and the carrier, as far as
/// they are known.
fn rate_label(rate: u32, carrier: Option<&str>) -> String {
    match (rate, carrier) {
        (0, None) => "Rate not known".to_string(),
        (0, Some(carrier)) => carrier.to_string(),
        (rate, None) => format!("{rate} bit/s"),
        (rate, Some(carrier)) => format!("{rate} {carrier}"),
    }
}

impl StatsView {
    /// The panels with nothing known: every value an em dash. This is
    /// what they show while the manager is not connected.
    pub fn empty() -> StatsView {
        StatsView {
            calls: CALL_LABELS
                .iter()
                .map(|label| (*label, EMPTY.to_string()))
                .collect(),
            rates: Vec::new(),
            carrier: CARRIER_LABELS
                .iter()
                .map(|label| (*label, EMPTY.to_string()))
                .collect(),
        }
    }

    /// The panels for what the service reports. `engines` is the engines
    /// the lines run, and `now_ms` the time now, for the uptime.
    pub fn new(
        stats: &Stats,
        info: &ServiceInfo,
        engines: &str,
        now_ms: u64,
        zone: &dyn TimeZone,
    ) -> StatsView {
        let length = |ms: u64| {
            // With no connected call ended yet there is no length to
            // give.
            if stats.longest_call_ms == 0 && stats.avg_call_ms == 0 {
                EMPTY.to_string()
            } else {
                duration(ms)
            }
        };
        let loaded = zone.wall(info.config_loaded_ms);
        let today = zone.wall(now_ms).date() == loaded.date();
        let values = [
            grouped(stats.calls_answered),
            grouped(stats.calls_connected),
            grouped(stats.calls_failed),
            grouped(stats.calls_rejected),
            grouped(stats.calls_abandoned),
            length(stats.avg_call_ms),
            length(stats.longest_call_ms),
            stats.peak_lines_in_use.to_string(),
            format!("{} / {}", grouped(stats.bytes_in), grouped(stats.bytes_out)),
            grouped(stats.at_commands),
            uptime(now_ms.saturating_sub(info.started_ms)),
            // The time alone if it was today, and the date with it if not.
            if today {
                loaded.time()
            } else {
                loaded.date_time()
            },
        ];

        // The connect rates: the largest groups, and if there are more
        // than fit, the rest together in the last row.
        let mut groups: Vec<(String, u64)> = stats
            .connects
            .iter()
            .map(|connect| {
                (
                    rate_label(connect.rate, connect.carrier.as_deref()),
                    connect.count,
                )
            })
            .collect();
        if groups.len() > RATE_ROWS {
            let rest: u64 = groups[RATE_ROWS - 1..].iter().map(|(_, count)| count).sum();
            groups.truncate(RATE_ROWS - 1);
            groups.push(("Other".to_string(), rest));
        }
        let counts: Vec<u64> = groups.iter().map(|(_, count)| *count).collect();
        let rates = groups
            .into_iter()
            .zip(percentages(&counts))
            .map(|((label, count), percent)| RateRow {
                label,
                percent,
                count,
            })
            .collect();

        let connected = stats.calls_connected > 0;
        let named = |tallies: &[ctlproto::Tally]| match tallies.first() {
            Some(most) if tallies.len() == 1 => most.name.clone(),
            Some(most) => format!("{} (and {} more)", most.name, tallies.len() - 1),
            None => EMPTY.to_string(),
        };
        let carrier = [
            // A count means something once a call has connected; before
            // that, and with an engine that never retrains, there is
            // nothing to show.
            if connected && stats.retrains > 0 {
                grouped(stats.retrains)
            } else {
                EMPTY.to_string()
            },
            // The share of the calls that connected with error control:
            // LAPM is the one there is.
            match stats.protocols.first() {
                Some(_) => {
                    let corrected: u64 = stats.protocols.iter().map(|tally| tally.count).sum();
                    let calls = stats.calls_connected.max(corrected).max(1);
                    format!(
                        "{} on {}% of calls",
                        named(&stats.protocols),
                        (corrected * 100 + calls / 2) / calls
                    )
                }
                None if connected => "None".to_string(),
                None => EMPTY.to_string(),
            },
            if engines.is_empty() {
                EMPTY.to_string()
            } else {
                engines.to_string()
            },
            // A service with no SIP has no RTP to say anything of.
            if stats.codec.is_empty() {
                EMPTY.to_string()
            } else {
                format!("{} / {} ms", stats.codec, stats.ptime_ms)
            },
            if stats.sip.listener == ListenerState::Listening {
                format!("{:.1} / {:.1} ms", stats.jitter_ms_avg, stats.jitter_ms_max)
            } else {
                EMPTY.to_string()
            },
            if stats.sip.listener == ListenerState::Listening {
                format!("{:.2} %", stats.packet_loss_percent)
            } else {
                EMPTY.to_string()
            },
        ];
        StatsView {
            calls: CALL_LABELS.iter().copied().zip(values).collect(),
            rates,
            carrier: CARRIER_LABELS.iter().copied().zip(carrier).collect(),
        }
    }

    /// The statistics and the connect rates as a CSV file: a statistic and
    /// its value to a row, then a rate, its calls and its share to a row.
    pub fn csv(&self) -> String {
        let mut out = csv_row(&["Statistic", "Value"]);
        for (label, value) in self.calls.iter().chain(&self.carrier) {
            out.push_str(&csv_row(&[label, value]));
        }
        out.push_str("\r\n");
        out.push_str(&csv_row(&["Connect rate", "Calls", "Percent"]));
        for rate in &self.rates {
            out.push_str(&csv_row(&[
                &rate.label,
                &rate.count.to_string(),
                &rate.percent.to_string(),
            ]));
        }
        out
    }
}
