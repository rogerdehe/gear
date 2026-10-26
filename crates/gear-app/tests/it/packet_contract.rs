//! Contract for gear_app::plan::render_task_packet (T-53).

use gear_app::plan::render_task_packet;
use gear_app::tasks::start_attempt;
use gear_model::{ErrorCode, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

fn world(attempt: bool) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        tx.insert_task(&TaskRow { project: "p".into(), id: "T-7".into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
            lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![], followup_of: None,
            wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: T0 })?;
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 1, goal: "Fix the login crash".into(),
            accept: "repro passes".into(), evidence_files: vec!["notes/T-7.md".into()], evidence_due_min: Some(60), body: String::new(), created: T0 })?;
        if attempt { start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?; }
        Ok(())
    }).unwrap();
    s
}

#[test]
fn packet_has_contract_and_copyable_commands() {
    let mut s = world(true);
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    for needle in [
        "# T-7 — Fix the login crash",
        "Lane: heat · Contract: v1 · State: ready",
        "## Goal", "## Acceptance", "repro passes", "## Evidence", "notes/T-7.md",
        "gear send --kind progress",
        "gear send --kind result --task T-7 --attempt 1 --contract 1 --candidate <commit> \"Report: <path>\"",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in\n{text}");
    }
    assert!(!text.contains("Pending update"));
}

#[test]
fn pending_update_and_missing_task() {
    let mut s = world(true);
    s.write(|tx| {
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 2, goal: "Fix it better".into(),
            accept: "a2".into(), evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0 + 1 })?;
        let mut t = tx.get_task("p", "T-7")?.unwrap();
        t.contract_version = 2;
        tx.update_task(&t)
    }).unwrap();
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(text.contains("# T-7 — Fix the login crash"), "rendered from the adopted v1:\n{text}");
    assert!(text.contains("## Pending update: v2"), "{text}");
    assert_eq!(s.read(|tx| render_task_packet(tx, "p", "T-9")).unwrap_err().code, ErrorCode::NoTarget);
    let mut s = world(false);
    assert!(s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap().contains("--attempt <n>"));
}
