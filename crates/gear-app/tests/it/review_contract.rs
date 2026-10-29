//! Contract for `gear send --kind review` (T-2): a temp reviewer hands in a
//! verdict on one attempt. The verdict is stored on the attempt's review
//! record; fails are actionable and passes are information. Sending settles nothing.

use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::review::Outcome;
use gear_app::tasks::{on_delivery_confirmed, start_attempt};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, RequestOutcome, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const TEMP_PANE: &str = "w1:p3";

/// Project `p`: a captain, the lane that wrote T-1 and a temp reviewer bound
/// to T-1. T-1 has one started attempt.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        testing::seed_temp_seat(tx, "p", "r-1", "T-1", TEMP_PANE, "opencode", T0)?;
        tx.insert_task(&TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: "T-1".into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
            lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![], followup_of: None,
            wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: T0 })?;
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-1".into(), version: 1, goal: "g".into(), accept: "a".into(),
            evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0, mechanical: None, })?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    }).unwrap();
    s
}

fn env(seat: &str, pane: &str) -> Env {
    Env { ctx: Some(GearCtx { project: "p".into(), seat: seat.into(), generation: 1 }), pane_id: Some(pane.into()), admin: false }
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint { pane_id: pane.into(), kind: Some(kind.into()), session_ref: None }
}

fn review(outcome: Outcome, findings: &str, rid: &str) -> SendArgs {
    SendArgs { kind: MsgKind::Review, body: findings.into(), task: Some("T-1".into()), attempt: Some(1),
        contract: None, candidate: None, verdict: Some(outcome), blocking: None, base: None, about: None, to: None, reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None, request_id: rid.into() }
}

#[test]
fn a_temp_review_is_stored_and_reaches_the_captain() {
    let mut s = world();

    let sent = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &review(Outcome::Pass, "P1: the row drifts right", "r1"), T0 + 5).unwrap();
    assert_eq!(sent.recipient, "p:captain");

    // The verdict sits on the attempt's review record.
    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.passed);
    assert_eq!(record.findings, "P1: the row drifts right");
    assert_eq!(record.by_seat, "p:r-1");

    // A passing review is retained without an action item or home-page work.
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
    let message = s.read(|tx| tx.get_message("p", &sent.msg_id)).unwrap().unwrap();
    assert_eq!((message.kind, message.state), (MsgKind::Review, MsgState::Info));
    assert!(message.body.contains("pass") && message.body.contains("P1: the row drifts right"));
    let home = home::build(&mut s, &env("captain", CAP_PANE), Some(&fp(CAP_PANE, "codex")), T0 + 7).unwrap();
    assert!(!home.text.contains("INBOX"), "{}", home.text);

    // The task packet shows the review record.
    let packet = s.read(|tx| gear_app::plan::render_task_packet(tx, "p", "T-1")).unwrap();
    assert!(packet.contains("## Review"), "{packet}");
    assert!(packet.contains("pass"), "{packet}");
    assert!(packet.contains("P1: the row drifts right"), "{packet}");

    // Nothing is settled: the attempt and the task keep their state.
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Started);
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state, TaskState::Running);
}

#[test]
fn a_fail_verdict_is_stored_as_a_fail() {
    let mut s = world();
    mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &review(Outcome::Fail, "P1: the store forgets the verdict", "r1"), T0 + 5).unwrap();
    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(!record.passed);
    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap();
    assert!(inbox[0].body.contains("fail"), "{}", inbox[0].body);
}

#[test]
fn a_lane_seat_may_not_hand_in_a_verdict() {
    let mut s = world();
    let err = mail::send(&mut s, &env("heat", LANE_PANE), Some(&fp(LANE_PANE, "opencode")),
        &review(Outcome::Pass, "looks fine to me", "r1"), T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
    // Nothing was written: no record, no item in the captain's inbox.
    assert!(s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().is_none());
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
}

#[test]
fn a_temp_seat_bound_to_another_task_may_not_review() {
    let mut s = world();
    // The reviewer of T-1 names a task it is not bound to.
    let mut args = review(Outcome::Pass, "looks fine to me", "r1");
    args.task = Some("T-2".into());
    let err = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &args, T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);
    assert!(s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().is_none());
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
}

#[test]
fn a_review_of_an_unknown_task_is_no_target() {
    let mut s = world();
    let mut args = review(Outcome::Pass, "looks fine to me", "r1");
    args.task = Some("T-9".into());
    let err = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &args, T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget);
}

#[test]
fn a_review_without_a_verdict_is_bad_format() {
    let mut s = world();
    let mut args = review(Outcome::Pass, "looks fine to me", "r1");
    args.verdict = None;
    let err = mail::send(&mut s, &env("r-1", TEMP_PANE), Some(&fp(TEMP_PANE, "opencode")),
        &args, T0 + 5).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("--verdict"), "{}", err.detail);
}

#[test]
fn the_same_request_id_replays_without_a_second_record() {
    let mut s = world();
    let (env, fp) = (env("r-1", TEMP_PANE), fp(TEMP_PANE, "opencode"));
    let args = review(Outcome::Pass, "P1: the row drifts right", "r1");
    mail::send(&mut s, &env, Some(&fp), &args, T0 + 5).unwrap();
    let again = mail::send(&mut s, &env, Some(&fp), &args, T0 + 6).unwrap();
    assert!(again.replayed);
    assert_eq!(s.read(|tx| tx.review_verdicts_of_task("p", "T-1")).unwrap().len(), 1);
}

#[test]
fn a_second_different_verdict_for_the_same_attempt_is_refused() {
    let mut s = world();
    let (env, fp) = (env("r-1", TEMP_PANE), fp(TEMP_PANE, "opencode"));
    mail::send(&mut s, &env, Some(&fp), &review(Outcome::Pass, "P1: the row drifts right", "r1"), T0 + 5).unwrap();
    let err = mail::send(&mut s, &env, Some(&fp), &review(Outcome::Fail, "P1: wrong", "r2"), T0 + 6).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadTransition);
    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.passed, "the first verdict stands");
}

/// A temp findings file, as a reviewer's worktree would hold.
fn findings_file(text: &str, tag: &str) -> std::path::PathBuf {
    let path = crate::temp("gear-findings").join(format!("{tag}.md"));
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn a_findings_path_is_stored_as_its_text() {
    let mut s = world();
    let file = findings_file("# Findings\n\nP1: the row drifts right.\n", "path");

    // The lane handed the work in: the attempt is Submitted, the task
    // Verifying — a review is about a handed-in attempt.
    s.write(|tx| gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "c1", "Report: c1", T0 + 4))
        .unwrap();

    // The finding blocks (T-23), so the fail is a rework — a fail that names
    // no blocking finding goes to the captain and settles nothing (M-295).
    let mut args = review(Outcome::Fail, file.to_str().unwrap(), "r-post");
    args.blocking = Some("P1".into());
    mail::send(
        &mut s,
        &env("r-1", TEMP_PANE),
        Some(&fp(TEMP_PANE, "opencode")),
        &args,
        T0 + 5,
    )
    .unwrap();

    // The store holds the file's text, and the rework note and the packet
    // carry the findings — not a path (T-21).
    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.findings.contains("P1: the row drifts right"), "{}", record.findings);
    assert!(
        !record.findings.contains(&file.display().to_string()),
        "not the path: {}",
        record.findings
    );

    s.write(|tx| gear_app::tasks::settle_review(tx, "p", "T-1", 1, T0 + 6)).unwrap();
    let note = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().rework_note.unwrap_or_default();
    assert!(note.contains("P1: the row drifts right"), "{note}");
    let packet = s
        .read(|tx| gear_app::plan::render_task_packet(tx, "p", "T-1"))
        .unwrap();
    assert!(packet.contains("P1: the row drifts right"), "{packet}");

    std::fs::remove_file(&file).ok();
}

#[test]
fn a_findings_path_that_does_not_exist_stores_nothing() {
    let mut s = world();
    let before = s.read(|tx| tx.review_verdicts_of_task("p", "T-1")).unwrap().len();

    let err = mail::send(
        &mut s,
        &env("r-1", TEMP_PANE),
        Some(&fp(TEMP_PANE, "opencode")),
        &review(Outcome::Fail, "ops/reviews/T-404.md", "r-missing"),
        T0 + 5,
    )
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    let after = s.read(|tx| tx.review_verdicts_of_task("p", "T-1")).unwrap().len();
    assert_eq!(before, after, "nothing is stored");
}

/// The review of T-21 (F1): a path that names no file is refused whatever it
/// looks like — with a space in its last part, without an extension, written
/// as an explicit path — and nothing is stored: no verdict, no message, no
/// request row.
#[test]
fn a_missing_path_is_refused_whatever_it_looks_like() {
    for (arg, tag) in [
        ("ops/reviews/T-1 draft.md", "spaces"),
        ("./ops/reviews/T-1", "no-ext"),
        ("/tmp/gear-t21/findings", "absolute"),
    ] {
        let mut s = world();
        let request_id = format!("r-missing-{tag}");

        let err = mail::send(
            &mut s,
            &env("r-1", TEMP_PANE),
            Some(&fp(TEMP_PANE, "opencode")),
            &review(Outcome::Fail, arg, &request_id),
            T0 + 5,
        )
        .unwrap_err();

        assert_eq!(err.code, ErrorCode::BadFormat, "{arg}: {err:?}");
        assert!(err.detail.contains("no findings file"), "{}", err.detail);

        // No verdict and no message ...
        assert_eq!(
            s.read(|tx| tx.review_verdicts_of_task("p", "T-1")).unwrap().len(),
            0,
            "{arg}: a verdict was stored"
        );
        assert_eq!(
            s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 9)).unwrap().len(),
            0,
            "{arg}: a message was stored"
        );
        // ... and no request row: the same request id is still free (a stored
        // row with another key would be a conflict, not a fresh request).
        let outcome = s
            .read(|tx| {
                tx.run_request(&request_id, "another-key", T0 + 6, |_tx| Ok(serde_json::json!({})))
            })
            .unwrap();
        assert!(
            matches!(outcome, RequestOutcome::Executed(_)),
            "{arg}: a request row was stored"
        );
    }
}

/// The review of T-21 (F1): the file that *does* exist is still read, spaces
/// in its name included.
#[test]
fn an_existing_file_with_a_space_in_its_name_is_stored_as_its_text() {
    let mut s = world();
    let path = crate::temp("gear-findings").join("draft.md");
    std::fs::write(&path, "# Findings\n\nP1: the header count is stale.\n").unwrap();

    mail::send(
        &mut s,
        &env("r-1", TEMP_PANE),
        Some(&fp(TEMP_PANE, "opencode")),
        &review(Outcome::Fail, path.to_str().unwrap(), "r-spaces"),
        T0 + 5,
    )
    .unwrap();

    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.findings.contains("P1: the header count is stale"), "{}", record.findings);
    assert!(!record.findings.contains(".md"), "not the path: {}", record.findings);

    std::fs::remove_file(&path).ok();
}

/// The review of T-21 (F2): a repeated request id replays the stored result
/// and never touches the reviewer's file again — the file may be gone.
#[test]
fn a_repeated_request_id_replays_even_when_the_file_is_gone() {
    let mut s = world();
    let file = findings_file("# Findings\n\nP1: the header count is stale.\n", "replay");
    let args = review(Outcome::Fail, file.to_str().unwrap(), "r-replay");

    let first = mail::send(
        &mut s,
        &env("r-1", TEMP_PANE),
        Some(&fp(TEMP_PANE, "opencode")),
        &args,
        T0 + 5,
    )
    .unwrap();
    assert!(!first.replayed);

    // The worktree is gone (or the reviewer cleaned up) and the send is
    // repeated with the same request id: the stored result comes back.
    std::fs::remove_file(&file).ok();
    let again = mail::send(
        &mut s,
        &env("r-1", TEMP_PANE),
        Some(&fp(TEMP_PANE, "opencode")),
        &args,
        T0 + 6,
    )
    .unwrap();
    assert!(again.replayed, "the retry replays the stored result");
    assert_eq!(again.msg_id, first.msg_id);

    // Exactly one verdict and one message.
    assert_eq!(s.read(|tx| tx.review_verdicts_of_task("p", "T-1")).unwrap().len(), 1);
    let inbox = s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 9)).unwrap();
    assert_eq!(inbox.len(), 1, "{inbox:?}");
}

/// The review of T-21 (F3): the file is read bounded — at most 64 KiB are
/// read — and a longer file is cut with a note.
#[test]
fn a_huge_findings_file_is_cut_with_a_note() {
    let mut s = world();
    let path = crate::temp("gear-findings").join("huge.md");
    let big = "x".repeat(gear_app::review::FINDINGS_MAX + 4096);
    std::fs::write(&path, &big).unwrap();

    mail::send(
        &mut s,
        &env("r-1", TEMP_PANE),
        Some(&fp(TEMP_PANE, "opencode")),
        &review(Outcome::Fail, path.to_str().unwrap(), "r-huge"),
        T0 + 5,
    )
    .unwrap();

    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.findings.starts_with(&"x".repeat(64)), "the file's first bytes are stored");
    assert!(
        record.findings.len() > gear_app::review::FINDINGS_MAX,
        "the stored text is the cut plus the note: {} bytes",
        record.findings.len()
    );
    assert!(
        record.findings.contains(&format!("findings cut at {} bytes", gear_app::review::FINDINGS_MAX)),
        "{}",
        &record.findings[record.findings.len().saturating_sub(120)..]
    );

    std::fs::remove_file(&path).ok();
}
