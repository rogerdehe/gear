//! T-243: one project's failure does not stop the other projects' tick.
//! Several projects share one engine (one per Herdr session), so a bad row or a
//! config the engine refuses in one project must never stop the voyages of the
//! others: the failure is reported in `TickReport::project_failed` and the tick
//! goes on with the next project.

use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const BROKEN_CAP_PANE: &str = "w1:p1";
const CAP_PANE: &str = "w2:p1";
const LANE_PANE: &str = "w2:p2";

/// A project config the engine refuses: the TOML reads, but a
/// `[profiles.<name>]` block is not a profile (`tail_lines` is a number).
const UNREADABLE_PROFILE_CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/tmp/p"
[quota]
lanes = 2
temps = 0
[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"
[profiles.codex]
tail_lines = "many"
"#;

/// A config that is not TOML at all.
const NOT_TOML_CONFIG: &str = "\n[project\nid = \"p\"\n";

fn profile(kind: &str) -> gear_herdr::Profile {
    gear_herdr::Profile {
        kind: kind.into(),
        new_session: String::new(),
        compact: String::new(),
        usage_log: None,
        error_markers: Vec::new(),
        dialogs: Vec::new(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: Vec::new(),
        quota_markers: Vec::new(),
        quota_bar: None,
        auth_markers: Vec::new(),
        backoff_markers: Vec::new(),
        blocked_markers: Vec::new(),
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

/// Two projects in one store: `p` with `broken_config`, and `q` with a ready
/// task and an idle lane. `p` sorts first, so a tick that stopped at the first
/// failing project would never deliver `q`'s task.
fn world(broken_config: &str) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                BROKEN_CAP_PANE,
                "codex",
                T0,
            )?;
            let mut broken = tx.get_project("p")?.unwrap();
            broken.config_toml = broken_config.into();
            tx.update_project(&broken)?;

            testing::seed_project(tx, "q", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "q",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "q", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "q".into(),
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
                estimate_min: Some(30),
                issue: None,
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "q".into(),
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
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(BROKEN_CAP_PANE, "codex", AgentStatus::Idle);
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

/// The healthy project's task is delivered by the tick that failed on `p`.
fn assert_delivered(w: &mut World) {
    let prompts = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(
        prompts.len(),
        1,
        "the healthy project's delivery: {prompts:?}"
    );
    assert!(
        prompts[0].starts_with("New task T-1 (contract v1)."),
        "{}",
        prompts[0]
    );
    assert_eq!(
        w.store
            .read(|tx| tx.get_attempt("q", "T-1", 1))
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Delivered
    );
}

#[test]
fn a_project_whose_config_the_engine_refuses_does_not_stop_the_tick() {
    let mut w = world(UNREADABLE_PROFILE_CONFIG);
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 100).unwrap();

    assert_delivered(&mut w);

    // The broken project is reported, never fatal, and the report names it.
    assert!(
        !report.project_failed.is_empty(),
        "the broken project is reported"
    );
    assert!(
        report
            .project_failed
            .iter()
            .all(|line| line.starts_with("p: BAD_FORMAT: ")),
        "{:?}",
        report.project_failed
    );
    assert!(
        report
            .project_failed
            .iter()
            .any(|line| line.contains("profile codex")),
        "{:?}",
        report.project_failed
    );
}

#[test]
fn a_project_whose_config_is_not_toml_does_not_stop_the_tick() {
    // Every per-project reader tolerates a config text it cannot parse (the
    // built-in profiles stay, onboard parks the lanes), so such a project is
    // not a *failed* project: nothing about it can fail this tick.
    let mut w = world(NOT_TOML_CONFIG);
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 100).unwrap();

    assert_delivered(&mut w);
    assert!(
        report.project_failed.is_empty(),
        "{:?}",
        report.project_failed
    );
}
