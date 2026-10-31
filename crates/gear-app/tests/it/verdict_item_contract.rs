//! T-90: a review fail is settled where it can be — by a `# Verdicts` line,
//! not by `gear reply … --resolve`.
//!
//! M3: the captain resolved two fails by reply; the items closed and nothing
//! was settled. The home page now names the line to write and a reply that
//! would close a fail is refused with that line. T-187: the plan that
//! settles the attempt closes the item with it, so the captain does not have
//! to resolve it by hand (M-2412).

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::mail::{self, ReplyArgs, ReplyMode};
use gear_app::plan::apply;
use gear_app::tasks::{on_delivery_confirmed, start_attempt, submit_result};
use gear_app::{home, Env};
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, NewMessage, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const CANDIDATE: &str = "abc1234def5678";

fn cap_env() -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some(CAP.into()),
        admin: false,
    }
}

fn fp() -> Fingerprint {
    Fingerprint {
        pane_id: CAP.into(),
        kind: Some("codex".into()),
        session_ref: None,
    }
}

fn apply_plan(
    s: &mut Store,
    text: &str,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let mut draft = parse(text).unwrap();
    // T-461: a first voyage's departure is the user's word.
    if draft.base == 0 {
        draft.confirmed = true;
        draft.decisions.push(
            "2026-10-27 10:05 confirmed: the voyage departs | why: the user said depart"
                .to_string(),
        );
    }
    apply(s, &cap_env(), Some(&fp()), None, &draft, text, rid, T0 + 50)
}

const V1: &str = "<!-- gear-plan project=p base=0 -->\n# Milestone M3: delivery\ndone: it ships\nbar: it holds\neta: 8h\n\n# Queue\n## NEW [must] lane=heat milestone=M3\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";

/// The line the plan writes to settle the fail.
const VERDICTS_LINE: &str = "T-1 attempt=1 contract=1 candidate=abc1234def5678 fail rework=\"…\"";

/// A project on M3 with T-1 handed in on lane heat and a **review item** in
/// the captain's inbox; `passed` says how the reviewer judged it.
fn world_with(passed: bool) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    apply_plan(&mut s, V1, "v1").unwrap();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, CANDIDATE, "ops/reports/T-1.md", T0)?;
        // The reviewer failed it: the verdict, the blocking finding and the
        // item in the captain's inbox (a review message, still open).
        tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed,
            findings: if passed {
                "all good".into()
            } else {
                "F1: the edge case is not handled".into()
            },
            by_seat: "p:review-1".into(),
            created: T0,
        })?;
        if !passed {
            tx.set_review_blocking("p", "T-1", 1, "F1", T0)?;
        }
        tx.insert_message(&NewMessage {
            meta: None,
            project: "p".into(),
            thread: "T-1:1".into(),
            task: Some("T-1".into()),
            attempt: Some(1),
            sender: "p:review-1".into(),
            sender_gen: 1,
            recipient: "p:captain".into(),
            kind: MsgKind::Review,
            urgent: false,
            body: if passed {
                "review pass on T-1 attempt 1\nall good".into()
            } else {
                "review fail on T-1 attempt 1\nF1: the edge case is not handled".into()
            },
            reply_to: None,
            needs_ack: false,
            // As `gear send` stores it: a pass is information, a fail is an
            // action item.
            state: if passed {
                MsgState::Info
            } else {
                MsgState::Open
            },
            created: T0,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

/// The review failed: the item needs a verdict.
fn world() -> Store {
    world_with(false)
}

fn resolve(s: &mut Store, id: &str) -> gear_model::GearResult<mail::ReplyResult> {
    mail::reply(
        s,
        &cap_env(),
        Some(&fp()),
        &ReplyArgs {
            ids: vec![id.into()],
            text: "done".into(),
            mode: ReplyMode::Resolve,
            wait_on: None,
            wait_until: None,
            downgrade: false,
            request_id: format!("r-{id}"),
        },
        T0 + 20,
    )
}

fn task_state(s: &mut Store) -> TaskState {
    s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state
}

#[test]
fn the_home_page_names_the_verdicts_line_not_a_reply() {
    let mut s = world();

    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 10).unwrap();

    let expected =
        format!("next: gear plan apply gear/voyages/M3/plan.md (# Verdicts: {VERDICTS_LINE})");
    assert!(
        page.text.contains(&expected),
        "{}\n---\nexpected: {expected}",
        page.text
    );
    // The review fail is not something a reply settles.
    assert!(
        !page.text.contains("gear reply M-1 --resolve"),
        "{}",
        page.text
    );
}

#[test]
fn resolving_the_fail_by_reply_is_refused_and_leaves_it_verifying() {
    let mut s = world();

    let error = resolve(&mut s, "M-1").unwrap_err();

    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error
            .detail
            .contains(&format!("# Verdicts: {VERDICTS_LINE}")),
        "the refusal names the line: {}",
        error.detail
    );
    // Nothing was settled and the item is still there.
    assert_eq!(task_state(&mut s), TaskState::Verifying);
    let item = s.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
    assert_eq!(item.state, MsgState::Open);
    assert!(s
        .read(|tx| tx.verdicts_of_task("p", "T-1"))
        .unwrap()
        .is_empty());
}

/// T-187: the verdict that settles the attempt closes the review item too:
/// the captain does not have to resolve it by hand (M-2412).
#[test]
fn after_the_plan_settles_it_the_item_is_closed() {
    let mut s = world();
    assert_eq!(
        resolve(&mut s, "M-1").unwrap_err().code,
        ErrorCode::BadFormat
    );

    // The captain writes the line the home page named.
    let text = format!(
        "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={CANDIDATE} fail rework=\"handle the edge case\"\n"
    );
    apply_plan(&mut s, &text, "v2").unwrap();
    // The verdict settled the attempt: the task is back in the lane's queue.
    assert_eq!(task_state(&mut s), TaskState::Ready);
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .rework_note
            .as_deref(),
        Some("handle the edge case")
    );

    // The item is closed with the plan that settled it.
    let item = s.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
    assert_eq!(item.state, MsgState::Resolved);
    assert_eq!(item.result.as_deref(), Some("settled in plan 2"));
    // And a reply to it is refused: it is handled.
    let error = resolve(&mut s, "M-1").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition);
}

#[test]
fn a_failed_merge_is_settled_by_a_verdict_too() {
    let mut s = world_with(true);
    // The review passed and the task is done; its merge then failed.
    let merge_item = s
        .write(|tx| {
            gear_app::tasks::settle_review(tx, "p", "T-1", 1, T0 + 2)?;
            let op = tx.insert_op(&gear_store::NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Merge,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: T0 + 3,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 4)?;
            tx.set_op_state(
                "p",
                &op.id,
                OpState::Failed,
                Some("conflict merging abc1234"),
                T0 + 5,
            )?;
            // The engine's item, naming the task and attempt (T-90).
            let item = tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "merge:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:captain".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Question,
                urgent: false,
                body: "merge of T-1 failed: conflict merging abc1234\nnothing was pushed".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0 + 6,
            })?;
            Ok(item.id)
        })
        .unwrap();
    assert_eq!(task_state(&mut s), TaskState::Done);

    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 10).unwrap();
    assert!(
        page.text.contains("# Staff: - remerge T-1"),
        "a failed merge is retried, not reworked (B5): {}",
        page.text
    );

    // The captain's fail reopens the task: the accepted attempt stays, the
    // work goes back for a rework on a new attempt.
    let text = format!(
        "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={CANDIDATE} fail rework=\"rebase the branch on dev\"\n"
    );
    apply_plan(&mut s, &text, "v2").unwrap();
    assert_eq!(task_state(&mut s), TaskState::Ready);
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        gear_model::AttemptState::Accepted,
        "the attempt did pass; the rework is a new attempt"
    );

    // F1 of the review: once the reopening fail is recorded the merge item is
    // an ordinary one — the page stops naming the verdict (the failed op stays
    // as history) and a reply resolves it.
    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 20).unwrap();
    assert!(!page.text.contains("# Verdicts:"), "{}", page.text);
    let handled = resolve(&mut s, &merge_item).unwrap();
    assert_eq!(
        handled.handled,
        vec![(merge_item.clone(), MsgState::Resolved.as_str().to_string())]
    );

    // … and it stays ordinary once the lane starts the rework attempt.
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 30)?;
        Ok(())
    })
    .unwrap();
    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 31).unwrap();
    assert!(!page.text.contains("# Verdicts:"), "{}", page.text);
}

/// F2 of the review: an ordinary question about a task whose attempt is
/// handed in is still answered and closed by a reply — it is not a review
/// fail, so no verdict is asked for.
#[test]
fn an_ordinary_question_is_answered_without_a_verdict() {
    let mut s = world_with(true);
    // The lane asks the captain something about the task it has handed in.
    let question = s
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "T-1:2".into(),
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    sender: "p:heat".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Question,
                    urgent: false,
                    body: "Shall I also touch the docs?".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: T0 + 7,
                })?
                .id)
        })
        .unwrap();
    assert_eq!(task_state(&mut s), TaskState::Verifying);
    let verdicts_before = s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap().len();

    // The page offers the reply, and the reply goes through.
    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 10).unwrap();
    assert!(!page.text.contains("# Verdicts:"), "{}", page.text);
    assert!(
        page.text.contains(&format!(
            "gear reply {question} --resolve -m \"<what was done>\""
        )),
        "{}",
        page.text
    );
    resolve(&mut s, &question).unwrap();

    // Nothing was settled: the task still verifies, no verdict was written.
    assert_eq!(task_state(&mut s), TaskState::Verifying);
    assert_eq!(
        s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap().len(),
        verdicts_before
    );
}

/// T-109: a merge that failed and was later remerged through closes the
/// question. The old Failed op stays as history, but the item is an ordinary
/// one — the home page stops naming the verdict and a reply resolves it.
#[test]
fn a_remerge_that_landed_closes_the_merge_item_by_reply() {
    let mut s = world_with(true);
    let merge_item = s
        .write(|tx| {
            gear_app::tasks::settle_review(tx, "p", "T-1", 1, T0 + 2)?;
            // The first merge failed…
            let failed = tx.insert_op(&gear_store::NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Merge,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: T0 + 3,
            })?;
            tx.set_op_state("p", &failed.id, OpState::Executing, None, T0 + 4)?;
            tx.set_op_state(
                "p",
                &failed.id,
                OpState::Failed,
                Some("push refused"),
                T0 + 5,
            )?;
            let item = tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "push:T-1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: MsgKind::Question,
                urgent: true,
                body: "push refused, nothing was pushed".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0 + 6,
            })?;
            Ok(item.id)
        })
        .unwrap();

    // Only the Failed op: the item still asks for a plan line, and a reply
    // that would close it is refused with that line.
    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 10).unwrap();
    assert!(
        page.text.contains("# Staff: - remerge T-1"),
        "a failed merge is retried, not reworked (B5): {}",
        page.text
    );
    let err = resolve(&mut s, &merge_item).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("- remerge T-1"), "{err:?}");
    assert!(
        err.detail.contains("fail rework"),
        "the rework line is named too: {err:?}"
    );

    // … then the remerge landed: a Confirmed op for the same attempt.
    s.write(|tx| {
        let again = tx.insert_op(&gear_store::NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Merge,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({}),
            created: T0 + 20,
        })?;
        tx.set_op_state("p", &again.id, OpState::Executing, None, T0 + 21)?;
        tx.set_op_state(
            "p",
            &again.id,
            OpState::Confirmed,
            Some("merged T-1 into dev: def5678"),
            T0 + 22,
        )
    })
    .unwrap();

    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 30).unwrap();
    assert!(
        !page.text.contains("# Verdicts:"),
        "the merge went through: {}",
        page.text
    );
    let handled = resolve(&mut s, &merge_item).unwrap();
    assert_eq!(
        handled.handled,
        vec![(merge_item.clone(), MsgState::Resolved.as_str().to_string())]
    );
}
