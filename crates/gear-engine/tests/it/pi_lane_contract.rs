//! Contract for a pi lane (T-114).
//!
//! A lane whose chain entry runs on `pi` is started with `pi --model
//! <provider>/<id>` in its worktree: no OpenCode session API, no
//! `[models.switch]` labels (pi has no model picker GEAR drives). A quota stop
//! on pi moves to the next chain entry by **exiting** pi and starting it again
//! on the next model — the same pane, the same worktree — and the lane's task
//! is delivered again in the new session. An exhausted chain parks the lane.

use gear_engine::observe;
use gear_engine::quota::apply_quota_policy;
use gear_engine::EngineCfg;
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::{HerdrRead, Profile};
use gear_model::{OpKind, OpState, SeatRole, SeatState, Ts};
use gear_store::testing;
use gear_store::{ContractRow, ProjectRow, Store, TaskRow};
use std::collections::BTreeMap;

const T0: i64 = 1_800_000_000;
const PANE: &str = "w1:p1";

/// The first model of the chain and the one the quota stop moves to. Both run
/// on pi.
const FIRST: &str = "opencode-go/deepseek-v4.1-flash";
const SECOND: &str = "opencode-go/deepseek-v4-pro";

/// A project with one pi lane on a two-model chain. The pi profile is the
/// project's own: the engine only needs its kind (`pi`), its exit keys and its
/// markers.
fn config(chain: &str) -> String {
    format!(
        r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w9:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "pi"
profile = "pi"

[models]
lanes = [{chain}]
on_quota = {{ lanes = "fallback" }}
quota_reset_h = 5

[profiles.pi]
kind = "pi"
unattended_args = ["--approve"]
# pi exits on ctrl+d; ctrl+c only clears its input (checked live, pi 1.4.0).
exit_keys = ["ctrl+d"]
quota_markers = ["usage limit reached"]
busy_markers = ["esc to interrupt"]
# The policy needs two quota screens in a row; the test counts the ticks.
confirm_rounds = 2
"#
    )
}

/// The engine's configuration: the built-ins plus the project's own profiles.
fn cfg(config_text: &str) -> (EngineCfg, BTreeMap<String, Profile>) {
    let mut c = EngineCfg { db_path: crate::temp("gear-engine-tests").join("gear.db").to_string_lossy().into_owned(), ..Default::default() };
    for (name, profile) in gear_herdr::builtins() {
        c.profiles.insert(name, profile);
    }
    let parsed = gear_app::config::parse(config_text).unwrap();
    let profiles = gear_app::config::profile_table(&c.profiles, &parsed).unwrap();
    (c, profiles)
}

/// A project with a **queued** pi lane (for the start) and no attempt.
fn queued_world(config_text: &str) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&ProjectRow {
                id: "toy".into(),
                herdr_session: "sess".into(),
                config_toml: config_text.into(),
                config_version: 1,
                quota_lanes: 1,
                quota_temps: 0,
                quota_includes_captain: false,
                plan_version: 0,
                created: T0,
            })?;
            tx.insert_seat(&gear_store::SeatRow {
                id: "toy:heat".into(),
                project: "toy".into(),
                role: SeatRole::Lane,
                name: "heat".into(),
                profile: "pi".into(),
                generation: 0,
                state: SeatState::Queued,
                state_since: T0,
                state_reason: Some("registered".into()),
                task_id: None,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

/// A project with an **active** pi lane whose task T-1 is open (so the lane
/// has something to continue after a restart).
fn active_world(config_text: &str) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&ProjectRow {
                id: "toy".into(),
                herdr_session: "sess".into(),
                config_toml: config_text.into(),
                config_version: 1,
                quota_lanes: 1,
                quota_temps: 0,
                quota_includes_captain: false,
                plan_version: 0,
                created: T0,
            })?;
            testing::seed_active_seat(tx, "toy", "heat", SeatRole::Lane, PANE, "pi", T0)?;
            testing::seed_active_seat(tx, "toy", "captain", SeatRole::Captain, "w9:p1", "claude", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "toy".into(),
                id: "T-1".into(),
                rank: 1,
                state: gear_model::TaskState::Ready,
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
                project: "toy".into(),
                task: "T-1".into(),
                version: 1,
                goal: "the goal".into(),
                accept: "the accept".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            gear_app::tasks::start_attempt(tx, "toy", "T-1", "toy:heat", 1, T0)?;
            gear_app::tasks::on_delivery_confirmed(tx, "toy", "T-1", 1, T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// The start the engine asked for: (kind, pane, args).
fn agent_starts(herdr: &FakeHerdr) -> Vec<(String, String, Vec<String>)> {
    herdr.calls
        .iter()
        .filter_map(|call| match call {
            Call::AgentStart { kind, pane, args, .. } => {
                Some((kind.clone(), pane.clone(), args.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The closing calls the engine sent to the pane (the profile's exit: its
/// command or its keys). The engine closes the agent once — a second key
/// would land in the shell.
fn closing_calls(herdr: &FakeHerdr) -> Vec<String> {
    herdr.calls
        .iter()
        .filter_map(|call| match call {
            Call::Prompt { pane, text, .. } if pane == PANE => Some(format!("command:{text}")),
            Call::SendKeys { pane, keys } if pane == PANE => {
                Some(format!("keys:{}", keys.join("+")))
            }
            _ => None,
        })
        .collect()
}

/// The texts the engine delivered to the lane.
fn deliveries(store: &mut Store) -> Vec<String> {
    store
        .read(|tx| tx.ops_in_state("toy", OpState::Intended))
        .unwrap()
        .into_iter()
        .filter(|op| op.kind == OpKind::Deliver)
        .filter_map(|op| {
            op.payload
                .get("text")
                .and_then(|text| text.as_str())
                .map(str::to_string)
        })
        .collect()
}

/// The screen of a pi agent that hit the provider's limit.
const QUOTA_SCREEN: &str = "  Error: usage limit reached. Try again later.";

/// One quota tick of the policy, with the lane observed as it is on screen.
fn tick_quota(
    store: &mut Store,
    herdr: &mut FakeHerdr,
    cfg: &EngineCfg,
    profiles: &BTreeMap<String, Profile>,
    now: Ts,
) -> Vec<String> {
    let agents = herdr.agents().unwrap();
    let observed = store
        .read(|tx| observe::reconcile(tx, herdr, "toy", &agents, profiles))
        .unwrap();
    apply_quota_policy(store, herdr, cfg, profiles, "toy", &observed, now)
        .unwrap()
        .events
}

/// T-114: a pi lane is started with `pi --model <provider>/<id>` in its
/// worktree. No OpenCode session API is asked (pi has no session GEAR owns)
/// and no `[models.switch]` labels are needed (pi has no model picker GEAR
/// drives).
#[test]
fn a_pi_lane_starts_with_its_model() {
    let text = config(&format!(r#"{{ model = "{FIRST}", cli = "pi" }}"#));
    let (cfg, _profiles) = cfg(&text);
    let mut store = queued_world(&text);
    let mut herdr = FakeHerdr::new();

    let active = gear_engine::onboard::onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();
    assert_eq!(active, vec!["toy:heat".to_string()]);

    let starts = agent_starts(&herdr);
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(starts[0].0, "pi", "the kind of the chain entry");
    assert_eq!(starts[0].1, "wt1:p1", "the pane of the lane's worktree");
    assert_eq!(
        starts[0].2,
        vec![
            "--approve".to_string(),
            "--append-system-prompt".to_string(),
            std::path::Path::new(&cfg.db_path).parent().unwrap().join("rules/lane.md").to_string_lossy().into_owned(),
            "--model".to_string(),
            FIRST.to_string(),
        ],
        "pi is started with --model <provider>/<id>"
    );
    // No OpenCode session API, no model keys: the model is given at the start.
    assert!(
        !herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::OpenCodeApi { .. })),
        "pi asks no OpenCode session API: {:?}",
        herdr.calls
    );
    assert!(
        !herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::PaneType { .. } | Call::SendKeys { .. })),
        "nothing is typed into the agent: {:?}",
        herdr.calls
    );
    // The lane is up and on its chain's model.
    let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
    assert_eq!(seat.profile, "pi");
    let model = store
        .read(|tx| tx.get_meta(&format!("engine.model.toy:heat")))
        .unwrap()
        .unwrap();
    assert_eq!(model, FIRST);
}

/// T-114: the chain entry's own arguments follow the model flag — pi's
/// `--thinking` is the entry's, never a constant in the code (T-35).
#[test]
fn a_pi_lane_starts_with_the_arguments_of_its_chain_entry() {
    let text = config(&format!(
        r#"{{ model = "{FIRST}", cli = "pi", args = ["--thinking", "high"] }}"#
    ));
    let (cfg, _profiles) = cfg(&text);
    let mut store = queued_world(&text);
    let mut herdr = FakeHerdr::new();

    gear_engine::onboard::onboard_queued(&mut store, &mut herdr, &cfg, T0 + 1).unwrap();

    let starts = agent_starts(&herdr);
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(
        starts[0].2,
        vec![
            "--approve".to_string(),
            "--append-system-prompt".to_string(),
            std::path::Path::new(&cfg.db_path).parent().unwrap().join("rules/lane.md").to_string_lossy().into_owned(),
            "--model".to_string(),
            FIRST.to_string(),
            "--thinking".to_string(),
            "high".to_string(),
        ],
        "the entry's arguments follow the model flag"
    );
}

/// T-114: a quota stop on pi moves to the next chain entry by exiting pi and
/// starting it again on the next model — the same pane, the same worktree —
/// and the lane's task is delivered again in the new session.
#[test]
fn a_quota_stop_restarts_pi_on_the_next_model() {
    let text = config(&format!(
        r#"{{ model = "{FIRST}", cli = "pi" }}, {{ model = "{SECOND}", cli = "pi" }}"#
    ));
    let (cfg, profiles) = cfg(&text);
    let mut store = active_world(&text);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(PANE, "pi", gear_herdr::AgentStatus::Idle);
    herdr.set_screen(PANE, QUOTA_SCREEN);

    // Tick 1: one quota screen is not enough (`confirm_rounds` is 2).
    assert!(tick_quota(&mut store, &mut herdr, &cfg, &profiles, T0 + 10).is_empty());
    assert!(closing_calls(&herdr).is_empty(), "nothing sent on the first tick");

    // Tick 2: the limit is confirmed — pi is closed with the profile's own
    // exit (its command or its keys), and nothing else happens yet.
    let events = tick_quota(&mut store, &mut herdr, &cfg, &profiles, T0 + 20);
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(
        closing_calls(&herdr).len(),
        1,
        "pi is closed once: {:?}",
        herdr.calls
    );
    assert!(agent_starts(&herdr).is_empty(), "nothing started yet");
    assert!(deliveries(&mut store).is_empty(), "no delivery before the agent is up");

    // The agent leaves: the pane is a shell again.
    herdr.exit_agent(PANE);

    // Tick 3: pi starts again on the next model, in the same pane, and the
    // lane continues its task.
    let events = tick_quota(&mut store, &mut herdr, &cfg, &profiles, T0 + 30);
    assert_eq!(events.len(), 1, "{events:?}");
    let starts = agent_starts(&herdr);
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(starts[0].0, "pi", "the same CLI, started again");
    assert_eq!(starts[0].1, PANE, "the same pane");
    assert!(
        starts[0].2.contains(&"--model".to_string())
            && starts[0].2.contains(&SECOND.to_string()),
        "pi is started on the next model: {:?}",
        starts[0].2
    );
    assert!(
        !starts[0].2.contains(&FIRST.to_string()),
        "not on the model that ran out: {:?}",
        starts[0].2
    );

    // The lane's task is delivered again in the new session.
    let delivered = deliveries(&mut store);
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert!(delivered[0].starts_with("Continue T-1"), "{delivered:?}");
    let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
    assert_eq!(seat.generation, 2, "a new session");
    assert_eq!(seat.profile, "pi");
    let model = store
        .read(|tx| tx.get_meta(&format!("engine.model.toy:heat")))
        .unwrap()
        .unwrap();
    assert_eq!(model, SECOND);

    // More quota ticks add no second delivery.
    tick_quota(&mut store, &mut herdr, &cfg, &profiles, T0 + 40);
    assert_eq!(deliveries(&mut store).len(), 1, "one delivery only");
}

/// T-114: an exhausted chain parks the pi lane until the provider resets, and
/// the captain is rung — the lane is not closed and started for ever.
#[test]
fn an_exhausted_chain_parks_the_pi_lane() {
    let text = config(&format!(r#"{{ model = "{FIRST}", cli = "pi" }}"#));
    let (cfg, profiles) = cfg(&text);
    let mut store = active_world(&text);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(PANE, "pi", gear_herdr::AgentStatus::Idle);
    herdr.set_screen(PANE, QUOTA_SCREEN);

    tick_quota(&mut store, &mut herdr, &cfg, &profiles, T0 + 10);
    let events = tick_quota(&mut store, &mut herdr, &cfg, &profiles, T0 + 20);

    assert_eq!(events.len(), 1, "{events:?}");
    assert!(events[0].contains("chain exhausted"), "{events:?}");
    let seat = store.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Parked);
    let until = T0 + 20 + 5 * 3600;
    assert!(
        seat.state_reason.as_deref().unwrap_or_default().contains(&until.to_string()),
        "{:?}",
        seat.state_reason
    );
    // The captain is rung: the lane is stopped until the provider resets.
    let open = store
        .read(|tx| tx.actionable_for("toy", "toy:captain", T0 + 30))
        .unwrap();
    assert!(
        open.iter().any(|m| m.urgent && m.body.contains("parked until")),
        "an urgent captain item: {open:?}"
    );
    // Nothing was closed: the agent stays until the chain has an entry again.
    assert!(agent_starts(&herdr).is_empty(), "no restart: {starts:?}", starts = agent_starts(&herdr));
}

#[test]
fn pi_lane_start_injects_its_rules_file() {
    let dir = tempfile::tempdir().unwrap();
    let text = config(&format!("\"{FIRST}\""));
    let (mut cfg, _) = cfg(&text);
    cfg.db_path = dir.path().join("gear.db").to_string_lossy().into();
    let mut store = queued_world(&text);
    let mut herdr = FakeHerdr::new();
    assert_eq!(gear_engine::onboard::onboard_queued(&mut store, &mut herdr, &cfg, T0+1).unwrap(), vec!["toy:heat"]);
    let starts = agent_starts(&herdr);
    let args = &starts[0].2;
    let path = dir.path().join("rules/lane.md");
    assert!(args.windows(2).any(|w| w[0] == "--append-system-prompt" && w[1] == path.to_string_lossy()));
    let rules = std::fs::read_to_string(path).unwrap();
    assert!(rules.starts_with(include_str!("../../../../skills/gear/references/rules/lane.md").trim_end()));
    assert!(rules.ends_with("Voyage done: -\nBar: -\n"));
}
