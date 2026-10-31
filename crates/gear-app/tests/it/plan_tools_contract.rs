//! T-473: the plan tools — a held task, an explicit departure, a milestone
//! with no history removed, and the shared-file warning.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, TaskState};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
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
        testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p3", "opencode", NOW)?;
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

/// Apply the text; a draft that departs a milestone is confirmed with its
/// line (T-461), as the captain does when the user said depart.
fn run(
    s: &mut Store,
    text: &str,
    rid: &str,
    now: i64,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let mut draft = parse(text).unwrap();
    if draft.queue.iter().any(|item| item.milestone.is_some()) {
        draft.confirmed = true;
        draft.decisions.push(
            "2026-10-27 10:05 confirmed: the voyage departs | why: the user said depart"
                .to_string(),
        );
    }
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &draft, text, rid, now)
}

/// A draft with one milestone block and one task on it; `depart` adds the
/// explicit line.
fn voyage(base: i64, depart: bool) -> String {
    format!(
        "<!-- gear-plan project=p base={base} -->\n# Milestone M1: delivery\ndone: users install it\nbar: keep it simple\neta: 8h\n{}# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        if depart { "depart: now\n" } else { "" }
    )
}

#[test]
fn a_new_task_can_be_held_out_of_delivery_and_sent_ready_later() {
    let mut s = world();
    let held = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: hold\n";
    let result = run(&mut s, held, "r1", NOW).unwrap();
    assert_eq!(result.created, vec!["T-1".to_string()]);
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.state, TaskState::Draft, "a held task is not delivered");

    let ready =
        "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-1 [must] lane=heat\nstate: ready\n";
    run(&mut s, ready, "r2", NOW + 10).unwrap();
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.state, TaskState::Ready, "ready sends the held task");
}

#[test]
fn a_ready_task_is_held_and_a_delivered_one_is_refused() {
    let mut s = world();
    let ready = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
    run(&mut s, ready, "r1", NOW).unwrap();
    let hold =
        "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-1 [must] lane=heat\nstate: hold\n";
    run(&mut s, hold, "r2", NOW + 10).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Draft
    );

    // A task the engine has already delivered is not held: its attempt is on
    // it.
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Running;
        tx.update_task(&task)
    })
    .unwrap();
    let hold_again =
        "<!-- gear-plan project=p base=2 -->\n# Queue\n## T-1 [must] lane=heat\nstate: hold\n";
    let err = run(&mut s, hold_again, "r3", NOW + 20).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail
            .contains("only a draft or ready task is held here"),
        "{}",
        err.detail
    );
}

#[test]
fn a_milestone_departs_only_on_its_explicit_depart_now_line() {
    let mut s = world();
    // A block and a task, no `depart:`: the voyage is stored but has not
    // departed.
    run(&mut s, &voyage(0, false), "r1", NOW).unwrap();
    let stored = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(stored.departure, None, "no line, no departure");
    assert_eq!(stored.title, "delivery");

    // The line arrives: the voyage departs now, with its task count and scope.
    run(&mut s, &voyage(1, true), "r2", NOW + 60).unwrap();
    let stored = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (stored.departure, stored.departure_tasks),
        (Some(NOW + 60), Some(2)),
        "the voyage departs with the tasks it has then"
    );

    // A value other than `now` is refused: a timed departure is
    // `[schedule] depart_at` in the config, not a second way.
    let timed = "<!-- gear-plan project=p base=2 -->\n# Milestone M1: delivery\ndone: users install it\nbar: keep it simple\neta: 8h\ndepart: 2026-10-31T14:00:00Z\n# Queue\n## T-1 [must] lane=heat milestone=M1\n";
    let err = run(&mut s, timed, "r3", NOW + 120).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("depart is `now`"), "{}", err.detail);
    // The refused apply wrote nothing: the departure is still the one it had.
    assert_eq!(
        s.read(|tx| tx.get_voyage("p", "M1"))
            .unwrap()
            .unwrap()
            .departure,
        Some(NOW + 60)
    );
}

#[test]
fn a_milestone_with_no_task_and_no_history_can_be_dropped() {
    let mut s = world();
    // A milestone with its words and no task: it has no departure and no
    // history.
    let block = "<!-- gear-plan project=p base=0 -->\n# Milestone M3: a spare\n done: nothing yet\nbar: keep it simple\neta: 2h\n";
    run(&mut s, block, "r1", NOW).unwrap();
    assert!(s.read(|tx| tx.get_voyage("p", "M3")).unwrap().is_some());

    let drop = "<!-- gear-plan project=p base=1 -->\n# Staff\n- drop M3\n";
    let result = run(&mut s, drop, "r2", NOW + 10).unwrap();
    assert_eq!(result.staffed, vec!["drop M3".to_string()]);
    assert!(
        s.read(|tx| tx.get_voyage("p", "M3")).unwrap().is_none(),
        "the spare milestone is gone"
    );

    // A milestone the plan still describes is not dropped: its block would
    // create it again.
    let described = "<!-- gear-plan project=p base=2 -->\n# Milestone M4: kept\n done: it ships\nbar: keep it simple\neta: 2h\n# Staff\n- drop M4\n";
    let err = run(&mut s, described, "r3", NOW + 20).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail
            .contains("is described by this plan: remove its block too"),
        "{}",
        err.detail
    );

    // A milestone with a task is not dropped.
    run(&mut s, &voyage(2, true), "r4", NOW + 30).unwrap();
    let drop_m1 = "<!-- gear-plan project=p base=3 -->\n# Staff\n- drop M1\n";
    let err = run(&mut s, drop_m1, "r5", NOW + 40).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("still has a task"), "{}", err.detail);
    // A milestone whose task left it keeps the move's decision: that is
    // history, so it stays.
    let mut s = world();
    run(&mut s, &voyage(0, true), "r1", NOW).unwrap();
    let moved = "<!-- gear-plan project=p base=1 -->\n# Milestone M2: the next one\ndone: it ships\nbar: keep it simple\neta: 2h\ndepart: now\n# Decisions\n- 2026-10-27 10:30 confirmed: T-1 moves to M2 | why: the user asked\n# Queue\n## T-1 [must] lane=heat milestone=M2\n";
    run(&mut s, moved, "r2", NOW + 10).unwrap();
    let drop_m1 = "<!-- gear-plan project=p base=2 -->\n# Staff\n- drop M1\n";
    let err = run(&mut s, drop_m1, "r3", NOW + 20).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("has decisions"),
        "the move's decision is history: {}",
        err.detail
    );
}

#[test]
fn a_dropped_milestone_id_can_be_reused_and_dropped_again() {
    let mut s = world();
    let block = "<!-- gear-plan project=p base=0 -->\n# Milestone M3: a spare\ndone: nothing yet\nbar: keep it simple\neta: 2h\n";
    run(&mut s, block, "r1", NOW).unwrap();
    let drop = "<!-- gear-plan project=p base=1 -->\n# Staff\n- drop M3\n";
    run(&mut s, drop, "r2", NOW + 10).unwrap();
    assert!(s.read(|tx| tx.get_voyage("p", "M3")).unwrap().is_none());

    // The same id again: a new voyage, no inherited history, and it can be
    // dropped again (F2: the drop's own decision is not attached to the id).
    let again = "<!-- gear-plan project=p base=2 -->\n# Milestone M3: reused\ndone: something now\nbar: keep it simple\neta: 3h\n";
    run(&mut s, again, "r3", NOW + 20).unwrap();
    assert!(s.read(|tx| tx.get_voyage("p", "M3")).unwrap().is_some());
    let drop_again = "<!-- gear-plan project=p base=3 -->\n# Staff\n- drop M3\n";
    run(&mut s, drop_again, "r4", NOW + 30).unwrap();
    assert!(
        s.read(|tx| tx.get_voyage("p", "M3")).unwrap().is_none(),
        "the reused id is dropped again"
    );
}

#[test]
fn a_plan_apply_warns_when_two_ready_tasks_on_different_lanes_name_one_file() {
    let mut s = world();
    let text = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: change crates/x/src/y.rs so the parse holds\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=data\ngoal: add a test for crates/x/src/y.rs\nevidence-files: crates/x/src/y.rs\naccept: a\nestimate: 30\ndeps: -\n";
    let result = run(&mut s, text, "r1", NOW).unwrap();
    let warning = result
        .warnings
        .iter()
        .find(|warning| warning.contains("crates/x/src/y.rs"))
        .unwrap_or_else(|| panic!("the shared-file warning: {:?}", result.warnings));
    assert!(warning.contains("T-1 (heat)"), "{warning}");
    assert!(warning.contains("T-2 (data)"), "{warning}");

    // The same lane works in order: no warning.
    let mut s = world();
    let same_lane = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: change crates/x/src/y.rs\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\ngoal: also crates/x/src/y.rs\naccept: a\nestimate: 30\ndeps: -\n";
    let result = run(&mut s, same_lane, "r1", NOW).unwrap();
    assert!(
        !result
            .warnings
            .iter()
            .any(|warning| warning.contains("crates/x/src/y.rs")),
        "{:?}",
        result.warnings
    );

    // A held task is not Ready: it is not counted.
    let mut s = world();
    let held = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: change crates/x/src/y.rs\naccept: a\nestimate: 30\ndeps: -\nstate: hold\n\n## NEW [should] lane=data\ngoal: also crates/x/src/y.rs\naccept: a\nestimate: 30\ndeps: -\n";
    let result = run(&mut s, held, "r1", NOW).unwrap();
    assert!(
        !result
            .warnings
            .iter()
            .any(|warning| warning.contains("crates/x/src/y.rs")),
        "{:?}",
        result.warnings
    );
}

#[test]
fn a_timed_voyage_departs_at_its_scheduled_depart_at_without_a_depart_line() {
    let mut s = world();
    // 2026-10-30T14:00:00Z, the instant the config names.
    let depart_at = 1_793_368_800;
    s.write(|tx| {
        let mut project = tx.get_project("p")?.expect("the project");
        project.config_toml = "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\n[quota]\nlanes = 2\ntemps = 0\n\n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\n[schedule]\ndepart_at = \"2026-10-30T14:00:00Z\"\n".into();
        tx.update_project(&project)
    })
    .unwrap();
    let text = "<!-- gear-plan project=p base=0 -->\n# Milestone M1: the timed one\ndone: it ships\nbar: keep it simple\neta: 2h\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
    run(&mut s, text, "r1", NOW).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (voyage.departure, voyage.departure_tasks),
        (Some(depart_at), Some(1)),
        "a block without depart: takes [schedule] depart_at"
    );
    assert_eq!(voyage.departure_estimate_min, Some(30));

    // A later block never moves it.
    let more = "<!-- gear-plan project=p base=1 -->\n# Milestone M1: the timed one\ndone: it ships\nbar: keep it simple\neta: 2h\n# Queue\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n";
    run(&mut s, more, "r2", NOW + 60).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (voyage.departure, voyage.departure_tasks),
        (Some(depart_at), Some(1)),
        "the scheduled departure is set once"
    );
}
