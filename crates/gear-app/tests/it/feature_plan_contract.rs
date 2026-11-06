//! Plan apply for features (M6.2, part 2/7).
//!
//! - a block with `feature: F-<n>` stores the mark and moves the task to the
//!   feature's branch with the feature's check, never the project's gate;
//! - a feature holds at most `[conventions].feature_max_tasks` tasks that are
//!   not cancelled;
//! - a task outside the feature never depends on one of its members: the wait
//!   is rewritten to the feature's integration task, and the apply says so
//!   (T-587);
//! - a task that has started joins no feature (T-587);
//! - a member waits on an unmerged dep outside its feature, so the feature's
//!   branch is cut from an integration branch that holds it (T-587);
//! - each feature gets one integration task, created in the apply that first
//!   names a member and kept equal to the members that are not cancelled;
//! - the members' issue moves to that integration task;
//! - a project with no `feature:` line is untouched.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, TaskState};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

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

[[lanes]]
name = "wire"
duty = "the wire format"
owns = ["src/wire/"]
kind = "opencode"
profile = "opencode"

[conventions]
integration = "dev"
release = "main"
gate = "make test"
"#;

fn world_with(config: &str) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", NOW)?;
        project.config_toml = config.to_string();
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

fn world() -> Store {
    world_with(CONFIG)
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

fn task(s: &mut Store, id: &str) -> gear_store::TaskRow {
    s.read(|tx| tx.get_task("p", id)).unwrap().unwrap()
}

/// Mark tasks Done: the feature's members and its carrier have landed.
fn land(s: &mut Store, ids: &[&str]) {
    s.write(|tx| {
        for id in ids {
            let mut row = tx.get_task("p", id)?.expect("the task");
            row.state = TaskState::Done;
            tx.update_task(&row)?;
        }
        Ok(())
    })
    .unwrap();
}

/// The first attempt of a task, started: the task has begun (T-587).
fn start(s: &mut Store, id: &str) {
    s.write(|tx| {
        tx.insert_attempt(&attempt(id, NOW))?;
        tx.set_attempt_time("p", id, 1, gear_store::AttemptStep::Started, NOW)
    })
    .unwrap();
}

/// The task's work merged: Done, with an attempt whose merge landed (T-587).
fn merge(s: &mut Store, id: &str) {
    s.write(|tx| {
        let mut row = tx.get_task("p", id)?.expect("the task");
        row.state = TaskState::Done;
        tx.update_task(&row)?;
        tx.insert_attempt(&attempt(id, NOW))?;
        tx.set_attempt_time("p", id, 1, gear_store::AttemptStep::Merged, NOW + 5)
    })
    .unwrap();
}

fn attempt(id: &str, at: i64) -> gear_store::AttemptRow {
    gear_store::AttemptRow {
        project: "p".into(),
        task: id.into(),
        n: 1,
        seat: "p:heat".into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state: gear_model::AttemptState::Started,
        candidate: None,
        report: None,
        safe_point: None,
        started: at,
        updated: at,
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
    }
}

/// A plan of three members of feature F-1: two on `heat`, one on `wire`.
fn three_members() -> String {
    "# Queue\n## NEW [must] lane=heat\nfeature: F-1\nissue: #12\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\nfeature: F-1\nissue: #12\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n\n## NEW [could] lane=wire\nfeature: F-1\nissue: #12\ngoal: C\naccept: c\nestimate: 30\ndeps: T-1\n".to_string()
}

/// Two members and the feature's integration task: the members' base and gate
/// are the feature's, the carrier's own are its lane's, it takes the lane with
/// most members, the members' deps and their issue.
#[test]
fn a_feature_gets_one_integration_task_and_its_members_its_branch() {
    let mut s = world();
    let result = run(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{}", three_members()),
        "r1",
    )
    .unwrap();

    // The carrier is the one task the apply created beyond the draft's three,
    // and the store names it as the feature's.
    let carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .expect("the feature has an integration task");
    assert!(
        result.created.contains(&carrier),
        "the carrier is created by this apply: {:?}",
        result.created
    );

    for member in ["T-1", "T-2", "T-3"] {
        let row = task(&mut s, member);
        assert_eq!(
            row.base.as_deref(),
            Some("p/feature/f-1"),
            "{member} works on the feature's branch"
        );
        assert_eq!(
            row.gate.as_deref(),
            Some("true"),
            "{member} runs no gate: the feature's own check is none"
        );
        assert_eq!(row.issue, None, "{member} keeps no issue");
        assert_eq!(
            s.read(|tx| tx.task_feature("p", member))
                .unwrap()
                .as_deref(),
            Some("F-1"),
            "{member} is marked with the feature"
        );
    }

    let row = task(&mut s, &carrier);
    assert_eq!(
        row.deps,
        vec!["T-1".to_string(), "T-2".to_string(), "T-3".to_string()],
        "the carrier waits for every member"
    );
    assert_eq!(
        row.lane.as_deref(),
        Some("heat"),
        "the lane holding most members"
    );
    assert_eq!(row.priority, "must", "the highest priority of the members");
    assert_eq!(
        row.issue,
        Some(12),
        "the members' issue moves to the carrier"
    );
    assert_eq!(row.estimate_min, Some(10), "the carrier is a small task");
    assert_eq!(
        row.base.as_deref(),
        Some("dev"),
        "the carrier lands on the integration branch"
    );
    assert_eq!(
        row.gate.as_deref(),
        Some("make test"),
        "the carrier meets the full gate"
    );
    assert_eq!(
        s.read(|tx| tx.task_branch_override("p", &carrier))
            .unwrap()
            .as_deref(),
        Some("p/feature/f-1"),
        "the carrier's candidate is the feature's branch"
    );
    let contract = s
        .read(|tx| tx.get_contract("p", &carrier, 1))
        .unwrap()
        .unwrap();
    assert!(
        contract.goal.starts_with("Integrate feature F-1:"),
        "{}",
        contract.goal
    );
    for goal in ["A", "B", "C"] {
        assert!(
            contract.goal.lines().any(|line| line.trim() == goal),
            "the members' goals are one per line: {}",
            contract.goal
        );
    }
    assert!(
        contract.accept.contains("merged into dev"),
        "{}",
        contract.accept
    );
}

/// A project that names a `feature_check` runs it on the feature's branch: the
/// members' gate is that command, the carrier's stays the full gate.
#[test]
fn a_configured_feature_check_is_the_members_gate() {
    let mut s = world_with(&CONFIG.replace(
        "gate = \"make test\"",
        "gate = \"make test\"\nfeature_check = \"make feature-check\"",
    ));
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    let carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .unwrap();
    assert_eq!(
        task(&mut s, "T-1").gate.as_deref(),
        Some("make feature-check")
    );
    assert_eq!(
        task(&mut s, &carrier).gate.as_deref(),
        Some("make test"),
        "the carrier meets the full gate"
    );
}

/// A feature holds at most `[conventions].feature_max_tasks` tasks that are not
/// cancelled: the eleventh is refused, naming the feature and the cap.
#[test]
fn a_member_over_the_cap_is_refused() {
    let mut s = world();
    let mut body = String::from("# Queue\n");
    for n in 0..11 {
        body.push_str(&format!(
            "## NEW [must] lane=heat\nfeature: F-1\ngoal: task {n}\naccept: a\nestimate: 30\ndeps: -\n\n"
        ));
    }
    let error = run_at(&mut s, &body, "r1").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("feature F-1")
            && error.detail.contains("cap of 10")
            && error.detail.contains("feature_max_tasks"),
        "the refusal names the feature and the cap: {}",
        error.detail
    );
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap(),
        None,
        "a refused apply writes nothing"
    );
}

/// T-587: a task that has started joins no feature. Its base and gate never
/// change: the apply is refused and nothing of it is written.
#[test]
fn a_running_task_joining_a_feature_is_refused_and_keeps_its_base() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    start(&mut s, "T-1");

    let error = run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\nfeature: F-1\ndeps: -\n",
        "r2",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains(
            "T-1 has started: it finishes on the integration branch; only a task with no attempt joins a feature"
        ),
        "{}",
        error.detail
    );
    let row = task(&mut s, "T-1");
    assert_eq!(
        row.base.as_deref(),
        Some("dev"),
        "a started task keeps its lane's integration branch"
    );
    assert_eq!(row.gate.as_deref(), Some("make test"));
    assert_eq!(s.read(|tx| tx.task_feature("p", "T-1")).unwrap(), None);
    assert_eq!(
        s.read(|tx| tx.feature_carrier("p", "F-1")).unwrap(),
        None,
        "a refused apply mints no carrier"
    );
}

/// T-587: a task outside a feature waits for the feature's integration task,
/// not for one of its members — and the apply says so in one line. A wait the
/// task already has on the carrier is not written twice.
#[test]
fn an_outside_dep_on_a_member_names_the_integration_task() {
    let mut s = world();
    let result = run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n",
        "r1",
    )
    .unwrap();
    let carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .expect("the feature's integration task");
    assert_eq!(
        task(&mut s, "T-2").deps,
        vec![carrier.clone()],
        "the outside task waits for the carrier"
    );
    assert_eq!(
        result.rewritten,
        vec![format!("T-2: dep T-1 -> {carrier} (feature F-1)")],
        "the apply names the rewrite"
    );

    // The same wait, now written together with the carrier: one entry, and the
    // member's dep is rewritten again without doubling the carrier.
    let later = run_at(
        &mut s,
        &format!("# Queue\n## T-2 [should] lane=wire\ndeps: T-1, {carrier}\n"),
        "r2",
    )
    .unwrap();
    assert_eq!(
        task(&mut s, "T-2").deps,
        vec![carrier.clone()],
        "the carrier is named once"
    );
    assert_eq!(
        later.rewritten,
        vec![format!("T-2: dep T-1 -> {carrier} (feature F-1)")],
        "one line per rewrite"
    );
}

/// T-587: a member whose dep lies outside the feature and has not merged yet
/// holds every member of that feature; once the dep has merged it is ignored.
#[test]
fn a_member_waits_for_an_unmerged_outside_dep_and_not_for_a_merged_one() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=wire\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\nfeature: F-1\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n\n## NEW [could] lane=heat\nfeature: F-1\ngoal: C\naccept: c\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    // T-1 is outside the feature and unmerged: both members wait for it, not
    // only the one that names it.
    assert_eq!(task(&mut s, "T-2").deps, vec!["T-1".to_string()]);
    assert_eq!(
        task(&mut s, "T-3").deps,
        vec!["T-1".to_string()],
        "no member is delivered before the outside dep merged"
    );

    merge(&mut s, "T-1");
    run_at(
        &mut s,
        "# Queue\n## T-2 [should] lane=heat\nfeature: F-1\ndeps: T-1\n\n## T-3 [could] lane=heat\nfeature: F-1\ndeps: -\n",
        "r2",
    )
    .unwrap();
    assert_eq!(
        task(&mut s, "T-3").deps,
        Vec::<String>::new(),
        "a merged outside dep is ignored"
    );
    assert_eq!(task(&mut s, "T-2").deps, vec!["T-1".to_string()]);
}

/// T-624: a member whose block drops its outside dep releases every other
/// member on the next apply: the spread is computed from the members' own
/// deps, not from the stored deps of the previous apply.
#[test]
fn a_member_dropping_its_outside_dep_releases_every_other_member() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=wire\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\nfeature: F-1\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n\n## NEW [could] lane=heat\nfeature: F-1\ngoal: C\naccept: c\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    assert_eq!(task(&mut s, "T-2").deps, vec!["T-1".to_string()]);
    assert_eq!(task(&mut s, "T-3").deps, vec!["T-1".to_string()]);

    // T-2's block drops its outside dep: no member declares T-1 any more.
    run_at(
        &mut s,
        "# Queue\n## T-2 [should] lane=heat\nfeature: F-1\ndeps: -\n\n## T-3 [could] lane=heat\nfeature: F-1\ndeps: -\n",
        "r2",
    )
    .unwrap();
    assert_eq!(
        task(&mut s, "T-2").deps,
        Vec::<String>::new(),
        "the member that dropped the dep is released"
    );
    assert_eq!(
        task(&mut s, "T-3").deps,
        Vec::<String>::new(),
        "every other member is released too"
    );
}

/// T-624: a member's own `deps: -` is kept while another member still
/// declares the outside dep, and the output names that member.
#[test]
fn a_member_keeps_its_own_deps_and_the_output_names_the_declaring_member() {
    let mut s = world();
    let result = run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=wire\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\nfeature: F-1\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n\n## NEW [could] lane=heat\nfeature: F-1\ngoal: C\naccept: c\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    assert!(
        result
            .warnings
            .iter()
            .any(|line| line == "T-3 waits on T-1 (F-1: T-2 needs it)"),
        "{:?}",
        result.warnings
    );
    assert_eq!(task(&mut s, "T-3").deps, vec!["T-1".to_string()]);
}

/// T-624: `plan show` keeps the spread out of `deps:` and prints it on its
/// own line.
#[test]
fn plan_show_keeps_the_spread_out_of_deps() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=wire\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\nfeature: F-1\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n\n## NEW [could] lane=heat\nfeature: F-1\ngoal: C\naccept: c\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    let out = s.read(|tx| gear_app::plan::show(tx, "p", "T-3")).unwrap();
    assert!(out.contains("deps: -\n"), "{out}");
    assert!(!out.contains("deps: T-1"), "{out}");
    assert!(out.contains("waits (feature F-1): T-1"), "{out}");
}

/// A later apply adds a member: the carrier's deps stay equal to the members,
/// and it keeps the issue.
#[test]
fn a_re_apply_adds_a_member_to_the_carriers_deps() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\nissue: #12\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\nfeature: F-1\nissue: #12\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    let carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .unwrap();

    let added = run_at(
        &mut s,
        "# Queue\n## NEW [could] lane=wire\nfeature: F-1\nissue: #12\ngoal: C\naccept: c\nestimate: 30\ndeps: -\n",
        "r2",
    )
    .unwrap();
    let third = added.created[0].clone();
    let row = task(&mut s, &carrier);
    assert_eq!(
        row.deps,
        vec!["T-1".to_string(), "T-2".to_string(), third.clone()],
        "the carrier waits for every member"
    );
    assert_eq!(row.issue, Some(12), "the carrier keeps the issue");
    assert_eq!(
        s.read(|tx| tx.feature_carrier("p", "F-1"))
            .unwrap()
            .as_deref(),
        Some(carrier.as_str()),
        "one integration task per feature"
    );
}

/// A plan with no `feature:` line is untouched: no member, no integration task,
/// the lane's integration branch and full gate on every task.
#[test]
fn a_plan_without_a_feature_line_is_unchanged() {
    let mut s = world();
    let result = run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n",
        "r1",
    )
    .unwrap();
    assert_eq!(
        result.created,
        vec!["T-1".to_string(), "T-2".to_string()],
        "no task is added"
    );
    assert_eq!(s.read(|tx| tx.feature_carrier("p", "F-1")).unwrap(), None);
    for id in ["T-1", "T-2"] {
        let row = task(&mut s, id);
        assert_eq!(row.base.as_deref(), Some("dev"));
        assert_eq!(row.gate.as_deref(), Some("make test"));
        assert_eq!(s.read(|tx| tx.task_branch_override("p", id)).unwrap(), None);
    }
}

/// T-578 F1: a task whose feature changes moves to that feature's branch and
/// check. The stored mark is read before the write, so the change is seen: an
/// existing task keeps neither its lane's integration branch nor its full gate.
#[test]
fn an_existing_tasks_feature_change_moves_its_branch_and_gate() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    let row = task(&mut s, "T-1");
    assert_eq!(row.base.as_deref(), Some("dev"));
    assert_eq!(row.gate.as_deref(), Some("make test"));

    run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\nfeature: F-1\ndeps: -\n",
        "r2",
    )
    .unwrap();

    let row = task(&mut s, "T-1");
    assert_eq!(
        row.base.as_deref(),
        Some("p/feature/f-1"),
        "an existing task moves to the feature's branch"
    );
    assert_eq!(
        row.gate.as_deref(),
        Some("true"),
        "its merge runs the feature's check, not the full gate"
    );
    assert_eq!(
        s.read(|tx| tx.task_feature("p", "T-1")).unwrap().as_deref(),
        Some("F-1"),
        "the mark is stored"
    );
    let carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .expect("the feature gets its integration task");
    assert_ne!(carrier, "T-1");
    assert_eq!(
        task(&mut s, &carrier).deps,
        vec!["T-1".to_string()],
        "the carrier waits for the member it now has"
    );
}

/// T-578 F1: clearing the feature (`feature: -`) puts the task back on its
/// lane's integration branch with the project's full gate.
#[test]
fn clearing_a_feature_restores_the_lanes_branch_and_gate() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    assert_eq!(
        task(&mut s, "T-1").base.as_deref(),
        Some("p/feature/f-1"),
        "a new member starts on the feature's branch"
    );

    run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\nfeature: -\ndeps: -\n",
        "r2",
    )
    .unwrap();

    let row = task(&mut s, "T-1");
    assert_eq!(
        row.base.as_deref(),
        Some("dev"),
        "a task whose feature is cleared is back on its lane's integration branch"
    );
    assert_eq!(
        row.gate.as_deref(),
        Some("make test"),
        "and it meets the full gate again"
    );
    assert_eq!(s.read(|tx| tx.task_feature("p", "T-1")).unwrap(), None);
}

/// T-578 F2: a feature needs a carrier only while it has a member that is not
/// terminal. Once every member landed, a later apply mints no second
/// integration task, even when it names the feature again.
#[test]
fn a_re_apply_of_a_landed_feature_creates_no_carrier() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    let carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .expect("the feature has an integration task");
    land(&mut s, &["T-1", &carrier]);

    let later = run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\nfeature: F-1\ndeps: -\n\n## NEW [should] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n",
        "r2",
    )
    .unwrap();

    assert_eq!(
        later.created.len(),
        1,
        "only the new task is created: {:?}",
        later.created
    );
    assert!(
        !later.created.contains(&carrier),
        "the landed carrier is not minted again: {:?}",
        later.created
    );
    assert_eq!(
        s.read(|tx| tx.feature_carrier("p", "F-1"))
            .unwrap()
            .as_deref(),
        Some(carrier.as_str()),
        "the landed feature keeps the carrier it had"
    );
    assert_eq!(
        task(&mut s, &carrier).deps,
        vec!["T-1".to_string()],
        "its deps are not wiped"
    );
}

/// T-578 F2: a member added after the feature landed gets a fresh carrier,
/// waiting only for the members that are still open; the landed carrier keeps
/// its own deps.
#[test]
fn a_member_added_after_the_feature_landed_gets_a_new_carrier() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    let landed_carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .expect("the feature has an integration task");
    land(&mut s, &["T-1", &landed_carrier]);

    let later = run_at(
        &mut s,
        "# Queue\n## NEW [should] lane=wire\nfeature: F-1\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n",
        "r2",
    )
    .unwrap();
    let member = later.created[0].clone();
    let new_carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .expect("the feature has a carrier again");
    assert_ne!(
        new_carrier, landed_carrier,
        "a member added after the landing starts a fresh carrier"
    );
    assert!(
        later.created.contains(&new_carrier),
        "the new carrier is created by this apply: {:?}",
        later.created
    );
    assert_eq!(
        task(&mut s, &new_carrier).deps,
        vec![member.clone()],
        "it waits for the open member only"
    );
    assert_eq!(
        task(&mut s, &landed_carrier).deps,
        vec!["T-1".to_string()],
        "the landed carrier keeps its own deps"
    );
    assert_eq!(
        task(&mut s, &member).base.as_deref(),
        Some("p/feature/f-1"),
        "the new member works on the feature's branch"
    );
}

/// A done member that is not merged into the feature branch stays one of the
/// carrier's deps when the plan is applied again (Lawrence M2: T-94 was done,
/// an apply moved it to another lane and dropped it from T-100's deps, and
/// T-100 was handed in without it).
#[test]
fn a_done_member_stays_a_dep_of_its_carrier_on_a_later_apply() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n## NEW [must] lane=heat\nfeature: F-1\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    let carrier = s
        .read(|tx| tx.feature_carrier("p", "F-1"))
        .unwrap()
        .expect("the feature has an integration task");
    // Both members are done (passed review), neither merged.
    land(&mut s, &["T-1", "T-2"]);
    s.write(|tx| {
        let mut row = tx.get_task("p", &carrier)?.expect("the carrier");
        row.deps = vec!["T-2".to_string()];
        tx.update_task(&row)
    })
    .unwrap();
    run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=wire\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r2",
    )
    .unwrap();
    assert_eq!(
        task(&mut s, &carrier).deps,
        vec!["T-1".to_string(), "T-2".to_string()],
        "the done member is still waited for"
    );
}
