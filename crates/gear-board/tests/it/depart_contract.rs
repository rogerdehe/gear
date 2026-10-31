//! T-449: a voyage that waits for its departure time says so on the board,
//! so the user sees why nothing moves; without `[schedule]` nothing changes.
//! T-451: while the fleet rests outside its working hours the same line says
//! the rest instead, so the user sees what the seat waits for.

use gear_board::{build, idle, render, Options, View};
use gear_model::TaskState;
use gear_store::{testing, ProjectRow, Store, TaskRow, VoyageRow};

const AROUND: i64 = 1_800_000_000;

/// The one second whose local clock is `wanted`: the search walks the day
/// around `AROUND` minute by minute, so the clock is right in any time zone.
fn clock_at(wanted: &str) -> i64 {
    let mut ts = AROUND - 43_200;
    while ts < AROUND + 43_200 {
        if idle::clock(ts) == wanted {
            return ts;
        }
        ts += 60;
    }
    panic!("no minute of the day shows {wanted:?}");
}

/// `YYYY-MM-DDTHH:MM:SSZ` for unix seconds (civil-from-days, no extra crate).
fn utc(ts: i64) -> String {
    let (days, secs) = (ts.div_euclid(86_400), ts.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// A project with one voyage that has not departed and one open task, so the
/// board shows its line; `schedule` is the config's `[schedule]` text.
fn world(schedule: Option<&str>) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", AROUND)?;
            if let Some(schedule) = schedule {
                let row = tx.get_project("p")?.expect("the project");
                tx.update_project(&ProjectRow {
                    config_toml: format!("{schedule}\n"),
                    ..row
                })?;
            }
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M5".into(),
                title: "the next voyage".into(),
                done: Some("it sails".into()),
                bar: None,
                eta: Some("2h".into()),
                departure: None,
                departure_tasks: Some(1),
                departure_estimate_min: Some(30),
                updated: AROUND,
            })?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
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
                estimate_min: Some(30),
                issue: None,
                updated: AROUND,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

fn drawn(store: &mut Store, now: i64) -> String {
    let options = Options {
        project: Some("p".into()),
        ..Default::default()
    };
    let board = build(store, &options, now).expect("the board");
    render(&board, View::Lanes)
}

#[test]
fn the_board_says_when_a_waiting_voyage_departs() {
    let depart = clock_at("14:00");
    let mut store = world(Some(&format!(
        "[schedule]\ndepart_at = \"{}\"",
        utc(depart)
    )));

    let text = drawn(&mut store, depart - 600);

    assert!(text.contains("departs at 14:00"), "{text}");
}

#[test]
fn after_the_departure_time_the_voyage_line_is_unchanged() {
    let depart = clock_at("14:00");
    let mut store = world(Some(&format!(
        "[schedule]\ndepart_at = \"{}\"",
        utc(depart)
    )));

    let text = drawn(&mut store, depart);

    assert!(
        text.contains("Voyage M5 the next voyage · done: it sails"),
        "{text}"
    );
    assert!(!text.contains("departs at"), "{text}");
}

#[test]
fn without_a_departure_time_the_voyage_line_is_unchanged() {
    let mut store = world(None);
    let text = drawn(&mut store, AROUND);

    assert!(
        text.contains("Voyage M5 the next voyage · done: it sails"),
        "{text}"
    );
    assert!(!text.contains("departs at"), "{text}");
}

#[test]
fn the_board_says_the_fleet_rests_outside_the_hours() {
    let evening = clock_at("19:00");
    let mut store = world(Some("[schedule]\nhours = \"10:00-18:00\""));

    let text = drawn(&mut store, evening);

    assert!(text.contains("off hours until 10:00"), "{text}");
}

#[test]
fn inside_the_hours_the_voyage_line_is_unchanged() {
    let noon = clock_at("12:00");
    let mut store = world(Some("[schedule]\nhours = \"10:00-18:00\""));

    let text = drawn(&mut store, noon);

    assert!(
        text.contains("Voyage M5 the next voyage · done: it sails"),
        "{text}"
    );
    assert!(!text.contains("off hours until"), "{text}");
}

#[test]
fn without_hours_the_voyage_line_is_unchanged() {
    let evening = clock_at("19:00");
    let mut store = world(None);

    let text = drawn(&mut store, evening);

    assert!(
        text.contains("Voyage M5 the next voyage · done: it sails"),
        "{text}"
    );
    assert!(!text.contains("off hours until"), "{text}");
}

#[test]
fn the_board_says_the_rest_on_the_projects_clock() {
    // T-488: the hours are read in the project's zone, and the wait is shown
    // on that clock, not the machine's.
    let mut store = world(Some(
        "[schedule]\nhours = \"10:00-18:00\"\ntz = \"Asia/Shanghai\"",
    ));
    // 08:00 UTC is 16:00 in Shanghai: inside the hours.
    let text = drawn(&mut store, AROUND);
    assert!(!text.contains("off hours until"), "{text}");
    // 12:00 UTC is 20:00 in Shanghai: resting until 10:00 there.
    let text = drawn(&mut store, AROUND + 4 * 3600);
    assert!(text.contains("off hours until 10:00"), "{text}");
}

#[test]
fn while_the_fleet_rests_the_board_says_the_rest_not_the_departure() {
    let evening = clock_at("19:00");
    let depart = evening + 3 * 3600;
    let mut store = world(Some(&format!(
        "[schedule]\nhours = \"10:00-18:00\"\ndepart_at = \"{}\"",
        utc(depart)
    )));

    let text = drawn(&mut store, evening);

    assert!(text.contains("off hours until 10:00"), "{text}");
    assert!(!text.contains("departs at"), "{text}");
}
