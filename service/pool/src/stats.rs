// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The statistics of the pool since it started.
//!
//! The call counts are of calls offered to lines. With the null switch
//! every call has two ends in the pool, the line that dialled and the line
//! that was called; counting the called end only counts each call once,
//! and is what stays true when calls come from a network instead.

use std::collections::BTreeMap;

use ctlproto::{CallRecord, ConnectInfo, ConnectTally, Direction, EndReason, Stats, Tally};

/// What the pool counts as it goes.
#[derive(Debug, Default)]
pub(crate) struct Tallies {
    answered: u64,
    connected: u64,
    failed: u64,
    rejected: u64,
    abandoned: u64,
    /// The connected calls that have ended, and their total length.
    ended: u64,
    ended_ms: u64,
    longest_ms: u64,
    peak_in_use: u32,
    bytes_in: u64,
    bytes_out: u64,
    at_commands: u64,
    retrains: u64,
    connects: BTreeMap<(u32, Option<String>), u64>,
    protocols: BTreeMap<String, u64>,
    compressions: BTreeMap<String, u64>,
}

impl Tallies {
    /// A line answered a call offered to it.
    pub fn answered(&mut self) {
        self.answered += 1;
    }

    /// The modems of a call offered to a line connected.
    pub fn connected(&mut self, connect: &ConnectInfo) {
        self.connected += 1;
        *self
            .connects
            .entry((connect.rate, connect.carrier.clone()))
            .or_default() += 1;
        if let Some(protocol) = &connect.protocol {
            *self.protocols.entry(protocol.clone()).or_default() += 1;
        }
        if let Some(compression) = &connect.compression {
            *self.compressions.entry(compression.clone()).or_default() += 1;
        }
    }

    /// A call was turned away before any line was rung for it, or the line
    /// rung for it could not take it.
    pub fn rejected(&mut self) {
        self.rejected += 1;
    }

    /// A call ended. `answered` says whether the line had answered it.
    pub fn ended(&mut self, record: &CallRecord, answered: bool) {
        if record.direction != Direction::Incoming {
            return;
        }
        if record.reason == EndReason::Abandoned {
            self.abandoned += 1;
        }
        match record.connected_ms {
            Some(connected_ms) => {
                let length = record.ended_ms.saturating_sub(connected_ms);
                self.ended += 1;
                self.ended_ms += length;
                self.longest_ms = self.longest_ms.max(length);
            }
            None if answered => self.failed += 1,
            None => {}
        }
    }

    /// This many lines have a call now.
    pub fn in_use(&mut self, lines: u32) {
        self.peak_in_use = self.peak_in_use.max(lines);
    }

    /// Data bytes went from a host to a far end.
    pub fn bytes_out(&mut self, count: u64) {
        self.bytes_out += count;
    }

    /// Data bytes came from a far end to a host.
    pub fn bytes_in(&mut self, count: u64) {
        self.bytes_in += count;
    }

    /// A line parsed an AT command line.
    pub fn at_command(&mut self) {
        self.at_commands += 1;
    }

    /// An engine retrained or changed rate.
    pub fn retrain(&mut self) {
        self.retrains += 1;
    }

    /// The statistics now, with `lines_in_use` lines in a call.
    pub fn snapshot(&self, lines_in_use: u32) -> Stats {
        let mut connects: Vec<ConnectTally> = self
            .connects
            .iter()
            .map(|((rate, carrier), count)| ConnectTally {
                rate: *rate,
                carrier: carrier.clone(),
                count: *count,
            })
            .collect();
        // Largest count first; among equals the faster rate, then the
        // carrier name, so the order does not depend on arrival.
        connects.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(b.rate.cmp(&a.rate))
                .then(a.carrier.cmp(&b.carrier))
        });
        Stats {
            calls_answered: self.answered,
            calls_connected: self.connected,
            calls_failed: self.failed,
            calls_rejected: self.rejected,
            calls_abandoned: self.abandoned,
            avg_call_ms: self.ended_ms.checked_div(self.ended).unwrap_or(0),
            longest_call_ms: self.longest_ms,
            peak_lines_in_use: self.peak_in_use.max(lines_in_use),
            lines_in_use,
            bytes_in: self.bytes_in,
            bytes_out: self.bytes_out,
            at_commands: self.at_commands,
            connects,
            retrains: self.retrains,
            protocols: named(&self.protocols),
            compressions: named(&self.compressions),
        }
    }
}

/// A tally by name, largest count first and then by name.
fn named(counts: &BTreeMap<String, u64>) -> Vec<Tally> {
    let mut tallies: Vec<Tally> = counts
        .iter()
        .map(|(name, count)| Tally {
            name: name.clone(),
            count: *count,
        })
        .collect();
    tallies.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
    tallies
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(rate: u32, carrier: Option<&str>) -> ConnectInfo {
        ConnectInfo {
            rate,
            carrier: carrier.map(str::to_string),
            protocol: None,
            compression: None,
        }
    }

    fn record(
        direction: Direction,
        connected: Option<u64>,
        ended: u64,
        why: EndReason,
    ) -> CallRecord {
        CallRecord {
            id: 1,
            line: 1,
            direction,
            number: "2".to_string(),
            started_ms: 0,
            connected_ms: connected,
            ended_ms: ended,
            reason: why,
            connect: None,
            bytes_tx: 0,
            bytes_rx: 0,
        }
    }

    #[test]
    fn nothing_has_happened_at_the_start() {
        assert_eq!(Tallies::default().snapshot(0), Stats::default());
    }

    #[test]
    fn connects_are_grouped_and_the_largest_group_is_first() {
        let mut tallies = Tallies::default();
        for (rate, carrier) in [
            (2400, Some("V.22bis")),
            (300, Some("V.21")),
            (2400, Some("V.22bis")),
            (1200, Some("V.22")),
            (1200, Some("Bell 212A")),
            (0, None),
            (2400, Some("V.22bis")),
        ] {
            tallies.connected(&connect(rate, carrier));
        }
        let stats = tallies.snapshot(0);
        assert_eq!(stats.calls_connected, 7);
        let groups: Vec<(u32, Option<&str>, u64)> = stats
            .connects
            .iter()
            .map(|c| (c.rate, c.carrier.as_deref(), c.count))
            .collect();
        assert_eq!(
            groups,
            [
                (2400, Some("V.22bis"), 3),
                (1200, Some("Bell 212A"), 1),
                (1200, Some("V.22"), 1),
                (300, Some("V.21"), 1),
                (0, None, 1),
            ]
        );
    }

    #[test]
    fn protocols_and_compressions_are_tallied_when_an_engine_names_them() {
        let mut tallies = Tallies::default();
        let mut info = connect(2400, Some("V.22bis"));
        tallies.connected(&info);
        info.protocol = Some("LAPM".to_string());
        tallies.connected(&info);
        info.compression = Some("V.42bis".to_string());
        tallies.connected(&info);
        info.protocol = Some("MNP".to_string());
        tallies.connected(&info);
        let stats = tallies.snapshot(0);
        let names = |tallies: &[Tally]| -> Vec<(String, u64)> {
            tallies.iter().map(|t| (t.name.clone(), t.count)).collect()
        };
        assert_eq!(
            names(&stats.protocols),
            [("LAPM".to_string(), 2), ("MNP".to_string(), 1)]
        );
        assert_eq!(names(&stats.compressions), [("V.42bis".to_string(), 2)]);
    }

    #[test]
    fn only_calls_offered_to_lines_are_counted_when_they_end() {
        let mut tallies = Tallies::default();
        // The dialling end of a call is not counted.
        tallies.ended(
            &record(Direction::Outgoing, Some(1000), 61_000, EndReason::Hangup),
            false,
        );
        assert_eq!(tallies.snapshot(0), Stats::default());

        tallies.ended(
            &record(Direction::Incoming, Some(1000), 61_000, EndReason::Remote),
            true,
        );
        tallies.ended(
            &record(Direction::Incoming, Some(1000), 11_000, EndReason::Hangup),
            true,
        );
        // Answered, never connected.
        tallies.ended(
            &record(Direction::Incoming, None, 5000, EndReason::Remote),
            true,
        );
        // Never answered: the caller gave up.
        tallies.ended(
            &record(Direction::Incoming, None, 5000, EndReason::Abandoned),
            false,
        );
        let stats = tallies.snapshot(0);
        assert_eq!(stats.avg_call_ms, 35_000);
        assert_eq!(stats.longest_call_ms, 60_000);
        assert_eq!(stats.calls_failed, 1);
        assert_eq!(stats.calls_abandoned, 1);
    }

    #[test]
    fn the_peak_is_never_below_what_is_in_use_now() {
        let mut tallies = Tallies::default();
        tallies.in_use(3);
        tallies.in_use(1);
        assert_eq!(tallies.snapshot(1).peak_lines_in_use, 3);
        assert_eq!(tallies.snapshot(4).peak_lines_in_use, 4);
        assert_eq!(tallies.snapshot(4).lines_in_use, 4);
    }
}
