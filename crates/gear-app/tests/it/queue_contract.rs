//! The rework of T-163: the findings F1-F3 of `ops/reviews/T-163.md`.
//!
//! Each test is the one its finding names: the elapsed minutes of a task
//! that is in flight, the lane that is idle for a reason, and the reviewers
//! that are available.

use gear_app::queue;
use gear_model::{AttemptState, SeatRole, SeatState, TaskState};
use gear_store::testing;
use gear_store::{AttemptRow, AttemptStep, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

/// A task of a lane.
fn task(id: &str, lane: &str, state: TaskState, estimate: i64) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank: 1,
        state,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(estimate),
        issue: None,
        updated: T0,
    }
}

/// A captain and one lane.
fn world() -> gear_store::Store {
    let mut s = gear_store::Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

/// An attempt of a lane that has begun working and is still running.
fn running_attempt(s: &mut gear_store::Store, task_id: &str) {
    s.write(|tx| {
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: task_id.into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: AttemptState::Started,
            candidate: None,
            report: None,
            safe_point: None,
            started: T0,
            updated: T0,
        })
        .unwrap();
        // Only the start is recorded: the round is still running.
        tx.set_attempt_time("p", task_id, 1, AttemptStep::Started, T0)
            .unwrap();
        Ok(())
    })
    .unwrap();
}

/// F1: the elapsed minutes of a task that is in flight run to the clock, not
/// to its last recorded step.
#[test]
fn the_elapsed_minutes_of_a_running_task_run_to_now() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&task("T-1", "heat", TaskState::Running, 60))
            .unwrap();
        Ok(())
    })
    .unwrap();
    running_attempt(&mut s, "T-1");

    let queue = s.read(|tx| queue::queue(tx, "p", T0 + 38 * 60)).unwrap();
    let heat = queue.lanes.iter().find(|lane| lane.lane == "heat").unwrap();
    let working = heat.working.as_ref().expect("the lane works");
    assert_eq!(working.actual_min, Some(38), "38 minutes of work so far");
    assert_eq!(working.estimate_min, Some(60));
    // The text says the same.
    let text = queue::render(&queue);
    assert!(text.contains("working T-1 38/60m"), "{text}");
}

/// F2: a lane that is idle for a reason is not working, and its unfinished
/// task stays in its left list.
#[test]
fn a_held_lane_with_an_open_attempt_is_idle() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&task("T-1", "heat", TaskState::Running, 60))
            .unwrap();
        Ok(())
    })
    .unwrap();
    running_attempt(&mut s, "T-1");
    // The captain holds the lane.
    s.write(|tx| {
        tx.set_seat_state(
            "p:heat",
            SeatState::Held,
            Some("hold: the captain paused it"),
            T0 + 60,
        )
        .unwrap();
        Ok(())
    })
    .unwrap();

    let queue = s.read(|tx| queue::queue(tx, "p", T0 + 38 * 60)).unwrap();
    let heat = queue.lanes.iter().find(|lane| lane.lane == "heat").unwrap();
    assert!(
        heat.working.is_none(),
        "a held lane does not work: {heat:?}"
    );
    assert_eq!(
        heat.idle.as_deref(),
        Some("held: the captain paused it"),
        "the reason comes from the board's own function"
    );
    // The unfinished task is still left to do.
    assert_eq!(heat.left.len(), 1);
    assert_eq!(heat.left[0].id, "T-1");
    // The text and the JSON say the same.
    let text = queue::render(&queue);
    assert!(text.contains("idle: held: the captain paused it"), "{text}");
    assert!(!text.contains("working T-1"), "{text}");
}

/// F2: a lane that declared a wait is idle too, even though its attempt
/// stays open.
#[test]
fn a_waiting_lane_with_an_open_attempt_is_idle() {
    let mut s = world();
    s.write(|tx| {
        let mut waiting = task("T-1", "heat", TaskState::Running, 60);
        // The task waits for another task (§5.3).
        waiting.wait_on = Some("T-9".into());
        tx.insert_task(&waiting).unwrap();
        tx.insert_task(&task("T-9", "keel", TaskState::Ready, 30))
            .unwrap();
        Ok(())
    })
    .unwrap();
    running_attempt(&mut s, "T-1");

    let queue = s.read(|tx| queue::queue(tx, "p", T0 + 38 * 60)).unwrap();
    let heat = queue.lanes.iter().find(|lane| lane.lane == "heat").unwrap();
    assert!(
        heat.working.is_none(),
        "a waiting lane does not work: {heat:?}"
    );
    let idle = heat.idle.as_deref().expect("the lane waits");
    assert!(idle.starts_with("waiting for T-9"), "{idle}");
}

/// F3: a released or retired reviewer is not in the queue; a standby one
/// says how long it has been standing by.
#[test]
fn only_the_available_reviewers_are_in_the_queue() {
    let mut s = world();
    s.write(|tx| {
        // An active reviewer with a job.
        testing::seed_temp_seat(tx, "p", "r-1", "T-1", "w1:p3", "codex", T0).unwrap();
        // A standby reviewer, standing by for three minutes.
        testing::seed_temp_seat(tx, "p", "r-2", "T-2", "w1:p4", "codex", T0).unwrap();
        tx.set_seat_state("p:r-2", SeatState::Standby, None, T0)
            .unwrap();
        // A released reviewer: its agent and its pane are gone (§3.6).
        testing::seed_temp_seat(tx, "p", "r-3", "T-3", "w1:p5", "codex", T0).unwrap();
        tx.set_seat_state("p:r-3", SeatState::Released, None, T0)
            .unwrap();
        // A retired reviewer.
        testing::seed_temp_seat(tx, "p", "r-4", "T-4", "w1:p6", "codex", T0).unwrap();
        tx.set_seat_state("p:r-4", SeatState::Retired, None, T0)
            .unwrap();
        Ok(())
    })
    .unwrap();

    let queue = s.read(|tx| queue::queue(tx, "p", T0 + 180)).unwrap();
    let names: Vec<&str> = queue
        .reviewers
        .iter()
        .map(|reviewer| reviewer.name.as_str())
        .collect();
    assert_eq!(names, vec!["r-1", "r-2"], "only the available reviewers");
    let standby = queue
        .reviewers
        .iter()
        .find(|reviewer| reviewer.name == "r-2")
        .unwrap();
    assert_eq!(standby.standby_min, Some(3), "three minutes of standing by");
    assert_eq!(standby.reviewing.as_deref(), Some("T-2"));

    // The text says the same.
    let text = queue::render(&queue);
    assert!(text.contains("r-1 reviewing T-1"), "{text}");
    assert!(text.contains("r-2 standby 3m"), "{text}");
    assert!(!text.contains("r-3"), "{text}");
    assert!(!text.contains("r-4"), "{text}");
}

/// T-208: an overlap with the integration branch is an information line: the
/// queue shows it after the lanes, and the JSON carries it as `overlaps`.
#[test]
fn the_queue_shows_the_overlaps_with_the_integration_branch() {
    let mut s = world();
    s.write(|tx| {
        let value = serde_json::json!({
            "task": "T-9",
            "integration": "dev",
            "files": ["crates/gear-app/src/queue.rs"],
        })
        .to_string();
        tx.set_meta("parallel:p:integration:T-9", &value)
    })
    .unwrap();

    let queue = s.read(|tx| queue::queue(tx, "p", T0)).unwrap();
    assert_eq!(queue.overlaps.len(), 1);
    assert_eq!(queue.overlaps[0].task, "T-9");
    assert_eq!(queue.overlaps[0].integration, "dev");

    // The text says the same, after the lane lines.
    let text = queue::render(&queue);
    let lane_line = text.lines().next().unwrap_or_default();
    let overlap_line = text
        .lines()
        .find(|line| line.starts_with("overlap:"))
        .unwrap_or_default();
    assert_eq!(
        overlap_line,
        "overlap: T-9 shares crates/gear-app/src/queue.rs with dev"
    );
    assert!(
        text.find("overlap:").unwrap() > text.find(lane_line).unwrap(),
        "the overlap comes after the lanes: {text}"
    );

    // The JSON carries the same list.
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&queue).unwrap()).unwrap();
    assert_eq!(json["overlaps"][0]["task"], "T-9");
    assert_eq!(json["overlaps"][0]["integration"], "dev");
    assert_eq!(
        json["overlaps"][0]["files"][0],
        "crates/gear-app/src/queue.rs"
    );
}

/// T-208: a cleared overlap shows nothing.
#[test]
fn a_cleared_overlap_is_not_in_the_queue() {
    let mut s = world();
    s.write(|tx| tx.set_meta("parallel:p:integration:T-9", ""))
        .unwrap();

    let queue = s.read(|tx| queue::queue(tx, "p", T0)).unwrap();
    assert!(queue.overlaps.is_empty());
    assert!(!queue::render(&queue).contains("overlap:"));
}
