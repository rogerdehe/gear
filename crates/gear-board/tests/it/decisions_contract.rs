//! T-166: the board's fourth view (Decisions) and fifth view (Time & tokens),
//! as the prototype of `ops/plan/m4-board` draws them.

use gear_board::frame::{frame, Tab};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, NewDecision, NewFriction, NewRound, RoundKey, RoundTokens,
    Store, TaskRow,
};

const NOW: i64 = 1_800_000_000;

/// Now, in epoch seconds.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// A project with a voyage, a lane that worked a task, the captain's
/// decisions and two friction reports (one open, one closed).
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one seat model".into(),
                done: Some("a rework resumes its session".into()),
                bar: None,
                eta: Some("12h".into()),
                departure: Some(departure),
                departure_tasks: Some(2),
                departure_estimate_min: Some(70),
                updated: NOW,
            })?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Done,
                priority: "must".into(),
                lane: Some("hull".into()),
                milestone: Some("M4".into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(40),
                issue: None,
                updated: NOW,
            })?;
            Ok(())
        })
        .unwrap();
    // The work: 44 minutes of work on 40 of estimate, with its tokens.
    store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:hull".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: Some("abc1234".into()),
                report: None,
                safe_point: None,
                started: departure,
                updated: departure,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, departure)?;
            tx.set_attempt_time(
                "p",
                "T-1",
                1,
                AttemptStep::HandedIn,
                departure + 44 * 60,
            )?;
            tx.set_attempt_time(
                "p",
                "T-1",
                1,
                AttemptStep::Merged,
                departure + 44 * 60,
            )?;
            // The round of the work, with its tokens and its cost (§4.3).
            let key = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
            };
            tx.insert_round(&NewRound {
                key: key.clone(),
                seat: "p:hull".into(),
                generation: 1,
                cli: "pi".into(),
                model: Some("minimax-cn/MiniMax-M3.1-Flash".into()),
                start_at: departure,
            })?;
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(500_000),
                    output: Some(12_000),
                    cache_read: Some(300_000),
                    cache_write: None,
                    reasoning: None,
                    cost_usd: Some(0.31),
                    cost_source: "agent".into(),
                },
                departure + 44 * 60,
            )?;
            // The captain's decisions (§7.1): one starred, one not.
            tx.insert_decision_if_new(&NewDecision {
                project: "p".into(),
                key: "d1".into(),
                voyage: "M4".into(),
                at: "2026-10-27 11:40".into(),
                kind: "scope".into(),
                what: "PR-mode landing added as T-152 (user at kickoff)".into(),
                why: None,
                alt: None,
                changes: None,
                star: false,
                source: "plan".into(),
                line: None,
                plan_version: None,
            })?;
            tx.insert_decision_if_new(&NewDecision {
                project: "p".into(),
                key: "d2".into(),
                voyage: "M4".into(),
                at: "2026-10-27 12:05".into(),
                kind: "cut".into(),
                what: "light procedure moved to M5".into(),
                why: Some("needs token data from T-141 first".into()),
                alt: None,
                changes: None,
                star: true,
                source: "plan".into(),
                line: None,
                plan_version: None,
            })?;
            // The friction reports the agents left (§6.1).
            tx.insert_friction(&NewFriction {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
                seat: "p:hull".into(),
                body: "the two-repo fixture had no gate script; wrote one by hand (10m)".into(),
                required: false,
            })?;
            let closed = tx.insert_friction(&NewFriction {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
                seat: "p:deck".into(),
                body: "scripts/test.sh ran 4 min under 4 parallel builds".into(),
                required: false,
            })?;
            tx.close_friction("p", &closed, "fixed", departure + 100)?;
            Ok(())
        })
        .unwrap();
    store
}

/// The board of the fixture.
fn board(store: &mut Store) -> gear_board::Board {
    gear_board::build(
        store,
        &gear_board::Options {
            project: Some("p".into()),
            ..Default::default()
        },
        now_seconds(),
    )
    .unwrap()
}

#[test]
fn the_decisions_view_shows_the_log_the_star_and_the_frictions() {
    let mut store = world();
    let board = board(&mut store);
    let text = frame(&board, Tab::Decisions, 0, None, (120, 40));

    assert!(text.contains("DECISIONS MADE  (★ = worth a look at landing)"), "{text}");
    // Every decision, in full, with the star on the one worth a look.
    assert!(text.contains("11:40"), "the time of the decision: {text}");
    assert!(text.contains("scope"), "{text}");
    assert!(
        text.contains("PR-mode landing added as T-152 (user at kickoff)"),
        "{text}"
    );
    assert!(text.contains("★"), "the star: {text}");
    assert!(text.contains("light procedure moved to M5"), "{text}");
    assert!(
        text.contains("why: needs token data from T-141 first"),
        "the reason of the decision: {text}"
    );
    // The count of the decisions by kind.
    assert!(text.contains("by kind: 1 cut, 1 scope"), "{text}");
    // The friction reports, open and closed.
    assert!(text.contains("FRICTION REPORTS FROM AGENTS"), "{text}");
    assert!(
        text.contains("the two-repo fixture had no gate script"),
        "{text}"
    );
    assert!(text.contains("open"), "an open report: {text}");
    assert!(text.contains("closed"), "a closed report: {text}");
}

#[test]
fn the_time_and_tokens_view_shows_the_work_of_every_task() {
    let mut store = world();
    let board = board(&mut store);
    let text = frame(&board, Tab::Tokens, 0, None, (120, 40));

    assert!(text.contains("task"), "the columns: {text}");
    assert!(text.contains("tokens"), "the columns: {text}");
    // The row of the task: its estimate, its actual time, the ratio, the
    // rounds, the tokens and the cost.
    assert!(text.contains("T-1"), "{text}");
    assert!(text.contains("hull"), "{text}");
    assert!(text.contains("40m"), "the estimate: {text}");
    assert!(text.contains("44m"), "the actual time: {text}");
    assert!(text.contains("1.1x"), "the ratio: {text}");
    assert!(text.contains("812k"), "the tokens: {text}");
    assert!(text.contains("$  0.31"), "the cost: {text}");
    // And the total of the voyage so far.
    assert!(text.contains("so far: 0.8M tokens, $0.31"), "{text}");
}

#[test]
fn the_time_and_tokens_view_opens_the_task_panel_of_its_row() {
    let mut store = world();
    let board = board(&mut store);
    // The rows of the view are the tasks that ran; the panel of the selected
    // one is its rounds with their tokens.
    let tasks = gear_board::frame::token_tasks(&board.projects[0]);
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].0, "hull");
    assert_eq!(tasks[0].1.id, "T-1");
}
