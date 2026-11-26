// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The script format: read, refused, and run, one peer against another.

use std::sync::atomic::{AtomicU16, Ordering};

use sippeer::script::{ScriptError, Step, parse, run};

/// A loopback port for one test: each takes its own, in a stretch the
/// other test programs do not use, because a port that was free a moment
/// ago is given to the next test that asks as well.
fn free_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    44_000 + (std::process::id() % 100) as u16 * 40 + NEXT.fetch_add(1, Ordering::Relaxed)
}

#[test]
fn every_step_is_read() {
    let script = r"
# A script with one of everything.
user alice
codec pcma pcmu
ptime 10
seed 42
media silence
media pattern
media null
media file C:\sounds\a tone.ulaw
impair loss=10 duplicate=5 reorder=5 jitter=3 ppm=-50
listen 127.0.0.1:5070
registrar accept
registrar accept 120
registrar challenge example.org alice secret
registrar reject 403
registrar silent
call sip:5551000@10.0.0.1 10.0.0.1:5060
ring
answer
reject 486
cancel
hangup
expect incoming
expect ringing 2.5
expect answer
expect refused
expect cancelled
expect ack
expect hangup
expect register 30
talk 0.5
send ATDT\r\n\x00\\
receive CONNECT 5
check echo
check clean
report
";
    let steps = parse(script).unwrap();
    assert_eq!(steps.len(), 35);
    assert_eq!(steps[0].0, 3, "lines are numbered from 1");
    assert!(matches!(&steps[7].1, Step::MediaFile(path) if path == r"C:\sounds\a tone.ulaw"));
    assert!(matches!(
        &steps[8].1,
        Step::Impair(faults) if faults.loss == 10 && faults.jitter_ms == 3 && faults.clock_ppm == -50
    ));
    assert!(matches!(&steps[30].1, Step::Send(data) if data == b"ATDT\r\n\x00\\"));
    assert!(matches!(
        &steps[31].1,
        Step::Receive { data, within } if data == b"CONNECT" && within.as_secs() == 5
    ));
}

#[test]
fn a_line_that_is_not_a_step_is_refused_with_its_number() {
    for (script, line) in [
        ("user", 1),
        ("\n\ndance", 3),
        ("ptime 25", 1),
        ("codec g729", 1),
        ("# fine\nimpair loss=1001", 2),
        ("impair wobble=3", 1),
        ("listen 127.0.0.1", 1),
        ("listen [::1]:5060", 1),
        ("call sip:a@b", 1),
        ("expect applause", 1),
        ("expect answer soon", 1),
        ("talk -1", 1),
        ("send a\\q", 1),
        ("send \\x4", 1),
        ("reject", 1),
        ("check", 1),
        ("media file", 1),
    ] {
        let refused = parse(script).unwrap_err();
        assert_eq!(refused.line, line, "{script:?}: {refused}");
    }
}

#[test]
fn a_step_out_of_place_or_an_expectation_not_met_stops_the_script() {
    let stopped = |script: &str| run(script, &mut Vec::new()).unwrap_err();
    assert_eq!(
        stopped("ring"),
        ScriptError {
            line: 1,
            reason: "listen comes first".to_owned()
        }
    );
    assert_eq!(stopped("listen 127.0.0.1:0\nuser late").line, 2);
    assert_eq!(
        stopped("listen 127.0.0.1:0\n\nexpect incoming 0.05").line,
        3
    );
    assert_eq!(stopped("listen 127.0.0.1:0\nanswer").line, 2);
    assert_eq!(stopped("listen 127.0.0.1:0\ncancel").line, 2);
    assert_eq!(stopped("listen 127.0.0.1:0\nreport").line, 2);
    assert_eq!(stopped("media file Z:\\no\\such\\file").line, 1);
    // A script that runs out has not stopped.
    assert_eq!(run("listen 127.0.0.1:0", &mut Vec::new()), Ok(()));
}

#[test]
fn two_scripts_call_each_other_and_carry_data() {
    let port = free_port();
    let callee = format!(
        "user bob
media null
listen 127.0.0.1:{port}
expect incoming
ring
answer
expect ack
receive hello
send world\\r\\n
expect hangup
check clean
report
"
    );
    let caller = format!(
        "user alice
media null
ptime 10
listen 127.0.0.1:0
call sip:bob@127.0.0.1:{port} 127.0.0.1:{port}
expect ringing
expect answer
send hello
receive world\\r\\n
talk 0.3
check clean
hangup
"
    );
    let answering = std::thread::spawn(move || {
        let mut out = Vec::new();
        let result = run(&callee, &mut out);
        (result, String::from_utf8(out).unwrap())
    });
    // The callee is bound before the INVITE goes; and if it is not, the
    // INVITE goes again.
    let mut out = Vec::new();
    let called = run(&caller, &mut out);
    let (answered, said) = answering.join().unwrap();
    assert_eq!(called, Ok(()));
    assert_eq!(answered, Ok(()), "{said}");
    assert!(said.starts_with(&format!("SIP on 127.0.0.1:{port}, RTP on 127.0.0.1:")));
    assert!(
        said.contains("(lost 0, late 0, duplicate 0, resync 0, ignored 0"),
        "{said}"
    );
}

#[test]
fn a_call_that_is_refused_fails_the_expectation_of_an_answer() {
    let port = free_port();
    let callee = format!(
        "listen 127.0.0.1:{port}
expect incoming
reject 486
talk 0.3
"
    );
    let caller = format!(
        "listen 127.0.0.1:0
call sip:bob@127.0.0.1:{port} 127.0.0.1:{port}
expect answer
"
    );
    let answering = std::thread::spawn(move || run(&callee, &mut Vec::new()));
    let called = run(&caller, &mut Vec::new());
    assert_eq!(answering.join().unwrap(), Ok(()));
    assert_eq!(
        called,
        Err(ScriptError {
            line: 3,
            reason: "the call was refused with 486".to_owned()
        })
    );
}
