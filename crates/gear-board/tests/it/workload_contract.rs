//! T-158: the projection per lane (design §10.7 row B4).

use gear_board::frame::lane_panel;
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, Store, TaskRow};

const NOW: i64 = 1_800_000_000;

/// Now, in epoch seconds.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// A voyage with two lanes of very different size and pace: `hull` has little
/// work left, `deck` has much more. Both hand their task in after 30 minutes;
/// deck's merge lands at 60, which is not lane time any more (T-390).
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            testing::seed_active_seat(tx, "p", "deck", SeatRole::Lane, "w1:p3", "pi", NOW)?;
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one seat model".into(),
                done: Some("a rework resumes its session".into()),
                bar: None,
                eta: Some("12h".into()),
                departure: Some(departure),
                departure_tasks: Some(4),
                departure_estimate_min: Some(140),
                updated: NOW,
            })?;
            // hull: 30 minutes of estimate, 30 minutes of work, 20 left.
            task(tx, "T-1", "hull", 1, TaskState::Done, Some(30))?;
            task(tx, "T-2", "hull", 2, TaskState::Ready, Some(20))?;
            // deck: 30 minutes of estimate, 60 minutes of work, 60 left.
            task(tx, "T-3", "deck", 3, TaskState::Done, Some(30))?;
            task(tx, "T-4", "deck", 4, TaskState::Ready, Some(60))?;
            Ok(())
        })
        .unwrap();
    // The work: hull measured 30 minutes on 30 of estimate (pace 1.0), deck
    // measured the same 30 minutes of lane time on 30 of estimate (pace 1.0):
    // its minute 30 to 60 was review and merge, run while the lane was free.
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            attempt(tx, "T-1", departure, departure + 30 * 60, departure + 30 * 60)?;
            attempt(tx, "T-3", departure, departure + 30 * 60, departure + 60 * 60)?;
            Ok(())
        })
        .unwrap();
    store
}

/// A task of the fixture.
fn task(
    tx: &gear_store::Tx<'_>,
    id: &str,
    lane: &str,
    rank: i64,
    state: TaskState,
    estimate: Option<i64>,
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
        milestone: Some("M4".into()),
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: estimate,
        issue: None,
        updated: NOW,
    })
}

/// A finished task of the fixture: it started, it was handed in and it was
/// merged at `merged`.
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
    tx.set_attempt_time("p", task, 1, gear_store::AttemptStep::Started, started)?;
    tx.set_attempt_time("p", task, 1, gear_store::AttemptStep::HandedIn, handed_in)?;
    tx.set_attempt_time("p", task, 1, gear_store::AttemptStep::Merged, merged)?;
    Ok(())
}

/// An open attempt of the fixture, on the step it is: the state the stage
/// rules read (T-346).
fn open_attempt(
    tx: &gear_store::Tx<'_>,
    task: &str,
    state: AttemptState,
    started: i64,
) -> gear_model::GearResult<()> {
    tx.insert_attempt(&AttemptRow {
        project: "p".into(),
        task: task.into(),
        n: 1,
        seat: "p:deck".into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state,
        candidate: None,
        report: None,
        safe_point: None,
        started,
        updated: started,
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
    })?;
    tx.set_attempt_time("p", task, 1, gear_store::AttemptStep::Started, started)?;
    Ok(())
}

#[test]
fn the_slower_lane_sets_the_finish_and_is_named() {
    let mut store = world();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let voyage = &board.projects[0].voyages[0];

    // Each lane at its own pace: hull needs 20 minutes (pace 1.0), deck needs
    // 60 (pace 1.0 on 60 minutes of open estimate). Deck still sets the finish
    // because it has far more work left.
    let hull = voyage.lanes.iter().find(|lane| lane.lane == "hull").unwrap();
    let deck = voyage.lanes.iter().find(|lane| lane.lane == "deck").unwrap();
    assert_eq!(hull.done_min, 30);
    assert_eq!(hull.left_min, 20);
    assert_eq!(hull.pace, Some(1.0));
    assert_eq!(hull.projected_min, Some(20));
    assert_eq!(deck.done_min, 30, "the merge tail is not the lane's own time");
    assert_eq!(deck.left_min, 60);
    assert_eq!(deck.pace, Some(1.0));
    assert_eq!(deck.projected_min, Some(60));

    // The slowest lane sets the finish and is named.
    assert_eq!(
        voyage.slowest_lane.as_ref().map(|(lane, _)| lane.clone()),
        Some("deck".to_string())
    );
    assert_eq!(voyage.slowest_lane.as_ref().map(|(_, min)| *min), Some(60));
    let elapsed = voyage.elapsed_min.unwrap();
    assert_eq!(voyage.projected_min, Some(elapsed + 60));
}

/// T-390: a lane is measured by its throughput, not by the sum of its tasks'
/// spans. Task 1 is merged at 40 while task 2 is already running from 20, so
/// the two spans overlap and the sum would count those minutes twice.
#[test]
fn a_lane_is_measured_by_its_throughput_not_the_sum_of_its_spans() {
    let mut store = Store::open_in_memory().unwrap();
    let departure = now_seconds() - 120 * 60;
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            testing::seed_active_seat(tx, "p", "mast", SeatRole::Lane, "w1:p3", "pi", NOW)?;
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one seat model".into(),
                done: Some("a rework resumes its session".into()),
                bar: None,
                eta: Some("12h".into()),
                departure: Some(departure),
                departure_tasks: Some(3),
                departure_estimate_min: Some(90),
                updated: NOW,
            })?;
            task(tx, "T-1", "hull", 1, TaskState::Done, Some(30))?;
            task(tx, "T-2", "hull", 2, TaskState::Done, Some(30))?;
            // mast has open work and no measured history at all.
            task(tx, "T-3", "mast", 3, TaskState::Ready, Some(100))
        })
        .unwrap();
    store
        .write(|tx| {
            attempt(tx, "T-1", departure, departure + 20 * 60, departure + 40 * 60)?;
            attempt(tx, "T-2", departure + 20 * 60, departure + 40 * 60, departure + 60 * 60)
        })
        .unwrap();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let voyage = &board.projects[0].voyages[0];
    let hull = voyage.lanes.iter().find(|lane| lane.lane == "hull").unwrap();
    // From the earliest start (0) to the last hand-in (40): 40 minutes of lane
    // time, where the sum of the spans would be 40 + 40.
    assert_eq!(hull.done_min, 40);
    assert_eq!(hull.done_estimate_min, 60);
    assert_eq!(hull.pace, Some(40.0 / 60.0));
    assert_eq!(voyage.measured_actual_min, 40);
    assert_eq!(voyage.pace(), Some(40.0 / 60.0));
    // A lane with no done task keeps no pace, and the finish stays unknown.
    let mast = voyage.lanes.iter().find(|lane| lane.lane == "mast").unwrap();
    assert_eq!(mast.done_min, 0);
    assert_eq!(mast.pace, None);
    assert_eq!(voyage.slowest_lane, None);
    assert_eq!(voyage.projected_min, None);
    // Each task's own span still sets the slowest task.
    assert_eq!(voyage.slowest, Some(("T-1".into(), 40)));
}

#[test]
fn the_burn_up_is_the_scope_done_so_far() {
    let mut store = world();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let voyage = &board.projects[0].voyages[0];
    // The burn-up is cumulative: hull's 30 minutes in the first hour and
    // deck's 30 minutes in the second, so 30 and 60 (the prototype's
    // BURN = [0, 0, 95, 170, 260, 330]).
    assert_eq!(voyage.burn_up, vec![30, 60]);
}

#[test]
fn the_burn_up_keeps_the_hours_without_progress() {
    let mut store = world();
    // Three hours later: the voyage is in its fifth hour and nothing was
    // merged since the second.
    let later = now_seconds() + 3 * 3600;
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, later).unwrap();
    let voyage = &board.projects[0].voyages[0];
    // The hours without progress stay in the series, and the scope done so
    // far is kept.
    assert_eq!(voyage.burn_up.len(), 5, "{:?}", voyage.burn_up);
    assert_eq!(voyage.burn_up[0], 30);
    assert_eq!(voyage.burn_up[1], 60);
    assert_eq!(*voyage.burn_up.last().unwrap(), 60, "the scope done is kept");
}

#[test]
fn the_lane_panel_shows_what_it_did_and_what_is_left() {
    let mut store = world();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let lane = board.projects[0].lanes.iter().find(|lane| lane.lane == "deck").unwrap();
    let panel = lane_panel(lane, &board.projects[0].graph);
    let text = panel.join("\n");
    assert!(text.contains("lane deck"), "{text}");
    assert!(text.contains("DONE"), "{text}");
    assert!(text.contains("T-3"), "{text}");
    assert!(text.contains("LEFT, in order"), "{text}");
    assert!(text.contains("T-4"), "{text}");
    assert!(text.contains("left: 1 tasks, 60m of estimates"), "{text}");
}

/// A lane with open work and no measured history: its pace is unknown, so the
/// finish of the voyage is unknown too (F2).
#[test]
fn an_unmeasured_lane_leaves_the_finish_unknown() {
    let mut store = world();
    // A new lane with a big open task and nothing measured yet.
    store
        .write(|tx| {
            testing::seed_active_seat(tx, "p", "mast", SeatRole::Lane, "w1:p4", "pi", NOW)?;
            task(tx, "T-5", "mast", 5, TaskState::Ready, Some(1000))
        })
        .unwrap();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let voyage = &board.projects[0].voyages[0];
    // The lane is in the report with its work, but it has no pace.
    let mast = voyage.lanes.iter().find(|lane| lane.lane == "mast").unwrap();
    assert_eq!(mast.left_min, 1000);
    assert_eq!(mast.pace, None);
    assert_eq!(mast.projected_min, None);
    // The finish of the voyage is unknown: the unmeasured lane is neither
    // dropped nor given another lane's pace.
    assert_eq!(voyage.slowest_lane, None);
    assert_eq!(voyage.projected_min, None);
}

/// An open task without an estimate is unknown work, not zero work (F2).
#[test]
fn an_open_task_without_an_estimate_leaves_the_finish_unknown() {
    let mut store = world();
    store
        .write(|tx| task(tx, "T-5", "deck", 5, TaskState::Ready, None))
        .unwrap();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let voyage = &board.projects[0].voyages[0];
    let deck = voyage.lanes.iter().find(|lane| lane.lane == "deck").unwrap();
    assert!(!deck.left_known, "the open task without an estimate is unknown");
    assert_eq!(voyage.slowest_lane, None);
    assert_eq!(voyage.projected_min, None);
}

/// The lane panel: the running and the reviewing task are in NOW, the queued
/// ones in LEFT — each task once (F4).
#[test]
fn the_lane_panel_shows_the_running_and_the_queued_work_once() {
    let mut store = world();
    store
        .write(|tx| {
            let mut running = tx.get_task("p", "T-4")?.unwrap();
            running.state = TaskState::Running;
            tx.update_task(&running)?;
            open_attempt(tx, "T-4", AttemptState::Started, now_seconds())?;
            task(tx, "T-5", "deck", 5, TaskState::Verifying, Some(20))?;
            open_attempt(tx, "T-5", AttemptState::Submitted, now_seconds())?;
            task(tx, "T-6", "deck", 6, TaskState::Ready, Some(15))
        })
        .unwrap();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let lane = board.projects[0].lanes.iter().find(|lane| lane.lane == "deck").unwrap();
    let text = lane_panel(lane, &board.projects[0].graph).join("\n");
    // The running and the reviewing task are in NOW, once each.
    assert_eq!(text.matches("T-4").count(), 1, "{text}");
    assert_eq!(text.matches("T-5").count(), 1, "{text}");
    // LEFT holds the queued task only.
    assert!(text.contains("○ T-6"), "{text}");
    assert!(text.contains("left: 1 tasks, 15m of estimates"), "{text}");
}
