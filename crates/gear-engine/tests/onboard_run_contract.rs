//! Contract for onboard_queued (T-62): the walking-skeleton onboarding of lanes.
//! Needs T-57 (admin init), T-58 (plan_onboard), T-59 (seat transitions).

use gear_app::{admin, Env};
use gear_engine::onboard::onboard_queued;
use gear_engine::EngineCfg;
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::builtin;
use gear_model::SeatState;
use gear_store::{BindingStatus, Store};

const T0: i64 = 1_800_000_000;
const CONFIG: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#;

fn cfg() -> EngineCfg {
    let mut c = EngineCfg { db_path: "/state/gear.db".into(), ..Default::default() };
    c.profiles.insert("opencode".into(), builtin("opencode").unwrap());
    c
}

#[test]
#[ignore = "T-62: un-ignored by the captain when T-62 merges"]
fn queued_lane_gets_worktree_env_and_agent() {
    let mut s = Store::open_in_memory().unwrap();
    admin::init(&mut s, &Env { ctx: None, pane_id: None, admin: true }, CONFIG, T0).unwrap();
    let mut herdr = FakeHerdr::new();
    let active = onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap();
    assert_eq!(active, vec!["toy:heat".to_string()]);
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!((seat.state, seat.generation), (SeatState::Active, 1));
    let b = s.read(|tx| tx.get_binding("toy:heat", 1)).unwrap().unwrap();
    assert_eq!(b.status, BindingStatus::Active);
    assert!(herdr.worktree_branches().contains(&"home/heat"));
    assert!(herdr.calls.iter().any(|c| matches!(c, Call::PaneRun { command, .. }
        if command == "export GEAR_CTX=toy:heat:1; export GEAR_DB=/state/gear.db")), "{:?}", herdr.calls);
    assert!(herdr.calls.iter().any(|c| matches!(c, Call::AgentStart { name, kind, args, .. }
        if name == "toy-heat" && kind == "opencode" && args == &vec!["--auto".to_string()])), "{:?}", herdr.calls);
    // the captain is not onboarded by the engine in this cut
    assert_eq!(s.read(|tx| tx.get_seat("toy:captain")).unwrap().unwrap().state, SeatState::Queued);
    // nothing to do on the next run
    assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 2).unwrap().is_empty());
}

#[test]
#[ignore = "T-62: un-ignored by the captain when T-62 merges"]
fn a_failed_step_parks_the_seat_in_needs_check() {
    let mut s = Store::open_in_memory().unwrap();
    admin::init(&mut s, &Env { ctx: None, pane_id: None, admin: true }, CONFIG, T0).unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse { code: "worktree_exists".into() });
    assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap().is_empty());
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::NeedsCheck);
    assert!(seat.state_reason.unwrap_or_default().contains("worktree"));
}
