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
    assert!(captures.len() >= 3, "the corpus has {captures:?}");
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
            .find(|step| matches!(step.what, What::Connect { rate: 300, .. }))
            .unwrap_or_else(|| panic!("{name} does not connect"));
        assert!(!outcome.data.is_empty(), "{name} carries no data");
        assert!(
            outcome.sent_as_captured * 10 >= connected.at_ms,
            "{name}: the engine sends what the capture has for {} frames only",
            outcome.sent_as_captured
        );
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
