//! Contract for `gear_app::tasks::settle_review` (T-13): the reviewer's stored
//! verdict settles the task without the captain — a pass is done and queued for
//! the merge, a fail puts the rework first in the lane's queue, and the captain
//! gets an information line instead of an action item.

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::review::Outcome;
use gear_app::tasks::{on_delivery_confirmed, settle_review, start_attempt, submit_result};
use gear_app::Env;
use gear_model::{
    AttemptState, ErrorCode, GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState,
};
use gear_store::{testing, ContractRow, NewMessage, ReviewVerdictRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CANDIDATE: &str = "abc1234";
const FINDINGS: &str = "P1: the rows drift right on narrow screens\nP2: the header keeps a stale count";
/// Findings with ids, as the review packet asks for (T-23).
const FINDINGS_WITH_IDS: &str = "F1: the store forgets the verdict\nF2: the wording of a comment";

fn task(id: &str, rank: i64, state: TaskState) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state,
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
         issue: None,updated: T0,
    }
}

/// Project `p`: a captain, the lane `heat` and another lane `pricing`, three
/// queued tasks for `heat` (T-1 is handed in, T-2/T-3 are waiting) and the
/// review of T-1 attempt 1 in the captain's inbox. `passed` picks the verdict.
fn world(passed: bool) -> Store {
    world_with(passed, FINDINGS, "")
}

/// The world with the findings and the ids the reviewer named as blocking
/// (T-23).
fn world_with(passed: bool, findings: &str, blocking: &str) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        testing::seed_active_seat(tx, "p", "pricing", SeatRole::Lane, "w1:p4", "opencode", T0)?;
        testing::seed_temp_seat(tx, "p", "r-1", "T-1", "w1:p3", "opencode", T0)?;
        for row in [
            task("T-1", 7, TaskState::Ready),
            task("T-2", 8, TaskState::Ready),
            task("T-3", 10, TaskState::Ready),
            TaskRow { lane: Some("pricing".into()), ..task("T-4", 9, TaskState::Ready) },
        ] {
            tx.insert_task(&row)?;
        }
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
        // T-1 is handed in: Delivered → Started → Submitted, task Verifying.
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, CANDIDATE, "Report: ops/reports/T-1.md", T0 + 1)?;
        // What T-2 stores when the reviewer hands the verdict in.
        tx.insert_review_verdict(&ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed,
            findings: findings.into(),
            by_seat: "p:r-1".into(),
            created: T0 + 2,
        })?;
        if !blocking.is_empty() {
            tx.set_review_blocking("p", "T-1", 1, blocking, T0 + 2)?;
        }
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
            body: format!(
                "review {} on T-1 attempt 1\n{findings}",
                if passed { "pass" } else { "fail" }
            ),
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

fn task_state(s: &mut Store, id: &str) -> TaskState {
    s.read(|tx| tx.get_task("p", id)).unwrap().unwrap().state
}

fn attempt_state(s: &mut Store) -> AttemptState {
    s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state
}

fn ops(s: &mut Store) -> Vec<gear_store::OpRow> {
    s.read(|tx| tx.ops_in_state("p", OpState::Intended)).unwrap()
}

fn captain_inbox(s: &mut Store) -> Vec<gear_store::MessageRow> {
    s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 100)).unwrap()
}

fn settlement_info(s: &mut Store) -> Vec<gear_store::MessageRow> {
    s.read(|tx| Ok(tx.thread_messages("p", "T-1:1")?.into_iter()
        .chain(tx.thread_messages("p", "T-1:review")?)
        .filter(|m| m.kind == MsgKind::System && m.state == MsgState::Info)
        .collect())).unwrap()
}

#[test]
fn a_pass_settles_the_task_and_queues_the_merge() {
    let mut s = world(true);

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();

    // The attempt and the task are settled.
    assert_eq!(attempt_state(&mut s), AttemptState::Accepted);
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Done);

    // One merge op waits for the engine, with the branch and the candidate.
    let intended = ops(&mut s);
    assert_eq!(intended.len(), 1, "{intended:?}");
    let op = &intended[0];
    assert_eq!(
        (op.kind, op.state, op.task.as_deref(), op.attempt, op.seat.as_str()),
        (OpKind::Merge, OpState::Intended, Some("T-1"), Some(1), "p:heat")
    );
    assert_eq!(op.payload["branch"], serde_json::json!("heat/t1"));
    assert_eq!(op.payload["candidate"], serde_json::json!(CANDIDATE));

    // The review item is no longer an action item; the captain got one line.
    assert!(captain_inbox(&mut s).is_empty(), "no action item is left");
    let info = settlement_info(&mut s);
    assert_eq!(info.len(), 1, "{info:?}");
    assert_eq!((info[0].kind, info[0].state), (MsgKind::System, MsgState::Info));
    assert!(info[0].body.contains("pass"), "{}", info[0].body);
    assert!(info[0].body.contains("merge"), "{}", info[0].body);

    // The acceptance verdict is recorded as the task's history.
    let verdicts = s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap();
    assert_eq!(verdicts.len(), 1, "{verdicts:?}");
    assert_eq!(
        (verdicts[0].passed, verdicts[0].by_seat.as_str(), verdicts[0].candidate.as_str()),
        (true, "p:r-1", CANDIDATE)
    );
}

#[test]
fn a_fail_puts_the_rework_first_in_the_lane_queue() {
    let mut s = world_with(false, FINDINGS_WITH_IDS, "F1");

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();

    assert_eq!(attempt_state(&mut s), AttemptState::Rejected);
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Ready);
    // The rework note carries the blocking findings as text (T-23): the lane
    // reads it from GEAR, not from a review branch (T-21).
    let row = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    let note = row.rework_note.unwrap_or_default();
    assert!(note.contains("F1: the store forgets the verdict"), "{note}");
    assert!(!note.contains("F2"), "the logged finding is not a rework: {note}");
    assert!(!note.contains("git show origin/review"), "no pointer at a review branch: {note}");
    // The task packet shows the findings of the latest review.
    let packet = s.read(|tx| gear_app::plan::render_task_packet(tx, "p", "T-1")).unwrap();
    assert!(packet.contains("F1: the store forgets the verdict"), "the packet carries the findings:\n{packet}");
    // The rework comes before the lane's other ready tasks.
    let ready: Vec<TaskRow> = s.read(|tx| tx.tasks_of_project("p")).unwrap();
    let other_heat: Vec<i64> = ready
        .iter()
        .filter(|t| t.id != "T-1")
        .filter(|t| t.lane.as_deref() == Some("heat"))
        .filter(|t| t.state == TaskState::Ready)
        .map(|t| t.rank)
        .collect();
    assert!(!other_heat.is_empty(), "the test world has other ready work");
    assert!(row.rank < *other_heat.iter().min().unwrap(), "{} vs {other_heat:?}", row.rank);
    // No merge op for a fail.
    assert!(ops(&mut s).is_empty());
    // Same line to the captain, no action item.
    assert!(captain_inbox(&mut s).is_empty());
    let info = settlement_info(&mut s);
    assert_eq!(info.len(), 1, "{info:?}");
    assert!(info[0].body.contains("fail"), "{}", info[0].body);
}

#[test]
fn a_second_call_is_a_no_op() {
    let mut s = world(true);
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();
    let merge_ops = ops(&mut s).len();
    let info = settlement_info(&mut s).len();
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();

    // Again, later: nothing happens.
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 9)).unwrap();

    assert_eq!(ops(&mut s).len(), merge_ops, "no second merge op");
    assert_eq!(
        settlement_info(&mut s).len(),
        info,
        "no second information line"
    );
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap(), task);
}

#[test]
fn a_missing_verdict_is_no_target() {
    let mut s = world(true);
    let err = s.write(|tx| settle_review(tx, "p", "T-2", 1, T0 + 3)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);
    assert!(err.detail.contains("review"), "{}", err.detail);
}

// ---------------------------------------------------------------- rework R-67

fn reviewer() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "r-1".into(), generation: 1 }),
            pane_id: Some("w1:p3".into()),
            admin: false,
        },
        Fingerprint { pane_id: "w1:p3".into(), kind: Some("opencode".into()), session_ref: None },
    )
}

/// A second review of T-1 attempt 1 through the public `mail::send` (T-2).
fn send_review(outcome: Outcome, rid: &str) -> SendArgs {
    SendArgs {
        kind: MsgKind::Review,
        body: FINDINGS.into(),
        task: Some("T-1".into()),
        attempt: Some(1),
        contract: None,
        candidate: None,
        verdict: Some(outcome),
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
        request_id: rid.into(),
    }
}

/// P1 (R-67): a pass that completes an *older* contract than the task carries
/// now must not mark the task done — it becomes a rework on the current one.
#[test]
fn a_pass_on_an_older_contract_is_a_rework() {
    let mut s = world(true);
    // The captain moved the contract to v2 after the attempt was submitted.
    s.write(|tx| {
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 2,
            goal: "g2".into(),
            accept: "a2".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0 + 3,
            mechanical: None,
        })?;
        let mut row = tx.get_task("p", "T-1")?.unwrap();
        row.contract_version = 2;
        row.updated = T0 + 3;
        tx.update_task(&row)
    })
    .unwrap();

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 5)).unwrap();

    // Not accepted: the attempt is rejected and the task waits for rework.
    assert_eq!(attempt_state(&mut s), AttemptState::Rejected);
    let row = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Ready);
    let rework = row.rework_note.unwrap_or_default();
    assert!(rework.contains("v1") && rework.contains("v2"), "{rework}");
    // No merge for a candidate that does not complete the current contract.
    assert!(ops(&mut s).is_empty());
    // The captain still gets one line, and no action item.
    assert!(captain_inbox(&mut s).is_empty());
    let info = settlement_info(&mut s);
    assert_eq!(info.len(), 1, "{info:?}");
    assert!(info[0].body.contains("v2"), "{}", info[0].body);
}

/// P1 (R-67): two review submissions for the same attempt must not leave an
/// action item behind; a late one is refused and creates none.
#[test]
fn duplicate_reviews_leave_no_action_item() {
    let mut s = world(true);
    let (env, fp) = reviewer();
    // The same review twice, through the public send path, with two ids; the
    // world already carries the first item T-2 wrote.
    mail::send(&mut s, &env, Some(&fp), &send_review(Outcome::Pass, "r-1"), T0 + 3).unwrap();
    mail::send(&mut s, &env, Some(&fp), &send_review(Outcome::Pass, "r-2"), T0 + 4).unwrap();
    assert_eq!(captain_inbox(&mut s).len(), 1, "only the legacy open review needs closing; new passes are info");

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 5)).unwrap();

    assert!(captain_inbox(&mut s).is_empty(), "every copy is resolved");
    assert_eq!(ops(&mut s).len(), 1, "still one merge");
    let info = settlement_info(&mut s);
    assert_eq!(info.len(), 1, "one information line");

    // A duplicate after the settlement is refused and creates no item.
    let err = mail::send(&mut s, &env, Some(&fp), &send_review(Outcome::Pass, "r-3"), T0 + 6)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadTransition);
    assert!(captain_inbox(&mut s).is_empty());
    assert_eq!(ops(&mut s).len(), 1);
}

/// P1 (R-67): a review that arrives after its task was cancelled closes the
/// attempt (so the lane is free) and settles nothing.
#[test]
fn an_obsolete_review_closes_the_attempt_and_settles_nothing() {
    let mut s = world(true);
    s.write(|tx| {
        let mut row = tx.get_task("p", "T-1")?.unwrap();
        row.state = TaskState::Cancelled;
        row.updated = T0 + 3;
        tx.update_task(&row)
    })
    .unwrap();

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 5)).unwrap();

    assert_eq!(attempt_state(&mut s), AttemptState::Rejected, "the attempt is closed");
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Cancelled, "the task keeps its state");
    assert!(ops(&mut s).is_empty(), "nothing is merged");
    assert!(captain_inbox(&mut s).is_empty(), "no action item");
    let info = settlement_info(&mut s);
    assert_eq!(info.len(), 1, "{info:?}");
    assert!(info[0].body.contains("cancelled"), "{}", info[0].body);
}

/// T-23 (M-295): a fail that names no blocking finding is never converted
/// into a pass — the captain decides and the task stays Verifying.
#[test]
fn a_fail_without_a_blocking_finding_goes_to_the_captain() {
    let mut s = world_with(false, FINDINGS_WITH_IDS, "");

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();

    assert_eq!(attempt_state(&mut s), AttemptState::Submitted, "nothing is settled");
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Verifying, "the task stays Verifying");
    assert!(ops(&mut s).is_empty(), "no merge, no rework");
    // One action item for the captain.
    let inbox = captain_inbox(&mut s);
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert!(inbox[0].body.contains("fail without blocking findings"), "{}", inbox[0].body);
    // A second settlement adds none.
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 4)).unwrap();
    assert_eq!(captain_inbox(&mut s).len(), 1, "one action item, not one per tick");
}

/// T-23 F1 (the rework of attempt 1): a fail whose findings report a failed
/// mechanical fact is never converted into a pass, even with no blocking ids:
/// the attempt stays handed in, the task stays Verifying, no merge is queued,
/// and the captain gets the item.
#[test]
fn a_fail_reporting_a_failed_mechanical_fact_is_never_a_pass() {
    let findings = "Mechanical acceptance: `cargo test --workspace` FAILED: a contract test fails";
    let mut s = world_with(false, findings, "");

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();

    assert_eq!(attempt_state(&mut s), AttemptState::Submitted, "nothing is settled");
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Verifying, "the task stays Verifying");
    assert!(ops(&mut s).is_empty(), "no merge for a candidate known to fail");
    let inbox = captain_inbox(&mut s);
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert!(inbox[0].body.contains("fail without blocking findings"), "{}", inbox[0].body);
}

/// T-23 F2 (the rework of attempt 1): a fail on an attempt that adopted an
/// older contract is not accepted either — the contract freshness applies to
/// every acceptance path, and here there is no acceptance at all: the task
/// stays Verifying and nothing merges the old candidate.
#[test]
fn a_fail_on_an_older_contract_is_never_a_pass() {
    let mut s = world_with(false, FINDINGS_WITH_IDS, "");
    // The captain moved the contract to v2 after the attempt was submitted.
    s.write(|tx| {
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 2,
            goal: "g2".into(),
            accept: "a2".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0 + 3,
            mechanical: None,
        })?;
        let mut row = tx.get_task("p", "T-1")?.unwrap();
        row.contract_version = 2;
        row.updated = T0 + 3;
        tx.update_task(&row)
    })
    .unwrap();

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 5)).unwrap();

    assert_eq!(attempt_state(&mut s), AttemptState::Submitted, "nothing is settled");
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Verifying);
    assert!(ops(&mut s).is_empty(), "the old candidate is not merged");
    let inbox = captain_inbox(&mut s);
    assert_eq!(inbox.len(), 1, "{inbox:?}");
}

/// T-23: the rework rounds are capped — past the cap the captain decides and
/// the task stays Verifying.
#[test]
fn the_rework_rounds_are_capped() {
    let mut s = world_with(false, FINDINGS_WITH_IDS, "F1");
    // Four rounds of rework: over the default cap of three.
    s.write(|tx| {
        for n in 2..=4 {
            tx.insert_attempt(&gear_store::AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Rejected,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
        }
        Ok(())
    })
    .unwrap();

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();

    // Nothing is settled: the task stays Verifying and the attempt handed in.
    assert_eq!(attempt_state(&mut s), AttemptState::Submitted);
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Verifying);
    assert!(ops(&mut s).is_empty(), "no merge, no rework");
    // One action item for the captain.
    let inbox = captain_inbox(&mut s);
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert!(inbox[0].body.contains("cap"), "{}", inbox[0].body);
    // A second settlement does not add another one.
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 4)).unwrap();
    assert_eq!(captain_inbox(&mut s).len(), 1, "one action item, not one per tick");
}

/// T-23: a restart between storing the review and settling it must not settle
/// twice: the second call changes nothing.
#[test]
fn a_restart_between_the_review_and_the_settlement_does_not_double_settle() {
    let mut s = world_with(false, FINDINGS_WITH_IDS, "F1");

    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();
    let merges = ops(&mut s).len();
    let notes = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().rework_note.clone();

    // The engine restarts and settles again.
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 400)).unwrap();

    assert_eq!(ops(&mut s).len(), merges, "no second merge");
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().rework_note.clone(),
        notes,
        "the same rework, not a second one"
    );
    let info = settlement_info(&mut s);
    assert_eq!(info.len(), 1, "one information line: {info:?}");
}

// ------------------------------------------------- rework budget (T-312)

/// A fail world whose T-1 carries `estimate_min` and whose review names
/// `count` blocking findings (`F1`, `F2`, …).
fn fail_world(estimate_min: Option<i64>, count: usize) -> Store {
    let findings: Vec<String> = (1..=count).map(|n| format!("F{n}: finding {n}")).collect();
    let blocking: Vec<String> = (1..=count).map(|n| format!("F{n}")).collect();
    let mut s = world_with(false, &findings.join("\n"), &blocking.join(","));
    s.write(|tx| {
        let mut row = tx.get_task("p", "T-1")?.unwrap();
        row.estimate_min = estimate_min;
        row.updated = T0 + 2;
        tx.update_task(&row)
    })
    .unwrap();
    s
}

/// The budget the next round of T-1 gets (§5.1, attempt 2 is a rework).
fn rework_budget(s: &mut Store) -> i64 {
    s.read(|tx| tx.round_budget_min("p", "T-1", 2, "rework", None)).unwrap()
}

/// T-312: a rework round's budget grows with the blocking findings it carries
/// — 8 minutes each, never below half the estimate at least 10.
#[test]
fn a_rework_budget_grows_with_its_blocking_findings() {
    let mut s = fail_world(Some(30), 5);
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();
    assert_eq!(rework_budget(&mut s), 40, "five blocking findings are 40 min");

    // One finding is below half of 30: the old rule still wins.
    let mut s = fail_world(Some(30), 1);
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();
    assert_eq!(rework_budget(&mut s), 15, "half of 30 is still the floor");

    // A long task keeps its half: two findings are 16, half of 100 is 50.
    let mut s = fail_world(Some(100), 2);
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();
    assert_eq!(rework_budget(&mut s), 50, "half of 100 beats 8 per finding");
}

/// T-312: a task without an estimate counts from the default round budget.
#[test]
fn a_rework_budget_without_an_estimate_starts_from_the_default() {
    let mut s = fail_world(None, 4);
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 3)).unwrap();
    // Half of the default 30 is 15; four findings are 32.
    assert_eq!(
        rework_budget(&mut s), 32,
        "eight minutes per finding, above half the default budget"
    );
}

/// T-312: a captain's fail verdict with `estimate=` still sets its own minutes
/// (plan.rs writes them after its settle).
#[test]
fn a_captains_fail_verdict_keeps_its_own_minutes() {
    let mut s = fail_world(Some(30), 2);
    let text = "<!-- gear-plan project=p base=0 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 fail estimate=25 rework=\"handle the edge case\"\n";
    let env = Env {
        ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some("w1:p1".into()),
        admin: false,
    };
    let fp = Fingerprint {
        pane_id: "w1:p1".into(),
        kind: Some("codex".into()),
        session_ref: None,
    };
    gear_app::plan::apply(
        &mut s,
        &env,
        Some(&fp),
        None,
        &gear_app::draft::parse(text).unwrap(),
        text,
        "t312-captain",
        T0 + 6,
    )
    .unwrap();

    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().rework_estimate_min,
        Some(25)
    );
    assert_eq!(rework_budget(&mut s), 25, "the captain's minutes win over 8 per finding");
}
