//! T-158: the board's third view (Workload) and the projection per lane
//! (design §10.7 row B4).

use gear_board::frame::{frame, lane_panel, Tab};
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
/// work left and works at its estimate, `deck` has much more work left and
/// works twice as slow.
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
            task(tx, "T-1", "hull", 1, TaskState::Done, 30)?;
            task(tx, "T-2", "hull", 2, TaskState::Ready, 20)?;
            // deck: 30 minutes of estimate, 60 minutes of work, 60 left.
            task(tx, "T-3", "deck", 3, TaskState::Done, 30)?;
            task(tx, "T-4", "deck", 4, TaskState::Ready, 60)?;
            Ok(())
        })
        .unwrap();
    // The work: hull measured 30 minutes on 30 of estimate (pace 1.0), deck
    // measured 60 minutes on 30 of estimate (pace 2.0).
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
        milestone: Some("M4".into()),
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

#[test]
fn the_slower_lane_sets_the_finish_and_is_named() {
    let mut store = world();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let voyage = &board.projects[0].voyages[0];

    // Each lane at its own pace: hull needs 20 minutes (pace 1.0), deck needs
    // 120 (pace 2.0 on 60 minutes of estimate).
    let hull = voyage.lanes.iter().find(|lane| lane.lane == "hull").unwrap();
    let deck = voyage.lanes.iter().find(|lane| lane.lane == "deck").unwrap();
    assert_eq!(hull.done_min, 30);
    assert_eq!(hull.left_min, 20);
    assert_eq!(hull.pace, Some(1.0));
    assert_eq!(hull.projected_min, Some(20));
    assert_eq!(deck.done_min, 60);
    assert_eq!(deck.left_min, 60);
    assert_eq!(deck.pace, Some(2.0));
    assert_eq!(deck.projected_min, Some(120));

    // The slowest lane sets the finish and is named.
    assert_eq!(
        voyage.slowest_lane.as_ref().map(|(lane, _)| lane.clone()),
        Some("deck".to_string())
    );
    assert_eq!(voyage.slowest_lane.as_ref().map(|(_, min)| *min), Some(120));
    let elapsed = voyage.elapsed_min.unwrap();
    assert_eq!(voyage.projected_min, Some(elapsed + 120));
}

#[test]
fn the_burn_up_counts_the_estimate_merged_per_hour() {
    let mut store = world();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let voyage = &board.projects[0].voyages[0];
    // The estimate merged per hour of the voyage: hull's 30 minutes in the
    // first hour, deck's 30 minutes in the second.
    assert_eq!(voyage.burn_up, vec![30, 30]);
}

#[test]
fn the_workload_view_shows_the_lanes_and_the_burn_up() {
    let mut store = world();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let text = frame(&board, Tab::Workload, 0, None, (120, 40));
    assert!(text.contains("WORK DONE AND LEFT, BY LANE"), "{text}");
    assert!(text.contains("hull"), "{text}");
    assert!(text.contains("deck"), "{text}");
    // The bars of the lanes: the done minutes and the minutes left.
    assert!(text.contains("done 1 (30m) · left 1 (20m)"), "{text}");
    assert!(text.contains("done 1 (30m) · left 1 (60m)"), "{text}");
    // The burn-up and its hours.
    assert!(text.contains("BURN-UP  (minutes of estimate merged, per voyage hour)"), "{text}");
    assert!(text.contains("h0"), "{text}");
    // The bars of the burn-up: the estimate merged in the hour.
    assert!(text.contains("██"), "{text}");
}

#[test]
fn the_lane_panel_shows_what_it_did_and_what_is_left() {
    let mut store = world();
    let board = gear_board::build(&mut store, &gear_board::Options { project: Some("p".into()), ..Default::default() }, now_seconds()).unwrap();
    let lane = board.projects[0].lanes.iter().find(|lane| lane.lane == "deck").unwrap();
    let panel = lane_panel(lane);
    let text = panel.join("\n");
    assert!(text.contains("lane deck"), "{text}");
    assert!(text.contains("DONE"), "{text}");
    assert!(text.contains("T-3"), "{text}");
    assert!(text.contains("LEFT, in order"), "{text}");
    assert!(text.contains("T-4"), "{text}");
    assert!(text.contains("left: 1 tasks, 60m of estimates"), "{text}");
}
