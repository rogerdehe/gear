//! T-215: a lane that was retired before the voyage is not one of the
//! fleet's lanes (ops/reviews/T-171.md round 2, F10).
//!
//! The fixture is the one the review names: heat and keel, keel retired at
//! T0, the voyage departs at T0+60 with its only task on heat, heat's quota
//! pause is open from the departure, and the report is at T0+3660.

use gear_app::time;
use gear_model::{SeatRole, SeatState, TaskState};
use gear_store::testing;
use gear_store::{RoundKey, TaskRow, VoyageRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const HEAT_PANE: &str = "w1:p2";
const KEEL_PANE: &str = "w1:p3";
const MAST_PANE: &str = "w1:p4";

/// A task of the voyage.
fn task(id: &str, lane: &str) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank: 1,
        state: TaskState::Ready,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: Some("M4".into()),
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(30),
        issue: None,
        updated: T0,
    }
}

/// The voyage, with its departure.
fn voyage(departure: i64) -> VoyageRow {
    VoyageRow {
        project: "p".into(),
        milestone: "M4".into(),
        title: "the M4 station".into(),
        done: Some("(1) the first clause".into()),
        bar: Some("STABLE".into()),
        eta: Some("12h".into()),
        departure: Some(departure),
        departure_tasks: Some(1),
        departure_estimate_min: Some(30),
        updated: T0,
    }
}

/// A captain and two lanes: heat and keel.
fn world() -> gear_store::Store {
    let mut s = gear_store::Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, HEAT_PANE, "opencode", T0)?;
        testing::seed_active_seat(tx, "p", "keel", SeatRole::Lane, KEEL_PANE, "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

/// F10: a lane retired before the voyage is neither a line of the report nor
/// a part of the fleet's denominator.
#[test]
fn a_lane_retired_before_the_voyage_is_not_a_lane_of_it() {
    let mut s = world();
    s.write(|tx| {
        // Keel is retired before the voyage departs.
        tx.set_seat_state("p:keel", SeatState::Retired, None, T0)
            .unwrap();
        // The voyage departs an hour later, with its only task on heat.
        tx.upsert_voyage(&voyage(T0 + 60)).unwrap();
        tx.insert_task(&task("T-1", "heat")).unwrap();
        // Heat waits for the quota from the departure, and waits still.
        tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0 + 60)
            .unwrap();
        Ok(())
    })
    .unwrap();

    let measured = s
        .read(|tx| time::work_of(tx, "p", "M4", T0 + 3660))
        .unwrap();
    // The fleet is stopped: the only lane of the voyage waits for the quota.
    assert_eq!(measured.fleet_stopped_min, 60, "one hour of fleet stop");
    assert_eq!(measured.fleet_stops, 1);
    assert_eq!(measured.elapsed_min, Some(60), "an hour of the voyage");
    assert_eq!(
        measured.elapsed_without_quota_min,
        Some(0),
        "the whole voyage was a quota stop"
    );
    // Keel is no line of the report.
    let names: Vec<&str> = measured
        .lanes
        .iter()
        .map(|lane| lane.lane.as_str())
        .collect();
    assert!(
        !names.contains(&"keel"),
        "a retired lane is no line: {names:?}"
    );
    assert!(names.contains(&"heat"), "{names:?}");
    // The line of the report says the same.
    let line = time::voyage_work_line(&measured);
    assert!(
        line.contains("fleet stopped by quota 1h00m (1 stop)"),
        "{line}"
    );
    assert!(line.contains("elapsed without quota stops 0h00m"), "{line}");
    let rendered: Vec<String> = measured.lanes.iter().map(time::lane_line).collect();
    assert!(
        !rendered.iter().any(|line| line.contains("keel")),
        "no keel line: {rendered:?}"
    );
}

/// F10: a lane that was retired during the voyage, but worked in it, still is
/// one of the fleet's lanes.
#[test]
fn a_lane_retired_during_the_voyage_still_shows() {
    let mut s = world();
    s.write(|tx| {
        // A third lane, which works in the voyage and is retired in it.
        testing::seed_active_seat(tx, "p", "mast", SeatRole::Lane, MAST_PANE, "opencode", T0)
            .unwrap();
        // And keel, retired before the voyage, without work in it.
        tx.set_seat_state("p:keel", SeatState::Retired, None, T0)
            .unwrap();
        tx.upsert_voyage(&voyage(T0 + 60)).unwrap();
        tx.insert_task(&task("T-1", "heat")).unwrap();
        tx.insert_task(&task("T-2", "mast")).unwrap();
        // Mast worked for a quarter of an hour inside the voyage.
        let key = RoundKey {
            project: "p".into(),
            task: "T-2".into(),
            attempt: 1,
            role: "dev".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: key.clone(),
            seat: "p:mast".into(),
            generation: 1,
            cli: "opencode".into(),
            model: None,
            start_at: T0 + 120,
        })
        .unwrap();
        tx.set_round_active(&key, 900).unwrap();
        // And then it was retired, during the voyage.
        tx.set_seat_state("p:mast", SeatState::Retired, None, T0 + 1200)
            .unwrap();
        // Heat waits for the quota from the departure, and waits still.
        tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0 + 60)
            .unwrap();
        Ok(())
    })
    .unwrap();

    let measured = s
        .read(|tx| time::work_of(tx, "p", "M4", T0 + 3660))
        .unwrap();
    let names: Vec<&str> = measured
        .lanes
        .iter()
        .map(|lane| lane.lane.as_str())
        .collect();
    assert!(
        names.contains(&"mast"),
        "a lane that worked in the voyage shows: {names:?}"
    );
    let mast = measured
        .lanes
        .iter()
        .find(|lane| lane.lane == "mast")
        .unwrap();
    assert_eq!(mast.work_s, 900, "its work of the voyage");
    // Keel, retired before the voyage and without work in it, is not one.
    assert!(!names.contains(&"keel"), "{names:?}");
}
