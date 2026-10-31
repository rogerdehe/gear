//! T-334: the board's model knows the voyage's stations — which are reached,
//! which is now, which is next, and how far each is.
//!
//! The stations are the store's (the stations store task, M5 S3): the tests
//! write them as that store API writes them, one `<id>: <needs>` line under
//! `voyage-stations:<project>:<milestone>` and the task's id under
//! `task-station:<project>:<task>`.

use gear_board::stations::{course, goal_text, progress, StationProgress, StationState};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, Store, TaskRow, VoyageRow};

const NOW: i64 = 1_800_000_000;

/// The voyage's stations, as the plan words them.
const STATIONS: [(&str, &str); 3] = [
    ("S1", "reliability fixes and conventions as configuration"),
    ("S2", "the board shows the stations"),
    ("S3", "the voyage is released"),
];

/// A voyage with S1 (two tasks, both merged), S2 (one merged and one Running
/// with 30 minutes of estimate) and S3 (one Ready task).
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            voyage(tx, Some("users install it"))?;
            stations(tx, &STATIONS)?;
            task(tx, "T-1", 1, TaskState::Done, 30, Some("S1"))?;
            task(tx, "T-2", 2, TaskState::Done, 30, Some("S1"))?;
            task(tx, "T-3", 3, TaskState::Done, 60, Some("S2"))?;
            task(tx, "T-4", 4, TaskState::Running, 30, Some("S2"))?;
            task(tx, "T-5", 5, TaskState::Ready, 45, Some("S3"))?;
            Ok(())
        })
        .unwrap();
    // S1 and S2's first task are merged; T-4 and T-5 are not yet.
    store
        .write(|tx| {
            attempt(tx, "T-1", NOW - 3_000, NOW - 2_400)?;
            attempt(tx, "T-2", NOW - 3_000, NOW - 2_400)?;
            attempt(tx, "T-3", NOW - 2_400, NOW - 1_800)
        })
        .unwrap();
    store
}

fn voyage(tx: &gear_store::Tx<'_>, done: Option<&str>) -> gear_model::GearResult<()> {
    tx.upsert_voyage(&VoyageRow {
        project: "p".into(),
        milestone: "M5".into(),
        title: "a complete version that never stops".into(),
        done: done.map(str::to_string),
        bar: Some("an error in normal use blocks the task".into()),
        eta: Some("16h".into()),
        departure: Some(NOW - 3_600),
        departure_tasks: Some(5),
        departure_estimate_min: Some(195),
        updated: NOW,
    })
}

/// The stations of the voyage, in plan order (the store's own format).
fn stations(tx: &gear_store::Tx<'_>, stations: &[(&str, &str)]) -> gear_model::GearResult<()> {
    let text = stations
        .iter()
        .map(|(id, needs)| format!("{id}: {needs}"))
        .collect::<Vec<_>>()
        .join("\n");
    tx.set_meta("voyage-stations:p:M5", &text)
}

fn task(
    tx: &gear_store::Tx<'_>,
    id: &str,
    rank: i64,
    state: TaskState,
    estimate: i64,
    station: Option<&str>,
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
        lane: Some("hull".into()),
        milestone: Some("M5".into()),
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(estimate),
        issue: None,
        updated: NOW,
    })?;
    if let Some(station) = station {
        tx.set_meta(&format!("task-station:p:{id}"), station)?;
    }
    Ok(())
}

/// A merged attempt of the task: it started, was handed in and merged.
fn attempt(
    tx: &gear_store::Tx<'_>,
    task: &str,
    started: i64,
    merged: i64,
) -> gear_model::GearResult<()> {
    tx.insert_attempt(&AttemptRow {
        project: "p".into(),
        task: task.into(),
        n: 1,
        seat: "p:hull".into(),
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

fn board(store: &mut Store) -> gear_board::ProjectBoard {
    let options = gear_board::Options {
        project: Some("p".into()),
        ..Default::default()
    };
    gear_board::build(store, &options, NOW)
        .unwrap()
        .projects
        .into_iter()
        .next()
        .expect("the project")
}

fn station(stations: &[StationProgress], id: &str) -> StationProgress {
    stations
        .iter()
        .find(|station| station.id == id)
        .unwrap_or_else(|| panic!("no station {id}"))
        .clone()
}

#[test]
fn the_stations_are_reached_now_and_next() {
    let mut store = world();
    let stations = store.read(|tx| progress(tx, "p", "M5")).unwrap();

    // S1 is merged whole; S2 is the work now (one task left, 30 minutes of
    // estimate); S3 is the next one.
    assert_eq!(
        stations
            .iter()
            .map(|station| station.state)
            .collect::<Vec<_>>(),
        vec![StationState::Reached, StationState::Now, StationState::Next]
    );
    let s1 = station(&stations, "S1");
    assert_eq!((s1.tasks, s1.merged, s1.left_min), (2, 2, 0));
    assert_eq!(s1.needs, STATIONS[0].1);
    let s2 = station(&stations, "S2");
    assert_eq!((s2.tasks, s2.merged, s2.left_min), (2, 1, 30));
    let s3 = station(&stations, "S3");
    assert_eq!((s3.tasks, s3.merged, s3.left_min), (1, 0, 45));

    // The board carries them: the measured voyage's stations.
    let plan = board(&mut store);
    assert_eq!(plan.stations, stations);
}

#[test]
fn a_done_task_without_a_merge_time_is_not_merged() {
    let mut store = world();
    // S3's task is done — a verdict says so — but its merge has not landed.
    store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-5")?.expect("T-5");
            task.state = TaskState::Done;
            task.updated = NOW;
            tx.update_task(&task)
        })
        .unwrap();
    let stations = store.read(|tx| progress(tx, "p", "M5")).unwrap();
    let s3 = station(&stations, "S3");
    assert_eq!((s3.tasks, s3.merged), (1, 0), "a verdict is not a merge");
    assert_eq!(s3.state, StationState::Next);
    // The voyage counts it as work left.
    let left = store.read(|tx| course(tx, "p", "M5")).unwrap();
    assert!(left.ends_with("2 tasks left in the voyage"), "{left}");
}

#[test]
fn the_course_names_the_station_that_is_now_and_what_is_left() {
    let mut store = world();
    let line = store.read(|tx| course(tx, "p", "M5")).unwrap();
    assert_eq!(
        line,
        "station S2 (1 of 3 reached): the board shows the stations · 1 tasks left in S2 · 2 tasks left in the voyage"
    );
}

#[test]
fn a_voyage_with_every_station_reached_has_nothing_left() {
    let mut store = world();
    store
        .write(|tx| {
            attempt(tx, "T-4", NOW - 1_800, NOW - 1_200)?;
            attempt(tx, "T-5", NOW - 1_200, NOW - 600)
        })
        .unwrap();
    let stations = store.read(|tx| progress(tx, "p", "M5")).unwrap();
    assert!(stations
        .iter()
        .all(|station| station.state == StationState::Reached));
    let line = store.read(|tx| course(tx, "p", "M5")).unwrap();
    assert_eq!(line, "all 3 stations reached · 0 tasks left in the voyage");
}

#[test]
fn a_voyage_without_stations_says_what_is_left() {
    let mut store = world();
    store
        .write(|tx| tx.set_meta("voyage-stations:p:M5", ""))
        .unwrap();
    assert!(store.read(|tx| progress(tx, "p", "M5")).unwrap().is_empty());
    let line = store.read(|tx| course(tx, "p", "M5")).unwrap();
    assert_eq!(line, "2 tasks left in the voyage");
    assert!(board(&mut store).stations.is_empty());
}

#[test]
fn the_goal_text_carries_the_done_the_bar_and_the_course() {
    let mut store = world();
    let text = store
        .read(|tx| goal_text(tx, "p", "M5"))
        .unwrap()
        .expect("a goal");
    assert!(
        text.starts_with("Your goal, voyage M5: done: users install it · bar: an error in normal use blocks the task · station S2 (1 of 3 reached): the board shows the stations · 1 tasks left in S2 · 2 tasks left in the voyage. Judge every decision against it."),
        "{text}"
    );
}

#[test]
fn a_voyage_without_its_done_has_no_goal_text() {
    let mut store = world();
    store.write(|tx| voyage(tx, None)).unwrap();
    assert_eq!(store.read(|tx| goal_text(tx, "p", "M5")).unwrap(), None);
    // A voyage that is not there has none either.
    assert_eq!(store.read(|tx| goal_text(tx, "p", "M9")).unwrap(), None);
}

#[test]
fn the_golden_fixture_carries_the_prototypes_stations() {
    let (mut store, now) = crate::golden_fixture::load();
    let options = gear_board::Options {
        project: Some("gear".into()),
        milestone: Some("M5".into()),
        ..Default::default()
    };
    let plan = gear_board::build(&mut store, &options, now)
        .unwrap()
        .projects
        .into_iter()
        .next()
        .expect("the project");
    assert_eq!(
        plan.stations
            .iter()
            .map(|station| (station.id.as_str(), station.needs.as_str(), station.state))
            .collect::<Vec<_>>(),
        vec![
            ("S1", "small-task procedure merged", StationState::Reached),
            ("S2", "board views to the prototype", StationState::Now),
            ("S3", "goal lock + v0.5.0", StationState::Next),
        ]
    );
}
