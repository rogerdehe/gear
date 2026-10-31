//! T-157: the graph view of the board (design §8.1, the prototype of
//! `gear/voyages/M4/board`).
//!
//! The fixture is the prototype's fake data: the tasks of the lanes, their
//! dependencies, the rounds and the merges.

use gear_board::frame::lane_panel;
use gear_board::graph::{git_rows, lines, panel, tasks, GraphRow, GraphTask, Stage};
use gear_board::style::{lane_color, Color, Style};
use gear_model::{AttemptState, MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, NewMessage, NewOp, NewRound, RoundKey, Store, TaskRow};

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
                    // The attempt says what the task does: a running one is
                    // started, a task that waits for its verdict is handed
                    // in, and an accepted one passed.
                    let state = match task.state {
                        TaskState::Running => AttemptState::Started,
                        TaskState::Verifying if task.id == "T-141" => AttemptState::Accepted,
                        TaskState::Verifying => AttemptState::Submitted,
                        _ => AttemptState::Accepted,
                    };
                    tx.insert_attempt(&AttemptRow {
                        project: "p".into(),
                        task: task.id.into(),
                        n: round,
                        seat: format!("p:{}", task.lane),
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
                        session_ref: None,
                        session_cli: None,
                        session_action: None,
                        sent_at: None,
                    })?;
                    tx.set_attempt_time(
                        "p",
                        task.id,
                        round,
                        gear_store::AttemptStep::Started,
                        started,
                    )?;
                    tx.set_attempt_time(
                        "p",
                        task.id,
                        round,
                        gear_store::AttemptStep::HandedIn,
                        started + task.actual * 60,
                    )?;
                    if let Some(merged) = task.merged {
                        tx.set_attempt_time(
                            "p",
                            task.id,
                            round,
                            gear_store::AttemptStep::Merged,
                            merged,
                        )?;
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
            "T-148", "T-150", "T-151", "T-141", "T-142", "T-143", "T-144", "T-139", "T-136",
            "T-135"
        ] // T-141 passed and T-142 waits for a reviewer: the open tasks follow
          // the queued ones, in the order of the plan.
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
    assert!(legend.contains("○ not started"), "{legend:?}");
    assert!(legend.contains("◌ delivered"), "{legend:?}");
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

    // T-142 waits for its reviewer: the review column reaches its lane
    // (hull, the second column).
    let row = line_of(&lines, "T-142");
    assert_eq!(at(row, 9, 1), "◎", "{row:?}");
    // The deck's lane line crosses the line of the review.
    assert_eq!(at(row, 10, 11), "─────│─────", "{row:?}");
    assert_eq!(at(row, 21, 1), "◎", "{row:?}");
    assert!(row.contains("handed in, waits for a reviewer"), "{row:?}");

    // T-141 passed and waits to merge: the same line, and the info column
    // says so.
    let row = line_of(&lines, "T-141");
    assert_eq!(at(row, 9, 1), "◍", "{row:?}");
    assert_eq!(at(row, 10, 5), "─────", "{row:?}");
    assert_eq!(at(row, 15, 1), "◍", "{row:?}");
    assert!(row.contains("passed, waits to merge"), "{row:?}");
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
    let panel = store.read(|tx| panel(tx, "p", "T-139", T0)).unwrap();
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
    let panel = store.read(|tx| panel(tx, "p", "T-141", T0)).unwrap();
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

/// The line of a task in the graph: the row whose id stands in the id column
/// (a row may name other tasks in its info and in its marks).
fn line_of<'a>(lines: &'a [String], task: &str) -> &'a str {
    let needle = format!(" {task} ");
    lines
        .iter()
        .find(|line| {
            // The column of the id, in characters (the box signs are more
            // than one byte).
            line.match_indices(&needle)
                .any(|(byte, _)| line[..byte].chars().count() < 60)
        })
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

// ---------------------------------------------------------------- T-346: the
// sixteen stages of a task, from the facts the store already has.

/// The store of the stage rules: one task per stage, built once.
///
/// T-1 is a dependency that is not merged yet; T-20 waits for it. T-21 works,
/// T-22 asked a question, T-23 was delivered and not started. T-24 was sent
/// back and T-25 reworks on attempt 2. T-26's merge runs, T-27's merge failed
/// and T-28 passed. T-29 waits for a reviewer, T-30 is in review and T-31
/// waits for the captain's verdict. T-32 is merged.
fn stage_world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p0", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p1", "pi", T0)?;
            let tasks: Vec<(&str, TaskState, Vec<&str>)> = vec![
                ("T-1", TaskState::Ready, vec![]),
                ("T-20", TaskState::Ready, vec!["T-1"]),
                ("T-21", TaskState::Running, vec![]),
                ("T-22", TaskState::Running, vec![]),
                ("T-23", TaskState::Running, vec![]),
                ("T-24", TaskState::Running, vec![]),
                ("T-25", TaskState::Running, vec![]),
                ("T-26", TaskState::Verifying, vec![]),
                ("T-27", TaskState::Verifying, vec![]),
                ("T-28", TaskState::Verifying, vec![]),
                ("T-29", TaskState::Verifying, vec![]),
                ("T-30", TaskState::Verifying, vec![]),
                ("T-31", TaskState::Verifying, vec![]),
                ("T-32", TaskState::Done, vec![]),
            ];
            for (rank, (id, state, deps)) in tasks.iter().enumerate() {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: id.to_string(),
                    rank: rank as i64 + 1,
                    state: *state,
                    priority: "must".into(),
                    lane: Some("hull".into()),
                    milestone: Some("M5".into()),
                    required: false,
                    contract_version: 1,
                    deps: deps.iter().map(|dep| dep.to_string()).collect(),
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(20),
                    issue: None,
                    updated: T0,
                })?;
            }
            // The attempts of the tasks: the state of the newest one is the
            // fact the stage rules read.
            let attempts: Vec<(&str, i64, AttemptState)> = vec![
                ("T-21", 1, AttemptState::Started),
                ("T-22", 1, AttemptState::Started),
                ("T-23", 1, AttemptState::Delivered),
                ("T-24", 1, AttemptState::Rejected),
                ("T-25", 1, AttemptState::Rejected),
                ("T-25", 2, AttemptState::Started),
                ("T-26", 1, AttemptState::Accepted),
                ("T-27", 1, AttemptState::Accepted),
                ("T-28", 1, AttemptState::Accepted),
                ("T-29", 1, AttemptState::Submitted),
                ("T-30", 1, AttemptState::Submitted),
                ("T-31", 1, AttemptState::Submitted),
            ];
            for (id, n, state) in &attempts {
                let started = moment(12, 0);
                tx.insert_attempt(&AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: id.to_string(),
                    n: *n,
                    seat: "p:hull".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: *state,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started,
                    updated: started,
                })?;
                tx.set_attempt_time("p", id, *n, gear_store::AttemptStep::Started, started)?;
            }
            // The passed verdicts of the tasks that wait for their merge.
            for id in ["T-26", "T-27", "T-28"] {
                tx.insert_verdict(&gear_store::VerdictRow {
                    project: "p".into(),
                    task: id.into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: "abc1234".into(),
                    passed: true,
                    evidence: None,
                    by_seat: "p:captain".into(),
                    created: moment(12, 10),
                })?;
            }
            // The merge op of T-26 runs; the merge op of T-27 failed.
            for (id, state) in [("T-26", OpState::Executing), ("T-27", OpState::Failed)] {
                let op = tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:captain".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some(id.into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({}),
                    created: moment(12, 15),
                })?;
                tx.set_op_state("p", &op.id, state, None, moment(12, 16))?;
            }
            // The reviewer of T-30 holds the task; T-31's reviewer handed its
            // verdict in.
            testing::seed_temp_seat(tx, "p", "review-1", "T-30", "w1:p5", "codex", T0)?;
            tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                project: "p".into(),
                task: "T-31".into(),
                attempt: 1,
                passed: true,
                findings: String::new(),
                by_seat: "p:review-2".into(),
                created: moment(12, 20),
            })?;
            // T-22 asked the captain and waits for the answer.
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-22:1".into(),
                task: Some("T-22".into()),
                attempt: Some(1),
                sender: "p:hull".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Question,
                urgent: false,
                body: "which one?".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: moment(12, 5),
                meta: None,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

/// The stage and the words of a task of the stage fixture.
fn stage_and_words(store: &mut Store, task: &str) -> (Stage, String) {
    let (found, words) = store
        .read(|tx| {
            let tasks = tasks(tx, "p", T0)?;
            let found = tasks
                .iter()
                .find(|found| found.id == task)
                .map(|found| found.stage);
            let words = tasks
                .iter()
                .find(|found| found.id == task)
                .map(Stage::words);
            Ok((found, words))
        })
        .unwrap();
    (
        found.unwrap_or_else(|| panic!("no task {task}")),
        words.unwrap_or_default(),
    )
}

#[test]
fn each_stage_comes_from_a_fact_of_the_store() {
    let mut store = stage_world();
    let table: [(&str, Stage, &str); 14] = [
        ("T-20", Stage::Deps, "waits for 1 to merge"),
        ("T-1", Stage::Queued, "queued"),
        ("T-23", Stage::Delivered, "delivered, not started"),
        ("T-21", Stage::Working, "working"),
        ("T-22", Stage::Asking, "waits for the captain's answer"),
        ("T-29", Stage::Handed, "handed in, waits for a reviewer"),
        ("T-30", Stage::Review, "in review"),
        ("T-31", Stage::Verdict, "waits for the captain's verdict"),
        ("T-24", Stage::SentBack, "sent back, rework queued"),
        ("T-25", Stage::Rework, "reworking (round 2)"),
        ("T-28", Stage::Passed, "passed, waits to merge"),
        ("T-26", Stage::Merging, "merging (gate running)"),
        (
            "T-27",
            Stage::MergeFailed,
            "merge failed, waits for the captain",
        ),
        ("T-32", Stage::Merged, "merged"),
    ];
    for (task, stage, words) in table {
        let (found, said) = stage_and_words(&mut store, task);
        assert_eq!(found, stage, "{task}");
        assert_eq!(said, words, "{task}");
    }
}

/// The board's "to handle" lines of the stage fixture, as the Now view lists
/// them (T-360).
fn board_waiting(store: &mut Store) -> Vec<String> {
    gear_board::build(
        store,
        &gear_board::Options {
            project: Some("p".into()),
            ..Default::default()
        },
        moment(12, 30),
    )
    .unwrap()
    .projects[0]
        .waiting_on
        .clone()
}

/// T-460: an attempt whose review is in and waits for the captain is named
/// with the item that waits in his inbox (`waits for your verdict (M-<id>)`),
/// not only by its state: another project's captain read a finished review as
/// a stall. A verdict without an item keeps the plain words.
#[test]
fn a_verdict_that_waits_for_the_captain_names_the_item() {
    let mut store = stage_world();
    // No item waits yet: the plain words.
    assert!(
        board_waiting(&mut store).contains(&"T-31 waits for the captain's verdict".to_string()),
        "the plain words without an item"
    );
    // The reviewer's fail verdict waits in the captain's inbox: the item the
    // board has to name.
    store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "T-31:1".into(),
                task: Some("T-31".into()),
                attempt: Some(1),
                sender: "p:review-2".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Review,
                urgent: false,
                body: "review fail on T-31 attempt 1\nthe check misses".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: moment(12, 21),
            })
        })
        .unwrap();
    let waiting = board_waiting(&mut store);
    assert!(
        waiting.contains(&"T-31 waits for your verdict (M-2)".to_string()),
        "the item is named: {waiting:?}"
    );
}

/// F2 of the T-346 review: a newest attempt that was abandoned counts as no
/// attempt — it does not resurrect the older one.
#[test]
fn an_abandoned_newest_attempt_counts_as_no_attempt() {
    let mut store = stage_world();
    let mut abandoned = store
        .read(|tx| tx.attempts_of_task("p", "T-21"))
        .unwrap()
        .into_iter()
        .next()
        .expect("T-21 has its attempt");
    abandoned.n = 2;
    abandoned.state = AttemptState::Abandoned;
    store.write(|tx| tx.insert_attempt(&abandoned)).unwrap();
    let (stage, words) = stage_and_words(&mut store, "T-21");
    assert_eq!(stage, Stage::Queued, "the abandoned attempt is no attempt");
    assert_eq!(words, "queued");
}

/// F1 of the T-346 review: the lane panel shows each task in the stage the
/// graph gave it, not one of five words repeated (the prototype's
/// `lane_panel`).
#[test]
fn the_lane_panel_shows_each_task_in_its_stage() {
    let mut store = stage_world();
    let board = gear_board::build(
        &mut store,
        &gear_board::Options {
            project: Some("p".into()),
            ..Default::default()
        },
        T0,
    )
    .unwrap();
    let lane = board.projects[0]
        .lanes
        .iter()
        .find(|lane| lane.lane == "hull")
        .expect("the hull lane");
    let text = lane_panel(lane, &board.projects[0].graph).join("\n");
    let row = |task: &str| -> String {
        text.lines()
            .find(|line| line.contains(&format!(" {task} ")))
            .unwrap_or_else(|| panic!("no row for {task} in\n{text}"))
            .to_string()
    };
    // Handed and Review are told apart: the same `◎` but not the same words.
    assert!(row("T-29").starts_with("  ◎ T-29 "), "{text}");
    assert!(
        row("T-29").ends_with("handed in, waits for a reviewer"),
        "{text}"
    );
    assert!(row("T-30").starts_with("  ◎ T-30 "), "{text}");
    assert!(row("T-30").ends_with("in review"), "{text}");
    // A failed merge is not a merge that runs.
    assert!(row("T-27").starts_with("  ✗ T-27 "), "{text}");
    assert!(
        row("T-27").ends_with("merge failed, waits for the captain"),
        "{text}"
    );
    // The rework says which round it is, and a question is not work.
    assert!(row("T-25").starts_with("  ↺ T-25 "), "{text}");
    assert!(row("T-25").ends_with("reworking (round 2)"), "{text}");
    assert!(row("T-22").starts_with("  ◉ T-22 "), "{text}");
    assert!(
        row("T-22").ends_with("waits for the captain's answer"),
        "{text}"
    );
    // The queued work lists what it waits for.
    assert!(row("T-20").contains("needs T-1"), "{text}");
    assert!(text.contains("left: 2 tasks, 40m of estimates"), "{text}");
}

#[test]
fn the_sixteen_stages_have_their_symbols_colours_and_words() {
    let open = [
        Stage::Delivered,
        Stage::Working,
        Stage::Asking,
        Stage::Handed,
        Stage::Review,
        Stage::Verdict,
        Stage::SentBack,
        Stage::Rework,
        Stage::Passed,
        Stage::Merging,
        Stage::MergeFailed,
    ];
    let table = [
        (Stage::Deps, "○", Color::Dim),
        (Stage::Queued, "○", Color::Dim),
        (Stage::Delivered, "◌", Color::Dim),
        (Stage::Working, "◉", Color::Warn),
        (Stage::Asking, "◉", Color::Warn),
        (Stage::Handed, "◎", Color::Acc),
        (Stage::Review, "◎", Color::Acc),
        (Stage::Verdict, "◎", Color::Warn),
        (Stage::SentBack, "↺", Color::Bad),
        (Stage::Rework, "↺", Color::Warn),
        (Stage::Passed, "◍", Color::Acc),
        (Stage::Merging, "◍", Color::Acc),
        (Stage::MergeFailed, "✗", Color::Bad),
        (Stage::Merged, "●", Color::Ok),
        (Stage::Failed, "✗", Color::Bad),
        (Stage::Moved, "→", Color::Dim),
    ];
    for (stage, symbol, color) in table {
        assert_eq!(stage.symbol(), symbol, "{stage:?}");
        assert_eq!(stage.color(), color, "{stage:?}");
        assert_eq!(stage.is_open(), open.contains(&stage), "{stage:?}");
    }
    // The words of the two stages the store here cannot give: a failed task is
    // not in the flow and a moved one has another milestone.
    let failed = GraphTask {
        stage: Stage::Failed,
        moved_to: None,
        deps: Vec::new(),
        rounds: 3,
        ..sample()
    };
    assert_eq!(Stage::words(&failed), "failed (rounds used up)");
    let moved = GraphTask {
        stage: Stage::Moved,
        moved_to: Some("M6".into()),
        ..sample()
    };
    assert_eq!(Stage::words(&moved), "moved to M6");
}

/// A task of the fixture, for the words of a stage.
fn sample() -> GraphTask {
    GraphTask {
        id: "T-1".into(),
        rank: 1,
        lane: "hull".into(),
        stage: Stage::Queued,
        moved_to: None,
        title: "one step".into(),
        est_min: Some(20),
        actual_min: None,
        rounds: 0,
        deps: Vec::new(),
        finished: None,
        started: None,
    }
}

/// One task of the prototype's M5 voyage: the id, the lane, the stage, the
/// title, the estimate and the time it took, its rounds, what it depends on,
/// when its work started and when it merged, both in minutes from the
/// departure (the prototype's `WHEN`).
struct VoyageTask {
    id: &'static str,
    lane: &'static str,
    stage: Stage,
    title: &'static str,
    est: i64,
    actual: i64,
    rounds: usize,
    deps: &'static [&'static str],
    started: Option<i64>,
    merged: Option<i64>,
}

/// The prototype's M5 voyage (`gear/voyages/M5/board/prototype.py`), in its
/// plan order: the departure is 0, so every time is a minute from it.
fn m5() -> Vec<GraphTask> {
    let voyage = [
        VoyageTask {
            id: "T-257",
            lane: "hull",
            stage: Stage::Merged,
            title: "small task: the plan marks it",
            est: 20,
            actual: 18,
            rounds: 1,
            deps: &[],
            started: Some(5),
            merged: Some(31),
        },
        VoyageTask {
            id: "T-258",
            lane: "mast",
            stage: Stage::Merged,
            title: "board: voyages and the [ ] keys",
            est: 40,
            actual: 44,
            rounds: 1,
            deps: &[],
            started: Some(5),
            merged: Some(58),
        },
        VoyageTask {
            id: "T-259",
            lane: "keel",
            stage: Stage::Merged,
            title: "goal box, check and milestones",
            est: 30,
            actual: 27,
            rounds: 1,
            deps: &[],
            started: Some(8),
            merged: Some(46),
        },
        VoyageTask {
            id: "T-260",
            lane: "hull",
            stage: Stage::Merged,
            title: "small task: no review, accept proof",
            est: 30,
            actual: 41,
            rounds: 2,
            deps: &["T-257"],
            started: Some(32),
            merged: Some(102),
        },
        VoyageTask {
            id: "T-261",
            lane: "mast",
            stage: Stage::Merged,
            title: "board: the full width",
            est: 45,
            actual: 52,
            rounds: 1,
            deps: &["T-258"],
            started: Some(59),
            merged: Some(128),
        },
        VoyageTask {
            id: "T-262",
            lane: "keel",
            stage: Stage::Review,
            title: "goal lock: GOAL_LOCKED at apply",
            est: 40,
            actual: 35,
            rounds: 1,
            deps: &[],
            started: Some(47),
            merged: None,
        },
        VoyageTask {
            id: "T-263",
            lane: "hull",
            stage: Stage::Rework,
            title: "small task: the captain settles it",
            est: 25,
            actual: 49,
            rounds: 2,
            deps: &["T-260"],
            started: Some(103),
            merged: None,
        },
        VoyageTask {
            id: "T-264",
            lane: "mast",
            stage: Stage::Working,
            title: "board: Time & tokens by lane",
            est: 40,
            actual: 22,
            rounds: 1,
            deps: &["T-261"],
            started: Some(129),
            merged: None,
        },
        VoyageTask {
            id: "T-268",
            lane: "keel",
            stage: Stage::Passed,
            title: "doorbell names the station",
            est: 15,
            actual: 12,
            rounds: 1,
            deps: &[],
            started: Some(70),
            merged: None,
        },
        VoyageTask {
            id: "T-265",
            lane: "keel",
            stage: Stage::Deps,
            title: "goal-change line with the user's words",
            est: 35,
            actual: 0,
            rounds: 0,
            deps: &["T-262"],
            started: None,
            merged: None,
        },
        VoyageTask {
            id: "T-266",
            lane: "mast",
            stage: Stage::Queued,
            title: "board: graph draws dependencies",
            est: 60,
            actual: 0,
            rounds: 0,
            deps: &["T-261"],
            started: None,
            merged: None,
        },
        VoyageTask {
            id: "T-267",
            lane: "hull",
            stage: Stage::Queued,
            title: "release v0.5.0",
            est: 15,
            actual: 0,
            rounds: 0,
            deps: &["T-263", "T-266"],
            started: None,
            merged: None,
        },
    ];
    voyage
        .iter()
        .enumerate()
        .map(|(index, task)| GraphTask {
            id: task.id.into(),
            rank: index as i64 + 1,
            lane: task.lane.into(),
            stage: task.stage,
            moved_to: None,
            title: task.title.into(),
            est_min: Some(task.est),
            actual_min: Some(task.actual),
            rounds: task.rounds,
            deps: task.deps.iter().map(|dep| dep.to_string()).collect(),
            finished: task.merged.map(|minute| minute * 60),
            started: task.started.map(|minute| minute * 60),
        })
        .collect()
}

/// The character a graph row writes at a column: the last cell there wins.
fn glyph(row: &GraphRow, x: usize) -> Option<char> {
    row.cells
        .iter()
        .rev()
        .find(|(column, _, _)| *column == x)
        .map(|(_, glyph, _)| *glyph)
}

#[test]
fn the_git_graph_has_the_rows_of_the_prototype_m5_voyage() {
    let tasks = m5();
    let lanes: Vec<String> = ["hull", "mast", "keel"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    let rows = git_rows(&tasks, &lanes, 0, 160);
    assert_eq!(
        rows.len(),
        24,
        "{:?}",
        rows.iter().map(|row| &row.text).collect::<Vec<_>>()
    );

    // The first rows are the tasks that have not started, the last to run on
    // top (reverse rank: T-267, T-266, T-265 — the graph reads newest on top
    // like git). Each hangs dashed in its lane and says what it waits for.
    let first = &rows[0];
    assert_eq!(first.task.as_deref(), Some("T-267"));
    assert_eq!(glyph(first, 9), Some('○'));
    // T-502: a waiting row stays dim; the two spaces between the title and
    // the stage are the row's own.
    for span in [0, 1, 3, 4] {
        assert_eq!(
            first.spans[span].style,
            Style::fg(Color::Dim),
            "{:?}",
            first.spans
        );
    }
    assert!(
        first.text.starts_with("T-267  release v0.5.0"),
        "{:?}",
        first.text
    );
    assert!(first.text.contains("queued"), "{:?}", first.text);
    assert!(first.text.ends_with("15m"), "{:?}", first.text);
    let dashed = &rows[1];
    assert_eq!(dashed.task.as_deref(), Some("T-267"));
    assert_eq!(glyph(dashed, 9), Some('┆'));
    assert_eq!(dashed.text, "");
    assert_eq!(rows[2].task.as_deref(), Some("T-266"));
    assert_eq!(glyph(&rows[2], 15), Some('○'));
    assert_eq!(rows[3].task.as_deref(), Some("T-266"));
    assert_eq!(glyph(&rows[3], 15), Some('┆'));
    let last_waiting = &rows[4];
    assert_eq!(last_waiting.task.as_deref(), Some("T-265"));
    assert_eq!(glyph(last_waiting, 21), Some('○'));
    assert!(
        last_waiting
            .text
            .starts_with("T-265  goal-change line with the user's words"),
        "{:?}",
        last_waiting.text
    );
    assert!(
        last_waiting.text.contains("waits for 1 to merge"),
        "{:?}",
        last_waiting.text
    );
    assert_eq!(rows[5].task.as_deref(), Some("T-265"));
    assert_eq!(glyph(&rows[5], 21), Some('┆'));

    // The eighth row: T-264's branch leaves dev at +2h09m, after T-261 merged
    // (mast stands at column 15).
    let fork = &rows[7];
    assert_eq!(fork.task, None);
    assert_eq!(glyph(fork, 3), Some('├'));
    for x in 4..=14 {
        assert_eq!(glyph(fork, x), Some('─'), "column {x} of {:?}", fork.text);
    }
    assert_eq!(glyph(fork, 15), Some('╮'));
    assert_eq!(glyph(fork, 16), None);
    assert_eq!(fork.text, "       starts +2h09m after T-261");
    assert_eq!(fork.spans[0].style, Style::fg(Color::Dim));

    // The merge of T-261, below its lane's line: dev joins the branch.
    let merge = &rows[8];
    assert_eq!(merge.task.as_deref(), Some("T-261"));
    assert_eq!(glyph(merge, 3), Some('●'));
    assert_eq!(glyph(merge, 15), Some('╯'));
    let mast = Style::fg(lane_color(&lanes, "mast"));
    assert!(
        merge
            .cells
            .iter()
            .any(|(x, glyph, style)| *x == 15 && *glyph == '╯' && *style == mast),
        "{:?}",
        merge.cells
    );
    assert!(
        merge.text.starts_with("T-261  board: the full width"),
        "{:?}",
        merge.text
    );
    assert!(merge.text.contains("52/45m"), "{:?}", merge.text);
    // The clock of the merge, as the user's clock shows it.
    let (_, merged_at) = merge
        .text
        .rsplit_once("  merged ")
        .expect("the merge clock");
    assert_eq!(merged_at.len(), 5, "{:?}", merge.text);
    assert_eq!(&merged_at[2..3], ":", "{:?}", merge.text);

    // An open task stands on its branch: T-264 is the mast's working task.
    let working = &rows[6];
    assert_eq!(working.task.as_deref(), Some("T-264"));
    assert_eq!(glyph(working, 15), Some('◉'));
    assert_eq!(working.spans[0].style, Style::bold());
    // T-502: the stage words carry the stage's colour, bold while live.
    assert_eq!(
        working.spans[3].style,
        Style::fg(Stage::Working.color()).and_bold()
    );

    // The last two rows are the forks of T-257 and T-258: they started at the
    // same minute, so they keep the plan's order (hull first, then mast).
    let before = &rows[22];
    let last = &rows[23];
    assert_eq!(before.task, None);
    assert_eq!(last.task, None);
    assert_eq!(before.text, "       starts +0h05m");
    assert_eq!(last.text, "       starts +0h05m");
    assert_eq!(glyph(before, 9), Some('╮'));
    assert_eq!(glyph(last, 15), Some('╮'));
}

#[test]
fn the_tasks_carry_their_rank_and_the_moment_their_work_started() {
    let mut store = world();
    let tasks = store.read(|tx| tasks(tx, "p", T0)).unwrap();
    let by_id = |id: &str| tasks.iter().find(|task| task.id == id).expect("the task");
    // The plan's queue position: the order of the fixture.
    assert_eq!(by_id("T-135").rank, 1);
    assert_eq!(by_id("T-151").rank, 10);
    // A merged task started when its first attempt started; a task nobody
    // has started has no start.
    assert_eq!(by_id("T-135").started, Some(moment(11, 2) - 44 * 60));
    assert_eq!(by_id("T-135").finished, Some(moment(11, 2)));
    assert_eq!(by_id("T-148").started, None);
    assert_eq!(by_id("T-148").finished, None);
}

#[test]
fn a_waiting_task_hangs_above_the_ones_that_run_before_it() {
    // The last to run is on top, so the graph reads newest on top like git
    // (T-361 contract v2): T-2 (rank 2) hangs above T-1 (rank 1), nearest the
    // running work.
    let tasks = vec![
        GraphTask {
            id: "T-1".into(),
            rank: 1,
            ..sample()
        },
        GraphTask {
            id: "T-2".into(),
            rank: 2,
            ..sample()
        },
    ];
    let lanes = vec!["hull".to_string()];
    let rows = git_rows(&tasks, &lanes, 0, 120);
    let named: Vec<Option<&str>> = rows.iter().map(|row| row.task.as_deref()).collect();
    assert_eq!(
        named,
        vec![Some("T-2"), Some("T-2"), Some("T-1"), Some("T-1")],
        "{named:?}"
    );
    // The row that names the task is its ○ row, the ┆ row under it.
    assert_eq!(glyph(&rows[0], 9), Some('○'));
    assert_eq!(glyph(&rows[1], 9), Some('┆'));
    assert_eq!(glyph(&rows[2], 9), Some('○'));
    assert_eq!(glyph(&rows[3], 9), Some('┆'));
}

#[test]
fn a_task_that_started_and_waits_again_keeps_its_row() {
    // A task moved to another lane has its attempt abandoned and its stage
    // back to queued, but its earliest start stays: open is the times, not the
    // stage, so it still stands on its branch (F2 of the T-361 review).
    let tasks = vec![GraphTask {
        id: "T-3".into(),
        rank: 3,
        stage: Stage::Queued,
        started: Some(60),
        finished: None,
        ..sample()
    }];
    let lanes = vec!["hull".to_string()];
    let rows = git_rows(&tasks, &lanes, 0, 120);
    assert_eq!(
        rows.len(),
        2,
        "{:?}",
        rows.iter().map(|row| &row.text).collect::<Vec<_>>()
    );
    assert_eq!(rows[0].task.as_deref(), Some("T-3"));
    assert_eq!(glyph(&rows[0], 9), Some('○'));
    // T-502: queued is not a live stage, so the row is dim again.
    assert_eq!(rows[0].spans[0].style, Style::fg(Color::Dim));
    assert_eq!(glyph(&rows[1], 3), Some('├'));
    assert_eq!(rows[1].task, None);
    assert_eq!(rows[1].text, "       starts +0h01m");
}

/// T-471: the graph of the tasks a board shows reads the history of those
/// tasks only, and gives the same rows in the same order as the full graph
/// filtered by them.
#[test]
fn the_graph_of_the_shown_tasks_is_the_filtered_full_graph() {
    let mut store = world();
    let now = T0 + 3600;
    let all = store.read(|tx| tasks(tx, "p", now)).unwrap();
    let shown: std::collections::BTreeSet<&str> =
        all.iter().take(2).map(|task| task.id.as_str()).collect();
    let expected: Vec<GraphTask> = all
        .iter()
        .filter(|task| shown.contains(task.id.as_str()))
        .cloned()
        .collect();
    let narrowed = store
        .read(|tx| gear_board::graph::tasks_shown(tx, "p", now, &shown))
        .unwrap();
    assert_eq!(narrowed, expected);
}
