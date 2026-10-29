//! T-343: the store the golden frames were drawn from.
//!
//! The prototype in `gear/voyages/M5/board` writes its fake data to
//! `golden/fixture.json`. This helper reads that file and seeds an in-memory
//! store through the store's own API, so the real board is drawn from the data
//! the golden frames were drawn from: one copy of the agreed data, not two.
//!
//! This helper is T-343's and T-344's: the project, the voyages and every task
//! are here, and so are the reviewer, the review rounds, the decisions, the
//! frictions, the quota readings and the goal checks.

use std::collections::HashMap;

use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, ContractRow, NewDecision, NewFriction, NewRound, RoundKey,
    RoundTokens, Store, TaskRow, VerdictRow, VoyageRow,
};
use serde::Deserialize;
use serde_json::json;

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
    /// `(provider, window, used_percent, resets)`.
    quota: Vec<FixtureQuota>,
    when: HashMap<String, (i64, Option<i64>)>,
    voyages: Vec<FixtureVoyage>,
}

/// One quota reading of the fixture.
#[derive(Deserialize)]
struct FixtureQuota {
    provider: String,
    window: String,
    /// Kept as the fixture writes it: a whole number stays a whole number.
    used: serde_json::Number,
    /// `HH:MM` (a time today) or `Mon DD` (a date).
    resets: String,
}

#[derive(Deserialize)]
struct FixtureVoyage {
    #[serde(rename = "LANES")]
    lanes: Vec<String>,
    #[serde(rename = "T")]
    tasks: Vec<Task>,
    /// `(at, kind, what, why, star)`.
    #[serde(rename = "DECISIONS")]
    decisions: Vec<(String, String, String, String, bool)>,
    /// `(met, text)`.
    #[serde(rename = "DONE")]
    done: Vec<(bool, String)>,
    /// `(id, what, state, at_m)`: the prototype's stations (T-352); the
    /// `at_m` minute is not stored — nothing draws it.
    #[serde(rename = "STATIONS")]
    stations: Vec<(String, String, String, i64)>,
    /// `(task, lane, status, body)`.
    #[serde(rename = "FRICTION")]
    friction: Vec<(String, String, String, String)>,
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
            let mut project = testing::seed_project(tx, "gear", "s", now)?;
            // The prototype's project reads the two providers its quota line
            // draws; the board shows a reading only when the project reads it
            // now (T-383).
            project.config_toml = concat!(
                "[[quota.sources]]\nprovider = \"tokenrhythm\"\n",
                "[[quota.sources]]\nprovider = \"codex\"\n",
            )
            .to_string();
            tx.update_project(&project)?;
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
                let decisions = &voyage.decisions;
                let done = &voyage.done;
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

                // The decisions the prototype's log draws, in order.
                for (i, (at, kind, what, why, star)) in decisions.iter().enumerate() {
                    tx.insert_decision_if_new(&NewDecision {
                        project: "gear".into(),
                        key: format!("golden-{}-{i}", words.milestone),
                        voyage: words.milestone.clone(),
                        at: format!("2026-10-28 {at}"),
                        kind: kind.clone(),
                        what: what.clone(),
                        why: if why.is_empty() { None } else { Some(why.clone()) },
                        alt: None,
                        changes: None,
                        star: *star,
                        source: "plan".into(),
                        line: None,
                        plan_version: None,
                    })?;
                }
                // The goal check the board shows from T-354.
                let check: Vec<serde_json::Value> = done
                    .iter()
                    .map(|(met, text)| json!({"met": met, "text": text}))
                    .collect();
                tx.set_meta(
                    &format!("goal-check:gear:{}", words.milestone),
                    &serde_json::to_string(&check).unwrap(),
                )?;

                // The stations the header draws (T-352): the prototype's
                // STATIONS as the store's station words (id, needs = what),
                // and each station's tasks, so `stations::progress` reads
                // "done" as Reached, "now" as Now and "next" as Next. A
                // task counts as merged only with a merge time: the fixture's
                // "done" tasks have one, the open stages have none.
                let stations: Vec<gear_store::VoyageStation> = voyage
                    .stations
                    .iter()
                    .map(|(id, what, _, _)| gear_store::VoyageStation {
                        id: id.clone(),
                        needs: what.clone(),
                    })
                    .collect();
                tx.set_voyage_stations("gear", &words.milestone, &stations)?;
                let merged: Vec<&String> = voyage
                    .tasks
                    .iter()
                    .filter(|task| task.2 == "done")
                    .map(|task| &task.0)
                    .collect();
                let open: Vec<&String> = voyage
                    .tasks
                    .iter()
                    .filter(|task| {
                        matches!(
                            task.2.as_str(),
                            "review" | "passed" | "rework" | "working" | "deps" | "queued"
                        )
                    })
                    .map(|task| &task.0)
                    .collect();
                let reached_count = voyage
                    .stations
                    .iter()
                    .filter(|(_, _, state, _)| state == "done")
                    .count();
                let ahead_count = stations.len() - reached_count;
                assert!(
                    merged.len() >= reached_count,
                    "{}: every reached station needs a merged task",
                    words.milestone
                );
                assert!(
                    open.len() >= ahead_count,
                    "{}: every station ahead needs an open task",
                    words.milestone
                );
                // Spread each kind over its stations, so every one of them
                // keeps at least one task and its state in `progress`.
                let (mut done_rank, mut open_rank) = (0, 0);
                for (id, _, state, _) in &voyage.stations {
                    let station_tasks: Vec<&String> = if state == "done" {
                        let picked = merged
                            .iter()
                            .enumerate()
                            .filter(|(n, _)| n % reached_count == done_rank)
                            .map(|(_, task)| *task)
                            .collect();
                        done_rank += 1;
                        picked
                    } else {
                        let picked = open
                            .iter()
                            .enumerate()
                            .filter(|(n, _)| n % ahead_count == open_rank)
                            .map(|(_, task)| *task)
                            .collect();
                        open_rank += 1;
                        picked
                    };
                    for task in station_tasks {
                        tx.set_task_station("gear", task, Some(id))?;
                    }
                }

                for task in &voyage.tasks {
                    rank += 1;
                    let (
                        id,
                        lane,
                        stage,
                        title,
                        est,
                        actual,
                        rounds,
                        tokens_k,
                        cost,
                        deps,
                        _,
                        marks,
                    ) = task;
                    if stage == "moved" {
                        // A task that left the voyage is one line of the log.
                        tx.insert_decision_if_new(&NewDecision {
                            project: "gear".into(),
                            key: format!("golden-move-{id}"),
                            voyage: words.milestone.clone(),
                            at: format!("2026-10-28 {}", gear_board::idle::clock(now)),
                            kind: "move".into(),
                            what: format!("{id} moved to {marks}"),
                            why: None,
                            alt: None,
                            changes: None,
                            star: false,
                            source: "gear".into(),
                            line: None,
                            plan_version: None,
                        })?;
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

                    // The review round: on the reviewer's seat, a quarter of the
                    // tokens and the cost. T-262 is the round open right now.
                    if *tokens_k > 0 {
                        let s = start.expect("a task whose round is seeded has a WHEN start");
                        let handed_in = tx
                            .attempt_times_of_task("gear", id)?
                            .into_iter()
                            .filter(|(_, times)| times.handed_in.is_some())
                            .max_by_key(|(n, _)| *n);
                        let (a, handed) = match handed_in {
                            Some((n, times)) => (n, times.handed_in.unwrap()),
                            None => (1, d(s)),
                        };
                        let key = RoundKey {
                            project: "gear".into(),
                            task: id.clone(),
                            attempt: a,
                            role: "review".into(),
                        };
                        let open = id == "T-262";
                        tx.insert_round(&NewRound {
                            key: key.clone(),
                            seat: "gear:review-3".into(),
                            generation: 1,
                            cli: "codex".into(),
                            model: None,
                            start_at: if open { now - 12 * 60 } else { handed },
                        })?;
                        tx.set_round_tokens(
                            &key,
                            &RoundTokens {
                                input: Some((*tokens_k - *tokens_k * 3 / 4) * 1000),
                                output: Some(0),
                                cache_read: Some(0),
                                cache_write: Some(0),
                                reasoning: None,
                                cost_usd: Some(cost * 0.25),
                                cost_source: "agent".into(),
                            },
                            now,
                        )?;
                        if !open {
                            tx.close_round(&key, handed)?;
                        }
                    }
                }

                // The friction reports; the captain has read the older one.
                for (task, lane, status, body) in &voyage.friction {
                    let id = tx.insert_friction(&NewFriction {
                        project: "gear".into(),
                        task: task.clone(),
                        attempt: 1,
                        role: "dev".into(),
                        seat: format!("gear:{lane}"),
                        body: body.clone(),
                        required: false,
                    })?;
                    if status == "read" {
                        tx.close_friction("gear", &id, "read", now)?;
                    }
                }
            }

            // The reviewer that holds T-262 right now.
            testing::seed_temp_seat(tx, "gear", "review-3", "T-262", "w1:p9", "codex", now)?;

            // The quota readings the board shows beside the lanes.
            for reading in &fixture.quota {
                let resets = if reading.resets.contains(':') {
                    // A clock time: the first minute after now that shows it.
                    let mut ts = now + 60;
                    let limit = now + 2 * 86_400;
                    while gear_board::idle::clock(ts) != reading.resets {
                        assert!(ts < limit, "no minute today shows {:?}", reading.resets);
                        ts += 60;
                    }
                    ts
                } else {
                    // A date: two days out.
                    now + 2 * 86_400
                };
                tx.set_meta(
                    &format!("quota.reading.{}", reading.provider),
                    &json!({
                        "provider": reading.provider,
                        "windows": [{
                            "name": reading.window,
                            "used_percent": reading.used,
                            "resets_at": resets,
                        }],
                        "read_at": now,
                    })
                    .to_string(),
                )?;
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

#[test]
fn the_reviewer_holds_t262_and_its_round_is_open() {
    let (mut store, now) = load();
    store
        .read(|tx| {
            let seat = tx
                .seats_of_project("gear")?
                .into_iter()
                .find(|seat| seat.id == "gear:review-3")
                .expect("the fixture's reviewer");
            assert_eq!(seat.task_id.as_deref(), Some("T-262"));

            let rounds = tx.rounds_of_task("gear", "T-262")?;
            let review = rounds
                .iter()
                .find(|round| round.role == "review")
                .expect("the review round of T-262");
            assert_eq!(review.seat, "gear:review-3");
            assert_eq!(review.cli, "codex");
            assert_eq!(review.attempt, 1);
            assert_eq!(review.start_at, now - 12 * 60);
            assert_eq!(review.end_at, None);
            // A quarter of the task's tokens and cost.
            assert_eq!(review.input, Some(183_000));
            assert_eq!(review.cost_usd, Some(0.0775));
            Ok(())
        })
        .unwrap();
}

#[test]
fn every_voyage_has_the_fixtures_decisions() {
    let (mut store, _) = load();
    store
        .read(|tx| {
            let m5 = tx.decisions_of("gear", "M5")?;
            assert_eq!(m5.len(), 3, "{m5:?}");
            assert_eq!(m5.iter().filter(|decision| decision.star).count(), 2);
            assert_eq!(m5[0].at, "2026-10-28 11:05");
            assert_eq!(m5[0].kind, "scope");
            assert_eq!(m5[0].source, "plan");

            let m42 = tx.decisions_of("gear", "M4.2")?;
            let moved = m42
                .iter()
                .find(|decision| decision.kind == "move")
                .expect("the moved task's line");
            assert_eq!(moved.what, "T-182 moved to M5");
            assert_eq!(moved.source, "gear");
            assert_eq!(moved.why, None);
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_fixtures_frictions_keep_their_state() {
    let (mut store, _) = load();
    store
        .read(|tx| {
            let all = tx.frictions_of_project("gear")?;
            let open = tx.open_frictions("gear")?;
            assert_eq!(all.len(), 2, "{all:?}");
            assert_eq!(open.len(), 1, "{open:?}");
            assert_eq!(open[0].task, "T-260");
            assert_eq!(open[0].seat, "gear:hull");
            let read = all
                .iter()
                .find(|friction| friction.task == "T-248")
                .expect("the friction the captain read");
            assert_eq!(read.outcome.as_deref(), Some("read"));
            assert!(read.closed_at.is_some());
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_quota_and_the_goal_check_are_in_the_meta() {
    let (mut store, now) = load();
    store
        .read(|tx| {
            let codex: serde_json::Value =
                serde_json::from_str(&tx.get_meta("quota.reading.codex")?.unwrap()).unwrap();
            let window = &codex["windows"][0];
            assert_eq!(window["name"], "week");
            assert_eq!(window["used_percent"], 63);
            // A date two days out.
            assert_eq!(window["resets_at"].as_i64(), Some(now + 2 * 86_400));
            assert_eq!(codex["read_at"].as_i64(), Some(now));

            let rhythm: serde_json::Value =
                serde_json::from_str(&tx.get_meta("quota.reading.tokenrhythm")?.unwrap()).unwrap();
            let resets = rhythm["windows"][0]["resets_at"].as_i64().unwrap();
            assert!(resets > now);
            assert_eq!(gear_board::idle::clock(resets), "22:00");

            let check: serde_json::Value =
                serde_json::from_str(&tx.get_meta("goal-check:gear:M5")?.unwrap()).unwrap();
            let items = check.as_array().unwrap();
            assert_eq!(items.len(), 3);
            assert_eq!(items[1]["met"], true);
            assert_eq!(items[1]["text"], "the board shows every voyage");
            Ok(())
        })
        .unwrap();
}
