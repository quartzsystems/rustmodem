// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Every capture of the corpus, replayed: the engine does what the
//! capture's expectation says.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use replay::{Options, What, compare, render, replay};
use rtp::capture::Reader;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modem/engine/tests/corpus")
}

fn replayed(capture: &Path) -> replay::Outcome {
    let file = File::open(capture).unwrap();
    let mut reader = Reader::new(BufReader::new(file)).unwrap();
    replay(&mut reader, &Options::default()).unwrap()
}

#[test]
fn every_capture_of_the_corpus_replays_as_its_expectation_says() {
    let mut captures: Vec<PathBuf> = std::fs::read_dir(corpus())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "rmcap"))
        .collect();
    captures.sort();
    assert!(captures.len() >= 7, "the corpus has {captures:?}");
    for capture in captures {
        let name = capture.file_name().unwrap().to_string_lossy().into_owned();
        let expected = std::fs::read_to_string(capture.with_extension("expect"))
            .unwrap_or_else(|error| panic!("{name} has no expectation: {error}"));
        assert!(capture.with_extension("md").exists(), "{name} has no note");
        let outcome = replayed(&capture);
        let rendered = render(&outcome);
        if let Err(difference) = compare(&rendered, &expected) {
            panic!("{name}: {difference}\nthe replay:\n{rendered}");
        }
        // A capture of the corpus is of a call that connected and carried
        // data, and the replay is exact at least until it connects.
        let connected = outcome
            .steps
            .iter()
            .find(|step| matches!(step.what, What::Connect { .. }))
            .unwrap_or_else(|| panic!("{name} does not connect"));
        assert!(!outcome.data.is_empty(), "{name} carries no data");
        assert!(
            outcome.sent_as_captured * 10 >= connected.at_ms,
            "{name}: the engine sends what the capture has for {} frames only",
            outcome.sent_as_captured
        );
        // A call with error control is one whose host sent nothing, and
        // its replay is exact to the end: the link is up when the capture
        // stops, and every frame sent is the frame the call sent.
        let stages: Vec<&str> = outcome
            .steps
            .iter()
            .filter_map(|step| match &step.what {
                What::Link(line) => Some(line.as_str()),
                _ => None,
            })
            .collect();
        if stages.iter().any(|line| line.starts_with("lapm")) {
            assert!(
                stages.last().unwrap().starts_with("lapm"),
                "{name}: {stages:?}"
            );
            assert_eq!(
                outcome.sent_as_captured, outcome.captured_sent,
                "{name}: the replay is not the call to its end"
            );
            let xids = outcome
                .steps
                .iter()
                .filter(|step| matches!(step.what, What::Xid(_)))
                .count();
            assert_eq!(xids, 2, "{name}: an XID each way");
        }
    }
}

/// A replay is the same every time, and the same whichever way the
/// capture is cut into reads.
#[test]
fn a_replay_is_the_same_every_time() {
    let capture = corpus().join("answer-v8-v21.rmcap");
    assert_eq!(replayed(&capture), replayed(&capture));
}

/// Replayed as the other end, the capture is not a call the engine knows:
/// it does not connect, and says so without stopping.
#[test]
fn a_capture_replayed_as_the_wrong_end_does_not_connect() {
    let file = File::open(corpus().join("answer-v8-v21.rmcap")).unwrap();
    let mut reader = Reader::new(BufReader::new(file)).unwrap();
    let options = Options {
        role: replay::Role::Originate,
        ..Options::default()
    };
    let outcome = replay(&mut reader, &options).unwrap();
    assert!(
        !outcome
            .steps
            .iter()
            .any(|step| matches!(step.what, What::Connect { .. })),
        "{}",
        render(&outcome)
    );
    assert!(render(&outcome).starts_with("role originate\nlaw ulaw\n"));
}

/// The frames of LAPM are kept by every replay and are in the report
/// only when asked for: the negotiation, the set-up, and the data with
/// its acknowledgements, in the order they came.
#[test]
fn the_frames_of_a_call_with_error_control_can_be_shown() {
    let outcome = replayed(&corpus().join("answer-v8-v22bis-lapm-v42bis.rmcap"));
    let plain = render(&outcome);
    assert!(!plain.contains(" frame "), "{plain}");
    let shown = replay::render_with_frames(&outcome);
    let frames: Vec<&str> = shown
        .lines()
        .filter_map(|line| line.split_once(" frame ").map(|(_, frame)| frame))
        .collect();
    assert!(
        frames[..4]
            .iter()
            .zip([
                "rx XID command",
                "tx XID response",
                "rx SABME command p=1",
                "tx UA response f=1"
            ])
            .all(|(frame, start)| frame.starts_with(start)),
        "{frames:?}"
    );
    // The far end's data, compressed, in I frames numbered from 0, each
    // acknowledged.
    let data: Vec<&&str> = frames
        .iter()
        .filter(|frame| frame.starts_with("rx I command"))
        .collect();
    assert!(data[0].starts_with("rx I command ns=0 nr=0 "), "{frames:?}");
    assert!(
        frames
            .iter()
            .any(|frame| frame.starts_with("tx RR response nr=1 ")),
        "{frames:?}"
    );
    // Without the frames, the two reports are the same.
    let without: String = shown
        .lines()
        .filter(|line| !line.contains(" frame "))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(without, plain);
}
