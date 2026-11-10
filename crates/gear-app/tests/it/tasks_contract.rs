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

/// T-457's candidate, as it was stored with an 8-character prefix.
const FULL_SHA: &str = "7c9be5e8f1a2b3c4d5e6f708192a3b4c5d6e7f80";
const SHA_PREFIX: &str = "7c9be5e8";

/// T-1 Ready → attempt 1 Submitted under `candidate`.
fn submitted_with(candidate: &str) -> Store {
    let mut s = world(&[task("T-1", TaskState::Ready, &[])]);
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0 + 1)?;
        submit_result(tx, "p", "T-1", 1, 1, candidate, "report", T0 + 2)
    })
    .unwrap();
    s
}

/// The verdict of the fixture, on another candidate.
fn verdict_on(candidate: &str, pass: bool) -> Verdict {
    Verdict {
        candidate: candidate.into(),
        ..verdict(pass)
    }
}

/// A verdict names its candidate by any sha prefix of the stored one, or the
/// stored one by a prefix of it (M7 backlog, T-457).
#[test]
fn a_verdict_names_its_candidate_by_any_sha_prefix() {
    // The full sha settles the attempt stored with its 8-character prefix.
    let mut s = submitted_with(SHA_PREFIX);
    s.write(|tx| settle(tx, &verdict_on(FULL_SHA, false), T0 + 3))
        .unwrap();
    assert_eq!(
        (task_state(&mut s, "T-1"), attempt_state(&mut s, "T-1", 1)),
        (TaskState::Ready, AttemptState::Rejected)
    );
    // The reverse: the prefix settles the attempt stored with the full sha.
    let mut s = submitted_with(FULL_SHA);
    s.write(|tx| settle(tx, &verdict_on(SHA_PREFIX, false), T0 + 3))
        .unwrap();
    assert_eq!(
        (task_state(&mut s, "T-1"), attempt_state(&mut s, "T-1", 1)),
        (TaskState::Ready, AttemptState::Rejected)
    );
}

/// Six shared characters are not a prefix: the verdict is refused.
#[test]
fn a_candidate_that_shares_six_characters_is_refused() {
    let mut s = submitted_with(FULL_SHA);
    for bad in ["7c9be5", "7c9be5e9"] {
        assert_eq!(
            s.write(|tx| settle(tx, &verdict_on(bad, true), T0 + 3))
                .unwrap_err()
                .code,
            ErrorCode::BadFormat,
            "{bad}"
        );
    }
    assert_eq!(attempt_state(&mut s, "T-1", 1), AttemptState::Submitted);
}

/// A verdict replayed with the other length is the same verdict: it settles
/// nothing.
#[test]
fn a_verdict_replayed_with_the_other_length_changes_nothing() {
    let mut s = submitted_with(FULL_SHA);
    s.write(|tx| settle(tx, &verdict_on(FULL_SHA, true), T0 + 3))
        .unwrap();
    let before = (
        task_state(&mut s, "T-1"),
        attempt_state(&mut s, "T-1", 1),
        s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap().len(),
    );
    s.write(|tx| settle(tx, &verdict_on(SHA_PREFIX, true), T0 + 4))
        .unwrap();
    assert_eq!(
        (
            task_state(&mut s, "T-1"),
            attempt_state(&mut s, "T-1", 1),
            s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap().len(),
        ),
        before,
        "the replay changes nothing"
    );
}

/// A rework hand-in of the candidate that failed under a prefix is the same
/// candidate: it is refused.
#[test]
fn a_hand_in_of_the_failed_candidate_by_its_full_sha_is_refused() {
    let mut s = submitted_with(SHA_PREFIX);
    s.write(|tx| settle(tx, &verdict_on(FULL_SHA, false), T0 + 3))
        .unwrap();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 4)?;
        on_delivery_confirmed(tx, "p", "T-1", 2, T0 + 5)
    })
    .unwrap();
    assert_eq!(
        s.write(|tx| submit_result(tx, "p", "T-1", 2, 1, FULL_SHA, "report", T0 + 6))
            .unwrap_err()
            .code,
        ErrorCode::BadFormat,
        "the full sha of the failed candidate"
    );
    assert_eq!(attempt_state(&mut s, "T-1", 2), AttemptState::Started);
}

/// T-635: past the rework cap (3 by default) a plain fail is refused; only
/// the captain's ruling sends the task back, and it heads the rework note.
#[test]
fn past_the_rework_cap_only_a_ruling_sends_a_task_back() {
    let mut s = world(&[task("T-1", TaskState::Ready, &[])]);
    let fail = |n: i64, rework: &str| Verdict {
        attempt: n,
        candidate: format!("c{n}"),
        rework: Some(rework.to_string()),
        ..verdict(false)
    };
    for n in 1..=4 {
        s.write(|tx| {
            start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + n * 10)?;
            on_delivery_confirmed(tx, "p", "T-1", n, T0 + n * 10 + 1)?;
            submit_result(
                tx,
                "p",
                "T-1",
                n,
                1,
                &format!("c{n}"),
                "report",
                T0 + n * 10 + 2,
            )
        })
        .unwrap();
        if n <= 3 {
            s.write(|tx| settle(tx, &fail(n, "edge cases"), T0 + n * 10 + 3))
                .unwrap();
        }
    }
    let refused = s
        .write(|tx| settle(tx, &fail(4, "edge cases"), T0 + 50))
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::BadFormat);
    assert!(
        refused
            .detail
            .contains("sent back 4 times, over the cap of 3")
            && refused.detail.contains("ruling="),
        "{}",
        refused.detail
    );
    assert_eq!(attempt_state(&mut s, "T-1", 4), AttemptState::Submitted);
    let ruling = format!(
        "{}the reviewer asked twice for X; do only X",
        gear_app::tasks::RULING
    );
    s.write(|tx| settle(tx, &fail(4, &ruling), T0 + 51))
        .unwrap();
    assert_eq!(
        (task_state(&mut s, "T-1"), attempt_state(&mut s, "T-1", 4)),
        (TaskState::Ready, AttemptState::Rejected)
    );
    let note = s
        .read(|tx| tx.get_task("p", "T-1"))
        .unwrap()
        .unwrap()
        .rework_note
        .unwrap();
    assert!(
        note.starts_with("Captain's ruling.") && note.ends_with("do only X"),
        "{note}"
    );
}
