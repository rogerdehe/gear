//! T-217: a lane restart carries its Delivered attempt to the new generation.
//!
//! A lane that is restarted (or retried) while its attempt waits for the lane
//! moves on with the attempt: the attempt's generation follows the seat's, so
//! the hand-in of the new generation is accepted. Without this the hand-in is
//! refused ("not owned by this seat and generation") and the attempt is stuck
//! until a human moves it by hand (M4 06:20, hull's T-175 rework).

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::Env;
use gear_model::{AttemptState, GearCtx, MsgKind, SeatRole, SeatState, TaskState};
use gear_store::{testing, BindingRow, BindingStatus, ContractRow, Store, TaskRow};

const T: i64 = 1_800_000_000;
const PANE: &str = "w1:p2";
const OTHER_PANE: &str = "w1:p3";

/// Project `p`: a captain and two lanes (`heat`, `deck`) at generation 5, each
/// with a Delivered attempt of its own.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T)?;
        testing::seed_active_seat(tx, "p", "deck", SeatRole::Lane, OTHER_PANE, "pi", T)?;
        for (seat, pane) in [("p:heat", PANE), ("p:deck", OTHER_PANE)] {
            tx.set_seat_generation(seat, 5)?;
            tx.insert_binding(&BindingRow {
                seat: seat.to_string(),
                generation: 5,
                session: "sess".to_string(),
                pane_id: pane.to_string(),
                kind: "pi".to_string(),
                session_ref: None,
                status: BindingStatus::Active,
                handover_id: None,
                created: T,
            })?;
        }
        for (task, lane) in [("T-1", "heat"), ("T-2", "deck")] {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: task.into(),
                rank: 1,
                state: TaskState::Ready,
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
                estimate_min: Some(30),
                issue: None,
                updated: T,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: task.into(),
                version: 1,
                goal: "goal".into(),
                accept: "accept".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T,
                mechanical: None,
            })?;
        }
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 5, T)?;
        gear_app::tasks::start_attempt(tx, "p", "T-2", "p:deck", 5, T)?;
        Ok(())
    })
    .unwrap();
    s
}

/// The captain's `- retry <lane>`: the lane goes back to `Queued`, so the
/// engine onboards it again — a new generation.
fn retry(store: &mut Store, seat: &str, now: i64) -> i64 {
    store
        .write(|tx| {
            tx.set_seat_state(seat, SeatState::Unknown, Some("lost"), now)?;
            tx.set_seat_state(seat, SeatState::Queued, Some("retried by the captain"), now)?;
            let generation =
                gear_app::seats::begin_onboard(tx, seat, PANE, "pi", now)?;
            gear_app::seats::finish_onboard(tx, seat, generation, None, now)?;
            Ok(generation)
        })
        .unwrap()
}

fn env(seat: &str, pane: &str, generation: i64) -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: seat.into(),
            generation,
        }),
        pane_id: Some(pane.into()),
        admin: false,
    }
}

fn fp(pane: &str) -> Fingerprint {
    Fingerprint {
        pane_id: pane.into(),
        kind: Some("pi".into()),
        session_ref: None,
    }
}

fn hand_in(task: &str, request_id: &str) -> SendArgs {
    SendArgs {
        kind: MsgKind::Result,
        body: "report".into(),
        task: Some(task.into()),
        attempt: Some(1),
        contract: Some(1),
        candidate: Some("abc123".into()),
        verdict: None,
        blocking: None,
        base: None,
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        request_id: request_id.into(),
    }
}

#[test]
fn a_retry_carries_the_delivered_attempt_to_the_new_generation() {
    let mut s = world();
    let generation = retry(&mut s, "p:heat", T + 1);
    assert_eq!(generation, 6, "the retry moves the seat to the next generation");
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.generation, 6, "the attempt moves with the seat");
    assert_eq!(attempt.state, AttemptState::Delivered);

    // The attempt of the other seat is untouched.
    let other = s.read(|tx| tx.get_attempt("p", "T-2", 1)).unwrap().unwrap();
    assert_eq!(other.generation, 5);
    assert_eq!(other.state, AttemptState::Delivered);

    // The hand-in of the new generation is accepted.
    mail::send(
        &mut s,
        &env("heat", PANE, 6),
        Some(&fp(PANE)),
        &hand_in("T-1", "r1"),
        T + 2,
    )
    .unwrap();
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Submitted);
    assert_eq!(attempt.generation, 6);
}

#[test]
fn a_hand_in_of_the_old_generation_is_still_refused() {
    // Without the carry the seat's attempt stays at the old generation and
    // the hand-in of the new generation is refused; the old generation is
    // refused by the seat's own generation, not by the attempt.
    let mut s = world();
    let generation = retry(&mut s, "p:heat", T + 1);
    assert_eq!(generation, 6);
    let error = mail::send(
        &mut s,
        &env("heat", PANE, 5),
        Some(&fp(PANE)),
        &hand_in("T-1", "r1"),
        T + 2,
    )
    .unwrap_err();
    assert_eq!(error.code, gear_model::ErrorCode::StaleGeneration);
}
