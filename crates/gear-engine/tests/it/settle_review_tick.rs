//! Contract for the tick's review settlement (T-13): a review verdict that
//! arrived settles its task in the engine tick — no captain action is needed.

use gear_app::tasks::{on_delivery_confirmed, start_attempt, submit_result};
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::{builtin, AgentStatus};
use gear_model::{AttemptState, MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, NewMessage, ReviewVerdictRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// Project `p` with a captain, the lane `heat`, T-1 handed in and its review
/// (a pass) waiting in the store and in the captain's inbox.
fn world() -> Store {
    world_verdict(true)
}

/// The same world with the given verdict.
fn world_verdict(passed: bool) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        tx.insert_task(&TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: "T-1".into(),
            rank: 1,
            state: TaskState::Ready,
            priority: "must".into(),
            lane: Some("heat".into()),
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
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "g".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
            mechanical: None,
        })?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: r", T0 + 1)?;
        tx.insert_review_verdict(&ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed,
            findings: "ops/reviews/T-1.md".into(),
            by_seat: "p:r-1".into(),
            created: T0 + 2,
        })?;
        tx.insert_message(&NewMessage {
            meta: None,
            project: "p".into(),
            thread: "T-1:1".into(),
            task: Some("T-1".into()),
            attempt: Some(1),
            sender: "p:r-1".into(),
            sender_gen: 1,
            recipient: "p:captain".into(),
            kind: MsgKind::Review,
            urgent: false,
            body: "review pass on T-1 attempt 1\nops/reviews/T-1.md".into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: T0 + 2,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn a_review_verdict_settles_the_task_in_the_tick() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    // The lane's agent is in its pane (a missing agent alerts the captain, L-56).
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Working);

    let report = tick(&mut s, &mut herdr, &auto(), T0 + 10).unwrap();
    assert_eq!(report.settled, vec!["p:T-1/1".to_string()], "{report:?}");

    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Done
    );
    // The merge waits for the engine's merge op executor (T-12).
    let ops = s
        .read(|tx| tx.ops_in_state("p", OpState::Intended))
        .unwrap();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(
        (ops[0].kind, ops[0].task.as_deref()),
        (OpKind::Merge, Some("T-1"))
    );

    // The captain was not asked to settle anything: the review item is
    // closed, one line tells it. The keep-lanes-fed watch may still ask for
    // work — that is the never-stop warning, not a review item.
    let actionable = s
        .read(|tx| tx.actionable_for("p", "p:captain", T0 + 20))
        .unwrap();
    assert!(
        actionable
            .iter()
            .all(|message| message.thread.starts_with("lane:")),
        "no review item is left for the captain: {actionable:?}"
    );
    let info = s
        .read(|tx| tx.messages_to_since("p", "p:captain", 0))
        .unwrap();
    assert!(
        info.is_empty(),
        "settlement information must not enter the unshown reply queue: {info:?}"
    );

    // A second tick settles nothing again.
    let report = tick(&mut s, &mut herdr, &auto(), T0 + 30).unwrap();
    assert!(report.settled.is_empty(), "{report:?}");
    assert_eq!(
        s.read(|tx| tx.ops_in_state("p", OpState::Intended))
            .unwrap()
            .len(),
        1,
        "no second merge op"
    );
}

// ---------------------------------------------------------------- rework R-67

const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const BOARD_PANE: &str = "w1:p3";

fn cfg() -> EngineCfg {
    let mut c = EngineCfg {
        db_path: crate::temp("gear-engine-tests")
            .join("gear.db")
            .to_string_lossy()
            .into_owned(),
        auto_settle: true,
        ..Default::default()
    };
    let mut profile = builtin("opencode").unwrap();
    profile.new_session.clear();
    c.profiles.insert("opencode".into(), profile);
    c
}

fn herdr_world() -> FakeHerdr {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.add_agent(BOARD_PANE, "opencode", AgentStatus::Idle);
    herdr
}

fn set(w: &mut Store, id: &str, state: TaskState) {
    w.write(|tx| {
        let mut row = tx.get_task("p", id)?.unwrap();
        row.state = state;
        tx.update_task(&row)
    })
    .unwrap();
}

/// The review world plus a second ready task for the lane and a second lane
/// (`board`) with its own ready task, so a tick has unrelated work to do.
fn world_with_ready_work() -> Store {
    let mut s = world();
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "board", SeatRole::Lane, BOARD_PANE, "opencode", T0)?;
        for (id, lane, rank) in [("T-2", "heat", 2), ("T-3", "board", 3)] {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: id.into(),
                rank,
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
                estimate_min: None,
                issue: None,
                updated: T0,
            })?;
        }
        Ok(())
    })
    .unwrap();
    s
}

/// P1 (R-67): a review of a task cancelled mid-review must not fail the tick;
/// the attempt is closed, and unrelated ready work is dispatched in the same
/// tick — across a restart.
#[test]
fn a_cancelled_task_s_review_does_not_stop_the_tick() {
    let mut s = world_with_ready_work();
    let mut herdr = herdr_world();
    set(&mut s, "T-1", TaskState::Cancelled);

    // A restart: the engine recovers, then ticks.
    gear_engine::ops::recover_on_start(&mut s, T0 + 9).unwrap();
    let report = tick(&mut s, &mut herdr, &cfg(), T0 + 10).unwrap();
    assert!(report.settle_failed.is_empty(), "{report:?}");

    // The cancelled task keeps its state; its attempt is closed.
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Cancelled
    );
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Rejected
    );
    // Unrelated ready work was dispatched despite the obsolete review.
    let t2 = s.read(|tx| tx.get_attempt("p", "T-2", 1)).unwrap();
    assert!(t2.is_some(), "T-2 gets an attempt: {report:?}");
    assert_eq!(herdr.prompts_to(LANE_PANE).len(), 1, "T-2 is delivered");
}

/// P1 (R-67): a settlement that fails is reported and never aborts the tick;
/// another lane's ready work is still delivered in the same tick.
#[test]
fn a_failing_settlement_does_not_abort_the_tick() {
    let mut s = world_with_ready_work();
    let mut herdr = herdr_world();
    // A task in Waiting cannot be settled by a review (Waiting -> done is not
    // a legal move): the settlement fails.
    set(&mut s, "T-1", TaskState::Waiting);

    let report = tick(&mut s, &mut herdr, &cfg(), T0 + 10).unwrap();

    assert_eq!(report.settle_failed.len(), 1, "{report:?}");
    assert!(report.settle_failed[0].contains("T-1/1"), "{report:?}");
    assert!(
        report.settle_failed[0].contains("BAD_TRANSITION"),
        "{report:?}"
    );
    // The other lane's ready task is still dispatched.
    let t3 = s.read(|tx| tx.get_attempt("p", "T-3", 1)).unwrap();
    assert!(t3.is_some(), "the tick went on: {report:?}");
    assert_eq!(herdr.prompts_to(BOARD_PANE).len(), 1);
}

/// Automatic settlement is opt-in (L-57).
fn auto() -> EngineCfg {
    let mut cfg = EngineCfg {
        auto_settle: true,
        ..Default::default()
    };
    let mut profile = gear_herdr::builtin("opencode").unwrap();
    profile.new_session.clear();
    cfg.profiles.insert("opencode".into(), profile);
    cfg
}

#[test]
fn without_auto_settle_a_verdict_waits_for_the_captain() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Working);
    let report = tick(&mut s, &mut herdr, &EngineCfg::default(), T0 + 10).unwrap();
    assert!(report.settled.is_empty(), "{report:?}");
}

/// A failing verdict is never settled by the engine: the captain judges its
/// findings and writes the rework (L-57).
#[test]
fn a_failing_verdict_waits_for_the_captain_even_with_auto_settle() {
    let mut s = world_verdict(false);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Working);
    let report = tick(&mut s, &mut herdr, &auto(), T0 + 10).unwrap();
    assert!(report.settled.is_empty(), "{report:?}");
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Verifying
    );
}
