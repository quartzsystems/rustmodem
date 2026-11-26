// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Runs every transcript in `tests/transcripts/`. The format is described
//! in `common`.

mod common;

use std::path::Path;

#[test]
fn every_transcript_passes() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/transcripts");
    let mut paths: Vec<_> = std::fs::read_dir(&directory)
        .expect("the transcripts directory")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "txt"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 30,
        "only {} transcripts; the handoff asks for at least 30",
        paths.len()
    );

    let mut failures = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).expect("a readable transcript");
        if let Err(problem) = common::try_run(&text) {
            failures.push(format!("{name}: {problem}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} transcripts failed:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}

#[test]
fn the_runner_notices_a_wrong_transcript() {
    // Unclaimed output, a wrong expectation and a wrong action all fail.
    assert!(common::try_run("> AT\\r\n").is_err());
    assert!(common::try_run("> AT\\r\n< AT\\r\\r\\nERROR\\r\\n\n").is_err());
    assert!(common::try_run("> ATD5\\r\n< ATD5\\r\n! answer\n").is_err());
    assert!(common::try_run("! mode online\n").is_err());
    assert!(common::try_run("? what\n").is_err());
    assert!(common::try_run("> AT\\r\n< AT\\r\\r\\nOK\\r\\n\n").is_ok());
}
