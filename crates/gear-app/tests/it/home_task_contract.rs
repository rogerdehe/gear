//! Contract: the lane home page shows and records the current task (T-51).

use gear_app::home::build;
use gear_app::identity::Fingerprint;
use gear_app::tasks::start_attempt;
use gear_app::Env;
use gear_model::{GearCtx, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const LANE: &str = "w1:p2";

fn lane() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: "heat".into(),
                generation: 1,
            }),
            pane_id: Some(LANE.into()),
            admin: false,
        },
        Fingerprint {
            pane_id: LANE.into(),
            kind: Some("opencode".into()),
            session_ref: None,
        },
    )
}

fn world(with_attempt: bool) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T0)?;
        tx.insert_task(&TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: "T-7".into(),
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
            updated: T0,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-7".into(),
            version: 1,
            goal: "Fix the login crash".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
            mechanical: None,
        })?;
        if with_attempt {
            start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
        }
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn current_task_is_shown_and_recorded() {
    let mut s = world(true);
    let (env, fp) = lane();
    let home = build(&mut s, &env, Some(&fp), T0 + 10).unwrap();
    assert!(
        home.text
            .contains("task T-7 attempt 1 contract v1 — Fix the login crash"),
        "{}",
        home.text
    );
    assert_eq!(
        (
            home.view.shown_task.as_deref(),
            home.view.shown_attempt,
            home.view.shown_contract
        ),
        (Some("T-7"), Some(1), Some(1))
    );
}

#[test]
fn no_open_attempt_no_task_line() {
    let mut s = world(false);
    let (env, fp) = lane();
    let home = build(&mut s, &env, Some(&fp), T0 + 10).unwrap();
    assert!(!home.text.contains("task T-7"), "{}", home.text);
    assert_eq!(
        (
            home.view.shown_task,
            home.view.shown_attempt,
            home.view.shown_contract
        ),
        (None, None, None)
    );
}
