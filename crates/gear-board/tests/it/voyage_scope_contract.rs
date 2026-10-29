//! T-246: each voyage is independent, so the board shows the current voyage
//! only. T-350: the board shows one voyage and nothing of another; the captain
//! home page keeps the older rule — a task of a landed voyage that still waits
//! for work is a decision the captain owes.

use gear_board::frame::token_tasks;
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, Store, TaskRow, VoyageRow};

const NOW: i64 = 1_800_000_000;

/// A project with two voyages: M1 departed earlier and landed, M2 departed
/// later and carries the work.
fn world(m1_left_open: bool) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            voyage(tx, "M1", NOW - 7200)?;
            voyage(tx, "M2", NOW - 3600)?;
            // M1: one task, done unless the fixture leaves it open.
            let state = if m1_left_open { TaskState::Ready } else { TaskState::Done };
            task(tx, "T-1", 1, "M1", state)?;
            // M2: one done, one ready.
            task(tx, "T-2", 2, "M2", TaskState::Done)?;
            task(tx, "T-3", 3, "M2", TaskState::Ready)?;
            Ok(())
        })
        .unwrap();
    // The M1 task ran an hour before the board looks at the plan.
    store
        .write(|tx| {
            attempt(tx, "T-1", NOW - 6600, NOW - 5400, NOW - 5400)?;
            attempt(tx, "T-2", NOW - 3000, NOW - 2400, NOW - 2400)
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
        lane: Some("hull".into()),
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

/// A finished task: it started, was handed in and was merged at `merged`.
fn attempt(
    tx: &gear_store::Tx<'_>,
    task: &str,
    started: i64,
    handed_in: i64,
    merged: i64,
) -> gear_model::GearResult<()> {
    tx.insert_attempt(&AttemptRow {
        project: "p".into(),
        task: task.into(),
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
        started,
        updated: started,
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
    })?;
    tx.set_attempt_time("p", task, 1, AttemptStep::Started, started)?;
    tx.set_attempt_time("p", task, 1, AttemptStep::HandedIn, handed_in)?;
    tx.set_attempt_time("p", task, 1, AttemptStep::Merged, merged)?;
    Ok(())
}

fn board(store: &mut Store) -> gear_board::ProjectBoard {
    let options = gear_board::Options { project: Some("p".into()), ..Default::default() };
    gear_board::build(store, &options, NOW)
        .unwrap()
        .projects
        .into_iter()
        .next()
        .expect("the project")
}

#[test]
fn the_board_counts_and_works_the_current_voyage_only() {
    let mut store = world(false);
    let plan = board(&mut store);

    // The two M2 tasks; the done task of the landed M1 voyage is history.
    assert_eq!(plan.totals.tasks, 2, "the tasks of M2: {:?}", plan.totals);
    assert_eq!(plan.totals.done, 1);
    // The Time & tokens view lists no task of M1.
    let listed: Vec<&str> = token_tasks(&plan).iter().map(|(_, task)| task.id.as_str()).collect();
    assert!(!listed.contains(&"T-1"), "no task of the landed voyage: {listed:?}");
    assert!(listed.contains(&"T-2"), "the measured task of M2: {listed:?}");
    // Nor does the lane line.
    let lane = plan.lanes.iter().find(|lane| lane.lane == "hull").expect("the lane");
    let ids: Vec<&str> = lane.tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(ids, vec!["T-2", "T-3"], "the lane works M2 only");
    // The graph follows the same list.
    let graphed: Vec<&str> = plan.graph.iter().map(|task| task.id.as_str()).collect();
    assert!(!graphed.contains(&"T-1"), "no task of the landed voyage: {graphed:?}");
}

#[test]
fn the_board_shows_one_voyage_but_the_home_page_keeps_every_open_task() {
    let mut store = world(true);
    let plan = board(&mut store);

    // The board measures M1 (it still has open work) and shows its tasks only:
    // an open task of another voyage is not on it (README change 6, T-350).
    let lane = plan.lanes.iter().find(|lane| lane.lane == "hull").expect("the lane");
    let ids: Vec<&str> = lane.tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(ids, vec!["T-1"], "the board works the measured voyage only: {ids:?}");

    // The home page, which names no voyage, keeps today's rule: the open task
    // of M2 is not history, it is a decision the captain owes.
    let home = store
        .read(|tx| gear_board::project_with_agents(tx, "p", NOW, None))
        .unwrap();
    let lane = home.lanes.iter().find(|lane| lane.lane == "hull").expect("the lane");
    let ids: Vec<&str> = lane.tasks.iter().map(|task| task.id.as_str()).collect();
    assert!(ids.contains(&"T-3"), "the open task of M2 on the home page: {ids:?}");
}
