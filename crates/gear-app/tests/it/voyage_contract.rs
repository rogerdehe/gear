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

/// The project's cap on a task's planned time, raised (T-279): the tests that
/// plan long tasks do it to exercise the voyage's pace and scope arithmetic.
fn lift_the_cap(s: &mut Store, max_task_min: u32) {
    s.write(|tx| {
        let mut project = tx.get_project("p")?.unwrap();
        project.config_toml = format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\n\
             [quota]\nlanes = 2\ntemps = 0\n\n\
             [captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\n\
             [conventions]\ngate = \"make test\"\nmax_task_min = {max_task_min}\n"
        );
        tx.update_project(&project)
    })
    .unwrap();
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
    apply(s, &env, Some(&fp), None, &draft, text, rid, now)
}

/// A draft with one milestone block (body as given) and two tasks on it.
fn plan(base: i64, body: &str) -> String {
    format!(
        "<!-- gear-plan project=p base={base} -->\n# Milestone M1: delivery v0.1.0\n{body}\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n"
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

/// T-387: the `eta:` is a duration GEAR measures the voyage against, and
/// nothing else. A block whose eta reads as prose is refused with nothing
/// written — for a new voyage and for an edit of one that already departed.
#[test]
fn an_eta_that_is_not_a_duration_is_refused() {
    let mut s = world();
    // Durations apply: one group, then several.
    run(&mut s, &plan(0, "done: users install it\nbar: keep it simple\neta: 8h\n"), "r1", NOW).unwrap();
    run(
        &mut s,
        &plan(1, "done: users install it\nbar: keep it simple\neta: 1d 4h\n"),
        "r2",
        NOW + 10,
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap().eta.as_deref(),
        Some("1d 4h")
    );

    // Prose and a style word after a duration are refused, and the stored
    // voyage keeps the eta it had.
    for (n, eta) in ["8h, hard", "do it properly"].into_iter().enumerate() {
        // A refused apply leaves the plan version where it was.
        let base = s.read(|tx| tx.plan_version("p")).unwrap();
        let text = plan(base, &format!("done: users install it\nbar: keep it simple\neta: {eta}\n"));
        let err = run(&mut s, &text, &format!("r-bad-{n}"), NOW + 20 + n as i64).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat, "{eta}");
        assert!(
            err.detail.contains(
                "milestone M1: eta must be a duration GEAR measures the voyage against, e.g. `eta: 8h` or `eta: 1d 4h`; reasons go in # Decisions"
            ),
            "{}",
            err.detail
        );
        assert!(err.detail.contains(&format!("got \"{eta}\"")), "{}", err.detail);
    }
    assert_eq!(
        s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap().eta.as_deref(),
        Some("1d 4h")
    );

    // A fresh plan carrying such an eta writes nothing: no voyage, no task, no
    // departure — the voyage cannot depart.
    let mut fresh = world();
    for (n, eta) in ["8h, hard", "do it properly"].into_iter().enumerate() {
        let text = plan(0, &format!("done: users install it\nbar: keep it simple\neta: {eta}\n"));
        let err = run(&mut fresh, &text, &format!("r-fresh-{n}"), NOW).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat, "{eta}");
        assert!(fresh.read(|tx| tx.get_voyage("p", "M1")).unwrap().is_none(), "{eta}");
        assert!(fresh.read(|tx| tx.tasks_of_project("p")).unwrap().is_empty(), "{eta}");
    }
    assert_eq!(fresh.read(|tx| tx.plan_version("p")).unwrap(), 0);

    // The refusal shows the first 40 characters of what it got, no more.
    let long = "do it properly and carefully, this one is long";
    let text = plan(0, &format!("done: users install it\nbar: keep it simple\neta: {long}\n"));
    let err = run(&mut fresh, &text, "r-long", NOW).unwrap_err();
    assert!(
        err.detail.contains(&format!("got \"{}\"", long.chars().take(40).collect::<String>())),
        "{}",
        err.detail
    );
    assert!(!err.detail.contains("is long"), "{}", err.detail);
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
    let text = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
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
    let first_task = "<!-- gear-plan project=p base=1 -->\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
    run(&mut s, first_task, "r2", NOW + 10).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW + 10), Some(1)));
    assert!(s.read(|tx| render_task_packet(tx, "p", "T-1")).unwrap().contains("with 1 tasks"));

    // base 2: the block again plus a second task — the words update, the
    // departure stays.
    let more = "<!-- gear-plan project=p base=2 -->\n# Milestone M1: release v2\ndone: users install v2\nbar: keep it simple\neta: 9h\n# Queue\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n";
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
    let first = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
    run(&mut s, first, "r1", NOW).unwrap();
    // Much later the block arrives together with T-2.
    let block = "<!-- gear-plan project=p base=1 -->\n# Milestone M1: release\ndone: users install it\nbar: block only loop breaks\neta: 8h\n# Queue\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n";
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

/// R3 (review) of T-22, narrowed by T-246: the snapshot lists only the
/// voyage with open work, and names no other.
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
    text.push_str("# Queue\n## NEW [must] lane=heat milestone=M4\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n");
    run(&mut s, &text, "r1", NOW).unwrap();

    let (env, fp) = cap();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 10).unwrap();
    assert!(page.text.contains("Voyage M4 voyage M4"), "{}", page.text);
    assert!(page.text.contains("done: M4 done"), "{}", page.text);
    let snapshot = &page.text[page.text.find("SNAPSHOT").expect("the snapshot")..];
    assert!(!snapshot.contains("Voyage M3 voyage M3"), "only the voyage with work: {snapshot}");
    assert!(!page.text.contains("more voyages"), "every voyage is its own: {}", page.text);
}

/// T-246: each voyage is independent. A voyage that landed is not listed with
/// the one that carries the work, and nothing is counted across them.
#[test]
fn the_snapshot_shows_only_the_current_voyage() {
    let mut s = world();
    // M1 departed earlier and landed: its only task is done.
    let first = "<!-- gear-plan project=p base=0 -->\n# Milestone M1: the first voyage\ndone: it ships\nbar: stable\neta: 4h\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
    run(&mut s, first, "r1", NOW - 7200).unwrap();
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.expect("the task");
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();
    // M2 departs later with one done and one ready task.
    let second = "<!-- gear-plan project=p base=1 -->\n# Milestone M2: the second voyage\ndone: it ships too\nbar: stable\neta: 8h\n# Queue\n## NEW [must] lane=heat milestone=M2\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat milestone=M2\ngoal: C\naccept: c\nestimate: 30\ndeps: T-2\n";
    run(&mut s, second, "r2", NOW - 60).unwrap();
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-2")?.expect("the task");
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();

    let (env, fp) = cap();
    let page = home::build(&mut s, &env, Some(&fp), NOW).unwrap();
    let snapshot = &page.text[page.text.find("SNAPSHOT").expect("the snapshot")..];
    // The voyage line of M2, the landed M1 nowhere, and no count of the rest.
    assert!(snapshot.contains("Voyage M2 the second voyage"), "{snapshot}");
    assert!(!snapshot.contains("Voyage M1"), "the landed voyage is history: {snapshot}");
    assert!(!page.text.contains("more voyages"), "{}", page.text);
    // The totals are the current voyage's two tasks, not all three.
    assert!(snapshot.contains("SNAPSHOT 2 tasks · 1 done"), "{snapshot}");
}

/// T-39: one shared five-line report precedes the snapshot and inbox, with a
/// measured pace rather than counting unstarted estimates as completed work.
#[test]
fn voyage_report_starts_home_and_matches_both_board_views() {
    use gear_model::AttemptState;
    use gear_store::{AttemptRow, AttemptStep};
    let mut s = world();
    lift_the_cap(&mut s, 120);
    let first = "<!-- gear-plan project=p base=0 -->\n# Milestone M2: course\ndone: ship the voyage\nbar: stable\neta: 8h\n# Queue\n## NEW [must] lane=heat milestone=M2\ngoal: A\naccept: a\nestimate: 60\ndeps: -\n";
    run(&mut s, first, "r1", NOW).unwrap();
    let more = "<!-- gear-plan project=p base=1 -->\n# Queue\n## NEW [must] lane=heat milestone=M2\ngoal: B\naccept: b\nestimate: 90\ndeps: -\n";
    run(&mut s, more, "r2", NOW + 60).unwrap();
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)?;
        for (n, state, start, finish) in [
            (1, AttemptState::Rejected, 0, 50),
            (2, AttemptState::Accepted, 60, 120),
        ] {
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(), task: "T-1".into(), n, seat: "p:heat".into(),
                generation: 1, contract_version: 1, adopted_version: 1, pending_update: None,
                state, candidate: Some("abc".into()), report: None, safe_point: None,
                started: NOW + start * 60, updated: NOW + finish * 60,
            })?;
            tx.set_attempt_time("p", "T-1", n, AttemptStep::Started, NOW + start * 60)?;
            tx.set_attempt_time("p", "T-1", n, AttemptStep::Settled, NOW + finish * 60)?;
        }
        Ok(())
    }).unwrap();
    s.write(|tx| {
        tx.insert_message(&gear_store::NewMessage {
            meta: None,
            project: "p".into(), thread: "question".into(), task: None, attempt: None,
            sender: "p:heat".into(), sender_gen: 1, recipient: "p:captain".into(),
            kind: gear_model::MsgKind::Question, urgent: false, body: "Review scope".into(),
            reply_to: None, needs_ack: false, state: gear_model::MsgState::Open, created: NOW,
        })?;
        Ok(())
    }).unwrap();
    let now = NOW + 180 * 60;
    let reports = s.read(|tx| gear_board::voyage::reports(tx, "p", now)).unwrap();
    let report = &reports[0];
    assert_eq!(report.elapsed_min, Some(180));
    assert_eq!(report.pace(), Some(2.0));
    assert_eq!(report.remaining_estimate_min, Some(90));
    assert_eq!(report.projected_min, Some(180 + 90 * 2));
    assert_eq!(report.projected_at, Some(NOW + 360 * 60));
    assert_eq!(report.scope_min, Some(150));
    assert_eq!(report.departure_scope_min, Some(60));
    assert_eq!(report.rework_rounds, 1);
    assert_eq!(report.slowest, Some(("T-1".into(), 120)));
    let lines = report.lines();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], "Voyage M2 course · done: ship the voyage");
    assert_eq!(lines[1], "  elapsed 180m / eta 8h");
    assert_eq!(lines[2], "  pace 2.00x · projected finish 360m from departure");
    assert_eq!(lines[3], "  scope now 150m / departure 60m");
    assert_eq!(lines[4], "  rework 1 round · slowest T-1 120m");
    let block = lines.join("\n");
    let (env, fp) = cap();
    // Every home build starts with it, including when there are no new messages.
    for _ in 0..2 {
        let page = home::build(&mut s, &env, Some(&fp), now).unwrap();
        assert!(page.text.starts_with(&block), "{}", page.text);
        assert!(page.text.find("SNAPSHOT").unwrap() > block.len());
        assert!(page.text.find("INBOX").unwrap() > block.len());
        home::record_view(&mut s, &page.view).unwrap();
    }
    let project = s.read(|tx| gear_board::project(tx, "p", now)).unwrap();
    assert_eq!(project.voyages, reports);
    let board = gear_board::Board {
        engine: gear_board::EngineState::Running,
        now,
        projects: vec![project],
    };
    for view in [gear_board::View::Lanes, gear_board::View::Deps] {
        let text = gear_board::render(&board, view);
        assert!(text.contains(&block), "{text}");
    }
    let json: serde_json::Value = serde_json::from_str(&gear_board::to_json(&board).unwrap()).unwrap();
    let json = &json["projects"][0]["voyages"][0];
    assert_eq!(json["pace"], 2.0);
    assert_eq!(json["projected_min"], 360);
    assert_eq!(json["scope_min"], 150);
    assert_eq!(json["departure_scope_min"], 60);

    // Editing today's estimate must not change the departure baseline.
    let edit = "<!-- gear-plan project=p base=2 -->\n# Queue\n## T-1 [must] lane=heat milestone=M2\nestimate: 120\n";
    run(&mut s, edit, "r3", now).unwrap();
    assert_eq!(s.read(|tx| tx.get_voyage("p", "M2")).unwrap().unwrap().departure_estimate_min, Some(60));
    // A running task may be the slowest, but cannot dilute completed-task pace.
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-2")?.unwrap();
        task.state = TaskState::Running;
        tx.update_task(&task)?;
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(), task: "T-2".into(), n: 1, seat: "p:heat".into(),
            generation: 1, contract_version: 1, adopted_version: 1, pending_update: None,
            state: AttemptState::Started, candidate: None, report: None, safe_point: None,
            started: NOW + 10 * 60, updated: now,
        })?;
        tx.set_attempt_time("p", "T-2", 1, AttemptStep::Started, NOW + 10 * 60)
    }).unwrap();
    let reports = s.read(|tx| gear_board::voyage::reports(tx, "p", now)).unwrap();
    assert_eq!(reports[0].slowest, Some(("T-2".into(), 170)));
    assert_eq!(reports[0].pace(), Some(1.0));
    assert_eq!(reports[0].projected_min, Some(270));
}

#[test]
fn voyage_report_keeps_unknowns_and_omits_finished_milestones() {
    let mut s = world();
    run(&mut s, &plan(0, FULL), "r1", NOW).unwrap();
    let reports = s.read(|tx| gear_board::voyage::reports(tx, "p", NOW + 60)).unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].pace(), None, "no completed measured task yet");
    assert_eq!(reports[0].projected_min, None);
    assert_eq!(reports[0].slowest, None, "unstarted tasks have no elapsed span");
    assert_eq!(reports[0].departure_scope_min, Some(60));
    s.write(|tx| {
        let mut a = tx.get_task("p", "T-1")?.unwrap();
        a.state = TaskState::Done;
        tx.update_task(&a)?;
        let mut b = tx.get_task("p", "T-2")?.unwrap();
        b.estimate_min = None;
        tx.update_task(&b)?;
        Ok(())
    }).unwrap();
    let reports = s.read(|tx| gear_board::voyage::reports(tx, "p", NOW + 60)).unwrap();
    assert_eq!(reports[0].pace(), None, "legacy completion has no measured time");
    assert_eq!(reports[0].scope_min, None, "unknown estimate is not zero");
    assert_eq!(reports[0].remaining_estimate_min, None);
    assert_eq!(reports[0].projected_min, None);
    s.write(|tx| {
        let mut b = tx.get_task("p", "T-2")?.unwrap();
        b.state = TaskState::Cancelled;
        tx.update_task(&b)
    }).unwrap();
    assert!(s.read(|tx| gear_board::voyage::reports(tx, "p", NOW + 60)).unwrap().is_empty());
}

/// A migrated voyage recovers its estimate from the departure plan, even when
/// the first new edit changes that task's estimate. Legacy goals count too.
#[test]
fn historical_departure_scope_uses_the_original_plan() {
    let mut old = world();
    lift_the_cap(&mut old, 300);
    run(&mut old, &plan(0, FULL), "r1", NOW).unwrap();
    let (tasks, contracts, mut voyage, mut history) = old.read(|tx| {
        let tasks = tx.tasks_of_project("p")?;
        let contracts = tasks.iter().map(|t| tx.get_contract("p", &t.id, 1)
            .map(|c| c.unwrap())).collect::<gear_model::GearResult<Vec<_>>>()?;
        Ok((tasks, contracts, tx.get_voyage("p", "M1")?.unwrap(), tx.get_plan("p", 1)?.unwrap()))
    }).unwrap();
    voyage.departure_estimate_min = None;
    // The old plan had no estimate field, but stated each estimate in its goal.
    history.draft = history.draft.replace("estimate: 30\n", "")
        .replace("goal: A", "goal: A. Estimate: 0.5h")
        .replace("goal: B", "goal: B. Estimate: 30m");
    let mut s = world();
    lift_the_cap(&mut s, 300);
    s.write(|tx| {
        for task in &tasks { tx.insert_task(task)?; }
        for contract in &contracts { tx.insert_contract(contract)?; }
        tx.upsert_voyage(&voyage)?;
        tx.insert_plan(&history)
    }).unwrap();
    let edit = "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-1 [must] lane=heat milestone=M1\nestimate: 300\n";
    run(&mut s, edit, "r2", NOW + 60).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(voyage.departure, Some(NOW));
    assert_eq!(voyage.departure_estimate_min, Some(60));
    let reports = s.read(|tx| gear_board::voyage::reports(tx, "p", NOW + 60)).unwrap();
    assert_eq!(reports[0].scope_min, Some(330));
    assert_eq!(reports[0].departure_scope_min, Some(60));
}

/// T-239: a milestone emptied of its tasks keeps no departure, so a reused id
/// starts its clock when its new tasks come — and its departure scope is those
/// new tasks' estimates, not the earlier voyage's plan.
#[test]
fn an_emptied_milestone_loses_its_departure_and_a_reused_id_starts_again() {
    let mut s = world();
    // M1 departs at NOW with two tasks of 30m each.
    run(&mut s, &plan(0, FULL), "r1", NOW).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (voyage.departure, voyage.departure_tasks, voyage.departure_estimate_min),
        (Some(NOW), Some(2), Some(60))
    );

    // The trial is moved to M0: M1 loses every task and with them its departure.
    let moved = "<!-- gear-plan project=p base=1 -->\n# Queue\n## T-1 [must] lane=heat milestone=M0\n## T-2 [should] lane=heat milestone=M0\n";
    run(&mut s, moved, "r2", NOW + 100).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (voyage.departure, voyage.departure_tasks, voyage.departure_estimate_min),
        (None, None, None),
        "a milestone with no task has no departure"
    );
    // M0, which now holds the tasks, departed on this apply.
    let voyage = s.read(|tx| tx.get_voyage("p", "M0")).unwrap().unwrap();
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW + 100), Some(2)));

    // The real voyage reuses M1: its departure is its own first task, not the
    // trial's, and its scope is the new tasks' estimates.
    let reuse = "<!-- gear-plan project=p base=2 -->\n# Milestone M1: delivery v0.2.0\ndone: users install v0.2.0\nbar: block only loop breaks\neta: 3h\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: C\naccept: c\nestimate: 45\ndeps: -\n";
    run(&mut s, reuse, "r3", NOW + 200).unwrap();
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(
        (voyage.departure, voyage.departure_tasks, voyage.departure_estimate_min),
        (Some(NOW + 200), Some(1), Some(45)),
        "the reused id starts when its new tasks come"
    );
    assert_eq!(voyage.title, "delivery v0.2.0");
}

/// T-301: a voyage id is any word without spaces, not only `M<n>`: a plan
/// with `# Milestone sprint-3: …` and a task `milestone=sprint-3` applies, the
/// voyage is stored under that id, the snapshot shows it and its report names
/// its task.
#[test]
fn a_voyage_id_need_not_start_with_m() {
    let mut s = world();
    let text = "<!-- gear-plan project=p base=0 -->\n# Milestone sprint-3: the third sprint\ndone: users install v0.1.0 without Rust\nbar: block only loop breaks\neta: 8h\n# Queue\n## NEW [must] lane=heat milestone=sprint-3\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
    run(&mut s, text, "r1", NOW).unwrap();

    let voyage = s.read(|tx| tx.get_voyage("p", "sprint-3")).unwrap().unwrap();
    assert_eq!((voyage.milestone.as_str(), voyage.title.as_str()), ("sprint-3", "the third sprint"));
    assert_eq!((voyage.departure, voyage.departure_tasks), (Some(NOW), Some(1)));
    assert_eq!(
        s.read(|tx| tx.tasks_of_project("p")).unwrap()[0].milestone.as_deref(),
        Some("sprint-3")
    );

    let (env, fp) = cap();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 10).unwrap();
    assert!(page.text.contains("Voyage sprint-3 the third sprint"), "{}", page.text);

    let report =
        s.read(|tx| gear_app::voyage::report(tx, "p", "sprint-3", false, NOW + 60)).unwrap();
    assert_eq!(report.milestone, "sprint-3");
    let named: Vec<&str> = report.time.tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(named, ["T-1"], "the report names the task on the voyage");
}

/// T-301: the snapshot lists the voyages by departure, not by id — `M10`
/// departed first comes before `M9`, and `M9` departed first comes before
/// `M10`, though the id alone would put `M10` first both times.
#[test]
fn the_snapshot_lists_the_voyages_by_departure_not_by_id() {
    let block = "<!-- gear-plan project=p base={base} -->\n# Milestone {id}: voyage {id}\ndone: it ships\nbar: stable\neta: 8h\n# Queue\n## NEW [must] lane=heat milestone={id}\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
    let (env, fp) = cap();
    for (first, second) in [("M10", "M9"), ("M9", "M10")] {
        let mut s = world();
        run(&mut s, &block.replace("{base}", "0").replace("{id}", first), "r1", NOW).unwrap();
        run(&mut s, &block.replace("{base}", "1").replace("{id}", second), "r2", NOW + 3600)
            .unwrap();
        let page = home::build(&mut s, &env, Some(&fp), NOW + 4000).unwrap();
        let snapshot = &page.text[page.text.find("SNAPSHOT").expect("the snapshot")..];
        let earlier = snapshot
            .find(&format!("Voyage {first} voyage {first}"))
            .unwrap_or_else(|| panic!("{first} is in the snapshot: {snapshot}"));
        let later = snapshot
            .find(&format!("Voyage {second} voyage {second}"))
            .unwrap_or_else(|| panic!("{second} is in the snapshot: {snapshot}"));
        assert!(earlier < later, "{first} departed first, {second} second: {snapshot}");
    }
}

/// M5 S3: a voyage's stations come from its block, and each task's station
/// from its queue header; both are stored, and `plan show` prints the task's.
#[test]
fn a_voyage_stores_its_stations_and_each_task_its_own() {
    let mut s = world();
    let text = "<!-- gear-plan project=p base=0 -->\n# Milestone M1: release\n\
                done: users install it\nbar: keep it simple\neta: 8h\n\
                station S1: reliability fixes and conventions as configuration, released as v0.5.0\n\
                station S2: the board matches the prototype\n\
                # Queue\n\
                ## NEW [must] lane=heat milestone=M1 station=S2\n\
                goal: A\naccept: a\nestimate: 30\ndeps: -\n\n\
                ## NEW [should] lane=heat milestone=M1\n\
                goal: B\naccept: b\nestimate: 30\ndeps: -\n";
    run(&mut s, text, "r1", NOW).unwrap();

    let stations = s.read(|tx| tx.voyage_stations("p", "M1")).unwrap();
    let named: Vec<(&str, &str)> = stations
        .iter()
        .map(|station| (station.id.as_str(), station.needs.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("S1", "reliability fixes and conventions as configuration, released as v0.5.0"),
            ("S2", "the board matches the prototype"),
        ]
    );
    assert_eq!(s.read(|tx| tx.task_station("p", "T-1")).unwrap().as_deref(), Some("S2"));
    assert_eq!(s.read(|tx| tx.task_station("p", "T-2")).unwrap(), None);

    // `gear plan show` prints the station in the header, after the milestone.
    let block = s.read(|tx| gear_app::plan::show(tx, "p", "T-1")).unwrap();
    assert!(
        block.starts_with("## T-1 [must] lane=heat milestone=M1 station=S2"),
        "{block}"
    );
}

/// M5 S3: a task may name only a station its milestone block has; a station
/// without a milestone is refused too, and a refusal writes nothing.
#[test]
fn a_task_naming_an_unknown_station_is_refused() {
    let mut s = world();
    let text = "<!-- gear-plan project=p base=0 -->\n# Milestone M1: release\n\
                done: users install it\nbar: keep it simple\neta: 8h\n\
                station S1: the first station\n\
                # Queue\n\
                ## NEW [must] lane=heat milestone=M1 station=S9\n\
                goal: A\naccept: a\nestimate: 30\ndeps: -\n";
    let err = run(&mut s, text, "r1", NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("T-1 names station S9, but milestone M1 has no `station S9:` line"),
        "{}",
        err.detail
    );
    assert!(s.read(|tx| tx.get_voyage("p", "M1")).unwrap().is_none());
    assert!(s.read(|tx| tx.tasks_of_project("p")).unwrap().is_empty());

    // The same station without a milestone on the task.
    let no_milestone = "<!-- gear-plan project=p base=0 -->\n# Queue\n\
                        ## NEW [must] lane=heat station=S2\n\
                        goal: A\naccept: a\nestimate: 30\ndeps: -\n";
    let err = run(&mut s, no_milestone, "r2", NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("T-1 names station S2 but has no milestone"),
        "{}",
        err.detail
    );
    assert!(s.read(|tx| tx.tasks_of_project("p")).unwrap().is_empty());
}

/// M5 S3: a block always carries every field, so re-applying the task without
/// `station=` clears the station it had.
#[test]
fn a_block_without_station_clears_the_tasks_station() {
    let mut s = world();
    let first = "<!-- gear-plan project=p base=0 -->\n# Milestone M1: release\n\
                 done: users install it\nbar: keep it simple\neta: 8h\n\
                 station S1: the first station\n\
                 station S2: the second station\n\
                 # Queue\n\
                 ## NEW [must] lane=heat milestone=M1 station=S2\n\
                 goal: A\naccept: a\nestimate: 30\ndeps: -\n";
    run(&mut s, first, "r1", NOW).unwrap();
    assert_eq!(s.read(|tx| tx.task_station("p", "T-1")).unwrap().as_deref(), Some("S2"));

    let again = "<!-- gear-plan project=p base=1 -->\n# Queue\n\
                 ## T-1 [must] lane=heat milestone=M1\n";
    run(&mut s, again, "r2", NOW + 10).unwrap();
    assert_eq!(s.read(|tx| tx.task_station("p", "T-1")).unwrap(), None);
    let block = s.read(|tx| gear_app::plan::show(tx, "p", "T-1")).unwrap();
    assert!(!block.contains("station="), "{block}");
}
