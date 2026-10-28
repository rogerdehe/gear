//! T-155: why a lane is idle (design §8.2, §13.6).
//!
//! One store fixture per reason: the reason and its text. A lane that works
//! has none.

use gear_board::idle::{describe, idle_reason, IdleReason};
use gear_model::{AttemptState, SeatRole, SeatState, TaskState};
use gear_store::{testing, AttemptRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const LANE: &str = "heat";

/// A project with one lane, one task and no attempt.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", LANE, SeatRole::Lane, "w1:p2", "pi", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// A task of the lane.
fn task(
    store: &mut Store,
    id: &str,
    rank: i64,
    state: TaskState,
    deps: Vec<&str>,
    wait_on: Option<&str>,
) {
    store
        .write(|tx| {
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
                lane: Some(LANE.into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: deps.into_iter().map(str::to_string).collect(),
                followup_of: None,
                wait_on: wait_on.map(str::to_string),
                wait_cond: wait_on.map(|_| "merged".to_string()),
                rework_note: None,
                estimate_min: None,
                issue: None,
                updated: T0,
            })
        })
        .unwrap();
}

/// An attempt of the lane, in the state the test needs.
fn attempt(store: &mut Store, id: &str, n: i64, state: AttemptState) {
    store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: id.into(),
                n,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })
        })
        .unwrap();
}

/// The seat of the lane, in the state the test needs.
fn seat(store: &mut Store, state: SeatState, reason: Option<&str>) {
    store
        .write(|tx| tx.set_seat_state("p:heat", state, reason, T0))
        .unwrap();
}

/// The reason of the lane, and its text.
fn reason_of(store: &mut Store, now: i64) -> (Option<IdleReason>, String) {
    let reason = store.read(|tx| idle_reason(tx, "p", LANE, now)).unwrap();
    let text = store
        .read(|tx| {
            Ok(match &reason {
                Some(reason) => Some(describe(reason, tx, "p")),
                None => None,
            })
        })
        .unwrap();
    (reason, text.unwrap_or_default())
}

#[test]
fn a_quota_parked_lane_says_until_when_the_provider_is_back() {
    let mut store = world();
    // The engine parks a lane on quota and says so in the seat's reason.
    seat(
        &mut store,
        SeatState::Parked,
        Some("quota: MiniMax-M3.1-Flash; resume at 1800003600"),
    );
    store
        .write(|tx| tx.set_meta("engine.resume.p:heat", "1800003600"))
        .unwrap();

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::Quota {
            until: 1_800_003_600
        }),
        "{reason:?}"
    );
    assert!(text.starts_with("quota until "), "{text}");
    // The clock of the user: two digits, a colon, two digits.
    let clock = text.trim_start_matches("quota until ");
    assert_eq!(clock.len(), 5, "{clock}");
    assert_eq!(clock.as_bytes()[2], b':', "{clock}");
}

#[test]
fn a_parked_lane_says_until_when_it_is_let_go() {
    let mut store = world();
    // The plan parks a lane with `until=<RFC 3339>`.
    seat(
        &mut store,
        SeatState::Parked,
        Some("until=2026-10-27T10:00:00Z"),
    );

    let (reason, text) = reason_of(&mut store, T0);
    let Some(IdleReason::Parked { until }) = reason else {
        panic!("{reason:?}");
    };
    assert!(
        until.is_some(),
        "the plan says until when the seat is let go"
    );
    assert!(text.starts_with("parked until "), "{text}");
}

#[test]
fn a_held_lane_says_why() {
    let mut store = world();
    seat(
        &mut store,
        SeatState::Held,
        Some("hold: waiting for the user"),
    );

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::Held {
            reason: "waiting for the user".into()
        }),
        "{reason:?}"
    );
    assert_eq!(text, "held: waiting for the user");
}

#[test]
fn a_lane_that_waits_for_a_task_says_which_one() {
    let mut store = world();
    task(
        &mut store,
        "T-2",
        1,
        TaskState::Running,
        vec![],
        Some("T-9"),
    );
    task(&mut store, "T-9", 2, TaskState::Ready, vec![], None);
    // The attempt of the task stays Started (§5.3): the wait comes first.
    attempt(&mut store, "T-2", 1, AttemptState::Started);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::Waiting { on: "T-9".into() }),
        "{reason:?}"
    );
    assert_eq!(text, "waiting for T-9 (heat, queued)");
}

#[test]
fn a_lane_whose_next_task_waits_for_a_dependency_says_which_one() {
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Ready, vec!["T-9"], None);
    task(&mut store, "T-9", 2, TaskState::Ready, vec![], None);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::DepWait {
            task: "T-2".into(),
            on: "T-9".into()
        }),
        "{reason:?}"
    );
    assert_eq!(text, "next T-2 waits for T-9 (heat, queued)");
}

#[test]
fn a_dependency_that_is_done_is_not_a_wait() {
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Ready, vec!["T-9"], None);
    task(&mut store, "T-9", 2, TaskState::Done, vec![], None);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::Undelivered { task: "T-2".into() }),
        "{reason:?}"
    );
    assert_eq!(text, "next T-2 is ready but not delivered");
}

#[test]
fn a_lane_with_a_ready_task_that_was_never_delivered_says_which_one() {
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Ready, vec![], None);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::Undelivered { task: "T-2".into() }),
        "{reason:?}"
    );
    assert_eq!(text, "next T-2 is ready but not delivered");
}

#[test]
fn a_lane_with_nothing_left_says_so() {
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Done, vec![], None);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(reason, Some(IdleReason::NoWork), "{reason:?}");
    assert_eq!(text, "nothing left planned");
}

#[test]
fn a_lane_that_works_has_no_reason() {
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Running, vec![], None);
    attempt(&mut store, "T-2", 1, AttemptState::Started);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(reason, None, "{reason:?}");
    assert!(text.is_empty(), "{text}");
}

#[test]
fn a_lane_with_nothing_at_all_says_nothing_is_left() {
    let mut store = world();
    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(reason, Some(IdleReason::NoWork), "{reason:?}");
    assert_eq!(text, "nothing left planned");
}

#[test]
fn a_lane_whose_work_is_in_review_has_nothing_left() {
    // The work was handed in and waits for its review in the background: the
    // lane is idle and has nothing left (review F1).
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Verifying, vec![], None);
    attempt(&mut store, "T-2", 1, AttemptState::Submitted);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(reason, Some(IdleReason::NoWork), "{reason:?}");
    assert_eq!(text, "nothing left planned");
}

#[test]
fn the_task_after_the_one_in_review_is_the_next_one() {
    // The work in review does not hide the task that is queued behind it.
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Verifying, vec![], None);
    attempt(&mut store, "T-2", 1, AttemptState::Submitted);
    task(&mut store, "T-3", 2, TaskState::Ready, vec![], None);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::Undelivered { task: "T-3".into() }),
        "{reason:?}"
    );
    assert_eq!(text, "next T-3 is ready but not delivered");
}

#[test]
fn the_task_after_the_one_in_review_may_wait_for_a_dependency() {
    let mut store = world();
    task(&mut store, "T-2", 1, TaskState::Verifying, vec![], None);
    attempt(&mut store, "T-2", 1, AttemptState::Submitted);
    task(&mut store, "T-3", 2, TaskState::Ready, vec!["T-9"], None);
    task(&mut store, "T-9", 3, TaskState::Ready, vec![], None);

    let (reason, text) = reason_of(&mut store, T0);
    assert_eq!(
        reason,
        Some(IdleReason::DepWait {
            task: "T-3".into(),
            on: "T-9".into()
        }),
        "{reason:?}"
    );
    assert_eq!(text, "next T-3 waits for T-9 (heat, queued)");
}
