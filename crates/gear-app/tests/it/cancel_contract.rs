//! T-93: cancelling a task from the plan finishes it.
//!
//! M3: T-80 was cancelled from the plan while running; its attempt stayed
//! `started`, the seat looked busy and the lane sat idle with six tasks
//! queued until the captain edited the store. Now the cancel abandons the
//! open attempt, tells the lane one line, and a plan that keeps saying
//! `state: cancelled` applies cleanly.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::tasks::{on_delivery_confirmed, settle, start_attempt, submit_result, Verdict};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, Store};

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

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    run(&mut s, "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n", 0, "v1").unwrap();
    s
}

fn run(
    s: &mut Store,
    queue: &str,
    base: i64,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Queue\n{queue}");
    let (env, fp) = cap();
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(&text).unwrap(),
        &text,
        rid,
        T0 + 10,
    )
}

fn attempt(s: &mut Store, task: &str, n: i64) -> AttemptState {
    s.read(|tx| tx.get_attempt("p", task, n))
        .unwrap()
        .unwrap()
        .state
}

fn state(s: &mut Store, task: &str) -> TaskState {
    s.read(|tx| tx.get_task("p", task)).unwrap().unwrap().state
}

fn lane_messages(s: &mut Store, task: &str) -> Vec<gear_store::MessageRow> {
    s.read(|tx| tx.thread_messages("p", &format!("{task}:cancel")))
        .unwrap()
}

const CANCEL_T1: &str = "## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n\n## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n";

/// The same plan, with T-2 still waiting on T-1 (T-428: refused).
const CANCEL_T1_WITH_DEPS: &str = "## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n\n## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n";

/// The task the lane is working on is cancelled: its attempt is abandoned —
/// the seat is free — and the lane gets one line telling it to stop.
#[test]
fn cancelling_a_started_task_abandons_its_attempt_and_tells_the_lane() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Started);

    run(&mut s, CANCEL_T1, 1, "v2").unwrap();

    assert_eq!(state(&mut s, "T-1"), TaskState::Cancelled);
    assert_eq!(
        attempt(&mut s, "T-1", 1),
        AttemptState::Abandoned,
        "the seat is free"
    );
    assert_eq!(
        s.read(|tx| tx.in_progress_attempts_of_seat("p:heat"))
            .unwrap()
            .len(),
        0,
        "no attempt holds the lane"
    );
    let messages = lane_messages(&mut s, "T-1");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].recipient, "p:heat");
    assert_eq!(messages[0].kind, MsgKind::Instruction);
    assert_eq!(messages[0].state, MsgState::Open);
    assert_eq!(
        messages[0].body,
        "T-1 is cancelled; stop work on it, nothing to hand in"
    );
}

/// The plan keeps saying `state: cancelled` (it is the plan of record):
/// re-applying it is a no-op, not an error, and adds no second line.
#[test]
fn re_applying_the_same_cancel_is_a_no_op() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    run(&mut s, CANCEL_T1, 1, "v2").unwrap();
    let first = lane_messages(&mut s, "T-1");
    assert_eq!(first.len(), 1);

    let again = run(&mut s, CANCEL_T1, 2, "v3").unwrap();
    assert_eq!(again.version, 3);
    assert_eq!(again.settled.len(), 0);
    assert_eq!(state(&mut s, "T-1"), TaskState::Cancelled);
    assert_eq!(lane_messages(&mut s, "T-1").len(), 1, "one line, once");
}

/// Every open state closes: a handed-in attempt and a delivered one too.
#[test]
fn a_handed_in_or_delivered_attempt_is_abandoned_as_well() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, "abc1234", "ops/reports/T-1.md", T0)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Submitted);

    run(&mut s, CANCEL_T1, 1, "v2").unwrap();

    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Abandoned);
    assert_eq!(lane_messages(&mut s, "T-1").len(), 1);
}

/// An attempt whose outcome the engine could not read is closed too: it is
/// stopped first (the model never abandons UNKNOWN directly, so the engine
/// does not guess) and then abandoned, so the lane is free.
#[test]
fn an_unknown_attempt_is_closed_too() {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
        attempt.state = AttemptState::Unknown;
        tx.update_attempt(&attempt)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Unknown);

    run(&mut s, CANCEL_T1, 1, "v2").unwrap();

    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Abandoned);
    assert_eq!(lane_messages(&mut s, "T-1").len(), 1);
}

/// A task that is done is not cancelled after the fact; an idle task needs no
/// line (nothing was working on it).
#[test]
fn a_finished_task_cannot_be_cancelled_and_an_idle_one_gets_no_line() {
    let mut s = world();
    // T-461: a cancel line changes the state only, so it repeats the dep.
    let text = "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\nstate: cancelled\n";
    let (env, fp) = cap();
    apply(
        &mut s,
        &env,
        Some(&fp),
        None,
        &parse(text).unwrap(),
        text,
        "v2",
        T0 + 10,
    )
    .unwrap();
    assert_eq!(state(&mut s, "T-2"), TaskState::Cancelled);
    assert!(
        lane_messages(&mut s, "T-2").is_empty(),
        "nothing was running"
    );

    // A done task is refused, naming it.
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();
    let bad = "<!-- gear-plan project=p base=2 -->\n# Queue\n## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n";
    let error = apply(
        &mut s,
        &env,
        Some(&fp),
        None,
        &parse(bad).unwrap(),
        bad,
        "v3",
        T0 + 20,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("T-1"), "{}", error.detail);
}

// ---- T-428: a plan never leaves a task depending on a cancelled one ------

/// A plan that cancels T-1 while T-2 depends on it is refused with one line
/// naming the pair, and nothing is applied.
#[test]
fn a_plan_that_cancels_a_dep_is_refused_and_names_the_pair() {
    let mut s = world();

    let error = run(&mut s, CANCEL_T1_WITH_DEPS, 1, "v2").unwrap_err();

    assert_eq!(error.code, ErrorCode::BadFormat);
    assert_eq!(
        error.detail,
        "T-2 depends on cancelled T-1: drop or replace that dependency"
    );
    assert_eq!(
        state(&mut s, "T-1"),
        TaskState::Ready,
        "nothing was cancelled"
    );
    assert_eq!(state(&mut s, "T-2"), TaskState::Ready);
    assert_eq!(
        s.read(|tx| tx.plan_version("p")).unwrap(),
        1,
        "the plan did not apply"
    );
}

/// A dep that is already cancelled in the store is refused the same way: the
/// plan may not leave a live task waiting for it.
#[test]
fn a_live_task_newly_depending_on_an_already_cancelled_task_is_refused() {
    let mut s = world();
    run(&mut s, CANCEL_T1, 1, "v2").unwrap();
    assert_eq!(state(&mut s, "T-1"), TaskState::Cancelled);

    let error = run(
        &mut s,
        "## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n",
        2,
        "v3",
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::BadFormat);
    assert_eq!(
        error.detail,
        "T-2 depends on cancelled T-1: drop or replace that dependency"
    );
    assert_eq!(
        s.read(|tx| tx.plan_version("p")).unwrap(),
        2,
        "the plan did not apply"
    );
    let deps = s.read(|tx| tx.get_task("p", "T-2")).unwrap().unwrap().deps;
    assert!(deps.is_empty(), "the dep was not written: {deps:?}");
}

/// Dropping the dependency lets the cancel through.
#[test]
fn dropping_the_dependency_lets_the_cancel_apply() {
    let mut s = world();

    let result = run(&mut s, CANCEL_T1, 1, "v2").unwrap();

    assert_eq!(result.cancelled, vec!["T-1".to_string()]);
    assert_eq!(state(&mut s, "T-1"), TaskState::Cancelled);
    let deps = s.read(|tx| tx.get_task("p", "T-2")).unwrap().unwrap().deps;
    assert!(deps.is_empty(), "the dep was dropped: {deps:?}");
}

/// A finished task's old dependency on a cancelled one is not refused: it is
/// not waiting for anything any more.
#[test]
fn a_done_task_keeping_a_cancelled_dep_is_not_refused() {
    let mut s = world();
    s.write(|tx| {
        let mut dep = tx.get_task("p", "T-1")?.unwrap();
        dep.state = TaskState::Cancelled;
        tx.update_task(&dep)?;
        let mut done = tx.get_task("p", "T-2")?.unwrap();
        done.deps = vec!["T-1".to_string()];
        done.state = TaskState::Done;
        tx.update_task(&done)
    })
    .unwrap();

    let result = run(
        &mut s,
        "## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n",
        1,
        "v2",
    )
    .unwrap();

    assert_eq!(result.version, 2);
    assert_eq!(state(&mut s, "T-2"), TaskState::Done);
}

/// One line per pair: two live tasks waiting on the cancelled one are both
/// named in the one refusal.
#[test]
fn every_pair_gets_its_own_line() {
    let mut s = world();
    run(
        &mut s,
        "## NEW [must] lane=heat\ngoal: C\naccept: c\nestimate: 30\ndeps: T-1\n",
        1,
        "v2",
    )
    .unwrap();

    let error = run(
        &mut s,
        "## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n\n## T-2 [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n\n## T-3 [must] lane=heat\ngoal: C\naccept: c\nestimate: 30\ndeps: T-1\n",
        2,
        "v3",
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::BadFormat);
    let lines: Vec<&str> = error.detail.lines().collect();
    assert_eq!(
        lines,
        vec![
            "T-2 depends on cancelled T-1: drop or replace that dependency",
            "T-3 depends on cancelled T-1: drop or replace that dependency",
        ]
    );
    assert_eq!(
        state(&mut s, "T-1"),
        TaskState::Ready,
        "nothing was applied"
    );
}

// ---- T-112: a done task whose merge was given up can be cancelled ----------

use std::path::{Path, PathBuf};

use gear_model::{OpKind, OpState};
use gear_store::{AttemptRow, ContractRow, NewMessage, NewOp, OpRow, TaskRow};

fn sh(dir: &Path, command: &str) {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .expect("run sh");
    assert!(
        out.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn sh_out(dir: &Path, command: &str) -> String {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .expect("run sh");
    assert!(
        out.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The fixture of this file, built **once**: the repository before the merge
/// or after it. A test copies it (T-128) — twenty git processes per test cost
/// more than the cancel under test, and a copy of the tree costs none.
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

/// A repo whose `dev` is on a bare origin and whose lane branch carries one
/// commit — merged into `dev` when `merged`, pushed only on its own otherwise.
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
    sh(
        origin,
        &format!("git fetch -q {} refs/heads/{branch}", source.display()),
    );
    sh(
        origin,
        &format!("git update-ref refs/heads/{branch} {}", sha.trim()),
    );
}

fn config(repo: &Path) -> String {
    format!(
        "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
         \n[quota]\nlanes = 1\ntemps = 0\n\
         \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
         \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
         kind = \"opencode\"\nprofile = \"opencode\"\n\
         \n[conventions]\nintegration = \"dev\"\n",
        repo = serde_json::json!(repo.to_string_lossy()).to_string()
    )
}

/// The project with T-1 Done, its accepted attempt on `candidate`, a failed
/// merge op and the merge/push items still open.
fn done_world(repo: &Path, candidate: &str) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        let mut project = tx.get_project("p")?.unwrap();
        project.config_toml = config(repo);
        tx.update_project(&project)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        tx.insert_task(&TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: "T-1".into(),
            rank: 1,
            state: TaskState::Done,
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
            estimate_min: Some(30),
            issue: None,
            updated: T0,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "A".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
            mechanical: None,
        })?;
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: "T-1".into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: AttemptState::Accepted,
            candidate: Some(candidate.to_string()),
            report: Some("ops/reports/T-1.md".into()),
            safe_point: None,
            started: T0,
            updated: T0,
        })?;
        let op = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Merge,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "branch": "heat/t1" }),
            created: T0,
        })?;
        tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
        tx.set_op_state("p", &op.id, OpState::Failed, Some("the gate said no"), T0)?;
        for thread in ["merge-failed:T-1", "push:T-1"] {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: thread.into(),
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
                created: T0,
            })?;
        }
        Ok(())
    })
    .unwrap();
    s
}

const CANCEL_DONE: &str =
    "## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n";

fn merge_ops(s: &mut Store) -> Vec<OpRow> {
    s.read(|tx| tx.ops_in_state("p", OpState::Failed)).unwrap()
}

/// A done task whose accepted work never reached the integration branch is
/// cancelled from the plan: the task ends Cancelled, its open merge and push
/// items are resolved "cancelled by the captain", the failed merge op stays as
/// history and no op is created.
#[test]
fn a_done_task_whose_merge_was_given_up_is_cancelled_with_its_items() {
    let scratch = tempfile::tempdir().unwrap();
    let (work, candidate) = repo(scratch.path(), false);
    let mut s = done_world(&work, &candidate);

    let result = run(&mut s, CANCEL_DONE, 0, "v1").unwrap();

    assert_eq!(result.cancelled, vec!["T-1".to_string()]);
    assert_eq!(
        result.resolved.len(),
        2,
        "the merge and push items: {result:?}"
    );
    assert_eq!(state(&mut s, "T-1"), TaskState::Cancelled);
    for thread in ["merge-failed:T-1", "push:T-1"] {
        let items = s.read(|tx| tx.thread_messages("p", thread)).unwrap();
        assert_eq!(items.len(), 1, "{thread}: {items:?}");
        assert_eq!(items[0].state, MsgState::Resolved, "{thread}: {items:?}");
        assert_eq!(items[0].result.as_deref(), Some("cancelled by the captain"));
    }
    let ops = merge_ops(&mut s);
    assert_eq!(
        ops.len(),
        1,
        "the failed merge op stays as history: {ops:?}"
    );
    assert_eq!(ops[0].result.as_deref(), Some("the gate said no"));
    assert_eq!(merge_ops(&mut s).len(), 1, "no op was created");
}

/// A done task whose accepted work *is* on the integration branch stays
/// uncancellable, as before.
#[test]
fn a_done_task_whose_work_is_merged_cannot_be_cancelled() {
    let scratch = tempfile::tempdir().unwrap();
    let (work, candidate) = repo(scratch.path(), true);
    let mut s = done_world(&work, &candidate);

    let error = run(&mut s, CANCEL_DONE, 0, "v1").unwrap_err();

    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("T-1"), "{}", error.detail);
    assert!(
        error.detail.contains("cannot be cancelled"),
        "{}",
        error.detail
    );
    assert_eq!(state(&mut s, "T-1"), TaskState::Done);
    let items = s
        .read(|tx| tx.thread_messages("p", "merge-failed:T-1"))
        .unwrap();
    assert_eq!(items[0].state, MsgState::Open, "nothing was resolved");
}

// ---- T-311: moving a running task to another lane moves its work ----------

/// The world of the move tests: the cancel world, plus the lane the plan sends
/// the tasks to.
fn move_world() -> Store {
    let mut s = world();
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "cold", SeatRole::Lane, "w1:p3", "opencode", T0)
    })
    .unwrap();
    s
}

fn moved_messages(s: &mut Store, task: &str) -> Vec<gear_store::MessageRow> {
    s.read(|tx| tx.thread_messages("p", &format!("{task}:moved")))
        .unwrap()
}

fn lane(s: &mut Store, task: &str) -> Option<String> {
    s.read(|tx| tx.get_task("p", task)).unwrap().unwrap().lane
}

const MOVE_T1_TO_COLD: &str =
    "## T-1 [must] lane=cold\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";

/// A started task moves: its attempt is abandoned — the seat is free — it is
/// Ready on the new lane, and the old lane gets one line telling it to stop.
#[test]
fn moving_a_started_task_abandons_its_attempt_and_tells_the_old_lane() {
    let mut s = move_world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Started);

    run(&mut s, MOVE_T1_TO_COLD, 1, "v2").unwrap();

    assert_eq!(lane(&mut s, "T-1").as_deref(), Some("cold"));
    assert_eq!(state(&mut s, "T-1"), TaskState::Ready);
    assert_eq!(
        attempt(&mut s, "T-1", 1),
        AttemptState::Abandoned,
        "the seat is free"
    );
    assert_eq!(
        s.read(|tx| tx.in_progress_attempts_of_seat("p:heat"))
            .unwrap()
            .len(),
        0,
        "no attempt holds the old lane"
    );
    let messages = moved_messages(&mut s, "T-1");
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].recipient, "p:heat");
    assert_eq!(messages[0].kind, MsgKind::Instruction);
    assert_eq!(messages[0].state, MsgState::Open);
    assert_eq!(
        messages[0].body,
        "T-1 moved to lane cold; stop work on it, nothing to hand in"
    );
    // The decision log records the move (design §7.4).
    let logged = s.read(|tx| tx.decisions_of("p", "")).unwrap();
    let moved = logged.iter().find(|d| d.what == "T-1 moved heat → cold");
    let moved = moved.unwrap_or_else(|| panic!("no move in the log: {logged:?}"));
    assert_eq!(moved.kind, "staff");
}

/// A delivered attempt — the prompt reached the lane, it never started — is
/// abandoned too, and the lane is told.
#[test]
fn moving_a_delivered_task_abandons_its_attempt_too() {
    let mut s = move_world();
    s.write(|tx| start_attempt(tx, "p", "T-1", "p:heat", 1, T0).map(|_| ()))
        .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Delivered);

    run(&mut s, MOVE_T1_TO_COLD, 1, "v2").unwrap();

    assert_eq!(lane(&mut s, "T-1").as_deref(), Some("cold"));
    assert_eq!(state(&mut s, "T-1"), TaskState::Ready);
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Abandoned);
    assert_eq!(moved_messages(&mut s, "T-1").len(), 1);
}

/// The rule is keyed on the attempts, not on the state: after a fail verdict
/// the task is Ready and the rework attempt is open — a move abandons it and
/// the new lane gets the task.
#[test]
fn a_rework_attempt_opened_after_a_fail_verdict_is_abandoned_and_the_new_lane_gets_the_task() {
    let mut s = move_world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, "c1", "ops/reports/T-1.md", T0)?;
        settle(
            tx,
            &Verdict {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                contract_version: 1,
                candidate: "c1".into(),
                pass: false,
                evidence: None,
                rework: Some("edge cases".into()),
                integrated_by: None,
                by_seat: "p:captain".into(),
            },
            T0 + 1,
        )
    })
    .unwrap();
    assert_eq!(
        state(&mut s, "T-1"),
        TaskState::Ready,
        "a fail sends the task back"
    );
    s.write(|tx| start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 2).map(|_| ()))
        .unwrap();
    assert_eq!(
        state(&mut s, "T-1"),
        TaskState::Ready,
        "the rework attempt opens on a Ready task"
    );
    assert_eq!(attempt(&mut s, "T-1", 2), AttemptState::Delivered);

    run(&mut s, MOVE_T1_TO_COLD, 1, "v2").unwrap();

    assert_eq!(
        attempt(&mut s, "T-1", 1),
        AttemptState::Rejected,
        "the settled attempt keeps its verdict"
    );
    assert_eq!(
        attempt(&mut s, "T-1", 2),
        AttemptState::Abandoned,
        "the open rework closes"
    );
    assert_eq!(lane(&mut s, "T-1").as_deref(), Some("cold"));
    assert_eq!(state(&mut s, "T-1"), TaskState::Ready);
    assert_eq!(moved_messages(&mut s, "T-1").len(), 1);
}

/// A handed-in attempt is under review: its verdict belongs to the lane that
/// did the work, so the move is refused and nothing is written.
#[test]
fn moving_a_handed_in_task_is_refused_and_nothing_is_written() {
    let mut s = move_world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        submit_result(tx, "p", "T-1", 1, 1, "c1", "ops/reports/T-1.md", T0)
    })
    .unwrap();
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Submitted);

    let error = run(&mut s, MOVE_T1_TO_COLD, 1, "v2").unwrap_err();

    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error.detail.contains(
            "T-1 is handed in (attempt 1) and under review: change its lane after the verdict"
        ),
        "{}",
        error.detail
    );
    assert_eq!(lane(&mut s, "T-1").as_deref(), Some("heat"));
    assert_eq!(state(&mut s, "T-1"), TaskState::Verifying);
    assert_eq!(attempt(&mut s, "T-1", 1), AttemptState::Submitted);
    assert!(
        moved_messages(&mut s, "T-1").is_empty(),
        "nothing was written"
    );
    assert_eq!(
        s.read(|tx| tx.plan_version("p")).unwrap(),
        1,
        "the plan did not apply"
    );
}

/// A task with no attempt in progress only changes its lane: no state change,
/// no line to any lane.
#[test]
fn moving_a_task_with_no_attempt_changes_only_its_lane() {
    let mut s = move_world();
    assert_eq!(state(&mut s, "T-1"), TaskState::Ready);

    run(&mut s, MOVE_T1_TO_COLD, 1, "v2").unwrap();

    assert_eq!(lane(&mut s, "T-1").as_deref(), Some("cold"));
    assert_eq!(state(&mut s, "T-1"), TaskState::Ready);
    assert!(
        moved_messages(&mut s, "T-1").is_empty(),
        "nothing was running"
    );
    assert_eq!(
        lane(&mut s, "T-2").as_deref(),
        Some("heat"),
        "the other task keeps its lane"
    );
}
