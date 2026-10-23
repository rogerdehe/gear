//! Contract for onboard_queued (T-62): the walking-skeleton onboarding of lanes.
//! Needs T-57 (admin init), T-58 (plan_onboard), T-59 (seat transitions).

use gear_app::{admin, Env};
use gear_engine::onboard::onboard_queued;
use gear_engine::EngineCfg;
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::builtin;
use gear_model::{SeatRole, SeatState};
use gear_store::{BindingStatus, SeatRow, Store};

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
    // the captain is bound at init (T-63), not onboarded by the engine
    assert_eq!(s.read(|tx| tx.get_seat("toy:captain")).unwrap().unwrap().state, SeatState::Active);
    // nothing to do on the next run
    assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 2).unwrap().is_empty());
}

#[test]
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

/// T-6: a lane that a config change adds is a queued seat, and the engine
/// onboards it with the **latest** config (the repo it names now).
#[test]
fn a_lane_added_by_a_config_change_is_onboarded_with_the_latest_config() {
    let mut s = Store::open_in_memory().unwrap();
    let admin_env = Env { ctx: None, pane_id: None, admin: true };
    admin::init(&mut s, &admin_env, CONFIG, T0).unwrap();
    let mut herdr = FakeHerdr::new();
    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap(),
        vec!["toy:heat".to_string()]
    );

    // The config changes: a second lane, another repo, a bigger quota.
    let changed = CONFIG
        .replace("lanes = 1", "lanes = 2")
        .replace(r#"repo = "/repo""#, r#"repo = "/repo2""#)
        + "\n[[lanes]]\nname = \"pricing\"\nduty = \"prices\"\nowns = [\"lib/\"]\nkind = \"opencode\"\nprofile = \"opencode\"\n";
    admin::init(&mut s, &admin_env, &changed, T0 + 2).unwrap();

    // The new lane is onboarded; the lane that was there is not touched.
    let active = onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 3).unwrap();
    assert_eq!(active, vec!["toy:pricing".to_string()]);
    assert!(herdr.calls.iter().any(|c| matches!(c, Call::WorktreeCreate { repo, branch, .. }
        if repo == "/repo2" && branch == "home/pricing")), "{:?}", herdr.calls);
    let seat = s.read(|tx| tx.get_seat("toy:pricing")).unwrap().unwrap();
    assert_eq!((seat.state, seat.generation), (SeatState::Active, 1));
}

/// Rework of T-6 (review P1): the config and the seat roster come from one
/// snapshot, and a queued lane that the config does not name (the state a
/// re-init landing between two reads used to produce) is **skipped**, never
/// parked — the next tick reads the newer config and onboards it.
#[test]
fn a_lane_missing_from_the_config_snapshot_is_skipped_not_parked() {
    let mut s = Store::open_in_memory().unwrap();
    let admin_env = Env { ctx: None, pane_id: None, admin: true };
    admin::init(&mut s, &admin_env, CONFIG, T0).unwrap();
    // The seat a re-init would have committed just before the roster read.
    s.write(|tx| {
        tx.insert_seat(&SeatRow {
            id: "toy:pricing".into(),
            project: "toy".into(),
            role: SeatRole::Lane,
            name: "pricing".into(),
            profile: "opencode".into(),
            generation: 0,
            state: SeatState::Queued,
            state_since: T0,
            state_reason: Some("registered".into()),
            task_id: None,
        })
    })
    .unwrap();

    let mut herdr = FakeHerdr::new();
    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap(),
        vec!["toy:heat".to_string()],
        "only the lane the config names is onboarded"
    );
    let pricing = s.read(|tx| tx.get_seat("toy:pricing")).unwrap().unwrap();
    assert_eq!(
        pricing.state,
        SeatState::Queued,
        "not parked for a config that may be about to arrive"
    );

    // The config change lands; the next tick onboards the lane.
    let changed = CONFIG.replace("lanes = 1", "lanes = 2")
        + "\n[[lanes]]\nname = \"pricing\"\nduty = \"prices\"\nowns = [\"lib/\"]\nkind = \"opencode\"\nprofile = \"opencode\"\n";
    admin::init(&mut s, &admin_env, &changed, T0 + 2).unwrap();
    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 3).unwrap(),
        vec!["toy:pricing".to_string()]
    );
    let pricing = s.read(|tx| tx.get_seat("toy:pricing")).unwrap().unwrap();
    assert_eq!((pricing.state, pricing.generation), (SeatState::Active, 1));
}
