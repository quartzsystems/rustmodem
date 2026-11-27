// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Lines with the modem engine: what the pool shows of a call's modem,
//! the diagnostics it publishes, and the loopback self-test.

mod common;

use common::{Harness, SharedProfiles, config_with_lines, noise, summarise};
use config::{EngineKind, LineModem, ModulationName};
use ctlproto::{Event, LINE_DIAG_PER_SECOND, LineState};
use pool::{InlineRunner, NullSwitch, SELFTEST_OCTETS, TICK_MS, engine_for};

/// Lines 1 to `count` with the modem engine, and one more with the null
/// engine, on the null switch with the engines run by the pool's own
/// tick. Every port is open. The modems are the 300 bit/s ones.
fn modems(count: u16) -> Harness {
    modems_with(count, vec![ModulationName::V21, ModulationName::Bell103])
}

/// The same with these modulations in `[modem]`.
fn modems_with(count: u16, modulations: Vec<ModulationName>) -> Harness {
    let mut config = config_with_lines(count + 1);
    config.modem.modulations = modulations;
    for line in &mut config.lines[..usize::from(count)] {
        line.engine = EngineKind::Modem;
    }
    let mut harness = Harness::with_parts(
        config,
        SharedProfiles::default(),
        Box::new(NullSwitch::new()),
        Box::new(InlineRunner::new()),
        Box::new(engine_for),
    );
    for id in 1..=count + 1 {
        harness.host(id).open();
    }
    harness.advance(TICK_MS);
    for id in 1..=count + 1 {
        harness.quiet(id);
    }
    harness.take_events();
    harness.take_logs();
    harness
}

/// Line 1 dials line 2, which answers, and the modems connect.
fn connect(h: &mut Harness) {
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), "\r\nRING\r\n");
    assert_eq!(h.command(2, "ATA"), "");
    // The answerer is silent for 2150 ms, sends its tone, and V.8 takes
    // its time: some seconds in all.
    h.advance(8000);
    assert_eq!(h.read(1), "\r\nCONNECT 300\r\n");
    assert_eq!(h.read(2), "\r\nCONNECT 300\r\n");
    assert_eq!(h.state(1), LineState::Online);
}

#[test]
fn a_line_shows_its_modem_settings_and_which_are_its_own() {
    let mut h = modems(2);
    let line = h.pool.line(1).unwrap();
    assert_eq!(line.engine, "modem");
    assert_eq!(line.modem.modulations, ["v21", "bell103"]);
    assert_eq!((line.modem.v8, line.modem.tx_level_dbm0), (true, -12));
    assert!(line.modem.own.is_empty());

    let mut config = h.config.clone();
    config.modem.tx_level_dbm0 = -15;
    config.lines[0].modem = LineModem {
        modulations: Some(vec![ModulationName::Bell103]),
        v8: Some(false),
        ..LineModem::default()
    };
    h.pool.apply_config(&config);
    let line = h.pool.line(1).unwrap();
    assert_eq!(line.modem.modulations, ["bell103"]);
    assert_eq!((line.modem.v8, line.modem.tx_level_dbm0), (false, -15));
    assert_eq!(line.modem.own, ["modulations", "v8"]);
    // The line with another engine has the settings all the same: they
    // are what it would use.
    assert_eq!(h.pool.line(3).unwrap().modem.tx_level_dbm0, -15);
}

#[test]
fn a_call_shows_its_modem_and_the_statistics_count_what_it_connected_with() {
    let mut h = modems(2);
    assert_eq!(h.pool.line_diag(1).unwrap().modem, None);
    connect(&mut h);

    for id in [1, 2] {
        let call = h.pool.line(id).unwrap().call.unwrap();
        let connect = call.connect.unwrap();
        assert_eq!(
            (connect.rate, connect.carrier.as_deref()),
            (300, Some("V.21"))
        );
        let modem = call.modem.expect("the call has a modem");
        assert_eq!(modem.state, "data");
        assert!(modem.carrier);
        // Sent at -12 dBm0 and received as it was sent, there being
        // nothing between the two.
        let (rx, tx) = (
            modem.rx_level_tenth_dbm0.unwrap(),
            modem.tx_level_tenth_dbm0.unwrap(),
        );
        assert!((-135..=-105).contains(&rx), "received at {rx}");
        assert!((-125..=-115).contains(&tx), "sent at {tx}");
        assert!(modem.frequency_error_mhz.abs() < 2000, "{modem:?}");
        assert!(modem.quality_percent > 50, "{modem:?}");
        assert_eq!((modem.framing_errors, modem.retrains), (0, 0));

        let diag = h.pool.line_diag(id).unwrap();
        assert_eq!(diag.line, id);
        assert_eq!(diag.modem, Some(modem));
        assert_eq!(diag.history.last().unwrap().state, "data");
        assert!(diag.history.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
        assert_eq!(diag.scope_kind, "discriminator");
        assert_eq!(diag.scope.len(), 64);
    }
    // The answerer's history is the answering side of the handshake.
    let states: Vec<String> = h
        .pool
        .line_diag(2)
        .unwrap()
        .history
        .into_iter()
        .map(|change| change.state)
        .collect();
    assert_eq!(
        states,
        [
            "answer-silence",
            "answer-tone",
            "v8-jm",
            "gap",
            "training",
            "data"
        ]
    );

    // Data crosses, and the statistics have the call by what it
    // connected with.
    h.host(1).write(b"hello");
    h.advance(1000);
    assert_eq!(h.read(2), "hello");
    let stats = h.pool.stats();
    assert_eq!(stats.connects.len(), 1);
    assert_eq!(
        (
            stats.connects[0].rate,
            stats.connects[0].carrier.as_deref(),
            stats.connects[0].count
        ),
        (300, Some("V.21"), 1)
    );

    // The call over, the line has no modem to show; the record of the
    // call kept its connection.
    h.host(1).set_dtr(false);
    h.advance(3000);
    assert_eq!(h.read(2), "\r\nNO CARRIER\r\n");
    assert_eq!(h.pool.line_diag(1).unwrap().modem, None);
    assert_eq!(h.pool.line_diag(2).unwrap().modem, None);
    assert!(h.pool.line_diag(9).is_err());
    let record = h.pool.calls(None).pop().unwrap();
    assert_eq!(record.connect.unwrap().carrier.as_deref(), Some("V.21"));
}

#[test]
fn the_diagnostics_of_a_watched_line_are_published_at_their_rate() {
    let mut h = modems(2);
    h.pool.watch_diagnostics(&[2]);
    h.advance(TICK_MS);
    // A line just watched is said at once, with nothing to show.
    assert_eq!(summarise(&h.take_events()), ["diag 2 none"]);
    h.advance(1000);
    assert!(h.take_events().is_empty());

    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), "\r\nRING\r\n");
    assert_eq!(h.command(2, "ATA"), "");
    h.take_events();
    h.advance(4000);
    let diags: Vec<_> = h
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            Event::LineDiag(diag) => Some(diag),
            _ => None,
        })
        .collect();
    // Four a second, for the line that is watched and no other.
    let most = 4 * LINE_DIAG_PER_SECOND as usize;
    assert!(
        (most - 2..=most).contains(&diags.len()),
        "{} events in four seconds",
        diags.len()
    );
    assert!(diags.iter().all(|diag| diag.line == 2));
    assert_eq!(
        diags[0].modem.as_ref().unwrap().state,
        "answer-silence",
        "{:?}",
        diags[0]
    );
    assert!(
        diags
            .iter()
            .any(|diag| diag.modem.as_ref().unwrap().state == "answer-tone")
    );

    // The call ends: said once, with no modem, and then nothing.
    h.host(2).set_dtr(false);
    h.advance(500);
    let events = summarise(&h.take_events());
    assert_eq!(
        events.iter().filter(|e| e.starts_with("diag")).count(),
        1,
        "{events:?}"
    );
    assert!(events.contains(&"diag 2 none".to_string()), "{events:?}");
    h.advance(2000);
    assert!(
        !summarise(&h.take_events())
            .iter()
            .any(|e| e.starts_with("diag"))
    );

    // Nobody watching: nothing is published.
    h.pool.watch_diagnostics(&[]);
    h.host(2).set_dtr(true);
    h.advance(100);
    h.take_events();
    connect_again(&mut h);
    assert!(
        !summarise(&h.take_events())
            .iter()
            .any(|e| e.starts_with("diag"))
    );
}

fn connect_again(h: &mut Harness) {
    // Whatever the hosts were sent when the last call ended is read.
    h.command(2, "ATE0");
    h.read(1);
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), "\r\nRING\r\n");
    assert_eq!(h.command(2, "ATA"), "");
    h.advance(2000);
}

#[test]
fn a_modem_that_gives_up_is_logged_with_the_state_it_reached() {
    let mut h = modems(1);
    // The null line calls the modem line: what the modem hears is no
    // modem, and it gives up when it has offered everything it has.
    assert_eq!(h.command(2, "ATD1"), "");
    assert_eq!(h.read(1), "\r\nRING\r\n");
    assert_eq!(h.command(1, "ATA"), "");
    h.take_logs();
    h.advance(20_000);
    assert_eq!(h.read(1), "\r\nNO CARRIER\r\n");
    let logs: Vec<String> = h.take_logs().into_iter().map(|log| log.text).collect();
    let said: Vec<&String> = logs
        .iter()
        .filter(|text| text.starts_with("RMD7001"))
        .collect();
    assert_eq!(
        said,
        ["RMD7001: The modem on line 1 did not connect: it gave up in the state fallback."],
        "{logs:?}"
    );
}

#[test]
fn a_self_test_holds_its_lines_busy_and_says_what_the_modems_did() {
    let mut h = modems(3);
    let refused = |h: &mut Harness, a, b| h.pool.selftest_begin(a, b).unwrap_err().id;
    assert_eq!(refused(&mut h, 1, 1), "RMD7004");
    assert_eq!(refused(&mut h, 1, 9), "RMD1001");
    // Line 4 has the null engine.
    assert_eq!(refused(&mut h, 1, 4), "RMD7003");
    h.host(2).set_dtr(false);
    h.advance(TICK_MS);
    let not_ready = h.pool.selftest_begin(1, 2).unwrap_err();
    assert_eq!(not_ready.id, "RMD7002");
    assert!(not_ready.text.ends_with("it is not_ready."), "{not_ready}");
    // A test that was refused holds nothing.
    assert_eq!(h.state(1), LineState::Idle);
    h.host(2).set_dtr(true);
    h.advance(TICK_MS);
    h.take_events();

    let mut job = h.pool.selftest_begin(1, 2).unwrap();
    assert_eq!((job.a, job.b, job.octets), (1, 2, SELFTEST_OCTETS));
    assert_eq!(
        summarise(&h.take_events()),
        ["line 1 handshaking", "line 2 handshaking"]
    );
    // Busy to everything: another test, a call from another line, and
    // the line's own host.
    assert_eq!(refused(&mut h, 2, 3), "RMD7002");
    assert_eq!(h.command(3, "ATD1"), "\r\nBUSY\r\n");
    assert_eq!(h.command(1, "ATD3"), "\r\nBUSY\r\n");
    assert_eq!(h.state(1), LineState::Handshaking);
    assert_eq!(h.read(3), "");

    // The test itself, short: the modems connect and the data is whole.
    job.octets = 300;
    let result = job.run();
    assert!(result.passed, "{result:?}");
    assert_eq!((result.a, result.b, &result.error), (1, 2, &None));
    let connect = result.connect.clone().unwrap();
    assert_eq!(
        (connect.rate, connect.carrier.as_deref()),
        (300, Some("V.21"))
    );
    let connect_ms = result.connect_ms.unwrap();
    assert!((4000..9000).contains(&connect_ms), "{connect_ms}");
    assert_eq!(
        (result.octets, result.octets_a_to_b, result.octets_b_to_a),
        (300, 300, 300)
    );
    // Three hundred characters are ten seconds of the call.
    assert!(result.call_ms > connect_ms + 10_000, "{result:?}");
    // It is the same every time.
    assert_eq!(job.run(), result);

    h.take_events();
    h.pool.selftest_end(1, 2);
    assert_eq!(summarise(&h.take_events()), ["line 1 idle", "line 2 idle"]);
    assert_eq!(h.command(3, "ATD1"), "");
    assert_eq!(h.read(1), "\r\nRING\r\n");
}

#[test]
fn a_self_test_of_modems_with_nothing_in_common_fails_and_says_why() {
    let mut h = modems(2);
    let mut config = h.config.clone();
    config.lines[0].modem.modulations = Some(vec![ModulationName::V21]);
    config.lines[1].modem = LineModem {
        modulations: Some(vec![ModulationName::Bell103]),
        v8: Some(false),
        ..LineModem::default()
    };
    h.pool.apply_config(&config);
    let mut job = h.pool.selftest_begin(1, 2).unwrap();
    job.octets = 100;
    let result = job.run();
    assert!(!result.passed);
    assert_eq!((result.connect, result.connect_ms), (None, None));
    assert_eq!((result.octets_a_to_b, result.octets_b_to_a), (0, 0));
    let error = result.error.unwrap();
    assert_eq!(error.msgid, "RMD7005");
    assert!(
        error
            .text
            .starts_with("The self-test of lines 1 and 2 failed: the modems did not connect."),
        "{error}"
    );
    // A line that was removed meanwhile is no matter to the end of it.
    h.pool.apply_config(&config_with_lines(1));
    h.pool.selftest_end(1, 2);
}

// The modems at 1200 and 2400 bit/s, and the data link.

/// Lines 1 and 2 with every modulation.
fn fast_modems() -> Harness {
    modems_with(2, ModulationName::ALL.to_vec())
}

/// Line 1 dials line 2, which answers; the time of the handshake and the
/// data link passes.
fn dial(h: &mut Harness) {
    assert_eq!(h.command(1, "ATD2"), "");
    assert_eq!(h.read(2), "\r\nRING\r\n");
    assert_eq!(h.command(2, "ATA"), "");
    h.advance(9000);
}

#[test]
fn two_lines_connect_at_2400_with_error_control_and_compression() {
    let mut h = fast_modems();
    // The originator's host asks for every report.
    assert_eq!(h.command(1, "AT+MR=1;+ER=1;+DR=1;+ILRR=1"), "\r\nOK\r\n");
    dial(&mut h);
    assert_eq!(
        h.read(1),
        "\r\n+MCR: V22B\r\n\r\n+MRR: 2400\r\n\r\n+ER: LAPM\r\n\r\n+DR: V42B\r\n\r\n+ILRR: 2400\r\n\
         \r\nCONNECT 2400\r\n"
    );
    assert_eq!(h.read(2), "\r\nCONNECT 2400\r\n");
    for id in [1, 2] {
        let call = h.pool.line(id).unwrap().call.unwrap();
        let connect = call.connect.unwrap();
        assert_eq!(
            (
                connect.rate,
                connect.carrier.as_deref(),
                connect.protocol.as_deref(),
                connect.compression.as_deref()
            ),
            (2400, Some("V.22bis"), Some("LAPM"), Some("V.42bis"))
        );
        let modem = call.modem.expect("the call has a modem");
        assert_eq!(modem.state, "data");
        assert!(modem.carrier);
        assert!(modem.quality_percent > 80, "{modem:?}");
        assert_eq!((modem.framing_errors, modem.retrains), (0, 0));
        // There is a constellation, and no scope.
        let diag = h.pool.line_diag(id).unwrap();
        assert_eq!(diag.scope_kind, "none");
        let states: Vec<&str> = diag.history.iter().map(|c| c.state.as_str()).collect();
        assert!(
            states.ends_with(&["training", "link", "data"]),
            "{states:?}"
        );
    }
    let stats = h.pool.stats();
    assert_eq!(stats.connects.len(), 1);
    assert_eq!(
        (stats.connects[0].rate, stats.connects[0].carrier.as_deref()),
        (2400, Some("V.22bis"))
    );
    assert_eq!(stats.protocols.len(), 1);
    assert_eq!(
        (stats.protocols[0].name.as_str(), stats.protocols[0].count),
        ("LAPM", 1)
    );
    assert_eq!(
        (
            stats.compressions[0].name.as_str(),
            stats.compressions[0].count
        ),
        ("V.42bis", 1)
    );

    // Data both ways, text that compresses and octets that do not.
    let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog. "
        .iter()
        .copied()
        .cycle()
        .take(3000)
        .collect();
    let octets = noise(7, 3000);
    h.host(1).write(&text);
    h.host(2).write(&octets);
    h.advance(20_000);
    assert!(h.host(2).read() == text, "what line 1's host sent");
    assert!(h.host(1).read() == octets, "what line 2's host sent");
    assert_eq!(h.state(1), LineState::Online);
}

#[test]
fn the_hosts_commands_and_the_lines_policy_decide_the_error_control() {
    // The answerer's host has error control off, as `\N0` has it: the
    // modems connect at 2400 bit/s with none.
    let mut h = fast_modems();
    assert_eq!(h.command(2, "AT\\N0"), "\r\nOK\r\n");
    assert_eq!(h.command(1, "AT+ER=1"), "\r\nOK\r\n");
    dial(&mut h);
    assert_eq!(h.read(1), "\r\n+ER: NONE\r\n\r\nCONNECT 2400\r\n");
    // The answerer's host is given the originator's detection pattern as
    // the characters it is, after its CONNECT.
    let heard = h.read(2);
    assert!(heard.starts_with("\r\nCONNECT 2400\r\n"), "{heard:?}");
    let call = h.pool.line(1).unwrap().call.unwrap().connect.unwrap();
    assert_eq!(
        (call.rate, call.protocol, call.compression),
        (2400, None, None)
    );
    assert!(h.pool.stats().protocols.is_empty());

    // The line's policy is the factory value of the host's +ES and +DS.
    let mut h = fast_modems();
    let mut config = h.config.clone();
    config.modem.error_control = config::ErrorControlName::Required;
    config.lines[1].modem.error_control = Some(config::ErrorControlName::Off);
    config.lines[1].modem.compression = Some(false);
    h.pool.apply_config(&config);
    h.advance(TICK_MS);
    // The hosts have set nothing of their own, so the lines' new policy
    // is theirs at once; and it is what &F returns to.
    assert_eq!(
        h.command(1, "AT+ES?;+DS?"),
        "\r\n+ES: 3,2,4\r\n\r\n+DS: 3,0,2048,32\r\n\r\nOK\r\n"
    );
    assert_eq!(
        h.command(2, "AT+ES?;+DS?"),
        "\r\n+ES: 1,0,1\r\n\r\n+DS: 0,0,2048,32\r\n\r\nOK\r\n"
    );
    assert_eq!(h.command(2, "AT\\N3%C3"), "\r\nOK\r\n");
    assert_eq!(
        h.command(2, "AT&FE0+ES?;+DS?"),
        "\r\n+ES: 1,0,1\r\n\r\n+DS: 0,0,2048,32\r\n\r\nOK\r\n"
    );
    // Error control is required at line 1 and line 2 has none: line 2
    // connects, having nothing to wait for, and line 1 clears the call.
    // Its host has NO CARRIER and no CONNECT, and the log says why.
    h.take_logs();
    dial(&mut h);
    h.advance(3000);
    assert_eq!(h.read(1), "\r\nNO CARRIER\r\n");
    let heard = h.read(2);
    assert!(heard.starts_with("\r\nCONNECT 2400\r\n"), "{heard:?}");
    assert!(heard.ends_with("\r\nNO CARRIER\r\n"), "{heard:?}");
    let logs: Vec<String> = h.take_logs().into_iter().map(|log| log.text).collect();
    assert!(
        logs.iter().any(|text| text
            == "RMD8001: The call on line 1 was cleared: error control is required and the far \
                end has none."),
        "{logs:?}"
    );
    assert_eq!(h.state(1), LineState::Idle);
}

#[test]
fn a_host_that_sends_faster_than_the_line_carries_is_held_off_with_cts() {
    let mut h = fast_modems();
    dial(&mut h);
    assert_eq!(h.read(1), "\r\nCONNECT 2400\r\n");
    assert!(h.host(1).signals().cts);
    h.read(2);
    // Twenty seconds of the line's data at once.
    let data = noise(3, 6000);
    h.host(1).write(&data);
    h.advance(200);
    assert!(
        !h.host(1).signals().cts,
        "CTS is off while the line is behind"
    );
    // Not all of it has been read from the port: the pool stops reading
    // too.
    let mut got = Vec::new();
    let mut released = false;
    for _ in 0..400 {
        h.advance(100);
        got.extend(h.host(2).read());
        released |= h.host(1).signals().cts;
        if got.len() == data.len() && h.host(1).signals().cts {
            break;
        }
    }
    assert!(released, "CTS came on again");
    assert!(
        got == data,
        "{} of {} octets arrived",
        got.len(),
        data.len()
    );

    // With &K0 the host is not held off, and nothing is lost for that:
    // the pool does not read what it has no room for.
    h.advance(1100);
    h.host(1).write(b"+++");
    h.advance(1200);
    assert_eq!(h.read(1), "\r\nOK\r\n");
    // The escape characters went to the far end as the data they are.
    h.advance(1000);
    assert_eq!(h.read(2), "+++");
    assert_eq!(h.command(1, "AT&K0O"), "\r\nCONNECT 2400\r\n");
    h.host(1).write(&data);
    h.advance(200);
    assert!(h.host(1).signals().cts);
    let mut got = Vec::new();
    for _ in 0..400 {
        h.advance(100);
        got.extend(h.host(2).read());
        if got.len() == data.len() {
            break;
        }
    }
    assert!(
        got == data,
        "{} of {} octets arrived",
        got.len(),
        data.len()
    );
}
