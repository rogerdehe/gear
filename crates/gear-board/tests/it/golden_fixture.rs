//! T-343: the store the golden frames were drawn from.
//!
//! The prototype in `gear/voyages/M5/board` writes its fake data to
//! `golden/fixture.json`. This helper reads that file and seeds an in-memory
//! store through the store's own API, so the real board is drawn from the data
//! the golden frames were drawn from: one copy of the agreed data, not two.
//!
//! The decisions, the frictions, the quota and the reviewer are T-344's; this
//! helper seeds the project, the voyages and every task.

use std::collections::HashMap;

use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, ContractRow, NewRound, RoundKey, RoundTokens, Store, TaskRow,
    VerdictRow, VoyageRow,
};
use serde::Deserialize;

const FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../gear/voyages/M5/board/golden/fixture.json");

/// The minute the prototype's `now` was written near (2026-10-28 14:32 UTC):
/// the search for the exact second starts here.
const AROUND: i64 = 1_793_197_920;

/// One task of the fixture: `(id, lane, stage, title, est, actual, rounds,
/// tokens_k, cost, deps, finished, marks)`.
type Task = (String, String, String, String, i64, i64, i64, i64, f64, Vec<String>, String, String);

#[derive(Deserialize)]
struct Fixture {
    now: String,
    review_merge_m: i64,
    when: HashMap<String, (i64, Option<i64>)>,
    voyages: Vec<FixtureVoyage>,
}

#[derive(Deserialize)]
struct FixtureVoyage {
    #[serde(rename = "LANES")]
    lanes: Vec<String>,
    #[serde(rename = "T")]
    tasks: Vec<Task>,
    #[serde(rename = "V")]
    v: FixtureVoyageWords,
}

/// The `V` block of a voyage: the words the board shows.
#[derive(Deserialize)]
struct FixtureVoyageWords {
    milestone: String,
    title: String,
    goal: String,
    bar: String,
    eta_m: i64,
    used_m: i64,
    state: String,
}

/// The store the golden frames are drawn from, and the `now` they were drawn
/// at.
pub fn load() -> (Store, i64) {
    let fixture = read_fixture();
    let now = find_now(&fixture.now);
    let review = fixture.review_merge_m;
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "gear", "s", now)?;
            testing::seed_active_seat(
                tx,
                "gear",
                "captain",
                SeatRole::Captain,
                "w1:p0",
                "claude",
                now,
            )?;
            // Each lane once, in the order the names first appear.
            let mut lanes: Vec<&str> = Vec::new();
            for voyage in &fixture.voyages {
                for lane in &voyage.lanes {
                    if !lanes.contains(&lane.as_str()) {
                        lanes.push(lane);
                    }
                }
            }
            for (i, lane) in lanes.iter().enumerate() {
                testing::seed_active_seat(
                    tx,
                    "gear",
                    lane,
                    SeatRole::Lane,
                    &format!("w1:p{}", i + 1),
                    "pi",
                    now,
                )?;
            }

            let mut rank = 0;
            for (i, voyage) in fixture.voyages.iter().enumerate() {
                let words = &voyage.v;
                let used = words.used_m;
                let departure = if words.state == "sailing" {
                    now - used * 60
                } else {
                    now - 86_400 * i as i64 - used * 60
                };
                let seeded: Vec<&Task> =
                    voyage.tasks.iter().filter(|task| task.2 != "moved").collect();
                tx.upsert_voyage(&VoyageRow {
                    project: "gear".into(),
                    milestone: words.milestone.clone(),
                    title: words.title.clone(),
                    done: Some(words.goal.clone()),
                    bar: Some(words.bar.clone()),
                    eta: Some(format!("{}m", words.eta_m)),
                    departure: Some(departure),
                    departure_tasks: Some(seeded.len() as i64),
                    departure_estimate_min: Some(seeded.iter().map(|task| task.4).sum()),
                    updated: now,
                })?;
                // A minute of a voyage's clock.
                let d = |m: i64| departure + m * 60;

                for task in &voyage.tasks {
                    rank += 1;
                    let (id, lane, stage, title, est, actual, rounds, tokens_k, cost, deps, _, _) =
                        task;
                    if stage == "moved" {
                        continue;
                    }
                    let state = match stage.as_str() {
                        "done" => TaskState::Done,
                        "review" | "passed" => TaskState::Verifying,
                        "rework" | "working" => TaskState::Running,
                        "deps" | "queued" => TaskState::Ready,
                        other => panic!("the fixture has the stage {other:?} for {id}"),
                    };
                    tx.insert_task(&TaskRow {
                        repo: None,
                        base: None,
                        gate: None,
                        rework_estimate_min: None,
                        project: "gear".into(),
                        id: id.clone(),
                        rank,
                        state,
                        priority: "must".into(),
                        lane: Some(lane.clone()),
                        milestone: Some(words.milestone.clone()),
                        required: false,
                        contract_version: 1,
                        deps: deps.clone(),
                        followup_of: None,
                        wait_on: None,
                        wait_cond: None,
                        rework_note: None,
                        estimate_min: Some(*est),
                        issue: None,
                        updated: now,
                    })?;
                    tx.insert_contract(&ContractRow {
                        project: "gear".into(),
                        task: id.clone(),
                        version: 1,
                        goal: title.clone(),
                        accept: String::new(),
                        evidence_files: vec![],
                        evidence_due_min: None,
                        body: String::new(),
                        created: now,
                        mechanical: None,
                    })?;

                    let when = fixture.when.get(id);
                    let start = when.map(|(s, _)| *s);
                    match stage.as_str() {
                        "done" => {
                            let s = start.expect("a done task has a WHEN start");
                            let merged = when
                                .and_then(|(_, m)| *m)
                                .expect("a done task has a WHEN merge");
                            let reviewed = merged - review;
                            for n in 1..=*rounds {
                                let state = if n == *rounds {
                                    AttemptState::Accepted
                                } else {
                                    AttemptState::Rejected
                                };
                                tx.insert_attempt(&attempt(id, lane, n, state, d(s)))?;
                            }
                            tx.set_attempt_time("gear", id, 1, AttemptStep::Delivered, d(s))?;
                            tx.set_attempt_time("gear", id, 1, AttemptStep::Started, d(s))?;
                            for n in 1..*rounds {
                                tx.set_attempt_time("gear", id, n, AttemptStep::HandedIn, d(s))?;
                                tx.insert_verdict(&verdict(id, n, false, d(s)))?;
                            }
                            if *rounds > 1 {
                                tx.set_attempt_time(
                                    "gear",
                                    id,
                                    *rounds,
                                    AttemptStep::Started,
                                    d(s),
                                )?;
                            }
                            tx.set_attempt_time(
                                "gear",
                                id,
                                *rounds,
                                AttemptStep::HandedIn,
                                d(reviewed),
                            )?;
                            tx.set_attempt_time(
                                "gear",
                                id,
                                *rounds,
                                AttemptStep::Verdict,
                                d(reviewed),
                            )?;
                            tx.set_attempt_time(
                                "gear",
                                id,
                                *rounds,
                                AttemptStep::Settled,
                                d(reviewed),
                            )?;
                            tx.set_attempt_time("gear", id, *rounds, AttemptStep::Merged, d(merged))?;
                            tx.insert_verdict(&verdict(id, *rounds, true, d(reviewed)))?;
                        }
                        "review" => {
                            let s = start.expect("a task in review has a WHEN start");
                            tx.insert_attempt(&attempt(id, lane, 1, AttemptState::Submitted, d(s)))?;
                            tx.set_attempt_time("gear", id, 1, AttemptStep::Started, d(s))?;
                            tx.set_attempt_time(
                                "gear",
                                id,
                                1,
                                AttemptStep::HandedIn,
                                now - 12 * 60,
                            )?;
                        }
                        "rework" => {
                            let s = start.expect("a task in rework has a WHEN start");
                            for n in 1..=*rounds {
                                if n == *rounds {
                                    // The captain scopes what is left: it waits.
                                    tx.insert_attempt(&attempt(id, lane, n, AttemptState::Started, now))?;
                                    tx.set_attempt_time("gear", id, n, AttemptStep::Started, now)?;
                                } else {
                                    tx.insert_attempt(&attempt(
                                        id,
                                        lane,
                                        n,
                                        AttemptState::Rejected,
                                        d(s),
                                    ))?;
                                    tx.set_attempt_time("gear", id, n, AttemptStep::Started, d(s))?;
                                    tx.set_attempt_time("gear", id, n, AttemptStep::HandedIn, d(s))?;
                                    tx.insert_verdict(&verdict(id, n, false, d(s)))?;
                                }
                            }
                        }
                        "working" => {
                            let s = start.expect("a working task has a WHEN start");
                            tx.insert_attempt(&attempt(id, lane, 1, AttemptState::Started, d(s)))?;
                            tx.set_attempt_time("gear", id, 1, AttemptStep::Started, d(s))?;
                        }
                        "passed" => {
                            let s = start.expect("a passed task has a WHEN start");
                            let handed = d(s + actual);
                            tx.insert_attempt(&attempt(id, lane, 1, AttemptState::Accepted, d(s)))?;
                            tx.set_attempt_time("gear", id, 1, AttemptStep::Started, d(s))?;
                            tx.set_attempt_time("gear", id, 1, AttemptStep::HandedIn, handed)?;
                            tx.set_attempt_time("gear", id, 1, AttemptStep::Verdict, handed)?;
                            tx.insert_verdict(&verdict(id, 1, true, handed))?;
                        }
                        "deps" | "queued" => {}
                        other => panic!("the fixture has the stage {other:?} for {id}"),
                    }

                    // The dev round; the review round is T-344's.
                    if *tokens_k > 0 {
                        let s = start.expect("a task whose round is seeded has a WHEN start");
                        let key = RoundKey {
                            project: "gear".into(),
                            task: id.clone(),
                            attempt: 1,
                            role: "dev".into(),
                        };
                        tx.insert_round(&NewRound {
                            key: key.clone(),
                            seat: format!("gear:{lane}"),
                            generation: 1,
                            cli: "pi".into(),
                            model: None,
                            start_at: d(s),
                        })?;
                        tx.set_round_active(&key, actual * 60)?;
                        tx.set_round_tokens(
                            &key,
                            &RoundTokens {
                                input: Some(tokens_k * 3 / 4 * 1000),
                                output: Some(0),
                                cache_read: Some(0),
                                cache_write: Some(0),
                                reasoning: None,
                                cost_usd: Some(cost * 0.75),
                                cost_source: "agent".into(),
                            },
                            now,
                        )?;
                        if !["working", "rework"].contains(&stage.as_str()) {
                            tx.close_round(&key, d(s) + actual * 60)?;
                        }
                    }
                }
            }
            Ok(())
        })
        .unwrap();
    (store, now)
}

/// The fixture, read from the prototype's directory.
fn read_fixture() -> Fixture {
    let text = std::fs::read_to_string(FIXTURE).unwrap_or_else(|error| panic!("{FIXTURE}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{FIXTURE}: {error}"))
}

/// The one second whose local clock is the fixture's `now`: the search walks
/// minute by minute over the day around `AROUND`, so the clocks are right in
/// any time zone without touching `TZ`.
fn find_now(wanted: &str) -> i64 {
    let mut ts = AROUND - 43_200;
    while ts < AROUND + 43_200 {
        if gear_board::idle::clock(ts) == *wanted {
            return ts;
        }
        ts += 60;
    }
    panic!("no minute of the day the fixture was drawn in shows {wanted:?}");
}

/// An attempt of a task, started at `started`.
fn attempt(id: &str, lane: &str, n: i64, state: AttemptState, started: i64) -> AttemptRow {
    AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
        project: "gear".into(),
        task: id.into(),
        n,
        seat: format!("gear:{lane}"),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state,
        candidate: None,
        report: None,
        safe_point: None,
        started,
        updated: started,
    }
}

/// The captain's verdict on an attempt.
fn verdict(id: &str, n: i64, passed: bool, created: i64) -> VerdictRow {
    VerdictRow {
        project: "gear".into(),
        task: id.into(),
        attempt: n,
        contract_version: 1,
        candidate: "c".into(),
        passed,
        evidence: None,
        by_seat: "gear:captain".into(),
        created,
    }
}

#[test]
fn the_store_is_drawn_from_the_fixtures_data() {
    let (mut store, now) = load();
    assert_eq!(gear_board::idle::clock(now), "14:32");

    store
        .read(|tx| {
            let m5: Vec<TaskRow> = tx
                .tasks_of_project("gear")?
                .into_iter()
                .filter(|task| task.milestone.as_deref() == Some("M5"))
                .collect();
            assert_eq!(m5.len(), 12, "{m5:?}");
            assert_eq!(tx.get_task("gear", "T-262")?.unwrap().state, TaskState::Verifying);
            assert_eq!(tx.get_task("gear", "T-265")?.unwrap().state, TaskState::Ready);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_merged_task_has_the_fixtures_step_times() {
    let (mut store, now) = load();
    store
        .read(|tx| {
            let times = tx.attempt_times_of_task("gear", "T-257")?;
            assert_eq!(times.len(), 1);
            assert_eq!(times[0].1.started, Some(now - 207 * 60));
            assert_eq!(times[0].1.merged, Some(now - 181 * 60));

            // T-260 was rejected once and accepted the second time.
            let attempts = tx.attempts_of_task("gear", "T-260")?;
            assert_eq!(attempts.len(), 2);
            assert_eq!(attempts[0].n, 1);
            assert_eq!(attempts[0].state, AttemptState::Rejected);
            assert_eq!(attempts[1].state, AttemptState::Accepted);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_working_task_has_its_open_dev_round() {
    let (mut store, _) = load();
    store
        .read(|tx| {
            let rounds = tx.rounds_of_task("gear", "T-264")?;
            let dev =
                rounds.iter().find(|round| round.role == "dev").expect("the dev round of T-264");
            assert_eq!(dev.active_s, 1320);
            assert_eq!(dev.input, Some(285_000));
            assert_eq!(dev.end_at, None);

            // A task moved to another milestone has no row of its own.
            assert!(tx.get_task("gear", "T-182")?.is_none());
            Ok(())
        })
        .unwrap();
}
