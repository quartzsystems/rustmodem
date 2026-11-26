// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The requests the manager has sent and not yet had an answer to.
//!
//! Every request carries an id, and the answer carries it back. What the
//! manager remembers of a request is what the answer is for and when to
//! give up on it: a request with no answer in [`TIMEOUT`] fails with
//! message `RMD4001`.

use std::time::{Duration, Instant};

use ctlproto::{Call, ClientFrame, ErrorInfo, encode};
use model::Command;
use msg::message;
use serde_json::Value;

/// How long the manager waits for an answer.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// What the answer to a request is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// `service.info`: the service the manager is connected to.
    Service,
    /// `subscribe`: the events.
    Subscribe,
    /// `lines.list`: every line.
    Lines,
    /// `stats.get`: the statistics.
    Stats,
    /// `log.tail`: the event log so far.
    LogTail,
    /// `calls.list`: the ended calls.
    Calls,
    /// A command on one line, which answers with the line afterwards.
    Line(Command),
    /// `config.reload`.
    Reload,
    /// `lines.remove`, of the line with this id.
    Remove(u16),
    /// A request something is waiting for the answer to, with the number
    /// it knows the request by: a dialog, which goes on only once it has
    /// the answer.
    Waited(u32),
}

impl Purpose {
    /// Whether the user asked for this, and so is told when it fails.
    /// What the manager asks for by itself fails into the status bar.
    pub fn asked_for(self) -> bool {
        matches!(
            self,
            Purpose::Line(_) | Purpose::Reload | Purpose::Remove(_)
        )
    }
}

struct Pending {
    id: u64,
    method: &'static str,
    purpose: Purpose,
    deadline: Instant,
}

/// The requests that are waiting for their answers.
#[derive(Default)]
pub struct Requests {
    last_id: u64,
    pending: Vec<Pending>,
}

impl Requests {
    /// No requests.
    pub fn new() -> Requests {
        Requests::default()
    }

    /// Starts a request at `now`. Returns the line to send to the
    /// service.
    pub fn begin(&mut self, call: &Call, purpose: Purpose, now: Instant) -> String {
        self.last_id += 1;
        self.pending.push(Pending {
            id: self.last_id,
            method: call.method(),
            purpose,
            deadline: now + TIMEOUT,
        });
        encode(&ClientFrame::Request(call.request(self.last_id)))
    }

    /// Whether a request for this is waiting for its answer.
    pub fn waiting_for(&self, purpose: Purpose) -> bool {
        self.pending
            .iter()
            .any(|request| request.purpose == purpose)
    }

    /// How many requests are waiting.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Whether no request is waiting.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The answer with this id arrived. Returns what it is for; `None`
    /// for an answer to a request that was given up on, or never made.
    pub fn answered(&mut self, id: u64) -> Option<Purpose> {
        let index = self.pending.iter().position(|request| request.id == id)?;
        Some(self.pending.remove(index).purpose)
    }

    /// Gives up on the requests whose time has run out at `now`, oldest
    /// first. Each comes back with the message that says so.
    pub fn expire(&mut self, now: Instant) -> Vec<(Purpose, ErrorInfo)> {
        let mut expired = Vec::new();
        self.pending.retain(|request| {
            if request.deadline > now {
                return true;
            }
            expired.push((request.purpose, timed_out(request.method)));
            false
        });
        expired
    }

    /// Forgets every request: the connection they were sent on is gone.
    /// Returns what the requests were for, oldest first.
    pub fn abandon(&mut self) -> Vec<Purpose> {
        self.pending
            .drain(..)
            .map(|request| request.purpose)
            .collect()
    }
}

/// The message for a request that had no answer.
pub fn timed_out(method: &str) -> ErrorInfo {
    let seconds = TIMEOUT.as_secs().to_string();
    ErrorInfo::from(message("RMD4001", &[method, &seconds]))
}

/// Reads the result of a request as what the method answers with. A
/// result that is something else is message `RMD2013`.
pub fn read<T: serde::de::DeserializeOwned>(result: Value) -> Result<T, ErrorInfo> {
    serde_json::from_value(result)
        .map_err(|e| ErrorInfo::from(message("RMD2013", &[&e.to_string()])))
}

#[cfg(test)]
mod tests {
    use ctlproto::{ClientFrame, LineRef, ServiceMode, decode_client};

    use super::*;

    fn request_of(line: &str) -> ctlproto::RequestFrame {
        match decode_client(line).expect("the line is a frame") {
            ClientFrame::Request(request) => request,
            other => panic!("not a request: {other:?}"),
        }
    }

    #[test]
    fn each_request_has_its_own_id_and_its_answer_finds_its_purpose() {
        let now = Instant::now();
        let mut requests = Requests::new();
        let first = request_of(&requests.begin(&Call::LinesList, Purpose::Lines, now));
        let second = request_of(&requests.begin(
            &Call::LinesAnswer(LineRef { id: 3 }),
            Purpose::Line(Command::Answer),
            now,
        ));
        assert_eq!(first.method, "lines.list");
        assert_eq!(second.method, "lines.answer");
        assert_eq!(second.params["id"], 3);
        assert_ne!(first.id, second.id);
        assert_eq!(requests.len(), 2);
        assert!(requests.waiting_for(Purpose::Lines));
        assert!(!requests.waiting_for(Purpose::Stats));

        // Answers come in any order.
        assert_eq!(
            requests.answered(second.id),
            Some(Purpose::Line(Command::Answer))
        );
        assert_eq!(requests.answered(first.id), Some(Purpose::Lines));
        // An answer comes once.
        assert_eq!(requests.answered(first.id), None);
        assert_eq!(requests.answered(999), None);
        assert!(requests.is_empty());
    }

    #[test]
    fn a_request_with_no_answer_in_ten_seconds_fails_with_a_message() {
        let start = Instant::now();
        let mut requests = Requests::new();
        let early = request_of(&requests.begin(&Call::StatsGet, Purpose::Stats, start));
        let late = request_of(&requests.begin(
            &Call::ConfigReload,
            Purpose::Reload,
            start + Duration::from_secs(4),
        ));

        assert!(requests.expire(start + Duration::from_secs(9)).is_empty());
        let expired = requests.expire(start + Duration::from_secs(10));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, Purpose::Stats);
        assert_eq!(expired[0].1.msgid, "RMD4001");
        assert_eq!(
            expired[0].1.text,
            "The service did not answer stats.get within 10 seconds."
        );
        // An answer that comes after all is not for anything any more.
        assert_eq!(requests.answered(early.id), None);

        assert!(requests.waiting_for(Purpose::Reload));
        let expired = requests.expire(start + Duration::from_secs(60));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, Purpose::Reload);
        assert!(expired[0].1.text.contains("config.reload"));
        assert_eq!(requests.answered(late.id), None);
    }

    #[test]
    fn a_lost_connection_takes_its_requests_with_it() {
        let now = Instant::now();
        let mut requests = Requests::new();
        let sent = request_of(&requests.begin(&Call::LinesList, Purpose::Lines, now));
        assert_eq!(requests.abandon(), vec![Purpose::Lines]);
        assert!(requests.is_empty());
        assert!(requests.abandon().is_empty());
        assert_eq!(requests.answered(sent.id), None);
        // The ids go on: an answer to an old request is never taken for
        // the answer to a new one.
        let next = request_of(&requests.begin(&Call::LinesList, Purpose::Lines, now));
        assert!(next.id > sent.id);
    }

    #[test]
    fn only_what_the_user_asked_for_is_reported_to_them() {
        assert!(Purpose::Line(Command::HangUp).asked_for());
        assert!(Purpose::Reload.asked_for());
        assert!(Purpose::Remove(3).asked_for());
        for purpose in [
            Purpose::Service,
            Purpose::Subscribe,
            Purpose::Lines,
            Purpose::Stats,
            Purpose::LogTail,
            Purpose::Calls,
            // What waits for this is told by its answer.
            Purpose::Waited(1),
        ] {
            assert!(!purpose.asked_for(), "{purpose:?}");
        }
    }

    #[test]
    fn a_result_of_the_wrong_shape_is_a_message() {
        let info: Result<ctlproto::ServiceInfo, _> = read(serde_json::json!({"lines": "many"}));
        assert_eq!(info.unwrap_err().msgid, "RMD2013");
        let mode: ServiceMode = read(serde_json::json!("demo")).unwrap();
        assert_eq!(mode, ServiceMode::Demo);
    }
}
