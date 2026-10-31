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

// ---------------------------------------------------------------- T-323
//
// A lane that helps on an unanswered instruction of the captain counts as
// busy: it has no attempt in flight, but the captain asked it for something.

use gear_board::{build, lane_line, Options};
use gear_model::{MsgKind, MsgState};
use gear_store::NewMessage;

const CAPTAIN: &str = "captain";

/// The world with a captain, so an instruction can be sent.
fn world_with_captain() -> Store {
    let mut store = world();
    store
        .write(|tx| {
            testing::seed_active_seat(tx, "p", CAPTAIN, SeatRole::Captain, "w1:p1", "claude", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// A task in the lane the test names.
fn task_on(store: &mut Store, id: &str, lane: &str, rank: i64, state: TaskState) {
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
                lane: Some(lane.into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                issue: None,
                updated: T0,
            })
        })
        .unwrap();
}

/// A message to the lane; returns its id.
fn message(
    store: &mut Store,
    kind: MsgKind,
    sender: &str,
    task: Option<&str>,
    body: &str,
    reply_to: Option<&str>,
    created: i64,
) -> String {
    store
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "T-2:1".into(),
                    task: task.map(str::to_string),
                    attempt: None,
                    sender: sender.into(),
                    sender_gen: 1,
                    recipient: "p:heat".into(),
                    kind,
                    urgent: false,
                    body: body.into(),
                    reply_to: reply_to.map(str::to_string),
                    needs_ack: false,
                    state: MsgState::Open,
                    created,
                })?
                .id)
        })
        .unwrap()
}

/// The help the lane gives, if any.
fn help_of(store: &mut Store, now: i64) -> Option<gear_board::idle::Helping> {
    store
        .read(|tx| {
            let seat = tx.get_seat("p:heat")?.expect("the lane's seat");
            gear_board::idle::helping(tx, "p", &seat, now)
        })
        .unwrap()
}

/// The board of the project `p`.
fn options() -> Options {
    Options {
        project: Some("p".into()),
        ..Options::default()
    }
}

#[test]
fn a_lane_that_helps_says_which_lane_and_what_it_does() {
    let mut store = world_with_captain();
    task_on(&mut store, "T-2", "mast", 1, TaskState::Running);
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        Some("T-2"),
        "Read the log of T-2 and send mast the failing line please",
        None,
        T0 + 60,
    );

    let help = help_of(&mut store, T0 + 120).expect("the lane helps");
    assert_eq!(help.text(), "helping mast: Read the log of T-2 and …");
    assert_eq!(help.since, T0 + 60);

    // The lane is not idle: the help comes before the queue's reasons.
    let (reason, text) = reason_of(&mut store, T0 + 120);
    assert_eq!(reason, None, "{reason:?}");
    assert!(text.is_empty(), "{text}");

    let board = build(&mut store, &options(), T0 + 120).unwrap();
    let lane = board.projects[0]
        .lanes
        .iter()
        .find(|lane| lane.lane == LANE)
        .expect("the lane");
    assert_eq!(
        lane.helping.as_deref(),
        Some("helping mast: Read the log of T-2 and …")
    );
    assert!(
        lane_line(lane).contains("helping mast: Read the log"),
        "{}",
        lane_line(lane)
    );
}

#[test]
fn an_instruction_without_a_task_is_help_for_the_captain() {
    let mut store = world_with_captain();
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        None,
        "Check the quota line",
        None,
        T0 + 60,
    );

    let help = help_of(&mut store, T0 + 120).expect("the lane helps");
    assert_eq!(help.text(), "helping the captain: Check the quota line");
    assert_eq!(help.lane, None);
    let (reason, _) = reason_of(&mut store, T0 + 120);
    assert_eq!(reason, None, "{reason:?}");
}

#[test]
fn an_instruction_about_the_lanes_own_task_is_help_for_the_captain() {
    let mut store = world_with_captain();
    task_on(&mut store, "T-2", LANE, 1, TaskState::Running);
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        Some("T-2"),
        "Send me the failing line",
        None,
        T0 + 60,
    );

    let help = help_of(&mut store, T0 + 120).expect("the lane helps");
    assert_eq!(help.text(), "helping the captain: Send me the failing line");
}

#[test]
fn a_reply_ends_the_help_and_the_lane_is_idle_again() {
    let mut store = world_with_captain();
    let instruction = message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        None,
        "Look at the quota line of the provider",
        None,
        T0 + 60,
    );
    // The lane answers it.
    message(
        &mut store,
        MsgKind::Reply,
        "p:heat",
        None,
        "The quota line says 80% used",
        Some(&instruction),
        T0 + 90,
    );

    assert_eq!(help_of(&mut store, T0 + 120), None);
    let (reason, text) = reason_of(&mut store, T0 + 120);
    assert_eq!(reason, Some(IdleReason::NoWork), "{reason:?}");
    assert_eq!(text, "nothing left planned");
}

#[test]
fn an_instruction_older_than_the_limit_does_not_count() {
    let mut store = world_with_captain();
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        None,
        "Look at the quota line of the provider",
        None,
        T0,
    );

    assert_eq!(help_of(&mut store, T0 + 1801), None);
    let (reason, _) = reason_of(&mut store, T0 + 1801);
    assert_eq!(reason, Some(IdleReason::NoWork), "{reason:?}");
}

#[test]
fn an_instruction_of_the_engine_is_not_the_captain() {
    let mut store = world_with_captain();
    message(
        &mut store,
        MsgKind::Instruction,
        "engine",
        None,
        "Look at the quota line of the provider",
        None,
        T0 + 60,
    );

    assert_eq!(help_of(&mut store, T0 + 120), None);
}

#[test]
fn a_lane_with_an_attempt_in_flight_does_not_help() {
    let mut store = world_with_captain();
    task_on(&mut store, "T-2", LANE, 1, TaskState::Running);
    attempt(&mut store, "T-2", 1, AttemptState::Started);
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        None,
        "Look at the quota line of the provider",
        None,
        T0 + 60,
    );

    assert_eq!(help_of(&mut store, T0 + 120), None);
}

#[test]
fn a_project_without_a_captain_has_no_help() {
    let mut store = world();
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        None,
        "Look at the quota line of the provider",
        None,
        T0 + 60,
    );

    assert_eq!(help_of(&mut store, T0 + 120), None);
}

#[test]
fn an_instruction_sent_during_an_attempt_handed_in_later_is_no_help() {
    // T-463: the captain sent it while the lane worked on the task, and the
    // work went on past it; after the hand-in it does not pin the lane.
    let mut store = world_with_captain();
    task_on(&mut store, "T-2", LANE, 1, TaskState::Verifying);
    attempt(&mut store, "T-2", 1, AttemptState::Submitted);
    store
        .write(|tx| {
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::Started, T0)?;
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::HandedIn, T0 + 120)?;
            Ok(())
        })
        .unwrap();
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        Some("T-2"),
        "Read the log of T-2 and send mast the failing line please",
        None,
        T0 + 60,
    );

    assert_eq!(help_of(&mut store, T0 + 200), None);
    let (reason, _) = reason_of(&mut store, T0 + 200);
    assert_eq!(reason, Some(IdleReason::NoWork), "{reason:?}");
}

#[test]
fn an_instruction_sent_before_the_attempt_started_is_still_help() {
    // The lane held no attempt when the instruction came: it still has to
    // answer it until the reply or the limit.
    let mut store = world_with_captain();
    task_on(&mut store, "T-2", "mast", 1, TaskState::Verifying);
    attempt(&mut store, "T-2", 1, AttemptState::Submitted);
    store
        .write(|tx| {
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::Started, T0 + 90)?;
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::HandedIn, T0 + 150)?;
            Ok(())
        })
        .unwrap();
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        Some("T-2"),
        "Read the log of T-2 and send mast the failing line please",
        None,
        T0 + 60,
    );

    let help = help_of(&mut store, T0 + 200).expect("the lane helps");
    assert_eq!(help.text(), "helping mast: Read the log of T-2 and …");
    assert_eq!(help.since, T0 + 60);
}

#[test]
fn an_instruction_without_a_task_sent_during_a_handed_in_attempt_is_no_help() {
    // T-463 (B): any instruction the captain sent while this seat worked is
    // part of that work, whatever task it names.
    let mut store = world_with_captain();
    task_on(&mut store, "T-2", LANE, 1, TaskState::Verifying);
    attempt(&mut store, "T-2", 1, AttemptState::Submitted);
    store
        .write(|tx| {
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::Started, T0)?;
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::HandedIn, T0 + 120)?;
            Ok(())
        })
        .unwrap();
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        None,
        "Check the quota line",
        None,
        T0 + 60,
    );

    assert_eq!(help_of(&mut store, T0 + 200), None);
}

#[test]
fn an_instruction_sent_after_the_hand_in_is_still_help_after_settle_and_merge() {
    // F1 of the T-463 review: the work span ends at the earliest of hand-in,
    // settle and merge. An instruction the captain sent after the hand-in —
    // when the lane held no attempt — still pins the lane until its reply or
    // the limit, even after the attempt is settled and merged.
    let mut store = world_with_captain();
    task_on(&mut store, "T-2", LANE, 1, TaskState::Done);
    attempt(&mut store, "T-2", 1, AttemptState::Accepted);
    store
        .write(|tx| {
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::Started, T0)?;
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::HandedIn, T0 + 120)?;
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::Settled, T0 + 240)?;
            tx.set_attempt_time("p", "T-2", 1, gear_store::AttemptStep::Merged, T0 + 300)?;
            Ok(())
        })
        .unwrap();
    message(
        &mut store,
        MsgKind::Instruction,
        "p:captain",
        Some("T-2"),
        "Check the quota line",
        None,
        T0 + 180,
    );

    let help = help_of(&mut store, T0 + 400).expect("the lane still helps");
    assert_eq!(help.text(), "helping the captain: Check the quota line");
    assert_eq!(help.since, T0 + 180);
}
