//! T-453: a delivery counts as landed only when the agent shows it took it.
//!
//! The engine proves a delivery from the attempt's first check-in, or from the
//! agent's idle-to-busy turn after the text was submitted. The text is typed
//! into an empty composer (a command left in the box is submitted before the
//! packet follows it), a text typed into a busy agent has its window paused
//! while the agent works and starts when it is idle again, and a delivery that
//! never lands gets one Enter, one resend, then the captain's item.

use gear_engine::{session::prepare_lane, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, NewOp, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";
const PACKET: &str = "Continue T-1: run `gear` first";

fn seed(store: &mut Store, kind: &str, state: TaskState) {
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, kind, T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state,
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
                task: "T-1".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            Ok(())
        })
        .unwrap();
}

/// A project whose lane has an attempt waiting for its check-in and a packet
/// on its way: the delivery is Submitted at `T0 + 1`.
fn delivered_world(kind: &str) -> (Store, FakeHerdr, EngineCfg) {
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store, kind, TaskState::Ready);
    store
        .write(|tx| {
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": PACKET }),
                created: T0 + 1,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 1)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 1)
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(CAP, "opencode", AgentStatus::Idle, "p-captain");
    herdr.add_agent_named(LANE, kind, AgentStatus::Idle, "p-heat");
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        cooldown_s: 0,
        delivery_grace_s: 8,
        ..Default::default()
    };
    (store, herdr, cfg)
}

fn delivery(store: &mut Store) -> gear_store::OpRow {
    store
        .read(|tx| {
            tx.ops_of_seat_kind("p:heat", OpKind::Deliver)?
                .into_iter()
                .next_back()
                .ok_or_else(|| {
                    gear_model::GearError::new(gear_model::ErrorCode::NoTarget, "no delivery")
                })
        })
        .unwrap()
}

fn stall_items(store: &mut Store) -> Vec<gear_store::MessageRow> {
    store
        .read(|tx| tx.thread_messages("p", "stall:p:heat"))
        .unwrap()
}

/// The first shape: the packet was typed and its Enter was lost. The text sits
/// in the composer; after its window one Enter is given, and the agent's
/// idle-to-busy turn lands the attempt (a late check-in) — no captain item.
#[test]
fn a_text_left_in_the_composer_gets_its_enter_and_lands_on_the_turn() {
    let (mut store, mut herdr, cfg) = delivered_world("pi");
    // The pane shows the packet in its input box.
    herdr.set_screen(LANE, &format!("> {PACKET}"));
    // The Enter submits it: the composer empties.
    herdr.screen_after = vec![("enter".into(), "> \n  ? for shortcuts".into())];

    // The window is over and the needle is still in the composer: one Enter
    // is given before the delivery fails. Nobody is rung: the retry may still
    // come.
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 40).unwrap();
    assert!(
        stall_items(&mut store).is_empty(),
        "a delivery inside its retry opens no item"
    );
    assert!(!report.flagged.contains(&"p:heat".to_string()));
    assert_eq!(
        herdr
            .calls
            .iter()
            .filter(|call| matches!(call, gear_herdr::fake::Call::SendKeys { keys, .. } if keys == &["enter".to_string()]))
            .count(),
        1,
        "the lost Enter is given once"
    );
    assert!(
        herdr.prompts_to(LANE).is_empty(),
        "the text is in the box: it is not typed again"
    );
    assert_eq!(delivery(&mut store).state, OpState::Failed);

    // The Enter took: the agent starts its turn. The turn is a late check-in:
    // the attempt starts and the lane is not stuck.
    herdr.set_status(LANE, AgentStatus::Working);
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 45).unwrap();
    assert_eq!(report.late_confirmed.len(), 1, "{report:?}");
    assert_eq!(
        store
            .read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Started
    );
    assert!(
        stall_items(&mut store).is_empty(),
        "the lane showed it took the packet"
    );
}

/// The second shape: `/new` was typed and its Enter was lost. The packet is
/// not typed until the command left the composer, and the command is not
/// typed twice.
#[test]
fn a_command_left_in_the_composer_holds_the_packet_back() {
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store, "codex", TaskState::Ready);
    store
        .write(|tx| {
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(LANE, "codex", AgentStatus::Idle, "p-heat");
    // The composer starts empty.
    herdr.set_screen(LANE, "> \n  ? for shortcuts");
    // The first Enter is lost: `/new` still shows in the composer.
    herdr.screen_after = vec![
        ("/new".into(), "> /new\n  ? for shortcuts".into()),
        ("enter".into(), "> \n  ? for shortcuts".into()),
    ];
    let mut cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();

    // The command is typed with its Enter, but the composer still holds it:
    // the step waits and the packet is not typed.
    let prep = prepare_lane(&mut store, &mut herdr, &cfg, &seat, "T-1", 1, T0 + 1).unwrap();
    assert_eq!(prep, gear_engine::session::Prep::Wait);
    let state = store
        .read(|tx| tx.get_meta("engine.session_step.p:heat"))
        .unwrap()
        .unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["step"], "enter", "the Enter did not take: {state}");

    // The next tick submits it again; the composer empties and the step moves
    // to waiting for the new conversation.
    let prep = prepare_lane(&mut store, &mut herdr, &cfg, &seat, "T-1", 1, T0 + 2).unwrap();
    assert_eq!(prep, gear_engine::session::Prep::Wait);
    let state: serde_json::Value = serde_json::from_str(
        &store
            .read(|tx| tx.get_meta("engine.session_step.p:heat"))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(state["step"], "wait_action", "{state}");
    assert_eq!(
        herdr
            .calls
            .iter()
            .filter(|call| matches!(call, gear_herdr::fake::Call::PaneType { text, .. } if text == "/new"))
            .count(),
        1,
        "the command is typed once"
    );
    // The codex conversation starts empty, so the step goes on: the packet
    // will be typed into an empty composer.
    assert_eq!(
        prepare_lane(&mut store, &mut herdr, &cfg, &seat, "T-1", 1, T0 + 8).unwrap(),
        gear_engine::session::Prep::Ready
    );
}

/// The third shape: a delivery typed into a busy agent. Its window is paused
/// while the agent works and starts when it is idle again; no captain item is
/// raised before that, and the turn that takes the queued text lands it.
#[test]
fn a_delivery_typed_into_a_busy_agent_is_not_a_stall() {
    let (mut store, mut herdr, cfg) = delivered_world("pi");
    // The text went into the working agent's input queue: the engine recorded
    // it when it typed (T-453).
    let op = delivery(&mut store);
    store
        .write(|tx| {
            tx.set_meta(&format!("engine.deliver_working.{}", op.id), "1")?;
            tx.set_meta(
                &format!("engine.deliver_submitted.{}", op.id),
                &(T0 + 1).to_string(),
            )
        })
        .unwrap();
    herdr.set_status(LANE, AgentStatus::Working);

    // The agent works far past the window: the delivery still waits, no item.
    for at in [T0 + 10, T0 + 300] {
        let report = tick(&mut store, &mut herdr, &cfg, at).unwrap();
        assert!(
            stall_items(&mut store).is_empty(),
            "no item while the text waits behind the working agent"
        );
        assert!(!report.flagged.contains(&"p:heat".to_string()));
        assert_eq!(delivery(&mut store).state, OpState::Submitted);
    }

    // The agent is idle again: the window starts now, not at the intent.
    herdr.set_status(LANE, AgentStatus::Idle);
    tick(&mut store, &mut herdr, &cfg, T0 + 301).unwrap();
    assert_eq!(delivery(&mut store).state, OpState::Submitted);
    assert!(stall_items(&mut store).is_empty());

    // The queued text starts a turn: the delivery landed.
    herdr.set_status(LANE, AgentStatus::Working);
    tick(&mut store, &mut herdr, &cfg, T0 + 303).unwrap();
    let op = delivery(&mut store);
    assert_eq!(op.state, OpState::Confirmed, "{op:?}");
    assert!(
        op.result.as_deref().unwrap_or("").contains("turn"),
        "{op:?}"
    );
    assert_eq!(
        store
            .read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Started
    );
    assert!(stall_items(&mut store).is_empty());
}

/// The fourth shape: a fresh Codex conversation that starts empty. The packet
/// is typed only after `/new` left the composer, and the turn that reads it
/// lands the delivery without a check-in.
#[test]
fn a_fresh_codex_conversation_lands_the_packet_on_its_turn() {
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store, "codex", TaskState::Ready);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(CAP, "opencode", AgentStatus::Idle, "p-captain");
    herdr.add_agent_named(LANE, "codex", AgentStatus::Idle, "p-heat");
    // The fresh conversation starts empty, and a prompt starts its turn.
    herdr.set_screen(LANE, "> \n  ? for shortcuts");
    herdr.screen_after = vec![("/new".into(), "> \n  ? for shortcuts".into())];
    herdr.prompt_starts_turn = true;
    let mut cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        cooldown_s: 0,
        delivery_grace_s: 8,
        ..Default::default()
    };
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());

    // `/new` is typed and entered; the session step waits for the empty
    // conversation, so no packet is typed yet.
    tick(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert!(
        herdr.calls.iter().any(
            |call| matches!(call, gear_herdr::fake::Call::PaneType { text, .. } if text == "/new")
        ),
        "the new session is typed"
    );
    assert!(
        herdr.prompts_to(LANE).is_empty(),
        "the packet waits for the empty composer"
    );

    // The session is ready: the packet is typed into the empty conversation,
    // and the prompt starts the agent's turn.
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    let prompts = herdr.prompts_to(LANE);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("New task T-1"), "{prompts:?}");
    let op = delivery(&mut store);
    assert_eq!(op.state, OpState::Submitted, "{op:?}");

    // The turn is the agent taking the packet: the delivery lands.
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 15).unwrap();
    assert!(report.confirmed.contains(&op.id));
    let op = delivery(&mut store);
    assert_eq!(op.state, OpState::Confirmed, "{op:?}");
    assert!(
        op.result.as_deref().unwrap_or("").contains("turn"),
        "{op:?}"
    );
    assert_eq!(
        store
            .read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Started
    );
    assert!(stall_items(&mut store).is_empty());
}

/// A delivery that never lands gets its Enter, then one resend, then the
/// captain's item — and no item while it is inside its window.
#[test]
fn a_lost_delivery_is_resent_once_and_then_rung() {
    let (mut store, mut herdr, mut cfg) = delivered_world("pi");
    cfg.delivery_grace_s = 8;

    // Inside the window: nothing is rung and nothing is resent.
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 5).unwrap();
    assert!(stall_items(&mut store).is_empty());
    assert!(!report.flagged.contains(&"p:heat".to_string()));

    // The window is over: the delivery fails, the captain is not rung yet —
    // the one automatic retry of the lane's own path may still come.
    tick(&mut store, &mut herdr, &cfg, T0 + 20).unwrap();
    assert_eq!(delivery(&mut store).state, OpState::Failed);
    assert!(stall_items(&mut store).is_empty());

    // The retry goes once.
    tick(&mut store, &mut herdr, &cfg, T0 + 130).unwrap();
    let retries: Vec<_> = store
        .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Deliver))
        .unwrap()
        .into_iter()
        .filter(|op| op.payload.get("redeliver").is_some())
        .collect();
    assert_eq!(retries.len(), 1, "{retries:?}");
    assert!(stall_items(&mut store).is_empty());

    // The retry fails too: one item, once.
    tick(&mut store, &mut herdr, &cfg, T0 + 145).unwrap();
    let items = stall_items(&mut store);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, MsgState::Open);
    tick(&mut store, &mut herdr, &cfg, T0 + 165).unwrap();
    assert_eq!(stall_items(&mut store).len(), 1, "one item only");
}
