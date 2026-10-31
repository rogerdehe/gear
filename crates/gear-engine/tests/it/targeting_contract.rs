//! T-149: never type into the wrong thing (design §10.1).
//!
//! Three rules, one fake Herdr:
//! - a prompt aims at the live **agent name**, never at the pane id: a pane
//!   that lost its agent refuses the text instead of typing it into a shell;
//! - when Herdr refuses, `agent_get` decides: an agent of the seat's kind is
//!   there → it lost its name (M3.1), it is renamed and the prompt is retried
//!   next tick, nothing is restarted; no agent in the pane → the delivery
//!   fails at once as `agent-gone`;
//! - `pane_run` (a shell command) runs only in a pane that hosts no agent.

use gear_engine::dispatch::DeliverAction;
use gear_engine::ops::{execute_intended, intend_deliveries};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::{AgentStatus, HerdrAct, Outcome};
use gear_model::{OpState, SeatRole};
use gear_store::{testing, AttemptRow, Store};

const T0: i64 = 1_800_000_000;
const LANE_PANE: &str = "w1:p2";
const TEXT: &str = "New task T-1 (contract v1). Task packet: gear --show T-1.";

/// A project with one lane (`heat`, opencode) whose agent runs in `LANE_PANE`.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// A Deliver op for the lane's open attempt: the packet the engine types.
fn intended_delivery(store: &mut Store) -> String {
    let op = store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
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
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            let ops = intend_deliveries(
                tx,
                &[DeliverAction {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    task: "T-1".into(),
                    attempt: 1,
                    text: TEXT.into(),
                }],
                T0,
            )?;
            Ok(ops[0].id.clone())
        })
        .unwrap();
    op
}

/// The state of the op, and the reason the engine stored with it.
fn op_state(store: &mut Store, id: &str) -> (OpState, Option<String>) {
    let op = store.read(|tx| tx.get_op("p", id)).unwrap().unwrap();
    (op.state, op.result)
}

/// T-149: a prompt to an unnamed live agent renames it and retries next tick
/// without a restart.
#[test]
fn a_prompt_to_an_unnamed_live_agent_renames_it_and_retries_next_tick() {
    let mut store = world();
    let id = intended_delivery(&mut store);
    let mut herdr = FakeHerdr::new();
    // Herdr cleared the name of the agent (the M3.1 state after a restart).
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    assert_eq!(herdr.agent_name(LANE_PANE), None);

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 10).unwrap();

    // The agent is named again — and nothing is typed into it yet.
    assert_eq!(herdr.agent_name(LANE_PANE), Some("p-heat"));
    assert_eq!(
        herdr.renames_to("p-heat"),
        vec![LANE_PANE],
        "one rename, to the seat's name"
    );
    assert!(
        herdr.prompts_to(LANE_PANE).is_empty(),
        "nothing is typed while the agent has no name: {:?}",
        herdr.prompts_to(LANE_PANE)
    );
    // The op waits for the next tick; it is neither submitted nor failed.
    assert_eq!(done, vec![(id.clone(), "intended".to_string())]);
    assert_eq!(op_state(&mut store, &id).0, OpState::Intended);
    // Nothing was restarted: the agent of the pane is the same one.
    assert!(
        herdr.started_agents.is_empty(),
        "no restart: {:?}",
        herdr.started_agents
    );

    // The next tick: the packet goes out, aimed at the agent's name.
    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 20).unwrap();

    assert_eq!(done, vec![(id.clone(), "submitted".to_string())]);
    assert_eq!(op_state(&mut store, &id).0, OpState::Submitted);
    assert_eq!(herdr.prompts_to(LANE_PANE), vec![TEXT]);
    assert_eq!(
        herdr.prompt_targets(),
        vec!["p-heat"],
        "the text aims at the agent name, never at the pane id"
    );
}

/// T-149: a prompt to a pane with only a shell fails as agent-gone and types
/// nothing.
#[test]
fn a_prompt_to_a_pane_with_only_a_shell_fails_as_agent_gone() {
    let mut store = world();
    let id = intended_delivery(&mut store);
    let mut herdr = FakeHerdr::new();
    // The agent is gone: the pane is a shell again.
    herdr.add_shell(LANE_PANE);

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 10).unwrap();

    assert_eq!(done, vec![(id.clone(), "failed".to_string())]);
    let (state, reason) = op_state(&mut store, &id);
    assert_eq!(state, OpState::Failed);
    let reason = reason.unwrap_or_default();
    assert!(
        reason.contains("agent-gone"),
        "the reason names it: {reason}"
    );
    assert!(
        herdr.prompts_to(LANE_PANE).is_empty(),
        "nothing is typed into a shell: {:?}",
        herdr.prompts_to(LANE_PANE)
    );
    assert!(herdr.prompt_targets().is_empty(), "no prompt at all");
    // F2 of the T-407 review: a delivery that fails while it is aimed logs its
    // own `failed` and `done` lines, as a confirmed one logs its own.
    let log: Vec<String> = gear_engine::ops::probe_log()
        .into_iter()
        .filter(|line| line.starts_with("probe deliver T-1 "))
        .collect();
    assert!(
        log.contains(&"probe deliver T-1 failed 10000 total=10000".to_string()),
        "the failed delivery is logged: {log:?}"
    );
    assert!(
        log.contains(&"probe deliver T-1 done 10000 total=10000".to_string()),
        "the failed delivery has its completion line: {log:?}"
    );
}

/// T-149: a pane that hosts a foreign agent is not the seat's agent either.
#[test]
fn a_prompt_to_a_pane_with_a_foreign_agent_fails_as_agent_gone() {
    let mut store = world();
    let id = intended_delivery(&mut store);
    let mut herdr = FakeHerdr::new();
    // Another CLI took the pane.
    herdr.add_agent_named(LANE_PANE, "codex", AgentStatus::Idle, "other-agent");

    let done = execute_intended(&mut store, &mut herdr, "p", T0 + 10).unwrap();

    assert_eq!(done, vec![(id.clone(), "failed".to_string())]);
    let reason = op_state(&mut store, &id).1.unwrap_or_default();
    assert!(
        reason.contains("agent-gone"),
        "the reason names it: {reason}"
    );
    assert!(
        herdr.prompts_to(LANE_PANE).is_empty(),
        "the foreign agent is not typed into: {:?}",
        herdr.prompts_to(LANE_PANE)
    );
}

/// T-149: `pane_run` is a shell command — it runs only where no agent runs.
#[test]
fn pane_run_with_an_agent_present_is_refused() {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(LANE_PANE, "opencode", AgentStatus::Idle, "p-heat");

    let outcome = herdr.pane_run(LANE_PANE, "export GEAR_CTX=gear:keel:2");

    match &outcome {
        Outcome::Refused { code, .. } => assert_eq!(code, "agent_present", "{outcome:?}"),
        other => panic!("a pane with an agent refuses a shell command: {other:?}"),
    }
    // A refusal proves nothing was typed (the call log holds the attempt).
    assert!(
        herdr
            .calls
            .iter()
            .all(|call| matches!(call, gear_herdr::fake::Call::PaneRun { .. })),
        "only the refused attempt is in the log: {:?}",
        herdr.calls
    );
}

/// T-149: the same command in a pane without an agent still runs.
#[test]
fn pane_run_in_a_pane_without_an_agent_runs() {
    let mut herdr = FakeHerdr::new();
    herdr.add_shell(LANE_PANE);

    let outcome = herdr.pane_run(LANE_PANE, "export GEAR_CTX=gear:keel:2");

    assert!(matches!(outcome, Outcome::Ok(())), "{outcome:?}");
    assert!(
        herdr
            .calls
            .iter()
            .any(|call| matches!(call, gear_herdr::fake::Call::PaneRun { .. })),
        "the command went to the shell: {:?}",
        herdr.calls
    );
}

/// The review of attempt 1, F2: a refused prompt to a **live named** agent
/// (a transient refusal right after an agent start is an ordinary Herdr
/// condition) renames the agent through the naming and leaves the op Intended
/// for the next tick — the decision is `agent_get`'s alone.
#[test]
fn a_refused_prompt_to_a_live_agent_is_renamed_and_retried_next_tick() {
    let mut store = world();
    let id = intended_delivery(&mut store);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(LANE_PANE, "opencode", AgentStatus::Idle, "p-heat");
    herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse {
        code: "agent_not_ready".into(),
    });

    execute_intended(&mut store, &mut herdr, "p", T0 + 10).unwrap();

    assert_eq!(
        op_state(&mut store, &id).0,
        OpState::Intended,
        "the op waits for the next tick: {:?}",
        op_state(&mut store, &id)
    );
    assert!(
        herdr.started_agents.is_empty(),
        "nothing is restarted: {:?}",
        herdr.started_agents
    );

    // The next tick: the packet goes out, aimed at the name. The call log
    // holds the refused attempt too — a refusal proves nothing was typed.
    execute_intended(&mut store, &mut herdr, "p", T0 + 20).unwrap();
    assert_eq!(op_state(&mut store, &id).0, OpState::Submitted);
    assert_eq!(
        herdr.prompts_to(LANE_PANE),
        vec![TEXT, TEXT],
        "the refused attempt and the packet"
    );
    assert_eq!(
        herdr.prompt_targets(),
        vec!["p-heat", "p-heat"],
        "both by the name"
    );
}

/// The review of attempt 1, F3: the sender has no exception. An unnamed agent
/// is named again and the op stays Intended: nothing is typed until the next
/// tick, and the text goes out by name then.
#[test]
fn the_sender_waits_for_the_next_tick_when_the_agent_has_no_name() {
    let mut store = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);

    gear_engine::ops::deliver_doorbell_now(
        &mut store,
        &mut herdr,
        "p",
        "p:heat",
        1,
        "M-1",
        true,
        T0 + 10,
    )
    .unwrap();

    assert!(
        herdr.prompt_targets().is_empty(),
        "nothing is typed in this call: {:?}",
        herdr.prompt_targets()
    );
    assert_eq!(
        herdr.agent_name(LANE_PANE),
        Some("p-heat"),
        "the agent is named"
    );
    let op = store
        .read(|tx| tx.ops_of_seat_kind("p:heat", gear_model::OpKind::Doorbell))
        .unwrap();
    assert_eq!(op.len(), 1, "{op:?}");
    assert_eq!(op[0].state, OpState::Intended, "the op waits: {op:?}");

    // The next tick: the doorbell goes out, aimed at the name.
    execute_intended(&mut store, &mut herdr, "p", T0 + 20).unwrap();
    assert_eq!(herdr.prompt_targets(), vec!["p-heat"]);
    assert!(
        herdr.prompts_to(LANE_PANE)[0].contains("M-1"),
        "{:?}",
        herdr.prompts_to(LANE_PANE)
    );
}
