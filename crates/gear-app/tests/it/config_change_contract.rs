//! Contract for the config change of `gear admin init` (T-6): a registered
//! project takes the **next** config version. A new lane becomes a queued
//! seat, a lane that is dropped is refused, and the store hands the new
//! config (its models included) to whoever reads it.

use gear_app::admin::init;
use gear_app::config;
use gear_app::Env;
use gear_model::{ErrorCode, SeatRole, SeatState};
use gear_store::Store;

const T0: i64 = 1_800_000_000;

const HEAT: &str = r#"
[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"
"#;

const PRICING: &str = r#"
[[lanes]]
name = "pricing"
duty = "price engine"
owns = ["src/pricing/"]
kind = "opencode"
profile = "opencode"
"#;

/// A config with the given lanes and lane model chain.
fn config_with(lanes: &str, lane_models: &str) -> String {
    format!(
        r#"
[project]
id = "oxford"
herdr_session = "selyra"
repo = "/repo"

[quota]
lanes = 3
temps = 1

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"
{lanes}
[models]
lanes = [{lane_models}]
review = ["gpt-6.1-sol"]
"#
    )
}

/// The registered config: one lane, one model chain.
fn config() -> String {
    config_with(HEAT, "\"minimax-cn/MiniMax-M3.1-Flash\"")
}

fn admin() -> Env {
    Env { ctx: None, pane_id: None, admin: true }
}

#[test]
fn a_changed_models_chain_is_the_next_config_version() {
    let mut s = Store::open_in_memory().unwrap();
    init(&mut s, &admin(), &config(), T0).unwrap();

    let changed = config_with(HEAT, "\"deepseek/deepseek-flash\"");
    let r = init(&mut s, &admin(), &changed, T0 + 1).unwrap();
    assert_eq!(r.config_version, 2);

    // The store holds the new config as the current version.
    let row = s.read(|tx| tx.get_project("oxford")).unwrap().unwrap();
    assert_eq!(row.config_version, 2);
    assert_eq!(row.config_toml, changed);

    // And it hands the new models to whoever reads the config.
    let current = config::latest(&mut s, "oxford").unwrap();
    assert_eq!(current.models.lanes, vec!["deepseek/deepseek-flash".to_string()]);
    assert_eq!(current.models.review, vec!["gpt-6.1-sol".to_string()]);
}

#[test]
fn a_new_lane_becomes_a_queued_seat() {
    let mut s = Store::open_in_memory().unwrap();
    init(&mut s, &admin(), &config(), T0).unwrap();
    // The lane that is already there is running: a config change must not
    // touch its seat.
    s.write(|tx| {
        tx.set_seat_generation("oxford:heat", 1)?;
        tx.set_seat_state("oxford:heat", SeatState::Active, Some("onboarded"), T0)
    })
    .unwrap();

    let more = config_with(&format!("{HEAT}{PRICING}"), "\"minimax-cn/MiniMax-M3.1-Flash\"");
    let r = init(&mut s, &admin(), &more, T0 + 1).unwrap();
    assert_eq!(r.config_version, 2);
    assert!(r.seats.contains(&"oxford:pricing".to_string()), "{:?}", r.seats);

    // The new lane is registered and waits for the engine.
    let pricing = s.read(|tx| tx.get_seat("oxford:pricing")).unwrap().unwrap();
    assert_eq!(
        (pricing.role, pricing.state, pricing.generation),
        (SeatRole::Lane, SeatState::Queued, 0)
    );
    assert_eq!(pricing.profile, "opencode");

    // The lane that was there keeps its seat, its generation and its state.
    let heat = s.read(|tx| tx.get_seat("oxford:heat")).unwrap().unwrap();
    assert_eq!((heat.state, heat.generation), (SeatState::Active, 1));
}

#[test]
fn a_dropped_lane_is_refused_and_named() {
    let mut s = Store::open_in_memory().unwrap();
    let registered = config();
    init(&mut s, &admin(), &registered, T0).unwrap();

    // The lane "heat" is not in the new config any more.
    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    let err = init(&mut s, &admin(), &fewer, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("heat"), "{}", err.detail);

    // Nothing was written: the config version and the seats are as they were.
    let row = s.read(|tx| tx.get_project("oxford")).unwrap().unwrap();
    assert_eq!(row.config_version, 1);
    assert_eq!(row.config_toml, registered);
    assert!(s.read(|tx| tx.get_seat("oxford:pricing")).unwrap().is_none());
}

/// A retired lane may leave the config: a new voyage starts with new lanes.
#[test]
fn a_retired_lane_may_be_dropped() {
    let mut s = Store::open_in_memory().unwrap();
    init(&mut s, &admin(), &config(), T0).unwrap();
    s.write(|tx| tx.set_seat_state("oxford:heat", SeatState::Retired, Some("reclaimed"), T0 + 1)).unwrap();

    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    init(&mut s, &admin(), &fewer, T0 + 2).unwrap();

    let row = s.read(|tx| tx.get_project("oxford")).unwrap().unwrap();
    assert_eq!(row.config_version, 2);
    assert!(s.read(|tx| tx.get_seat("oxford:pricing")).unwrap().is_some());
}

#[test]
fn an_invalid_config_is_refused_and_nothing_is_written() {
    let mut s = Store::open_in_memory().unwrap();
    let registered = config();
    init(&mut s, &admin(), &registered, T0).unwrap();

    // The new lane owns a path the lane "heat" owns: the config is invalid.
    let broken = config_with(
        &format!("{HEAT}{}\n", PRICING.replace("src/pricing/", "src/heat/x.rs")),
        "\"minimax-cn/MiniMax-M3.1-Flash\"",
    );
    let err = init(&mut s, &admin(), &broken, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("lanes[1].owns"), "{}", err.detail);

    let row = s.read(|tx| tx.get_project("oxford")).unwrap().unwrap();
    assert_eq!(row.config_version, 1);
    assert_eq!(row.config_toml, registered);
}

#[test]
fn the_session_and_the_bound_captain_do_not_change_mid_run() {
    let mut s = Store::open_in_memory().unwrap();
    init(&mut s, &admin(), &config(), T0).unwrap();

    let other_session = config().replace(r#"herdr_session = "selyra""#, r#"herdr_session = "other""#);
    let err = init(&mut s, &admin(), &other_session, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("session"), "{}", err.detail);

    let other_pane = config().replace(r#"pane = "w2:p1""#, r#"pane = "w2:p9""#);
    let err = init(&mut s, &admin(), &other_pane, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("pane"), "{}", err.detail);

    let other_kind = config().replace(r#"kind = "codex""#, r#"kind = "claude""#);
    let err = init(&mut s, &admin(), &other_kind, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("kind"), "{}", err.detail);

    assert_eq!(s.read(|tx| tx.get_project("oxford")).unwrap().unwrap().config_version, 1);
}

#[test]
fn a_lane_profile_follows_the_config() {
    let mut s = Store::open_in_memory().unwrap();
    init(&mut s, &admin(), &config(), T0).unwrap();

    let changed = config().replace(r#"profile = "opencode""#, r#"profile = "codex""#);
    init(&mut s, &admin(), &changed, T0 + 1).unwrap();
    let heat = s.read(|tx| tx.get_seat("oxford:heat")).unwrap().unwrap();
    assert_eq!(heat.profile, "codex");
}

#[test]
fn a_lane_may_not_take_the_seat_id_of_another_role() {
    let mut s = Store::open_in_memory().unwrap();
    init(&mut s, &admin(), &config(), T0).unwrap();

    // A new lane named "captain" would take the captain's seat id.
    let captain_lane = r#"
[[lanes]]
name = "captain"
duty = "a lane called captain"
owns = ["src/captain/"]
kind = "opencode"
profile = "opencode"
"#;
    let clash = config_with(&format!("{HEAT}{captain_lane}"), "\"minimax-cn/MiniMax-M3.1-Flash\"");
    let err = init(&mut s, &admin(), &clash, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("captain"), "{}", err.detail);

    // The captain is untouched.
    let captain = s.read(|tx| tx.get_seat("oxford:captain")).unwrap().unwrap();
    assert_eq!(captain.role, SeatRole::Captain);
    assert_eq!(captain.profile, "codex");
}
