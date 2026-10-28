//! Contract for `- remerge T-<n>` in plan apply, as the review branch and the
//! op order are read (T-268).
//!
//! - the queued merge op names the review branch the project's
//!   `[conventions].review_branch` template gives (T-252), not the built-in
//!   default, so a project with its own template remerges the branch its
//!   reviews actually cut;
//! - the task's last merge op is found by its number, not by its id as a
//!   string, where `O-999` sorts after `O-1000` in the same second.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{GearCtx, OpKind, OpState, SeatRole};
use gear_store::{testing, NewOp, OpRow, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";

/// The project's config with the review-branch template it names.
fn config(review_branch: &str) -> String {
    format!(
        "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\n\
         [quota]\nlanes = 2\ntemps = 0\n\n\
         [captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\n\
         [conventions]\nintegration = \"dev\"\ngate = \"make test\"\n\
         review_branch = \"{review_branch}\"\n"
    )
}

/// A project, the captain and the lane `heat`. `None` keeps the store's config
/// text empty: the defaults are what every path reads then.
fn world(review_branch: Option<&str>) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", T0)?;
        if let Some(template) = review_branch {
            project.config_toml = config(template);
            tx.update_project(&project)?;
        }
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP.into()),
            admin: false,
        },
        Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None },
    )
}

/// Apply a plan body against the plan version the store holds.
fn run(
    s: &mut Store,
    body: &str,
    rid: &str,
    now: i64,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    let text = format!("<!-- gear-plan project=p base={base} -->\n{body}");
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &parse(&text).unwrap(), &text, rid, now)
}

/// Task T-1 on lane `heat`, with attempt 1 accepted on candidate `c1`.
fn accepted_task(s: &mut Store) {
    run(
        s,
        "# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "v1",
        T0,
    )
    .unwrap();
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 1)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0 + 1)?;
        gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "c1", "report", T0 + 2)
    })
    .unwrap();
    run(
        s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=c1 pass evidence=logs/t1.md\n",
        "v2",
        T0 + 3,
    )
    .unwrap();
}

/// A merge op of T-1 that was created at `created` and ended `Failed`.
fn failed_merge(s: &mut Store, created: i64) -> String {
    s.write(|tx| {
        let op = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Merge,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({}),
            created,
        })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, created)?;
        tx.set_op_state("p", &op.id, OpState::Failed, Some("the gate failed"), created + 1)?;
        Ok(op.id)
    })
    .unwrap()
}

/// The merge op the remerge queues: the only Intended merge op of T-1.
fn queued_merge(s: &mut Store) -> OpRow {
    s.read(|tx| Ok(tx.ops_in_state("p", OpState::Intended)?))
        .unwrap()
        .into_iter()
        .find(|op| op.kind == OpKind::Merge && op.task.as_deref() == Some("T-1"))
        .expect("the remerge queued a merge op")
}

#[test]
fn a_remerge_names_the_projects_review_branch() {
    let mut s = world(Some("{project}/review/t{n}"));
    accepted_task(&mut s);
    failed_merge(&mut s, T0 + 10);
    run(&mut s, "# Staff\n- remerge T-1\n", "v3", T0 + 20).unwrap();
    let op = queued_merge(&mut s);
    assert_eq!(op.payload["review"], "p/review/t1", "the project's template, not review/t1");
    assert_eq!(op.payload["candidate"], "c1");
    assert_eq!(op.payload["branch"], "heat/t1");
    assert_eq!(op.payload["remerge"], true);
}

#[test]
fn the_default_config_still_writes_review_t_n() {
    let mut s = world(None);
    accepted_task(&mut s);
    failed_merge(&mut s, T0 + 10);
    run(&mut s, "# Staff\n- remerge T-1\n", "v3", T0 + 20).unwrap();
    assert_eq!(queued_merge(&mut s).payload["review"], "review/t1");
}

/// Two merge ops of one task in the same second, with four-digit ids: the last
/// one is `O-1000` (failed), so the remerge goes through. The old string sort
/// read `O-999` as later and saw the confirmed op there, which refuses with
/// "already merged".
#[test]
fn the_last_merge_is_the_higher_op_number_not_the_larger_string() {
    let mut s = world(None);
    accepted_task(&mut s);
    const CREATED: i64 = T0 + 10;
    s.write(|tx| {
        // Ops O-1 … O-998: any op, so the two merges get four-digit ids.
        for _ in 0..998 {
            tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Doorbell,
                task: None,
                attempt: None,
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: CREATED,
            })?;
        }
        // O-999 merged, O-1000 failed; both created in the same second.
        let confirmed = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Merge,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({}),
            created: CREATED,
        })?;
        assert_eq!(confirmed.id, "O-999");
        tx.set_op_state("p", &confirmed.id, OpState::Executing, None, CREATED)?;
        tx.set_op_state("p", &confirmed.id, OpState::Submitted, None, CREATED)?;
        tx.set_op_state("p", &confirmed.id, OpState::Confirmed, Some("merged T-1"), CREATED)?;
        let failed = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Merge,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({}),
            created: CREATED,
        })?;
        assert_eq!(failed.id, "O-1000");
        tx.set_op_state("p", &failed.id, OpState::Executing, None, CREATED)?;
        tx.set_op_state("p", &failed.id, OpState::Failed, Some("the gate failed"), CREATED)
    })
    .unwrap();

    run(&mut s, "# Staff\n- remerge T-1\n", "v3", T0 + 20).unwrap();
    let op = queued_merge(&mut s);
    assert_eq!(op.attempt, Some(1), "the accepted attempt is retried");
    assert_eq!(op.payload["candidate"], "c1");
}
