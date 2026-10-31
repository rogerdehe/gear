//! Slice 2 acceptance: the work loop end to end (design/05, /06, /08, /09).
//!
//! captain `gear plan apply` (new task) → engine assigns it to the idle lane
//! (attempt + delivery) → no resend while unconfirmed → lane `gear` (view shows
//! the task) → delivery confirmed, task Running → lane `gear send --kind result`
//! → task Verifying, captain rung → captain applies a pass verdict → task Done →
//! nothing more is sent to the lane.
//!
//! Do not edit these tests to make them pass; report back if one looks wrong.

use gear_app::identity::Fingerprint;
use gear_app::{draft, home, mail, plan, Env};
use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus, HerdrRead as _};
use gear_model::{AttemptState, GearCtx, MsgKind, SeatRole, TaskState};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn profile(kind: &str) -> gear_herdr::Profile {
    gear_herdr::Profile {
        kind: kind.into(),
        new_session: String::new(),
        compact: String::new(),
        usage_log: None,
        error_markers: Vec::new(),
        dialogs: Vec::new(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec![],
        quota_markers: vec![],
        quota_bar: None,
        auth_markers: vec![],
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
    }
}

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
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    let mut cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        ..Default::default()
    };
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.to_string(), profile(kind));
    }
    World { store, herdr, cfg }
}

fn env(seat: &str, pane: &str) -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: seat.into(),
            generation: 1,
        }),
        pane_id: Some(pane.into()),
        admin: false,
    }
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint {
        pane_id: pane.into(),
        kind: Some(kind.into()),
        session_ref: None,
    }
}

fn apply(w: &mut World, text: &str, rid: &str, now: i64) -> plan::ApplyResult {
    let d = draft::parse(text).unwrap();
    // The session's agents, as the captain's `gear plan apply` reads them
    // before its write transaction (T-133).
    let agents = w.herdr.agents().unwrap();
    plan::apply(
        &mut w.store,
        &env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        Some(&agents),
        &d,
        text,
        rid,
        now,
    )
    .unwrap()
}

fn lane_home(w: &mut World, now: i64) -> String {
    let h = home::build(
        &mut w.store,
        &env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        now,
    )
    .unwrap();
    home::record_view(&mut w.store, &h.view).unwrap();
    h.text
}

fn task_state(w: &mut World) -> TaskState {
    w.store
        .read(|tx| tx.get_task("p", "T-1"))
        .unwrap()
        .unwrap()
        .state
}

fn attempt_state(w: &mut World) -> AttemptState {
    w.store
        .read(|tx| tx.get_attempt("p", "T-1", 1))
        .unwrap()
        .unwrap()
        .state
}

#[test]
fn full_work_loop() {
    let mut w = world();

    // 1. the captain plans one task for lane heat
    let r = apply(&mut w, "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: Fix the login crash\naccept: repro passes\nestimate: 30\ndeps: -\n", "v1", T0 + 1);
    assert_eq!(r.created, vec!["T-1".to_string()]);

    // 2. the engine assigns it to the idle lane and delivers once
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 100).unwrap();
    let prompts = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(prompts.len(), 1, "one delivery");
    assert!(
        prompts[0].starts_with("New task T-1 (contract v1). Branch: heat/t1"),
        "{}",
        prompts[0]
    );
    assert_eq!(attempt_state(&mut w), AttemptState::Delivered);
    w.herdr.set_status(LANE_PANE, AgentStatus::Working); // the lane picked it up

    // 3. unconfirmed: no resend, even after the cooldown
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 200).unwrap();
    assert_eq!(
        w.herdr.prompts_to(LANE_PANE).len(),
        1,
        "never resend an unsettled delivery"
    );

    // 4. the lane looks at its home page: the task is shown; the next tick confirms
    let text = lane_home(&mut w, T0 + 210);
    assert!(text.contains("task T-1 attempt 1 contract v1"), "{text}");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 215).unwrap();
    assert_eq!(
        (attempt_state(&mut w), task_state(&mut w)),
        (AttemptState::Started, TaskState::Running)
    );

    // 5. the lane hands in its result
    let args = mail::SendArgs {
        kind: MsgKind::Result,
        body: "Report: notes/T-1.md".into(),
        task: Some("T-1".into()),
        attempt: Some(1),
        contract: Some(1),
        candidate: Some("abc1234".into()),
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        verdict: None,
        request_id: "res-1".into(),
        blocking: None,
        base: None,
    };
    mail::send(
        &mut w.store,
        &env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        &args,
        T0 + 300,
    )
    .unwrap();
    assert_eq!(
        (attempt_state(&mut w), task_state(&mut w)),
        (AttemptState::Submitted, TaskState::Verifying)
    );
    w.herdr.set_status(LANE_PANE, AgentStatus::Idle); // turn ended

    // 6. the lane gets nothing new (no ready task, result pending). Nothing in
    //    the project is startable any more, so the feed rings nobody (T-232) —
    //    the result waits for the captain in the inbox, not in a doorbell.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 400).unwrap();
    assert!(
        w.herdr.prompts_to(CAP_PANE).is_empty(),
        "no lane has work left to start"
    );
    assert_eq!(
        w.herdr.prompts_to(LANE_PANE).len(),
        1,
        "no continue for a submitted attempt"
    );
    let said = w
        .store
        .read(|tx| tx.messages_from_since("p", "p:heat", T0 + 300))
        .unwrap();
    assert!(
        said.iter()
            .any(|m| m.kind == MsgKind::Result && m.task.as_deref() == Some("T-1")),
        "the captain has the result: {said:?}"
    );

    // 7. the captain accepts it
    let r = apply(&mut w, "<!-- gear-plan project=p base=1 -->\n# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc1234 pass evidence=notes/T-1.md\n", "v2", T0 + 500);
    assert_eq!(r.settled, vec!["T-1 attempt 1 pass".to_string()]);
    assert_eq!(
        (attempt_state(&mut w), task_state(&mut w)),
        (AttemptState::Accepted, TaskState::Done)
    );

    // 8. quiet: nothing more for the lane
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1000).unwrap();
    assert_eq!(w.herdr.prompts_to(LANE_PANE).len(), 1);
}

/// T-133: a held lane is assigned nothing; the plan's `- release` puts it back
/// on the loop, and it gets its next task — no store edit, no `retry`.
#[test]
fn a_held_lane_gets_its_next_task_after_release() {
    let mut w = world();

    // 1. one task waits for lane heat
    apply(&mut w, "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: Fix the login crash\naccept: repro passes\nestimate: 30\ndeps: -\n", "v1", T0 + 1);

    // 2. the captain holds the lane: the engine assigns it nothing
    let r = apply(&mut w, "<!-- gear-plan project=p base=1 -->\n# Staff\n- hold heat reason=the machine is recovering\n", "v2", T0 + 2);
    assert_eq!(r.staffed, vec!["hold heat".to_string()]);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 100).unwrap();
    assert_eq!(
        w.herdr.prompts_to(LANE_PANE).len(),
        0,
        "a held lane is assigned nothing"
    );

    // 3. release it: its agent is still in its pane, so it continues — and the
    //    engine gives it the task it was held away from
    let r = apply(
        &mut w,
        "<!-- gear-plan project=p base=2 -->\n# Staff\n- release heat\n",
        "v3",
        T0 + 200,
    );
    assert_eq!(r.staffed, vec!["release heat".to_string()]);
    let seat = w.store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(
        seat.state,
        gear_model::SeatState::Active,
        "its agent continues"
    );
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 300).unwrap();
    let prompts = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(prompts.len(), 1, "its next task is delivered");
    assert!(
        prompts[0].starts_with("New task T-1 (contract v1). Branch: heat/t1"),
        "{}",
        prompts[0]
    );
}
