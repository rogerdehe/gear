//! Tests for `gear_engine::observe::reconcile` (T-21).

use gear_engine::observe::{reconcile, Presence};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::{AgentStatus, HerdrRead, Profile};
use gear_model::{SeatRole, SeatState};
use gear_store::{testing, SeatRow, Store, Tx};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const SEAT: &str = "p:captain";

fn profile(kind: &str) -> Profile {
    Profile {
        kind: kind.into(),
        new_session: String::new(),
        compact: String::new(),
        usage_log: None,
        error_markers: Vec::new(),
        dialogs: Vec::new(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec!["Error: Rate limit exceeded".into()],
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

fn profiles() -> std::collections::BTreeMap<String, Profile> {
    let mut map = std::collections::BTreeMap::new();
    map.insert("codex".to_string(), profile("codex"));
    map
}

/// A store with project `p` and an onboarded captain on `w1:p1` (kind codex).
fn store_with_seat() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess-p", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            Ok(())
        })
        .unwrap();
    store
}

/// A temp seat that has never been onboarded (no agent expected).
fn queued_seat(tx: &Tx<'_>) -> gear_model::GearResult<()> {
    tx.insert_seat(&SeatRow {
        id: "p:temp".into(),
        project: "p".into(),
        role: SeatRole::Temp,
        name: "temp".into(),
        profile: "opencode".into(),
        generation: 0,
        state: SeatState::Queued,
        state_since: T0,
        state_reason: None,
        task_id: None,
    })?;
    Ok(())
}

fn observe(
    store: &mut Store,
    herdr: &mut FakeHerdr,
    profiles: &std::collections::BTreeMap<String, Profile>,
) -> Vec<gear_engine::observe::SeatObs> {
    let agents = herdr.agents().unwrap();
    store
        .read(|tx| reconcile(tx, herdr, "p", &agents, profiles))
        .unwrap()
}

#[test]
fn present() {
    let mut store = store_with_seat();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);

    let observed = observe(&mut store, &mut herdr, &profiles());

    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].seat.id, SEAT);
    assert_eq!(observed[0].presence, Presence::Present);
    assert_eq!(observed[0].status, Some(AgentStatus::Idle));
    assert!(observed[0].idle);
}

#[test]
fn missing() {
    let mut store = store_with_seat();
    let mut herdr = FakeHerdr::new();
    // No agent in the pane: the seat expects one and it is not there.
    herdr.add_shell(CAP_PANE);

    let observed = observe(&mut store, &mut herdr, &profiles());

    assert_eq!(observed[0].presence, Presence::Missing);
    assert_eq!(observed[0].status, None);
    assert!(!observed[0].idle, "a missing agent is never idle");
}

#[test]
fn changed() {
    let mut store = store_with_seat();
    let mut herdr = FakeHerdr::new();
    // Another agent occupies the pane.
    herdr.add_agent(CAP_PANE, "opencode", AgentStatus::Idle);

    let observed = observe(&mut store, &mut herdr, &profiles());

    assert_eq!(observed[0].presence, Presence::Changed);
    assert!(!observed[0].idle, "a foreign occupant is never idle");
}

#[test]
fn not_expected() {
    let mut store = store_with_seat();
    store.write(|tx| queued_seat(tx)).unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Idle);

    let observed = observe(&mut store, &mut herdr, &profiles());

    let temp = observed.iter().find(|o| o.seat.id == "p:temp").unwrap();
    assert_eq!(temp.presence, Presence::NotExpected);
    assert!(!temp.idle);
    assert_eq!(
        observed[0].presence,
        Presence::Present,
        "the other seat is unaffected"
    );
}

#[test]
fn retired_seat_is_not_expected() {
    let mut store = store_with_seat();
    store
        .write(|tx| {
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            // A lane retires: Active -> Retiring -> Retired (the model's transitions).
            tx.set_seat_state("p:heat", SeatState::Retiring, Some("retiring"), T0 + 1)?;
            tx.set_seat_state("p:heat", SeatState::Retired, Some("retired"), T0 + 2)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    // The retired seat's pane still hosts its agent: it must not be judged Present.
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Idle);

    let observed = observe(&mut store, &mut herdr, &profiles());

    let heat = observed.iter().find(|o| o.seat.id == "p:heat").unwrap();
    assert_eq!(
        heat.presence,
        Presence::NotExpected,
        "a retired seat expects no agent"
    );
    assert!(!heat.idle);
    assert_eq!(
        observed[0].presence,
        Presence::Present,
        "the active seat is unaffected"
    );
}

#[test]
fn idle_when_quiet() {
    let mut store = store_with_seat();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.set_screen(CAP_PANE, "all done");

    let observed = observe(&mut store, &mut herdr, &profiles());

    assert_eq!(observed[0].presence, Presence::Present);
    assert!(observed[0].idle);
    assert!(observed[0].flags.is_empty());
}

#[test]
fn a_busy_marker_does_not_make_an_agent_busy() {
    // M4 §10.2: whether an agent is busy is Herdr's state, never the text on
    // its screen. Herdr says idle: the agent is idle, busy marker or not.
    let mut store = store_with_seat();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.set_screen(CAP_PANE, "working\n  esc interrupt  ");

    let observed = observe(&mut store, &mut herdr, &profiles());

    assert!(observed[0].idle, "Herdr's state decides, not the screen");
    assert!(observed[0].flags.is_empty());
}

#[test]
fn working_not_read() {
    let mut store = store_with_seat();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Working);
    // A busy marker on the screen of a working agent: it must not be read.
    herdr.set_screen(CAP_PANE, "esc interrupt");

    let observed = observe(&mut store, &mut herdr, &profiles());

    assert!(!observed[0].idle);
    assert!(
        observed[0].flags.is_empty(),
        "the screen of a working agent is not read"
    );
}

#[test]
fn unknown_profile_fails_closed() {
    let mut store = store_with_seat();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);

    // The seat's profile ("codex") is not known to the engine.
    let observed = observe(&mut store, &mut herdr, &std::collections::BTreeMap::new());

    assert_eq!(observed[0].presence, Presence::Present);
    assert!(!observed[0].idle, "an unknown profile is never idle");
}
