//! Contract for gear_app::tasks (T-42 start/confirm/submit, T-43 settle; design/06).

use gear_app::tasks::{on_delivery_confirmed, settle, start_attempt, submit_result, Verdict};
use gear_model::{AttemptState, ErrorCode, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

fn task(id: &str, state: TaskState, deps: &[&str]) -> TaskRow {
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
        lane: Some("heat".into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: deps.iter().map(|d| d.to_string()).collect(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: None,
        issue: None,
        updated: T0,
    }
}

fn world(tasks: &[TaskRow]) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        for t in tasks {
            tx.insert_task(t)?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: t.id.clone(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
        }
        Ok(())
    })
    .unwrap();
    s
}

fn task_state(s: &mut Store, id: &str) -> TaskState {
    s.read(|tx| tx.get_task("p", id)).unwrap().unwrap().state
}
fn attempt_state(s: &mut Store, id: &str, n: i64) -> AttemptState {
    s.read(|tx| tx.get_attempt("p", id, n))
        .unwrap()
        .unwrap()
        .state
}

/// T-1 Ready → attempt 1 Delivered → Started → Submitted (candidate "c1").
fn submitted() -> Store {
    let mut s = world(&[task("T-1", TaskState::Ready, &[])]);
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0 + 1)?;
        submit_result(tx, "p", "T-1", 1, 1, "c1", "report", T0 + 2)
    })
    .unwrap();
    s
}

fn verdict(pass: bool) -> Verdict {
    Verdict {
        project: "p".into(),
        task: "T-1".into(),
        attempt: 1,
        contract_version: 1,
        candidate: "c1".into(),
        pass,
        evidence: pass.then(|| "logs/v.md".to_string()),
        rework: (!pass).then(|| "edge cases".to_string()),
        integrated_by: None,
        by_seat: "p:captain".into(),
    }
}

#[test]
fn attempt_lifecycle_to_verifying() {
    let mut s = submitted();
    assert_eq!(task_state(&mut s, "T-1"), TaskState::Verifying);
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(
        (
            a.state,
            a.candidate.as_deref(),
            a.report.as_deref(),
            a.adopted_version
        ),
        (AttemptState::Submitted, Some("c1"), Some("report"), 1)
    );
}

#[test]
fn start_preconditions() {
    let mut s = world(&[
        task("T-1", TaskState::Ready, &["T-2"]),
        task("T-2", TaskState::Ready, &[]),
        task("T-3", TaskState::Draft, &[]),
        task("T-4", TaskState::Ready, &[]),
    ]);
    let code = |s: &mut Store, t: &str, gen: i64| {
        s.write(|tx| start_attempt(tx, "p", t, "p:heat", gen, T0).map(|_| ()))
            .unwrap_err()
            .code
    };
    assert_eq!(code(&mut s, "T-1", 1), ErrorCode::Forbidden, "dep not done");
    assert_eq!(code(&mut s, "T-3", 1), ErrorCode::Forbidden, "not ready");
    assert_eq!(
        code(&mut s, "T-2", 2),
        ErrorCode::Forbidden,
        "stale generation"
    );
    s.write(|tx| start_attempt(tx, "p", "T-2", "p:heat", 1, T0).map(|_| ()))
        .unwrap();
    assert_eq!(
        code(&mut s, "T-4", 1),
        ErrorCode::Forbidden,
        "seat already has an in-progress attempt"
    );
    assert_eq!(code(&mut s, "T-9", 1), ErrorCode::NoTarget);
}

#[test]
fn submit_must_name_the_adopted_contract() {
    let mut s = world(&[task("T-1", TaskState::Ready, &[])]);
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    assert_eq!(
        s.write(|tx| submit_result(tx, "p", "T-1", 1, 2, "c", "r", T0))
            .unwrap_err()
            .code,
        ErrorCode::BadFormat
    );
    assert_eq!(attempt_state(&mut s, "T-1", 1), AttemptState::Started);
}

#[test]
fn pass_and_fail_settle() {
    let mut s = submitted();
    s.write(|tx| settle(tx, &verdict(true), T0 + 3)).unwrap();
    assert_eq!(
        (task_state(&mut s, "T-1"), attempt_state(&mut s, "T-1", 1)),
        (TaskState::Done, AttemptState::Accepted)
    );
    s.write(|tx| settle(tx, &verdict(true), T0 + 4)).unwrap();
    assert_eq!(
        s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap().len(),
        1,
        "same verdict again writes nothing"
    );
    // A pass then a fail on the same attempt: the captain reopens the task
    // (T-90 — its merge failed). The accepted attempt keeps its state, the
    // task goes back for a rework.
    s.write(|tx| settle(tx, &verdict(false), T0 + 5)).unwrap();
    assert_eq!(
        (task_state(&mut s, "T-1"), attempt_state(&mut s, "T-1", 1)),
        (TaskState::Ready, AttemptState::Accepted)
    );
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .rework_note
            .as_deref(),
        Some("edge cases")
    );

    let mut s = submitted();
    s.write(|tx| settle(tx, &verdict(false), T0 + 3)).unwrap();
    assert_eq!(
        (task_state(&mut s, "T-1"), attempt_state(&mut s, "T-1", 1)),
        (TaskState::Ready, AttemptState::Rejected)
    );
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .rework_note
            .as_deref(),
        Some("edge cases")
    );
    // A fail then a pass on one attempt is still a conflict: the rework is a
    // new attempt, never a second verdict on the settled one.
    assert_eq!(
        s.write(|tx| settle(tx, &verdict(true), T0 + 4))
            .unwrap_err()
            .code,
        ErrorCode::BadTransition
    );
}

#[test]
fn verdict_must_match_the_attempt() {
    let mut s = submitted();
    for bad in [
        Verdict {
            candidate: "other".into(),
            ..verdict(true)
        },
        Verdict {
            contract_version: 2,
            ..verdict(true)
        },
        Verdict {
            evidence: None,
            ..verdict(true)
        },
        Verdict {
            rework: None,
            ..verdict(false)
        },
    ] {
        assert_eq!(
            s.write(|tx| settle(tx, &bad, T0 + 3)).unwrap_err().code,
            ErrorCode::BadFormat,
            "{bad:?}"
        );
    }
    assert_eq!(attempt_state(&mut s, "T-1", 1), AttemptState::Submitted);
}
