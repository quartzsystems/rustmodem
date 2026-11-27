// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The commands that read a call capture file. They ask nothing of the
//! service: a capture can be checked on any machine.
//!
//! `capture check <file>` says what happened in the call, and exits with 0
//! only when nothing was lost, nothing was resynchronised and nothing
//! slipped. `capture wav <file> <out.wav>` writes the call as sound.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};

use msg::{Message, message};
use rtp::capture::{CaptureError, Header, Reader, Report, check, write_wav};
use serde::Serialize;

use crate::output;

/// What `capture check --json` prints.
#[derive(Serialize)]
struct Checked<'a> {
    file: &'a str,
    line: u16,
    call_id: &'a str,
    codec: &'static str,
    ptime_ms: u8,
    started_ms: u64,
    received: u64,
    sent: u64,
    lost: u64,
    late: u64,
    duplicate: u64,
    resync: u64,
    ignored: u64,
    largest_gap_us: u64,
    slips: u64,
    free_run: u64,
    dropped_records: u64,
    clean: bool,
}

fn codec(payload_type: u8) -> &'static str {
    match payload_type {
        0 => "PCMU",
        8 => "PCMA",
        _ => "unknown",
    }
}

/// The `RMD` message for a capture that could not be read.
fn unreadable(path: &str, error: &CaptureError) -> Message {
    match error {
        CaptureError::Io(error) => message("RMD6003", &[path, &error.to_string()]),
        other => message("RMD6004", &[path, &other.to_string()]),
    }
}

fn open(path: &str) -> Result<Reader<BufReader<File>>, Message> {
    let file = File::open(path).map_err(|error| message("RMD6003", &[path, &error.to_string()]))?;
    Reader::new(BufReader::new(file)).map_err(|error| unreadable(path, &error))
}

/// The report of `capture check` as text.
pub fn report(path: &str, header: &Header, found: &Report) -> String {
    let verdict = if found.is_clean() {
        "clean: nothing lost, nothing resynchronised, nothing slipped".to_string()
    } else {
        let mut faults = Vec::new();
        for (count, what) in [
            (found.lost, "lost"),
            (found.resync, "resynchronised"),
            (found.slips, "slipped"),
            (found.dropped_records, "missing from the capture"),
        ] {
            if count > 0 {
                faults.push(format!("{count} {what}"));
            }
        }
        format!("NOT CLEAN: {}", faults.join(", "))
    };
    output::fields(&[
        ("Capture", path.to_string()),
        (
            "Call",
            format!("{} on line {}", header.call_id, header.line),
        ),
        (
            "Codec",
            format!(
                "{}, {} ms packets",
                codec(header.payload_type),
                header.ptime_ms
            ),
        ),
        (
            "Started",
            format!("{} UTC", output::timestamp(header.started_unix_ms)),
        ),
        ("Received", format!("{} packets", found.received)),
        ("Sent", format!("{} packets", found.sent)),
        ("Lost", found.lost.to_string()),
        ("Late", found.late.to_string()),
        ("Duplicate", found.duplicate.to_string()),
        ("Resync", found.resync.to_string()),
        ("Ignored", found.ignored.to_string()),
        (
            "Largest gap",
            format!(
                "{}.{} ms between arrivals",
                found.largest_gap_us / 1000,
                found.largest_gap_us % 1000 / 100
            ),
        ),
        (
            "Free run",
            format!("{} frames before the far end began", found.free_run),
        ),
        ("Slips", found.slips.to_string()),
        ("Dropped records", found.dropped_records.to_string()),
        ("Result", verdict),
    ])
}

/// `capture check <file>`: prints the report. `Ok(true)` when the call was
/// clean.
pub fn run_check(path: &str, json: bool, out: &mut dyn Write) -> Result<bool, Message> {
    let mut reader = open(path)?;
    let header = reader.header().clone();
    let found = check(&mut reader).map_err(|error| unreadable(path, &error))?;
    let text = if json {
        let checked = Checked {
            file: path,
            line: header.line,
            call_id: &header.call_id,
            codec: codec(header.payload_type),
            ptime_ms: header.ptime_ms,
            started_ms: header.started_unix_ms,
            received: found.received,
            sent: found.sent,
            lost: found.lost,
            late: found.late,
            duplicate: found.duplicate,
            resync: found.resync,
            ignored: found.ignored,
            largest_gap_us: found.largest_gap_us,
            slips: found.slips,
            free_run: found.free_run,
            dropped_records: found.dropped_records,
            clean: found.is_clean(),
        };
        format!("{}\n", ctlproto::encode(&checked))
    } else {
        report(path, &header, &found)
    };
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
    Ok(found.is_clean())
}

/// `capture wav <file> <out.wav>`: writes the call as a two-channel G.711
/// WAV file, received on the left and sent on the right.
pub fn run_wav(path: &str, wav: &str, out: &mut dyn Write) -> Result<(), Message> {
    let mut reader = open(path)?;
    let written = |error: std::io::Error| message("RMD6005", &[wav, &error.to_string()]);
    let mut file = BufWriter::new(File::create(wav).map_err(written)?);
    let frames = write_wav(&mut reader, &mut file).map_err(|error| match error {
        // An error in the middle is the reading's or the writing's; a
        // capture that is damaged is the reading's.
        CaptureError::Io(error) => written(error),
        other => unreadable(path, &other),
    })?;
    file.flush().map_err(written)?;
    let seconds = frames / 100;
    let text = format!(
        "Wrote {wav}: {seconds}.{:02} s, received on the left and sent on the right.\n",
        frames % 100
    );
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rtp::RtpHeader;
    use rtp::capture::{Kind, Record, Writer};

    use super::*;

    /// A file of the test's own in the temporary directory.
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustmodem-ctl-{}-{name}", std::process::id()))
    }

    fn packet(kind: Kind, n: u32, at_ms: u64) -> Record {
        let header = RtpHeader {
            marker: n == 0,
            payload_type: 8,
            sequence: n as u16,
            timestamp: 160 * n,
            ssrc: if kind == Kind::Received { 1 } else { 2 },
        };
        Record::new(kind, at_ms * 1_000_000, header, &[n as u8; 160])
    }

    /// A capture of five packets each way; the far end's packet `lost`
    /// never came.
    fn capture(name: &str, lost: Option<u32>) -> String {
        let path = scratch(name);
        let header = Header {
            payload_type: 8,
            ptime_ms: 20,
            line: 3,
            started_unix_ms: 1_800_000_000_000,
            call_id: "abc@example".to_owned(),
        };
        let mut writer = Writer::new(File::create(&path).unwrap(), &header).unwrap();
        let mut sent = 0;
        for n in 0..5 {
            if lost == Some(n) {
                continue;
            }
            writer
                .record(&packet(Kind::Received, n, 20 * u64::from(n)))
                .unwrap();
            // A packet goes for every one that comes, and one more for
            // each that was filled in.
            while sent <= n {
                writer
                    .record(&packet(Kind::Sent, sent, 20 * u64::from(n)))
                    .unwrap();
                sent += 1;
            }
        }
        writer.flush().unwrap();
        path.to_str().unwrap().to_owned()
    }

    #[test]
    fn a_clean_capture_is_reported_and_is_clean() {
        let path = capture("clean.rmcap", None);
        let mut out = Vec::new();
        assert_eq!(
            run_check(&path, false, &mut out).map_err(|m| m.id),
            Ok(true)
        );
        let text = String::from_utf8(out).unwrap();
        let text = text
            .split(" ")
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        for part in [
            "abc@example on line 3",
            "PCMA, 20 ms packets",
            "Received: 5 packets",
            "Ignored: 0\n",
            "20.0 ms between arrivals",
            "clean: nothing lost",
        ] {
            assert!(text.contains(part), "{part:?} is not in:\n{text}");
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_capture_with_a_lost_packet_is_not_clean() {
        let path = capture("lost.rmcap", Some(2));
        let mut out = Vec::new();
        assert_eq!(
            run_check(&path, false, &mut out).map_err(|m| m.id),
            Ok(false)
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("NOT CLEAN: 1 lost"), "{text}");

        let mut out = Vec::new();
        assert_eq!(
            run_check(&path, true, &mut out).map_err(|m| m.id),
            Ok(false)
        );
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(json["lost"], 1);
        assert_eq!(json["slips"], 0);
        assert_eq!(json["received"], 4);
        assert_eq!(json["sent"], 5);
        assert_eq!(json["codec"], "PCMA");
        assert_eq!(json["clean"], false);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_capture_becomes_a_wav_file() {
        let path = capture("sound.rmcap", Some(1));
        let wav = scratch("sound.wav");
        let mut out = Vec::new();
        run_wav(&path, wav.to_str().unwrap(), &mut out).unwrap();
        let written = std::fs::read(&wav).unwrap();
        assert_eq!(&written[..4], b"RIFF");
        assert_eq!(&written[8..12], b"WAVE");
        // Five packets of two frames, the lost one filled, on two channels.
        assert_eq!(written.len(), 58 + 10 * 80 * 2);
        let text = String::from_utf8(out).unwrap();
        assert!(text.ends_with(": 0.10 s, received on the left and sent on the right.\n"));
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn a_file_that_is_missing_or_is_no_capture_is_a_message() {
        let missing = scratch("missing.rmcap");
        let missing = missing.to_str().unwrap();
        let refused = run_check(missing, false, &mut Vec::new()).unwrap_err();
        assert_eq!(refused.id, "RMD6003");
        assert!(refused.text.contains(missing));

        let other = scratch("other.rmcap");
        std::fs::write(&other, b"RIFF, not a capture at all").unwrap();
        let other_name = other.to_str().unwrap();
        let refused = run_check(other_name, false, &mut Vec::new()).unwrap_err();
        assert_eq!(refused.id, "RMD6004");
        let refused = run_wav(other_name, "unused.wav", &mut Vec::new()).unwrap_err();
        assert_eq!(refused.id, "RMD6004");

        let capture = capture("nowhere.rmcap", None);
        let nowhere = scratch("no-such-directory").join("out.wav");
        let refused = run_wav(&capture, nowhere.to_str().unwrap(), &mut Vec::new()).unwrap_err();
        assert_eq!(refused.id, "RMD6005");
        let _ = std::fs::remove_file(other);
        let _ = std::fs::remove_file(capture);
    }
}
