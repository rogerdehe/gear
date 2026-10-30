//! T-350: the board can be built for any voyage of the project, and shows
//! that voyage only. A landed voyage keeps its data: its lanes read "retired
//! at the landing", and its end is the last merge of its tasks.

use gear_board::{Options, ProjectBoard};
use gear_model::{AttemptState, SeatRole, SeatState, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, NewDecision, Store, TaskRow, VoyageRow,
};

const NOW: i64 = 1_800_000_000;

/// M1's task was merged here: the voyage's end.
const MERGED: i64 = NOW - 6000;

/// A project with two voyages: M1 landed with a retired lane `old` that
/// merged T-1, and M2 sailing on the lane `hull`.
fn world() -> Store {
    world_with_m2(true)
}

/// The same project with only the landed M1: no voyage sails, so the board
/// shows the landed one.
fn world_landed() -> Store {
    world_with_m2(false)
}

fn world_with_m2(sailing: bool) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            let old = testing::seed_active_seat(tx, "p", "old", SeatRole::Lane, "w1:p3", "pi", NOW)?;
            tx.set_seat_state(&old.id, SeatState::Retired, None, NOW)?;
            voyage(tx, "M1", NOW - 7200)?;
            // M1: one task it merged, and one it cut.
            task(tx, "T-1", 1, "M1", "old", TaskState::Done)?;
            task(tx, "T-4", 4, "M1", "old", TaskState::Cancelled)?;
            decision(tx, "d1", "M1", "the old lane lands with M1")?;
            if sailing {
                voyage(tx, "M2", NOW - 3600)?;
                // M2: one done, one ready.
                task(tx, "T-2", 2, "M2", "hull", TaskState::Done)?;
                task(tx, "T-3", 3, "M2", "hull", TaskState::Ready)?;
                decision(tx, "d2", "M2", "the hull lane keeps the work")?;
            }
            Ok(())
        })
        .unwrap();
    store
        .write(|tx| {
            attempt(tx, "T-1", "p:old", NOW - 6600, MERGED)?;
            if sailing {
                attempt(tx, "T-2", "p:hull", NOW - 3000, NOW - 2400)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

fn voyage(tx: &gear_store::Tx<'_>, milestone: &str, departure: i64) -> gear_model::GearResult<()> {
    tx.upsert_voyage(&VoyageRow {
        project: "p".into(),
        milestone: milestone.into(),
        title: format!("voyage {milestone}"),
        done: Some("it is delivered".into()),
        bar: None,
        eta: Some("8h".into()),
        departure: Some(departure),
        departure_tasks: Some(1),
        departure_estimate_min: Some(30),
        updated: departure,
    })
}

fn task(
    tx: &gear_store::Tx<'_>,
    id: &str,
    rank: i64,
    milestone: &str,
    lane: &str,
    state: TaskState,
) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: Some(milestone.into()),
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
}

fn decision(
    tx: &gear_store::Tx<'_>,
    key: &str,
    voyage: &str,
    what: &str,
) -> gear_model::GearResult<()> {
    tx.insert_decision_if_new(&NewDecision {
        project: "p".into(),
        key: key.into(),
        voyage: voyage.into(),
        at: "2026-10-27 11:40".into(),
        kind: "scope".into(),
        what: what.into(),
        why: None,
        alt: None,
        changes: None,
        star: false,
        source: "plan".into(),
        line: None,
        plan_version: None,
    })?;
    Ok(())
}

/// A finished task: it started, was handed in and was merged at `merged`.
fn attempt(
    tx: &gear_store::Tx<'_>,
    task: &str,
    seat: &str,
    started: i64,
    merged: i64,
) -> gear_model::GearResult<()> {
    tx.insert_attempt(&AttemptRow {
        project: "p".into(),
        task: task.into(),
        n: 1,
        seat: seat.into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state: AttemptState::Accepted,
        candidate: Some("abc".into()),
        report: None,
        safe_point: None,
        started,
        updated: started,
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
    })?;
    tx.set_attempt_time("p", task, 1, AttemptStep::Started, started)?;
    tx.set_attempt_time("p", task, 1, AttemptStep::HandedIn, merged)?;
    tx.set_attempt_time("p", task, 1, AttemptStep::Merged, merged)?;
    Ok(())
}

/// The line the engine writes when a task leaves a voyage (T-351).
fn moved(
    tx: &gear_store::Tx<'_>,
    task: &str,
    old: &str,
    new: &str,
) -> gear_model::GearResult<()> {
    tx.insert_decision_if_new(&NewDecision {
        project: "p".into(),
        key: format!("move|{task}|{old}|{new}"),
        voyage: old.into(),
        at: "2026-10-27 11:40".into(),
        kind: "move".into(),
        what: format!("{task} moved to {new}"),
        why: None,
        alt: None,
        changes: None,
        star: false,
        source: "gear".into(),
        line: None,
        plan_version: None,
    })?;
    Ok(())
}

fn board(store: &mut Store, milestone: Option<&str>) -> ProjectBoard {
    let options = Options {
        project: Some("p".into()),
        milestone: milestone.map(str::to_string),
        ..Default::default()
    };
    gear_board::build(store, &options, NOW)
        .unwrap()
        .projects
        .into_iter()
        .next()
        .expect("the project")
}

#[test]
fn the_board_built_without_a_milestone_shows_the_sailing_voyage_only() {
    let mut store = world();
    let plan = board(&mut store, None);

    // The two M2 tasks: no task of the landed M1, and not the one it cut.
    assert_eq!(plan.totals.tasks, 2, "the tasks of M2: {:?}", plan.totals);
    let listed: Vec<&str> = plan
        .lanes
        .iter()
        .flat_map(|lane| lane.tasks.iter())
        .filter(|task| task.actual_min.unwrap_or(0) > 0)
        .map(|task| task.id.as_str())
        .collect();
    assert!(!listed.contains(&"T-1"), "no task of the landed voyage: {listed:?}");
    let graphed: Vec<&str> = plan.graph.iter().map(|task| task.id.as_str()).collect();
    assert!(!graphed.contains(&"T-1"), "no task of the landed voyage: {graphed:?}");
    // The lanes are the sailing voyage's, the retired one included only when
    // it is a lane of the shown voyage.
    let names: Vec<&str> = plan.lanes.iter().map(|lane| lane.lane.as_str()).collect();
    assert_eq!(names, vec!["hull"], "the lanes of M2: {names:?}");

    // Every voyage of the project, the newest departure first.
    let list: Vec<(&str, bool)> =
        plan.voyage_list.iter().map(|e| (e.milestone.as_str(), e.sailing)).collect();
    assert_eq!(list, vec![("M2", true), ("M1", false)], "the voyage list: {list:?}");
    let m1 = &plan.voyage_list[1];
    assert_eq!(m1.departure, Some(NOW - 7200));
    assert_eq!(m1.ended, Some(MERGED), "M1 ended at its last merge");
    assert_eq!(plan.voyage_list[0].ended, None, "a sailing voyage has not ended");

    // The one the board shows is the first of the list.
    let shown = plan.shown.expect("a measured voyage");
    assert_eq!(shown.milestone, "M2");
    assert!(shown.sailing);
}

#[test]
fn the_board_built_for_a_landed_voyage_keeps_its_data() {
    let mut store = world();
    let plan = board(&mut store, Some("M1"));

    // Its tasks only, and the one it cut is not counted (T-415).
    assert_eq!(plan.totals.tasks, 1, "the tasks of M1: {:?}", plan.totals);
    assert_eq!(plan.totals.done, 1);

    // Its lanes, retired ones included, retired at the landing.
    let names: Vec<&str> = plan.lanes.iter().map(|lane| lane.lane.as_str()).collect();
    assert_eq!(names, vec!["old"], "the lanes of M1: {names:?}");
    let lane = &plan.lanes[0];
    assert_eq!(lane.idle_text.as_deref(), Some("retired at the landing"));
    let ids: Vec<&str> = lane.tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["T-1", "T-4"],
        "the lane works M1 only, and the task it cut stays in its queue (T-415)"
    );

    // The graph and the time view follow the shown tasks.
    let graphed: Vec<&str> = plan.graph.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(graphed, vec!["T-1"], "the graph of M1: {graphed:?}");

    // The voyage it shows ended at T-1's merge.
    let shown = plan.shown.expect("the measured voyage");
    assert_eq!(shown.milestone, "M1");
    assert!(!shown.sailing);
    assert_eq!(shown.ended, Some(MERGED));
    assert_eq!(plan.voyage_list.first().map(|e| e.milestone.as_str()), Some("M2"));

    // The decisions are M1's only.
    let whats: Vec<&str> = plan.decisions.iter().map(|d| d.what.as_str()).collect();
    assert_eq!(whats, vec!["the old lane lands with M1"], "the decisions of M1: {whats:?}");
}

/// T-353: the first row lists every voyage, the landed ones with ⚓ and the
/// sailing ones with ⛵, and the shown voyage's label is lit (README change 1).
#[test]
fn the_head_lists_every_voyage_and_lights_the_shown_one() {
    use gear_board::frame::{self, Tab};
    use gear_board::style::{Color, Style};

    let mut store = world();
    let board = gear_board::build(
        &mut store,
        &Options { project: Some("p".into()), milestone: None, ..Default::default() },
        NOW,
    )
    .unwrap();
    let size = (160, 40);
    let screen = crate::screen(Tab::Now, 0, size);
    let canvas = frame::canvas(&board, &screen, None);
    let text = canvas.text();
    let head = frame::layout(&board, &screen);

    // The two voyages, newest first: M2 sails, M1 landed.
    let labels: Vec<&str> = head.voyages.iter().map(|(_, _, milestone)| milestone.as_str()).collect();
    assert_eq!(labels, vec!["M2", "M1"], "the labels of the first row: {labels:?}");
    let row = text.lines().next().unwrap_or_default();
    let m2 = row.find("M2").unwrap_or_else(|| panic!("M2's label: {row:?}"));
    let m1 = row.find("M1").unwrap_or_else(|| panic!("M1's label: {row:?}"));
    assert!(m2 < m1, "the newest voyage is the first label: {row:?}");
    assert!(row[m2..].starts_with("M2 ⛵"), "a sailing voyage carries ⛵: {row:?}");
    assert!(row[m1..].starts_with("M1 ⚓"), "a landed voyage carries ⚓: {row:?}");

    // The shown voyage's label is in reverse video and bold; the landed one
    // is dim. Each range holds its label's cells, the mark and its
    // continuation included.
    for (first, last, milestone) in &head.voyages {
        let shown = milestone == "M2";
        let style = canvas.style_at(*first, 0);
        assert_eq!(style.reverse, shown, "the label of {milestone} on row 0: {text}");
        assert_eq!(style.bold, shown, "the label of {milestone} on row 0: {text}");
        assert_eq!(style.fg, if shown { None } else { Some(Color::Dim) }, "{text}");
        let cells: String = (*first..=*last).map(|column| canvas.char_at(column, 0)).collect();
        let mark = if shown { '⛵' } else { '⚓' };
        assert!(cells.contains(mark), "the cells of {milestone} hold {mark}: {cells:?}");
    }
    // The label of the landed voyage is dim, as the prototype draws it.
    let (first, _, _) = head.voyages[1];
    assert_eq!(
        canvas.style_at(first, 0),
        Style { fg: Some(Color::Dim), bold: false, reverse: false }
    );
}

/// T-351: a task that left the voyage is named by the voyage it left, and
/// counted nowhere: it carries the milestone of the voyage it moved to.
#[test]
fn a_task_that_left_the_voyage_is_named_but_not_counted() {
    let mut store = world();
    store
        .write(|tx| {
            // T-1 was M1's; it left for M2 and carries M2's milestone now.
            let mut task = tx.get_task("p", "T-1")?.expect("the task");
            task.milestone = Some("M2".into());
            tx.update_task(&task)?;
            moved(tx, "T-1", "M1", "M2")
        })
        .unwrap();

    let m1 = board(&mut store, Some("M1"));
    assert_eq!(m1.moved_out, vec![("T-1".to_string(), "M2".to_string())]);
    // Its lanes' tasks — the rows the Cost view reads (T-363) — its graph and
    // its totals leave it out.
    let listed: Vec<&str> = m1
        .lanes
        .iter()
        .flat_map(|lane| lane.tasks.iter())
        .map(|task| task.id.as_str())
        .collect();
    assert!(!listed.contains(&"T-1"), "a moved task is M2's now: {listed:?}");
    let graphed: Vec<&str> = m1.graph.iter().map(|task| task.id.as_str()).collect();
    assert!(!graphed.contains(&"T-1"), "a moved task is M2's now: {graphed:?}");
    let lane = m1.lanes.iter().find(|lane| lane.lane == "old").expect("the lane");
    let ids: Vec<&str> = lane.tasks.iter().map(|task| task.id.as_str()).collect();
    assert!(!ids.contains(&"T-1"), "a moved task is M2's now: {ids:?}");
    assert_eq!(m1.totals.tasks, 0, "M1 has no task left: {:?}", m1.totals);

    // The voyage it joined records no move, so it names nothing as moved out.
    let m2 = board(&mut store, Some("M2"));
    assert!(m2.moved_out.is_empty(), "{:?}", m2.moved_out);
}

/// The text of the graph row a task's branch leaves `dev` on: the fork row's
/// `starts +<h>h<mm>m`, or `None` for a row that is not one.
fn starts_of(plan: &ProjectBoard) -> Vec<String> {
    gear_board::frame::task_rows(plan, NOW, 160)
        .into_iter()
        .map(|row| row.text)
        .filter(|text| text.contains("starts +"))
        .collect()
}

/// T-362 (F1 of the review of attempt 1): the Tasks view measures its rows from
/// the shown voyage's own departure, so a landed voyage picked with `[` reads
/// its tasks' starts there — M1 departed at NOW − 7200 and its task started at
/// NOW − 6600, so its branch leaves dev at +0h10m, not 50 minutes before it.
#[test]
fn the_tasks_view_measures_a_landed_voyage_from_its_own_departure() {
    let mut store = world();
    let plan = board(&mut store, Some("M1"));
    assert_eq!(
        plan.shown.as_ref().and_then(|voyage| voyage.departure),
        Some(NOW - 7200),
        "the shown voyage is M1"
    );
    assert_eq!(starts_of(&plan), vec!["       starts +0h10m"], "M1's rows");
}

/// The same for a project whose only voyage has landed: the board shows it, and
/// the Tasks view still measures from its departure, not from `now`.
#[test]
fn a_landed_only_project_measures_the_tasks_view_from_its_departure() {
    let mut store = world_landed();
    let plan = board(&mut store, None);
    assert_eq!(plan.shown.as_ref().map(|voyage| voyage.milestone.as_str()), Some("M1"));
    assert!(
        plan.shown.as_ref().is_some_and(|voyage| !voyage.sailing),
        "the landed voyage is the one shown"
    );
    assert_eq!(starts_of(&plan), vec!["       starts +0h10m"], "M1's rows");
}

/// A milestone the plan never described (T-441): its tasks fixed a departure,
/// but the row has no title and no done sentence.
fn planless(
    tx: &gear_store::Tx<'_>,
    milestone: &str,
    departure: i64,
) -> gear_model::GearResult<()> {
    tx.upsert_voyage(&VoyageRow {
        project: "p".into(),
        milestone: milestone.into(),
        title: String::new(),
        done: None,
        bar: None,
        eta: None,
        departure: Some(departure),
        departure_tasks: Some(1),
        departure_estimate_min: Some(30),
        updated: departure,
    })
}

/// T-441: with no voyage sailing, a milestone the plan never described must not
/// hide the latest voyage it did describe, even when it departed later.
#[test]
fn a_plan_less_milestone_that_departed_later_does_not_hide_the_landed_voyage() {
    let mut store = world_landed();
    store
        .write(|tx| {
            // Departed after M1, with one task it cut and no done sentence.
            planless(tx, "M1.1-cancelled", NOW - 1800)?;
            task(tx, "T-5", 5, "M1.1-cancelled", "old", TaskState::Cancelled)
        })
        .unwrap();

    let plan = board(&mut store, None);
    let voyage = plan.workload.as_ref().expect("the measured voyage");
    assert_eq!(voyage.milestone, "M1");
    assert_eq!(voyage.title, "voyage M1", "the described voyage's title");
    assert_eq!(voyage.done.as_deref(), Some("it is delivered"), "its done sentence");
    // The board shows M1's tasks only, not the one M1.1-cancelled cut.
    let shown = plan.shown.expect("the shown voyage");
    assert_eq!(shown.milestone, "M1");
    assert_eq!(shown.title, "voyage M1");
    assert!(!shown.sailing);
    assert_eq!(plan.totals.tasks, 1, "the tasks of M1: {:?}", plan.totals);
    assert_eq!(plan.totals.done, 1);

    // The voyage list shows the voyages the plan described only (T-445): the
    // plan-less milestone is not listed, though naming it still shows it.
    let list: Vec<&str> = plan.voyage_list.iter().map(|e| e.milestone.as_str()).collect();
    assert_eq!(list, vec!["M1"], "the voyage list: {list:?}");

    // A voyage named with a milestone is unchanged: M1.1-cancelled is shown.
    let named = board(&mut store, Some("M1.1-cancelled"));
    assert_eq!(
        named.shown.as_ref().map(|voyage| voyage.milestone.as_str()),
        Some("M1.1-cancelled")
    );
}

/// T-441: a project whose only milestone the plan never described still shows
/// it as today's voyage.
#[test]
fn a_project_with_only_a_plan_less_milestone_still_shows_it() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "old", SeatRole::Lane, "w1:p1", "pi", NOW)?;
            planless(tx, "M1.1-cancelled", NOW - 1800)?;
            task(tx, "T-1", 1, "M1.1-cancelled", "old", TaskState::Cancelled)
        })
        .unwrap();

    let plan = board(&mut store, None);
    let voyage = plan.workload.as_ref().expect("the measured voyage");
    assert_eq!(voyage.milestone, "M1.1-cancelled");
    assert_eq!(voyage.done, None, "no done sentence to show");
    let shown = plan.shown.expect("the shown voyage");
    assert_eq!(shown.milestone, "M1.1-cancelled");
    assert!(!shown.sailing);
}

/// T-445: a milestone the plan never described — departed, with a task it cut
/// and no done sentence — is not a voyage of the first row; a described voyage
/// that departed later is listed first.
#[test]
fn the_voyage_list_shows_only_the_voyages_a_plan_described() {
    let mut store = world_landed();
    store
        .write(|tx| {
            planless(tx, "M1.1-cancelled", NOW - 1800)?;
            task(tx, "T-5", 5, "M1.1-cancelled", "old", TaskState::Cancelled)
        })
        .unwrap();

    let plan = board(&mut store, None);
    let list: Vec<&str> = plan.voyage_list.iter().map(|e| e.milestone.as_str()).collect();
    assert_eq!(list, vec!["M1"], "the voyage list: {list:?}");
    // The shown voyage is still chosen as T-441 made it: M1.
    assert_eq!(plan.shown.as_ref().map(|voyage| voyage.milestone.as_str()), Some("M1"));

    // A described voyage that departed later is listed first, M1 after it.
    store.write(|tx| voyage(tx, "M2", NOW - 600)).unwrap();
    let plan = board(&mut store, None);
    let list: Vec<&str> = plan.voyage_list.iter().map(|e| e.milestone.as_str()).collect();
    assert_eq!(list, vec!["M2", "M1"], "the voyage list: {list:?}");
}

/// T-441: an active voyage still wins over the described landed one.
#[test]
fn an_active_voyage_still_wins_over_a_later_departed_plan_less_one() {
    let mut store = world();
    store
        .write(|tx| {
            planless(tx, "M1.1-cancelled", NOW - 1800)?;
            task(tx, "T-5", 5, "M1.1-cancelled", "hull", TaskState::Cancelled)
        })
        .unwrap();

    let plan = board(&mut store, None);
    assert_eq!(plan.shown.as_ref().map(|voyage| voyage.milestone.as_str()), Some("M2"));
    assert!(
        plan.shown.as_ref().is_some_and(|voyage| voyage.sailing),
        "the sailing voyage is the one shown"
    );
}
