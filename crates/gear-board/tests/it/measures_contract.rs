//! The never-stop measures contract (T-16): the rework rate of a lane and
//! the time since it last received a task, read from the store only.
//!
//! The fixture is the packet's: one lane with two hand-ins (one sent back)
//! and a delivery five minutes ago, plus one registered lane with nothing at
//! all. Both must render.

use gear_board::{build, render, to_json, Options, View};
use gear_model::{AttemptState, OpKind, SeatRole, TaskState};
use gear_store::{testing, NewOp, Store, TaskRow};

const T0: i64 = 1_800_000_000;

fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                "w1:p1",
                "claude",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            // A registered lane that never received a task.
            testing::seed_active_seat(tx, "p", "runtime", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            for (id, rank, state) in [
                ("T-1", 1, TaskState::Verifying),
                ("T-2", 2, TaskState::Ready),
            ] {
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
                    lane: Some("data".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: Vec::new(),
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
            }
            // Two hand-ins on T-1: the first was sent back, the second is in
            // review.
            for (n, state) in [(1, AttemptState::Rejected), (2, AttemptState::Submitted)] {
                tx.insert_attempt(&gear_store::AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: "T-1".into(),
                    n,
                    seat: "p:data".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state,
                    candidate: Some(format!("c{n}")),
                    report: None,
                    safe_point: None,
                    started: T0 - 900,
                    updated: T0 - 600,
                })?;
            }
            // The lane last received a task five minutes before the frame.
            tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:data".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-1".into()),
                attempt: Some(2),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: T0 - 300,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

fn lane_header<'a>(text: &'a str, lane: &str) -> &'a str {
    text.lines()
        .find(|line| line.starts_with(&format!("Lane {lane} ·")))
        .expect("lane header")
}

fn options() -> Options {
    Options {
        project: Some("p".into()),
        ..Options::default()
    }
}

#[test]
fn the_board_shows_the_rework_rate_and_the_time_since_the_last_task() {
    let mut store = world();
    let text = render(&build(&mut store, &options(), T0).unwrap(), View::Lanes);

    // One lane: two hand-ins, one sent back, and the delivery five minutes
    // ago.
    assert!(
        lane_header(&text, "data").contains("rework 1/2 · last task 5m ago"),
        "{text}"
    );
    // The other never received a task.
    assert!(
        lane_header(&text, "runtime").contains("rework 0/0 · no task yet"),
        "{text}"
    );
    // The project-wide rate is on the totals line.
    assert!(
        text.contains("Totals: 2 tasks · 0 done · 0 in progress · 0 waiting · rework 1/2"),
        "{text}"
    );
}

#[test]
fn the_measure_follows_the_seat_that_made_the_attempt() {
    let mut store = world();
    // The task moves to another lane after the rejection: the board lane's
    // hands did the work, so its rate keeps the two hand-ins (the review of
    // T-16).
    store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-1").unwrap().unwrap();
            task.lane = Some("runtime".into());
            tx.update_task(&task)?;
            Ok(())
        })
        .unwrap();

    let text = render(&build(&mut store, &options(), T0).unwrap(), View::Lanes);

    assert!(lane_header(&text, "data").contains("rework 1/2"), "{text}");
    assert!(
        lane_header(&text, "runtime").contains("rework 0/0"),
        "{text}"
    );
    // And the project-wide rate counts each hand-in once.
    assert!(text.contains("rework 1/2"), "{text}");
}

#[test]
fn the_measures_are_in_the_json() {
    let mut store = world();
    let board = build(&mut store, &options(), T0).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&to_json(&board).unwrap()).unwrap();

    assert_eq!(doc["projects"][0]["totals"]["handed_in"], 2);
    assert_eq!(doc["projects"][0]["totals"]["rejected"], 1);
    let data = &doc["projects"][0]["lanes"][0];
    assert_eq!(data["lane"], "data");
    assert_eq!(data["rework"]["handed_in"], 2);
    assert_eq!(data["rework"]["rejected"], 1);
    assert_eq!(data["rework"]["last_task_age_s"], 300);
    let runtime = &doc["projects"][0]["lanes"][1];
    assert_eq!(runtime["rework"]["handed_in"], 0);
    assert_eq!(runtime["rework"]["rejected"], 0);
    assert!(runtime["rework"]["last_task_age_s"].is_null());
}

#[test]
fn temp_hand_ins_count_in_the_project_rate() {
    let mut store = world();
    // A temp worker hands the task in: a third hand-in for the project, and
    // no lane's (F2 of the T-16 review).
    store
        .write(|tx| {
            tx.insert_attempt(&gear_store::AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 3,
                seat: "p:t2".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Submitted,
                candidate: Some("c3".into()),
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            Ok(())
        })
        .unwrap();

    let text = render(&build(&mut store, &options(), T0).unwrap(), View::Lanes);
    // One rejection in three hand-ins; the lane keeps its own 1/2 (F1).
    assert!(
        text.contains("Totals: 2 tasks · 0 done · 0 in progress · 0 waiting · rework 1/3"),
        "{text}"
    );
    assert!(lane_header(&text, "data").contains("rework 1/2"), "{text}");

    // The temp attempt is sent back: two rejections in three hand-ins.
    store
        .write(|tx| {
            let mut attempt = tx.get_attempt("p", "T-1", 3).unwrap().unwrap();
            attempt.state = AttemptState::Rejected;
            tx.update_attempt(&attempt)?;
            Ok(())
        })
        .unwrap();
    let board = build(&mut store, &options(), T0).unwrap();
    let text = render(&board, View::Lanes);
    assert!(
        text.contains("Totals: 2 tasks · 0 done · 0 in progress · 0 waiting · rework 2/3"),
        "{text}"
    );
    let doc: serde_json::Value = serde_json::from_str(&to_json(&board).unwrap()).unwrap();
    assert_eq!(doc["projects"][0]["totals"]["handed_in"], 3);
    assert_eq!(doc["projects"][0]["totals"]["rejected"], 2);
    // The lane still owns only its two hand-ins.
    assert_eq!(doc["projects"][0]["lanes"][0]["rework"]["handed_in"], 2);
}
