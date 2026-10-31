//! T-166: the board's fourth view (Decisions) and fifth view (Time & tokens),
//! as the prototype of `gear/voyages/M4/board` draws them.

use std::time::Duration;

use gear_board::frame::{frame, Tab};
use gear_board::{run_with, Event, Keys, Options};
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
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                "w1:p1",
                "codex",
                NOW,
            )?;
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
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, departure + 44 * 60)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, departure + 44 * 60)?;
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
            // The work time of the round (T-348): the board reads the minutes
            // the lane worked, not the clock since the delivery.
            tx.set_round_active(&key, 44 * 60)?;
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

/// T-335: the board carries the voyage's confirmed goal changes apart from
/// the decisions list, so the S4 header can draw them red. A goal change is a
/// decision of kind `user` with source `gear` (T-410): another kind, or the
/// goal change of another voyage, is not one.
#[test]
fn the_goal_changes_are_the_gear_user_decisions_of_the_measured_voyage() {
    let mut store = world();
    let departure = now_seconds() - 7200;
    store
        .write(|tx| {
            // An older voyage with a goal change of its own: not this board's.
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M1".into(),
                title: "the first voyage".into(),
                done: Some("it is done".into()),
                bar: None,
                eta: Some("8h".into()),
                departure: Some(departure),
                departure_tasks: Some(1),
                departure_estimate_min: Some(40),
                updated: departure,
            })?;
            // Two goal changes of the measured voyage, in the order they
            // appeared, and a decision of another kind beside them.
            for (key, at, what) in [
                (
                    "g1",
                    "2026-10-27 12:30",
                    "done of M4 changed from \"old done\" to \"new done\"",
                ),
                (
                    "g2",
                    "2026-10-27 12:40",
                    "eta of M4 changed from \"8h\" to \"10h\"",
                ),
            ] {
                tx.insert_decision_if_new(&NewDecision {
                    project: "p".into(),
                    key: key.into(),
                    voyage: "M4".into(),
                    at: at.into(),
                    kind: "user".into(),
                    what: what.into(),
                    why: Some("confirmed by the user".into()),
                    alt: None,
                    changes: None,
                    star: true,
                    source: "gear".into(),
                    line: None,
                    plan_version: None,
                })?;
            }
            // A gear decision of another kind is not a goal change.
            tx.insert_decision_if_new(&NewDecision {
                project: "p".into(),
                key: "g3".into(),
                voyage: "M4".into(),
                at: "2026-10-27 12:50".into(),
                kind: "cut".into(),
                what: "the light procedure moved to M5".into(),
                why: None,
                alt: None,
                changes: None,
                star: false,
                source: "gear".into(),
                line: None,
                plan_version: None,
            })?;
            // And the goal change of the older voyage.
            tx.insert_decision_if_new(&NewDecision {
                project: "p".into(),
                key: "g4".into(),
                voyage: "M1".into(),
                at: "2026-10-26 09:00".into(),
                kind: "user".into(),
                what: "eta of M1 changed from \"6h\" to \"8h\"".into(),
                why: Some("confirmed by the user".into()),
                alt: None,
                changes: None,
                star: true,
                source: "gear".into(),
                line: None,
                plan_version: None,
            })
        })
        .unwrap();
    let board = board(&mut store);
    let changes = &board.projects[0].goal_changes;
    let whats: Vec<&str> = changes.iter().map(|change| change.what.as_str()).collect();
    assert_eq!(
        whats,
        vec![
            "done of M4 changed from \"old done\" to \"new done\"",
            "eta of M4 changed from \"8h\" to \"10h\"",
        ],
        "the gear user decisions of the measured voyage, oldest first"
    );
    assert_eq!(changes[0].at, "2026-10-27 12:30");
    assert_eq!(changes[0].why.as_deref(), Some("confirmed by the user"));
}

#[test]
fn the_log_shows_what_changed_the_course_then_every_decision_and_the_frictions() {
    let mut store = world();
    let board = board(&mut store);
    let text = frame(&board, &crate::screen(Tab::Log, 0, (120, 40)), None);

    // README change 10: what changed the course comes first, the starred
    // decisions only — the summary the user reads at a glance.
    assert!(text.contains("WHAT CHANGED THE COURSE  (★)"), "{text}");
    assert!(
        text.contains("12:05 ★ light procedure moved to M5"),
        "the starred decision in the summary: {text}"
    );
    // Then every decision, the starred ones marked, the reason under its own
    // decision.
    assert!(text.contains("ALL DECISIONS (2)"), "the whole log: {text}");
    assert!(
        text.contains("11:40   scope"),
        "an unstarred decision is in the log too: {text}"
    );
    assert!(text.contains("PR-mode landing added as T-152"), "{text}");
    assert!(
        text.contains("why: needs token data from T-141 first"),
        "the reason of the decision: {text}"
    );
    assert!(
        !text.contains("by kind"),
        "the count of the kinds is gone: {text}"
    );
    // The friction reports, open and closed: unread and read.
    assert!(text.contains("FRICTION REPORTS FROM AGENTS"), "{text}");
    let open = text
        .lines()
        .find(|line| line.contains("the two-repo fixture had no gate script"))
        .unwrap_or_else(|| panic!("an open report: {text}"));
    assert!(
        open.contains("unread"),
        "an open report is unread: {open:?}"
    );
    let closed = text
        .lines()
        .find(|line| line.contains("scripts/test.sh ran 4 min"))
        .unwrap_or_else(|| panic!("a closed report: {text}"));
    assert!(
        closed.contains("read") && !closed.contains("unread"),
        "a closed report is read: {closed:?}"
    );
}

#[test]
fn a_log_without_a_starred_decision_reads_nothing() {
    let mut store = Store::open_in_memory().unwrap();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one seat model".into(),
                done: Some("a rework resumes its session".into()),
                bar: None,
                eta: Some("12h".into()),
                departure: Some(departure),
                departure_tasks: Some(1),
                departure_estimate_min: Some(40),
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
                state: TaskState::Ready,
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
            // A decision that changed nothing of the course.
            tx.insert_decision_if_new(&NewDecision {
                project: "p".into(),
                key: "d1".into(),
                voyage: "M4".into(),
                at: "2026-10-27 11:40".into(),
                kind: "scope".into(),
                what: "PR-mode landing added as T-152".into(),
                why: None,
                alt: None,
                changes: None,
                star: false,
                source: "plan".into(),
                line: None,
                plan_version: None,
            })?;
            Ok(())
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, &crate::screen(Tab::Log, 0, (120, 40)), None);

    let lines: Vec<&str> = text.lines().collect();
    let summary = lines
        .iter()
        .position(|line| line.contains("WHAT CHANGED THE COURSE"))
        .unwrap_or_else(|| panic!("the summary: {text}"));
    assert_eq!(
        lines[summary + 1].trim(),
        "nothing",
        "the empty summary: {text}"
    );
    assert!(
        text.contains("ALL DECISIONS (1)"),
        "the log still lists it: {text}"
    );
}

#[test]
fn the_cost_view_shows_a_lane_with_the_development_and_the_review_apart() {
    let mut store = world();
    let board = board(&mut store);
    let text = frame(&board, &crate::screen(Tab::Cost, 0, (120, 40)), None);

    assert!(text.contains("lane / task"), "the columns: {text}");
    assert!(text.contains("dev tokens"), "the columns: {text}");
    assert!(text.contains("review tokens"), "the columns: {text}");
    // The row of the lane: one lane of the view, closed.
    let row = text
        .lines()
        .find(|line| line.contains("hull"))
        .expect("the row of the lane");
    assert!(row.contains("▸ hull"), "the lane is closed: {row}");
    assert!(
        row.contains("   40m    44m"),
        "the estimate and the actual time: {row}"
    );
    assert!(row.contains("  1.1x"), "the ratio: {row}");
    assert!(row.contains("512k"), "the development tokens: {row}");
    assert!(row.contains("    0.31"), "the development cost: {row}");
    // The task has no review round: nothing counts for it, and the total says
    // that part is unknown.
    assert!(row.contains("0k"), "no review round counted tokens: {row}");
    assert!(
        text.contains("this voyage: 0.5M tokens (development 0.5M, review 0.0M), $0.31"),
        "{text}"
    );
    assert!(text.contains("(known so far; 1 unknown)"), "{text}");
}

#[test]
fn enter_opens_a_lane_and_closes_it() {
    let mut store = world();
    let opts = Options {
        project: Some("p".into()),
        color: false,
        refresh: Duration::from_millis(1),
        ..Options::default()
    };

    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &opts,
        &mut out,
        &mut Presses(vec!['3', '\r']),
        None,
    )
    .unwrap();
    let frame = last_frame(&out);
    // The selected lane is open and its task is under it.
    assert!(frame.contains("▾ hull"), "Enter opens the lane: {frame}");
    assert!(
        frame.contains("T-1"),
        "the lane's task is under it: {frame}"
    );

    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &opts,
        &mut out,
        &mut Presses(vec!['3', '\r', '\r']),
        None,
    )
    .unwrap();
    let frame = last_frame(&out);
    // A second Enter closes it again.
    assert!(frame.contains("▸ hull"), "Enter closes the lane: {frame}");
    assert!(!frame.contains("T-1"), "the lane's task is hidden: {frame}");
}

/// T-363: the development round and the review round of a task are counted
/// apart, on the lane's row and on the task's own.
#[test]
fn a_task_shows_its_development_and_its_review_tokens() {
    let mut store = world();
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            // The development round of T-1: 1 000 000 fresh tokens.
            let dev = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
            };
            tx.set_round_tokens(
                &dev,
                &RoundTokens {
                    input: Some(1_000_000),
                    output: Some(0),
                    cache_read: Some(0),
                    cache_write: Some(0),
                    reasoning: None,
                    cost_usd: Some(0.4),
                    cost_source: "agent".into(),
                },
                departure + 44 * 60,
            )?;
            // And its review round: 200 000.
            let review = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "review".into(),
            };
            tx.insert_round(&gear_store::NewRound {
                key: review.clone(),
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: departure + 44 * 60,
            })?;
            tx.set_round_tokens(
                &review,
                &RoundTokens {
                    input: Some(200_000),
                    output: Some(0),
                    cache_read: Some(0),
                    cache_write: Some(0),
                    reasoning: None,
                    cost_usd: Some(0.1),
                    cost_source: "agent".into(),
                },
                departure + 50 * 60,
            )
        })
        .unwrap();

    let board = board(&mut store);
    let text = frame(&board, &crate::screen(Tab::Cost, 0, (120, 40)), None);
    let row = text
        .lines()
        .find(|line| line.contains("hull"))
        .expect("the row of the lane");
    assert!(row.contains("1000k"), "the development tokens: {row}");
    assert!(row.contains("200k"), "the review tokens: {row}");

    // The task's own row, under the opened lane, has its two sides as well.
    let mut screen = crate::screen(Tab::Cost, 0, (120, 40));
    screen.opened = vec!["hull".into()];
    let text = frame(&board, &screen, None);
    let row = text
        .lines()
        .find(|line| line.contains("T-1"))
        .expect("the row of the task");
    assert!(
        row.contains("1000k"),
        "the task's development tokens: {row}"
    );
    assert!(row.contains("200k"), "the task's review tokens: {row}");
}

/// F2: the reason of a decision stands on its own line, so it is readable on
/// an ordinary terminal.
#[test]
fn the_reason_of_a_decision_is_readable_on_eighty_columns() {
    let mut store = world();
    let board = board(&mut store);
    let text = frame(&board, &crate::screen(Tab::Log, 0, (80, 40)), None);

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
    let text = frame(&board, &crate::screen(Tab::Log, 0, (120, 40)), None);

    assert!(
        text.contains("light procedure moved to M5"),
        "the decisions of the active voyage: {text}"
    );
}

/// F4: the rounds of a task are the rows of the rounds table: a dev round and
/// a review round are two rounds of one attempt. The Cost view counts the
/// attempt once (its `rounds`); the rounds table's count stays a model fact.
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

    let task = board.projects[0]
        .lanes
        .iter()
        .flat_map(|lane| lane.tasks.iter())
        .find(|task| task.id == "T-1")
        .expect("the task T-1");
    assert_eq!(task.rounds, 1, "one attempt");
    assert_eq!(task.stored_rounds, 2, "a dev round and a review round");
}

/// F5: a part the rounds never counted is unknown, not a measured zero. The
/// view counts it as zero in its sums and names it once in the voyage's line,
/// so a sum is never shown as a total that hides an unknown part.
#[test]
fn an_unknown_usage_counts_as_zero_and_the_total_names_it() {
    let mut store = world();
    // The dev round of T-1 counted nothing (a measured zero) and was never
    // priced; there is no review round, so the review side is unknown.
    let departure = now_seconds() - 3600;
    store
        .write(|tx| {
            let key = RoundKey {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
            };
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
                departure + 44 * 60,
            )
        })
        .unwrap();
    let board = board(&mut store);
    let text = frame(&board, &crate::screen(Tab::Cost, 0, (120, 40)), None);

    let row = text
        .lines()
        .find(|line| line.contains("hull"))
        .expect("the row of the lane");
    assert!(row.contains("0k"), "a measured zero is a zero: {row}");
    assert!(!row.contains('?'), "no cell is drawn as unknown: {row}");
    // No round was priced: the total names the missing prices, not a dollar
    // sum, and the unknown review side is named.
    assert!(text.contains("no prices set"), "{text}");
    assert!(text.contains("(known so far; 1 unknown)"), "{text}");
}

/// F5 of the T-166 review: a sum that hides an unknown part is not a total.
/// A measured dev round plus a review round that counted nothing leaves the
/// development sum whole, counts the review side as unknown, and says so.
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
    let text = frame(&board, &crate::screen(Tab::Cost, 0, (120, 40)), None);

    let row = text
        .lines()
        .find(|line| line.contains("hull"))
        .expect("the row of the lane");
    assert!(row.contains("512k"), "the development sum is whole: {row}");
    assert!(
        text.contains("(known so far; 1 unknown)"),
        "the total names what it does not know: {text}"
    );
}

/// F5 of the T-166 review: a review round that counted its tokens but was
/// never priced leaves the tokens whole and the price unknown. The total sums
/// the prices it knows and says it is known so far.
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
    let text = frame(&board, &crate::screen(Tab::Cost, 0, (120, 40)), None);

    let row = text
        .lines()
        .find(|line| line.contains("hull"))
        .expect("the row of the lane");
    assert!(row.contains("22k"), "the review tokens are whole: {row}");
    assert!(text.contains("$0.31"), "the price that is known: {text}");
    assert!(
        text.contains("(known so far; 1 unknown)"),
        "the total names the price it does not know: {text}"
    );
}

/// The user, 2026-10-28: a task that re-reads its context for twenty minutes
/// shows millions of cache reads. They are not added to the fresh tokens the
/// Cost view counts.
#[test]
fn the_cache_reads_are_not_added_to_the_fresh_tokens() {
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
    let text = frame(&board, &crate::screen(Tab::Cost, 0, (120, 40)), None);

    let row = text
        .lines()
        .find(|line| line.contains("hull"))
        .expect("the row of the lane");
    assert!(row.contains("1k"), "the fresh tokens: {row}");
    assert!(
        !row.contains("2001k"),
        "the cache reads are not added to the fresh tokens: {row}"
    );
}

/// The user, 2026-10-28: a project without `[prices]` priced no round, so the
/// Cost view sums no dollar at all; the total names the missing prices once.
#[test]
fn a_view_without_prices_names_the_missing_prices_once() {
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
    let text = frame(&board, &crate::screen(Tab::Cost, 0, (120, 40)), None);

    let row = text
        .lines()
        .find(|line| line.contains("hull"))
        .expect("the row of the lane");
    assert!(!row.contains('?'), "nothing is drawn as unknown: {row}");
    let total = text
        .lines()
        .find(|line| line.contains("this voyage:"))
        .expect("the total line");
    assert!(
        total.contains("no prices set"),
        "the total names the missing prices: {total}"
    );
    assert_eq!(
        total.matches("no prices set").count(),
        1,
        "said once: {total}"
    );
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

/// T-263: the friction reports, like the decisions beside them on the same
/// view, are the measured voyage's only — a report of an older voyage is
/// history, not something the captain reads under this voyage.
fn two_voyages_with_frictions(old_open: bool) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let departure = now_seconds() - 7200;
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            for (milestone, at) in [("M4", departure), ("M5", departure + 3600)] {
                tx.upsert_voyage(&gear_store::VoyageRow {
                    project: "p".into(),
                    milestone: milestone.into(),
                    title: format!("voyage {milestone}"),
                    done: Some("it is delivered".into()),
                    bar: None,
                    eta: Some("8h".into()),
                    departure: Some(at),
                    departure_tasks: Some(2),
                    departure_estimate_min: Some(60),
                    updated: at,
                })?;
            }
            // Each voyage: a task that is done and carries a report, and a
            // task that leaves the voyage open or finished.
            let m4_rest = if old_open {
                TaskState::Ready
            } else {
                TaskState::Done
            };
            let m5_rest = if old_open {
                TaskState::Done
            } else {
                TaskState::Ready
            };
            for (id, rank, milestone, with_friction) in [
                ("T-1", 1, "M4", true),
                ("T-2", 2, "M4", false),
                ("T-3", 3, "M5", true),
                ("T-4", 4, "M5", false),
            ] {
                let state = if with_friction {
                    TaskState::Done
                } else if milestone == "M4" {
                    m4_rest
                } else {
                    m5_rest
                };
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
                })?;
                if with_friction {
                    tx.insert_friction(&NewFriction {
                        project: "p".into(),
                        task: id.into(),
                        attempt: 1,
                        role: "dev".into(),
                        seat: "p:hull".into(),
                        body: format!("the friction of {milestone}"),
                        required: false,
                    })?;
                }
            }
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn the_frictions_are_the_ones_of_the_voyage_the_board_measures() {
    // M5 is the voyage with work left: only its report is on the board.
    let mut store = two_voyages_with_frictions(false);
    let measured_m5 = board(&mut store);
    let frictions = &measured_m5.projects[0].frictions;
    let tasks: Vec<&str> = frictions.iter().map(|f| f.task.as_str()).collect();
    assert_eq!(tasks, ["T-3"], "M5 measured: the frictions of M5");

    // And with M4 the voyage that has work left, only the M4 report.
    let mut store = two_voyages_with_frictions(true);
    let measured_m4 = board(&mut store);
    let frictions = &measured_m4.projects[0].frictions;
    let tasks: Vec<&str> = frictions.iter().map(|f| f.task.as_str()).collect();
    assert_eq!(tasks, ["T-1"], "M4 measured: the frictions of M4");
}

/// Keys the test presses, one per call; `q` after them.
struct Presses(Vec<char>);

impl Keys for Presses {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        if self.0.is_empty() {
            return Some(Event::Key('q'));
        }
        Some(Event::Key(self.0.remove(0)))
    }

    fn size(&self) -> (usize, usize) {
        (120, 40)
    }
}

/// The last frame the board drew: the frames are separated by the clear.
fn last_frame(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    text.rsplit("\u{1b}[?2026h\u{1b}[H")
        .next()
        .unwrap_or_default()
        .to_string()
}

/// T-342 (README change 13): a decision is picked, its reason is not. `Down`
/// on the log goes from one decision to the next, over the why line between
/// them.
#[test]
fn down_moves_over_the_reason_of_a_decision() {
    let mut store = world();
    // A second starred decision with a reason of its own: both are picks.
    store
        .write(|tx| {
            tx.insert_decision_if_new(&NewDecision {
                project: "p".into(),
                key: "d3".into(),
                voyage: "M4".into(),
                at: "2026-10-27 12:20".into(),
                kind: "scope".into(),
                what: "PR-mode landing was added as T-152".into(),
                why: Some("the user asked for it at the kickoff".into()),
                alt: None,
                changes: None,
                star: true,
                source: "plan".into(),
                line: None,
                plan_version: None,
            })?;
            Ok(())
        })
        .unwrap();
    let opts = Options {
        project: Some("p".into()),
        color: false,
        refresh: Duration::from_millis(1),
        ..Options::default()
    };
    let mut out: Vec<u8> = Vec::new();
    let mut keys = Presses(vec!['4', 'j', 'j']);

    run_with(&mut store, &opts, &mut out, &mut keys, None).unwrap();

    let frame = last_frame(&out);
    // The pick's mark sits in column 1, as the prototype draws it.
    let marked: Vec<&str> = frame.lines().filter(|line| line.contains('›')).collect();
    assert_eq!(marked.len(), 1, "one row is picked: {frame}");
    assert!(
        marked[0].contains("★ scope") && marked[0].contains("12:20"),
        "the third decision is picked, not the why line before it: {frame}"
    );
    assert!(
        !frame
            .lines()
            .any(|line| line.contains('›') && line.contains("why:")),
        "a reason is not a pick: {frame}"
    );
    // Both reasons are still drawn, under their decisions.
    assert!(
        frame.contains("why: needs token data from T-141 first"),
        "{frame}"
    );
    assert!(
        frame.contains("why: the user asked for it at the kickoff"),
        "{frame}"
    );
}
