//! The captain's decision log (design §7): the lines of `# Decisions`, the
//! kinds, the star, and the entries GEAR records itself.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{GearCtx, SeatRole};
use gear_store::{testing, DecisionRow, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

fn cap() -> (Env, Fingerprint) {
    (
        Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some(CAP_PANE.into()), admin: false },
        Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
    )
}

/// Apply a plan file with the base the store holds.
fn run(s: &mut Store, body: &str, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    let text = format!("<!-- gear-plan project=p base={base} -->\n{body}");
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &parse(&text).unwrap(), &text, rid, NOW)
}

fn decisions(s: &mut Store) -> Vec<DecisionRow> {
    s.read(|tx| tx.decisions_of("p", "")).unwrap()
}

/// §7.1, §7.3: a structured line is stored once, and applying the same file
/// again changes nothing.
#[test]
fn a_structured_line_is_stored_once_over_two_applies() {
    let mut s = world();
    let line = "- 2026-10-27 10:05 scope: T-1 moves to the api lane | why: the api owns it\n";
    let result = run(&mut s, &format!("# Decisions\n{line}"), "r1").unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let logged = decisions(&mut s);
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].kind, "scope");
    assert_eq!(logged[0].what, "T-1 moves to the api lane");
    assert_eq!(logged[0].why.as_deref(), Some("the api owns it"));
    assert_eq!(logged[0].at, "2026-10-27 10:05");
    assert_eq!(logged[0].source, "plan");
    assert!(!logged[0].star);

    // The same file again: the key is the same, so nothing is added.
    run(&mut s, &format!("# Decisions\n{line}"), "r2").unwrap();
    assert_eq!(decisions(&mut s).len(), 1, "the same line is stored once");
}

/// §7.3: the table is append-only — an edited line is a new decision and the
/// old one stays.
#[test]
fn an_edited_line_is_a_new_entry() {
    let mut s = world();
    run(
        &mut s,
        "# Decisions\n- 2026-10-27 10:05 scope: T-1 moves to the api lane | why: the api owns it\n",
        "r1",
    )
    .unwrap();
    run(
        &mut s,
        "# Decisions\n- 2026-10-27 10:06 scope: T-1 moves to the api lane | why: the api owns it\n",
        "r2",
    )
    .unwrap();

    let logged = decisions(&mut s);
    assert_eq!(logged.len(), 2, "{logged:?}");
    assert_eq!(logged[0].at, "2026-10-27 10:05", "the old one stays");
    assert_eq!(logged[1].at, "2026-10-27 10:06");
}

/// §7.2: a line that is not structured is kept as a note, and the apply says
/// so.
#[test]
fn an_unstructured_line_is_a_note_with_a_warning() {
    let mut s = world();
    let result = run(&mut s, "# Decisions\n- we talked about it\n", "r1").unwrap();

    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert!(
        result.warnings[0].contains("is not structured; logged as a note"),
        "{:?}",
        result.warnings
    );
    let logged = decisions(&mut s);
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].kind, "note");
    assert_eq!(logged[0].what, "we talked about it");
    assert!(!logged[0].star);
}

/// §7.2: the star is the captain's, written right after the dash.
#[test]
fn a_star_is_read() {
    let mut s = world();
    run(
        &mut s,
        "# Decisions\n- ★ 2026-10-27 10:05 cut: T-5 is cut | why: it is not needed\n",
        "r1",
    )
    .unwrap();

    let logged = decisions(&mut s);
    assert_eq!(logged.len(), 1);
    assert!(logged[0].star, "the star is read: {logged:?}");
    assert_eq!(logged[0].kind, "cut");
    assert_eq!(logged[0].what, "T-5 is cut");
}

/// §7.1: the tail keeps its own form, and the station line is read as a
/// station (the apply of a station needs a station item, so the station is
/// read on its own here).
#[test]
fn tail_and_station_keep_their_own_form() {
    let mut s = world();
    run(&mut s, "# Decisions\n- tail: heat\n", "r1").unwrap();

    let logged = decisions(&mut s);
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].kind, "tail");
    assert_eq!(logged[0].what, "heat");

    let station = gear_app::decision::parse("station M-9: keep course");
    assert_eq!(station.kind, "station");
    assert_eq!(station.what, "keep course");
    assert!(!station.note);
}

/// §7.4: GEAR records its own entries — a fail verdict, a cancelled task, a
/// staff line — and each of them appears once.
#[test]
fn each_gear_entry_appears_once() {
    let mut s = world();
    // A task to cancel and a verdict to settle need a lane and an attempt.
    run(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    s.write(|tx| {
        gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
        gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, NOW)?;
        gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "ops/reports/T-1.md", NOW)
    })
    .unwrap();

    // A fail verdict (a rework), a cancelled task and a staff line.
    run(
        &mut s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 fail rework=\"handle the edge case\"\n",
        "r2",
    )
    .unwrap();
    run(&mut s, "# Staff\n- hold heat reason=waiting for the user\n", "r3").unwrap();

    let logged = decisions(&mut s);
    let kinds: Vec<&str> = logged.iter().map(|d| d.kind.as_str()).collect();
    assert!(kinds.contains(&"rework"), "{logged:?}");
    assert!(kinds.contains(&"staff"), "{logged:?}");
    let rework = logged.iter().find(|d| d.kind == "rework").unwrap();
    assert_eq!(rework.what, "handle the edge case");
    assert_eq!(rework.source, "gear");
    assert!(!rework.star, "GEAR's own entries are never starred");

    // The same applies again: the entries are not duplicated.
    let before = decisions(&mut s).len();
    run(
        &mut s,
        "# Staff\n- hold heat reason=waiting for the user\n",
        "r4",
    )
    .unwrap();
    assert_eq!(decisions(&mut s).len(), before, "each GEAR entry appears once");
}

/// §7.4: a pass over a failed review is an override, with the why the captain
/// wrote.
#[test]
fn a_pass_over_a_failed_review_is_an_override() {
    let mut s = world();
    run(
        &mut s,
        "# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
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

    run(
        &mut s,
        "# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 pass evidence=ops/reports/T-1.md why=\"the review missed the edge case\"\n",
        "r2",
    )
    .unwrap();

    let logged = decisions(&mut s);
    let over = logged.iter().find(|d| d.kind == "override").expect("an override");
    assert_eq!(over.what, "T-1 attempt 1 passed over a failed review");
    assert_eq!(over.why.as_deref(), Some("the review missed the edge case"));
}

/// §7.4: `gear admin init` (a config change) is GEAR's own entry too.
#[test]
fn a_config_change_is_logged() {
    const CONFIG: &str = r#"
[project]
id = "harbor"
herdr_session = "marina"
repo = "/repo"

[quota]
lanes = 2
temps = 1

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#;
    let mut s = Store::open_in_memory().unwrap();
    let env = Env { ctx: None, pane_id: None, admin: true };
    gear_app::admin::init_with_home(&mut s, &env, CONFIG, Some(crate::skill_home()), None, NOW).unwrap();

    // The first init registers the project: nothing was re-applied, so the
    // log has no entry yet.
    assert!(s.read(|tx| tx.decisions_of("harbor", "")).unwrap().is_empty());

    // A re-apply is a config change, and GEAR records it.
    gear_app::admin::init_with_home(&mut s, &env, CONFIG, Some(crate::skill_home()), None, NOW + 1).unwrap();
    let logged = s.read(|tx| tx.decisions_of("harbor", "")).unwrap();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].kind, "config");
    assert_eq!(logged[0].what, "config v2");
    assert_eq!(logged[0].source, "gear");
    assert!(!logged[0].star);

    // And again: the next config version, and its own entry.
    gear_app::admin::init_with_home(&mut s, &env, CONFIG, Some(crate::skill_home()), None, NOW + 2).unwrap();
    let logged = s.read(|tx| tx.decisions_of("harbor", "")).unwrap();
    assert_eq!(logged.len(), 2, "{logged:?}");
    assert_eq!(logged[1].what, "config v3");
}

/// §7.1: the user's own correction at the landing is a `user` decision with
/// its date — no why and no warning.
#[test]
fn a_user_correction_is_logged_with_its_date() {
    let mut s = world();
    let result = run(
        &mut s,
        "# Decisions\n- 2026-10-27 user: keep the promised export\n",
        "r1",
    )
    .unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let logged = decisions(&mut s);
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].kind, "user");
    assert_eq!(logged[0].what, "keep the promised export");
    assert_eq!(logged[0].at, "2026-10-27");
    assert_eq!(logged[0].why, None);

    // Re-applying it changes nothing.
    run(
        &mut s,
        "# Decisions\n- 2026-10-27 user: keep the promised export\n",
        "r2",
    )
    .unwrap();
    assert_eq!(decisions(&mut s).len(), 1);
}

/// §7.4: the cut entry takes the why of the decision line that names the
/// task — a whole identifier only, so `T-1` never names `T-10`.
#[test]
fn a_cancelled_task_takes_the_why_of_the_line_that_names_it() {
    /// Two tasks and the decisions that name them.
    fn world_with_two_tasks() -> Store {
        let mut s = world();
        run(
            &mut s,
            "# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n",
            "r1",
        )
        .unwrap();
        s
    }

    // T-10 is named first; the task T-1 is cancelled.
    let mut s = world_with_two_tasks();
    run(
        &mut s,
        "# Decisions\n- 2026-10-27 10:05 cut: T-10 is cut | why: ten reason\n- 2026-10-27 10:06 cut: T-1 is cut | why: one reason\n\n# Queue\n## T-1 [must] lane=heat\nstate: cancelled\ndeps: -\n",
        "r2",
    )
    .unwrap();
    let logged = decisions(&mut s);
    let cut = logged
        .iter()
        .find(|decision| decision.kind == "cut" && decision.source == "gear")
        .expect("the automatic cut");
    assert_eq!(cut.what, "T-1 cancelled");
    assert_eq!(cut.why.as_deref(), Some("one reason"), "T-1 does not name T-10");

    // Only T-10 is named: the cut has no why of its own.
    let mut s = world_with_two_tasks();
    run(
        &mut s,
        "# Decisions\n- 2026-10-27 10:05 cut: T-10 is cut | why: ten reason\n\n# Queue\n## T-1 [must] lane=heat\nstate: cancelled\ndeps: -\n",
        "r2",
    )
    .unwrap();
    let logged = decisions(&mut s);
    let cut = logged
        .iter()
        .find(|decision| decision.kind == "cut" && decision.source == "gear")
        .expect("the automatic cut");
    assert_eq!(cut.why.as_deref(), Some("(no why written)"));

    // The re-apply does not duplicate the automatic entry.
    run(
        &mut s,
        "# Decisions\n- 2026-10-27 10:05 cut: T-10 is cut | why: ten reason\n\n# Queue\n## T-1 [must] lane=heat\nstate: cancelled\ndeps: -\n",
        "r3",
    )
    .unwrap();
    assert_eq!(
        decisions(&mut s)
            .iter()
            .filter(|decision| decision.kind == "cut" && decision.source == "gear")
            .count(),
        1,
        "the automatic cut is not duplicated"
    );
}

/// §7.4: a staff line the apply carries out is logged — also when the seat
/// is already at its target.
#[test]
fn a_staff_line_is_logged_even_when_the_seat_is_already_there() {
    let mut s = world();
    run(&mut s, "# Staff\n- hold heat reason=first reason\n", "r1").unwrap();
    run(&mut s, "# Staff\n- hold heat reason=second reason\n", "r2").unwrap();

    let staff: Vec<String> = decisions(&mut s)
        .iter()
        .filter(|decision| decision.kind == "staff")
        .map(|decision| decision.what.clone())
        .collect();
    assert_eq!(
        staff,
        vec![
            "hold heat reason=first reason".to_string(),
            "hold heat reason=second reason".to_string()
        ]
    );

    // The replay of the second adds nothing.
    run(&mut s, "# Staff\n- hold heat reason=second reason\n", "r3").unwrap();
    assert_eq!(
        decisions(&mut s)
            .iter()
            .filter(|decision| decision.kind == "staff")
            .count(),
        2
    );
}

/// §7.3: a plan with no milestone block belongs to the voyage the project is
/// in (the one that departed last).
#[test]
fn a_plan_without_a_milestone_logs_to_the_current_voyage() {
    let mut s = world();
    // A voyage that departs: a milestone block and a task on it.
    run(
        &mut s,
        "# Milestone M1: the first voyage\ndone: something\nbar: a bar\neta: 1h\n\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();

    // A decisions-only apply: the line belongs to M1.
    run(
        &mut s,
        "# Decisions\n- 2026-10-27 10:05 scope: T-1 moves to the api lane | why: the api owns it\n",
        "r2",
    )
    .unwrap();
    let logged = s.read(|tx| tx.decisions_of("p", "M1")).unwrap();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].kind, "scope");

    // A staff-only apply: the entry belongs to M1 too.
    run(&mut s, "# Staff\n- hold heat reason=waiting\n", "r3").unwrap();
    let logged = s.read(|tx| tx.decisions_of("p", "M1")).unwrap();
    assert_eq!(
        logged.iter().filter(|d| d.kind == "staff").count(),
        1,
        "{logged:?}"
    );
}

/// §7.1: the structured form needs the date §7.1 writes; anything else is a
/// note with the warning and without a star.
#[test]
fn a_malformed_date_is_a_note_with_the_warning() {
    let mut s = world();
    let result = run(
        &mut s,
        "# Decisions\n- yesterday scope: export added | why: user asked\n",
        "r1",
    )
    .unwrap();
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);

    let logged = decisions(&mut s);
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0].kind, "note");
    assert!(!logged[0].star);
}
