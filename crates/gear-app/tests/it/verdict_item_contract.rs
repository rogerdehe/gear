//! T-90: a review fail is settled where it can be — by a `# Verdicts` line,
//! not by `gear reply … --resolve`.
//!
//! M3: the captain resolved two fails by reply; the items closed and nothing
//! was settled. The home page now names the line to write, a reply that would
//! close a fail is refused with that line, and once the plan settles it the
//! item is an ordinary one again.

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
        ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some(CAP.into()),
        admin: false,
    }
}

fn fp() -> Fingerprint {
    Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None }
}

fn apply_plan(s: &mut Store, text: &str, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    apply(s, &cap_env(), Some(&fp()), &parse(text).unwrap(), text, rid, T0 + 50)
}

const V1: &str = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat milestone=M3\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";

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
            findings: if passed { "all good".into() } else { "F1: the edge case is not handled".into() },
            by_seat: "p:review-1".into(),
            created: T0,
        })?;
        if !passed {
            tx.set_review_blocking("p", "T-1", 1, "F1", T0)?;
        }
        tx.insert_message(&NewMessage {
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
            state: MsgState::Open,
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

    let expected = format!(
        "next: gear plan apply gear/voyages/M3.md (# Verdicts: {VERDICTS_LINE})"
    );
    assert!(page.text.contains(&expected), "{}\n---\nexpected: {expected}", page.text);
    // The review fail is not something a reply settles.
    assert!(!page.text.contains("gear reply M-1 --resolve"), "{}", page.text);
}

#[test]
fn resolving_the_fail_by_reply_is_refused_and_leaves_it_verifying() {
    let mut s = world();

    let error = resolve(&mut s, "M-1").unwrap_err();

    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error.detail.contains(&format!("# Verdicts: {VERDICTS_LINE}")),
        "the refusal names the line: {}",
        error.detail
    );
    // Nothing was settled and the item is still there.
    assert_eq!(task_state(&mut s), TaskState::Verifying);
    let item = s.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
    assert_eq!(item.state, MsgState::Open);
    assert!(s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap().is_empty());
}

#[test]
fn after_the_plan_settles_it_resolving_works() {
    let mut s = world();
    assert_eq!(resolve(&mut s, "M-1").unwrap_err().code, ErrorCode::BadFormat);

    // The captain writes the line the home page named.
    let text = format!(
        "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate={CANDIDATE} fail rework=\"handle the edge case\"\n"
    );
    apply_plan(&mut s, &text, "v2").unwrap();
    // The verdict settled the attempt: the task is back in the lane's queue.
    assert_eq!(task_state(&mut s), TaskState::Ready);
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().rework_note.as_deref(),
        Some("handle the edge case")
    );

    // The item is an ordinary one now: a reply resolves it.
    let handled = resolve(&mut s, "M-1").unwrap();
    assert_eq!(handled.handled, vec![("M-1".to_string(), MsgState::Resolved.as_str().to_string())]);
    assert_eq!(s.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap().state, MsgState::Resolved);
}

#[test]
fn a_failed_merge_is_settled_by_a_verdict_too() {
    let mut s = world_with(true);
    // The review passed and the task is done; its merge then failed.
    s.write(|tx| {
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
        tx.set_op_state("p", &op.id, OpState::Failed, Some("conflict merging abc1234"), T0 + 5)?;
        // The engine's item, naming the task and attempt (T-90).
        tx.insert_message(&NewMessage {
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
        Ok(())
    })
    .unwrap();
    assert_eq!(task_state(&mut s), TaskState::Done);

    let page = home::build(&mut s, &cap_env(), Some(&fp()), T0 + 10).unwrap();
    assert!(
        page.text.contains(&format!("# Verdicts: {VERDICTS_LINE}")),
        "{}",
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
        s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state,
        gear_model::AttemptState::Accepted,
        "the attempt did pass; the rework is a new attempt"
    );
}
