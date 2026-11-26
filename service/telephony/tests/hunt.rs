// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Routing and hunting, case by case and as properties.

use std::collections::BTreeMap;

use config::{HuntGroup, Route, Strategy as HuntStrategy, Target};
use ctlproto::LineState;
use proptest::prelude::*;
use telephony::{Decision, Hunter};

use LineState::{Disabled, Handshaking, Idle, NotReady, Online, Ringing};

type States = BTreeMap<u16, LineState>;

fn states(list: &[(u16, LineState)]) -> States {
    list.iter().copied().collect()
}

fn route(hunter: &mut Hunter, called: &str, states: &States) -> Decision {
    hunter.route(called, &|line| states.get(&line).copied())
}

fn group(name: &str, pilot: &str, lines: &[u16], strategy: HuntStrategy) -> HuntGroup {
    HuntGroup {
        name: name.to_string(),
        pilot: pilot.to_string(),
        lines: lines.to_vec(),
        strategy,
    }
}

fn to_line(called: &str, line: u16) -> Route {
    Route {
        called: called.to_string(),
        target: Target::Line(line),
    }
}

fn to_group(called: &str, name: &str) -> Route {
    Route {
        called: called.to_string(),
        target: Target::Group(name.to_string()),
    }
}

#[test]
fn a_number_nothing_is_called_by_is_not_found() {
    let mut hunter = Hunter::new(
        &[to_line("5552000", 1)],
        &[group("dialin", "5551000", &[1], HuntStrategy::FirstIdle)],
    );
    let all = states(&[(1, Idle)]);
    for called in ["", "5551", "55510000", "5552", "dialin", "1"] {
        assert_eq!(
            route(&mut hunter, called, &all),
            Decision::NotFound,
            "{called}"
        );
    }
    assert_eq!(Decision::NotFound.status(), Some(404));
    assert_eq!(
        Hunter::default().route("5551000", &|_| Some(Idle)),
        Decision::NotFound
    );
}

#[test]
fn a_route_to_a_line() {
    let mut hunter = Hunter::new(&[to_line("5552*", 2)], &[]);
    for (state, decision) in [
        (Some(Idle), Decision::Line(2)),
        (Some(Ringing), Decision::Busy),
        (Some(Handshaking), Decision::Busy),
        (Some(Online), Decision::Busy),
        (Some(NotReady), Decision::Unavailable),
        (Some(Disabled), Decision::Unavailable),
        // The line was removed after the route was read.
        (None, Decision::Unavailable),
    ] {
        assert_eq!(hunter.route("5552999", &|_| state), decision, "{state:?}");
    }
    assert_eq!(Decision::Busy.status(), Some(486));
    assert_eq!(Decision::Unavailable.status(), Some(480));
    assert_eq!(Decision::Line(2).status(), None);
}

#[test]
fn routes_are_tried_in_order_and_before_the_pilots() {
    let groups = [group("dialin", "5551000", &[3], HuntStrategy::FirstIdle)];
    let mut hunter = Hunter::new(
        &[
            to_line("5551234", 1),
            to_line("555*", 2),
            to_line("5551000", 4),
        ],
        &groups,
    );
    let all = states(&[(1, Idle), (2, Idle), (3, Idle), (4, Idle)]);
    assert_eq!(route(&mut hunter, "5551234", &all), Decision::Line(1));
    // The prefix route is before the exact one and before the pilot.
    assert_eq!(route(&mut hunter, "5551000", &all), Decision::Line(2));
    assert_eq!(route(&mut hunter, "5559", &all), Decision::Line(2));

    // The first route that matches decides, busy or not: the call does not
    // fall through to the next.
    let busy = states(&[(1, Idle), (2, Online), (3, Idle), (4, Idle)]);
    assert_eq!(route(&mut hunter, "5551000", &busy), Decision::Busy);

    // With no route in the way, the pilot.
    let mut hunter = Hunter::new(&[to_line("5551234", 1)], &groups);
    assert_eq!(route(&mut hunter, "5551000", &all), Decision::Line(3));
}

#[test]
fn a_route_to_a_group_hunts_it() {
    let mut hunter = Hunter::new(
        &[to_group("9*", "dialin")],
        &[group("dialin", "5551000", &[1, 2], HuntStrategy::FirstIdle)],
    );
    let all = states(&[(1, Online), (2, Idle)]);
    assert_eq!(route(&mut hunter, "911", &all), Decision::Line(2));
    assert_eq!(route(&mut hunter, "5551000", &all), Decision::Line(2));

    // The group was removed from under the route.
    hunter.reconfigure(&[to_group("9*", "dialin")], &[]);
    assert_eq!(route(&mut hunter, "911", &all), Decision::Unavailable);
    assert_eq!(route(&mut hunter, "5551000", &all), Decision::NotFound);
}

#[test]
fn a_call_for_a_registered_line_goes_to_that_line_only() {
    let hunter = Hunter::new(
        &[to_line("*", 9)],
        &[group("dialin", "7002", &[8, 9], HuntStrategy::FirstIdle)],
    );
    let all = states(&[(2, Idle), (8, Idle), (9, Idle)]);
    let state = |line: u16| all.get(&line).copied();
    assert_eq!(hunter.route_to_line(2, &state), Decision::Line(2));
    assert_eq!(hunter.route_to_line(2, &|_| Some(Online)), Decision::Busy);
    assert_eq!(
        hunter.route_to_line(2, &|_| Some(NotReady)),
        Decision::Unavailable
    );
    assert_eq!(hunter.route_to_line(5, &state), Decision::Unavailable);
}

#[test]
fn first_idle_takes_the_first_idle_line_in_list_order() {
    let mut hunter = Hunter::new(
        &[],
        &[group("g", "100", &[3, 1, 2], HuntStrategy::FirstIdle)],
    );
    let all = states(&[(1, Idle), (2, Idle), (3, Idle)]);
    // Every time, while the first stays idle.
    for _ in 0..3 {
        assert_eq!(route(&mut hunter, "100", &all), Decision::Line(3));
    }
    let some = states(&[(1, Idle), (2, Idle), (3, Online)]);
    assert_eq!(route(&mut hunter, "100", &some), Decision::Line(1));
    let last = states(&[(1, NotReady), (2, Idle), (3, Online)]);
    assert_eq!(route(&mut hunter, "100", &last), Decision::Line(2));
}

#[test]
fn round_robin_starts_after_the_line_that_took_the_last_call() {
    let mut hunter = Hunter::new(
        &[],
        &[
            group("g", "100", &[1, 2, 3], HuntStrategy::RoundRobin),
            group("h", "200", &[1, 2, 3], HuntStrategy::RoundRobin),
        ],
    );
    let all = states(&[(1, Idle), (2, Idle), (3, Idle)]);
    let picks: Vec<Decision> = (0..7).map(|_| route(&mut hunter, "100", &all)).collect();
    assert_eq!(
        picks,
        [1, 2, 3, 1, 2, 3, 1].map(Decision::Line),
        "it goes round"
    );
    // Each group remembers for itself.
    assert_eq!(route(&mut hunter, "200", &all), Decision::Line(1));

    // The next in turn is busy: the one after it.
    let busy = states(&[(1, Idle), (2, Online), (3, Idle)]);
    assert_eq!(route(&mut hunter, "100", &busy), Decision::Line(3));
    // And from there on.
    assert_eq!(route(&mut hunter, "100", &all), Decision::Line(1));

    // A call that is turned away does not move the turn.
    let none = states(&[(1, Online), (2, Online), (3, Online)]);
    assert_eq!(route(&mut hunter, "100", &none), Decision::Busy);
    assert_eq!(route(&mut hunter, "100", &all), Decision::Line(2));

    // The memory survives a change that keeps the group, and goes with a
    // group that is removed.
    let groups = [group("g", "100", &[1, 2, 3, 4], HuntStrategy::RoundRobin)];
    hunter.reconfigure(&[], &groups);
    assert_eq!(hunter.groups(), groups);
    let four = states(&[(1, Idle), (2, Idle), (3, Idle), (4, Idle)]);
    assert_eq!(route(&mut hunter, "100", &four), Decision::Line(3));
    hunter.reconfigure(&[], &[]);
    hunter.reconfigure(&[], &groups);
    assert_eq!(route(&mut hunter, "100", &four), Decision::Line(1));

    // The line that took the last call was taken out of the group: from
    // the start.
    assert_eq!(route(&mut hunter, "100", &four), Decision::Line(2));
    hunter.reconfigure(
        &[],
        &[group("g", "100", &[1, 3, 4], HuntStrategy::RoundRobin)],
    );
    assert_eq!(route(&mut hunter, "100", &four), Decision::Line(1));
}

#[test]
fn least_recent_takes_the_line_idle_longest() {
    let mut hunter = Hunter::new(
        &[],
        &[group("g", "100", &[1, 2, 3], HuntStrategy::LeastRecent)],
    );
    let all = states(&[(1, Idle), (2, Idle), (3, Idle)]);
    // None has had a call: list order.
    assert_eq!(route(&mut hunter, "100", &all), Decision::Line(1));

    hunter.released(1, 5000);
    hunter.released(3, 1000);
    // 2 has never had a call, then 3, then 1.
    assert_eq!(route(&mut hunter, "100", &all), Decision::Line(2));
    hunter.released(2, 9000);
    assert_eq!(route(&mut hunter, "100", &all), Decision::Line(3));
    hunter.released(3, 9500);
    assert_eq!(route(&mut hunter, "100", &all), Decision::Line(1));

    // Only among those that are idle.
    let some = states(&[(1, Online), (2, Idle), (3, Idle)]);
    assert_eq!(route(&mut hunter, "100", &some), Decision::Line(2));
    // Released at the same moment: list order.
    hunter.released(2, 9500);
    assert_eq!(route(&mut hunter, "100", &some), Decision::Line(2));
}

#[test]
fn a_group_with_no_idle_line_is_busy_or_unavailable() {
    for strategy in HuntStrategy::ALL {
        let mut hunter = Hunter::new(&[], &[group("g", "100", &[1, 2, 3], strategy)]);
        // Every member in a call.
        let busy = states(&[(1, Online), (2, Ringing), (3, Handshaking)]);
        assert_eq!(route(&mut hunter, "100", &busy), Decision::Busy);
        // No member ready.
        let down = states(&[(1, Disabled), (2, NotReady)]);
        assert_eq!(route(&mut hunter, "100", &down), Decision::Unavailable);
        // Some in a call and the rest not ready: the caller may try again
        // in a moment, so busy.
        let mixed = states(&[(1, Disabled), (2, Online), (3, NotReady)]);
        assert_eq!(route(&mut hunter, "100", &mixed), Decision::Busy);
        // A group with no lines.
        let mut empty = Hunter::new(&[], &[group("g", "100", &[], strategy)]);
        assert_eq!(route(&mut empty, "100", &busy), Decision::Unavailable);
    }
}

// ---------------------------------------------------------------------
// Properties.

fn line_state() -> impl Strategy<Value = LineState> {
    prop::sample::select(vec![Disabled, NotReady, Idle, Ringing, Handshaking, Online])
}

fn hunt_strategy() -> impl Strategy<Value = HuntStrategy> {
    prop::sample::select(HuntStrategy::ALL.to_vec())
}

/// Lines 1 to 8 in any states, some of them missing; a few groups over
/// them; a few routes to lines and groups, some of which are not there.
fn world() -> impl Strategy<Value = (States, Vec<HuntGroup>, Vec<Route>)> {
    let lines = prop::collection::btree_map(1u16..=8, line_state(), 0..=8);
    let groups = prop::collection::vec(
        (prop::collection::vec(1u16..=9, 0..6), hunt_strategy()),
        0..4,
    )
    .prop_map(|groups| {
        groups
            .into_iter()
            .enumerate()
            .map(|(n, (mut lines, strategy))| {
                lines.dedup();
                HuntGroup {
                    name: format!("g{n}"),
                    pilot: format!("10{n}"),
                    lines,
                    strategy,
                }
            })
            .collect::<Vec<_>>()
    });
    let routes = prop::collection::vec(
        (
            prop_oneof![Just("*".to_string()), "[12][0-9]{0,2}\\*?"],
            prop_oneof![
                (1u16..=9).prop_map(Target::Line),
                (0usize..5).prop_map(|n| Target::Group(format!("g{n}"))),
            ],
        ),
        0..4,
    )
    .prop_map(|routes| {
        routes
            .into_iter()
            .map(|(called, target)| Route { called, target })
            .collect::<Vec<_>>()
    });
    (lines, groups, routes)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// A call is never offered to a line that is not idle, whatever the
    /// routes, the groups and the calls before it.
    #[test]
    fn a_call_is_only_ever_offered_to_an_idle_line(
        (states, groups, routes) in world(),
        calls in prop::collection::vec(("[12][0-9]{0,3}", any::<u8>()), 1..20),
    ) {
        let mut hunter = Hunter::new(&routes, &groups);
        for (called, release) in calls {
            let decision = route(&mut hunter, &called, &states);
            if let Decision::Line(line) = decision {
                prop_assert_eq!(states.get(&line), Some(&Idle), "{} went to line {}", called, line);
                hunter.released(line, u64::from(release));
            }
            // The same for a call that names its line.
            let line = u16::from(release % 10);
            match hunter.route_to_line(line, &|line| states.get(&line).copied()) {
                Decision::Line(chosen) => {
                    prop_assert_eq!(chosen, line);
                    prop_assert_eq!(states.get(&line), Some(&Idle));
                }
                Decision::NotFound => prop_assert!(false, "a line is never not found"),
                Decision::Busy | Decision::Unavailable => {
                    prop_assert_ne!(states.get(&line), Some(&Idle));
                }
            }
        }
    }

    /// A group turns a call away only when it has no idle line: busy when
    /// a member is in a call, unavailable when none is.
    #[test]
    fn a_group_refuses_only_when_no_member_is_idle(
        states in prop::collection::btree_map(1u16..=8, line_state(), 0..=8),
        members in prop::collection::btree_set(1u16..=9, 0..8),
        strategy in hunt_strategy(),
    ) {
        let members: Vec<u16> = members.into_iter().collect();
        let mut hunter = Hunter::new(&[], &[group("g", "100", &members, strategy)]);
        let of = |line: &u16| states.get(line).copied();
        let any_idle = members.iter().any(|l| of(l) == Some(Idle));
        let any_in_call = members
            .iter()
            .any(|l| matches!(of(l), Some(Ringing | Handshaking | Online)));
        match route(&mut hunter, "100", &states) {
            Decision::Line(line) => {
                prop_assert!(members.contains(&line));
                prop_assert_eq!(of(&line), Some(Idle));
            }
            Decision::Busy => prop_assert!(!any_idle && any_in_call),
            Decision::Unavailable => prop_assert!(!any_idle && !any_in_call),
            Decision::NotFound => prop_assert!(false, "the pilot is the group's"),
        }
    }

    /// Round-robin visits every ready member: as many calls as there are
    /// idle members go to each of them once, wherever the turn stood.
    #[test]
    fn round_robin_visits_every_ready_member(
        mut states in prop::collection::btree_map(1u16..=8, line_state(), 0..=8),
        members in prop::collection::btree_set(1u16..=8, 1..8),
        ready_one in any::<prop::sample::Index>(),
        before in 0usize..10,
    ) {
        let members: Vec<u16> = members.into_iter().collect();
        // One member at least is ready.
        states.insert(members[ready_one.index(members.len())], Idle);
        let ready: Vec<u16> = members
            .iter()
            .copied()
            .filter(|l| states.get(l) == Some(&Idle))
            .collect();
        let mut hunter = Hunter::new(&[], &[group("g", "100", &members, HuntStrategy::RoundRobin)]);
        for _ in 0..before {
            route(&mut hunter, "100", &states);
        }
        let mut visited: Vec<u16> = (0..ready.len())
            .map(|_| match route(&mut hunter, "100", &states) {
                Decision::Line(line) => line,
                other => panic!("turned away with a ready member: {other:?}"),
            })
            .collect();
        // In turn, which is list order from wherever it stood.
        let start = ready.iter().position(|l| *l == visited[0]).unwrap();
        let mut in_turn = ready.clone();
        in_turn.rotate_left(start);
        prop_assert_eq!(&visited, &in_turn);
        visited.sort_unstable();
        prop_assert_eq!(visited, ready);
    }

    /// Least-recent takes an idle member that no idle member has been idle
    /// longer than.
    #[test]
    fn least_recent_takes_a_member_idle_longest(
        states in prop::collection::btree_map(1u16..=8, line_state(), 1..=8),
        members in prop::collection::btree_set(1u16..=8, 1..8),
        releases in prop::collection::btree_map(1u16..=8, 0u64..1000, 0..8),
    ) {
        let members: Vec<u16> = members.into_iter().collect();
        let mut hunter = Hunter::new(&[], &[group("g", "100", &members, HuntStrategy::LeastRecent)]);
        for (line, at) in &releases {
            hunter.released(*line, *at);
        }
        if let Decision::Line(chosen) = route(&mut hunter, "100", &states) {
            for line in members.iter().filter(|l| states.get(l) == Some(&Idle)) {
                prop_assert!(
                    releases.get(&chosen) <= releases.get(line),
                    "line {} was idle longer than line {}", line, chosen
                );
            }
        }
    }
}
