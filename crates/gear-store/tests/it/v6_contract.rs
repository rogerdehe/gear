//! T-135: schema v6 and the M4 store API (§1 and §13.1 of
//! gear/voyages/M4/design.md, with the budget of §5.1 and the pauses of §5.4).
//!
//! The tests say what the design says: a v5 store migrates to v6 and its new
//! columns are NULL, every function of §13.1 round-trips, and the budget of a
//! round is the one of §5.1.

use gear_model::{AttemptState, ErrorCode, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{
    testing, AttemptRow, NewDecision, NewFriction, NewMessage, NewRound, RoundKey, RoundTokens, Store,
    TaskRow, DEFAULT_REVIEW_BUDGET_MIN, SCHEMA_VERSION,
};
use rusqlite::Connection;

const PROJECT: &str = "p";
const NOW: i64 = 1_800_000_000;

/// The schema of v5: the v6 schema without the M4 tables and columns.
///
/// The fixture is built by taking a fresh store and taking the M4 part away
/// again (SQLite can drop a column and a table), so the store that is
/// migrated up is the shape a v5 store has.
fn a_store_of_v5(path: &std::path::Path) {
    {
        let mut store = Store::open(path).unwrap();
        store
            .write(|tx| {
                tx.insert_project(&gear_store::ProjectRow {
                    id: PROJECT.into(),
                    herdr_session: "sess".into(),
                    config_toml: String::new(),
                    config_version: 1,
                    quota_lanes: 4,
                    quota_temps: 2,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: NOW,
                })?;
                tx.insert_task(&TaskRow {
                    project: PROJECT.into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("hull".into()),
                    milestone: Some("M4".into()),
                    required: true,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(45),
                    issue: None,
                    repo: Some("gear".into()),
                    base: Some("dev".into()),
                    gate: Some("make test".into()),
                    rework_estimate_min: None,
                    updated: NOW,
                })?;
                tx.insert_attempt(&AttemptRow {
                    project: PROJECT.into(),
                    task: "T-1".into(),
                    n: 1,
                    seat: "p:hull".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Started,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: NOW,
                    updated: NOW,
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                })?;
                tx.insert_message(&NewMessage {
                    project: PROJECT.into(),
                    thread: "T-1:1".into(),
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    sender: "p:hull".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Progress,
                    urgent: false,
                    body: "hello".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: NOW,
                    meta: None,
                })?;
                Ok(())
            })
            .unwrap();
    }
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "DROP TABLE rounds;
         DROP TABLE seat_pauses;
         DROP TABLE frictions;
         DROP TABLE decisions;
         ALTER TABLE attempts DROP COLUMN session_ref;
         ALTER TABLE attempts DROP COLUMN session_cli;
         ALTER TABLE attempts DROP COLUMN session_action;
         ALTER TABLE attempts DROP COLUMN sent_at;
         ALTER TABLE tasks DROP COLUMN repo;
         ALTER TABLE tasks DROP COLUMN base;
         ALTER TABLE tasks DROP COLUMN gate;
         ALTER TABLE tasks DROP COLUMN rework_estimate_min;
         ALTER TABLE messages DROP COLUMN meta;
         UPDATE meta SET value='5' WHERE key='schema_version';",
    )
    .unwrap();
}

fn a_task(store: &mut Store, id: &str, estimate_min: Option<i64>) {
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                project: PROJECT.into(),
                id: id.into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("hull".into()),
                milestone: Some("M4".into()),
                required: true,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min,
                issue: None,
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                updated: NOW,
            })
        })
        .unwrap();
}

fn a_round(store: &mut Store, task: &str, attempt: i64, role: &str, start_at: i64) -> RoundKey {
    let key = RoundKey {
        project: PROJECT.into(),
        task: task.into(),
        attempt,
        role: role.into(),
    };
    store
        .write(|tx| {
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: key.project.clone(),
                    task: key.task.clone(),
                    attempt: key.attempt,
                    role: key.role.clone(),
                },
                seat: "p:hull".into(),
                generation: 1,
                cli: "pi".into(),
                model: Some("minimax".into()),
                start_at,
            })
        })
        .unwrap();
    key
}

#[test]
fn a_v5_store_migrates_to_v6_with_the_new_columns_null() {    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    a_store_of_v5(&path);

    // The store is of v5 before it is opened.
    let conn = Connection::open(&path).unwrap();
    let version: String = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, "5");

    // Opening it carries it up to v6.
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.read(|tx| tx.get_meta("schema_version")).unwrap().as_deref(),
        Some(SCHEMA_VERSION.to_string().as_str())
    );

    // The rows are still there.
    let task = store.read(|tx| tx.get_task(PROJECT, "T-1")).unwrap().unwrap();
    assert_eq!(task.estimate_min, Some(45));
    let attempt = store.read(|tx| tx.get_attempt(PROJECT, "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.seat, "p:hull");
    let message = store.read(|tx| tx.get_message(PROJECT, "M-1")).unwrap().unwrap();
    assert_eq!(message.body, "hello");

    // The new columns are NULL: the migration does not invent values.
    assert_eq!(task.repo, None);
    assert_eq!(task.base, None);
    assert_eq!(task.gate, None);
    assert_eq!(task.rework_estimate_min, None);
    assert_eq!(attempt.session_ref, None);
    assert_eq!(attempt.session_cli, None);
    assert_eq!(attempt.session_action, None);
    assert_eq!(attempt.sent_at, None);
    assert_eq!(message.meta, None);
}

#[test]
fn a_pause_table_of_the_older_key_is_rebuilt_by_the_migration() {
    // A store that a first attempt of this task migrated has the older
    // `seat_pauses` key (project, seat, start_at): the kind was not in it, so
    // a quota park and a declared wait of one seat could not be there at once.
    // The migration rebuilds the table with the key of §5.4 and keeps the
    // rows.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    {
        let mut store = Store::open(&path).unwrap();
        store
            .write(|tx| {
                tx.insert_project(&gear_store::ProjectRow {
                    id: PROJECT.into(),
                    herdr_session: "sess".into(),
                    config_toml: String::new(),
                    config_version: 1,
                    quota_lanes: 4,
                    quota_temps: 2,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: NOW,
                })?;
                tx.open_pause(PROJECT, "p:hull", "quota", "minimax-cn 5h", NOW)
            })
            .unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE seat_pauses;
         CREATE TABLE seat_pauses (project TEXT NOT NULL, seat TEXT NOT NULL, kind TEXT NOT NULL,
           reason TEXT, start_at INTEGER NOT NULL, end_at INTEGER,
           PRIMARY KEY (project, seat, start_at));
         INSERT INTO seat_pauses (project, seat, kind, reason, start_at, end_at)
           VALUES ('p', 'p:hull', 'quota', 'minimax-cn 5h', 1800000000, NULL);
         UPDATE meta SET value='5' WHERE key='schema_version';",
    )
    .unwrap();

    let mut store = Store::open(&path).unwrap();
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses.len(), 1, "{pauses:?}");
    assert_eq!(pauses[0].kind, "quota");
    assert_eq!(pauses[0].start_at, NOW);
    assert_eq!(pauses[0].end_at, None);

    // The key of §5.4 is there: a wait of the same seat may be open beside it.
    store
        .write(|tx| tx.open_pause(PROJECT, "p:hull", "wait", "T-2 is not merged", NOW + 10))
        .unwrap();
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses.len(), 2, "{pauses:?}");
}

#[test]
fn the_attempt_session_and_the_sent_time_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&gear_store::ProjectRow {
                id: PROJECT.into(),
                herdr_session: "sess".into(),
                config_toml: String::new(),
                config_version: 1,
                quota_lanes: 4,
                quota_temps: 2,
                quota_includes_captain: false,
                plan_version: 0,
                created: NOW,
            })?;
            tx.insert_task(&TaskRow {
                project: PROJECT.into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: None,
                milestone: None,
                required: true,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                updated: NOW,
            })?;
            tx.insert_attempt(&AttemptRow {
                project: PROJECT.into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:hull".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: NOW,
                updated: NOW,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })
        })
        .unwrap();

    store
        .write(|tx| {
            tx.set_attempt_session(PROJECT, "T-1", 1, "sess-42", "pi")?;
            tx.set_attempt_action(PROJECT, "T-1", 1, "resume")?;
            tx.set_attempt_sent(PROJECT, "T-1", 1, NOW + 5)
        })
        .unwrap();

    let attempt = store.read(|tx| tx.get_attempt(PROJECT, "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.session_ref.as_deref(), Some("sess-42"));
    assert_eq!(attempt.session_cli.as_deref(), Some("pi"));
    assert_eq!(attempt.session_action.as_deref(), Some("resume"));
    assert_eq!(attempt.sent_at, Some(NOW + 5));

    // An attempt that does not exist is refused.
    let err = store
        .write(|tx| tx.set_attempt_session(PROJECT, "T-9", 1, "s", "pi"))
        .err()
        .expect("must refuse");
    assert_eq!(err.code, ErrorCode::NoTarget);
}

#[test]
fn a_round_round_trips_and_a_second_one_of_the_same_key_fails() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));
    let key = a_round(&mut store, "T-1", 1, "dev", NOW);

    // The round is there, open.
    let open = store
        .read(|tx| tx.open_round_of_seat("p:hull"))
        .unwrap()
        .expect("the seat has an open round");
    assert_eq!(open.task, "T-1");
    assert_eq!(open.role, "dev");
    assert_eq!(open.start_at, NOW);
    assert_eq!(open.end_at, None);
    assert_eq!(open.active_s, 0);
    assert_eq!(open.paused_s, 0);
    assert_eq!(open.sessions, Vec::<String>::new());

    // The session of the seat, the pause, the active time and the tokens.
    store
        .write(|tx| {
            tx.add_round_session(&key, "sess-1")?;
            tx.add_round_session(&key, "sess-1")?;
            tx.add_round_session(&key, "sess-2")?;
            tx.add_round_pause(&key, 60)?;
            tx.set_round_active(&key, 900)?;
            tx.set_round_tokens(
                &key,
                &RoundTokens {
                    input: Some(100),
                    output: Some(20),
                    cache_read: Some(300),
                    cache_write: Some(50),
                    reasoning: Some(5),
                    cost_usd: Some(0.25),
                    cost_source: "agent".into(),
                },
                NOW + 100,
            )
        })
        .unwrap();

    let round = store.read(|tx| tx.rounds_of_task(PROJECT, "T-1")).unwrap();
    assert_eq!(round.len(), 1);
    let round = &round[0];
    assert_eq!(round.sessions, vec!["sess-1".to_string(), "sess-2".to_string()]);
    assert_eq!(round.paused_s, 60);
    assert_eq!(round.active_s, 900);
    assert_eq!(round.input, Some(100));
    assert_eq!(round.output, Some(20));
    assert_eq!(round.cache_read, Some(300));
    assert_eq!(round.cache_write, Some(50));
    assert_eq!(round.reasoning, Some(5));
    assert_eq!(round.cost_usd, Some(0.25));
    assert_eq!(round.cost_source.as_deref(), Some("agent"));
    assert_eq!(round.counted_at, Some(NOW + 100));

    // The round is closed at the next round's start (§4.1).
    store.write(|tx| tx.close_round(&key, NOW + 200)).unwrap();
    let round = store.read(|tx| tx.rounds_of_task(PROJECT, "T-1")).unwrap();
    assert_eq!(round[0].end_at, Some(NOW + 200));
    assert!(store.read(|tx| tx.open_round_of_seat("p:hull")).unwrap().is_none());

    // One round per (project, task, attempt, role).
    let err = store
        .write(|tx| {
            tx.insert_round(&NewRound {
                key: key.clone(),
                seat: "p:hull".into(),
                generation: 1,
                cli: "pi".into(),
                model: Some("minimax".into()),
                start_at: NOW,
            })
        })
        .err()
        .expect("the second round of the same key must fail");
    assert_eq!(err.code, ErrorCode::BadFormat);

    // The rounds of the project are the same rows.
    let all = store.read(|tx| tx.rounds_of_project(PROJECT)).unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn the_budget_of_a_dev_round_is_the_estimate_of_the_task() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));
    let budget = store
        .read(|tx| tx.round_budget_min(PROJECT, "T-1", 1, "dev", None))
        .unwrap();
    assert_eq!(budget, 45);
}

#[test]
fn the_budget_of_a_rework_round_is_the_estimate_of_the_verdict_or_half_the_estimate() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));

    // Without an estimate on the fail verdict: max(10, ceil(45 / 2)) = 23.
    let budget = store
        .read(|tx| tx.round_budget_min(PROJECT, "T-1", 2, "rework", None))
        .unwrap();
    assert_eq!(budget, 23);

    // With one: the verdict decides.
    store
        .write(|tx| tx.set_task_rework_estimate(PROJECT, "T-1", Some(30)))
        .unwrap();
    let budget = store
        .read(|tx| tx.round_budget_min(PROJECT, "T-1", 2, "rework", None))
        .unwrap();
    assert_eq!(budget, 30);

    // The estimate of the task is read back.
    let task = store.read(|tx| tx.get_task(PROJECT, "T-1")).unwrap().unwrap();
    assert_eq!(task.rework_estimate_min, Some(30));
}

#[test]
fn the_budget_of_a_review_round_is_measured_and_falls_back_to_thirty() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));

    // Fewer than three finished review rounds of the model: 30 min.
    let budget = store
        .read(|tx| tx.round_budget_min(PROJECT, "T-1", 1, "review", Some("minimax")))
        .unwrap();
    assert_eq!(budget, 30);

    // Five finished review rounds of 10 minutes each: 1.5 × 10 = 15 min.
    for i in 0..5 {
        let key = a_round(&mut store, "T-1", 10 + i, "review", NOW + i * 1000);
        store
            .write(|tx| {
                tx.set_round_active(&key, 600)?;
                tx.close_round(&key, NOW + i * 1000 + 700)
            })
            .unwrap();
    }
    let budget = store
        .read(|tx| tx.round_budget_min(PROJECT, "T-1", 1, "review", Some("minimax")))
        .unwrap();
    assert_eq!(budget, 15);
}

#[test]
fn a_review_round_that_measured_no_activity_does_not_enter_the_median() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));

    // Ten finished review rounds that never measured activity: nothing is
    // known about how long a review takes, so the default stands.
    for i in 0..10 {
        let key = a_round(&mut store, "T-1", 10 + i, "review", NOW + i * 1000);
        store
            .write(|tx| tx.close_round(&key, NOW + i * 1000 + 700))
            .unwrap();
    }
    let budget = store
        .read(|tx| tx.round_budget_min(PROJECT, "T-1", 1, "review", Some("minimax")))
        .unwrap();
    assert_eq!(budget, DEFAULT_REVIEW_BUDGET_MIN);
    assert_eq!(budget, 30);
}

#[test]
fn only_the_measured_review_rounds_build_the_median() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));

    // Three rounds of 10 minutes and seven of 0: the seven say nothing, so
    // the median is 10 min and the budget 1.5 × 10 = 15 min, not the floor.
    for i in 0..10 {
        let key = a_round(&mut store, "T-1", 10 + i, "review", NOW + i * 1000);
        store
            .write(|tx| {
                if i < 3 {
                    tx.set_round_active(&key, 600)?;
                }
                tx.close_round(&key, NOW + i * 1000 + 700)
            })
            .unwrap();
    }
    let budget = store
        .read(|tx| tx.round_budget_min(PROJECT, "T-1", 1, "review", Some("minimax")))
        .unwrap();
    assert_eq!(budget, 15);
}

#[test]
fn a_friction_report_is_f_1_then_f_2_and_can_be_closed() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));

    let first = store
        .write(|tx| {
            tx.insert_friction(&NewFriction {
                project: PROJECT.into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
                seat: "p:hull".into(),
                body: "the build took too long".into(),
                required: true,
            })
        })
        .unwrap();
    let second = store
        .write(|tx| {
            tx.insert_friction(&NewFriction {
                project: PROJECT.into(),
                task: "T-1".into(),
                attempt: 1,
                role: "dev".into(),
                seat: "p:hull".into(),
                body: "an unclear contract".into(),
                required: false,
            })
        })
        .unwrap();
    assert_eq!(first, "F-1");
    assert_eq!(second, "F-2");

    let open = store.read(|tx| tx.open_frictions(PROJECT)).unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].id, "F-1");
    assert!(open[0].required);
    assert!(!open[1].required);
    assert_eq!(open[0].closed_at, None);
    // The store writes the time of the report itself (§13.1: the input has no
    // `created`).
    assert!(open[0].created > 0, "{:?}", open[0].created);
    assert!(open[1].created >= open[0].created, "{:?}", open[1].created);

    store
        .write(|tx| tx.close_friction(PROJECT, "F-1", "fixed (a cache)", NOW + 10))
        .unwrap();
    let open = store.read(|tx| tx.open_frictions(PROJECT)).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, "F-2");
    let all = store.read(|tx| tx.frictions_of_project(PROJECT)).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].outcome.as_deref(), Some("fixed (a cache)"));
    assert_eq!(all[0].closed_at, Some(NOW + 10));
}

#[test]
fn a_decision_is_logged_once_and_read_back_in_its_order() {
    let mut store = Store::open_in_memory().unwrap();
    let decision = NewDecision {
        project: PROJECT.into(),
        key: "abc123".into(),
        voyage: "M4".into(),
        at: "2026-10-26 10:00".into(),
        kind: "cut".into(),
        what: "T-n cancelled".into(),
        why: Some("not needed".into()),
        alt: Some("keep it".into()),
        changes: Some("scope".into()),
        star: true,
        source: "gear".into(),
        line: None,
        plan_version: Some(20),
    };

    let written = store.write(|tx| tx.insert_decision_if_new(&decision)).unwrap();
    assert!(written);
    // The same line again: the key is there, nothing is written.
    let again = store.write(|tx| tx.insert_decision_if_new(&decision)).unwrap();
    assert!(!again);

    let decisions = store.read(|tx| tx.decisions_of(PROJECT, "M4")).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].key, "abc123");
    assert_eq!(decisions[0].seq, 1);
    assert_eq!(decisions[0].kind, "cut");
    assert_eq!(decisions[0].what, "T-n cancelled");
    assert_eq!(decisions[0].why.as_deref(), Some("not needed"));
    assert_eq!(decisions[0].alt.as_deref(), Some("keep it"));
    assert_eq!(decisions[0].changes.as_deref(), Some("scope"));
    assert!(decisions[0].star);
    assert_eq!(decisions[0].source, "gear");
    assert_eq!(decisions[0].plan_version, Some(20));
    // The store writes the time of the entry itself (§13.1: the input has no
    // `created`).
    assert!(decisions[0].created > 0, "{:?}", decisions[0].created);

    // A second decision gets the next number.
    let mut other = decision.clone();
    other.key = "def456".into();
    other.what = "another decision".into();
    store.write(|tx| tx.insert_decision_if_new(&other)).unwrap();
    let decisions = store.read(|tx| tx.decisions_of(PROJECT, "M4")).unwrap();
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[1].seq, 2);
    assert_eq!(decisions[1].what, "another decision");
}

#[test]
fn a_pause_opens_and_closes_and_the_project_reads_them_back() {
    let mut store = Store::open_in_memory().unwrap();

    store
        .write(|tx| tx.open_pause(PROJECT, "p:hull", "quota", "minimax-cn 5h", NOW))
        .unwrap();
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].seat, "p:hull");
    assert_eq!(pauses[0].kind, "quota");
    assert_eq!(pauses[0].reason.as_deref(), Some("minimax-cn 5h"));
    assert_eq!(pauses[0].start_at, NOW);
    assert_eq!(pauses[0].end_at, None);

    // A seat that is already paused keeps the pause it has.
    store
        .write(|tx| tx.open_pause(PROJECT, "p:hull", "quota", "minimax-cn 5h", NOW + 1))
        .unwrap();
    assert_eq!(store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap().len(), 1);

    store
        .write(|tx| tx.close_pause(PROJECT, "p:hull", "quota", NOW + 3600))
        .unwrap();
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].end_at, Some(NOW + 3600));

    // A pause that is not open cannot be closed.
    let err = store
        .write(|tx| tx.close_pause(PROJECT, "p:hull", "quota", NOW + 7200))
        .err()
        .expect("must refuse");
    assert_eq!(err.code, ErrorCode::NoTarget);
}

/// A quota park and a declared wait of one seat are independent stretches
/// (§5.4): they may be there at once, and each keeps its own time.
#[test]
fn a_quota_pause_and_a_declared_wait_of_one_seat_overlap() {
    let mut store = Store::open_in_memory().unwrap();

    store
        .write(|tx| tx.open_pause(PROJECT, "p:hull", "wait", "T-2 is not merged", NOW))
        .unwrap();
    store
        .write(|tx| tx.open_pause(PROJECT, "p:hull", "quota", "minimax-cn 5h", NOW + 10))
        .unwrap();

    // Both are there: the quota park is not dropped by the wait.
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses.len(), 2, "{pauses:?}");
    let wait = pauses.iter().find(|pause| pause.kind == "wait").expect("the wait");
    let quota = pauses.iter().find(|pause| pause.kind == "quota").expect("the quota park");
    assert_eq!(wait.start_at, NOW);
    assert_eq!(quota.start_at, NOW + 10);

    // Each is closed on its own.
    store
        .write(|tx| tx.close_pause(PROJECT, "p:hull", "quota", NOW + 3610))
        .unwrap();
    store
        .write(|tx| tx.close_pause(PROJECT, "p:hull", "wait", NOW + 100))
        .unwrap();
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses.len(), 2, "{pauses:?}");
    let wait = pauses.iter().find(|pause| pause.kind == "wait").expect("the wait");
    let quota = pauses.iter().find(|pause| pause.kind == "quota").expect("the quota park");
    assert_eq!(wait.end_at, Some(NOW + 100));
    assert_eq!(quota.end_at, Some(NOW + 3610));
    // Both durations survive: an hour of quota wait and 100 s of wait.
    assert_eq!(quota.end_at.unwrap() - quota.start_at, 3600);
    assert_eq!(wait.end_at.unwrap() - wait.start_at, 100);
}

/// The row key of a pause is (project, seat, kind, start_at): a pause of the
/// same kind that starts in the same second is that row again, and a pause
/// never ends before it began.
#[test]
fn a_pause_of_the_same_kind_in_the_same_second_reopens_that_row() {
    let mut store = Store::open_in_memory().unwrap();

    store
        .write(|tx| tx.open_pause(PROJECT, "p:hull", "quota", "minimax-cn 5h", NOW))
        .unwrap();
    store
        .write(|tx| tx.close_pause(PROJECT, "p:hull", "quota", NOW))
        .unwrap();
    // The same kind in the same second: the row is opened again, not a second
    // row with a time of its own.
    store
        .write(|tx| tx.open_pause(PROJECT, "p:hull", "quota", "minimax-cn 5h", NOW))
        .unwrap();
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses.len(), 1, "{pauses:?}");
    assert_eq!(pauses[0].start_at, NOW);
    assert_eq!(pauses[0].end_at, None);

    // A pause never ends before it began.
    store
        .write(|tx| tx.close_pause(PROJECT, "p:hull", "quota", NOW - 60))
        .unwrap();
    let pauses = store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap();
    assert_eq!(pauses[0].end_at, Some(NOW), "{pauses:?}");
}

#[test]
fn a_declared_wait_is_written_and_cleared() {
    let mut store = Store::open_in_memory().unwrap();
    a_task(&mut store, "T-1", Some(45));

    store
        .write(|tx| tx.set_task_wait(PROJECT, "T-1", "T-2", "merged"))
        .unwrap();
    let task = store.read(|tx| tx.get_task(PROJECT, "T-1")).unwrap().unwrap();
    assert_eq!(task.wait_on.as_deref(), Some("T-2"));
    assert_eq!(task.wait_cond.as_deref(), Some("merged"));

    store.write(|tx| tx.clear_task_wait(PROJECT, "T-1")).unwrap();
    let task = store.read(|tx| tx.get_task(PROJECT, "T-1")).unwrap().unwrap();
    assert_eq!(task.wait_on, None);
    assert_eq!(task.wait_cond, None);
}

#[test]
fn a_task_keeps_its_repo_base_and_gate() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                project: PROJECT.into(),
                id: "T-7".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("hull".into()),
                milestone: Some("M4".into()),
                required: true,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(45),
                issue: None,
                repo: Some("gear".into()),
                base: Some("dev".into()),
                gate: Some("make test".into()),
                rework_estimate_min: None,
                updated: NOW,
            })
        })
        .unwrap();
    let task = store.read(|tx| tx.get_task(PROJECT, "T-7")).unwrap().unwrap();
    assert_eq!(task.repo.as_deref(), Some("gear"));
    assert_eq!(task.base.as_deref(), Some("dev"));
    assert_eq!(task.gate.as_deref(), Some("make test"));
}

#[test]
fn the_seat_of_a_project_is_readable_next_to_the_new_tables() {
    // The store of a project with a seat still works after the migration: the
    // new tables do not touch the old ones.
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, PROJECT, "sess", NOW)?;
            testing::seed_active_seat(tx, PROJECT, "hull", SeatRole::Lane, "pane-1", "pi", NOW)?;
            Ok(())
        })
        .unwrap();
    let seats = store.read(|tx| tx.seats_of_project(PROJECT)).unwrap();
    assert!(!seats.is_empty());
    assert_eq!(store.read(|tx| tx.rounds_of_project(PROJECT)).unwrap().len(), 0);
    assert_eq!(store.read(|tx| tx.open_frictions(PROJECT)).unwrap().len(), 0);
    assert_eq!(store.read(|tx| tx.pauses_of_project(PROJECT)).unwrap().len(), 0);
    assert_eq!(store.read(|tx| tx.decisions_of(PROJECT, "M4")).unwrap().len(), 0);
}
