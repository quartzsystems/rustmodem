// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the model shows of SIP and RTP: the SIP and hunt group columns,
//! the grid narrowed to a hunt group, the errors of a call, the status
//! bar, the panels, the call history and the trace.

mod common;

use common::{START_MS, call, error, line, line_with_call, record, service};
use ctlproto::{
    Direction, EndReason, Event, LineInfo, LineSip, LineState, ListenerState, MediaInfo,
    Registration, RegistrationState, ServiceMode, SipMode, SipSummary, SipTrace, Stats,
    TraceDirection,
};
use model::{
    Availability, CallHistory, Column, Command, EMPTY, Model, RowChange, StateLook, TRACE_LIMIT,
    TraceBuffer, Utc, registration_failed, row_image, sip_tip, trace_entry,
};

fn trunk(id: u16, groups: &[&str]) -> LineInfo {
    let mut line = line(id, LineState::Idle);
    line.sip.mode = SipMode::Trunk;
    line.hunt_groups = groups.iter().map(|group| group.to_string()).collect();
    line
}

fn registered(id: u16, state: RegistrationState) -> LineInfo {
    let mut line = line(id, LineState::Idle);
    line.sip = LineSip {
        mode: SipMode::Registered,
        extension: Some(format!("2{id:02}")),
        server: Some("pbx.example.com".to_string()),
        auth_user: None,
        expires_s: Some(300),
        registration: Some(Registration {
            state,
            expires_in_s: (state == RegistrationState::Registered).then_some(280),
            error: (state == RegistrationState::Failed)
                .then(|| error("RMD5004", &[&id.to_string(), "pbx.example.com"])),
        }),
        has_password: true,
    };
    line
}

fn media(lost: u64, late: u64, resync: u64, slips: u64) -> MediaInfo {
    MediaInfo {
        codec: "PCMU".to_string(),
        ptime_ms: 20,
        remote: Some("10.10.20.11:16384".to_string()),
        rx_packets: 3000,
        tx_packets: 3000,
        lost,
        late,
        resync,
        slips,
        ..MediaInfo::default()
    }
}

fn connected(lines: Vec<LineInfo>) -> Model {
    let mut model = Model::new();
    model.connecting();
    model.connected_to(service(ServiceMode::Service, lines.len() as u32, 60));
    model.lines_listed(lines);
    model
}

fn sip_stats(registered: u32, registering: u32) -> Stats {
    Stats {
        rtp_kbps: 413,
        jitter_ms_avg: 0.25,
        jitter_ms_max: 1.875,
        packet_loss_percent: 0.125,
        codec: "PCMU".to_string(),
        ptime_ms: 20,
        cpu_percent: 1.26,
        sip: SipSummary {
            listener: ListenerState::Listening,
            registered,
            registering,
            dropped_untrusted: 0,
        },
        ..Stats::default()
    }
}

#[test]
fn the_sip_column_shows_the_extension_or_the_trunk_and_its_tip_the_registration() {
    let model = connected(vec![
        line(1, LineState::Idle),
        trunk(2, &[]),
        registered(3, RegistrationState::Registered),
        registered(4, RegistrationState::Failed),
        registered(5, RegistrationState::Registering),
    ]);
    let cell = |row: usize| model.lines.cell(row, Column::Sip, START_MS);
    assert_eq!(cell(0), EMPTY);
    assert_eq!(cell(1), "trunk");
    assert_eq!(cell(2), "203");
    assert_eq!(cell(3), "204");

    let tip = |id: u16| sip_tip(model.lines.line(id).unwrap());
    assert_eq!(tip(1), None);
    assert_eq!(
        tip(2).unwrap(),
        "Reached by the trunk: calls come from the trusted peers."
    );
    assert_eq!(
        tip(3).unwrap(),
        "Extension 203 of pbx.example.com: registered, 280 s left."
    );
    let failed = tip(4).unwrap();
    assert!(
        failed.starts_with("Extension 204 of pbx.example.com: registration failed. RMD5004: "),
        "{failed}"
    );
    assert_eq!(
        tip(5).unwrap(),
        "Extension 205 of pbx.example.com: registering."
    );
    // A line that registers, in a service where nothing does.
    let mut idle = registered(6, RegistrationState::Registered);
    idle.sip.registration = None;
    assert_eq!(
        sip_tip(&idle).unwrap(),
        "Extension 206 of pbx.example.com: not registering."
    );
}

#[test]
fn a_line_whose_registration_failed_has_the_red_image_while_it_waits() {
    let failed = registered(1, RegistrationState::Failed);
    assert!(registration_failed(&failed));
    assert_eq!(row_image(&failed), StateLook::PortError.image());
    // What it says of its state is still its state.
    assert_eq!(StateLook::of(&failed), StateLook::Idle);

    let fine = registered(2, RegistrationState::Registered);
    assert!(!registration_failed(&fine));
    assert_eq!(row_image(&fine), StateLook::Idle.image());
    assert_eq!(row_image(&trunk(3, &[])), StateLook::Idle.image());

    // A line that is doing something shows what it is doing.
    let mut busy = registered(4, RegistrationState::Failed);
    busy.state = LineState::Online;
    assert_eq!(row_image(&busy), StateLook::Online.image());
    let mut off = registered(5, RegistrationState::Failed);
    off.state = LineState::Disabled;
    assert_eq!(row_image(&off), StateLook::Disabled.image());
}

#[test]
fn the_hunt_group_column_names_the_groups_and_the_grid_narrows_to_one() {
    let mut model = connected(vec![
        trunk(1, &["dialup"]),
        trunk(2, &["dialup", "night"]),
        trunk(3, &[]),
        trunk(4, &["night"]),
    ]);
    let cell = |model: &Model, row: usize| model.lines.cell(row, Column::HuntGroup, START_MS);
    assert_eq!(cell(&model, 0), "dialup");
    assert_eq!(cell(&model, 1), "dialup, night");
    assert_eq!(cell(&model, 2), EMPTY);
    assert_eq!(model.lines.groups(), ["dialup", "night"]);
    assert_eq!(model.lines.group(), None);

    let shown = |model: &Model| -> Vec<u16> {
        (0..model.lines.row_count())
            .filter_map(|row| model.lines.row(row).map(|line| line.id))
            .collect()
    };
    assert_eq!(
        model.lines.set_group(Some("night".to_string())),
        RowChange::Reset
    );
    assert_eq!(shown(&model), [2, 4]);
    assert_eq!(model.lines.group(), Some("night"));
    assert_eq!(
        model.lines.set_group(Some("night".to_string())),
        RowChange::None
    );

    // A line that joins the group is a row added, one that leaves it a
    // row removed.
    assert_eq!(model.lines.apply(trunk(3, &["night"])), RowChange::Added(1));
    assert_eq!(
        model.lines.apply(trunk(2, &["dialup"])),
        RowChange::Removed(0)
    );
    assert_eq!(shown(&model), [3, 4]);

    // The filter and the group narrow together.
    let mut ringing = trunk(4, &["night"]);
    ringing.state = LineState::Ringing;
    model.lines.apply(ringing);
    model.lines.set_filter(model::Filter::OnCall);
    assert_eq!(shown(&model), [4]);
    model.lines.set_filter(model::Filter::All);

    // A group that is gone does not leave the grid empty.
    model.lines_listed(vec![trunk(1, &["dialup"]), trunk(2, &["dialup"])]);
    assert_eq!(model.lines.group(), None);
    assert_eq!(shown(&model), [1, 2]);
    assert_eq!(model.lines.set_group(None), RowChange::None);
}

#[test]
fn the_err_column_counts_what_went_wrong_with_the_rtp_of_the_call() {
    let mut with_media = call(Direction::Incoming, "5550100", 0);
    with_media.media = Some(Box::new(media(1200, 30, 4, 0)));
    let model = connected(vec![
        line(1, LineState::Idle),
        line_with_call(2, LineState::Online, with_media),
        line_with_call(3, LineState::Online, call(Direction::Incoming, "1", 0)),
    ]);
    let cell = |row: usize| model.lines.cell(row, Column::Errors, START_MS);
    assert_eq!(cell(0), EMPTY);
    assert_eq!(cell(1), "1,234");
    // A call with no RTP, between two lines of the service.
    assert_eq!(cell(2), EMPTY);
}

#[test]
fn the_status_bar_says_how_sip_stands_and_how_much_rtp_and_cpu_there_is() {
    let mut model = connected(vec![registered(1, RegistrationState::Registered)]);
    let parts = |model: &Model| model.status_parts(START_MS, &Utc);

    model.stats_received(sip_stats(1, 2));
    let shown = parts(&model);
    assert_eq!(shown[1], "SIP: 1 / 2 registered");
    assert_eq!(shown[3], "RTP: 413 kbit/s");
    assert_eq!(shown[4], "CPU: 1.3 %");

    // A trunk with no line that registers.
    model.stats_received(sip_stats(0, 0));
    assert_eq!(parts(&model)[1], "SIP: listening");

    let mut failed = sip_stats(0, 0);
    failed.sip.listener = ListenerState::Failed;
    model.stats_received(failed);
    let shown = parts(&model);
    assert_eq!(shown[1], "SIP: not listening");
    assert_eq!(shown[3], format!("RTP: {EMPTY}"));

    model.stats_received(Stats::default());
    assert_eq!(parts(&model)[1], "SIP: off");

    model.disconnected(None);
    let shown = parts(&model);
    assert_eq!(shown[1], format!("SIP: {EMPTY}"));
    assert_eq!(shown[4], format!("CPU: {EMPTY}"));
}

#[test]
fn the_panel_shows_the_engines_the_codec_the_jitter_and_the_loss() {
    let mut echo = trunk(2, &[]);
    echo.engine = "echo".to_string();
    let mut model = connected(vec![trunk(1, &[]), echo, trunk(3, &[])]);
    model.stats_received(sip_stats(0, 0));
    let view = model.stats_view(START_MS, &Utc);
    let value = |label: &str| -> String {
        view.carrier
            .iter()
            .find(|(name, _)| *name == label)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(value("Softmodem engine"), "echo, null");
    assert_eq!(value("RTP codec / ptime"), "PCMU / 20 ms");
    assert_eq!(value("Jitter (avg / max)"), "0.2 / 1.9 ms");
    assert_eq!(value("Packet loss"), "0.12 %");

    // A service with no SIP has none of the last three.
    model.stats_received(Stats::default());
    let view = model.stats_view(START_MS, &Utc);
    assert_eq!(view.carrier[2].1, "echo, null");
    for (_, value) in &view.carrier[3..6] {
        assert_eq!(value, EMPTY);
    }
}

#[test]
fn re_register_is_for_a_line_that_registers_or_for_all_of_them() {
    let mut model = connected(vec![
        trunk(1, &[]),
        registered(2, RegistrationState::Failed),
    ]);
    // Nothing selected: every line that registers.
    assert!(model.availability(Command::Reregister).is_enabled());
    model.lines.select_row(Some(0));
    assert_eq!(
        model.availability(Command::Reregister),
        Availability::Disabled("Line 1 does not register to a PBX.".to_string())
    );
    model.lines.select_row(Some(1));
    assert!(model.availability(Command::Reregister).is_enabled());

    let mut none = connected(vec![trunk(1, &[])]);
    assert_eq!(
        none.availability(Command::Reregister).reason(),
        Some("No line registers to a PBX.")
    );
    none.disconnected(None);
    for command in [
        Command::Reregister,
        Command::TrunkPeers,
        Command::HuntGroups,
        Command::Routes,
        Command::SipTrace,
        Command::CallCapture,
    ] {
        assert_eq!(
            none.availability(command).reason(),
            Some("Not connected to the service."),
            "{command:?}"
        );
    }
}

#[test]
fn the_call_history_shows_the_codec_and_marks_a_call_that_lost_packets() {
    let mut clean = record(1, 1, EndReason::Remote);
    clean.media = Some(Box::new(media(0, 2, 0, 0)));
    let mut lossy = record(2, 2, EndReason::Remote);
    lossy.media = Some(Box::new(media(1500, 0, 1, 0)));
    let mut slipped = record(3, 3, EndReason::Remote);
    slipped.media = Some(Box::new(media(0, 0, 0, 2)));
    let mut history = CallHistory::new();
    history.set_all(vec![clean, lossy, slipped, record(4, 4, EndReason::Busy)]);

    // Newest first: the call with no RTP, the slipped, the lossy, the
    // clean.
    let cells = |row: usize| -> Vec<String> {
        (11..14)
            .map(|column| history.cell(row, column, &Utc))
            .collect()
    };
    assert_eq!(cells(0), [EMPTY, EMPTY, EMPTY]);
    assert_eq!(cells(1), ["PCMU", "0", "2"]);
    assert_eq!(cells(2), ["PCMU", "1,500", "0"]);
    assert_eq!(cells(3), ["PCMU", "0", "0"]);
    assert_eq!(
        (0..5).map(|row| history.marked(row)).collect::<Vec<_>>(),
        [false, true, true, false, false]
    );

    let csv = history.csv(&Utc);
    let lines: Vec<&str> = csv.split("\r\n").collect();
    assert!(lines[0].ends_with(",Codec,Lost,Late,Duplicate,Resync,Slips"));
    assert!(lines[1].ends_with(",,,,,,"), "{}", lines[1]);
    assert!(lines[3].ends_with(",PCMU,1500,0,0,1,0"), "{}", lines[3]);
    assert!(lines[4].ends_with(",PCMU,0,2,0,0,0"), "{}", lines[4]);
}

fn message(n: u64, direction: TraceDirection) -> SipTrace {
    SipTrace {
        time_ms: START_MS + n * 1000 + 42,
        direction,
        peer: "10.10.20.11:5060".to_string(),
        text: format!("OPTIONS sip:rustmodem SIP/2.0\r\nCSeq: {n} OPTIONS\r\n\r\n"),
    }
}

#[test]
fn the_trace_keeps_the_last_messages_and_stands_still_while_paused() {
    let mut trace = TraceBuffer::new();
    assert!(trace.is_empty() && !trace.paused());
    assert!(trace.push(message(1, TraceDirection::Received)));
    assert!(trace.push(message(2, TraceDirection::Sent)));
    assert_eq!((trace.len(), trace.taken()), (2, 2));
    assert_eq!(
        trace.text(&Utc),
        "--- 08:00:01.042 received from 10.10.20.11:5060 ---\n\
         OPTIONS sip:rustmodem SIP/2.0\nCSeq: 1 OPTIONS\n\n\
         --- 08:00:02.042 sent to 10.10.20.11:5060 ---\n\
         OPTIONS sip:rustmodem SIP/2.0\nCSeq: 2 OPTIONS\n\n"
    );
    assert_eq!(
        trace_entry(trace.last().unwrap(), &Utc),
        "--- 08:00:02.042 sent to 10.10.20.11:5060 ---\n\
         OPTIONS sip:rustmodem SIP/2.0\nCSeq: 2 OPTIONS\n\n"
    );

    trace.set_paused(true);
    assert!(!trace.push(message(3, TraceDirection::Sent)));
    assert_eq!(trace.len(), 2);
    trace.set_paused(false);

    for n in 3..TRACE_LIMIT as u64 + 10 {
        trace.push(message(n, TraceDirection::Sent));
    }
    assert_eq!(trace.len(), TRACE_LIMIT);
    assert_eq!(trace.taken(), TRACE_LIMIT as u64 + 9);
    assert!(trace.messages().next().unwrap().text.contains("CSeq: 10 "));
    trace.clear();
    assert!(trace.is_empty());
    assert_eq!(trace.taken(), 0);
}

#[test]
fn a_trace_event_goes_to_the_trace_and_a_registration_changes_nothing_by_itself() {
    let mut model = connected(vec![registered(1, RegistrationState::Registering)]);
    let changes = model.event(Event::SipTrace(message(1, TraceDirection::Received)));
    assert!(changes.trace && !changes.log && !changes.calls);
    assert_eq!(changes.rows, RowChange::None);
    assert_eq!(model.trace.len(), 1);

    model.trace.set_paused(true);
    let changes = model.event(Event::SipTrace(message(2, TraceDirection::Sent)));
    assert!(!changes.trace);

    // The registration reaches the grid with the line's own event.
    let changes = model.event(Event::SipRegistration(ctlproto::SipRegistration {
        line: 1,
        server: "pbx.example.com".to_string(),
        user: "201".to_string(),
        registration: Registration {
            state: RegistrationState::Registered,
            expires_in_s: Some(300),
            error: None,
        },
    }));
    assert_eq!(changes, model::Changes::NONE);
    let changes = model.event(Event::LineState(Box::new(registered(
        1,
        RegistrationState::Registered,
    ))));
    assert_eq!(changes.rows, RowChange::Changed(0));
}
