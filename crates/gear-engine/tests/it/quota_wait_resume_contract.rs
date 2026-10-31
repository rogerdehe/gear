//! T-295: a lane parked on a quota wait resumes its **own session** at the
//! provider reset, shown through the whole engine tick (postponed from M2
//! #13).
//!
//! The unit proof of M2 lives in `crates/gear-engine/src/quota.rs`
//! (`a_spent_window_parks_the_lane_until_the_reset` and
//! `a_parked_lane_wakes_up_in_place`): it calls the quota step alone with a
//! fake reader. This contract drives `gear_engine::tick` with `FakeHerdr`:
//! a pi lane whose screen shows the provider's quota marker on two idle ticks
//! is parked; before the reset nothing is delivered; at the reset, with the
//! marker gone, the lane is Active again, the Continue is
//! "Continue T-1: your quota is back.", the binding still holds the session
//! from before the park, and nothing restarted or closed the agent.
//!
//! The project has no `[[quota.sources]]`: `sources_of` reads only the
//! config, so no reader runs and nothing touches the network. The park is the
//! unreadable-source park for `quota_reset_h` hours.

use gear_engine::{tick, EngineCfg, TickReport};
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::AgentStatus;
use gear_model::{OpKind, SeatRole, SeatState, TaskState, Ts};
use gear_store::testing;
use gear_store::{ContractRow, ProjectRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const PANE: &str = "w1:p1";
/// The model of the voyage's setup: a pi lane, so a wake switches no model and
/// no `[models.switch]` labels are needed.
const MODEL: &str = "tokenrhythm/deepseek-v4.1-flash-1017";
/// The session the lane worked in before the park.
const SESSION: &str = "sess-before-park";
/// The provider limit on a pi screen (its quota marker).
const QUOTA_SCREEN: &str =
    "429: {\"message\":\"You exceeded your current quota\",\"type\":\"insufficient_quota\"}";
/// The screen when the provider has reset.
const RESET_SCREEN: &str = "  Ask anything…";

/// One pi lane, one model, `on_quota` waits and one hour to resume. No
/// `[[quota.sources]]`.
fn config() -> String {
    format!(
        r#"
[project]
id = "p"
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
lanes = ["{MODEL}"]
on_quota = {{ lanes = "wait" }}
quota_reset_h = 1
"#
    )
}

/// The engine's configuration: the built-ins (pi among them) and a scratch db.
fn cfg() -> EngineCfg {
    let mut cfg = EngineCfg {
        db_path: crate::temp("gear-engine-tests")
            .join("gear.db")
            .to_string_lossy()
            .into_owned(),
        ..Default::default()
    };
    cfg.cooldown_s = 0;
    cfg.profiles = gear_herdr::builtins();
    cfg
}

/// A project with one active pi lane that works on T-1 (Started) in `SESSION`.
fn world(text: &str) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&ProjectRow {
                id: "p".into(),
                herdr_session: "sess".into(),
                config_toml: text.into(),
                config_version: 1,
                quota_lanes: 1,
                quota_temps: 0,
                quota_includes_captain: false,
                plan_version: 0,
                created: T0,
            })?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T0)?;
            tx.set_binding_session_ref("p:heat", 1, SESSION)?;
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
                estimate_min: Some(30),
                issue: None,
                updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
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
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
            Ok(())
        })
        .unwrap();
    store
}

fn seat(store: &mut Store) -> gear_store::SeatRow {
    store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap()
}

fn binding_session(store: &mut Store) -> Option<String> {
    let generation = seat(store).generation;
    store
        .read(|tx| tx.get_binding("p:heat", generation))
        .unwrap()
        .and_then(|binding| binding.session_ref)
}

/// The texts of every delivery op of the lane (whatever state it reached).
fn deliveries(store: &mut Store) -> Vec<String> {
    store
        .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Deliver))
        .unwrap()
        .into_iter()
        .filter_map(|op| {
            op.payload
                .get("text")
                .and_then(|text| text.as_str())
                .map(str::to_string)
        })
        .collect()
}

fn step(store: &mut Store, herdr: &mut FakeHerdr, cfg: &EngineCfg, now: Ts) -> TickReport {
    tick(store, herdr, cfg, now).unwrap()
}

/// The engine's calls after `from`, for the "nothing restarted" assertions.
fn calls_after<'a>(herdr: &'a FakeHerdr, from: usize) -> &'a [Call] {
    &herdr.calls[from..]
}

/// T-295: two idle ticks with the pi quota marker park the lane; before the
/// reset nothing is delivered; at the reset the lane is Active again, the
/// Continue is the one of §9.2, the session is the one from before the park,
/// and the agent was neither restarted nor closed.
#[test]
fn a_lane_parked_on_a_quota_wait_resumes_its_own_session_at_the_reset() {
    let text = config();
    let cfg = cfg();
    let mut store = world(&text);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(PANE, "pi", AgentStatus::Idle);
    herdr.set_agent_session(PANE, SESSION);
    herdr.set_screen(PANE, QUOTA_SCREEN);

    // The session of the lane before the park: same generation, same session
    // ref after it.
    let generation_before = seat(&mut store).generation;
    let session_before = binding_session(&mut store);

    // One idle tick with the marker is no fact; the second confirms it.
    step(&mut store, &mut herdr, &cfg, T0 + 10);
    assert_eq!(
        seat(&mut store).state,
        SeatState::Active,
        "one tick is no fact"
    );

    let report = step(&mut store, &mut herdr, &cfg, T0 + 20);
    let parked = seat(&mut store);
    assert_eq!(
        parked.state,
        SeatState::Parked,
        "{:?}: {report:?}",
        parked.state_reason
    );
    // `quota_reset_h = 1`: the park ends an hour after the tick that parked it.
    let until = T0 + 20 + 3600;
    assert!(
        parked
            .state_reason
            .as_deref()
            .unwrap_or_default()
            .contains(&until.to_string()),
        "{:?}",
        parked.state_reason
    );
    assert!(
        deliveries(&mut store).is_empty(),
        "the park delivers nothing"
    );
    let calls_at_park = herdr.calls.len();

    // Ticks before the reset deliver nothing, whatever the screen shows.
    for tick_no in 0..2 {
        let report = step(&mut store, &mut herdr, &cfg, T0 + 100 + tick_no * 600);
        assert_eq!(seat(&mut store).state, SeatState::Parked, "{report:?}");
        assert!(deliveries(&mut store).is_empty(), "{report:?}");
    }

    // The provider has reset: the marker is gone and the wait is over.
    herdr.set_screen(PANE, RESET_SCREEN);
    let report = step(&mut store, &mut herdr, &cfg, until);

    let awake = seat(&mut store);
    assert_eq!(
        awake.state,
        SeatState::Active,
        "{:?}: {report:?}",
        awake.state_reason
    );
    assert_eq!(
        deliveries(&mut store),
        vec!["Continue T-1: your quota is back.".to_string()],
        "the Continue of §9.2: {report:?}"
    );
    assert_eq!(
        awake.generation, generation_before,
        "the same generation, not a new session: {report:?}"
    );
    assert_eq!(
        binding_session(&mut store).as_deref(),
        Some(SESSION),
        "the lane resumed its own session: {report:?}"
    );
    assert_eq!(
        binding_session(&mut store),
        session_before,
        "the same binding session: {report:?}"
    );
    // Every call after the park: no AgentStart and no exit keys (none of the
    // parked ticks and not the wake closed or restarted the agent).
    let after = calls_after(&herdr, calls_at_park);
    assert!(
        after
            .iter()
            .all(|call| !matches!(call, Call::AgentStart { .. })),
        "no restart: {after:?}"
    );
    assert!(
        after.iter().all(|call| !matches!(
            call,
            Call::SendKeys { keys, .. }
                if keys.iter().any(|key| key == "ctrl+c" || key == "ctrl+d")
        )),
        "no exit keys: {after:?}"
    );
    assert!(
        after
            .iter()
            .all(|call| !matches!(call, Call::Prompt { text, .. } if text == "/quit")),
        "no exit command: {after:?}"
    );
}
