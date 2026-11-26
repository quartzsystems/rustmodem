// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The model as a whole: the connection, which commands can be chosen in
//! every state, the statistics, the event log and the call history.

mod common;

use common::{
    START_MS, broken_line, call, connected, error, line, line_with_call, log, record, service,
};
use ctlproto::{
    CallEvent, ConnectTally, Direction, EndReason, Event, LineState, LogLevel, ServiceMode, Stats,
    Tally,
};
use model::{
    Availability, CALL_COLUMNS, CALL_LABELS, CALL_LIMIT, CARRIER_LABELS, CallHistory, Changes,
    Command, ConnectionState, EMPTY, Filter, LOG_LIMIT, LogBuffer, LogClass, Model, RowChange,
    StatsView, Utc, filter_of,
};

/// A model connected to a service with these lines, the first selected.
fn connected_model(lines: Vec<ctlproto::LineInfo>) -> Model {
    let mut model = Model::new();
    model.connecting();
    model.connected_to(service(ServiceMode::Service, lines.len() as u32, 60));
    model.lines_listed(lines);
    model.lines.select_row(Some(0));
    model
}

// --------------------------------------------------------------------------
// The connection
// --------------------------------------------------------------------------

#[test]
fn the_connection_goes_from_disconnected_through_connecting_to_connected() {
    let mut model = Model::new();
    assert_eq!(*model.connection(), ConnectionState::Disconnected(None));
    assert!(!model.connected() && model.service().is_none());
    assert_eq!(model.status_message(), "Not connected.");
    assert_eq!(model.title(), "RustModem Manager");

    model.connecting();
    assert_eq!(*model.connection(), ConnectionState::Connecting);
    assert_eq!(model.status_message(), "Connecting to the service…");
    assert_eq!(model.status_parts(START_MS, &Utc)[0], "Service: connecting");

    model.connected_to(service(ServiceMode::Service, 2, 0));
    assert!(model.connected());
    assert_eq!(model.status_message(), "Ready");
    model.lines_listed(vec![line(1, LineState::Idle), line(2, LineState::Idle)]);
    // The title is the name of the program, whatever it is connected to.
    assert_eq!(model.title(), "RustModem Manager");

    // The service goes away: the reason is the RMD message, and what the
    // grid showed stays until the service is back.
    let reason = error("RMD2009", &["the service closed the connection"]);
    model.disconnected(Some(reason.clone()));
    assert_eq!(
        *model.connection(),
        ConnectionState::Disconnected(Some(reason))
    );
    assert_eq!(
        model.status_message(),
        "RMD2009: The connection to the service was lost: the service closed the connection"
    );
    assert_eq!(model.lines.len(), 2);
    assert!(model.stats().is_none());
    assert_eq!(model.title(), "RustModem Manager");

    // The user disconnects: no reason.
    model.disconnected(None);
    assert_eq!(model.status_message(), "Not connected.");
}

#[test]
fn the_log_s_title_says_where_the_log_is_kept() {
    let mut model = Model::new();
    model.connected_to(service(ServiceMode::Demo, 1, 0));
    model.lines_listed(vec![line(1, LineState::Idle)]);
    assert_eq!(model.title(), "RustModem Manager");
    assert_eq!(model.log_title(), "Event log \u{2014} demo service");
    let mut console = service(ServiceMode::Console, 0, 0);
    console.config_path = "relative/dir/other.toml".to_string();
    model.service_info(console);
    model.lines_listed(Vec::new());
    assert_eq!(model.title(), "RustModem Manager");
    assert_eq!(model.log_title(), "Event log \u{2014} rustmodem.log");
    // An answer to service.info that arrives after the connection has
    // gone is not a connection.
    model.disconnected(None);
    model.service_info(service(ServiceMode::Service, 3, 0));
    assert!(!model.connected());
}

#[test]
fn events_keep_the_model_current_and_say_what_they_changed() {
    let mut model = connected_model(vec![line(1, LineState::Idle), line(2, LineState::Idle)]);
    let ringing = line_with_call(2, LineState::Ringing, call(Direction::Incoming, "1", 5));
    assert_eq!(
        model.event(Event::LineState(Box::new(ringing.clone()))),
        Changes {
            rows: RowChange::Changed(1),
            log: false,
            calls: false,
            trace: false,
        }
    );
    // The call events themselves change nothing the window shows: the
    // line's own event carries the call.
    let started = Event::CallStarted(CallEvent {
        line: 2,
        call: ringing.call.clone().unwrap(),
    });
    assert_eq!(model.event(started), Changes::NONE);
    assert_eq!(
        model.event(Event::CallEnded(record(7, 2, EndReason::Abandoned))),
        Changes {
            rows: RowChange::None,
            log: false,
            calls: true,
            trace: false,
        }
    );
    assert_eq!(model.calls.len(), 1);
    assert_eq!(
        model.event(Event::Log(log(6, LogLevel::Info, "Line02", "a line"))),
        Changes {
            rows: RowChange::None,
            log: true,
            calls: false,
            trace: false,
        }
    );
    assert_eq!(
        model
            .event(Event::LineState(Box::new(line(3, LineState::Idle))))
            .rows,
        RowChange::Added(2)
    );

    // Answers to the manager's own requests are taken in the same way.
    assert_eq!(
        model.line_updated(line(1, LineState::Disabled)),
        RowChange::Changed(0)
    );
    assert_eq!(model.line_removed(3), RowChange::Removed(2));

    // After the connection has gone, what is still on its way is dropped.
    model.disconnected(None);
    let late = Event::LineState(Box::new(line(1, LineState::Online)));
    assert_eq!(model.event(late), Changes::NONE);
    assert_eq!(model.lines.line(1).unwrap().state, LineState::Disabled);
}

#[test]
fn what_events_said_before_the_first_list_arrived_is_not_lost() {
    let mut model = Model::new();
    model.connected_to(service(ServiceMode::Service, 2, 0));
    // The manager has subscribed and asked for the lines. Before the
    // answer comes, line 2 starts ringing and says so in an event. The
    // answer was made before that, and shows line 2 idle.
    let ringing = line_with_call(2, LineState::Ringing, call(Direction::Incoming, "1", 5));
    model.event(Event::LineState(Box::new(ringing)));
    model.lines_listed(vec![line(1, LineState::Idle), line(2, LineState::Idle)]);
    assert_eq!(model.lines.line(2).unwrap().state, LineState::Ringing);
    assert_eq!(model.lines.len(), 2);

    // A later list is the truth as it stands: nothing is laid over it.
    model.lines_listed(vec![line(1, LineState::Idle), line(2, LineState::Idle)]);
    assert_eq!(model.lines.line(2).unwrap().state, LineState::Idle);

    // The same for the calls and for the log.
    let mut model = Model::new();
    model.connected_to(service(ServiceMode::Service, 2, 0));
    model.event(Event::CallEnded(record(3, 1, EndReason::Remote)));
    model.event(Event::CallEnded(record(4, 1, EndReason::Remote)));
    model.event(Event::Log(log(
        9,
        LogLevel::Info,
        "SVC",
        "newer than the tail",
    )));
    model.event(Event::Log(log(5, LogLevel::Info, "SVC", "five")));
    // The list has call 3, which also came as an event, and not call 4.
    model.calls_listed(vec![
        record(1, 1, EndReason::Remote),
        record(2, 1, EndReason::Remote),
        record(3, 1, EndReason::Remote),
    ]);
    let ids: Vec<u64> = model.calls.calls().map(|call| call.id).collect();
    assert_eq!(ids, [4, 3, 2, 1], "newest first, each once");
    // The tail has the line of second 5, which also came as an event.
    model.log_tailed(vec![
        log(1, LogLevel::Info, "SVC", "one"),
        log(5, LogLevel::Info, "SVC", "five"),
    ]);
    let texts: Vec<&str> = model.log.lines().map(|l| l.event.text.as_str()).collect();
    assert_eq!(texts, ["one", "five", "newer than the tail"]);
}

#[test]
fn connecting_again_starts_the_log_and_the_history_over() {
    let mut model = connected_model(vec![line(1, LineState::Idle)]);
    model.log_tailed(vec![log(1, LogLevel::Info, "SVC", "old")]);
    model.calls_listed(vec![record(1, 1, EndReason::Remote)]);
    model.stats_received(Stats::default());
    let generation = model.log.generation();
    model.disconnected(Some(error("RMD2009", &["gone"])));
    model.connecting();
    model.connected_to(service(ServiceMode::Service, 1, 0));
    assert!(model.log.is_empty() && model.calls.is_empty());
    assert!(model.log.generation() > generation, "the pane starts over");
    assert!(model.stats().is_none());
    // The selection, by id, survives the lines being listed again.
    assert_eq!(model.lines.selected().unwrap().id, 1);
    model.lines_listed(vec![line(1, LineState::Idle), line(2, LineState::Idle)]);
    assert_eq!(model.lines.selected().unwrap().id, 1);
}

// --------------------------------------------------------------------------
// Commands
// --------------------------------------------------------------------------

/// The commands that can be chosen, in the order of [`Command::ALL`].
fn enabled(model: &Model) -> Vec<Command> {
    Command::ALL
        .into_iter()
        .filter(|command| model.availability(*command).is_enabled())
        .collect()
}

/// The commands that do not depend on the service or the selection.
const ALWAYS: [Command; 15] = [
    Command::Connect,
    Command::ServiceStart,
    Command::ServiceStop,
    Command::ServiceRestart,
    Command::ServiceInstall,
    Command::ServiceUninstall,
    Command::Exit,
    Command::AtConsole,
    Command::ShowAll,
    Command::ShowOnCall,
    Command::ShowIdle,
    Command::ShowNotReady,
    Command::ViewToolbar,
    Command::ViewStatusBar,
    Command::About,
];

/// The commands that need the service and nothing more.
const WITH_SERVICE: [Command; 13] = [
    Command::Disconnect,
    Command::ReloadConfig,
    Command::ExportStatistics,
    Command::NewLine,
    Command::TrunkPeers,
    Command::HuntGroups,
    Command::Routes,
    Command::SipTrace,
    Command::CallCapture,
    Command::CallHistory,
    Command::Refresh,
    Command::Settings,
    Command::PortsAndDrivers,
];

fn sorted(mut commands: Vec<Command>) -> Vec<Command> {
    commands.sort_by_key(|command| command.id());
    commands
}

#[test]
fn every_command_in_every_state_of_the_connection() {
    // Never connected.
    let mut model = Model::new();
    assert_eq!(enabled(&model), sorted(ALWAYS.to_vec()));
    assert_eq!(
        model.availability(Command::Disconnect).reason(),
        Some("The manager is not connected and is not trying to connect.")
    );
    for command in [Command::ReloadConfig, Command::Answer, Command::NewLine] {
        assert_eq!(
            model.availability(command).reason(),
            Some("Not connected to the service."),
            "{command:?}"
        );
    }

    // Connecting, and disconnected and trying again: the same, with
    // Disconnect to stop the trying.
    let mut expected = ALWAYS.to_vec();
    expected.push(Command::Disconnect);
    model.connecting();
    assert_eq!(enabled(&model), sorted(expected.clone()));
    model.disconnected(Some(error("RMD2008", &["rustmodem-control", "not found"])));
    assert_eq!(enabled(&model), sorted(expected));

    // Connected, with no line selected.
    let mut model = Model::new();
    model.connected_to(service(ServiceMode::Service, 1, 0));
    model.lines_listed(vec![line(1, LineState::Idle)]);
    let mut expected = ALWAYS.to_vec();
    expected.extend(WITH_SERVICE);
    assert_eq!(enabled(&model), sorted(expected));
    for command in [
        Command::Answer,
        Command::HangUp,
        Command::Reset,
        Command::Enable,
        Command::Disable,
        Command::EditLine,
        Command::RemoveLine,
    ] {
        assert_eq!(
            model.availability(command).reason(),
            Some("Select a line first."),
            "{command:?}"
        );
    }
    // The log has nothing in it yet.
    assert_eq!(
        model.availability(Command::ExportLog).reason(),
        Some("The event log is empty.")
    );
    model.log_tailed(vec![log(1, LogLevel::Info, "SVC", "x")]);
    assert!(model.availability(Command::ExportLog).is_enabled());
    // The log can still be exported after the service has gone.
    model.disconnected(None);
    assert!(model.availability(Command::ExportLog).is_enabled());
}

#[test]
fn every_line_command_for_every_state_of_the_selected_line() {
    use Command::{Answer, Disable, EditLine, Enable, HangUp, RemoveLine, Reset};
    let line_commands = [Answer, HangUp, Reset, Enable, Disable, EditLine, RemoveLine];
    let on_line = |selected: ctlproto::LineInfo| -> Vec<Command> {
        let model = connected_model(vec![selected]);
        line_commands
            .into_iter()
            .filter(|command| model.availability(*command).is_enabled())
            .collect()
    };
    let in_call = |state| line_with_call(1, state, call(Direction::Incoming, "2", 0));

    assert_eq!(
        on_line(line(1, LineState::Disabled)),
        [Reset, Enable, EditLine, RemoveLine]
    );
    assert_eq!(
        on_line(line(1, LineState::NotReady)),
        [Reset, Disable, EditLine, RemoveLine]
    );
    assert_eq!(
        on_line(broken_line(1)),
        [Reset, Disable, EditLine, RemoveLine]
    );
    assert_eq!(
        on_line(line(1, LineState::Idle)),
        [Reset, Disable, EditLine, RemoveLine]
    );
    // A line with a call can be answered only while it rings, and cannot
    // be removed until the call is over.
    assert_eq!(
        on_line(in_call(LineState::Ringing)),
        [Answer, HangUp, Reset, Disable, EditLine]
    );
    assert_eq!(
        on_line(in_call(LineState::Handshaking)),
        [HangUp, Reset, Disable, EditLine]
    );
    assert_eq!(
        on_line(in_call(LineState::Online)),
        [HangUp, Reset, Disable, EditLine]
    );

    // Why not, in each case, naming the line.
    let model = connected_model(vec![line(3, LineState::Idle)]);
    let reason = |model: &Model, command| model.availability(command).reason().map(String::from);
    assert_eq!(reason(&model, Answer).unwrap(), "Line 3 is not ringing.");
    assert_eq!(reason(&model, HangUp).unwrap(), "Line 3 has no call.");
    assert_eq!(
        reason(&model, Enable).unwrap(),
        "Line 3 is already enabled."
    );
    let model = connected_model(vec![line(3, LineState::Disabled)]);
    assert_eq!(
        reason(&model, Disable).unwrap(),
        "Line 3 is already disabled."
    );
    let model = connected_model(vec![in_call(LineState::Online)]);
    assert_eq!(
        reason(&model, RemoveLine).unwrap(),
        "Line 1 is in a call. Hang up before removing it."
    );

    // Without the service the selection does not matter.
    let mut model = connected_model(vec![in_call(LineState::Ringing)]);
    model.disconnected(Some(error("RMD2009", &["gone"])));
    assert!(model.lines.selected().is_some());
    for command in line_commands {
        assert_eq!(
            reason(&model, command).unwrap(),
            "Not connected to the service.",
            "{command:?}"
        );
    }
}

#[test]
fn the_commands_that_are_not_built_yet_are_never_enabled() {
    let states = [
        Model::new(),
        connected_model(vec![line(1, LineState::Idle)]),
        connected_model(vec![line_with_call(
            1,
            LineState::Online,
            call(Direction::Incoming, "2", 0),
        )]),
    ];
    for model in &states {
        for command in Command::ALL {
            let availability = model.availability(command);
            match command.blocked() {
                Some(phase) => {
                    let reason = availability
                        .reason()
                        .expect("a blocked command is disabled");
                    assert!(
                        reason.contains(&format!("phase {phase}")),
                        "{command:?}: {reason}"
                    );
                }
                None => {
                    // Every other command is enabled or says why not.
                    if let Availability::Disabled(reason) = &availability {
                        assert!(reason.ends_with('.'), "{command:?}: {reason}");
                        assert!(!reason.contains("phase"), "{command:?}: {reason}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_status_bar_describes_a_command_or_says_why_it_cannot_be_chosen() {
    let model = connected_model(vec![line(1, LineState::Idle)]);
    assert_eq!(
        model.help(Command::Reset),
        "Resets the selected line: hangs up and returns its modem to its stored profile."
    );
    assert_eq!(model.help(Command::Answer), "Line 1 is not ringing.");
    // What came with telephony says what it does, or why not now.
    assert!(
        model
            .help(Command::TrunkPeers)
            .starts_with("Says which addresses")
    );
    assert_eq!(
        model.help(Command::Reregister),
        "Line 1 does not register to a PBX."
    );
    assert_eq!(
        model.help(Command::LineDiagnostics),
        "Not available yet: this comes with the first modulation, in phase 3."
    );
    for command in Command::ALL {
        assert!(!model.help(command).is_empty());
    }
}

#[test]
fn the_filter_has_a_command_in_the_view_menu() {
    let mut model = Model::new();
    for filter in Filter::ALL {
        model.lines.set_filter(filter);
        assert_eq!(filter_of(model.filter_command()), Some(filter));
    }
    assert_eq!(filter_of(Command::Refresh), None);
}

// --------------------------------------------------------------------------
// The status bar
// --------------------------------------------------------------------------

#[test]
fn the_status_bar_has_the_connection_sip_answering_rtp_cpu_and_the_clock() {
    let mut model = connected_model(vec![
        line(1, LineState::Idle),
        line_with_call(2, LineState::Online, call(Direction::Incoming, "1", 0)),
        line(3, LineState::NotReady),
        line(4, LineState::Disabled),
        broken_line(5),
    ]);
    // 08:24:57 UTC. Until the statistics have come, nothing is known of
    // SIP, RTP or the processor.
    let parts = model.status_parts(START_MS + 1_497_170, &Utc);
    assert_eq!(
        parts,
        [
            "Service: connected",
            "SIP: \u{2014}",
            "Answering: 2 / 5",
            "RTP: \u{2014}",
            "CPU: \u{2014}",
            "08:24",
        ]
    );
    // A service with no SIP.
    model.stats_received(Stats::default());
    let parts = model.status_parts(START_MS + 1_497_170, &Utc);
    assert_eq!(
        parts[1..5],
        [
            "SIP: off",
            "Answering: 2 / 5",
            "RTP: \u{2014}",
            "CPU: 0.0 %"
        ]
    );
    model.disconnected(None);
    let parts = model.status_parts(START_MS, &Utc);
    assert_eq!(parts[0], "Service: not connected");
    assert_eq!(parts[2], "Answering: \u{2014}");
}

// --------------------------------------------------------------------------
// Statistics
// --------------------------------------------------------------------------

fn tally(rate: u32, carrier: &str, count: u64) -> ConnectTally {
    ConnectTally {
        rate,
        carrier: (!carrier.is_empty()).then(|| carrier.to_string()),
        count,
    }
}

#[test]
fn the_statistics_panels_show_what_the_service_reports() {
    let mut model = connected_model(vec![line(1, LineState::Idle)]);
    // Not asked for yet: every value an em dash.
    let empty = model.stats_view(START_MS, &Utc);
    assert_eq!(empty, StatsView::empty());
    assert!(empty.calls.iter().all(|(_, value)| value == EMPTY));
    assert!(empty.rates.is_empty());

    // The frozen demo's numbers.
    model.stats_received(Stats {
        calls_answered: 7,
        calls_connected: 6,
        calls_failed: 1,
        calls_rejected: 2,
        calls_abandoned: 1,
        avg_call_ms: 253_000,
        longest_call_ms: 727_000,
        peak_lines_in_use: 4,
        lines_in_use: 4,
        bytes_in: 122_400,
        bytes_out: 122_400,
        at_commands: 23,
        connects: vec![
            tally(2400, "V.22bis", 3),
            tally(1200, "V.22", 2),
            tally(300, "V.21", 1),
        ],
        retrains: 0,
        protocols: Vec::new(),
        compressions: Vec::new(),
        ..Stats::default()
    });
    let view = model.stats_view(START_MS + 1_497_170, &Utc);
    let calls: Vec<(&str, &str)> = view.calls.iter().map(|(l, v)| (*l, v.as_str())).collect();
    assert_eq!(
        calls,
        [
            ("Calls answered", "7"),
            ("Connected successfully", "6"),
            ("Failed to connect", "1"),
            ("Rejected (line busy)", "2"),
            ("Caller hung up early", "1"),
            ("Avg. call length", "4:13"),
            ("Longest call", "12:07"),
            ("Peak lines in use", "4"),
            ("Bytes in / out", "122,400 / 122,400"),
            ("AT commands parsed", "23"),
            ("Service uptime", "0:24:57"),
            ("Config loaded", "08:00:00"),
        ]
    );
    let rates: Vec<(&str, u32, u64)> = view
        .rates
        .iter()
        .map(|r| (r.label.as_str(), r.percent, r.count))
        .collect();
    assert_eq!(
        rates,
        [
            ("2400 V.22bis", 50, 3),
            ("1200 V.22", 33, 2),
            ("300 V.21", 17, 1)
        ]
    );
    assert_eq!(view.rates.iter().map(|r| r.percent).sum::<u32>(), 100);
    let carrier: Vec<(&str, &str)> = view.carrier.iter().map(|(l, v)| (*l, v.as_str())).collect();
    assert_eq!(
        carrier,
        [
            ("Retrains / renegotiations", EMPTY),
            ("Error correction", EMPTY),
            ("Softmodem engine", "null"),
            ("RTP codec / ptime", EMPTY),
            ("Jitter (avg / max)", EMPTY),
            ("Packet loss", EMPTY),
        ]
    );
    assert_eq!(view.calls.len(), CALL_LABELS.len());
    assert_eq!(view.carrier.len(), CARRIER_LABELS.len());

    // Without the service the panels go back to dashes.
    model.disconnected(None);
    assert_eq!(model.stats_view(START_MS, &Utc), StatsView::empty());
}

#[test]
fn the_connect_rates_fit_in_five_rows_and_always_add_up() {
    let info = service(ServiceMode::Service, 1, 0);
    let view = |connects: Vec<ConnectTally>| {
        let stats = Stats {
            calls_connected: connects.iter().map(|c| c.count).sum(),
            connects,
            ..Stats::default()
        };
        StatsView::new(&stats, &info, "null", START_MS, &Utc)
    };
    // The null engine: one group, of which nothing is known.
    let one = view(vec![tally(0, "", 4)]);
    assert_eq!(one.rates.len(), 1);
    assert_eq!(
        (one.rates[0].label.as_str(), one.rates[0].percent),
        ("Rate not known", 100)
    );
    // A rate with no carrier, and a carrier with no rate.
    let odd = view(vec![tally(9600, "", 1), tally(0, "V.32", 1)]);
    assert_eq!(odd.rates[0].label, "9600 bit/s");
    assert_eq!(odd.rates[1].label, "V.32");

    // Seven groups: the four largest, and the rest together.
    let many = view(vec![
        tally(33_600, "V.34", 40),
        tally(14_400, "V.32bis", 25),
        tally(2400, "V.22bis", 20),
        tally(1200, "V.22", 9),
        tally(300, "V.21", 3),
        tally(300, "Bell 103", 2),
        tally(0, "", 1),
    ]);
    let rows: Vec<(&str, u32, u64)> = many
        .rates
        .iter()
        .map(|r| (r.label.as_str(), r.percent, r.count))
        .collect();
    assert_eq!(
        rows,
        [
            ("33600 V.34", 40, 40),
            ("14400 V.32bis", 25, 25),
            ("2400 V.22bis", 20, 20),
            ("1200 V.22", 9, 9),
            ("Other", 6, 6),
        ]
    );
    // Exactly five groups are shown as they are.
    let five = view((1..=5).map(|n| tally(n * 300, "x", 1)).collect());
    assert_eq!(five.rates.len(), 5);
    assert!(five.rates.iter().all(|r| r.label != "Other"));
    assert_eq!(five.rates.iter().map(|r| r.percent).sum::<u32>(), 100);
    // No connects: no rows, and no lengths to give.
    let none = view(Vec::new());
    assert!(none.rates.is_empty());
    assert_eq!(none.calls[5].1, EMPTY);
    assert_eq!(none.calls[6].1, EMPTY);
}

#[test]
fn later_engines_fill_in_retrains_and_error_correction() {
    let info = service(ServiceMode::Service, 1, 0);
    let stats = Stats {
        calls_connected: 5,
        retrains: 1234,
        protocols: vec![
            Tally {
                name: "LAPM".to_string(),
                count: 4,
            },
            Tally {
                name: "MNP".to_string(),
                count: 1,
            },
        ],
        ..Stats::default()
    };
    let view = StatsView::new(&stats, &info, "null", START_MS, &Utc);
    assert_eq!(view.carrier[0].1, "1,234");
    assert_eq!(view.carrier[1].1, "LAPM (and 1 more)");
    let one = Stats {
        protocols: vec![Tally {
            name: "LAPM".to_string(),
            count: 4,
        }],
        ..Stats::default()
    };
    assert_eq!(
        StatsView::new(&one, &info, "null", START_MS, &Utc).carrier[1].1,
        "LAPM"
    );
}

#[test]
fn the_configuration_shows_its_date_once_it_was_not_loaded_today() {
    let mut info = service(ServiceMode::Service, 1, 0);
    info.config_loaded_ms = START_MS + 3_600_000;
    let stats = Stats::default();
    let same_day = StatsView::new(&stats, &info, "null", START_MS + 7_200_000, &Utc);
    assert_eq!(same_day.calls[11].1, "09:00:00");
    let next_day = StatsView::new(&stats, &info, "null", START_MS + 26 * 3_600_000, &Utc);
    assert_eq!(next_day.calls[11].1, "2027-01-15 09:00:00");
    // Three days, four hours, twelve minutes and seven seconds.
    let long = StatsView::new(&stats, &info, "null", START_MS + 274_327_000, &Utc);
    assert_eq!(long.calls[10].1, "3d 04:12:07");
}

#[test]
fn the_statistics_export_as_csv() {
    let info = service(ServiceMode::Service, 1, 0);
    let stats = Stats {
        calls_answered: 1234,
        calls_connected: 3,
        bytes_in: 1000,
        bytes_out: 2000,
        connects: vec![tally(2400, "V.22bis", 2), tally(1200, "V.22", 1)],
        ..Stats::default()
    };
    let csv = StatsView::new(&stats, &info, "null", START_MS + 60_000, &Utc).csv();
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert_eq!(lines[0], "Statistic,Value");
    // A value with a comma in it is quoted.
    assert_eq!(lines[1], "Calls answered,\"1,234\"");
    assert_eq!(lines[9], "Bytes in / out,\"1,000 / 2,000\"");
    assert_eq!(lines[11], "Service uptime,0:01:00");
    assert_eq!(lines[15], "Softmodem engine,null");
    assert_eq!(lines[19], "");
    assert_eq!(lines[20], "Connect rate,Calls,Percent");
    assert_eq!(lines[21], "2400 V.22bis,2,67");
    assert_eq!(lines[22], "1200 V.22,1,33");
    assert_eq!(lines.len(), 24, "and the file ends with a line ending");
}

// --------------------------------------------------------------------------
// The event log
// --------------------------------------------------------------------------

#[test]
fn log_lines_have_a_class_by_their_level_whatever_their_source() {
    let class = |level, source: &str| LogClass::of(&log(0, level, source, "x"));
    for source in ["Line01", "CTL", "SVC", "POOL", "Something"] {
        assert_eq!(class(LogLevel::Error, source), LogClass::Error);
        assert_eq!(class(LogLevel::Warning, source), LogClass::Warning);
        assert_eq!(class(LogLevel::Info, source), LogClass::Info);
        assert_eq!(class(LogLevel::Debug, source), LogClass::Debug);
    }
}

#[test]
fn a_log_line_shows_its_time_level_source_and_text() {
    let mut buffer = LogBuffer::new();
    buffer.push(log(
        3723,
        LogLevel::Info,
        "Line01",
        "RMD1005: Line 1 state is now idle.",
    ));
    buffer.push(log(3724, LogLevel::Warning, "POOL", "w"));
    buffer.push(log(3725, LogLevel::Error, "SVC", "e"));
    buffer.push(log(3726, LogLevel::Debug, "Line02", "> AT"));
    let shown: Vec<String> = buffer.lines().map(|line| line.text(&Utc)).collect();
    assert_eq!(
        shown,
        [
            "09:02:03 INFO  [Line01] RMD1005: Line 1 state is now idle.",
            "09:02:04 WARN  [POOL] w",
            "09:02:05 ERROR [SVC] e",
            "09:02:06 DEBUG [Line02] > AT",
        ]
    );
    // Exported, each line has its date too.
    let text = buffer.text(&Utc);
    assert!(
        text.starts_with(
            "2027-01-15 09:02:03 INFO  [Line01] RMD1005: Line 1 state is now idle.\r\n"
        )
    );
    assert_eq!(text.matches("\r\n").count(), 4);
}

#[test]
fn the_log_keeps_its_last_five_thousand_lines() {
    let mut buffer = LogBuffer::new();
    assert!(buffer.is_empty());
    for second in 0..LOG_LIMIT as u64 + 250 {
        assert!(buffer.push(log(
            second,
            LogLevel::Info,
            "SVC",
            &format!("line {second}")
        )));
    }
    assert_eq!(buffer.len(), LOG_LIMIT);
    assert_eq!(buffer.added(), LOG_LIMIT as u64 + 250);
    assert_eq!(buffer.lines().next().unwrap().event.text, "line 250");
    assert_eq!(buffer.lines().last().unwrap().event.text, "line 5249");
}

#[test]
fn a_pane_takes_the_lines_it_has_not_shown_yet() {
    let mut buffer = LogBuffer::new();
    let texts = |buffer: &LogBuffer, seen: u64| -> Vec<String> {
        buffer.since(seen).map(|l| l.event.text.clone()).collect()
    };
    buffer.push(log(1, LogLevel::Info, "SVC", "a"));
    buffer.push(log(2, LogLevel::Info, "SVC", "b"));
    assert_eq!(texts(&buffer, 0), ["a", "b"]);
    let seen = buffer.added();
    assert!(texts(&buffer, seen).is_empty());
    buffer.push(log(3, LogLevel::Info, "SVC", "c"));
    assert_eq!(texts(&buffer, seen), ["c"]);
    // A pane further behind than the buffer keeps gets what there is.
    for second in 4..LOG_LIMIT as u64 + 10 {
        buffer.push(log(second, LogLevel::Info, "SVC", "x"));
    }
    assert_eq!(buffer.since(seen).count(), LOG_LIMIT);
    // A pane that says it has seen more than there is gets nothing.
    assert_eq!(buffer.since(u64::MAX).count(), 0);

    // Starting over is a new generation, counted from nothing.
    let generation = buffer.generation();
    buffer.clear();
    assert_eq!((buffer.added(), buffer.len()), (0, 0));
    assert_eq!(buffer.generation(), generation + 1);
    buffer.set_tail(vec![log(1, LogLevel::Info, "SVC", "tail")]);
    assert_eq!(buffer.generation(), generation + 2);
    assert_eq!(texts(&buffer, 0), ["tail"]);
}

#[test]
fn a_line_that_comes_in_the_tail_and_as_an_event_is_taken_once() {
    let mut buffer = LogBuffer::new();
    let tail = vec![
        log(1, LogLevel::Info, "SVC", "one"),
        log(2, LogLevel::Info, "Line01", "two"),
        log(2, LogLevel::Info, "Line01", "two again"),
    ];
    buffer.set_tail(tail.clone());
    // The last lines of the tail arrive again as events.
    assert!(!buffer.push(tail[1].clone()));
    assert!(!buffer.push(tail[2].clone()));
    assert_eq!(buffer.len(), 3);
    // A line the tail did not have, at the same moment as its last line.
    assert!(buffer.push(log(2, LogLevel::Info, "Line01", "also at two")));
    // The same text later is another line: a host that gives AT twice
    // logs it twice.
    assert!(buffer.push(log(3, LogLevel::Debug, "Line01", "> AT")));
    assert!(buffer.push(log(3, LogLevel::Debug, "Line01", "> AT")));
    assert_eq!(buffer.len(), 6);
    // With no tail there is nothing to tell a repeat from.
    let mut plain = LogBuffer::new();
    assert!(plain.push(tail[0].clone()));
    assert!(plain.push(tail[0].clone()));
}

// --------------------------------------------------------------------------
// The call history
// --------------------------------------------------------------------------

#[test]
fn the_call_history_is_newest_first_and_takes_each_call_once() {
    let mut history = CallHistory::new();
    assert!(history.is_empty());
    // The service lists oldest first.
    history.set_all((1..=4).map(|id| record(id, 1, EndReason::Remote)).collect());
    let ids = |history: &CallHistory| -> Vec<u64> { history.calls().map(|c| c.id).collect() };
    assert_eq!(ids(&history), [4, 3, 2, 1]);
    assert!(history.push(record(5, 2, EndReason::Busy)));
    assert!(
        !history.push(record(5, 2, EndReason::Busy)),
        "the same call again"
    );
    assert_eq!(ids(&history), [5, 4, 3, 2, 1]);
    assert_eq!(history.row(0).unwrap().id, 5);
    assert!(history.row(9).is_none());

    // As many as the service keeps, and no more.
    for id in 6..=CALL_LIMIT as u64 + 20 {
        history.push(record(id, 1, EndReason::Remote));
    }
    assert_eq!(history.len(), CALL_LIMIT);
    assert_eq!(history.row(0).unwrap().id, CALL_LIMIT as u64 + 20);
    assert_eq!(history.calls().last().unwrap().id, 21);
    history.clear();
    assert!(history.is_empty());
}

#[test]
fn every_cell_of_the_call_history() {
    let mut connected_call = record(2, 3, EndReason::Dtr);
    connected_call.direction = Direction::Outgoing;
    connected_call.number = "2".to_string();
    connected_call.connected_ms = Some(connected_call.started_ms + 4000);
    connected_call.ended_ms = connected_call.started_ms + 4000 + 727_000;
    connected_call.connect = Some(ctlproto::ConnectInfo {
        rate: 2400,
        carrier: Some("V.22bis".to_string()),
        protocol: None,
        compression: None,
    });
    connected_call.bytes_tx = 48_000;
    connected_call.bytes_rx = 16_000;
    let mut history = CallHistory::new();
    history.set_all(vec![record(1, 1, EndReason::Abandoned), connected_call]);
    let row = |row: usize| -> Vec<String> {
        (0..CALL_COLUMNS.len())
            .map(|column| history.cell(row, column, &Utc))
            .collect()
    };
    assert_eq!(
        row(0),
        [
            "2027-01-15 08:02:00",
            "3",
            "Outgoing",
            "2",
            "12:07",
            "Host dropped DTR",
            "2400 V.22bis",
            "48,000 / 16,000",
            EMPTY,
            EMPTY,
            EMPTY,
        ]
    );
    assert_eq!(
        row(1),
        [
            "2027-01-15 08:01:00",
            "1",
            "Incoming",
            "5550100",
            EMPTY,
            "Caller gave up",
            EMPTY,
            "0 / 0",
            EMPTY,
            EMPTY,
            EMPTY,
        ]
    );
    assert_eq!(history.cell(5, 0, &Utc), "");
    assert_eq!(history.cell(0, 99, &Utc), "");
    let titles: Vec<&str> = CALL_COLUMNS.iter().map(|(title, _, _)| *title).collect();
    assert_eq!(
        titles,
        [
            "Time",
            "Line",
            "Direction",
            "Number",
            "Duration",
            "Reason",
            "Rate",
            "Bytes TX/RX",
            "Codec",
            "Lost",
            "Slips"
        ]
    );
    // Every reason has words of its own.
    let mut words = std::collections::BTreeSet::new();
    for reason in EndReason::ALL {
        assert!(words.insert(model::reason_text(reason)));
    }

    // As CSV the numbers are numbers.
    let csv = history.csv(&Utc);
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert_eq!(
        lines[0],
        "Time,Line,Direction,Number,Connected seconds,Reason,Rate,Carrier,Bytes TX,Bytes RX,\
         Codec,Lost,Late,Duplicate,Resync,Slips"
    );
    assert_eq!(
        lines[1],
        "2027-01-15 08:02:00,3,outgoing,2,727,dtr,2400,V.22bis,48000,16000,,,,,,"
    );
    assert_eq!(
        lines[2],
        "2027-01-15 08:01:00,1,incoming,5550100,,abandoned,,,0,0,,,,,,"
    );
    assert_eq!(lines.len(), 4);
}

#[test]
fn a_connected_call_of_which_nothing_is_known_says_so() {
    let mut history = CallHistory::new();
    let mut unknown = record(1, 1, EndReason::Remote);
    unknown.connected_ms = Some(unknown.started_ms);
    unknown.connect = Some(
        connected(call(Direction::Incoming, "1", 0), 0, 0, "")
            .connect
            .unwrap(),
    );
    history.push(unknown);
    assert_eq!(history.cell(0, 6, &Utc), "Not known");
    assert_eq!(history.cell(0, 4, &Utc), "0:05");
}
