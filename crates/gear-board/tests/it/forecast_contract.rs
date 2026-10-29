//! T-349: the board plays each lane's queue forward — when the lane ends and
//! how long it sits idle — and measures how long a hand-in needs to reach the
//! integration branch.
//!
//! The pure forecast is unit-tested in its module against the prototype's M5
//! jobs; here it is measured against the store it reads.

use gear_board::forecast::review_merge_min;
use gear_board::ProjectBoard;
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, Store, TaskRow, VoyageRow};

const NOW: i64 = 1_800_000_000;

/// A voyage of two merged tasks and one open one on the hull lane.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            testing::seed_active_seat(tx, "p", "keel", SeatRole::Lane, "w1:p3", "pi", NOW)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M5".into(),
                title: "the board says when each lane ends".into(),
                done: Some("every lane's end is on the board".into()),
                bar: Some("an error in normal use blocks the task".into()),
                eta: Some("16h".into()),
                departure: Some(NOW - 3_600),
                departure_tasks: Some(3),
                departure_estimate_min: Some(95),
                updated: NOW,
            })?;
            task(tx, "T-1", 1, "hull", TaskState::Done, 20, &[])?;
            task(tx, "T-2", 2, "hull", TaskState::Done, 30, &[])?;
            task(tx, "T-3", 3, "keel", TaskState::Ready, 15, &[])?;
            task(tx, "T-4", 4, "hull", TaskState::Ready, 25, &["T-3"])?;
            Ok(())
        })
        .unwrap();
    // T-1 waited 10 minutes for its merge, T-2 twenty.
    store
        .write(|tx| {
            merged(tx, "T-1", 1, Some(NOW - 1_200), NOW - 600)?;
            merged(tx, "T-2", 1, Some(NOW - 2_400), NOW - 1_200)
        })
        .unwrap();
    store
}

fn task(
    tx: &gear_store::Tx<'_>,
    id: &str,
    rank: i64,
    lane: &str,
    state: TaskState,
    estimate: i64,
    deps: &[&str],
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
        milestone: Some("M5".into()),
        required: false,
        contract_version: 1,
        deps: deps.iter().map(|dep| dep.to_string()).collect(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(estimate),
        issue: None,
        updated: NOW,
    })
}

/// An attempt of the task that was handed in — and, when `merged` is given,
/// merged that many seconds later. `merged` is None for an attempt that never
/// reached the branch.
fn merged(
    tx: &gear_store::Tx<'_>,
    task: &str,
    n: i64,
    handed_in: Option<i64>,
    merged: i64,
) -> gear_model::GearResult<()> {
    tx.insert_attempt(&AttemptRow {
        project: "p".into(),
        task: task.into(),
        n,
        seat: "p:hull".into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state: AttemptState::Accepted,
        candidate: Some("abc".into()),
        report: None,
        safe_point: None,
        started: NOW - 3_600,
        updated: NOW,
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
    })?;
    let handed_in = handed_in.unwrap_or(merged - 600);
    tx.set_attempt_time("p", task, n, AttemptStep::Started, NOW - 3_600)?;
    tx.set_attempt_time("p", task, n, AttemptStep::HandedIn, handed_in)?;
    tx.set_attempt_time("p", task, n, AttemptStep::Merged, merged)?;
    Ok(())
}

fn board(store: &mut Store) -> ProjectBoard {
    let options = gear_board::Options { project: Some("p".into()), ..Default::default() };
    gear_board::build(store, &options, NOW)
        .unwrap()
        .projects
        .into_iter()
        .next()
        .expect("the project")
}

#[test]
fn the_review_and_merge_takes_the_mean_of_the_merged_tasks() {
    let mut store = world();
    // T-1 waited 10 minutes, T-2 twenty: the mean is 15.
    let min = store.read(|tx| review_merge_min(tx, "p", "M5")).unwrap();
    assert_eq!(min, 15);
}

#[test]
fn a_voyage_that_merged_nothing_takes_the_default() {
    let mut store = world();
    // A done task whose merge never landed, and a voyage with no done task at
    // all: neither has a measurement, so both take the default 15 minutes.
    store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-3")?.expect("T-3");
            task.state = TaskState::Done;
            task.updated = NOW;
            tx.update_task(&task)
        })
        .unwrap();
    let min = store.read(|tx| review_merge_min(tx, "p", "M5")).unwrap();
    assert_eq!(min, 15);
    let other = store.read(|tx| review_merge_min(tx, "p", "M4")).unwrap();
    assert_eq!(other, 15);
}

#[test]
fn the_newest_attempt_that_reached_the_branch_counts() {
    let mut store = world();
    // T-3 was handed in twice: the first attempt never merged, the second took
    // 10 minutes. Only the second is a measurement.
    store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-3".into(),
                n: 1,
                seat: "p:hull".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Rejected,
                candidate: Some("old".into()),
                report: None,
                safe_point: None,
                started: NOW - 7_200,
                updated: NOW - 6_000,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            tx.set_attempt_time("p", "T-3", 1, AttemptStep::Started, NOW - 7_200)?;
            tx.set_attempt_time("p", "T-3", 1, AttemptStep::HandedIn, NOW - 6_000)?;
            // It did merge in the end, an hour after the hand-in: an attempt
            // the forecast must not measure.
            tx.set_attempt_time("p", "T-3", 1, AttemptStep::Merged, NOW - 2_400)?;
            let mut task = tx.get_task("p", "T-3")?.expect("T-3");
            task.state = TaskState::Done;
            task.updated = NOW;
            tx.update_task(&task)?;
            merged(tx, "T-3", 2, Some(NOW - 600), NOW)
        })
        .unwrap();
    // T-1 waited 10 minutes and T-2 twenty; T-3's newest attempt waited 10
    // more. The mean is 800 seconds: 13 whole minutes, where T-3's first
    // attempt would have made it 30.
    let min = store.read(|tx| review_merge_min(tx, "p", "M5")).unwrap();
    assert_eq!(min, 13);
}

#[test]
fn the_board_plays_each_lane_forward_while_the_voyage_has_open_work() {
    let mut store = world();
    let plan = board(&mut store);

    // One end per lane, in the lanes' order.
    assert_eq!(
        plan.forecast.iter().map(|end| end.lane.clone()).collect::<Vec<_>>(),
        plan.lanes.iter().map(|lane| lane.lane.clone()).collect::<Vec<_>>()
    );
    assert_eq!(plan.forecast.len(), 2);
    // T-4 waits for T-3, which is still on the keel lane: hull is idle until
    // then, and the wait names T-3.
    let hull = plan.forecast.iter().find(|end| end.lane == "hull").expect("hull");
    assert_eq!(
        hull.waits.iter().map(|(_, dep)| dep.as_str()).collect::<Vec<_>>(),
        vec!["T-3"]
    );
    assert!(hull.end_min > 0);

    // The task row carries its dependencies, which is where the forecast gets
    // them.
    let open = plan
        .lanes
        .iter()
        .flat_map(|lane| lane.tasks.iter())
        .find(|task| task.id == "T-4")
        .expect("T-4");
    assert_eq!(open.deps, vec!["T-3".to_string()]);

    // Every task merged: the voyage has no open work, so it has no forecast.
    store
        .write(|tx| {
            for id in ["T-3", "T-4"] {
                let mut task = tx.get_task("p", id)?.expect("the task");
                task.state = TaskState::Done;
                task.updated = NOW;
                tx.update_task(&task)?;
                merged(tx, id, 1, None, NOW - 600)?;
            }
            Ok(())
        })
        .unwrap();
    assert!(board(&mut store).forecast.is_empty());
}
