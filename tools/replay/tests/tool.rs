// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The program: what it prints, what it writes and how it exits; and the
//! comparison with an expectation.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use replay::{Difference, compare, write_wav};

fn capture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../modem/engine/tests/corpus/answer-v8-v21.rmcap")
}

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustmodem-replay"))
        .args(arguments)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// A directory of the test's own, made afresh.
fn scratch(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("rustmodem-replay-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn an_expectation_is_compared_line_by_line_and_comments_do_not_count() {
    let rendered = "role answer\nframes 3\n10 state data\n";
    assert_eq!(compare(rendered, rendered), Ok(()));
    assert_eq!(
        compare(
            rendered,
            "# made by hand\n\nrole answer\n  frames 3\n# then\n10 state data\n"
        ),
        Ok(())
    );
    assert_eq!(
        compare(rendered, "role answer\nframes 4\n10 state data\n"),
        Err(Difference {
            line: 2,
            expected: Some("frames 4".to_string()),
            found: Some("frames 3".to_string()),
        })
    );
    // The file expects more than the replay did, and less.
    assert_eq!(
        compare(
            rendered,
            "role answer\nframes 3\n10 state data\n20 carrier-lost\n"
        ),
        Err(Difference {
            line: 4,
            expected: Some("20 carrier-lost".to_string()),
            found: None,
        })
    );
    let short = compare(rendered, "role answer\n# no more\n").unwrap_err();
    assert_eq!(
        (short.line, short.expected.clone(), short.found.as_deref()),
        (2, None, Some("frames 3"))
    );
    assert_eq!(
        short.to_string(),
        "line 2: expected `nothing more`, and the replay has `frames 3`"
    );
}

#[test]
fn g711_is_written_as_a_wav_file_of_one_channel() {
    let mut wav = Vec::new();
    write_wav(&mut wav, &[0xFF, 0x7F, 0x00], false).unwrap();
    // The header, three octets and one of padding.
    assert_eq!(wav.len(), 58 + 3 + 1);
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
        wav.len() - 8
    );
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    // The u-law is format 7, with one channel at 8000 samples a second.
    assert_eq!(&wav[20..28], [7, 0, 1, 0, 0x40, 0x1F, 0, 0]);
    assert_eq!(&wav[50..54], b"data");
    assert_eq!(&wav[54..61], [3, 0, 0, 0, 0xFF, 0x7F, 0x00]);
    let mut alaw = Vec::new();
    write_wav(&mut alaw, &[0xD5; 80], true).unwrap();
    assert_eq!(alaw[20], 6);
    assert_eq!(alaw.len(), 58 + 80);
}

#[test]
fn the_program_prints_the_replay_and_checks_it_against_an_expectation() {
    let capture = capture();
    let capture = capture.to_str().unwrap();
    let plain = run(&[capture]);
    assert_eq!(plain.status.code(), Some(0), "{}", text(&plain.stderr));
    let printed = text(&plain.stdout);
    assert!(
        printed.starts_with("role answer\nlaw ulaw\nframes "),
        "{printed}"
    );
    assert!(printed.contains(" connect 300 V.21\n"), "{printed}");

    let directory = scratch("expect");
    let expectation = directory.join("call.expect");
    let wav = directory.join("sent.wav");
    std::fs::write(&expectation, format!("# as it was printed\n{printed}")).unwrap();
    let checked = run(&[
        capture,
        "--answer",
        "--expect",
        expectation.to_str().unwrap(),
        "--tx-out",
        wav.to_str().unwrap(),
    ]);
    assert_eq!(checked.status.code(), Some(0), "{}", text(&checked.stderr));
    assert_eq!(text(&checked.stdout), printed);
    // A frame is 80 octets, and one was sent for each that was given.
    let frames: usize = printed
        .lines()
        .find_map(|line| line.strip_prefix("frames "))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        std::fs::metadata(&wav).unwrap().len() as usize,
        58 + frames * 80
    );

    // The same capture as a modem with no V.8 does something else.
    let differs = run(&[
        capture,
        "--no-v8",
        "--expect",
        expectation.to_str().unwrap(),
    ]);
    assert_eq!(differs.status.code(), Some(1));
    let said = text(&differs.stderr);
    assert!(said.contains("call.expect: line "), "{said}");
    assert!(said.contains("expected `"), "{said}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn what_cannot_be_done_is_said_and_the_exit_code_says_which() {
    let help = run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    let usage = text(&help.stdout);
    assert!(usage.starts_with("Usage:\n  rustmodem-replay <file.rmcap> [options]"));
    // The limit of a replay is in the help.
    assert!(
        usage.contains("A replay is exact only until the engine's transmission changes."),
        "{usage}"
    );
    let version = run(&["--version"]);
    assert_eq!(
        text(&version.stdout),
        format!("rustmodem-replay {}\n", env!("CARGO_PKG_VERSION"))
    );

    for arguments in [
        &[][..],
        &["--answer"],
        &["a.rmcap", "b.rmcap"],
        &["a.rmcap", "--law", "pcm"],
        &["a.rmcap", "--law"],
        &["a.rmcap", "--modulations", "v32"],
        &["a.rmcap", "--modulations", "v21,v21"],
        &["a.rmcap", "--error-control", "some"],
        &["a.rmcap", "--error-control"],
        &["a.rmcap", "--fast"],
    ] {
        let refused = run(arguments);
        assert_eq!(refused.status.code(), Some(2), "{arguments:?}");
        assert!(text(&refused.stderr).starts_with("Usage:"), "{arguments:?}");
    }

    let directory = scratch("refused");
    let missing = directory.join("none.rmcap");
    let gone = run(&[missing.to_str().unwrap()]);
    assert_eq!(gone.status.code(), Some(1));
    assert!(text(&gone.stderr).contains("none.rmcap: "));
    let other = directory.join("other.rmcap");
    std::fs::write(&other, b"this is not a capture").unwrap();
    let not_one = run(&[other.to_str().unwrap()]);
    assert_eq!(not_one.status.code(), Some(1));
    assert!(text(&not_one.stderr).contains("other.rmcap: "));
    // An expectation that is not there fails the run too.
    let capture = capture();
    let no_file = run(&[
        capture.to_str().unwrap(),
        "--law",
        "ulaw",
        "--modulations",
        "v21,bell103",
        "--expect",
        directory.join("no.expect").to_str().unwrap(),
    ]);
    assert_eq!(no_file.status.code(), Some(1));
    assert!(text(&no_file.stderr).contains("no.expect: "));
    let _ = std::fs::remove_dir_all(&directory);
}

/// A call with error control: the program says what the link did, shows
/// its frames when asked, and compares an expectation without them. Told
/// that the modem had no error control, it replays another call.
#[test]
fn the_program_shows_the_data_link_and_its_frames() {
    let capture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../modem/engine/tests/corpus/answer-v8-v22bis-lapm-v42bis.rmcap");
    let expectation = capture.with_extension("expect");
    let (capture, expectation) = (capture.to_str().unwrap(), expectation.to_str().unwrap());
    let plain = run(&[capture, "--expect", expectation]);
    assert_eq!(plain.status.code(), Some(0), "{}", text(&plain.stderr));
    let printed = text(&plain.stdout);
    assert!(
        printed.contains(" link lapm window 15 compression both\n"),
        "{printed}"
    );
    assert!(
        printed.contains(" connect 2400 V.22bis LAPM V.42bis\n"),
        "{printed}"
    );
    assert!(printed.contains(" xid rx command n401 128/128 k 15/15 compression both "));
    assert!(!printed.contains(" frame "), "{printed}");

    let frames = run(&[capture, "--frames", "--expect", expectation]);
    assert_eq!(frames.status.code(), Some(0), "{}", text(&frames.stderr));
    let shown = text(&frames.stdout);
    assert!(shown.contains(" frame rx SABME command p=1\n"), "{shown}");
    assert!(shown.contains(" frame tx UA response f=1\n"), "{shown}");
    assert!(shown.len() > printed.len());

    let without = run(&[capture, "--no-compression", "--expect", expectation]);
    assert_eq!(without.status.code(), Some(1));
    assert!(
        text(&without.stdout).contains(" compression none\n"),
        "{}",
        text(&without.stdout)
    );
    let off = run(&[capture, "--error-control", "off"]);
    assert_eq!(off.status.code(), Some(0));
    let said = text(&off.stdout);
    assert!(said.contains(" link async\n"), "{said}");
    assert!(!said.contains(" link lapm "), "{said}");
}
