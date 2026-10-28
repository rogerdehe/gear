//! T-166: the board's fourth view (Decisions) and fifth view (Time & tokens),
//! as the prototype of `gear/voyages/M4/board` draws them.

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
    // The starred decision is in full: the time, the star, the kind, what was
    // decided.
    assert!(text.contains("12:05"), "the time of the decision: {text}");
    assert!(text.contains("★"), "the star: {text}");
    assert!(text.contains("light procedure moved to M5"), "{text}");
    assert!(
        text.contains("why: needs token data from T-141 first"),
        "the reason of the decision: {text}"
    );
    // The decision that is not starred is counted, not read in full.
    assert!(
        !text.contains("PR-mode landing added as T-152"),
        "an unstarred decision is not in full: {text}"
    );
    assert!(text.contains("by kind: 1 scope"), "the count: {text}");
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
    assert!(
        !text.contains("812k"),
        "the cache reads are not fresh tokens: {text}"
    );
    assert!(text.contains("512k"), "the fresh tokens: {text}");
    assert!(text.contains("300k"), "the cache reads are shown apart: {text}");
    assert!(text.contains("$  0.31"), "the cost: {text}");
    // And the total of the voyage so far.
    assert!(
        text.contains("so far: 0.5M tokens, 0.3M cache reads, $0.31"),
        "{text}"
    );
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

/// F2: the reason of a decision stands on its own line, so it is readable on
/// an ordinary terminal.
#[test]
fn the_reason_of_a_decision_is_readable_on_eighty_columns() {
    let mut store = world();
    let board = board(&mut store);
    let text = frame(&board, Tab::Decisions, 0, None, (80, 40));

    assert!(
        text.contains("why: needs token data from T-141 first"),
        "the whole reason is visible: {text}"
    );
}

/// F3: the decisions are the ones of the voyage the board measures, not the
/// ones of the first milestone by name.
#[test]
fn the_decisions_are_the_ones_of_the_voyage_the_board_measures() {
    let mut store = world();
    // An older voyage that is finished and has no decisions of its own.
    let departure = now_seconds() - 7200;
    store
        .write(|tx| {
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M1".into(),
                title: "the first voyage".into(),
                done: Some("it is done".into()),
                bar: None,
                eta: Some("12h".into()),
                departure: Some(departure),
                departure_tasks: Some(1),
                departure_estimate_min: Some(40),
                updated: departure,
            })?;
            // The voyage of the board has work left, so it is the active one.
            let mut task = tx.get_task("p", "T-1")?.expect("the task");
            task.state = TaskState::Running;
            tx.update_task(&task)
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, Tab::Decisions, 0, None, (120, 40));

    assert!(
        text.contains("light procedure moved to M5"),
        "the decisions of the active voyage: {text}"
    );
}

/// F4: the rounds of a task are the rows of the rounds table: a dev round and
/// a review round are two rounds of one attempt.
#[test]
fn the_rounds_of_a_task_are_the_rows_of_the_rounds_table() {
    let mut store = world();
    // The review round of the task: the same attempt, a second round.
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            let key = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "review".into(),
            };
            tx.insert_round(&gear_store::NewRound {
                key: key.clone(),
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: departure + 44 * 60,
            })?;
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(20_000),
                    output: Some(2_000),
                    cache_read: None,
                    cache_write: None,
                    reasoning: None,
                    cost_usd: Some(0.05),
                    cost_source: "agent".into(),
                },
                departure + 50 * 60,
            )
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, Tab::Tokens, 0, None, (120, 40));

    // The row of the task: two rounds now.
    assert!(text.contains("T-1"), "{text}");
    let row = text
        .lines()
        .find(|line| line.contains("T-1"))
        .expect("the row of the task");
    assert!(row.contains("  2"), "the rounds of the rounds table: {row}");
}

/// F5: the tokens and the cost that were never counted are unknown, not zero.
#[test]
fn the_unknown_usage_is_not_shown_as_zero() {
    let mut store = world();
    // A task whose round counted nothing: the fields are there, the numbers
    // are zero.
    store
        .write(|tx| {
            let key = RoundKey {
                project: "p".into(),
                task: "T-2".into(),
                attempt: 1,
                role: "dev".into(),
            };
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-2".into(),
                rank: 2,
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
                estimate_min: Some(20),
                issue: None,
                updated: NOW,
            })?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-2".into(),
                n: 1,
                seat: "p:hull".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: Some("def5678".into()),
                report: None,
                safe_point: None,
                started: NOW,
                updated: NOW,
            })?;
            tx.set_attempt_time("p", "T-2", 1, AttemptStep::Started, NOW)?;
            tx.set_attempt_time("p", "T-2", 1, AttemptStep::Merged, NOW)?;
            // The round counted nothing: a measured zero.
            tx.insert_round(&gear_store::NewRound {
                key: key.clone(),
                seat: "p:hull".into(),
                generation: 1,
                cli: "pi".into(),
                model: None,
                start_at: NOW,
            })?;
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(0),
                    output: Some(0),
                    cache_read: Some(0),
                    cache_write: Some(0),
                    reasoning: None,
                    cost_usd: None,
                    cost_source: "none".into(),
                },
                NOW,
            )
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, Tab::Tokens, 0, None, (120, 40));

    let row = text
        .lines()
        .find(|line| line.contains("T-2"))
        .expect("the row of the task");
    assert!(row.contains("0k"), "a measured zero is a zero: {row}");
    assert!(row.contains("?"), "the cost was never priced: {row}");
    assert!(
        text.contains("known so far"),
        "the total names what it does not know: {text}"
    );
}

/// F5 of the T-166 review: a sum that hides an unknown part is not a total.
/// A measured dev round plus a review round that counted nothing shows `?`
/// for the tokens and for the cost, and the footer says it is known so far.
#[test]
fn an_unmeasured_round_hides_no_part_of_the_sum() {
    let mut store = world();
    // The review round of the task: it counted nothing at all.
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            tx.insert_round(&gear_store::NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "review".into(),
                },
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: departure + 44 * 60,
            })
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, Tab::Tokens, 0, None, (120, 40));

    let row = text
        .lines()
        .find(|line| line.contains("T-1"))
        .expect("the row of the task");
    assert!(row.contains("?"), "the tokens are unknown: {row}");
    assert!(row.contains("      ?"), "the cost is unknown: {row}");
    assert!(
        !row.contains("812k"),
        "a partial sum is not shown as a total: {row}"
    );
    assert!(
        !row.contains("0.31"),
        "a partial price is not shown as a total: {row}"
    );
    assert!(
        text.contains("known so far"),
        "the total names what it does not know: {text}"
    );
}

/// F5 of the T-166 review: a review round that counted its tokens but was
/// never priced leaves the tokens whole and the cost unknown. No row of the
/// view then has a cost, so the cost column says nothing and the total names
/// the missing prices once (the user, 2026-10-28).
#[test]
fn an_unpriced_round_hides_no_part_of_the_sum() {
    let mut store = world();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            let key = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "review".into(),
            };
            tx.insert_round(&gear_store::NewRound {
                key: key.clone(),
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: departure + 44 * 60,
            })?;
            // The tokens were counted, the price was not.
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(20_000),
                    output: Some(2_000),
                    cache_read: None,
                    cache_write: None,
                    reasoning: None,
                    cost_usd: None,
                    cost_source: "none".into(),
                },
                departure + 50 * 60,
            )
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, Tab::Tokens, 0, None, (120, 40));

    let row = text
        .lines()
        .find(|line| line.contains("T-1"))
        .expect("the row of the task");
    assert!(row.contains("534k"), "the tokens are whole: {row}");
    assert!(
        !row.contains("0.31"),
        "a partial price is not shown as a total: {row}"
    );
    // No row of the view has a cost: the cost column says nothing, and the
    // total names the missing prices once instead of a dollar sum.
    let total = text
        .lines()
        .find(|line| line.contains("so far:"))
        .expect("the total line");
    assert!(
        total.ends_with("no prices set"),
        "the total names the missing prices: {total}"
    );
}

/// The user, 2026-10-28: a task that re-reads its context for twenty minutes
/// shows millions of cache reads. They are counted apart from the fresh
/// tokens, so the tokens say how much the work wrote, not how much it read.
#[test]
fn the_cache_reads_are_shown_apart_from_the_fresh_tokens() {
    let mut store = world();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            let key = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
            };
            // The round of the work: fresh tokens and cache reads.
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(1_000),
                    output: Some(500),
                    cache_read: Some(2_000_000),
                    cache_write: Some(0),
                    reasoning: None,
                    cost_usd: Some(0.31),
                    cost_source: "agent".into(),
                },
                departure + 44 * 60,
            )
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, Tab::Tokens, 0, None, (120, 40));

    let row = text
        .lines()
        .find(|line| line.contains("T-1"))
        .expect("the row of the task");
    assert!(row.contains("1k"), "the fresh tokens: {row}");
    assert!(
        !row.contains("2001k"),
        "the cache reads are not added to the fresh tokens: {row}"
    );
    assert!(row.contains("2000k"), "the cache reads: {row}");
    assert!(
        text.contains("so far: 0.0M tokens, 2.0M cache reads, $0.31"),
        "the total names both and keeps the price: {text}"
    );
}

/// The user, 2026-10-28: a project without `[prices]` priced no round, so
/// every cost cell said `?` and the total summed a dollar that does not
/// exist. The cost column now says nothing; the total says it once.
#[test]
fn a_view_without_prices_says_so_once_and_shows_no_cost_cell() {
    let mut store = world();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            let key = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
            };
            // The round of the work: counted, but never priced.
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(500_000),
                    output: Some(12_000),
                    cache_read: Some(300_000),
                    cache_write: None,
                    reasoning: None,
                    cost_usd: None,
                    cost_source: "none".into(),
                },
                departure + 44 * 60,
            )
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, Tab::Tokens, 0, None, (120, 40));

    let row = text
        .lines()
        .find(|line| line.contains("T-1"))
        .expect("the row of the task");
    assert!(
        !row.contains('?'),
        "nothing is unknown in the cost column without prices: {row}"
    );
    assert!(!row.contains('$'), "the cost column shows nothing: {row}");
    let total = text
        .lines()
        .find(|line| line.contains("so far:"))
        .expect("the total line");
    assert!(
        total.ends_with("no prices set"),
        "the total ends with the missing prices: {total}"
    );
    assert_eq!(
        total.matches("no prices set").count(),
        1,
        "said once: {total}"
    );
    assert!(!total.contains("unknown"), "nothing is unknown: {total}");
}

/// The machine-readable board carries the fresh tokens and the cache reads
/// apart, next to each other, as the view shows them (T-251).
#[test]
fn the_json_carries_the_fresh_tokens_and_the_cache_reads_apart() {
    let mut store = world();
    let board = board(&mut store);
    let doc: serde_json::Value =
        serde_json::from_str(&gear_board::to_json(&board).unwrap()).unwrap();
    let task = doc["projects"][0]["lanes"]
        .as_array()
        .expect("the lanes")
        .iter()
        .flat_map(|lane| lane["tasks"].as_array().cloned().unwrap_or_default())
        .find(|task| task["id"] == "T-1")
        .expect("the task T-1");
    assert_eq!(task["tokens"], 512_000, "the fresh tokens: {task}");
    assert_eq!(task["cache_read"], 300_000, "the cache reads: {task}");
}
