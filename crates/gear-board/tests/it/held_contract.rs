//! T-536: a held task stays in its voyage's scope and counts (decision 04:45).
//!
//! A task is Draft only through `state: hold`, so Draft reads as held: it is
//! remaining work of the voyage, listed as held, but a lane cannot start it.

use gear_board::{idle, state_word, stations, voyage};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, Store, TaskRow, VoyageRow};

const NOW: i64 = 1_800_000_000;

/// A project with two lanes, a departed voyage and three tasks: one merged,
/// one queued and one held on the lane that has nothing else.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                "w1:p1",
                "codex",
                NOW,
            )?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p3", "pi", NOW)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M1".into(),
                title: "the held voyage".into(),
                done: Some("it ships".into()),
                bar: Some("keep it simple".into()),
                eta: Some("4h".into()),
                departure: Some(NOW - 3_600),
                departure_tasks: Some(3),
                departure_estimate_min: Some(60),
                updated: NOW,
            })?;
            tx.set_meta("voyage-stations:p:M1", "S1: the voyage ships")?;
            // T-1 is merged, T-2 is queued on hull, T-3 is held on data.
            task(tx, "T-1", 1, TaskState::Done, "hull", 10)?;
            task(tx, "T-2", 2, TaskState::Ready, "hull", 20)?;
            task(tx, "T-3", 3, TaskState::Draft, "data", 30)?;
            for id in ["T-1", "T-2", "T-3"] {
                tx.set_meta(&format!("task-station:p:{id}"), "S1")?;
            }
            Ok(())
        })
        .unwrap();
    // T-1 started, was handed in and merged inside the voyage.
    store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:hull".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: Some("abc".into()),
                report: None,
                safe_point: None,
                started: NOW - 3_000,
                updated: NOW - 3_000,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, NOW - 3_000)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, NOW - 2_400)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, NOW - 2_400)
        })
        .unwrap();
    store
}

fn task(
    tx: &gear_store::Tx<'_>,
    id: &str,
    rank: i64,
    state: TaskState,
    lane: &str,
    estimate: i64,
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
        milestone: Some("M1".into()),
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(estimate),
        issue: None,
        updated: NOW,
    })
}

#[test]
fn a_held_task_counts_in_the_scope_the_stations_and_the_voyage_report() {
    let mut store = world();
    // The station scope: three tasks, one merged, and the held task's estimate
    // is left work like the queued one's.
    let progress = store.read(|tx| stations::progress(tx, "p", "M1")).unwrap();
    let s1 = progress.first().expect("the station");
    assert_eq!((s1.tasks, s1.merged, s1.left_min), (3, 1, 50));
    let course = store.read(|tx| stations::course(tx, "p", "M1")).unwrap();
    assert!(course.contains("2 tasks left in the voyage"), "{course}");
    // The voyage header: the held task is in its scope and in what is left.
    let reports = store.read(|tx| voyage::reports(tx, "p", NOW)).unwrap();
    let report = reports.first().expect("the voyage");
    assert_eq!(report.scope_min, Some(60));
    assert_eq!(report.remaining_estimate_min, Some(50));
    let data = report
        .lanes
        .iter()
        .find(|lane| lane.lane == "data")
        .expect("the data lane");
    assert_eq!((data.left_count, data.left_min), (1, 30));
    // The board's totals count it too (decision 04:45).
    let plan = store
        .read(|tx| gear_board::project_for(tx, "p", NOW, None, Some("M1")))
        .unwrap();
    assert_eq!(plan.totals.tasks, 3, "{:?}", plan.totals);
    assert_eq!(plan.totals.done, 1);
}

#[test]
fn a_held_task_is_open_in_the_deps_tree_and_reads_as_held() {
    let mut store = world();
    let plan = store
        .read(|tx| gear_board::project_for(tx, "p", NOW, None, Some("M1")))
        .unwrap();
    let held = plan
        .deps
        .iter()
        .find(|node| node.id == "T-3")
        .expect("the held task is open in the tree");
    assert_eq!(held.state, TaskState::Draft);
    assert_eq!(state_word(held.state), "held");
    // The queued and the held task are both open; the merged one is not.
    assert!(
        plan.deps.iter().any(|node| node.id == "T-2"),
        "{:?}",
        plan.deps
    );
    assert!(
        !plan.deps.iter().any(|node| node.id == "T-1"),
        "{:?}",
        plan.deps
    );
}

#[test]
fn a_lane_whose_only_task_is_held_has_nothing_it_can_start() {
    let mut store = world();
    let data = store
        .read(|tx| idle::idle_reason(tx, "p", "data", NOW))
        .unwrap();
    assert_eq!(data, Some(idle::IdleReason::NoWork), "{data:?}");
    // The lane with a queued task still has one it can start.
    let hull = store
        .read(|tx| idle::idle_reason(tx, "p", "hull", NOW))
        .unwrap();
    assert_eq!(
        hull,
        Some(idle::IdleReason::Undelivered { task: "T-2".into() }),
        "{hull:?}"
    );
}
