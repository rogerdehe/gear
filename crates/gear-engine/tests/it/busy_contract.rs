//! T-46: a Herdr Working lane has handed everything in but ready work waits.
use gear_app::{home, identity::Fingerprint, Env};
use gear_engine::{busy, observe, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";

struct World { store: Store, herdr: FakeHerdr, cfg: EngineCfg }

fn world_in(mut store: Store) -> World {
    store.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T0)?;
        for n in 1..=3 {
            tx.insert_task(&TaskRow {
                project: "p".into(), id: format!("T-{n}"), rank: n,
                state: if n == 1 { TaskState::Verifying } else { TaskState::Ready },
                priority: "must".into(), lane: Some("heat".into()), milestone: None,
                required: false, contract_version: 1, deps: vec![], followup_of: None,
                wait_on: None, wait_cond: None, rework_note: None, estimate_min: Some(30),  issue: None,updated: T0,
            })?;
        }
        tx.insert_attempt(&AttemptRow {
            project: "p".into(), task: "T-1".into(), n: 1, seat: "p:heat".into(), generation: 1,
            contract_version: 1, adopted_version: 1, pending_update: None,
            state: AttemptState::Submitted, candidate: Some("abc".into()), report: None,
            safe_point: None, started: T0-1800, updated: T0,
        })?;
        Ok(())
    }).unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "opencode", AgentStatus::Working);
    let mut cfg = EngineCfg::default();
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.into(), gear_herdr::builtin(kind).unwrap());
    }
    World { store, herdr, cfg }
}

fn world() -> World { world_in(Store::open_in_memory().unwrap()) }

fn run(w: &mut World, seconds: i64) -> gear_engine::TickReport {
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0+seconds).unwrap()
}

fn items(w: &mut World) -> Vec<gear_store::MessageRow> {
    w.store.read(|tx| tx.thread_messages("p", "lane-busy:p:heat")).unwrap()
}

fn show_captain(w: &mut World, seconds: i64) {
    let env = Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some(CAP.into()), admin: false };
    let fp = Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None };
    let page = home::build(&mut w.store, &env, Some(&fp), T0+seconds).unwrap();
    home::record_view(&mut w.store, &page.view).unwrap();
}

/// Observe and run only this monitor, so recovery tests do not also dispatch
/// the ready task while the test deliberately makes the lane idle.
fn watch(w: &mut World, seconds: i64) -> Vec<String> {
    let agents = observe::snapshot(&mut w.herdr).unwrap();
    let observed = w.store.read(|tx| observe::reconcile(tx, &mut w.herdr, "p", &agents, &w.cfg.profiles)).unwrap();
    w.store.write(|tx| busy::check(tx, "p", &observed, T0+seconds)).unwrap()
}

#[test]
fn working_with_only_submitted_attempts_alerts_once_after_ten_minutes() {
    let mut w = world();
    assert!(run(&mut w, 0).busy_lanes.is_empty());
    assert!(run(&mut w, 599).busy_lanes.is_empty());
    assert!(run(&mut w, 600).busy_lanes.is_empty(), "strictly more than ten minutes");
    assert!(items(&mut w).is_empty());
    assert_eq!(run(&mut w, 601).busy_lanes, vec!["p:heat"]);
    let messages = items(&mut w);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].body, "lane heat is busy with no task; ready work waits");
    assert_eq!(messages[0].recipient, "p:captain");
    assert_eq!(messages[0].kind, MsgKind::System);
    assert_eq!(messages[0].state, MsgState::Open);
    assert!(messages[0].urgent);
    assert_eq!(w.herdr.prompts_to(CAP).len(), 1, "the captain rings on the alert tick");
    show_captain(&mut w, 602);
    run(&mut w, 603); // confirm the original doorbell
    w.store.write(|tx| tx.update_message_state("p", &messages[0].id, MsgState::Resolved,
        Some("acknowledged"), None, None, T0+604)).unwrap();
    for seconds in [605, 700, 1200] { assert!(run(&mut w, seconds).busy_lanes.is_empty()); }
    assert_eq!(items(&mut w).len(), 1, "acknowledgment cannot reopen the same incident");
    assert_eq!(w.herdr.prompts_to(CAP).len(), 1);
}

#[test]
fn a_real_development_attempt_prevents_the_alert() {
    for state in [AttemptState::Delivered, AttemptState::Started, AttemptState::Stopped, AttemptState::Unknown] {
        let mut w = world();
        w.store.write(|tx| {
            let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
            attempt.state = state;
            tx.update_attempt(&attempt)
        }).unwrap();
        assert!(watch(&mut w, 0).is_empty());
        assert!(watch(&mut w, 1200).is_empty(), "{state:?} is still owned work");
        assert!(items(&mut w).is_empty());
    }
}

#[test]
fn blocked_wont_and_claimed_tasks_are_not_eligible_ready_work() {
    for condition in ["blocked", "wont", "claimed"] {
        let mut w = world();
        w.store.write(|tx| {
            for n in 2..=3 {
                let id = format!("T-{n}");
                let mut task = tx.get_task("p", &id)?.unwrap();
                match condition {
                    "blocked" => task.deps = vec!["T-1".into()],
                    "wont" => task.priority = "wont".into(),
                    _ => {
                        let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
                        attempt.task = id.clone();
                        attempt.seat = "p:other".into();
                        attempt.state = AttemptState::Delivered;
                        tx.insert_attempt(&attempt)?;
                    }
                }
                tx.update_task(&task)?;
            }
            Ok(())
        }).unwrap();
        assert!(watch(&mut w, 0).is_empty());
        assert!(watch(&mut w, 1200).is_empty(), "{condition}");
        assert!(items(&mut w).is_empty());
    }
}

#[test]
fn idle_recovery_closes_the_incident_and_restarts_the_timer() {
    let mut w = world();
    watch(&mut w, 0);
    assert_eq!(watch(&mut w, 601).len(), 1);
    w.herdr.set_status(LANE, AgentStatus::Idle);
    assert!(watch(&mut w, 602).is_empty());
    assert_eq!(items(&mut w)[0].state, MsgState::Resolved);
    w.herdr.set_status(LANE, AgentStatus::Working);
    assert!(watch(&mut w, 603).is_empty());
    assert!(watch(&mut w, 1203).is_empty());
    assert_eq!(watch(&mut w, 1204).len(), 1);
    assert_eq!(items(&mut w).len(), 2, "a new incident can be reported");
}

#[test]
fn an_observation_failure_cannot_count_as_continuous_working_time() {
    let mut w = world();
    run(&mut w, 0);
    w.herdr.observe_fails = true;
    assert_eq!(run(&mut w, 500).observe_failed, vec!["p"]);
    w.herdr.observe_fails = false;
    assert!(run(&mut w, 501).busy_lanes.is_empty());
    assert!(run(&mut w, 1101).busy_lanes.is_empty());
    assert_eq!(run(&mut w, 1102).busy_lanes, vec!["p:heat"]);
}

#[test]
fn timer_and_incident_latch_survive_reopening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    let mut w = world_in(Store::open(&path).unwrap());
    run(&mut w, 0);
    w.store = Store::open(&path).unwrap();
    assert_eq!(run(&mut w, 601).busy_lanes, vec!["p:heat"]);
    w.store = Store::open(&path).unwrap();
    assert!(run(&mut w, 700).busy_lanes.is_empty());
    assert_eq!(items(&mut w).len(), 1);
}

#[test]
fn registered_project_can_override_the_ten_minute_default() {
    let mut w = world();
    w.store.write(|tx| {
        let mut project = tx.get_project("p")?.unwrap();
        project.config_toml = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"
[quota]
lanes = 1
temps = 0
[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"
[timing]
lane_busy_idle_min = 5
"#.into();
        tx.update_project(&project)
    }).unwrap();
    assert!(run(&mut w, 0).busy_lanes.is_empty());
    assert!(run(&mut w, 300).busy_lanes.is_empty());
    assert_eq!(run(&mut w, 301).busy_lanes, vec!["p:heat"]);
}
