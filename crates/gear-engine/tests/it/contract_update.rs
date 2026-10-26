//! Contract for the contract-update delivery (T-8): a bump during an attempt
//! gives one line at the lane's next idle point; the lane's view adopts it.

use gear_app::draft::{parse, Draft};
use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::tasks::{on_delivery_confirmed, start_attempt, submit_result};
use gear_app::Env;
use gear_model::GearCtx;
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::{builtin, AgentStatus};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

/// Project `p` with T-1 (contract v1) and an attempt that is working.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
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
             issue: None,updated: T0,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "g1".into(),
            accept: "a1".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
        })?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    s
}

fn cfg() -> EngineCfg {
    let mut c = EngineCfg { db_path: "/state/gear.db".into(), ..Default::default() };
    c.profiles.insert("opencode".into(), builtin("opencode").unwrap());
    c
}

fn idle_herdr() -> FakeHerdr {
    let mut h = FakeHerdr::new();
    h.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    h.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    h
}

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
    )
}

fn lane() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }),
            pane_id: Some(LANE_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None },
    )
}

/// Apply a draft that changes T-1's goal (a contract bump).
fn bump(s: &mut Store, base: i64, goal: &str, rid: &str) -> i64 {
    let text = format!(
        "<!-- gear-plan project=p base={base} -->\n# Queue\n## T-1 [must] lane=heat\ngoal: {goal}\naccept: a1\ndeps: -\n"
    );
    let draft: Draft = parse(&text).unwrap();
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), &draft, &text, rid, T0 + 10).unwrap();
    s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().contract_version
}

#[test]
fn a_bump_is_delivered_once_and_the_view_adopts_it() {
    let mut s = world();
    let mut herdr = idle_herdr();
    bump(&mut s, 0, "g2", "r1");
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((a.adopted_version, a.pending_update), (1, Some(2)));

    let report = tick(&mut s, &mut herdr, &cfg(), T0 + 20).unwrap();
    assert_eq!(report.intended.len(), 1, "{report:?}");
    let prompts = herdr.prompts_to(LANE_PANE);
    assert_eq!(prompts.len(), 1, "one delivery: {prompts:?}");
    assert_eq!(
        prompts[0],
        "T-1 contract changed to v2: run `gear` and `gear --show T-1`"
    );

    // The lane's view adopts it.
    let (env, fp) = lane();
    let page = home::build(&mut s, &env, Some(&fp), T0 + 30).unwrap();
    assert!(page.text.contains("contract v2"), "{}", page.text);
    home::record_view(&mut s, &page.view).unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((a.adopted_version, a.pending_update), (2, None));

    // Nothing left to deliver.
    let mut herdr = idle_herdr();
    let report = tick(&mut s, &mut herdr, &cfg(), T0 + 40).unwrap();
    assert_eq!(report.intended.len(), 0, "{report:?}");
    assert!(herdr.prompts_to(LANE_PANE).is_empty());
}

#[test]
fn two_bumps_before_the_lane_looks_give_one_delivery_with_the_latest() {
    let mut s = world();
    bump(&mut s, 0, "g2", "r1");
    bump(&mut s, 1, "g3", "r2");
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(a.pending_update, Some(3));

    let mut herdr = idle_herdr();
    let report = tick(&mut s, &mut herdr, &cfg(), T0 + 20).unwrap();
    assert_eq!(report.intended.len(), 1, "{report:?}");
    let prompts = herdr.prompts_to(LANE_PANE);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("v3"), "{}", prompts[0]);

    // The view adopts the latest.
    let (env, fp) = lane();
    let page = home::build(&mut s, &env, Some(&fp), T0 + 30).unwrap();
    home::record_view(&mut s, &page.view).unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(a.adopted_version, 3);
}

#[test]
fn a_handed_in_attempt_is_not_nudged() {
    let mut s = world();
    s.write(|tx| submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: r", T0 + 5))
        .unwrap();
    bump(&mut s, 0, "g2", "r1");

    let mut herdr = idle_herdr();
    let report = tick(&mut s, &mut herdr, &cfg(), T0 + 20).unwrap();
    assert_eq!(report.intended.len(), 0, "{report:?}");
    assert!(herdr.prompts_to(LANE_PANE).is_empty(), "a handed-in attempt is not nudged");
    assert_eq!(
        s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state,
        AttemptState::Submitted
    );
}

#[test]
fn a_restart_between_the_bump_and_the_delivery_still_delivers_once() {
    let mut s = world();
    bump(&mut s, 0, "g2", "r1");

    // The engine restarts before it ever ticks on the bump.
    gear_engine::ops::recover_on_start(&mut s, T0 + 15).unwrap();
    let mut herdr = idle_herdr();
    let report = tick(&mut s, &mut herdr, &cfg(), T0 + 20).unwrap();
    assert_eq!(report.intended.len(), 1, "{report:?}");
    assert_eq!(herdr.prompts_to(LANE_PANE).len(), 1);
}
