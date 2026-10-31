//! T-461: a captain cannot lower the bar on its own. The goal, done, bar and
//! eta of a departed voyage, a new milestone's departure and a move between
//! milestones need the user's word — `--confirmed` and a `confirmed:` line in
//! the same `# Decisions`; a cancel changes the state only; the goal check is
//! written per done clause; a landing over an open clause or a cut of more
//! than a fifth is refused unless the user's line names it; the report lists
//! all of them.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::{apply, ApplyResult};
use gear_app::{station, voyage, Env};
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

/// Apply a plan, with or without the user's confirmation.
fn run(
    s: &mut Store,
    text: &str,
    rid: &str,
    now: i64,
    confirmed: bool,
) -> gear_model::GearResult<ApplyResult> {
    let mut draft = parse(text).unwrap();
    draft.confirmed = confirmed;
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &draft, text, rid, now)
}

/// The first voyage M1: two clauses and two tasks, confirmed at departure.
fn depart(s: &mut Store) {
    let text = "<!-- gear-plan project=p base=0 -->\n\
                # Milestone M1: delivery\n\
                done: (1) users install it; (2) the board shows it\n\
                bar: block only loop breaks\neta: 8h\ndepart: now\n\
                # Decisions\n\
                - 2026-10-27 10:05 confirmed: milestone M1 departs | why: the user said depart\n\
                # Queue\n\
                ## NEW [must] lane=heat milestone=M1\n\
                goal: A\naccept: a\nestimate: 30\ndeps: -\n\n\
                ## NEW [should] lane=heat milestone=M1\n\
                goal: B\naccept: b\nestimate: 30\ndeps: -\n";
    run(s, text, "r1", NOW, true).unwrap();
}

fn plan_with(base: i64, body: &str) -> String {
    format!("<!-- gear-plan project=p base={base} -->\n{body}")
}

#[test]
fn a_goal_change_needs_the_flag_and_its_confirmed_line() {
    let mut s = world();
    depart(&mut s);
    let block = "# Milestone M1: delivery\n\
                 done: (1) users install v2; (2) the board shows v2\n\
                 bar: block only loop breaks\neta: 8h\n";
    // Without the flag: refused.
    let err = run(&mut s, &plan_with(1, block), "r2-bare", NOW + 10, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail
            .contains("confirm it with them, then apply again with --confirmed"),
        "{}",
        err.detail
    );
    // The flag alone is not the record: refused without the line.
    let err = run(&mut s, &plan_with(1, block), "r2-flag", NOW + 20, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("no `confirmed:` line"),
        "{}",
        err.detail
    );
    assert_eq!(
        s.read(|tx| tx.plan_version("p")).unwrap(),
        1,
        "nothing written"
    );
    // With both: applied, and the line is recorded.
    let asked = format!(
        "{block}# Decisions\n- 2026-10-27 10:20 confirmed: done of M1 changed from \"(1) users install it; (2) the board shows it\" to \"(1) users install v2; (2) the board shows v2\" | why: the user asked for v2\n"
    );
    run(&mut s, &plan_with(1, &asked), "r2", NOW + 30, true).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_voyage("p", "M1"))
            .unwrap()
            .unwrap()
            .done
            .as_deref(),
        Some("(1) users install v2; (2) the board shows v2")
    );
    let confirmed: Vec<_> = s
        .read(|tx| tx.decisions_of("p", "M1"))
        .unwrap()
        .into_iter()
        .filter(|d| d.kind == "confirmed")
        .collect();
    assert_eq!(
        confirmed.len(),
        2,
        "the departure and the goal change: {confirmed:?}"
    );
    assert!(
        confirmed[1].what.contains("done of M1 changed from"),
        "{confirmed:?}"
    );
}

#[test]
fn a_new_milestones_departure_needs_the_flag_and_its_confirmed_line() {
    let mut s = world();
    depart(&mut s);
    let block = "# Milestone M2: the next one\n\
                 done: users install v2\nbar: it holds\neta: 4h\ndepart: now\n\
                 # Queue\n\
                 ## NEW [must] lane=heat milestone=M2\n\
                 goal: C\naccept: c\nestimate: 30\ndeps: -\n";
    let err = run(&mut s, &plan_with(1, block), "r2-bare", NOW + 10, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("milestone M2 departs in this apply"),
        "{}",
        err.detail
    );
    let err = run(&mut s, &plan_with(1, block), "r2-flag", NOW + 20, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("no `confirmed:` line"),
        "{}",
        err.detail
    );
    assert_eq!(
        s.read(|tx| tx.plan_version("p")).unwrap(),
        1,
        "nothing written"
    );
    assert!(s.read(|tx| tx.get_voyage("p", "M2")).unwrap().is_none());

    let asked = format!(
        "{block}# Decisions\n- 2026-10-27 10:30 confirmed: milestone M2 departs with 1 task | why: the user asked for the next voyage\n"
    );
    run(&mut s, &plan_with(1, &asked), "r2", NOW + 30, true).unwrap();
    let m2 = s.read(|tx| tx.get_voyage("p", "M2")).unwrap().unwrap();
    assert_eq!(m2.departure, Some(NOW + 30));
    assert_eq!(m2.departure_tasks, Some(1));
}

#[test]
fn a_move_between_milestones_needs_the_flag_and_its_confirmed_line() {
    let mut s = world();
    depart(&mut s);
    let second = "# Milestone M2: the next one\n\
                  done: users install v2\nbar: it holds\neta: 4h\ndepart: now\n\
                  # Decisions\n\
                  - 2026-10-27 10:30 confirmed: milestone M2 departs | why: the user asked for the next voyage\n\
                  # Queue\n\
                  ## NEW [must] lane=heat milestone=M2\n\
                  goal: C\naccept: c\nestimate: 30\ndeps: -\n";
    run(&mut s, &plan_with(1, second), "r2", NOW + 30, true).unwrap();
    let moved = "# Queue\n## T-1 [must] lane=heat milestone=M2\ngoal: A\naccept: a\ndeps: -\n";
    let err = run(&mut s, &plan_with(2, moved), "r3-bare", NOW + 40, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("T-1 moves from M1 to M2"),
        "{}",
        err.detail
    );
    let err = run(&mut s, &plan_with(2, moved), "r3-flag", NOW + 50, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("no `confirmed:` line"),
        "{}",
        err.detail
    );
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .milestone
            .as_deref(),
        Some("M1"),
        "nothing moved"
    );
    let asked = format!(
        "{moved}# Decisions\n- 2026-10-27 10:40 confirmed: T-1 moves from M1 to M2 | why: the user asked for it\n"
    );
    run(&mut s, &plan_with(2, &asked), "r3", NOW + 60, true).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .milestone
            .as_deref(),
        Some("M2")
    );
    let report = s
        .read(|tx| voyage::report(tx, "p", "M1", false, NOW + 60))
        .unwrap();
    assert!(
        report
            .cuts
            .iter()
            .any(|cut| cut.kind == "move" && cut.what == "T-1 moved to M2"),
        "{:?}",
        report.cuts
    );
}

#[test]
fn a_cancel_line_changes_only_the_state() {
    let mut s = world();
    depart(&mut s);
    // The milestone is described but the cancel block moves the task.
    let block = "# Milestone M2: the next one\ndone: it ships\nbar: it holds\neta: 4h\n";
    let moved = format!(
        "{block}# Queue\n## T-1 [must] lane=heat milestone=M2\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n"
    );
    let err = run(&mut s, &plan_with(1, &moved), "r2", NOW + 10, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("a cancel line changes the state only"),
        "{}",
        err.detail
    );
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Ready
    );
    // A lane change is refused too.
    let lane = "# Queue\n## T-1 [must] lane=keel milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n";
    let err = run(&mut s, &plan_with(1, lane), "r3", NOW + 20, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail
            .contains("lane (stored Some(\"heat\"), got Some(\"keel\"))"),
        "{}",
        err.detail
    );
    // The same fields: the cancel applies.
    let same = "# Queue\n## T-1 [must] lane=heat milestone=M1\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n";
    run(&mut s, &plan_with(1, same), "r4", NOW + 30, false).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Cancelled
    );
}

#[test]
fn the_goal_check_reaches_the_meta_the_board_reads() {
    let mut s = world();
    depart(&mut s);
    let text = plan_with(
        1,
        "# Decisions\n\
         - done (1): met | evidence: the install log in ops/reports/T-1.md\n\
         - done (2): open | the board is not drawn yet\n",
    );
    run(&mut s, &text, "r2", NOW + 10, false).unwrap();
    let raw = s
        .read(|tx| tx.get_meta("goal-check:p:M1"))
        .unwrap()
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 2, "{json}");
    assert_eq!(items[0]["met"], true);
    assert_eq!(items[0]["text"], "(1) users install it");
    assert_eq!(
        items[0]["evidence"],
        "the install log in ops/reports/T-1.md"
    );
    assert_eq!(items[1]["met"], false);
    assert_eq!(items[1]["text"], "(2) the board shows it");
    assert_eq!(items[1]["why"], "the board is not drawn yet");
    // The board reads the check as written.
    let project = s.read(|tx| gear_board::project(tx, "p", NOW + 10)).unwrap();
    assert_eq!(
        project.goal_check,
        vec![
            (true, "(1) users install it".to_string()),
            (false, "(2) the board shows it".to_string())
        ]
    );
    // The report lists each clause with its evidence.
    let report = s
        .read(|tx| voyage::report(tx, "p", "M1", false, NOW + 10))
        .unwrap();
    assert_eq!(report.goal_check.len(), 2);
    assert_eq!(
        report.goal_check[0].evidence.as_deref(),
        Some("the install log in ops/reports/T-1.md")
    );
    let rendered = voyage::render(&report);
    assert!(
        rendered.contains(
            "(1) (1) users install it · met | evidence: the install log in ops/reports/T-1.md"
        ),
        "{rendered}"
    );
    // One line per clause: a check with a clause missing is refused.
    let short = plan_with(
        2,
        "# Decisions\n- done (1): met | evidence: the install log\n",
    );
    let err = run(&mut s, &short, "r3", NOW + 20, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("one line per done clause"),
        "{}",
        err.detail
    );
}

/// The 10-task voyage: one clause, ten tasks, all created at departure.
fn ten_tasks(s: &mut Store) {
    let mut text = String::from(
        "<!-- gear-plan project=p base=0 -->\n\
         # Milestone M1: delivery\ndone: it ships\nbar: stable\neta: 8h\ndepart: now\n\
         # Decisions\n\
         - 2026-10-27 10:05 confirmed: milestone M1 departs | why: the user said depart\n\
         # Queue\n",
    );
    for n in 1..=10 {
        text.push_str(&format!(
            "## NEW [must] lane=heat milestone=M1\ngoal: task {n}\naccept: works\nestimate: 10\ndeps: -\n"
        ));
    }
    run(s, &text, "r1", NOW, true).unwrap();
}

fn done_tasks(s: &mut Store, ids: &[&str]) {
    s.write(|tx| {
        for id in ids {
            let mut task = tx.get_task("p", id)?.unwrap();
            task.state = TaskState::Done;
            tx.update_task(&task)?;
        }
        Ok(())
    })
    .unwrap();
}

/// Watch the voyage while it still has open tasks, so the final checkpoint
/// rings the landing item (station::checkpoints skips a voyage never watched).
fn watch(s: &mut Store) {
    s.write(|tx| station::checkpoints(tx, "p", NOW + 60))
        .unwrap();
}

fn land_item(s: &mut Store) -> String {
    let ids = s
        .write(|tx| station::checkpoints(tx, "p", NOW + 600))
        .unwrap();
    ids.into_iter()
        .find(|id| {
            s.read(|tx| tx.get_message("p", id))
                .unwrap()
                .unwrap()
                .body
                .contains("ready to land")
        })
        .expect("a landing item")
}

#[test]
fn a_landing_over_an_open_clause_is_refused_until_the_user_names_it() {
    let mut s = world();
    depart(&mut s);
    watch(&mut s);
    done_tasks(&mut s, &["T-1", "T-2"]);
    let land = land_item(&mut s);
    let land_line = format!("# Decisions\n- station {land}: land\n");
    // No goal check: every clause is open (the board's rule).
    let err = run(&mut s, &plan_with(1, &land_line), "r2", NOW + 700, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("open clauses (1), (2)"),
        "{}",
        err.detail
    );
    // The flag alone is not enough: the line must name the open clause.
    let err = run(&mut s, &plan_with(1, &land_line), "r3", NOW + 710, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail
            .contains("no `confirmed:` line names the open clauses"),
        "{}",
        err.detail
    );
    // The user's line names clause (1): the landing applies.
    let asked = format!(
        "# Decisions\n- 2026-10-27 11:00 confirmed: clauses (1), (2) stay open, the user accepted them | why: the user accepted the open clauses\n- station {land}: land\n"
    );
    run(&mut s, &plan_with(1, &asked), "r4", NOW + 720, true).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_message("p", &land))
            .unwrap()
            .unwrap()
            .state,
        gear_model::MsgState::Resolved
    );
}

#[test]
fn a_landing_after_a_cut_over_a_fifth_is_refused_until_the_user_names_the_cut() {
    let mut s = world();
    ten_tasks(&mut s);
    watch(&mut s);
    // Three of ten tasks are cancelled (T-461: a cancel repeats every field).
    let mut cancel = String::from("# Queue\n");
    for n in 1..=3 {
        cancel.push_str(&format!(
            "## T-{n} [must] lane=heat milestone=M1\ngoal: task {n}\naccept: works\nestimate: 10\ndeps: -\nstate: cancelled\n"
        ));
    }
    run(&mut s, &plan_with(1, &cancel), "r2", NOW + 60, false).unwrap();
    done_tasks(&mut s, &["T-4", "T-5", "T-6", "T-7", "T-8", "T-9", "T-10"]);
    let land = land_item(&mut s);
    let land_line = format!("# Decisions\n- station {land}: land\n");
    let err = run(&mut s, &plan_with(2, &land_line), "r3", NOW + 700, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("3 of 10 departure tasks"),
        "{}",
        err.detail
    );
    // A confirmed line that does not name the cut is refused.
    let unnamed = format!(
        "# Decisions\n- 2026-10-27 11:00 confirmed: the landing is fine | why: the user asked\n- station {land}: land\n"
    );
    let err = run(&mut s, &plan_with(2, &unnamed), "r4", NOW + 710, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail
            .contains("no `confirmed:` line names the open clauses or the cut"),
        "{}",
        err.detail
    );
    // The goal check closes clause (1); the line names the cut.
    let asked = format!(
        "# Decisions\n\
         - done (1): met | evidence: the seven remaining tasks merged\n\
         - 2026-10-27 11:10 confirmed: the cut of 3 tasks is the user's | why: the user accepted the cut\n\
         - station {land}: land\n"
    );
    run(&mut s, &plan_with(2, &asked), "r5", NOW + 720, true).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_message("p", &land))
            .unwrap()
            .unwrap()
            .state,
        gear_model::MsgState::Resolved
    );
}
