//! T-97: an unconfirmed delivery never holds a seat forever (live acceptance,
//! multi T-3).
//!
//! A delivery Herdr accepted is not a task the lane received. Past its check
//! budget the op now fails with the reason (the seat's slot is free), the
//! captain gets one item instead of a flag every tick, and the open attempt is
//! delivered again once the pane reports a new agent session — a hand restart
//! needs no store edit.

use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, OpRow, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

fn world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
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
            })?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "opencode", AgentStatus::Idle);
    herdr.add_agent(LANE, "opencode", AgentStatus::Idle);
    let mut cfg = EngineCfg { doorbell_check_budget: 2, cooldown_s: 5, ..Default::default() };
    cfg.profiles
        .insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
    World { store, herdr, cfg }
}

impl World {
    fn tick(&mut self, at: i64) {
        tick(&mut self.store, &mut self.herdr, &self.cfg, T0 + at).unwrap();
    }

    /// Every Deliver op of the lane, in any state, oldest first.
    fn deliveries(&mut self) -> Vec<OpRow> {
        let mut ops: Vec<OpRow> = self
            .store
            .read(|tx| {
                let mut out = Vec::new();
                for state in [
                    OpState::Intended,
                    OpState::Executing,
                    OpState::Submitted,
                    OpState::Unknown,
                    OpState::Confirmed,
                    OpState::Failed,
                    OpState::Void,
                ] {
                    for op in tx.ops_in_state("p", state)? {
                        if op.kind == OpKind::Deliver && op.seat == "p:heat" {
                            out.push(op);
                        }
                    }
                }
                Ok(out)
            })
            .unwrap();
        ops.sort_by_key(|op| op.created);
        ops
    }

    /// The captain's items about the lane being stuck.
    fn stall_items(&mut self) -> Vec<gear_store::MessageRow> {
        self.store
            .read(|tx| tx.thread_messages("p", "stall:p:heat"))
            .unwrap()
    }
}

#[test]
fn an_unconfirmed_delivery_fails_once_and_goes_again_in_a_new_session() {
    let mut w = world();

    // Tick 1: the task is delivered. Herdr takes the prompt; no view follows.
    w.tick(1);
    let ops = w.deliveries();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].state, OpState::Submitted);
    assert_eq!(w.herdr.prompts_to(LANE).len(), 1);

    // Ticks 2–3: the budget (2) is spent. The op fails with the reason — the
    // seat's slot is free — and the captain gets exactly one item.
    w.tick(5);
    w.tick(9);
    let ops = w.deliveries();
    assert_eq!(ops.len(), 1, "no new delivery while the record blocks: {ops:?}");
    assert_eq!(ops[0].state, OpState::Failed, "{ops:?}");
    let reason = ops[0].result.clone().unwrap_or_default();
    assert!(reason.contains("not confirmed after 2 checks"), "{reason}");
    assert!(reason.contains("T-1 attempt 1"), "{reason}");
    let items = w.stall_items();
    assert_eq!(items.len(), 1, "one captain item: {items:?}");
    assert_eq!(items[0].state, MsgState::Open);
    assert!(w.herdr.prompts_to(LANE).len() == 1, "the prompt is not repeated");

    // Ticks 4–5: the same session. Nothing is typed into it again.
    w.tick(13);
    w.tick(17);
    assert_eq!(w.herdr.prompts_to(LANE).len(), 1, "the same session is not re-typed");
    assert_eq!(w.deliveries().len(), 1, "{:?}", w.deliveries());
    assert_eq!(w.stall_items().len(), 1, "no second item");

    // The captain restarts the agent by hand: the pane reports a new session.
    w.herdr.set_agent_session(LANE, "ses_hand_restart");
    w.tick(21);
    let ops = w.deliveries();
    assert_eq!(ops.len(), 2, "one new delivery, not two: {ops:?}");
    assert_eq!(ops[1].state, OpState::Submitted);
    assert_eq!(ops[1].attempt, Some(1));
    let prompts = w.herdr.prompts_to(LANE);
    assert_eq!(prompts.len(), 2, "delivered again: {prompts:?}");
    assert!(prompts[1].contains("Continue T-1"), "same attempt: {prompts:?}");

    // The lane reads it: the new delivery is confirmed and the attempt starts.
    w.store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                ts: T0 + 22,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: Some("T-1".into()),
                shown_attempt: Some(1),
                shown_contract: Some(1),
            })
        })
        .unwrap();
    w.tick(25);

    let ops = w.deliveries();
    assert_eq!(ops[1].state, OpState::Confirmed, "{ops:?}");
    let attempt = w.store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Started);
    let task = w.store.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.state, TaskState::Running);
    assert_eq!(w.stall_items().len(), 1, "the item is still the one open one");
}

/// A second failure in the new session gives one more attempt, not a loop: the
/// record follows the session that failed.
#[test]
fn a_second_failure_waits_for_the_next_session() {
    let mut w = world();
    w.tick(1);
    w.tick(5);
    w.tick(9);
    assert_eq!(w.deliveries()[0].state, OpState::Failed);

    w.herdr.set_agent_session(LANE, "ses_two");
    w.tick(13);
    assert_eq!(w.deliveries().len(), 2, "one retry in the new session");

    // That delivery is never confirmed either: two more checks fail it, and no
    // third delivery follows in this session.
    w.tick(17);
    w.tick(21);
    w.tick(25);
    let ops = w.deliveries();
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[1].state, OpState::Failed, "{ops:?}");
    assert_eq!(w.herdr.prompts_to(LANE).len(), 2, "no loop in one session");
}

/// The lane reads the open attempt: a view that confirms its delivery.
fn view(w: &mut World, at: i64) {
    w.store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                ts: T0 + at,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: Some("T-1".into()),
                shown_attempt: Some(1),
                shown_contract: Some(1),
            })
        })
        .unwrap();
}

/// F1 of the review: an agent restarted *before* the budget expires is the
/// session the retry belongs to. The failure names the session the prompt went
/// into (A), not the one the binding shows by then (B) — so B receives the open
/// attempt at once instead of waiting for another restart.
#[test]
fn a_restart_before_the_budget_expires_gets_the_open_attempt() {
    let mut w = world();
    w.herdr.set_agent_session(LANE, "ses_A");
    w.tick(1); // the prompt goes into A
    w.tick(5); // first check
    w.herdr.set_agent_session(LANE, "ses_B");
    w.tick(9); // second check: the delivery fails

    let ops = w.deliveries();
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[0].state, OpState::Failed, "{ops:?}");
    assert_eq!(ops[1].state, OpState::Submitted, "B got the attempt: {ops:?}");
    assert_eq!(ops[1].attempt, Some(1));
    assert_eq!(w.herdr.prompts_to(LANE).len(), 2, "{:?}", w.herdr.prompts_to(LANE));
    assert_eq!(w.stall_items().len(), 1);

    // B reads it: the attempt starts.
    view(&mut w, 10);
    w.tick(13);
    let ops = w.deliveries();
    assert_eq!(ops[1].state, OpState::Confirmed, "{ops:?}");
    let attempt = w.store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Started);
}

/// F2 of the review: a failed **continue** is not delivered again in the same
/// session either — the guard covers Started/Stopped attempts, not only a
/// Delivered one.
#[test]
fn a_failed_continue_is_not_repeated_in_the_same_session() {
    let mut w = world();
    w.herdr.set_agent_session(LANE, "ses_A");
    w.tick(1); // the assignment goes into A
    view(&mut w, 2);
    w.tick(5); // confirmed: the attempt starts
    assert_eq!(w.deliveries()[0].state, OpState::Confirmed);

    w.tick(9); // a Continue goes (cooldown elapsed)
    let ops = w.deliveries();
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert!(
        w.herdr.prompts_to(LANE)[1].contains("Continue T-1"),
        "{:?}",
        w.herdr.prompts_to(LANE)
    );

    w.tick(13); // one check
    w.tick(17); // the budget is spent: the Continue fails
    let ops = w.deliveries();
    assert_eq!(ops.len(), 2, "no replacement in the same session: {ops:?}");
    assert_eq!(ops[1].state, OpState::Failed, "{ops:?}");
    w.tick(21);
    assert_eq!(w.deliveries().len(), 2, "{:?}", w.deliveries());
    assert_eq!(w.herdr.prompts_to(LANE).len(), 2, "nothing is retyped in A");

    // A new session gets exactly one delivery of the same attempt.
    w.herdr.set_agent_session(LANE, "ses_B");
    w.tick(25);
    let ops = w.deliveries();
    assert_eq!(ops.len(), 3, "{ops:?}");
    assert_eq!(ops[2].state, OpState::Submitted, "{ops:?}");
    assert_eq!(ops[2].attempt, Some(1));
    assert!(
        w.herdr.prompts_to(LANE)[2].contains("Continue T-1"),
        "{:?}",
        w.herdr.prompts_to(LANE)
    );
}
