//! Contract for `# Verdicts` in plan apply (T-50). Needs T-42, T-43.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::tasks::{on_delivery_confirmed, start_attempt, submit_result};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, NewMessage, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: "captain".into(),
                generation: 1,
            }),
            pane_id: Some(CAP.into()),
            admin: false,
        },
        Fingerprint {
            pane_id: CAP.into(),
            kind: Some("codex".into()),
            session_ref: None,
        },
    )
}

fn run(
    s: &mut Store,
    text: &str,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let (env, fp) = cap();
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(text).unwrap(),
        text,
        rid,
        T0 + 50,
    )
}

/// v1 creates T-1 and T-2 on lane heat; both get attempt 1 submitted with candidates c1/c2.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        testing::seed_active_seat(tx, "p", "wire", SeatRole::Lane, "w1:p3", "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    run(&mut s, "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [must] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n", "v1").unwrap();
    s.write(|tx| {
        for (t, seat, c) in [("T-1", "p:heat", "c1"), ("T-2", "p:wire", "c2")] {
            start_attempt(tx, "p", t, seat, 1, T0)?;
            on_delivery_confirmed(tx, "p", t, 1, T0)?;
            submit_result(tx, "p", t, 1, 1, c, "r", T0)?;
        }
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn verdicts_settle_in_the_apply() {
    let mut s = world();
    let r = run(&mut s, "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate=c1 pass evidence=logs/t1.md\n- T-2 attempt=1 contract=1 candidate=c2 fail rework=\"edge cases\"\n", "v2").unwrap();
    assert_eq!(r.version, 2);
    assert_eq!(
        r.settled,
        vec![
            "T-1 attempt 1 pass".to_string(),
            "T-2 attempt 1 fail".to_string()
        ]
    );
    let t = |s: &mut Store, id: &str| s.read(|tx| tx.get_task("p", id)).unwrap().unwrap().state;
    assert_eq!(
        (t(&mut s, "T-1"), t(&mut s, "T-2")),
        (TaskState::Done, TaskState::Ready)
    );
}

#[test]
fn a_bad_verdict_aborts_the_whole_apply() {
    let mut s = world();
    let e = run(&mut s, "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate=c1 pass evidence=logs/t1.md\n- T-2 attempt=1 contract=1 candidate=WRONG pass evidence=x\n", "v2").unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 1);
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Submitted,
        "T-1 not settled either"
    );
}

/// T-187: a verdict that settles an attempt closes the review item of that
/// attempt: the captain does not have to resolve it by hand (M-2412).
#[test]
fn a_verdict_closes_the_review_item_of_that_attempt() {
    let mut s = world();
    // The review items of the two attempts, in the captain's inbox.
    s.write(|tx| {
        for (task, attempt) in [("T-1", 1), ("T-2", 1)] {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: format!("{task}:1"),
                task: Some(task.into()),
                attempt: Some(attempt),
                sender: "p:review-1".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Review,
                urgent: true,
                body: "F1: the edge cases are missing".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0,
            })?;
        }
        Ok(())
    })
    .unwrap();

    // The verdict of T-2 (a fail) settles that attempt.
    run(&mut s, "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-2 attempt=1 contract=1 candidate=c2 fail rework=\"edge cases\"\n", "v2").unwrap();

    let item = |s: &mut Store, task: &str| {
        s.read(|tx| tx.thread_messages("p", &format!("{task}:1")))
            .unwrap()
            .into_iter()
            .find(|message| message.kind == MsgKind::Review)
            .unwrap()
    };
    // The item of the settled attempt is closed, with the plan's version.
    let settled = item(&mut s, "T-2");
    assert_eq!(settled.state, MsgState::Resolved, "{settled:?}");
    assert_eq!(
        settled.result.as_deref(),
        Some("settled in plan 2"),
        "the result says where it was settled: {settled:?}"
    );
    // The item of the other attempt is untouched.
    let other = item(&mut s, "T-1");
    assert_eq!(other.state, MsgState::Open, "{other:?}");
    assert_eq!(other.result, None, "{other:?}");
}
