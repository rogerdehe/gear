//! T-93: cancelling a task from the plan frees its lane for the next one.
//!
//! The plan's cancel abandons the task's open attempt (see gear-app's
//! `cancel_contract`); this is the consequence the engine must show: the lane
//! the cancelled task held is delivered its next ready task on the next tick.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::tasks::{on_delivery_confirmed, start_attempt};
use gear_app::Env;
use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{GearCtx, MsgState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";

fn cap() -> (Env, Fingerprint) {
    (
        Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some(CAP.into()), admin: false },
        Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None },
    )
}

fn world() -> (Store, FakeHerdr, EngineCfg) {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T0)?;
        for (id, rank, deps) in [("T-1", 1, vec![]), ("T-2", 2, vec![])] {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: id.into(),
                rank,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps,
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: id.into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
        }
        // The lane is at work on T-1.
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
        Ok(())
    })
    .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "opencode", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    for kind in ["opencode", "codex"] {
        cfg.profiles.insert(kind.into(), gear_herdr::builtin(kind).unwrap());
    }
    (s, herdr, cfg)
}

fn cancel(s: &mut Store) {
    let text = "<!-- gear-plan project=p base=0 -->\n# Queue\n## T-1 [must] lane=heat\ngoal: g\naccept: a\nestimate: 30\ndeps: -\nstate: cancelled\n";
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &parse(text).unwrap(), text, "cancel-1", T0 + 5).unwrap();
}

fn t2_attempt(s: &mut Store) -> Option<gear_model::AttemptState> {
    s.read(|tx| Ok(tx.get_attempt("p", "T-2", 1)?.map(|attempt| attempt.state)))
        .unwrap()
}

#[test]
fn a_cancelled_task_frees_the_lane_and_the_next_task_is_delivered() {
    let (mut s, mut herdr, cfg) = world();
    // The lane is busy with T-1: T-2 has no attempt.
    assert_eq!(t2_attempt(&mut s), None, "the lane is on T-1");

    cancel(&mut s);
    // The lane hears why, in one line.
    let line = s.read(|tx| tx.thread_messages("p", "T-1:cancel")).unwrap();
    assert_eq!(line.len(), 1);
    assert_eq!(line[0].state, MsgState::Open);

    // The next tick delivers the next ready task to the now free lane.
    tick(&mut s, &mut herdr, &cfg, T0 + 10).unwrap();
    assert_eq!(t2_attempt(&mut s), Some(gear_model::AttemptState::Delivered), "the free lane takes the next task");
}
