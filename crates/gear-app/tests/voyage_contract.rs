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
            voyage.done.as_str(),
            voyage.bar.as_str(),
            voyage.eta.as_str()
        ),
        (
            "delivery v0.1.0",
            "users install v0.1.0 without Rust",
            "block only loop breaks and data loss; log the rest",
            "8h"
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
    assert_eq!((voyage.done.as_str(), voyage.eta.as_str()), ("users install v0.1.1 without Rust", "9h"));
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
    assert!(s.read(|tx| tx.get_voyage("p", "M1")).unwrap().is_none());
}
