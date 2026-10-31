//! Contract for deliveries (T-39, design/08–09): text templates, Deliver op
//! intent, and `execute_intended` draining Deliver ops like doorbells.

use gear_engine::dispatch::{assign_text, continue_text, DeliverAction};
use gear_engine::ops::{execute_intended, intend_deliveries};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::{validate_template, AgentStatus, Profile};
use gear_model::{OpKind, OpState, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const LANE_PANE: &str = "w1:p2";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn texts_match_design_09() {
    assert_eq!(continue_text("T-17", 4, "ops/tasks/T-17.md"),
        "Continue T-17 (contract v4). Run `gear` first (it records that you got this). Task packet: ops/tasks/T-17.md. Report progress/blocked/done with gear send.");
    assert_eq!(assign_text("T-31", 1, "heat/t31-login", "ops/tasks/T-31.md"),
        "New task T-31 (contract v1). Branch: heat/t31-login (in your worktree). Run `gear` first (it records that you got this). Task packet: ops/tasks/T-31.md. Read it before starting; report with gear send. After your result, end your turn: the next task comes when you are idle.");
}

#[test]
fn texts_carry_no_screen_marker() {
    let p = Profile {
        kind: "opencode".into(),
        new_session: String::new(),
        compact: String::new(),
        usage_log: None,
        error_markers: Vec::new(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec!["rate limit".into()],
        quota_markers: vec![
            "Session failed".into(),
            "Token Plan".into(),
            "usage limit reached".into(),
        ],
        quota_bar: None,
        auth_markers: vec!["Unauthorized".into()],
        backoff_markers: vec![],
        blocked_markers: vec![],
        tail_lines: 16,
        confirm_rounds: 2,
        unattended_args: vec!["--auto".into()],
        instructions_args: vec![],
        session_ref: false,
        resume_args: None,
        exit_keys: vec![],
        exit_command: None,
        verified: true,
    };
    validate_template(&p, &continue_text("T-1", 1, "x.md")).unwrap();
    validate_template(&p, &assign_text("T-1", 1, "a/t1-x", "x.md")).unwrap();
}

#[test]
fn delivery_is_intended_then_executed() {
    let mut s = world();
    let text = continue_text("T-1", 1, "ops/tasks/T-1.md");
    let ops = s
        .write(|tx| {
            intend_deliveries(
                tx,
                &[DeliverAction {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    task: "T-1".into(),
                    attempt: 2,
                    text: text.clone(),
                }],
                T0,
            )
        })
        .unwrap();
    assert_eq!(ops.len(), 1);
    let op = &ops[0];
    assert_eq!(
        (op.kind, op.state, op.task.as_deref(), op.attempt),
        (OpKind::Deliver, OpState::Intended, Some("T-1"), Some(2))
    );
    assert_eq!(op.payload["text"], text.as_str());
    // the attempt the delivery is for exists and is the seat's open attempt (T-44, T-47)
    s.write(|tx| {
        tx.insert_attempt(&gear_store::AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: "T-1".into(),
            n: 2,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: gear_model::AttemptState::Delivered,
            candidate: None,
            report: None,
            safe_point: None,
            started: T0,
            updated: T0,
        })
    })
    .unwrap();
    let mut herdr = FakeHerdr::new();
    // The agent carries the seat's name (T-149: a delivery aims at the live
    // agent name; an agent that lost its name is renamed and the packet goes
    // on the next tick — see `targeting_contract.rs`).
    herdr.add_agent_named(LANE_PANE, "opencode", AgentStatus::Idle, "p-heat");
    let done = execute_intended(&mut s, &mut herdr, "p", T0 + 1).unwrap();
    assert_eq!(done, vec![(op.id.clone(), "submitted".to_string())]);
    assert_eq!(herdr.prompts_to(LANE_PANE), vec![text.as_str()]);
}
