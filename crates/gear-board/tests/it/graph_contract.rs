//! T-157: the graph view of the board (design §8.1, the prototype of
//! `gear/voyages/M4/board`).
//!
//! The fixture is the prototype's fake data: the tasks of the lanes, their
//! dependencies, the rounds and the merges.

use gear_board::graph::{lines, panel, tasks};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, NewRound, RoundKey, Store, TaskRow};

const T0: i64 = 1_793_016_000; // 2026-10-26T12:00:00Z

/// The epoch second of a time of the day (the fixture's times).
fn moment(hour: i64, minute: i64) -> i64 {
    T0 + (hour - 12) * 3600 + minute * 60
}

/// One task of the fixture: the lane, the stage, the dependencies, the
/// estimate, the minutes it took, the rounds and when it was merged.
struct Task {
    id: &'static str,
    lane: &'static str,
    state: TaskState,
    deps: Vec<&'static str>,
    est: i64,
    actual: i64,
    rounds: i64,
    merged: Option<i64>,
    title: &'static str,
}

/// The prototype's fake data, in a store.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            for lane in ["hull", "deck", "keel", "mast"] {
                testing::seed_active_seat(tx, "p", lane, SeatRole::Lane, "w1:p2", "pi", T0)?;
            }
            Ok(())
        })
        .unwrap();
    let tasks = vec![
        Task {
            id: "T-135",
            lane: "hull",
            state: TaskState::Done,
            deps: vec![],
            est: 40,
            actual: 44,
            rounds: 1,
            merged: Some(moment(11, 2)),
            title: "store: task keeps its repo",
        },
        Task {
            id: "T-136",
            lane: "deck",
            state: TaskState::Done,
            deps: vec![],
            est: 45,
            actual: 50,
            rounds: 1,
            merged: Some(moment(11, 10)),
            title: "profiles: session commands",
        },
        Task {
            id: "T-139",
            lane: "mast",
            state: TaskState::Done,
            deps: vec!["T-135"],
            est: 45,
            actual: 71,
            rounds: 2,
            merged: Some(moment(12, 44)),
            title: "rebind a lane to another repo",
        },
        Task {
            id: "T-141",
            lane: "deck",
            state: TaskState::Verifying,
            deps: vec!["T-136"],
            est: 40,
            actual: 39,
            rounds: 1,
            merged: None,
            title: "token count per round",
        },
        Task {
            id: "T-142",
            lane: "hull",
            state: TaskState::Verifying,
            deps: vec![],
            est: 30,
            actual: 26,
            rounds: 2,
            merged: None,
            title: "auto check-in",
        },
        Task {
            id: "T-143",
            lane: "hull",
            state: TaskState::Running,
            deps: vec!["T-136"],
            est: 60,
            actual: 38,
            rounds: 1,
            merged: None,
            title: "one seat model",
        },
        Task {
            id: "T-144",
            lane: "deck",
            state: TaskState::Running,
            deps: vec![],
            est: 45,
            actual: 22,
            rounds: 1,
            merged: None,
            title: "quota from provider APIs",
        },
        Task {
            id: "T-148",
            lane: "hull",
            state: TaskState::Ready,
            deps: vec![],
            est: 35,
            actual: 0,
            rounds: 0,
            merged: None,
            title: "station projection per lane",
        },
        Task {
            id: "T-150",
            lane: "keel",
            state: TaskState::Ready,
            deps: vec!["T-142"],
            est: 40,
            actual: 0,
            rounds: 0,
            merged: None,
            title: "friction reports at hand-in",
        },
        Task {
            id: "T-151",
            lane: "deck",
            state: TaskState::Ready,
            deps: vec!["T-141"],
            est: 50,
            actual: 0,
            rounds: 0,
            merged: None,
            title: "board: graph view",
        },
    ];
    store
        .write(|tx| {
            for (rank, task) in tasks.iter().enumerate() {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: task.id.into(),
                    rank: rank as i64 + 1,
                    state: task.state,
                    priority: "must".into(),
                    lane: Some(task.lane.into()),
                    milestone: Some("M4".into()),
                    required: false,
                    contract_version: 1,
                    deps: task.deps.iter().map(|dep| dep.to_string()).collect(),
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(task.est),
                    issue: None,
                    updated: T0,
                })?;
                tx.insert_contract(&gear_store::ContractRow {
                    project: "p".into(),
                    task: task.id.into(),
                    version: 1,
                    goal: task.title.into(),
                    accept: String::new(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
                if task.id == "T-141" {
                    // The task was accepted: it waits for its merge.
                    tx.insert_verdict(&gear_store::VerdictRow {
                        project: "p".into(),
                        task: task.id.into(),
                        attempt: 1,
                        contract_version: 1,
                        candidate: "abc1234".into(),
                        passed: true,
                        evidence: None,
                        by_seat: "p:captain".into(),
                        created: T0,
                    })?;
                }
                for round in 1..=task.rounds {
                    let started = task.merged.unwrap_or(moment(12, 0)) - task.actual * 60;
                    tx.insert_attempt(&AttemptRow {
                        project: "p".into(),
                        task: task.id.into(),
                        n: round,
                        seat: format!("p:{}", task.lane),
                        generation: 1,
                        contract_version: 1,
                        adopted_version: 1,
                        pending_update: None,
                        state: AttemptState::Accepted,
                        candidate: None,
                        report: None,
                        safe_point: None,
                        started,
                        updated: started,
                        session_ref: None,
                        session_cli: None,
                        session_action: None,
                        sent_at: None,
                    })?;
                    tx.set_attempt_time("p", task.id, round, gear_store::AttemptStep::Started, started)?;
                    tx.set_attempt_time(
                        "p",
                        task.id,
                        round,
                        gear_store::AttemptStep::HandedIn,
                        started + task.actual * 60,
                    )?;
                    if let Some(merged) = task.merged {
                        tx.set_attempt_time("p", task.id, round, gear_store::AttemptStep::Merged, merged)?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn the_graph_has_the_columns_of_the_prototype() {
    let mut store = world();
    let (tasks, lanes) = store
        .read(|tx| Ok((tasks(tx, "p", T0)?, gear_board::graph::lanes(tx, "p")?)))
        .unwrap();
    assert_eq!(lanes, vec!["deck", "hull", "keel", "mast"]);
    // The order of the prototype: the queued first, then the work, then the
    // reviews, then the merged ones with the newest first.
    let ids: Vec<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "T-148", "T-150", "T-151", "T-143", "T-144", "T-141", "T-142", "T-139", "T-136", "T-135"
        ]
        // T-141 is being merged and T-142 waits for its verdict: they stand
        // next to each other, in the order of the plan (review F2).
    );

    let lines = lines(&tasks, &lanes, None);
    // The head of the columns: dev, the review column, one column per lane.
    let head = &lines[0];
    assert_eq!(at(head, 2, 3), "dev", "{head:?}");
    assert_eq!(at(head, 8, 3), "rev", "{head:?}");
    assert_eq!(at(head, 14, 4), "deck", "{head:?}");
    assert_eq!(at(head, 20, 4), "hull", "{head:?}");
    assert_eq!(at(head, 26, 4), "keel", "{head:?}");
    assert_eq!(at(head, 32, 4), "mast", "{head:?}");

    // The queued row of T-148 (the hull's column is the first one).
    let row = line_of(&lines, "T-148");
    assert_eq!(at(row, 3, 1), "│", "{row:?}");
    assert_eq!(at(row, 21, 1), "○", "{row:?}");
    assert!(row.contains("queued · 35m"), "{row:?}");
    assert!(row.contains("T-148"), "{row:?}");
    assert!(row.contains("station projection per lane"), "{row:?}");
    // The legend at the end.
    let legend = lines.last().unwrap();
    assert!(legend.contains("○ queued"), "{legend:?}");
    assert!(legend.contains("↺ reworked"), "{legend:?}");
}

#[test]
fn a_merged_task_draws_the_line_of_the_merge() {
    let mut store = world();
    let (tasks, lanes) = store
        .read(|tx| Ok((tasks(tx, "p", T0)?, gear_board::graph::lanes(tx, "p")?)))
        .unwrap();
    let lines = lines(&tasks, &lanes, None);

    // T-135 was merged into dev: the merge line runs from dev to its lane.
    let row = line_of(&lines, "T-135");
    assert_eq!(at(row, 3, 1), "●", "{row:?}");
    // The review column's ✓ stands on the line of the merge.
    assert_eq!(at(row, 4, 17), "─────✓───────────", "{row:?}");
    assert_eq!(at(row, 21, 1), "●", "{row:?}");
    // The review column says that it was reviewed.
    assert_eq!(at(row, 9, 1), "✓", "{row:?}");
    // The info column: when it was merged (the clock of the user) and how
    // long it took.
    assert!(row.contains(" · 44/40m"), "{row:?}");
    assert!(row.contains("merged "), "{row:?}");

    // T-139 was reworked: the rework mark is in the review column.
    let row = line_of(&lines, "T-139");
    assert_eq!(at(row, 8, 1), "↺", "{row:?}");
    assert!(row.contains("2 rounds"), "{row:?}");
}

#[test]
fn a_task_in_review_draws_the_line_of_the_review() {
    let mut store = world();
    let (tasks, lanes) = store
        .read(|tx| Ok((tasks(tx, "p", T0)?, gear_board::graph::lanes(tx, "p")?)))
        .unwrap();
    let lines = lines(&tasks, &lanes, None);

    // T-142 waits for its verdict: the review column reaches its lane
    // (hull, the second column).
    let row = line_of(&lines, "T-142");
    assert_eq!(at(row, 9, 1), "◎", "{row:?}");
    // The deck's lane line crosses the line of the review.
    assert_eq!(at(row, 10, 11), "─────│─────", "{row:?}");
    assert_eq!(at(row, 21, 1), "◎", "{row:?}");
    assert!(row.contains("in review"), "{row:?}");

    // T-141 was accepted and waits for its merge: the same line, and the info
    // column says so.
    let row = line_of(&lines, "T-141");
    assert_eq!(at(row, 9, 1), "◎", "{row:?}");
    assert_eq!(at(row, 10, 5), "─────", "{row:?}");
    assert_eq!(at(row, 15, 1), "◎", "{row:?}");
    assert!(row.contains("merging"), "{row:?}");
}

#[test]
fn a_dependency_is_marked_as_needed() {
    let mut store = world();
    let (tasks, lanes) = store
        .read(|tx| Ok((tasks(tx, "p", T0)?, gear_board::graph::lanes(tx, "p")?)))
        .unwrap();

    // Without a selection, the row of a task with dependencies says what it
    // needs.
    let plain = lines(&tasks, &lanes, None);
    let row = line_of(&plain, "T-150");
    assert!(row.contains("needs T-142"), "{row:?}");

    // With T-141 selected (it needs T-136), the dependency lights up.
    let selected = tasks.iter().position(|task| task.id == "T-141").unwrap();
    let marked = lines(&tasks, &lanes, Some(selected));
    let row = line_of(&marked, "T-136");
    assert!(row.contains("◂ needed by T-141"), "{row:?}");
    // The task that waits for the selected one says so.
    let row = line_of(&marked, "T-151");
    assert!(row.contains("▸ waits for T-141"), "{row:?}");
}

#[test]
fn the_task_panel_lists_the_rounds_and_the_dependencies() {
    let mut store = world();
    let panel = store
        .read(|tx| panel(tx, "p", "T-139", T0))
        .unwrap();
    let text = panel.join("\n");
    assert!(text.contains("T-139 · lane mast · done"), "{text}");
    assert!(text.contains("rebind a lane to another repo"), "{text}");
    assert!(text.contains("estimate 45m · actual 71m"), "{text}");
    // The rounds of the task: the dev round and the rework round.
    assert!(text.contains("dev 1"), "{text}");
    assert!(text.contains("rework 2"), "{text}");
    // What it depends on, and what needs it.
    assert!(text.contains("DEPENDS ON"), "{text}");
    assert!(text.contains("T-135 hull"), "{text}");
    assert!(text.contains("NEEDED BY"), "{text}");
}

#[test]
fn the_task_panel_uses_the_rounds_of_the_store_when_there_are_any() {
    let mut store = world();
    store
        .write(|tx| {
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-141".into(),
                    attempt: 1,
                    role: "dev".into(),
                },
                seat: "p:deck".into(),
                generation: 1,
                cli: "pi".into(),
                model: Some("MiniMax-M3.1-Flash".into()),
                start_at: moment(12, 0),
            })?;
            tx.set_round_active(
                &RoundKey {
                    project: "p".into(),
                    task: "T-141".into(),
                    attempt: 1,
                    role: "dev".into(),
                },
                39 * 60,
            )?;
            tx.set_round_tokens(
                &RoundKey {
                    project: "p".into(),
                    task: "T-141".into(),
                    attempt: 1,
                    role: "dev".into(),
                },
                &gear_store::RoundTokens {
                    input: Some(400_000),
                    output: Some(100_000),
                    cache_read: Some(300_000),
                    cache_write: Some(20_000),
                    reasoning: Some(5),
                    cost_usd: Some(0.27),
                    cost_source: "agent".into(),
                },
                moment(12, 30),
            )
        })
        .unwrap();
    let panel = store
        .read(|tx| panel(tx, "p", "T-141", T0))
        .unwrap();
    let text = panel.join("\n");
    // The round of the store, with its time and its tokens.
    assert!(text.contains("dev   attempt 1"), "{text}");
    assert!(text.contains("39m"), "{text}");
    assert!(text.contains("820k tokens"), "{text}");
}

/// The characters of a line between two columns (the box drawing signs are
/// more than one byte).
fn at(row: &str, from: usize, len: usize) -> String {
    row.chars().skip(from).take(len).collect()
}

/// The line of a task in the graph: the row whose id stands in the text
/// column (a row may name other tasks in its marks).
fn line_of<'a>(lines: &'a [String], task: &str) -> &'a str {
    let needle = format!(" {task} ");
    lines
        .iter()
        .find(|line| line.contains(&needle))
        .map(String::as_str)
        .unwrap_or_else(|| panic!("no line for {task}"))
}

#[test]
fn a_stored_review_round_shows_how_the_review_ended() {
    // The review of T-141 ended with a pass.
    let mut store = world();
    review_round(&mut store, "T-141", Some(true));
    let text = store
        .read(|tx| panel(tx, "p", "T-141", T0))
        .unwrap()
        .join("\n");
    assert!(text.contains("review attempt 1"), "{text}");
    assert!(text.contains("→ pass"), "{text}");

    // The review came back: the lane works on it again.
    let mut store = world();
    review_round(&mut store, "T-141", Some(false));
    let text = store
        .read(|tx| panel(tx, "p", "T-141", T0))
        .unwrap()
        .join("\n");
    assert!(text.contains("→ fail: rework"), "{text}");

    // The review is still running: there is no verdict yet.
    let mut store = world();
    review_round(&mut store, "T-141", None);
    let text = store
        .read(|tx| panel(tx, "p", "T-141", T0))
        .unwrap()
        .join("\n");
    assert!(text.contains("(running)"), "{text}");
}

/// A review round of a task in the store, with the verdict of the review (None
/// while it is still running).
fn review_round(store: &mut Store, task: &str, passed: Option<bool>) {
    let key = RoundKey {
        project: "p".into(),
        task: task.into(),
        attempt: 1,
        role: "review".into(),
    };
    store
        .write(|tx| {
            tx.insert_round(&NewRound {
                key: key.clone(),
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: Some("gpt-6.1-sol".into()),
                start_at: moment(12, 10),
            })?;
            tx.set_round_active(&key, 7 * 60)?;
            if let Some(passed) = passed {
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: task.into(),
                    attempt: 1,
                    passed,
                    findings: String::new(),
                    by_seat: "p:review-1".into(),
                    created: moment(12, 20),
                })?;
            }
            Ok(())
        })
        .unwrap();
}
