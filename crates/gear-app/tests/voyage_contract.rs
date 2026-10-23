//! Contract for a milestone as a voyage (T-22): the plan's `# Milestone <id>`
//! block must carry `done:`, `bar:` and `eta:`; they are stored with the
//! departure (the first task's creation) and the task count there, and shown
//! on the captain's home page and in the task packet. A milestone the draft
//! does not mention keeps applying (grandfathered).

use gear_app::draft::{milestone_fields, milestone_parts, parse};
use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::plan::{apply, render_task_packet};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, TaskState};
use gear_store::{testing, Store};

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
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
    )
}

fn run(s: &mut Store, text: &str, rid: &str, now: i64) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let draft = parse(text).unwrap();
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), &draft, text, rid, now)
}

/// A draft with one milestone block (body as given) and two tasks on it.
fn plan(base: i64, body: &str) -> String {
    format!(
        "<!-- gear-plan project=p base={base} -->\n# Milestone M1: delivery v0.1.0\n{body}\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\ndeps: -\n\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\ndeps: -\n"
    )
}

const FULL: &str = "done: users install v0.1.0 without Rust\nbar: block only loop breaks and data loss; log the rest\neta: 8h\n";

#[test]
fn the_milestone_block_fields_are_read() {
    assert_eq!(milestone_parts("M1: delivery v0.1.0"), ("M1", "delivery v0.1.0"));
    assert_eq!(milestone_parts("M2"), ("M2", ""));
    let (done, bar, eta) = milestone_fields(FULL);
    assert_eq!(done.as_deref(), Some("users install v0.1.0 without Rust"));
    assert_eq!(bar.as_deref(), Some("block only loop breaks and data loss; log the rest"));
    assert_eq!(eta.as_deref(), Some("8h"));
}

#[test]
fn a_milestone_without_its_three_fields_is_rejected() {
    let mut s = world();
    for (missing, body) in [
        ("done", "bar: keep it simple\neta: 8h\n"),
        ("bar", "done: users install it\neta: 8h\n"),
        ("eta", "done: users install it\nbar: keep it simple\n"),
    ] {
        let err = run(&mut s, &plan(0, body), &format!("r-{missing}"), NOW).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat, "{missing}");
        assert!(err.detail.contains("milestone M1"), "{}", err.detail);
        assert!(err.detail.contains(missing), "{}", err.detail);
    }
    // Nothing was stored by the refusals.
    assert!(s.read(|tx| tx.get_voyage("p", "M1")).unwrap().is_none());
    assert!(s.read(|tx| tx.tasks_of_project("p")).unwrap().is_empty());
}

#[test]
fn a_voyage_is_stored_with_its_departure_and_shown() {
    let mut s = world();
    run(&mut s, &plan(0, FULL), "r1", NOW).unwrap();

    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (
            voyage.title.as_str(),
            voyage.done.as_deref(),
            voyage.bar.as_deref(),
            voyage.eta.as_deref()
        ),
        (
            "delivery v0.1.0",
            Some("users install v0.1.0 without Rust"),
            Some("block only loop breaks and data loss; log the rest"),
            Some("8h")
        )
    );
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW), Some(2)));

    // The task packet shows it.
    let packet = s.read(|tx| render_task_packet(tx, "p", "T-1")).unwrap();
    assert!(packet.contains("Voyage M1 delivery v0.1.0"), "{packet}");
    assert!(packet.contains("done: users install v0.1.0 without Rust"), "{packet}");
    assert!(packet.contains("bar: block only loop breaks"), "{packet}");
    assert!(packet.contains("eta 8h"), "{packet}");
    assert!(packet.contains("departed 2027-01-15 with 2 tasks"), "{packet}");

    // The captain's home page shows it before the inbox.
    let (env, fp) = cap();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 10).unwrap();
    assert!(page.text.contains("Voyage M1 delivery v0.1.0"), "{}", page.text);
    assert!(page.text.contains("done: users install v0.1.0 without Rust"), "{}", page.text);

    // A later apply updates the words but never moves the departure.
    let later = plan(1, "done: users install v0.1.1 without Rust\nbar: keep only loop breaks\neta: 9h\n");
    run(&mut s, &later, "r2", NOW + 100).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (voyage.done.as_deref(), voyage.eta.as_deref()),
        (Some("users install v0.1.1 without Rust"), Some("9h"))
    );
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW), Some(2)));
}

#[test]
fn an_untouched_old_milestone_keeps_applying() {
    let mut s = world();
    // A plan with tasks on M1 but no milestone block: no voyage is created and
    // nothing is rejected — old plans keep applying (grandfathered).
    let text = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\ndeps: -\n";
    run(&mut s, text, "r1", NOW).unwrap();
    let tasks = s.read(|tx| tx.tasks_of_project("p")).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].milestone.as_deref(), Some("M1"));
    assert_eq!(tasks[0].state, TaskState::Ready);
    // The departure is recorded (the tasks exist); the words wait for a block.
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW), Some(1)));
    assert_eq!((voyage.done, voyage.bar, voyage.eta), (None, None, None));
    let packet = s.read(|tx| render_task_packet(tx, "p", "T-1")).unwrap();
    assert!(packet.contains("Voyage M1"), "{packet}");
    assert!(packet.contains("not described yet"), "{packet}");
}

/// R1 (review): a milestone whose block was applied earlier departs when its
/// first **tasks** are created — a queue-only apply, no repeated block — and a
/// later block/task addition never moves the departure.
#[test]
fn the_departure_is_the_first_task_creation_even_without_the_block() {
    let mut s = world();
    // base 0: the block only, no tasks.
    let block_only = "<!-- gear-plan project=p base=0 -->\n# Milestone M1: release\ndone: users install it\nbar: block only loop breaks\neta: 8h\n";
    run(&mut s, block_only, "r1", NOW).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!((voyage.departure, voyage.departure_tasks), (None, None), "no task yet: no departure");

    // base 1: only the queue, one task on M1 — the milestone departs now.
    let first_task = "<!-- gear-plan project=p base=1 -->\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\ndeps: -\n";
    run(&mut s, first_task, "r2", NOW + 10).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW + 10), Some(1)));
    assert!(s.read(|tx| render_task_packet(tx, "p", "T-1")).unwrap().contains("with 1 tasks"));

    // base 2: the block again plus a second task — the words update, the
    // departure stays.
    let more = "<!-- gear-plan project=p base=2 -->\n# Milestone M1: release v2\ndone: users install v2\nbar: keep it simple\neta: 9h\n# Queue\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\ndeps: -\n";
    run(&mut s, more, "r3", NOW + 20).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW + 10), Some(1)));
    assert_eq!(voyage.done.as_deref(), Some("users install v2"));
}

/// R2 (review): a grandfathered milestone whose block arrives later keeps the
/// departure it had when its first task was created, even after another edit.
#[test]
fn editing_a_grandfathered_milestone_keeps_its_historical_departure() {
    let mut s = world();
    // T-1 is created on M1 with no block at all.
    let first = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\ndeps: -\n";
    run(&mut s, first, "r1", NOW).unwrap();
    // Much later the block arrives together with T-2.
    let block = "<!-- gear-plan project=p base=1 -->\n# Milestone M1: release\ndone: users install it\nbar: block only loop breaks\neta: 8h\n# Queue\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\ndeps: -\n";
    run(&mut s, block, "r2", NOW + 100).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (voyage.departure, voyage.departure_tasks),
        (Some(NOW), Some(1)),
        "the departure is the first task's creation, not the edit"
    );
    // Another metadata edit: still unchanged.
    let edit = "<!-- gear-plan project=p base=2 -->\n# Milestone M1: release v2\ndone: users install v2\nbar: keep it simple\neta: 9h\n# Queue\n";
    run(&mut s, edit, "r3", NOW + 200).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW), Some(1)));
    assert_eq!(voyage.done.as_deref(), Some("users install v2"));
}

/// R3 (review): the captain's snapshot shows the voyage that has work, even
/// when three earlier milestones exist without any.
#[test]
fn the_snapshot_shows_the_voyage_that_has_work() {
    let mut s = world();
    // Four voyages; only M4 carries a task.
    let mut text = String::from("<!-- gear-plan project=p base=0 -->\n");
    for id in ["M1", "M2", "M3", "M4"] {
        text.push_str(&format!(
            "# Milestone {id}: voyage {id}\ndone: {id} done\nbar: {id} bar\neta: 8h\n"
        ));
    }
    text.push_str("# Queue\n## NEW [must] lane=heat milestone=M4\ngoal: A\naccept: a\ndeps: -\n");
    run(&mut s, &text, "r1", NOW).unwrap();

    let (env, fp) = cap();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 10).unwrap();
    assert!(page.text.contains("Voyage M4 voyage M4"), "{}", page.text);
    assert!(page.text.contains("done: M4 done"), "{}", page.text);
    assert!(page.text.contains("+1 more voyages"), "{}", page.text);
}
