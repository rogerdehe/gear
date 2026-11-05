//! Plan apply for features (M6.2, part 2/7).
//!
//! - a block with `feature: F-<n>` stores the mark and moves the task to the
//!   feature's branch with the feature's check, never the project's gate;
//! - a feature holds at most `[conventions].feature_max_tasks` tasks that are
//!   not cancelled;
//! - a task outside the feature never depends on one of its members: it waits
//!   for the feature's integration task;
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

/// A task outside the feature does not depend on one of its members: its wait
/// names the feature's integration task.
#[test]
fn a_dep_from_outside_on_a_member_is_refused() {
    let mut s = world();
    let error = run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\nfeature: F-1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=wire\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n",
        "r1",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error
            .detail
            .contains("deps on a feature's task T-1 name the feature's integration task"),
        "{}",
        error.detail
    );
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
