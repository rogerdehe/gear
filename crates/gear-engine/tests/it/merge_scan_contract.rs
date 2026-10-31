//! T-516: the merge step is near idle when nothing merges.
//!
//! Step 1b of `merge_passed` (`tell_captain_lost`) reads every done task of the
//! project — its attempts, its attempt times, two threads and two metas each —
//! to name work that sat accepted and unmerged. On a quiet fleet with hundreds
//! of done tasks across voyages that held a core (25-41 % on v0.6.4, `slow:
//! merge: 2100-3300 ms` a tick).
//!
//! The scan now runs once every `LOST_SCAN_EVERY_S` (300 s) per project, and at
//! once in the tick after a merge op of the project reached a final state
//! (`Confirmed` or `Failed`): a merge that settled can leave an attempt lost,
//! and the captain hears about it in the next tick.

use std::time::Instant;

use gear_engine::{merge::merge_passed, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_model::{AttemptState, OpKind, OpState, TaskState};
use gear_store::{testing, AttemptRow, NewOp, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const LOST_SCAN_EVERY_S: i64 = 300;

fn config_text() -> String {
    r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[conventions]
gate = "true"
"#
    .to_string()
}

/// A project with a captain and a lane. The branch sweep (a network call) is
/// pinned to a recent run, so a test sees the merge step and nothing else.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&gear_store::ProjectRow {
                id: "p".into(),
                herdr_session: "sess".into(),
                config_toml: config_text(),
                config_version: 1,
                quota_lanes: 1,
                quota_temps: 0,
                quota_includes_captain: false,
                plan_version: 0,
                created: T0,
            })?;
            testing::seed_active_seat(
                tx,
                "p",
                "heat",
                gear_model::SeatRole::Lane,
                "w1:p2",
                "opencode",
                T0,
            )?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                gear_model::SeatRole::Captain,
                "w9:p1",
                "claude",
                T0,
            )?;
            // The branch sweep waits 600 s; keep it out of the test's ticks.
            tx.set_meta("engine.branch_sweep.p", &T0.to_string())
        })
        .unwrap();
    store
}

/// A done task whose one attempt was accepted. With `merged`, the attempt
/// carries a merge time: its work landed. Without, it is work nothing merged —
/// what the lost scan names.
fn done_task(store: &mut Store, id: &str, merged: bool) {
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                project: "p".into(),
                id: id.into(),
                rank: 1,
                state: TaskState::Done,
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
                task: id.into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: Some(format!("c0ffee{id}")),
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            if merged {
                tx.set_attempt_time("p", id, 1, gear_store::AttemptStep::Merged, T0)?;
            }
            Ok(())
        })
        .unwrap();
}

/// The lost-merge items of a task.
fn lost_items(store: &mut Store, task: &str) -> Vec<gear_store::MessageRow> {
    store
        .read(|tx| tx.thread_messages("p", &format!("merge-lost:{task}")))
        .unwrap()
}

fn tick(store: &mut Store, herdr: &mut FakeHerdr, cfg: &EngineCfg, now: i64) {
    merge_passed(store, herdr, cfg, "p", now).unwrap();
}

/// T-516: two ticks 10 s apart run the lost scan once; a tick 300 s later runs
/// it again. A lost task added after the first scan is named only by the scan
/// that comes due.
#[test]
fn the_lost_scan_runs_once_every_five_minutes() {
    let mut store = world();
    done_task(&mut store, "T-1", false);
    let mut herdr = FakeHerdr::new();
    let cfg = EngineCfg::default();

    // The first tick scans and names T-1.
    tick(&mut store, &mut herdr, &cfg, T0);
    assert_eq!(lost_items(&mut store, "T-1").len(), 1, "the scan named T-1");

    // More work is lost after the scan: the next tick, 10 s on, does not read
    // the tasks again.
    done_task(&mut store, "T-2", false);
    tick(&mut store, &mut herdr, &cfg, T0 + 10);
    assert!(
        lost_items(&mut store, "T-2").is_empty(),
        "the scan is not due 10 s after its last run"
    );

    // 300 s after the last run it is due again and names T-2.
    tick(&mut store, &mut herdr, &cfg, T0 + LOST_SCAN_EVERY_S);
    assert_eq!(
        lost_items(&mut store, "T-2").len(),
        1,
        "the scan ran when it came due"
    );
}

/// T-516: a merge op of the project that just reached a final state brings the
/// scan at once, in the tick after — for both `Confirmed` and `Failed`.
#[test]
fn a_settled_merge_op_brings_the_next_scan_at_once() {
    for state in [OpState::Confirmed, OpState::Failed] {
        let mut store = world();
        done_task(&mut store, "T-1", true);
        let mut herdr = FakeHerdr::new();
        let cfg = EngineCfg::default();

        // The scan runs at T0 and finds nothing lost.
        tick(&mut store, &mut herdr, &cfg, T0);
        done_task(&mut store, "T-2", false);

        // A merge op for another task settles 5 s later.
        store
            .write(|tx| {
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Merge,
                    task: Some("T-9".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "branch": "heat/t9", "candidate": "c0ffee" }),
                    created: T0 + 5,
                })?;
                tx.set_op_state("p", "O-1", state, Some("done"), T0 + 5)
            })
            .unwrap();

        // The next tick — long before the 300 s — scans again.
        tick(&mut store, &mut herdr, &cfg, T0 + 10);
        assert_eq!(
            lost_items(&mut store, "T-2").len(),
            1,
            "a {state:?} merge op brought the scan at once"
        );
    }
}

/// T-516: on a store with 500 done tasks, an idle `merge_passed` — no merge op,
/// nothing changed — costs under 100 ms once the scan has run.
#[test]
fn an_idle_merge_step_on_five_hundred_done_tasks_is_under_a_hundred_milliseconds() {
    let mut store = world();
    for n in 0..500 {
        done_task(&mut store, &format!("T-{n}"), true);
    }
    let mut herdr = FakeHerdr::new();
    let cfg = EngineCfg::default();

    // The first tick runs the scan (this one may take as long as the reading
    // of 500 tasks needs) and writes the scan's time.
    tick(&mut store, &mut herdr, &cfg, T0);

    // The next tick, before the scan is due, must not read the tasks again.
    let started = Instant::now();
    tick(&mut store, &mut herdr, &cfg, T0 + 10);
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "an idle merge step took {elapsed:?} on 500 done tasks"
    );
}
