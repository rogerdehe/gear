//! The `audit:` mark of a task (T-505): a voyage audit is planned as a queue
//! task that reviewers run against the integration branch. The field is stored
//! as `fresh:` is, shown by `gear plan show`, and the captain's verdict on an
//! audit attempt is a merge with no merge op (a pass) or a task back on Ready
//! (a fail).

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::tasks::{settle, Verdict};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 1

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "codex"
profile = "codex"

[conventions]
integration = "dev"
release = "main"
gate = "make test"
"#;

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", NOW)?;
        project.config_toml = CONFIG.to_string();
        tx.update_project(&project)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: "captain".into(),
                generation: 1,
            }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint {
            pane_id: CAP_PANE.into(),
            kind: Some("codex".into()),
            session_ref: None,
        },
    )
}

fn run(s: &mut Store, text: &str, rid: &str) -> gear_model::GearResult<()> {
    let (env, fp) = cap();
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(&format!("<!-- gear-plan project=p base={base} -->\n{text}")).unwrap(),
        &format!("<!-- gear-plan project=p base={base} -->\n{text}"),
        rid,
        NOW + 10,
    )
    .map(|_| ())
}

/// An audit attempt handed in at the integration branch's tip, and the task
/// Verifying (what the engine's audit step writes).
fn handed_in_audit(s: &mut Store, task: &str, candidate: &str) {
    s.write(|tx| {
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: task.into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: AttemptState::Submitted,
            candidate: Some(candidate.into()),
            report: None,
            safe_point: None,
            started: NOW,
            updated: NOW,
        })?;
        let mut row = tx.get_task("p", task)?.unwrap();
        row.state = TaskState::Verifying;
        row.updated = NOW;
        tx.update_task(&row)
    })
    .unwrap();
}

/// The captain's verdict on an audit attempt.
fn rule(s: &mut Store, task: &str, pass: bool, candidate: &str) {
    let v = Verdict {
        project: "p".into(),
        task: task.into(),
        attempt: 1,
        contract_version: 1,
        candidate: candidate.into(),
        pass,
        evidence: pass.then(|| "the promise holds".to_string()),
        rework: (!pass).then(|| "the gap".to_string()),
        integrated_by: None,
        by_seat: "p:captain".into(),
    };
    s.write(|tx| settle(tx, &v, NOW + 20)).unwrap();
}

#[test]
fn an_audit_block_is_stored_and_shown() {
    let mut s = world();
    run(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: the voyage holds end to end\naccept: each promise met\nestimate: 30\naudit: true\ndeps: -\n",
        "r1",
    )
    .unwrap();
    assert_eq!(s.read(|tx| tx.task_audit("p", "T-1")).unwrap(), Some(true));
    let shown = s.read(|tx| gear_app::plan::show(tx, "p", "T-1")).unwrap();
    assert!(shown.contains("audit: true"), "{shown}");

    // A block that names `audit: false` clears the mark; a block without the
    // field keeps it.
    run(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\naudit: false\ndeps: -\n",
        "r2",
    )
    .unwrap();
    assert_eq!(s.read(|tx| tx.task_audit("p", "T-1")).unwrap(), None);
    run(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\naudit: true\ndeps: -\n",
        "r3",
    )
    .unwrap();
    run(&mut s, "# Queue\n## T-1 [must] lane=heat\ndeps: -\n", "r4").unwrap();
    assert_eq!(s.read(|tx| tx.task_audit("p", "T-1")).unwrap(), Some(true));
}

#[test]
fn an_audit_needs_its_lane() {
    // An existing task whose block clears the lane cannot become an audit: the
    // review model choice and the writer seat are the lane's.
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&gear_store::TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: "T-1".into(),
            rank: 1,
            state: TaskState::Ready,
            priority: "must".into(),
            lane: None,
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
            updated: NOW,
        })
    })
    .unwrap();
    let error = run(
        &mut s,
        "# Queue\n## T-1 [must]\naudit: true\ndeps: -\n",
        "r1",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("keeps its `lane=`"),
        "{}",
        error.detail
    );
}

#[test]
fn the_audit_field_is_true_or_false() {
    let error = parse(
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\naudit: maybe\ndeps: -\n",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("`audit` is `true` or `false`"),
        "{}",
        error.detail
    );
}

#[test]
fn a_captain_pass_marks_the_audit_done_and_merged_with_no_merge_op() {
    let mut s = world();
    run(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: the voyage holds end to end\naccept: each promise met\nestimate: 30\naudit: true\ndeps: -\n",
        "r1",
    )
    .unwrap();
    handed_in_audit(&mut s, "T-1", "abc1234");
    rule(&mut s, "T-1", true, "abc1234");

    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.state, TaskState::Done);
    assert!(
        s.read(|tx| gear_app::tasks::merged_at(tx, "p", "T-1"))
            .unwrap()
            .is_some(),
        "the audit is merged at the candidate it checked"
    );
    let merge_ops = s
        .read(|tx| tx.ops_in_state("p", OpState::Intended))
        .unwrap()
        .into_iter()
        .filter(|op| op.kind == OpKind::Merge)
        .count();
    assert_eq!(merge_ops, 0, "an audit pass merges with no merge op");
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Accepted);
}

#[test]
fn a_captain_fail_puts_the_audit_back_on_ready() {
    let mut s = world();
    run(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: the voyage holds end to end\naccept: each promise met\nestimate: 30\naudit: true\ndeps: -\n",
        "r1",
    )
    .unwrap();
    handed_in_audit(&mut s, "T-1", "abc1234");
    rule(&mut s, "T-1", false, "abc1234");

    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.state, TaskState::Ready);
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Rejected);
    assert!(
        s.read(|tx| gear_app::tasks::merged_at(tx, "p", "T-1"))
            .unwrap()
            .is_none(),
        "nothing merged"
    );
}

#[test]
fn a_review_verdict_on_an_audit_waits_for_the_captain() {
    let mut s = world();
    run(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: the voyage holds end to end\naccept: each promise met\nestimate: 30\naudit: true\ndeps: -\n",
        "r1",
    )
    .unwrap();
    handed_in_audit(&mut s, "T-1", "abc1234");
    s.write(|tx| {
        // The reviewer handed a pass in.
        tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed: true,
            findings: String::new(),
            by_seat: "p:review-1".into(),
            created: NOW + 5,
        })
    })
    .unwrap();
    s.write(|tx| gear_app::tasks::settle_review(tx, "p", "T-1", 1, NOW + 6))
        .unwrap();

    // The audit is not auto-settled: it stays Verifying for the captain.
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(
        task.state,
        TaskState::Verifying,
        "an audit verdict is the captain's to rule"
    );
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Submitted);
}
