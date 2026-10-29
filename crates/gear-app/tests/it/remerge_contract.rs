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
use gear_model::{GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole};
use gear_store::{testing, NewMessage, NewOp, OpRow, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";

/// The project's config, naming `repo` as its repository when one is given
/// (a test that asks git where the candidate is points it at a real one) and
/// the review-branch template it names.
fn config_at(repo: Option<&std::path::Path>, review_branch: &str) -> String {
    let repo = match repo {
        Some(path) => serde_json::json!(path.to_string_lossy()).to_string(),
        None => "\"/repo\"".to_string(),
    };
    format!(
        "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\n\
         [quota]\nlanes = 2\ntemps = 0\n\n\
         [captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\n\
         [conventions]\nintegration = \"dev\"\ngate = \"make test\"\n\
         review_branch = \"{review_branch}\"\n"
    )
}

/// A project, the captain and the lane `heat`. `None` keeps the store's config
/// text empty: the defaults are what every path reads then.
fn world(review_branch: Option<&str>) -> Store {
    world_at(None, review_branch)
}

/// The same world with the project's config naming `repo` as its repository.
fn world_at(repo: Option<&std::path::Path>, review_branch: Option<&str>) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", T0)?;
        if repo.is_some() || review_branch.is_some() {
            project.config_toml = config_at(repo, review_branch.unwrap_or("review/t{n}"));
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
    accepted_task_on(s, "c1");
}

/// Task T-1 on lane `heat`, attempt 1 accepted on `candidate`.
fn accepted_task_on(s: &mut Store, candidate: &str) {
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
        gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, candidate, "report", T0 + 2)
    })
    .unwrap();
    let verdicts = format!(
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate={candidate} pass evidence=logs/t1.md\n"
    );
    run(s, &verdicts, "v2", T0 + 3).unwrap();
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

/// Any 998 ops, so the next two the test inserts are `O-999` and `O-1000`:
/// the ids where a string sort and an op-number sort disagree.
fn ops_up_to_998(tx: &gear_store::Tx<'_>, created: i64) -> gear_model::GearResult<()> {
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
            created,
        })?;
    }
    Ok(())
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
    // T-405: a captain's pass is a reviewed merge, so it keeps its review
    // branch and the op says nothing about being mechanical.
    assert_eq!(
        op.payload.get("mechanical"),
        None,
        "a reviewed merge carries no mechanical flag"
    );
}

/// Task T-1 on lane `heat`, its attempt 1 settled by the **engine** as a
/// mechanical hand-in (T-273): the pass verdict whose note starts
/// `mechanical:`. Such a task never had a reviewer, so it never has a review
/// branch (T-405).
fn mechanically_accepted_task(s: &mut Store) {
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
    s.write(|tx| {
        let verdict = gear_app::tasks::Verdict {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            contract_version: 1,
            candidate: "c1".into(),
            pass: true,
            evidence: Some("mechanical: the task is a rename".into()),
            rework: None,
            integrated_by: None,
            by_seat: "engine".into(),
        };
        gear_app::tasks::settle(tx, &verdict, T0 + 3)
    })
    .unwrap();
}

#[test]
fn a_remerge_of_a_mechanical_task_queues_no_review_branch() {
    // T-405: a mechanical task has no reviewer and therefore no review
    // branch; the retried merge says so, so it does not fail looking for one.
    let mut s = world(None);
    mechanically_accepted_task(&mut s);
    failed_merge(&mut s, T0 + 10);
    run(&mut s, "# Staff\n- remerge T-1\n", "v3", T0 + 20).unwrap();
    let op = queued_merge(&mut s);
    assert_eq!(
        op.payload["mechanical"],
        serde_json::json!(true),
        "the merge of a mechanically settled attempt needs no review branch"
    );
    assert_eq!(op.payload["candidate"], "c1");
    assert_eq!(op.attempt, Some(1), "the accepted attempt is retried");
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
        ops_up_to_998(tx, CREATED)?;
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

/// The same two ids the other way round: `O-1000` is the confirmed one, so the
/// remerge is refused and the refusal names it. The old string sort read
/// `O-999` as later, saw the failed op there and queued a retry of a task that
/// is already merged.
#[test]
fn a_confirmed_four_digit_merge_is_named_in_the_refusal() {
    let mut s = world(None);
    accepted_task(&mut s);
    const CREATED: i64 = T0 + 10;
    s.write(|tx| {
        ops_up_to_998(tx, CREATED)?;
        // O-999 failed, O-1000 merged; both created in the same second.
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
        assert_eq!(failed.id, "O-999");
        tx.set_op_state("p", &failed.id, OpState::Executing, None, CREATED)?;
        tx.set_op_state("p", &failed.id, OpState::Failed, Some("the gate failed"), CREATED)?;
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
        assert_eq!(confirmed.id, "O-1000");
        tx.set_op_state("p", &confirmed.id, OpState::Executing, None, CREATED)?;
        tx.set_op_state("p", &confirmed.id, OpState::Submitted, None, CREATED)?;
        tx.set_op_state("p", &confirmed.id, OpState::Confirmed, Some("merged T-1"), CREATED)
    })
    .unwrap();

    let error = run(&mut s, "# Staff\n- remerge T-1\n", "v3", T0 + 20).unwrap_err();
    assert_eq!(error.code, gear_model::ErrorCode::BadFormat, "{error:?}");
    assert!(error.detail.contains("already merged (O-1000)"), "{error:?}");
}

// ---- T-92 F1: a candidate that is already on the integration branch ---------

use std::path::{Path, PathBuf};

/// An open `merge-failed:T-1` item, as the engine leaves it when a merge fails.
fn open_failed_item(s: &mut Store) -> String {
    s.write(|tx| {
        tx.insert_message(&NewMessage {
            meta: None,
            project: "p".into(),
            thread: "merge-failed:T-1".into(),
            task: Some("T-1".into()),
            attempt: Some(1),
            sender: "engine".into(),
            sender_gen: 0,
            recipient: "p:captain".into(),
            kind: MsgKind::System,
            urgent: true,
            body: "the merge was refused".into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: T0 + 11,
        })
        .map(|message| message.id)
    })
    .unwrap()
}

fn merge_failed_items(s: &mut Store) -> Vec<gear_store::MessageRow> {
    s.read(|tx| tx.thread_messages("p", "merge-failed:T-1")).unwrap()
}

/// The merge ops of T-1 in any state: a queued remerge adds one.
fn merge_op_count(s: &mut Store) -> usize {
    let mut count = 0;
    for state in [
        OpState::Intended,
        OpState::Executing,
        OpState::Submitted,
        OpState::Unknown,
        OpState::Confirmed,
        OpState::Failed,
        OpState::Void,
    ] {
        count += s
            .read(|tx| tx.ops_in_state("p", state))
            .unwrap()
            .into_iter()
            .filter(|op| op.kind == OpKind::Merge && op.task.as_deref() == Some("T-1"))
            .count();
    }
    count
}

fn sh(dir: &Path, command: &str) {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .expect("run sh");
    assert!(out.status.success(), "{command}: {}", String::from_utf8_lossy(&out.stderr));
}

fn sh_out(dir: &Path, command: &str) -> String {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .expect("run sh");
    assert!(out.status.success(), "{command}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The fixture of this file, built **once**: a bare origin and a repo whose
/// `dev` carries the lane branch — or does not, when `merged` is false. A test
/// copies it (T-128), so twenty git processes are paid once.
struct Fixture {
    origin: PathBuf,
    work: PathBuf,
    candidate: String,
    /// The trees above live here, for as long as the process does.
    _root: tempfile::TempDir,
}

fn template(merged: bool) -> &'static Fixture {
    static UNMERGED: std::sync::OnceLock<Fixture> = std::sync::OnceLock::new();
    static MERGED: std::sync::OnceLock<Fixture> = std::sync::OnceLock::new();
    let cell = if merged { &MERGED } else { &UNMERGED };
    cell.get_or_init(|| {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        sh(root.path(), &format!("git init -q --bare {}", origin.display()));
        sh(root.path(), &format!("git init -q -b dev {}", work.display()));
        std::fs::write(work.join("README.md"), "dev\n").unwrap();
        sh(&work, "git add -A && git -c user.email=a@b -c user.name=a commit -qm dev");
        sh(&work, &format!("git remote add origin {}", origin.display()));
        publish(&origin, &work, "dev");
        sh(
            &work,
            "git switch -qc heat/t1 && echo work >> README.md && git add -A \
             && git -c user.email=a@b -c user.name=a commit -qm work",
        );
        publish(&origin, &work, "heat/t1");
        let candidate = sh_out(&work, "git rev-parse HEAD");
        if merged {
            sh(
                &work,
                "git switch -q dev && git -c user.email=a@b -c user.name=a merge -q --no-ff heat/t1 -m merge",
            );
            publish(&origin, &work, "dev");
        }
        Fixture { origin, work, candidate, _root: root }
    })
}

/// A copy of the fixture: its own bare origin and its own clone of it, which
/// the project's config names as its repository.
fn repo(scratch: &Path, merged: bool) -> (PathBuf, String) {
    let fixture = template(merged);
    let origin = scratch.join("origin.git");
    crate::copy_tree(&fixture.origin, &origin);
    let work = scratch.join("work");
    crate::copy_tree(&fixture.work, &work);
    crate::point_at(&work, &origin);
    (work, fixture.candidate.clone())
}

/// Publish `branch` of `source` in the bare origin's refs: the origin fetches
/// the objects and the ref is set — no test pushes (T-108).
fn publish(origin: &Path, source: &Path, branch: &str) {
    let sha = sh_out(source, &format!("git rev-parse refs/heads/{branch}"));
    sh(origin, &format!("git fetch -q {} refs/heads/{branch}", source.display()));
    sh(origin, &format!("git update-ref refs/heads/{branch} {}", sha.trim()));
}

/// A candidate that landed by another way (a hand merge) is on `dev` already:
/// the remerge queues no op, resolves the open failed-merge item with the sha
/// and the apply says so in its warnings.
#[test]
fn a_remerge_of_a_candidate_already_on_the_integration_branch_queues_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    let (work, candidate) = repo(scratch.path(), true);
    let mut s = world_at(Some(&work), Some("review/t{n}"));
    accepted_task_on(&mut s, &candidate);
    failed_merge(&mut s, T0 + 10);
    open_failed_item(&mut s);
    assert_eq!(merge_op_count(&mut s), 1, "the failed op is history");

    let result = run(&mut s, "# Staff\n- remerge T-1\n", "v3", T0 + 20).unwrap();

    assert_eq!(merge_op_count(&mut s), 1, "no merge op was queued: {result:?}");
    assert_eq!(
        result.warnings,
        vec![format!(
            "remerge T-1: {candidate} is already on the integration branch: no merge queued, its failed-merge item resolved"
        )],
        "{result:?}"
    );
    let items = merge_failed_items(&mut s);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
    assert_eq!(
        items[0].result.as_deref(),
        Some(format!("the candidate {candidate} is already on the integration branch").as_str())
    );
}

/// The candidate is not on `dev`: the remerge queues the merge op as before,
/// and the failed-merge item is left open for the engine's retry.
#[test]
fn a_remerge_of_a_candidate_not_on_the_integration_branch_queues_the_op() {
    let scratch = tempfile::tempdir().unwrap();
    let (work, candidate) = repo(scratch.path(), false);
    let mut s = world_at(Some(&work), Some("review/t{n}"));
    accepted_task_on(&mut s, &candidate);
    failed_merge(&mut s, T0 + 10);
    open_failed_item(&mut s);

    let result = run(&mut s, "# Staff\n- remerge T-1\n", "v3", T0 + 20).unwrap();

    assert_eq!(merge_op_count(&mut s), 2, "the retry is queued: {result:?}");
    assert_eq!(queued_merge(&mut s).payload["candidate"], candidate);
    assert!(result.warnings.is_empty(), "{result:?}");
    let items = merge_failed_items(&mut s);
    assert_eq!(items[0].state, MsgState::Open, "the engine's retry answers it");
}
