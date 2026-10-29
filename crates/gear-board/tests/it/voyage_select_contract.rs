//! T-350: the board can be built for any voyage of the project, and shows
//! that voyage only. A landed voyage keeps its data: its lanes read "retired
//! at the landing", and its end is the last merge of its tasks.

use gear_board::{Options, ProjectBoard};
use gear_model::{AttemptState, SeatRole, SeatState, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, NewDecision, Store, TaskRow, VoyageRow,
};

const NOW: i64 = 1_800_000_000;

/// M1's task was merged here: the voyage's end.
const MERGED: i64 = NOW - 6000;

/// A project with two voyages: M1 landed with a retired lane `old` that
/// merged T-1, and M2 sailing on the lane `hull`.
fn world() -> Store {
    world_with_m2(true)
}

/// The same project with only the landed M1: no voyage sails, so the board
/// shows the landed one.
fn world_landed() -> Store {
    world_with_m2(false)
}

fn world_with_m2(sailing: bool) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            let old = testing::seed_active_seat(tx, "p", "old", SeatRole::Lane, "w1:p3", "pi", NOW)?;
            tx.set_seat_state(&old.id, SeatState::Retired, None, NOW)?;
            voyage(tx, "M1", NOW - 7200)?;
            // M1: one task it merged, and one it cut.
            task(tx, "T-1", 1, "M1", "old", TaskState::Done)?;
            task(tx, "T-4", 4, "M1", "old", TaskState::Cancelled)?;
            decision(tx, "d1", "M1", "the old lane lands with M1")?;
            if sailing {
                voyage(tx, "M2", NOW - 3600)?;
                // M2: one done, one ready.
                task(tx, "T-2", 2, "M2", "hull", TaskState::Done)?;
                task(tx, "T-3", 3, "M2", "hull", TaskState::Ready)?;
                decision(tx, "d2", "M2", "the hull lane keeps the work")?;
            }
            Ok(())
        })
        .unwrap();
    store
        .write(|tx| {
            attempt(tx, "T-1", "p:old", NOW - 6600, MERGED)?;
            if sailing {
                attempt(tx, "T-2", "p:hull", NOW - 3000, NOW - 2400)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

fn voyage(tx: &gear_store::Tx<'_>, milestone: &str, departure: i64) -> gear_model::GearResult<()> {
    tx.upsert_voyage(&VoyageRow {
        project: "p".into(),
        milestone: milestone.into(),
        title: format!("voyage {milestone}"),
        done: Some("it is delivered".into()),
        bar: None,
        eta: Some("8h".into()),
        departure: Some(departure),
        departure_tasks: Some(1),
        departure_estimate_min: Some(30),
        updated: departure,
    })
}

fn task(
    tx: &gear_store::Tx<'_>,
    id: &str,
    rank: i64,
    milestone: &str,
    lane: &str,
    state: TaskState,
) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: Some(milestone.into()),
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(30),
        issue: None,
        updated: NOW,
    })
}

fn decision(
    tx: &gear_store::Tx<'_>,
    key: &str,
    voyage: &str,
    what: &str,
) -> gear_model::GearResult<()> {
    tx.insert_decision_if_new(&NewDecision {
        project: "p".into(),
        key: key.into(),
        voyage: voyage.into(),
        at: "2026-10-27 11:40".into(),
        kind: "scope".into(),
        what: what.into(),
        why: None,
        alt: None,
        changes: None,
        star: false,
        source: "plan".into(),
        line: None,
        plan_version: None,
    })?;
    Ok(())
}

/// A finished task: it started, was handed in and was merged at `merged`.
fn attempt(
    tx: &gear_store::Tx<'_>,
    task: &str,
    seat: &str,
    started: i64,
    merged: i64,
) -> gear_model::GearResult<()> {
    tx.insert_attempt(&AttemptRow {
        project: "p".into(),
        task: task.into(),
        n: 1,
        seat: seat.into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state: AttemptState::Accepted,
        candidate: Some("abc".into()),
        report: None,
        safe_point: None,
        started,
        updated: started,
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
    })?;
    tx.set_attempt_time("p", task, 1, AttemptStep::Started, started)?;
    tx.set_attempt_time("p", task, 1, AttemptStep::HandedIn, merged)?;
    tx.set_attempt_time("p", task, 1, AttemptStep::Merged, merged)?;
    Ok(())
}

fn board(store: &mut Store, milestone: Option<&str>) -> ProjectBoard {
    let options = Options {
        project: Some("p".into()),
        milestone: milestone.map(str::to_string),
        ..Default::default()
    };
    gear_board::build(store, &options, NOW)
        .unwrap()
        .projects
        .into_iter()
        .next()
        .expect("the project")
}

#[test]
fn the_board_built_without_a_milestone_shows_the_sailing_voyage_only() {
    let mut store = world();
    let plan = board(&mut store, None);

    // The two M2 tasks: no task of the landed M1, and not the one it cut.
    assert_eq!(plan.totals.tasks, 2, "the tasks of M2: {:?}", plan.totals);
    let listed: Vec<&str> = plan
        .lanes
        .iter()
        .flat_map(|lane| lane.tasks.iter())
        .filter(|task| task.actual_min.unwrap_or(0) > 0)
        .map(|task| task.id.as_str())
        .collect();
    assert!(!listed.contains(&"T-1"), "no task of the landed voyage: {listed:?}");
    let graphed: Vec<&str> = plan.graph.iter().map(|task| task.id.as_str()).collect();
    assert!(!graphed.contains(&"T-1"), "no task of the landed voyage: {graphed:?}");
    // The lanes are the sailing voyage's, the retired one included only when
    // it is a lane of the shown voyage.
    let names: Vec<&str> = plan.lanes.iter().map(|lane| lane.lane.as_str()).collect();
    assert_eq!(names, vec!["hull"], "the lanes of M2: {names:?}");

    // Every voyage of the project, the newest departure first.
    let list: Vec<(&str, bool)> =
        plan.voyage_list.iter().map(|e| (e.milestone.as_str(), e.sailing)).collect();
    assert_eq!(list, vec![("M2", true), ("M1", false)], "the voyage list: {list:?}");
    let m1 = &plan.voyage_list[1];
    assert_eq!(m1.departure, Some(NOW - 7200));
    assert_eq!(m1.ended, Some(MERGED), "M1 ended at its last merge");
    assert_eq!(plan.voyage_list[0].ended, None, "a sailing voyage has not ended");

    // The one the board shows is the first of the list.
    let shown = plan.shown.expect("a measured voyage");
    assert_eq!(shown.milestone, "M2");
    assert!(shown.sailing);
}

#[test]
fn the_board_built_for_a_landed_voyage_keeps_its_data() {
    let mut store = world();
    let plan = board(&mut store, Some("M1"));

    // Its tasks only, and the one it cut is not counted (T-415).
    assert_eq!(plan.totals.tasks, 1, "the tasks of M1: {:?}", plan.totals);
    assert_eq!(plan.totals.done, 1);

    // Its lanes, retired ones included, retired at the landing.
    let names: Vec<&str> = plan.lanes.iter().map(|lane| lane.lane.as_str()).collect();
    assert_eq!(names, vec!["old"], "the lanes of M1: {names:?}");
    let lane = &plan.lanes[0];
    assert_eq!(lane.idle_text.as_deref(), Some("retired at the landing"));
    let ids: Vec<&str> = lane.tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["T-1", "T-4"],
        "the lane works M1 only, and the task it cut stays in its queue (T-415)"
    );

    // The graph and the time view follow the shown tasks.
    let graphed: Vec<&str> = plan.graph.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(graphed, vec!["T-1"], "the graph of M1: {graphed:?}");

    // The voyage it shows ended at T-1's merge.
    let shown = plan.shown.expect("the measured voyage");
    assert_eq!(shown.milestone, "M1");
    assert!(!shown.sailing);
    assert_eq!(shown.ended, Some(MERGED));
    assert_eq!(plan.voyage_list.first().map(|e| e.milestone.as_str()), Some("M2"));

    // The decisions are M1's only.
    let whats: Vec<&str> = plan.decisions.iter().map(|d| d.what.as_str()).collect();
    assert_eq!(whats, vec!["the old lane lands with M1"], "the decisions of M1: {whats:?}");
}

/// The text of the graph row a task's branch leaves `dev` on: the fork row's
/// `starts +<h>h<mm>m`, or `None` for a row that is not one.
fn starts_of(plan: &ProjectBoard) -> Vec<String> {
    gear_board::frame::task_rows(plan, NOW, 160)
        .into_iter()
        .map(|row| row.text)
        .filter(|text| text.contains("starts +"))
        .collect()
}

/// T-362 (F1 of the review of attempt 1): the Tasks view measures its rows from
/// the shown voyage's own departure, so a landed voyage picked with `[` reads
/// its tasks' starts there — M1 departed at NOW − 7200 and its task started at
/// NOW − 6600, so its branch leaves dev at +0h10m, not 50 minutes before it.
#[test]
fn the_tasks_view_measures_a_landed_voyage_from_its_own_departure() {
    let mut store = world();
    let plan = board(&mut store, Some("M1"));
    assert_eq!(
        plan.shown.as_ref().and_then(|voyage| voyage.departure),
        Some(NOW - 7200),
        "the shown voyage is M1"
    );
    assert_eq!(starts_of(&plan), vec!["       starts +0h10m"], "M1's rows");
}

/// The same for a project whose only voyage has landed: the board shows it, and
/// the Tasks view still measures from its departure, not from `now`.
#[test]
fn a_landed_only_project_measures_the_tasks_view_from_its_departure() {
    let mut store = world_landed();
    let plan = board(&mut store, None);
    assert_eq!(plan.shown.as_ref().map(|voyage| voyage.milestone.as_str()), Some("M1"));
    assert!(
        plan.shown.as_ref().is_some_and(|voyage| !voyage.sailing),
        "the landed voyage is the one shown"
    );
    assert_eq!(starts_of(&plan), vec!["       starts +0h10m"], "M1's rows");
}
