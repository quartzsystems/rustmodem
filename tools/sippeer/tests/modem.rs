// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The modem as a script's media: the steps that ask for it, and two
//! scripts whose modems call each other.

use engine::Standard;
use sippeer::MediaKind;
use sippeer::script::{Step, parse, run};

/// A loopback port for the test, in a stretch the other test programs do
/// not use.
fn free_port() -> u16 {
    44_900 + (std::process::id() % 100) as u16
}

#[test]
fn the_steps_that_ask_for_the_modem_are_read() {
    let both = MediaKind::Modem {
        modulations: [Some(Standard::V21), Some(Standard::Bell103)],
        v8: true,
    };
    let bell_only = MediaKind::Modem {
        modulations: [Some(Standard::Bell103), None],
        v8: false,
    };
    let steps = parse(
        "media modem
media modem bell103 no-v8
call sip:a@10.0.0.1 10.0.0.1:5060 as modem originate v21
call sip:a@10.0.0.1 10.0.0.1:5060 as modem originate bell103 v21 no-v8
call sip:a@10.0.0.1 10.0.0.1:5060
answer as modem answer
answer
expect connect
expect connect 12.5
",
    )
    .unwrap();
    assert!(matches!(&steps[0].1, Step::Media(media) if *media == both));
    assert!(matches!(&steps[1].1, Step::Media(media) if *media == bell_only));
    assert!(matches!(
        &steps[2].1,
        Step::Call { media: Some(MediaKind::Modem { modulations, v8: true }), .. }
            if *modulations == [Some(Standard::V21), None]
    ));
    assert!(matches!(
        &steps[3].1,
        Step::Call { media: Some(MediaKind::Modem { modulations, v8: false }), .. }
            if *modulations == [Some(Standard::Bell103), Some(Standard::V21)]
    ));
    assert!(matches!(&steps[4].1, Step::Call { media: None, .. }));
    assert!(matches!(&steps[5].1, Step::Answer(Some(media)) if *media == both));
    assert!(matches!(&steps[6].1, Step::Answer(None)));
    assert!(matches!(&steps[7].1, Step::ExpectConnect(time) if time.as_secs() == 30));
    assert!(matches!(&steps[8].1, Step::ExpectConnect(time) if time.as_millis() == 12_500));

    for refused in [
        "media modem v22",
        "media modem v21 v21",
        "media modem v21 bell103 v21",
        "call sip:a@10.0.0.1 10.0.0.1:5060 as modem",
        "call sip:a@10.0.0.1 10.0.0.1:5060 as modem answer v21",
        "call sip:a@10.0.0.1 10.0.0.1:5060 as null",
        "answer as modem originate",
        "expect connect 1 2",
    ] {
        assert_eq!(parse(refused).unwrap_err().line, 1, "{refused}");
    }
}

/// Two scripts: one answers as a modem, the other calls as one. V.8
/// chooses V.21, they connect, and data goes both ways.
#[test]
fn two_scripts_call_each_other_as_modems_and_carry_data() {
    let port = free_port();
    let callee = format!(
        "user bob
listen 127.0.0.1:{port}
expect incoming
answer as modem answer
expect connect
receive hello 15
send world\\r\\n
expect hangup 20
report
"
    );
    let caller = format!(
        "user alice
listen 127.0.0.1:0
call sip:bob@127.0.0.1:{port} 127.0.0.1:{port} as modem originate v21
expect answer
expect connect
send hello
receive world\\r\\n 15
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
    let mut out = Vec::new();
    let called = run(&caller, &mut out);
    let said_by_caller = String::from_utf8(out).unwrap();
    let (answered, said) = answering.join().unwrap();
    assert_eq!(called, Ok(()), "{said_by_caller}");
    assert_eq!(answered, Ok(()), "{said}");
    for said in [&said, &said_by_caller] {
        assert!(
            said.contains("connected at 300 bit/s with V.21\n"),
            "{said}"
        );
    }
}

/// A script that waits for a modem where there is none says so.
#[test]
fn a_call_with_no_modem_does_not_connect() {
    let stopped = run("listen 127.0.0.1:0\nexpect connect 0.05", &mut Vec::new()).unwrap_err();
    assert_eq!(
        (stopped.line, stopped.reason.as_str()),
        (2, "the modem did not connect in time")
    );
}
