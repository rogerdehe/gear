//! Plan apply, design §1.1 and §3.3 (T-141).
//!
//! - a task keeps the repo, the base and the gate of the lane it was created
//!   for (§1.1): copied at insert, so a lane that is rebound to another repo
//!   later leaves its old tasks where they were;
//! - `followup-of` is live on existing tasks too and names a task that
//!   exists (§3.3);
//! - a verdict line carries the minutes a rework asks for (`estimate=`, §5.1),
//!   the why of a pass over a failed review (§7.4) and the task whose merge
//!   landed the work (`integrated-by=`, §10.7 row B03-01).

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

/// A project with two lanes: `heat` works in the child repo `api`, `wire` in
/// the project root.
const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 0

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
repo = "api"

[[lanes]]
name = "wire"
duty = "price engine"
owns = ["src/wire/"]
kind = "opencode"
profile = "opencode"

[[repos]]
name = "api"
path = "crates/api"
integration = "feat/api"
release = "main"
gate = "cargo test -p api"

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
        testing::seed_active_seat(tx, "p", "wire", SeatRole::Lane, "w1:p3", "opencode", NOW)?;
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

/// Apply a plan file (the captain's whole file, base and all).
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
        NOW + 10,
    )
}

/// Apply a plan with the base the store holds.
fn run_at(
    s: &mut Store,
    body: &str,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    run(
        s,
        &format!("<!-- gear-plan project=p base={base} -->\n{body}"),
        rid,
    )
}

/// A plan that creates two tasks: T-1 for `heat`, T-2 for `wire`.
fn two_new() -> String {
    "# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n".to_string()
}

fn task(s: &mut Store, id: &str) -> gear_store::TaskRow {
    s.read(|tx| tx.get_task("p", id)).unwrap().unwrap()
}

/// §1.1: a new task keeps the repo, the base and the gate of its lane.
#[test]
fn a_new_task_keeps_the_repo_base_and_gate_of_its_lane() {
    let mut s = world();
    let r = run(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{}", two_new()),
        "r1",
    )
    .unwrap();
    assert_eq!(r.created, vec!["T-1".to_string(), "T-2".to_string()]);

    // The lane in the child repo.
    let heat = task(&mut s, "T-1");
    assert_eq!(
        (
            heat.repo.as_deref(),
            heat.base.as_deref(),
            heat.gate.as_deref()
        ),
        (
            Some("/repo/crates/api"),
            Some("feat/api"),
            Some("cargo test -p api")
        ),
        "the task keeps the repo of its lane"
    );

    // The lane in the project root.
    let wire = task(&mut s, "T-2");
    assert_eq!(
        (
            wire.repo.as_deref(),
            wire.base.as_deref(),
            wire.gate.as_deref()
        ),
        (Some("/repo"), Some("dev"), Some("make test")),
        "the task keeps the repo of its lane"
    );
}

/// §3.3: `followup-of` is set on an existing task too.
#[test]
fn a_followup_is_set_on_an_existing_task_too() {
    let mut s = world();
    run(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{}", two_new()),
        "r1",
    )
    .unwrap();

    // The captain writes the follow-up on the task that already exists.
    run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\nfollowup-of: T-2\ndeps: -\n",
        "r2",
    )
    .unwrap();

    assert_eq!(task(&mut s, "T-1").followup_of.as_deref(), Some("T-2"));
    // A block without the field keeps the follow-up the task has.
    run_at(&mut s, "# Queue\n## T-1 [must] lane=heat\ndeps: -\n", "r3").unwrap();
    assert_eq!(task(&mut s, "T-1").followup_of.as_deref(), Some("T-2"));
}

/// T-481: a task block may carry `fresh: <why>` — the captain's mark that
/// the work is unrelated to what the lane did before. It is stored with the
/// task and `gear plan show` prints it back.
#[test]
fn a_fresh_mark_is_parsed_stored_and_shown() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nfresh: a new parser, unrelated to the old one\ndeps: -\n\n## NEW [should] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();

    assert_eq!(
        s.read(|tx| tx.task_fresh("p", "T-1")).unwrap().as_deref(),
        Some("a new parser, unrelated to the old one")
    );
    let shown = s.read(|tx| gear_app::plan::show(tx, "p", "T-1")).unwrap();
    assert!(
        shown.contains("fresh: a new parser, unrelated to the old one"),
        "{shown}"
    );

    // An existing task's block may set it too; a block without `fresh:` keeps
    // the mark the task has.
    run_at(
        &mut s,
        "# Queue\n## T-2 [should] lane=wire\nfresh: another subsystem\ndeps: -\n",
        "r2",
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.task_fresh("p", "T-2")).unwrap().as_deref(),
        Some("another subsystem")
    );
    run_at(
        &mut s,
        "# Queue\n## T-2 [should] lane=wire\ndeps: -\n",
        "r3",
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.task_fresh("p", "T-2")).unwrap().as_deref(),
        Some("another subsystem")
    );
}

/// T-481: `fresh:` needs a reason.
#[test]
fn an_empty_fresh_mark_is_refused() {
    let mut s = world();
    let error = run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nfresh:\ndeps: -\n",
        "r1",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("`fresh` needs the reason"),
        "{}",
        error.detail
    );
}

/// §3.3: the follow-up names a task that exists.
#[test]
fn an_unknown_followup_is_refused() {
    let mut s = world();
    run(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{}", two_new()),
        "r1",
    )
    .unwrap();

    let error = run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\nfollowup-of: T-99\ndeps: -\n",
        "r2",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("followup-of T-99 does not exist"),
        "{}",
        error.detail
    );
    // Nothing was written: the task keeps what it had.
    assert_eq!(task(&mut s, "T-1").followup_of, None);
}

/// A task that was handed in and failed its review: the captain's item.
fn world_with_a_failed_review() -> Store {
    let mut s = world();
    run(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{}", two_new()),
        "r1",
    )
    .unwrap();
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, NOW)?;
        gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "ops/reports/T-1.md", NOW)?;
        tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed: false,
            findings: "F1: the edge case is not handled".into(),
            by_seat: "p:review-1".into(),
            created: NOW,
        })
    })
    .unwrap();
    s
}

/// §5.1: a fail verdict asks for the minutes of the rework round.
#[test]
fn a_fail_asks_for_the_minutes_of_the_rework() {
    let mut s = world_with_a_failed_review();
    run_at(
        &mut s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 fail estimate=25 rework=\"handle the edge case\"\n",
        "r2",
    )
    .unwrap();

    assert_eq!(task(&mut s, "T-1").rework_estimate_min, Some(25));
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Rejected
    );
}

/// §7.4: a pass over a failed review says why.
#[test]
fn a_pass_over_a_failed_review_needs_a_why() {
    let mut s = world_with_a_failed_review();
    let error = run_at(
        &mut s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 pass evidence=ops/reports/T-1.md\n",
        "r2",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error
            .detail
            .contains("a pass over a failed review needs why="),
        "{}",
        error.detail
    );
    // Nothing was settled: the attempt is still handed in.
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Submitted
    );

    // With the why, the pass goes through.
    run_at(
        &mut s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 pass evidence=ops/reports/T-1.md why=\"the review missed the edge case\"\n",
        "r3",
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Accepted
    );
}

/// §7.4: a pass over a review that passed needs no why.
#[test]
fn a_pass_over_a_passed_review_needs_no_why() {
    let mut s = world();
    run(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{}", two_new()),
        "r1",
    )
    .unwrap();
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, NOW)?;
        gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "ops/reports/T-1.md", NOW)?;
        tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            passed: true,
            findings: "all good".into(),
            by_seat: "p:review-1".into(),
            created: NOW,
        })
    })
    .unwrap();

    run_at(
        &mut s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 pass evidence=ops/reports/T-1.md\n",
        "r2",
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Accepted
    );
}

/// §10.7 row B03-01: a pass may name the task whose merge landed the work,
/// but only a task that is merged: the named task is the merge that landed
/// this work. (The accepted case and the git side live in
/// `integrated_by_contract.rs`.)
#[test]
fn a_pass_naming_a_task_that_is_not_merged_is_refused() {
    let mut s = world_with_a_failed_review();
    let err = run_at(
        &mut s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 pass evidence=ops/reports/T-1.md integrated-by=T-2 why=\"the review missed the edge case\"\n",
        "r2",
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("T-2"), "{}", err.detail);
    // Nothing settled: the attempt is still handed in.
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Submitted
    );
}

/// The fields belong where they make sense: an estimate on a pass, a why on a
/// fail and an `integrated-by` on a fail are refused.
#[test]
fn a_misplaced_field_is_refused() {
    for (rid, line, needle) in [
        (
            "a",
            "- T-1 attempt=1 contract=1 candidate=abc1234 pass evidence=x estimate=25\n",
            "must not carry `estimate=`",
        ),
        (
            "b",
            "- T-1 attempt=1 contract=1 candidate=abc1234 fail why=\"y\"\n",
            "`why=` belongs to a pass",
        ),
        (
            "c",
            "- T-1 attempt=1 contract=1 candidate=abc1234 fail integrated-by=T-2 rework=\"x\"\n",
            "`integrated-by=` belongs to a pass",
        ),
    ] {
        let mut s = world_with_a_failed_review();
        let base = s.read(|tx| tx.plan_version("p")).unwrap();
        let text = format!("<!-- gear-plan project=p base={base} -->\n# Verdicts\n{line}");
        // The field is refused while the line is read.
        let error = parse(&text).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat, "{rid}: {}", error.detail);
        assert!(error.detail.contains(needle), "{rid}: {}", error.detail);
    }
}

#[test]
fn rebind_waits_for_all_open_attempts_then_changes_config_and_preserves_old_tasks() {
    let mut s = world();
    s.write(|tx| {
        let mut p = tx.get_project("p")?.unwrap();
        p.config_toml.push_str("\n[[repos]]\nname='engine'\npath='crates/engine'\nintegration='engine-dev'\ngate='cargo test -p engine'\n");
        tx.update_project(&p)
    }).unwrap();
    run_at(&mut s, &two_new(), "create").unwrap();
    s.write(|tx| {
        // Simulate a legacy task from before repo snapshots existed.
        let mut old = tx.get_task("p", "T-1")?.unwrap();
        old.repo = None;
        old.base = None;
        old.gate = None;
        tx.update_task(&old)?;
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW + 20)?;
        let mut a = tx.get_attempt("p", "T-1", 1)?.unwrap();
        a.state = AttemptState::Submitted;
        tx.update_attempt(&a)
    })
    .unwrap();
    let result = run_at(
        &mut s,
        "# Staff\n- rebind heat repo=engine owns=[src/core/, tests/]\n",
        "rebind",
    )
    .unwrap();
    assert!(
        result.warnings.contains(&"rebind heat: repo api → engine, owns [src/heat/] → [src/core/, tests/]: waits for T-1; nothing changes until then (to withdraw: - rebind heat cancel)".to_string()),
        "{:?}",
        result.warnings
    );
    let before = gear_app::config::latest(&mut s, "p").unwrap();
    assert_eq!(before.repo_of_lane("heat").name, "api");
    assert!(gear_app::plan::reconcile_rebinds(&mut s, "p")
        .unwrap()
        .is_empty());
    s.write(|tx| {
        let mut a = tx.get_attempt("p", "T-1", 1)?.unwrap();
        a.state = AttemptState::Accepted;
        tx.update_attempt(&a)
    })
    .unwrap();
    assert_eq!(
        gear_app::plan::reconcile_rebinds(&mut s, "p").unwrap(),
        vec!["heat"]
    );
    let after = gear_app::config::latest(&mut s, "p").unwrap();
    assert_eq!(after.repo_of_lane("heat").name, "engine");
    assert_eq!(
        after.lanes.iter().find(|l| l.name == "heat").unwrap().owns,
        ["src/core/", "tests/"]
    );
    let old = task(&mut s, "T-1");
    assert_eq!(
        (
            old.repo.as_deref(),
            old.base.as_deref(),
            old.gate.as_deref()
        ),
        (
            Some("/repo/crates/api"),
            Some("feat/api"),
            Some("cargo test -p api")
        )
    );
    assert!(gear_app::plan::reconcile_rebinds(&mut s, "p")
        .unwrap()
        .is_empty());
    let next = run_at(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: new\naccept: done\nestimate: 10\ndeps: -\n",
        "new",
    )
    .unwrap();
    assert_eq!(
        task(&mut s, &next.created[0]).repo.as_deref(),
        Some("/repo/crates/engine")
    );
}

#[test]
fn idle_rebind_applies_at_once_and_invalid_targets_are_atomic() {
    let mut s = world();
    let error = run_at(
        &mut s,
        "# Staff\n- rebind wire repo=missing owns=[src/]\n",
        "bad",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 0);
    let result = run_at(
        &mut s,
        "# Staff\n- rebind wire repo=api owns=[src/wire/]\n",
        "idle",
    )
    .unwrap();
    assert!(
        result.warnings.contains(
            &"rebind wire: repo root → api, owns [src/wire/] → [src/wire/]: applied".to_string()
        ),
        "{:?}",
        result.warnings
    );
    assert_eq!(
        gear_app::config::latest(&mut s, "p")
            .unwrap()
            .repo_of_lane("wire")
            .name,
        "api"
    );
    assert!(parse(
        "<!-- gear-plan project=p base=0 -->\n# Staff\n- rebind wire repo=api owns=[]\n"
    )
    .is_err());
}

#[test]
fn idle_rebind_and_new_task_in_one_apply_use_the_destination_repo() {
    let mut s = world();
    run_at(&mut s, &two_new(), "old-tasks").unwrap();
    s.write(|tx| {
        // A legacy task must be snapshotted before the existing-task map is
        // loaded, even when its queue block is updated in the same apply.
        let mut old = tx.get_task("p", "T-2")?.unwrap();
        old.repo = None;
        old.base = None;
        old.gate = None;
        tx.update_task(&old)
    })
    .unwrap();
    let result = run_at(&mut s, "# Staff\n- rebind wire repo=api owns=[src/new/]\n# Queue\n## T-2 [should] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n\n## NEW [must] lane=wire\ngoal: new repo work\naccept: done\nestimate: 10\ndeps: -\n", "rebind-and-new").unwrap();
    assert!(
        result.warnings.contains(
            &"rebind wire: repo root → api, owns [src/wire/] → [src/new/]: applied".to_string()
        ),
        "{:?}",
        result.warnings
    );
    let new = task(&mut s, &result.created[0]);
    assert_eq!(
        (
            new.repo.as_deref(),
            new.base.as_deref(),
            new.gate.as_deref()
        ),
        (
            Some("/repo/crates/api"),
            Some("feat/api"),
            Some("cargo test -p api")
        )
    );
    let old = task(&mut s, "T-2");
    assert_eq!(
        (
            old.repo.as_deref(),
            old.base.as_deref(),
            old.gate.as_deref()
        ),
        (Some("/repo"), Some("dev"), Some("make test"))
    );
}

fn world_with_two_pending_attempts() -> Store {
    let mut s = world();
    s.write(|tx| {
        let mut p = tx.get_project("p")?.unwrap();
        p.config_toml.push_str("\n[[repos]]\nname='engine'\npath='crates/engine'\nintegration='engine-dev'\ngate='cargo test -p engine'\n");
        tx.update_project(&p)
    }).unwrap();
    run_at(&mut s, &two_new(), "create-pending-fixture").unwrap();
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW + 20)?;
        gear_app::tasks::start_attempt(tx, "p", "T-2", "p:wire", 1, NOW + 20)?;
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn overlapping_pending_rebinds_in_one_apply_are_refused_atomically() {
    let mut s = world_with_two_pending_attempts();
    let error = run_at(
        &mut s,
        "# Staff\n- rebind heat repo=engine owns=[src/]\n- rebind wire repo=engine owns=[src/]\n",
        "both-conflict",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("overlaps"), "{}", error.detail);
    for seat in ["p:heat", "p:wire"] {
        assert!(s
            .read(|tx| tx.get_meta(&gear_app::plan::rebind_pending_key(seat)))
            .unwrap()
            .is_none());
    }
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 1);
    assert!(gear_app::plan::reconcile_rebinds(&mut s, "p")
        .unwrap()
        .is_empty());
    run_at(
        &mut s,
        "# Staff\n- rebind heat repo=engine owns=[src/]\n",
        "retry-valid",
    )
    .unwrap();
    s.write(|tx| {
        let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
        attempt.state = AttemptState::Accepted;
        tx.update_attempt(&attempt)
    })
    .unwrap();
    assert_eq!(
        gear_app::plan::reconcile_rebinds(&mut s, "p").unwrap(),
        vec!["heat"]
    );
}

#[test]
fn overlapping_pending_rebinds_across_applies_refuse_only_the_second() {
    let mut s = world_with_two_pending_attempts();
    let first = run_at(
        &mut s,
        "# Staff\n- rebind heat repo=engine owns=[src/]\n",
        "first-valid",
    )
    .unwrap();
    assert!(
        first.warnings.contains(&"rebind heat: repo api → engine, owns [src/heat/] → [src/]: waits for T-1; nothing changes until then (to withdraw: - rebind heat cancel)".to_string()),
        "{:?}",
        first.warnings
    );
    let error = run_at(
        &mut s,
        "# Staff\n- rebind wire repo=engine owns=[src/deep/]\n",
        "second-conflict",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("overlaps"), "{}", error.detail);
    assert!(s
        .read(|tx| tx.get_meta(&gear_app::plan::rebind_pending_key("p:wire")))
        .unwrap()
        .is_none());
    s.write(|tx| {
        let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
        attempt.state = AttemptState::Accepted;
        tx.update_attempt(&attempt)
    })
    .unwrap();
    assert_eq!(
        gear_app::plan::reconcile_rebinds(&mut s, "p").unwrap(),
        vec!["heat"]
    );
    assert_eq!(
        gear_app::config::latest(&mut s, "p")
            .unwrap()
            .repo_of_lane("heat")
            .name,
        "engine"
    );
    assert!(gear_app::plan::reconcile_rebinds(&mut s, "p")
        .unwrap()
        .is_empty());
}

/// T-305: the apply line is the preview — an idle lane's rebind names the old
/// repo and owns, the new ones, and the line that undoes it.
#[test]
fn an_idle_rebinds_apply_line_names_what_changes_and_the_undo() {
    let mut s = world();
    s.write(|tx| {
        let mut p = tx.get_project("p")?.unwrap();
        p.config_toml.push_str("\n[[repos]]\nname='engine'\npath='crates/engine'\nintegration='engine-dev'\ngate='cargo test -p engine'\n");
        tx.update_project(&p)
    }).unwrap();
    let result = run_at(
        &mut s,
        "# Staff\n- rebind heat repo=engine owns=[src/core/, tests/]\n",
        "idle-undo",
    )
    .unwrap();
    assert_eq!(
        result.warnings,
        vec![
            "rebind heat: repo api → engine, owns [src/heat/] → [src/core/, tests/]: applied (to undo: - rebind heat repo=api owns=[src/heat/])".to_string()
        ]
    );
    // The undo line is a real line: once the engine has moved the lane's
    // worktree (it clears this marker), it puts the lane back.
    s.write(|tx| tx.set_meta("lane-rebind-applied:p:heat", ""))
        .unwrap();
    run_at(
        &mut s,
        "# Staff\n- rebind heat repo=api owns=[src/heat/]\n",
        "undo",
    )
    .unwrap();
    let back = gear_app::config::latest(&mut s, "p").unwrap();
    assert_eq!(back.repo_of_lane("heat").name, "api");
    assert_eq!(
        back.lanes.iter().find(|l| l.name == "heat").unwrap().owns,
        ["src/heat/"]
    );
}

/// T-305: `- rebind <lane> cancel` withdraws a recorded request, so the
/// engine applies nothing when the lane's last attempt settles.
#[test]
fn a_cancel_withdraws_a_pending_rebind_and_nothing_is_applied() {
    let mut s = world_with_two_pending_attempts();
    let waiting = run_at(
        &mut s,
        "# Staff\n- rebind heat repo=engine owns=[src/]\n",
        "pending",
    )
    .unwrap();
    assert!(
        waiting.warnings.contains(&"rebind heat: repo api → engine, owns [src/heat/] → [src/]: waits for T-1; nothing changes until then (to withdraw: - rebind heat cancel)".to_string()),
        "{:?}",
        waiting.warnings
    );
    assert!(!s
        .read(|tx| tx.get_meta(&gear_app::plan::rebind_pending_key("p:heat")))
        .unwrap()
        .unwrap()
        .is_empty());
    let cancelled = run_at(&mut s, "# Staff\n- rebind heat cancel\n", "cancel").unwrap();
    assert_eq!(
        cancelled.warnings,
        vec!["rebind heat: the pending rebind is withdrawn".to_string()]
    );
    assert_eq!(
        s.read(|tx| tx.get_meta(&gear_app::plan::rebind_pending_key("p:heat")))
            .unwrap()
            .as_deref(),
        Some("")
    );
    s.write(|tx| {
        let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
        attempt.state = AttemptState::Accepted;
        tx.update_attempt(&attempt)
    })
    .unwrap();
    assert!(gear_app::plan::reconcile_rebinds(&mut s, "p")
        .unwrap()
        .is_empty());
    let after = gear_app::config::latest(&mut s, "p").unwrap();
    assert_eq!(after.repo_of_lane("heat").name, "api");
    assert_eq!(
        after.lanes.iter().find(|l| l.name == "heat").unwrap().owns,
        ["src/heat/"]
    );
}

/// T-305: a cancel with nothing pending warns, and is refused while the lane's
/// worktree is still moving.
#[test]
fn a_cancel_with_nothing_pending_warns_and_a_moving_lane_is_refused() {
    let mut s = world();
    let result = run_at(&mut s, "# Staff\n- rebind wire cancel\n", "nothing").unwrap();
    assert_eq!(
        result.warnings,
        vec!["rebind wire: nothing pending to withdraw".to_string()]
    );
    s.write(|tx| tx.set_meta("lane-rebind-applied:p:wire", "{}"))
        .unwrap();
    let error = run_at(&mut s, "# Staff\n- rebind wire cancel\n", "moving").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error
            .detail
            .contains("lane wire is still moving to its new worktree"),
        "{}",
        error.detail
    );
}
