//! The rework of T-171: the findings F1-F8 of `ops/reviews/T-171.md`.
//!
//! Each test is the one its finding names: the default voyage, the fleet
//! stops, the voyage's own work, an open pause, the review rounds, the
//! per-round lines, the per-lane tokens and the voyage's friction.

use gear_app::time::{self, Filter};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::testing;
use gear_store::{
    AttemptRow, AttemptStep, ContractRow, NewDecision, NewFriction, RoundKey, RoundTokens, TaskRow,
    VoyageRow,
};
use std::collections::BTreeSet;

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const LANE2_PANE: &str = "w1:p3";

/// A task of a voyage.
fn task(id: &str, lane: &str, milestone: &str, state: TaskState, rank: i64) -> TaskRow {
    TaskRow {
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
        updated: T0,
    }
}

/// A contract of a task.
fn contract(id: &str) -> ContractRow {
    ContractRow {
        project: "p".into(),
        task: id.into(),
        version: 1,
        goal: format!("the goal of {id}"),
        accept: format!("the acceptance of {id}"),
        evidence_files: vec![],
        evidence_due_min: None,
        body: String::new(),
        created: T0,
        mechanical: None,
    }
}

/// A voyage with a departure.
fn voyage(milestone: &str, departure: i64) -> VoyageRow {
    VoyageRow {
        project: "p".into(),
        milestone: milestone.into(),
        title: format!("the {milestone} station"),
        done: Some("(1) the first clause".into()),
        bar: Some("STABLE".into()),
        eta: Some("12h".into()),
        departure: Some(departure),
        departure_tasks: Some(1),
        departure_estimate_min: Some(30),
        updated: T0,
    }
}

/// A captain, two lanes and a reviewer.
fn world() -> gear_store::Store {
    let mut s = gear_store::Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        testing::seed_active_seat(tx, "p", "keel", SeatRole::Lane, LANE2_PANE, "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

/// The work and waiting time of a voyage, read from the store.
fn work(s: &mut gear_store::Store, milestone: &str, now: i64) -> time::VoyageWork {
    s.read(|tx| time::work_of(tx, "p", milestone, now)).unwrap()
}

/// F1: the landing's default command must pick the voyage that departed
/// last, not the first one by name.
#[test]
fn the_default_voyage_is_the_one_that_departed_last() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M1", T0 - 10 * 86400)).unwrap();
        tx.upsert_voyage(&voyage("M4", T0)).unwrap();
        Ok(())
    })
    .unwrap();
    let milestone = s
        .read(|tx| gear_app::plan::current_voyage(tx, "p"))
        .unwrap();
    assert_eq!(milestone, "M4", "the voyage that departed last");
}

/// F2: the fleet is stopped only while **every** lane is paused at once.
#[test]
fn the_fleet_is_stopped_only_while_every_lane_is_paused() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M4", T0)).unwrap();
        tx.insert_task(&task("T-1", "heat", "M4", TaskState::Ready, 1))
            .unwrap();
        tx.insert_task(&task("T-2", "keel", "M4", TaskState::Ready, 2))
            .unwrap();
        // Only the first lane is paused: the fleet still works.
        tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0)
            .unwrap();
        tx.close_pause("p", "p:heat", "quota", T0 + 3600).unwrap();
        Ok(())
    })
    .unwrap();
    let measured = work(&mut s, "M4", T0 + 3600);
    assert_eq!(
        measured.fleet_stopped_min, 0,
        "one paused lane does not stop the fleet"
    );
    assert_eq!(measured.fleet_stops, 0);
    // The lane itself waited an hour.
    let heat = measured
        .lanes
        .iter()
        .find(|lane| lane.lane == "heat")
        .unwrap();
    assert_eq!(heat.quota_wait_s, 3600);

    // Now both lanes are paused at once: the fleet stops.
    s.write(|tx| {
        tx.open_pause("p", "p:keel", "quota", "minimax-cn 5h", T0)
            .unwrap();
        tx.close_pause("p", "p:keel", "quota", T0 + 3600).unwrap();
        Ok(())
    })
    .unwrap();
    let measured = work(&mut s, "M4", T0 + 3600);
    assert_eq!(measured.fleet_stopped_min, 60, "both lanes paused at once");
    assert_eq!(measured.fleet_stops, 1);

    // The pauses of the two lanes at different times never stop the fleet.
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M4", T0)).unwrap();
        tx.insert_task(&task("T-1", "heat", "M4", TaskState::Ready, 1))
            .unwrap();
        tx.insert_task(&task("T-2", "keel", "M4", TaskState::Ready, 2))
            .unwrap();
        tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0)
            .unwrap();
        tx.close_pause("p", "p:heat", "quota", T0 + 3600).unwrap();
        tx.open_pause("p", "p:keel", "quota", "minimax-cn 5h", T0 + 7200)
            .unwrap();
        tx.close_pause("p", "p:keel", "quota", T0 + 10800).unwrap();
        Ok(())
    })
    .unwrap();
    let measured = work(&mut s, "M4", T0 + 10800);
    assert_eq!(measured.fleet_stopped_min, 0, "the pauses do not overlap");
    assert_eq!(measured.fleet_stops, 0);
}

/// F3: a voyage's work and waiting time is its own: the rounds and pauses of
/// an earlier voyage on the same lane are not counted.
#[test]
fn a_voyage_measures_its_own_work_only() {
    let mut s = world();
    s.write(|tx| {
        // M1 departed long ago and its work is done.
        tx.upsert_voyage(&voyage("M1", T0 - 10 * 86400)).unwrap();
        tx.insert_task(&task("T-1", "heat", "M1", TaskState::Done, 1))
            .unwrap();
        tx.insert_contract(&contract("T-1")).unwrap();
        // M2 departs now.
        tx.upsert_voyage(&voyage("M2", T0)).unwrap();
        tx.insert_task(&task("T-2", "heat", "M2", TaskState::Running, 2))
            .unwrap();
        tx.insert_contract(&contract("T-2")).unwrap();
        // The lane worked an hour for M1, before M2 departed.
        let key = RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: key.clone(),
            seat: "p:heat".into(),
            generation: 1,
            cli: "opencode".into(),
            model: None,
            start_at: T0 - 10 * 86400,
        })
        .unwrap();
        tx.set_round_active(&key, 3600).unwrap();
        // And it waited for the quota back then.
        tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0 - 10 * 86400)
            .unwrap();
        tx.close_pause("p", "p:heat", "quota", T0 - 10 * 86400 + 3600)
            .unwrap();
        Ok(())
    })
    .unwrap();
    let measured = work(&mut s, "M2", T0);
    assert_eq!(measured.elapsed_min, Some(0), "M2 departed now");
    let heat = measured
        .lanes
        .iter()
        .find(|lane| lane.lane == "heat")
        .unwrap();
    assert_eq!(heat.work_s, 0, "M1's work is not M2's work");
    assert_eq!(heat.quota_wait_s, 0, "M1's quota wait is not M2's");
    assert_eq!(measured.fleet_stopped_min, 0);
}

/// F4: a pause that is still open (a quota park or a declared wait that has
/// not ended) counts until now.
#[test]
fn an_open_pause_counts_until_now() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M4", T0)).unwrap();
        tx.insert_task(&task("T-1", "heat", "M4", TaskState::Ready, 1))
            .unwrap();
        // The quota park is still open: the lane waits.
        tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0)
            .unwrap();
        Ok(())
    })
    .unwrap();
    let measured = work(&mut s, "M4", T0 + 7200);
    let heat = measured
        .lanes
        .iter()
        .find(|lane| lane.lane == "heat")
        .unwrap();
    assert_eq!(heat.quota_wait_s, 7200, "the open pause counts until now");
    // The other lane works, so the fleet is not stopped.
    assert_eq!(measured.fleet_stopped_min, 0);
}

/// F5: the tokens and the cost of a task count every stored round, the
/// reviewer's included.
#[test]
fn a_tasks_tokens_count_the_review_round_too() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&task("T-1", "heat", "M2", TaskState::Done, 1))
            .unwrap();
        tx.insert_contract(&contract("T-1")).unwrap();
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: "T-1".into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: AttemptState::Accepted,
            candidate: Some("abc".into()),
            report: None,
            safe_point: None,
            started: T0,
            updated: T0,
        })
        .unwrap();
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)
            .unwrap();
        // The lane's own round.
        let dev = RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: dev.clone(),
            seat: "p:heat".into(),
            generation: 1,
            cli: "opencode".into(),
            model: None,
            start_at: T0,
        })
        .unwrap();
        tx.set_round_tokens(
            &dev,
            &RoundTokens {
                input: Some(100),
                output: Some(50),
                cache_read: Some(0),
                cache_write: Some(0),
                reasoning: Some(0),
                cost_usd: Some(0.10),
                cost_source: "prices".into(),
            },
            T0 + 60,
        )
        .unwrap();
        // The reviewer's round.
        let review = RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "review".into(),
        };
        tx.insert_round(&gear_store::NewRound {
            key: review.clone(),
            seat: "p:r-1".into(),
            generation: 1,
            cli: "codex".into(),
            model: None,
            start_at: T0 + 60,
        })
        .unwrap();
        tx.set_round_tokens(
            &review,
            &RoundTokens {
                input: Some(200),
                output: Some(100),
                cache_read: Some(0),
                cache_write: Some(0),
                reasoning: Some(0),
                cost_usd: Some(0.20),
                cost_source: "prices".into(),
            },
            T0 + 120,
        )
        .unwrap();
        Ok(())
    })
    .unwrap();
    let report = s
        .read(|tx| time::report(tx, "p", &Filter::default(), T0 + 300))
        .unwrap();
    let task_time = &report.tasks[0];
    assert_eq!(
        task_time.tokens.input,
        Some(300),
        "the dev and the review round"
    );
    assert_eq!(task_time.tokens.output, Some(150));
    let cost = task_time.tokens.cost_usd.expect("the cost");
    assert!(
        (cost - 0.30).abs() < 1e-9,
        "the cost of both rounds: {cost}"
    );
    // The two rounds are kept apart.
    let usage: Vec<(String, Option<i64>)> = task_time
        .rounds
        .iter()
        .flat_map(|round| round.usage.iter())
        .map(|usage| (usage.role.clone(), usage.tokens.input))
        .collect();
    assert_eq!(
        usage,
        vec![
            ("dev".to_string(), Some(100)),
            ("review".to_string(), Some(200)),
        ],
        "every stored round is kept apart"
    );
}

/// F6: the text report prints each round's own tokens and cost.
#[test]
fn the_text_report_prints_a_round_own_tokens() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&task("T-1", "heat", "M2", TaskState::Done, 1))
            .unwrap();
        tx.insert_contract(&contract("T-1")).unwrap();
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: "T-1".into(),
            n: 1,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: AttemptState::Accepted,
            candidate: Some("abc".into()),
            report: None,
            safe_point: None,
            started: T0,
            updated: T0,
        })
        .unwrap();
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)
            .unwrap();
        for (role, seat, input, cost) in [
            ("dev", "p:heat", 100i64, 0.10f64),
            ("review", "p:r-1", 200, 0.20),
        ] {
            let key = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: role.into(),
            };
            tx.insert_round(&gear_store::NewRound {
                key: key.clone(),
                seat: seat.into(),
                generation: 1,
                cli: "opencode".into(),
                model: None,
                start_at: T0,
            })
            .unwrap();
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(input),
                    output: Some(10),
                    cache_read: Some(0),
                    cache_write: Some(0),
                    reasoning: Some(0),
                    cost_usd: Some(cost),
                    cost_source: "prices".into(),
                },
                T0 + 60,
            )
            .unwrap();
        }
        Ok(())
    })
    .unwrap();
    let report = s
        .read(|tx| time::report(tx, "p", &Filter::default(), T0 + 300))
        .unwrap();
    let text = time::render(&report);
    assert!(
        text.contains("T-1 attempt 1 dev · tokens 100 in / 10 out"),
        "{text}"
    );
    assert!(
        text.contains("T-1 attempt 1 review · tokens 200 in / 10 out"),
        "{text}"
    );
}

/// F7: the voyage's part 3 carries the tokens and the cost per lane.
#[test]
fn a_voyage_carries_the_tokens_of_each_lane() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M4", T0)).unwrap();
        tx.insert_task(&task("T-1", "heat", "M4", TaskState::Done, 1))
            .unwrap();
        tx.insert_task(&task("T-2", "keel", "M4", TaskState::Done, 2))
            .unwrap();
        for (id, seat, input, cost) in [
            ("T-1", "p:heat", 100i64, 0.10f64),
            ("T-2", "p:keel", 300, 0.30),
        ] {
            let key = RoundKey {
                project: "p".into(),
                task: id.into(),
                attempt: 1,
                role: "dev".into(),
            };
            tx.insert_round(&gear_store::NewRound {
                key: key.clone(),
                seat: seat.into(),
                generation: 1,
                cli: "opencode".into(),
                model: None,
                start_at: T0,
            })
            .unwrap();
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(input),
                    output: Some(10),
                    cache_read: Some(0),
                    cache_write: Some(0),
                    reasoning: Some(0),
                    cost_usd: Some(cost),
                    cost_source: "prices".into(),
                },
                T0 + 60,
            )
            .unwrap();
        }
        Ok(())
    })
    .unwrap();
    let measured = work(&mut s, "M4", T0 + 3600);
    let heat = measured
        .lanes
        .iter()
        .find(|lane| lane.lane == "heat")
        .unwrap();
    assert_eq!(heat.tokens.input, Some(100));
    assert_eq!(heat.tokens.cost_usd, Some(0.10));
    let keel = measured
        .lanes
        .iter()
        .find(|lane| lane.lane == "keel")
        .unwrap();
    assert_eq!(keel.tokens.input, Some(300));
    assert_eq!(keel.tokens.cost_usd, Some(0.30));
}

/// F8: the voyage's friction is the friction of its own tasks.
#[test]
fn a_voyage_shows_only_its_own_friction() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M1", T0 - 10 * 86400)).unwrap();
        tx.upsert_voyage(&voyage("M4", T0)).unwrap();
        tx.insert_task(&task("T-1", "heat", "M1", TaskState::Done, 1))
            .unwrap();
        tx.insert_task(&task("T-2", "heat", "M4", TaskState::Done, 2))
            .unwrap();
        tx.insert_friction(&NewFriction {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
            seat: "p:heat".into(),
            body: "the friction of M1".into(),
            required: false,
        })
        .unwrap();
        tx.insert_friction(&NewFriction {
            project: "p".into(),
            task: "T-2".into(),
            attempt: 1,
            role: "dev".into(),
            seat: "p:heat".into(),
            body: "the friction of M4".into(),
            required: false,
        })
        .unwrap();
        Ok(())
    })
    .unwrap();
    let report = s
        .read(|tx| gear_app::voyage::report(tx, "p", "M4", false, T0 + 60))
        .unwrap();
    let bodies: Vec<&str> = report
        .frictions
        .iter()
        .map(|row| row.body.as_str())
        .collect();
    assert_eq!(
        bodies,
        vec!["the friction of M4"],
        "only this voyage's friction"
    );
}

/// T-335: the confirmed goal changes are a part of the report of their own
/// (M5 kickoff, S3), right after the `done:` sentence, with the before and the
/// after of every change.
#[test]
fn the_report_lists_the_goal_change_of_its_voyage() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M1", T0)).unwrap();
        // The user confirmed the change after departure (T-410).
        tx.insert_decision_if_new(&NewDecision {
            project: "p".into(),
            key: "g1".into(),
            voyage: "M1".into(),
            at: "2026-10-27 12:30".into(),
            kind: "user".into(),
            what: "done of M1 changed from \"users install v0.1.0\" to \"users install v0.2.0\""
                .into(),
            why: Some("confirmed by the user".into()),
            alt: None,
            changes: None,
            star: true,
            source: "gear".into(),
            line: None,
            plan_version: None,
        })
        .unwrap();
        // A captain decision of another kind is not a goal change.
        tx.insert_decision_if_new(&NewDecision {
            project: "p".into(),
            key: "d1".into(),
            voyage: "M1".into(),
            at: "2026-10-27 12:40".into(),
            kind: "cut".into(),
            what: "the light procedure moved to M5".into(),
            why: None,
            alt: None,
            changes: None,
            star: false,
            source: "plan".into(),
            line: None,
            plan_version: None,
        })
        .unwrap();
        Ok(())
    })
    .unwrap();
    let report = s
        .read(|tx| gear_app::voyage::report(tx, "p", "M1", false, T0 + 60))
        .unwrap();
    assert_eq!(
        report.goal_changes.len(),
        1,
        "one goal change: {:?}",
        report.goal_changes
    );
    let text = gear_app::voyage::render(&report);
    let lines: Vec<&str> = text.lines().collect();
    let done = lines
        .iter()
        .position(|line| *line == "Done (M1)")
        .expect("the done part");
    let part = lines
        .iter()
        .position(|line| *line == "Goal changes (M1)")
        .unwrap_or_else(|| panic!("the goal changes part: {text}"));
    assert!(part > done, "the part follows the done sentence: {text}");
    let change = lines[part + 1];
    assert!(
        change.contains("users install v0.1.0") && change.contains("users install v0.2.0"),
        "the before and the after: {change}"
    );
    assert!(
        report
            .goal_changes
            .iter()
            .all(|change| change.kind == "user"),
        "another kind is not a goal change: {text}"
    );
    // T-461: the cut is its own part of the report.
    assert!(
        text.contains("Cancelled or moved (M1)")
            && text.contains("the light procedure moved to M5"),
        "the cut is listed with its line: {text}"
    );
    // `--json` carries the same list.
    let doc = serde_json::to_value(&report).unwrap();
    let changes = doc["goal_changes"]
        .as_array()
        .expect("the goal changes in JSON");
    assert_eq!(changes.len(), 1, "{doc}");
    assert!(
        changes[0]["what"]
            .as_str()
            .unwrap_or_default()
            .contains("users install v0.2.0"),
        "{doc}"
    );
}

/// T-335: a voyage without a confirmed goal change says so, once.
#[test]
fn a_voyage_without_a_goal_change_says_so() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M1", T0)).unwrap();
        Ok(())
    })
    .unwrap();
    let report = s
        .read(|tx| gear_app::voyage::report(tx, "p", "M1", false, T0 + 60))
        .unwrap();
    assert!(report.goal_changes.is_empty());
    let text = gear_app::voyage::render(&report);
    let lines: Vec<&str> = text.lines().collect();
    let part = lines
        .iter()
        .position(|line| *line == "Goal changes (M1)")
        .unwrap_or_else(|| panic!("the goal changes part: {text}"));
    assert_eq!(lines[part + 1], "  no goal change", "{text}");
    assert_eq!(
        text.matches("no goal change").count(),
        1,
        "said once: {text}"
    );
}

/// The lane set of a voyage's report is the project's lanes.
#[test]
fn the_lanes_of_a_project_are_reported() {
    let mut s = world();
    s.write(|tx| {
        tx.upsert_voyage(&voyage("M4", T0)).unwrap();
        Ok(())
    })
    .unwrap();
    let measured = work(&mut s, "M4", T0 + 60);
    let names: BTreeSet<String> = measured
        .lanes
        .iter()
        .map(|lane| lane.lane.clone())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["heat".to_string(), "keel".to_string()])
    );
}
