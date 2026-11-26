// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The line table: every kind of change against the rows of the grid, the
//! selection through each, sorting, and what every cell shows.

mod common;

use common::{START_MS, broken_line, call, connected, line, line_with_call};
use ctlproto::{Direction, LineInfo, LineState};
use model::{Column, EMPTY, Filter, LineTable, RowChange, SortOrder, StateColour, StateLook};
use proptest::prelude::*;

/// A table with lines 1 to `count`, all idle.
fn table(count: u16) -> LineTable {
    let mut table = LineTable::new();
    table.set_all((1..=count).map(|id| line(id, LineState::Idle)).collect());
    table
}

fn ids(table: &LineTable) -> Vec<u16> {
    (0..table.row_count())
        .map(|row| table.row(row).unwrap().id)
        .collect()
}

fn sort(column: Column, descending: bool) -> SortOrder {
    SortOrder { column, descending }
}

// --------------------------------------------------------------------------
// Events against the table
// --------------------------------------------------------------------------

#[test]
fn a_line_that_changes_is_one_row_changed() {
    let mut table = table(4);
    assert_eq!(ids(&table), [1, 2, 3, 4]);
    let change = table.apply(line(3, LineState::Ringing));
    assert_eq!(change, RowChange::Changed(2));
    assert_eq!(table.row(2).unwrap().state, LineState::Ringing);
    // The same again changes nothing.
    assert_eq!(table.apply(line(3, LineState::Ringing)), RowChange::None);
    assert_eq!(ids(&table), [1, 2, 3, 4]);
}

#[test]
fn a_line_that_is_new_is_one_row_added_where_it_belongs() {
    let mut table = LineTable::new();
    assert_eq!(table.apply(line(5, LineState::Idle)), RowChange::Added(0));
    assert_eq!(table.apply(line(2, LineState::Idle)), RowChange::Added(0));
    assert_eq!(table.apply(line(9, LineState::Idle)), RowChange::Added(2));
    assert_eq!(table.apply(line(4, LineState::Idle)), RowChange::Added(1));
    assert_eq!(ids(&table), [2, 4, 5, 9]);
    assert_eq!(table.len(), 4);
    assert_eq!(table.lowest_free_id(), 1);
    table.apply(line(1, LineState::Idle));
    assert_eq!(table.lowest_free_id(), 3);
}

#[test]
fn a_line_that_is_removed_is_one_row_removed() {
    let mut table = table(4);
    assert_eq!(table.remove(2), RowChange::Removed(1));
    assert_eq!(ids(&table), [1, 3, 4]);
    assert_eq!(table.remove(2), RowChange::None, "it is already gone");
    assert_eq!(table.remove(4), RowChange::Removed(2));
    assert!(table.line(4).is_none());
}

#[test]
fn a_line_that_goes_out_of_the_filter_is_a_row_removed_and_back_in_a_row_added() {
    let mut table = table(4);
    assert_eq!(
        table.set_filter(Filter::Idle),
        RowChange::None,
        "all are idle"
    );
    // Line 2 starts ringing: it is not idle any more.
    assert_eq!(
        table.apply(line(2, LineState::Ringing)),
        RowChange::Removed(1)
    );
    assert_eq!(ids(&table), [1, 3, 4]);
    // It changes again while out of sight: the grid has nothing to do.
    assert_eq!(table.apply(line(2, LineState::Online)), RowChange::None);
    assert_eq!(table.len(), 4, "the line is still in the table");
    assert_eq!(table.line(2).unwrap().state, LineState::Online);
    // And comes back.
    assert_eq!(table.apply(line(2, LineState::Idle)), RowChange::Added(1));
    assert_eq!(ids(&table), [1, 2, 3, 4]);
    // A line that is new and that the filter does not let through.
    assert_eq!(table.apply(line(7, LineState::Disabled)), RowChange::None);
    assert_eq!(table.len(), 5);
    assert_eq!(table.row_of(7), None);
}

#[test]
fn every_filter_lets_through_the_lines_it_names() {
    let mut table = LineTable::new();
    table.set_all(vec![
        line(1, LineState::Disabled),
        line(2, LineState::NotReady),
        line(3, LineState::Idle),
        line(4, LineState::Ringing),
        line(5, LineState::Handshaking),
        line(6, LineState::Online),
        broken_line(7),
    ]);
    let shown = |table: &mut LineTable, filter: Filter| {
        table.set_filter(filter);
        ids(table)
    };
    assert_eq!(shown(&mut table, Filter::All), [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(shown(&mut table, Filter::OnCall), [4, 5, 6]);
    assert_eq!(shown(&mut table, Filter::Idle), [3]);
    assert_eq!(shown(&mut table, Filter::NotReady), [1, 2, 7]);
    assert_eq!(table.filter(), Filter::NotReady);
    // The counts in the title are of every line, whatever is shown.
    assert_eq!(
        table.title(),
        "Modem lines \u{2014} 7 configured, 3 on a call, 1 waiting"
    );
    assert_eq!((table.on_call(), table.waiting()), (3, 1));
    // Changing to a filter that shows the same rows is no change.
    table.set_filter(Filter::Idle);
    assert_eq!(table.set_filter(Filter::Idle), RowChange::None);
    assert_eq!(table.set_filter(Filter::All), RowChange::Reset);
    for (index, filter) in Filter::ALL.into_iter().enumerate() {
        assert_eq!(filter.index(), index);
        assert_eq!(Filter::at(index), Some(filter));
    }
    assert_eq!(Filter::at(4), None);
    let titles: Vec<&str> = Filter::ALL.iter().map(|f| f.title()).collect();
    assert_eq!(
        titles,
        ["All lines", "On a call", "Idle", "Not ready or disabled"]
    );
}

#[test]
fn a_fresh_list_changes_nothing_when_nothing_changed() {
    let mut table = table(3);
    let same: Vec<LineInfo> = (1..=3).map(|id| line(id, LineState::Idle)).collect();
    assert_eq!(table.set_all(same), RowChange::None);
    let mut other: Vec<LineInfo> = (1..=3).map(|id| line(id, LineState::Idle)).collect();
    other[1].state = LineState::NotReady;
    assert_eq!(table.set_all(other), RowChange::Reset);
    assert_eq!(table.set_all(Vec::new()), RowChange::Reset);
    assert!(table.is_empty());
    assert_eq!(table.clear(), RowChange::None);
}

// --------------------------------------------------------------------------
// Selection
// --------------------------------------------------------------------------

#[test]
fn the_selection_is_a_line_and_survives_every_change() {
    let mut table = table(5);
    table.select_row(Some(2));
    assert_eq!(table.selected().unwrap().id, 3);

    // The line itself changes.
    table.apply(line(3, LineState::Ringing));
    assert_eq!(table.selected().unwrap().state, LineState::Ringing);
    assert_eq!(table.selected_row(), Some(2));

    // A line is added above it: the row moves, the line is the same.
    table.remove(1);
    assert_eq!(table.selected_row(), Some(1));
    table.apply(line(1, LineState::Idle));
    assert_eq!(
        (table.selected().unwrap().id, table.selected_row()),
        (3, Some(2))
    );

    // The sort turns round.
    table.set_sort(sort(Column::Number, true));
    assert_eq!(ids(&table), [5, 4, 3, 2, 1]);
    assert_eq!(
        (table.selected().unwrap().id, table.selected_row()),
        (3, Some(2))
    );
    table.set_sort(sort(Column::State, false));
    assert_eq!(table.selected().unwrap().id, 3);

    // The filter hides it: it is still the selected line, with no row.
    table.set_sort(SortOrder::default());
    table.set_filter(Filter::Idle);
    assert_eq!(table.selected().unwrap().id, 3);
    assert_eq!(table.selected_row(), None);
    table.set_filter(Filter::All);
    assert_eq!(table.selected_row(), Some(2));

    // A fresh list that still has the line.
    table.set_all((1..=6).map(|id| line(id, LineState::Idle)).collect());
    assert_eq!(table.selected().unwrap().id, 3);
    // And one that does not: nothing is selected, not another line.
    table.set_all(vec![line(1, LineState::Idle), line(2, LineState::Idle)]);
    assert!(table.selected().is_none());

    table.select(Some(2));
    assert_eq!(table.selected_row(), Some(1));
    table.remove(2);
    assert!(table.selected().is_none(), "a removed line is not selected");
    table.select(Some(99));
    assert!(table.selected().is_none(), "there is no line 99");
    table.select_row(Some(0));
    table.select_row(None);
    assert!(table.selected().is_none());
    table.select_row(Some(50));
    assert!(table.selected().is_none(), "there is no row 50");
}

// --------------------------------------------------------------------------
// Sorting
// --------------------------------------------------------------------------

#[test]
fn clicking_a_header_sorts_by_it_and_clicking_again_turns_the_sort_round() {
    let table = table(3);
    assert_eq!(table.sort(), sort(Column::Number, false));
    assert_eq!(
        table.sort_after_click(Column::Number),
        sort(Column::Number, true)
    );
    assert_eq!(
        table.sort_after_click(Column::State),
        sort(Column::State, false)
    );
    let mut table = table;
    table.set_sort(sort(Column::State, false));
    assert_eq!(
        table.sort_after_click(Column::State),
        sort(Column::State, true)
    );
    assert_eq!(
        table.sort_after_click(Column::Port),
        sort(Column::Port, false)
    );
}

#[test]
fn every_column_sorts_and_lines_that_are_equal_stay_in_id_order() {
    let mut table = LineTable::new();
    let mut lines = vec![
        line(1, LineState::Idle),
        line_with_call(
            2,
            LineState::Online,
            connected(
                call(Direction::Incoming, "5550100", 10),
                12,
                2400,
                "V.22bis",
            ),
        ),
        line(3, LineState::Disabled),
        line_with_call(
            4,
            LineState::Online,
            connected(call(Direction::Outgoing, "2", 60), 63, 1200, "V.22"),
        ),
        line_with_call(5, LineState::Ringing, call(Direction::Incoming, "3", 90)),
        broken_line(6),
        line(10, LineState::Idle),
    ];
    lines[0].counters.bytes_tx = 500;
    lines[3].counters.bytes_rx = 9000;
    lines[6].binding = Some("COM20".to_string());
    lines[1].binding = Some("Standard 56000 bps Modem on COM12".to_string());
    table.set_all(lines);

    let mut sorted = |column: Column, descending: bool| {
        table.set_sort(sort(column, descending));
        ids(&table)
    };
    assert_eq!(sorted(Column::Number, false), [1, 2, 3, 4, 5, 6, 10]);
    assert_eq!(sorted(Column::Number, true), [10, 6, 5, 4, 3, 2, 1]);
    // Port names end in numbers that compare as numbers: CNCB9 is line
    // 10's port and comes after CNCB5.
    assert_eq!(sorted(Column::Port, false), [1, 2, 3, 4, 5, 6, 10]);
    assert_eq!(sorted(Column::Port, true), [10, 6, 5, 4, 3, 2, 1]);
    // By state, in the order of the state images; equal states in id
    // order, whichever way the sort goes.
    assert_eq!(sorted(Column::State, false), [3, 1, 10, 5, 2, 4, 6]);
    assert_eq!(sorted(Column::State, true), [6, 2, 4, 5, 1, 10, 3]);
    // Empty cells come last both ways.
    assert_eq!(sorted(Column::Rate, false), [4, 2, 1, 3, 5, 6, 10]);
    assert_eq!(sorted(Column::Rate, true), [2, 4, 1, 3, 5, 6, 10]);
    assert_eq!(sorted(Column::Modulation, false), [4, 2, 1, 3, 5, 6, 10]);
    assert_eq!(sorted(Column::Modulation, true), [2, 4, 1, 3, 5, 6, 10]);
    // The longest call first is the one that started first.
    assert_eq!(sorted(Column::Duration, true), [2, 4, 5, 1, 3, 6, 10]);
    assert_eq!(sorted(Column::Duration, false), [5, 4, 2, 1, 3, 6, 10]);
    assert_eq!(sorted(Column::Bytes, true), [4, 1, 2, 3, 5, 6, 10]);
    assert_eq!(sorted(Column::Binding, false), [10, 2, 1, 3, 4, 5, 6]);
    assert_eq!(sorted(Column::CallingParty, false), [5, 2, 4, 1, 3, 6, 10]);
    // The columns that are always empty leave the lines in id order.
    for column in [
        Column::Sip,
        Column::HuntGroup,
        Column::Retrains,
        Column::Errors,
    ] {
        assert_eq!(sorted(column, false), [1, 2, 3, 4, 5, 6, 10], "{column:?}");
        assert_eq!(sorted(column, true), [1, 2, 3, 4, 5, 6, 10], "{column:?}");
    }
}

#[test]
fn a_line_that_moves_in_the_sort_is_a_reset_and_one_that_does_not_is_one_row() {
    let mut table = table(4);
    table.set_sort(sort(Column::State, false));
    // Line 2 starts ringing and moves below the idle lines.
    assert_eq!(table.apply(line(2, LineState::Ringing)), RowChange::Reset);
    assert_eq!(ids(&table), [1, 3, 4, 2]);
    // Its call is answered: still last.
    assert_eq!(
        table.apply(line(2, LineState::Online)),
        RowChange::Changed(3)
    );
    // Sorting again by what the rows are already in the order of.
    table.set_sort(SortOrder::default());
    assert_eq!(ids(&table), [1, 2, 3, 4]);
    assert_eq!(table.set_sort(SortOrder::default()), RowChange::None);
}

// --------------------------------------------------------------------------
// Cells
// --------------------------------------------------------------------------

#[test]
fn the_columns_are_the_ones_the_handoff_lists() {
    let described: Vec<(&str, i32, bool)> = Column::ALL
        .iter()
        .map(|c| (c.title(), c.default_width(), c.right_aligned()))
        .collect();
    assert_eq!(
        described,
        [
            ("#", 48, true),
            ("COM port", 70, false),
            ("SIP", 60, false),
            ("Hunt group", 80, false),
            ("State", 100, false),
            ("Calling party", 110, false),
            ("Modulation", 90, false),
            ("TX/RX bps", 90, true),
            ("Duration", 70, true),
            ("Bytes TX/RX", 110, true),
            ("Retr.", 45, true),
            ("Err", 40, true),
            ("Binding", 200, false),
        ]
    );
    for (index, column) in Column::ALL.into_iter().enumerate() {
        assert_eq!(column.index(), index);
        assert_eq!(Column::at(index), Some(column));
    }
    assert_eq!(Column::at(13), None);
}

#[test]
fn every_cell_of_every_kind_of_line() {
    let now = START_MS + 300_000;
    let mut online = line_with_call(
        1,
        LineState::Online,
        connected(
            call(Direction::Incoming, "5550100", 90),
            95,
            2400,
            "V.22bis",
        ),
    );
    online.counters.bytes_tx = 15_200;
    online.counters.bytes_rx = 1_240_400;
    online.binding = Some("Standard 56000 bps Modem on COM11".to_string());
    let dialling = line_with_call(
        2,
        LineState::Handshaking,
        call(Direction::Outgoing, "4", 293),
    );
    let ringing = line_with_call(3, LineState::Ringing, call(Direction::Incoming, "2", 240));
    // Connected, and nothing known about the connection.
    let unknown = line_with_call(
        4,
        LineState::Online,
        connected(call(Direction::Outgoing, "9", 0), 0, 0, ""),
    );
    let mut table = LineTable::new();
    table.set_all(vec![
        online,
        dialling,
        ringing,
        unknown,
        line(5, LineState::Idle),
        line(6, LineState::NotReady),
        line(7, LineState::Disabled),
        broken_line(8),
    ]);
    let row = |row: usize| -> Vec<String> {
        Column::ALL
            .iter()
            .map(|column| table.cell(row, *column, now))
            .collect()
    };
    let e = EMPTY;
    assert_eq!(
        row(0),
        [
            "1",
            "CNCB0",
            e,
            e,
            "Online",
            "5550100",
            "V.22bis",
            "2400/2400",
            "3:25",
            "15,200 / 1,240,400",
            e,
            e,
            "Standard 56000 bps Modem on COM11",
        ]
    );
    // A line that dialled shows whom, with an arrow; before the modems
    // connect the duration counts from the start of the call.
    assert_eq!(
        row(1),
        [
            "2",
            "CNCB1",
            e,
            e,
            "Handshaking",
            "\u{2192} 4",
            e,
            e,
            "0:07",
            "0 / 0",
            e,
            e,
            e
        ]
    );
    assert_eq!(
        row(2),
        [
            "3", "CNCB2", e, e, "Ringing", "2", e, e, "1:00", "0 / 0", e, e, e
        ]
    );
    assert_eq!(
        row(3),
        [
            "4",
            "CNCB3",
            e,
            e,
            "Online",
            "\u{2192} 9",
            e,
            e,
            "5:00",
            "0 / 0",
            e,
            e,
            e
        ]
    );
    assert_eq!(
        row(4),
        [
            "5",
            "CNCB4",
            e,
            e,
            "Idle (AT)",
            e,
            e,
            e,
            e,
            "0 / 0",
            e,
            e,
            e
        ]
    );
    assert_eq!(row(5)[4], "Not ready");
    assert_eq!(row(6)[4], "Disabled");
    assert_eq!(row(7)[4], "Port error");
    // A row the grid does not have shows nothing.
    assert_eq!(table.cell(99, Column::State, now), "");
    assert!(table.look(99).is_none());

    // How each state is drawn: its image, and its colour.
    let looks: Vec<(usize, StateColour)> = (0..8)
        .map(|row| {
            let look = table.look(row).unwrap();
            (look.image(), look.colour())
        })
        .collect();
    use StateColour::{Default as Plain, Green, Red};
    assert_eq!(
        looks,
        [
            (5, Green),
            (4, Plain),
            (3, Plain),
            (5, Green),
            (2, Plain),
            (1, Plain),
            (0, Plain),
            (6, Red),
        ]
    );
    assert_eq!(StateLook::ALL.len(), 7);
}

#[test]
fn durations_tick_without_an_event_from_the_service() {
    let mut table = LineTable::new();
    table.set_all(vec![
        line(1, LineState::Idle),
        line_with_call(
            2,
            LineState::Online,
            connected(call(Direction::Incoming, "1", 0), 2, 2400, "V.22bis"),
        ),
        line_with_call(3, LineState::Ringing, call(Direction::Incoming, "4", 100)),
    ]);
    // The rows to draw again each second are the ones with a call.
    assert_eq!(table.ticking_rows(), [1, 2]);
    let at = |second: u64| table.cell(1, Column::Duration, START_MS + second * 1000);
    assert_eq!(at(2), "0:00");
    assert_eq!(at(3), "0:01");
    assert_eq!(at(61), "0:59");
    assert_eq!(at(62), "1:00");
    assert_eq!(at(3602), "1:00:00");
    // A clock that is behind the service's does not go below nothing.
    assert_eq!(at(0), "0:00");
    assert_eq!(table.cell(0, Column::Duration, START_MS), EMPTY);
    table.set_filter(Filter::Idle);
    assert!(table.ticking_rows().is_empty());
}

// --------------------------------------------------------------------------
// Any sequence of events
// --------------------------------------------------------------------------

/// Something that happens to the table.
#[derive(Clone, Debug)]
enum Step {
    /// A `line.state` event: a line, in a state, with or without a call.
    Event(u16, usize, bool, u64),
    /// The line is removed from the service.
    Remove(u16),
    Filter(usize),
    Sort(usize, bool),
    Select(usize),
    /// A fresh `lines.list`.
    Relist,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (1u16..7, 0usize..6, any::<bool>(), 0u64..5000)
            .prop_map(|(id, state, call, bytes)| Step::Event(id, state, call, bytes)),
        1 => (1u16..7).prop_map(Step::Remove),
        1 => (0usize..4).prop_map(Step::Filter),
        1 => (0usize..13, any::<bool>()).prop_map(|(column, down)| Step::Sort(column, down)),
        1 => (0usize..7).prop_map(Step::Select),
        1 => Just(Step::Relist),
    ]
}

fn event_line(id: u16, state: usize, with_call: bool, bytes: u64) -> LineInfo {
    let state = LineState::ALL[state];
    let mut info = line(id, state);
    info.counters.bytes_tx = bytes;
    let in_call = matches!(
        state,
        LineState::Ringing | LineState::Handshaking | LineState::Online
    );
    if in_call {
        let mut started = call(Direction::Incoming, "5550100", bytes);
        if state == LineState::Online && with_call {
            started = connected(started, bytes + 3, 2400, "V.22bis");
        }
        info.call = Some(started);
    }
    info
}

/// What the grid shows: the id and the State cell of each row.
fn picture(table: &LineTable) -> Vec<(u16, String)> {
    (0..table.row_count())
        .map(|row| {
            (
                table.row(row).unwrap().id,
                table.cell(row, Column::State, START_MS) + &table.cell(row, Column::Bytes, 0),
            )
        })
        .collect()
}

proptest! {
    /// Whatever happens, in whatever order: the table is what a fresh
    /// `lines.list` would give, the rows are that list filtered and
    /// sorted, a grid that only does what each change says shows exactly
    /// those rows, and the selection is still the same line.
    #[test]
    fn any_sequence_of_events_leaves_the_table_equal_to_a_fresh_list(
        steps in proptest::collection::vec(step(), 1..80)
    ) {
        let mut table = LineTable::new();
        // What the service has: the last report of each line.
        let mut service: std::collections::BTreeMap<u16, LineInfo> = Default::default();
        // What a grid shows that redraws only what it is told to.
        let mut grid: Vec<(u16, String)> = Vec::new();

        for step in steps {
            let selected_before = table.selected().map(|line| line.id);
            let change = match step.clone() {
                Step::Event(id, state, with_call, bytes) => {
                    let info = event_line(id, state, with_call, bytes);
                    service.insert(id, info.clone());
                    table.apply(info)
                }
                Step::Remove(id) => {
                    service.remove(&id);
                    table.remove(id)
                }
                Step::Filter(index) => table.set_filter(Filter::ALL[index]),
                Step::Sort(column, descending) => table.set_sort(SortOrder {
                    column: Column::ALL[column],
                    descending,
                }),
                Step::Select(row) => {
                    table.select_row(Some(row));
                    RowChange::None
                }
                Step::Relist => table.set_all(service.values().cloned().collect()),
            };

            // The grid does exactly what the change says.
            let now = picture(&table);
            match change {
                RowChange::None => {}
                RowChange::Changed(row) => grid[row] = now[row].clone(),
                RowChange::Added(row) => grid.insert(row, now[row].clone()),
                RowChange::Removed(row) => {
                    grid.remove(row);
                }
                RowChange::Reset => grid = now.clone(),
            }
            prop_assert_eq!(&grid, &now, "after {:?} reported as {:?}", step, change);

            // The table holds what the service has.
            let held: Vec<&LineInfo> = table.lines().collect();
            let truth: Vec<&LineInfo> = service.values().collect();
            prop_assert_eq!(held, truth);

            // The rows are a fresh list, filtered and sorted.
            let mut fresh = LineTable::new();
            fresh.set_filter(table.filter());
            fresh.set_sort(table.sort());
            fresh.set_all(service.values().cloned().collect());
            prop_assert_eq!(ids(&table), ids(&fresh));

            // The selection is the same line, unless that line went, or
            // this step chose another.
            if !matches!(step, Step::Select(_)) {
                let expected = selected_before.filter(|id| service.contains_key(id));
                prop_assert_eq!(table.selected().map(|line| line.id), expected);
            }
            if let Some(row) = table.selected_row() {
                prop_assert_eq!(table.row(row).map(|line| line.id), table.selected().map(|l| l.id));
            }
        }
    }
}
