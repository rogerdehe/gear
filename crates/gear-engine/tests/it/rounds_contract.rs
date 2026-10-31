//! T-151: the rounds bookkeeping (design §4.1) and the session of a check-in
//! (design §3.2 step 5).
//!
//! A round owns every agent record on its seat from its `start_at` until its
//! `end_at`: the start of the next round on the same seat, or the seat's
//! release. The tick of `rounds.rs` keeps that true: the session the seat
//! carries now joins its open round (once), and a round ends when the next
//! round starts or the seat is released.

use gear_engine::dispatch::DeliverAction;
use gear_engine::ops::{confirm_pending, intend_deliveries};
use gear_herdr::fake::FakeHerdr;
use gear_model::{AttemptState, OpKind, OpState, SeatRole, SeatState, TaskState};
use gear_store::{testing, AttemptRow, NewRound, RoundKey, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;
const LANE: &str = "p:heat";

/// A project with one lane whose agent runs in a pane, and a round that is
/// open on it.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            // The agent runs in a session of its own.
            tx.set_binding_session_ref(LANE, 1, "ses_A")?;
            Ok(())
        })
        .unwrap();
    store
}

/// A round on the lane, open since `start_at`.
fn round(store: &mut Store, task: &str, attempt: i64, role: &str, start_at: i64) {
    store
        .write(|tx| {
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: task.into(),
                    attempt,
                    role: role.into(),
                },
                seat: LANE.into(),
                generation: 1,
                cli: "opencode".into(),
                model: None,
                start_at,
            })
        })
        .unwrap();
}

/// The round of a task, as the store keeps it.
fn round_of(store: &mut Store, task: &str, attempt: i64, role: &str) -> gear_store::RoundRow {
    store
        .read(|tx| tx.rounds_of_task("p", task))
        .unwrap()
        .into_iter()
        .find(|row| row.attempt == attempt && row.role == role)
        .expect("the round")
}

/// T-151: a session that changes inside a round joins the round's sessions —
/// once, however often the tick runs.
#[test]
fn a_session_change_mid_round_lands_in_the_rounds_sessions_once() {
    let mut store = world();
    round(&mut store, "T-1", 1, "dev", T0);

    let mut herdr = FakeHerdr::new();
    // The agent is restarted inside the round: the binding carries a new
    // session.
    store
        .write(|tx| tx.set_binding_session_ref(LANE, 1, "ses_B"))
        .unwrap();

    let report =
        gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 10).unwrap();
    assert_eq!(report.sessions, vec!["T-1/1/dev"], "{report:?}");

    let row = round_of(&mut store, "T-1", 1, "dev");
    assert_eq!(
        row.sessions,
        vec!["ses_B"],
        "the new session owns the records"
    );

    // The next tick says it again — the list holds it once.
    let report =
        gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 20).unwrap();
    assert!(report.sessions.is_empty(), "nothing new: {report:?}");
    let row = round_of(&mut store, "T-1", 1, "dev");
    assert_eq!(row.sessions, vec!["ses_B"]);
}

/// T-151: a second round on the seat closes the first at its start.
#[test]
fn a_second_round_on_the_seat_closes_the_first_at_its_start() {
    let mut store = world();
    round(&mut store, "T-1", 1, "dev", T0);
    round(&mut store, "T-2", 1, "dev", T0 + 100);

    let mut herdr = FakeHerdr::new();
    let report =
        gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 110).unwrap();
    assert_eq!(report.closed, vec!["T-1/1/dev"], "{report:?}");

    let first = round_of(&mut store, "T-1", 1, "dev");
    assert_eq!(
        first.end_at,
        Some(T0 + 100),
        "the round ends at the next one's start"
    );
    let second = round_of(&mut store, "T-2", 1, "dev");
    assert_eq!(second.end_at, None, "the newest round stays open");
}

/// T-151: a release closes the open round.
#[test]
fn a_release_closes_the_open_round() {
    let mut store = world();
    round(&mut store, "T-1", 1, "review", T0);
    // The reviewer is released.
    store
        .write(|tx| tx.set_seat_state(LANE, SeatState::Released, Some("released"), T0 + 50))
        .unwrap();

    let mut herdr = FakeHerdr::new();
    let report =
        gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 60).unwrap();
    assert_eq!(report.closed, vec!["T-1/1/review"], "{report:?}");

    let row = round_of(&mut store, "T-1", 1, "review");
    assert_eq!(
        row.end_at,
        Some(T0 + 50),
        "the round ends when the seat is released"
    );
}

/// T-151 (§3.2 step 5): the check-in sets the attempt's session and its CLI.
#[test]
fn the_check_in_sets_the_attempts_session() {
    let mut store = world();
    // A delivery for the lane, on its way to the pane.
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                issue: None,
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                updated: T0,
            })?;
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: LANE.into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Delivered,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            let ops = intend_deliveries(
                tx,
                &[DeliverAction {
                    project: "p".into(),
                    seat: LANE.into(),
                    generation: 1,
                    task: "T-1".into(),
                    attempt: 1,
                    text: "New task T-1".into(),
                }],
                T0 + 10,
            )?;
            tx.set_op_state("p", &ops[0].id, OpState::Executing, None, T0 + 10)?;
            tx.set_op_state("p", &ops[0].id, OpState::Submitted, None, T0 + 11)?;
            Ok(())
        })
        .unwrap();

    // The lane checks in: the view shows the task and the attempt.
    store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: LANE.into(),
                generation: 1,
                ts: T0 + 20,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: Some("T-1".into()),
                shown_attempt: Some(1),
                shown_contract: Some(1),
            })
        })
        .unwrap();

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 30).unwrap();
    assert_eq!(confirmed.len(), 1, "{confirmed:?}");

    let attempt = store
        .read(|tx| tx.get_attempt("p", "T-1", 1))
        .unwrap()
        .unwrap();
    assert_eq!(attempt.state, AttemptState::Started);
    assert_eq!(
        attempt.session_ref.as_deref(),
        Some("ses_A"),
        "the session of the check-in"
    );
    assert_eq!(
        attempt.session_cli.as_deref(),
        Some("opencode"),
        "the CLI of the seat"
    );
}

/// T-151 (the review of attempt 1, F2): a round that is closed by the
/// release of its seat keeps the session of its last work: the tick closes the
/// superseded rounds, then gives the open round the session the seat carries
/// now, and closes the released round last.
#[test]
fn a_released_round_keeps_the_session_of_its_last_work() {
    let mut store = world();
    round(&mut store, "T-1", 1, "review", T0);
    let mut herdr = FakeHerdr::new();

    // The first tick: the round owns the session the seat carries now.
    gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 10).unwrap();
    assert_eq!(
        round_of(&mut store, "T-1", 1, "review").sessions,
        vec!["ses_A"]
    );

    // The agent is restarted (a new session) and the seat is released in the
    // same tick.
    store
        .write(|tx| tx.set_binding_session_ref(LANE, 1, "ses_B"))
        .unwrap();
    store
        .write(|tx| tx.set_seat_state(LANE, SeatState::Released, Some("released"), T0 + 50))
        .unwrap();

    let report =
        gear_engine::rounds::tick(&mut store, &mut herdr, &Default::default(), T0 + 60).unwrap();
    assert_eq!(report.closed, vec!["T-1/1/review"], "{report:?}");

    let row = round_of(&mut store, "T-1", 1, "review");
    assert_eq!(
        row.sessions,
        vec!["ses_A", "ses_B"],
        "the last work is in the round"
    );
    assert_eq!(row.end_at, Some(T0 + 50), "the release time is kept");
}

/// T-151 (the review of attempt 1, F1): a review delivery names the
/// **writer's** attempt, while its binding belongs to the reviewer. The
/// writer's session is the writer's: the review says nothing about it.
#[test]
fn a_review_confirmation_leaves_the_writers_session_alone() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T0)?;
            // The writer works in its own session.
            tx.set_binding_session_ref("p:heat", 1, "ses_A")?;
            tx.set_binding_session_ref("p:review-1", 1, "ses_REVIEWER")?;
            // The task the writer works on (its attempt waits for its
            // check-in, so the task is still Ready).
            tx.insert_task(&TaskRow {
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                issue: None,
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                updated: T0,
            })?;
            // The writer's attempt, already in its session.
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Delivered,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: Some("ses_A".into()),
                session_cli: Some("opencode".into()),
                session_action: None,
                sent_at: None,
            })?;
            // The reviewer's delivery: it names the writer's attempt.
            let op = tx.insert_op(&gear_store::NewOp {
                project: "p".into(),
                seat: "p:review-1".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "Review T-1 attempt 1"}),
                created: T0 + 10,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 10)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 11)?;
            // The reviewer hands in its verdict: the delivery is confirmed.
            tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed: true,
                findings: "ops/reviews/T-1.md".into(),
                by_seat: "p:review-1".into(),
                created: T0 + 20,
            })?;
            Ok(())
        })
        .unwrap();

    let (confirmed, _) = confirm_pending(&mut store, "p", 3, T0 + 30).unwrap();
    assert_eq!(
        confirmed.len(),
        1,
        "the review delivery is confirmed: {confirmed:?}"
    );

    let attempt = store
        .read(|tx| tx.get_attempt("p", "T-1", 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        attempt.session_ref.as_deref(),
        Some("ses_A"),
        "the writer keeps its session"
    );
    assert_eq!(
        attempt.session_cli.as_deref(),
        Some("opencode"),
        "the writer keeps its CLI"
    );
}

#[test]
fn token_windows_cover_rework_followups_compaction_reviews_and_model_changes() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str, lines: Vec<serde_json::Value>| {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            lines
                .into_iter()
                .map(|v| v.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        path.to_string_lossy().to_string()
    };
    let rec = |time, input, model: &str| json!({"type":"message", "timestamp":time, "message":{"role":"assistant", "model":model, "usage":{"input":input,"output":10,"cacheRead":20,"cacheWrite":5}}});
    let a = file(
        "a",
        vec![
            rec(T0 + 10, 100, "p/m1"),
            rec(T0 + 40, 200, "p/m1"),
            rec(T0 + 80, 300, "p/m1"),
        ],
    );
    let b = file(
        "b",
        vec![rec(T0 + 50, 400, "p/m1"), rec(T0 + 60, 500, "p/m1")],
    );
    let r = file(
        "r",
        vec![rec(T0 + 10, 600, "p/m1"), rec(T0 + 80, 700, "p/m1")],
    );
    let q = file("q", vec![rec(T0 + 50, 800, "p/m1")]);
    let switched = file(
        "switch",
        vec![rec(T0 + 10, 100, "p/m1"), rec(T0 + 20, 200, "p/m2")],
    );
    let mut store = world();
    store.write(|tx| {
        let mut project=tx.get_project("p")?.unwrap();
        project.config_toml="[prices.\"p/m1\"]\ninput=1\noutput=0\ncache_read=0\ncache_write=0\n[prices.\"p/m2\"]\ninput=3\noutput=0\ncache_read=0\ncache_write=0\n".into();
        tx.update_project(&project)
    }).unwrap();
    let mut cfg = gear_engine::EngineCfg::default();
    let mut profile = gear_herdr::builtin("opencode").unwrap();
    profile.usage_log = Some("pi-jsonl".into());
    cfg.profiles.insert("opencode".into(), profile);
    let cases = [
        ("T-1", 1, "dev", T0, T0 + 40, vec![a.clone()], 100),
        ("T-2", 1, "dev", T0 + 40, T0 + 70, vec![b.clone()], 900),
        ("T-1", 2, "dev", T0 + 70, T0 + 100, vec![a.clone()], 300),
        ("T-3", 1, "dev", T0 + 10, T0 + 40, vec![a.clone()], 100),
        (
            "T-4",
            1,
            "dev",
            T0 + 40,
            T0 + 70,
            vec![a.clone(), b.clone()],
            1100,
        ),
        ("T-5", 1, "review", T0, T0 + 40, vec![r.clone()], 600),
        ("T-6", 1, "review", T0 + 40, T0 + 70, vec![q.clone()], 800),
        ("T-5", 2, "review", T0 + 70, T0 + 100, vec![r.clone()], 700),
        ("T-7", 1, "dev", T0, T0 + 30, vec![switched.clone()], 300),
    ];
    for (task, attempt, role, start, end, sessions, expected) in cases {
        round(&mut store, task, attempt, role, start);
        let key = RoundKey {
            project: "p".into(),
            task: task.into(),
            attempt,
            role: role.into(),
        };
        store
            .write(|tx| {
                for session in &sessions {
                    tx.add_round_session(&key, session)?;
                }
                tx.close_round(&key, end)
            })
            .unwrap();
        let row = round_of(&mut store, task, attempt, role);
        let tokens = gear_engine::rounds::count_round(&mut store, &cfg, &row, T0 + 200).unwrap();
        assert_eq!(tokens.input, Some(expected), "{task}/{attempt}");
        if task == "T-7" {
            assert!((tokens.cost_usd.unwrap() - 0.0007).abs() < 1e-10);
            assert_eq!(tokens.cost_source, "prices");
        }
    }
    // Some logs name only the model; a matching round model supplies its provider.
    let bare = file("bare", vec![rec(T0 + 10, 100, "m1")]);
    let key = RoundKey {
        project: "p".into(),
        task: "T-10".into(),
        attempt: 1,
        role: "dev".into(),
    };
    store
        .write(|tx| {
            tx.insert_round(&NewRound {
                key: key.clone(),
                seat: LANE.into(),
                generation: 1,
                cli: "opencode".into(),
                model: Some("p/m1".into()),
                start_at: T0,
            })?;
            tx.add_round_session(&key, &bare)?;
            tx.close_round(&key, T0 + 30)
        })
        .unwrap();
    let row = round_of(&mut store, "T-10", 1, "dev");
    let cost = gear_engine::rounds::count_round(&mut store, &cfg, &row, T0 + 200)
        .unwrap()
        .cost_usd
        .unwrap();
    assert!((cost - 0.0001).abs() < 1e-10);
    // An open round is counted on demand without writing its columns.
    round(&mut store, "T-8", 1, "dev", T0);
    let key = RoundKey {
        project: "p".into(),
        task: "T-8".into(),
        attempt: 1,
        role: "dev".into(),
    };
    store.write(|tx| tx.add_round_session(&key, &a)).unwrap();
    let row = round_of(&mut store, "T-8", 1, "dev");
    assert_eq!(
        gear_engine::rounds::count_round(&mut store, &cfg, &row, T0 + 40)
            .unwrap()
            .input,
        Some(100)
    );
    assert_eq!(round_of(&mut store, "T-8", 1, "dev").input, None);
    let missing = file("unpriced", vec![rec(T0 + 10, 30, "p/unknown")]);
    round(&mut store, "T-9", 1, "dev", T0);
    let key = RoundKey {
        project: "p".into(),
        task: "T-9".into(),
        attempt: 1,
        role: "dev".into(),
    };
    store
        .write(|tx| tx.add_round_session(&key, &missing))
        .unwrap();
    let row = round_of(&mut store, "T-9", 1, "dev");
    let tokens = gear_engine::rounds::count_round(&mut store, &cfg, &row, T0 + 40).unwrap();
    assert_eq!(tokens.input, Some(30));
    assert_eq!(tokens.cost_usd, None);
    assert_eq!(tokens.cost_source, "none");
}

#[test]
fn closed_round_waits_for_late_logs_and_missing_logs_stay_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("later.jsonl");
    let mut store = world();
    round(&mut store, "T-1", 1, "dev", T0);
    let key = RoundKey {
        project: "p".into(),
        task: "T-1".into(),
        attempt: 1,
        role: "dev".into(),
    };
    store
        .write(|tx| {
            tx.add_round_session(&key, path.to_str().unwrap())?;
            tx.close_round(&key, T0 + 100)
        })
        .unwrap();
    let mut cfg = gear_engine::EngineCfg::default();
    let mut p = gear_herdr::builtin("opencode").unwrap();
    p.usage_log = Some("pi-jsonl".into());
    cfg.profiles.insert("opencode".into(), p);
    let mut h = FakeHerdr::new();
    gear_engine::rounds::tick(&mut store, &mut h, &cfg, T0 + 159).unwrap();
    assert_eq!(round_of(&mut store, "T-1", 1, "dev").counted_at, None);
    gear_engine::rounds::tick(&mut store, &mut h, &cfg, T0 + 160).unwrap();
    assert_eq!(round_of(&mut store, "T-1", 1, "dev").input, None);
    std::fs::write(path,format!("{}\n",serde_json::json!({"type":"message","timestamp":T0+20,"message":{"role":"assistant","usage":{"input":42,"cost":{"total":0.25}}}}))).unwrap();
    gear_engine::rounds::tick(&mut store, &mut h, &cfg, T0 + 219).unwrap();
    assert_eq!(round_of(&mut store, "T-1", 1, "dev").input, None);
    gear_engine::rounds::tick(&mut store, &mut h, &cfg, T0 + 220).unwrap();
    let row = round_of(&mut store, "T-1", 1, "dev");
    assert_eq!(row.input, Some(42));
    assert_eq!(row.cost_usd, Some(0.25));
    assert_eq!(row.cost_source.as_deref(), Some("agent"));
    assert_eq!(row.counted_at, Some(T0 + 220));
}

#[test]
fn closed_codex_round_keeps_its_reader_after_the_seat_moves_to_pi() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex.jsonl");
    let record = serde_json::json!({"type":"event_msg","timestamp":T0+20,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":0,"output_tokens":0}}}});
    std::fs::write(&path, format!("{record}\n")).unwrap();
    let mut store = world();
    let key = RoundKey {
        project: "p".into(),
        task: "T-20".into(),
        attempt: 1,
        role: "dev".into(),
    };
    store.write(|tx| {
        let mut project=tx.get_project("p")?.unwrap();
        project.config_toml="[profiles.pi]\nusage_log='pi-jsonl'\n[profiles.codex]\nusage_log='codex-rollout'\n[profiles.custom]\nkind='codex'\nusage_log='codex-rollout'\n".into();
        tx.update_project(&project)?;
        tx.insert_round(&NewRound {key:key.clone(),seat:LANE.into(),generation:1,cli:"codex".into(),model:None,start_at:T0})?;
        tx.add_round_session(&key,path.to_str().unwrap())?;
        tx.close_round(&key,T0+100)?;
        tx.set_seat_profile(LANE,"pi")
    }).unwrap();
    let mut cfg = gear_engine::EngineCfg::default();
    cfg.profiles = gear_herdr::builtins();
    let mut h = FakeHerdr::new();
    gear_engine::rounds::tick(&mut store, &mut h, &cfg, T0 + 160).unwrap();
    let row = round_of(&mut store, "T-20", 1, "dev");
    assert_eq!(row.input, Some(300));
    assert_eq!(row.counted_at, Some(T0 + 160));
    // A custom profile of the same CLI still applies its configured reader.
    store
        .write(|tx| tx.set_seat_profile(LANE, "custom"))
        .unwrap();
    assert_eq!(
        gear_engine::rounds::count_round(&mut store, &cfg, &row, T0 + 160)
            .unwrap()
            .input,
        Some(300)
    );
}

#[test]
fn codex_model_switch_prices_each_bare_id_and_rejects_ambiguity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("switch.jsonl");
    let context = |model| serde_json::json!({"type":"turn_context","timestamp":T0+1,"payload":{"model":model}});
    let count = |at, input| serde_json::json!({"type":"event_msg","timestamp":T0+at,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":0}}}});
    let lines = vec![context("m1"), count(10, 100), context("m2"), count(20, 300)];
    std::fs::write(
        &path,
        lines
            .into_iter()
            .map(|v| v.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    let mut store = world();
    let key = RoundKey {
        project: "p".into(),
        task: "T-21".into(),
        attempt: 1,
        role: "dev".into(),
    };
    store.write(|tx| {
        let mut project=tx.get_project("p")?.unwrap();
        project.config_toml="[prices.\"openai/m1\"]\ninput=1\noutput=0\ncache_read=0\ncache_write=0\n[prices.\"openai/m2\"]\ninput=3\noutput=0\ncache_read=0\ncache_write=0\n".into();
        tx.update_project(&project)?; tx.set_seat_profile(LANE,"codex")?;
        tx.insert_round(&NewRound{key:key.clone(),seat:LANE.into(),generation:1,cli:"codex".into(),model:Some("openai/m1".into()),start_at:T0})?;
        tx.add_round_session(&key,path.to_str().unwrap())?; tx.close_round(&key,T0+30)
    }).unwrap();
    let mut cfg = gear_engine::EngineCfg::default();
    cfg.profiles = gear_herdr::builtins();
    let row = round_of(&mut store, "T-21", 1, "dev");
    let tokens = gear_engine::rounds::count_round(&mut store, &cfg, &row, T0 + 100).unwrap();
    assert_eq!(tokens.input, Some(300));
    assert!((tokens.cost_usd.unwrap() - 0.0007).abs() < 1e-10);
    assert_eq!(tokens.cost_source, "prices");
    store
        .write(|tx| {
            let mut p = tx.get_project("p")?.unwrap();
            p.config_toml.push_str(
                "[prices.\"other/m2\"]\ninput=7\noutput=0\ncache_read=0\ncache_write=0\n",
            );
            tx.update_project(&p)
        })
        .unwrap();
    let tokens = gear_engine::rounds::count_round(&mut store, &cfg, &row, T0 + 100).unwrap();
    assert_eq!(tokens.input, Some(300));
    assert_eq!(tokens.cost_usd, None);
    assert_eq!(tokens.cost_source, "none");
}
