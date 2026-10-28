//! Contract for the config change of `gear admin init` (T-6): a registered
//! project takes the **next** config version. A new lane becomes a queued
//! seat, a lane that is dropped is refused, and the store hands the new
//! config (its models included) to whoever reads it.

use gear_app::admin::init_with_home;
use gear_app::config;
use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, SeatState};
use gear_store::Store;

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w2:p1";

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
id = "harbor"
herdr_session = "marina"
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
on_quota = {{ lanes = "wait", review = "wait" }}

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#
    )
}

/// The registered config: one lane, one model chain.
fn config() -> String {
    config_with(HEAT, "\"minimax-cn/MiniMax-M3.1-Flash\"")
}

fn admin() -> Env {
    Env {
        ctx: None,
        pane_id: None,
        admin: true,
    }
}

/// A plan with one NEW task on `lane`, applied by the captain.
fn apply_task_on(
    s: &mut Store,
    lane: &str,
    base: i64,
    request_id: &str,
) -> gear_app::plan::ApplyResult {
    let text = format!(
        "<!-- gear-plan project=harbor base={base} -->\n# Queue\n## NEW [must] lane={lane}\ngoal: G\naccept: a\nestimate: 30\ndeps: -\n"
    );
    let draft = parse(&text).unwrap();
    let env = Env {
        ctx: Some(GearCtx {
            project: "harbor".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    };
    let fp = Fingerprint {
        pane_id: CAP_PANE.into(),
        kind: Some("codex".into()),
        session_ref: None,
    };
    apply(s, &env, Some(&fp), None, &draft, &text, request_id, T0 + 3).unwrap()
}

#[test]
fn a_changed_models_chain_is_the_next_config_version() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();

    let changed = config_with(HEAT, "\"deepseek/deepseek-flash\"");
    let r = init_with_home(
        &mut s,
        &admin(),
        &changed,
        Some(crate::skill_home()),
        T0 + 1,
    )
    .unwrap();
    assert_eq!(r.config_version, 2);

    // The store holds the new config as the current version.
    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 2);
    assert_eq!(row.config_toml, changed);

    // And it hands the new models to whoever reads the config.
    let current = config::latest(&mut s, "harbor").unwrap();
    let lanes: Vec<&str> = current
        .models
        .lanes
        .iter()
        .map(|entry| entry.model())
        .collect();
    assert_eq!(lanes, vec!["deepseek/deepseek-flash"]);
    let review: Vec<&str> = current
        .models
        .review
        .iter()
        .map(|entry| entry.model())
        .collect();
    assert_eq!(review, vec!["gpt-6.1-sol"]);
}

#[test]
fn a_new_lane_becomes_a_queued_seat() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();
    // The lane that is already there is running: a config change must not
    // touch its seat.
    s.write(|tx| {
        tx.set_seat_generation("harbor:heat", 1)?;
        tx.set_seat_state("harbor:heat", SeatState::Active, Some("onboarded"), T0)
    })
    .unwrap();

    let more = config_with(
        &format!("{HEAT}{PRICING}"),
        "\"minimax-cn/MiniMax-M3.1-Flash\"",
    );
    let r = init_with_home(&mut s, &admin(), &more, Some(crate::skill_home()), T0 + 1).unwrap();
    assert_eq!(r.config_version, 2);
    assert!(
        r.seats.contains(&"harbor:pricing".to_string()),
        "{:?}",
        r.seats
    );

    // The new lane is registered and waits for the engine.
    let pricing = s.read(|tx| tx.get_seat("harbor:pricing")).unwrap().unwrap();
    assert_eq!(
        (pricing.role, pricing.state, pricing.generation),
        (SeatRole::Lane, SeatState::Queued, 0)
    );
    assert_eq!(pricing.profile, "opencode");

    // The lane that was there keeps its seat, its generation and its state.
    let heat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!((heat.state, heat.generation), (SeatState::Active, 1));
}

#[test]
fn a_dropped_lane_is_refused_and_named() {
    let mut s = Store::open_in_memory().unwrap();
    let registered = config();
    init_with_home(&mut s, &admin(), &registered, Some(crate::skill_home()), T0).unwrap();

    // The lane "heat" is not in the new config any more.
    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    let err =
        init_with_home(&mut s, &admin(), &fewer, Some(crate::skill_home()), T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("heat"), "{}", err.detail);

    // Nothing was written: the config version and the seats are as they were.
    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 1);
    assert_eq!(row.config_toml, registered);
    assert!(s
        .read(|tx| tx.get_seat("harbor:pricing"))
        .unwrap()
        .is_none());
}

/// A retired lane may leave the config: a new voyage starts with new lanes.
#[test]
fn a_retired_lane_may_be_dropped() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();
    s.write(|tx| tx.set_seat_state("harbor:heat", SeatState::Retired, Some("reclaimed"), T0 + 1))
        .unwrap();

    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    init_with_home(&mut s, &admin(), &fewer, Some(crate::skill_home()), T0 + 2).unwrap();

    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 2);
    assert!(s
        .read(|tx| tx.get_seat("harbor:pricing"))
        .unwrap()
        .is_some());
}

/// T-247: a retired lane that is in the config stays retired through a
/// re-apply — a config change starts no agent — and a task that names it is
/// what brings it back.
#[test]
fn a_retired_lane_stays_retired_until_a_task_names_it() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();
    s.write(|tx| {
        tx.set_seat_generation("harbor:heat", 1)?;
        tx.set_seat_state("harbor:heat", SeatState::Retired, Some("reclaimed"), T0 + 1)
    })
    .unwrap();

    // The lane "heat" is in the new config again: a re-apply alone (here the
    // model chain changed) does not register it again.
    let again = config_with(HEAT, "\"deepseek/deepseek-flash\"");
    init_with_home(&mut s, &admin(), &again, Some(crate::skill_home()), T0 + 2).unwrap();
    let heat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(heat.state, SeatState::Retired, "a re-apply starts no agent");
    assert_eq!(heat.state_reason.as_deref(), Some("reclaimed"));
    assert_eq!(heat.generation, 1);
    assert_eq!(heat.role, SeatRole::Lane);

    // A NEW task that names the lane brings it back.
    let applied = apply_task_on(&mut s, "heat", 0, "r1");
    assert!(
        applied.staffed.contains(&"start heat".to_string()),
        "{:?}",
        applied.staffed
    );
    let heat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(
        heat.state,
        SeatState::Queued,
        "the engine onboards it again"
    );
    assert_eq!(heat.state_reason.as_deref(), Some("a task names it again"));
    // The seat is the same one: its generation and its history stay.
    assert_eq!(heat.generation, 1);
    assert_eq!(heat.role, SeatRole::Lane);
}

/// T-247: a retired lane that is not in the config stays retired whatever its
/// tasks: only the captain brings such a lane back, by putting it back in the
/// config.
#[test]
fn a_retired_lane_out_of_the_config_stays_retired() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();
    s.write(|tx| {
        tx.set_seat_generation("harbor:heat", 1)?;
        tx.set_seat_state("harbor:heat", SeatState::Retired, Some("reclaimed"), T0 + 1)
    })
    .unwrap();

    // The retired lane leaves the config: a new voyage starts with new lanes.
    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    init_with_home(&mut s, &admin(), &fewer, Some(crate::skill_home()), T0 + 2).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_seat("harbor:heat"))
            .unwrap()
            .unwrap()
            .state,
        SeatState::Retired
    );

    // A NEW task names it, and it stays retired all the same.
    let applied = apply_task_on(&mut s, "heat", 0, "r1");
    assert!(
        !applied.staffed.iter().any(|line| line == "start heat"),
        "{:?}",
        applied.staffed
    );
    let heat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(heat.state, SeatState::Retired);
    assert_eq!(heat.state_reason.as_deref(), Some("reclaimed"));
}

#[test]
fn an_invalid_config_is_refused_and_nothing_is_written() {
    let mut s = Store::open_in_memory().unwrap();
    let registered = config();
    init_with_home(&mut s, &admin(), &registered, Some(crate::skill_home()), T0).unwrap();

    // The new lane owns a path the lane "heat" owns: the config is invalid.
    let broken = config_with(
        &format!(
            "{HEAT}{}\n",
            PRICING.replace("src/pricing/", "src/heat/x.rs")
        ),
        "\"minimax-cn/MiniMax-M3.1-Flash\"",
    );
    let err =
        init_with_home(&mut s, &admin(), &broken, Some(crate::skill_home()), T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("lanes[1].owns"), "{}", err.detail);

    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 1);
    assert_eq!(row.config_toml, registered);
}

#[test]
fn the_session_and_the_bound_captain_do_not_change_mid_run() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();

    let other_session =
        config().replace(r#"herdr_session = "marina""#, r#"herdr_session = "other""#);
    let err = init_with_home(
        &mut s,
        &admin(),
        &other_session,
        Some(crate::skill_home()),
        T0 + 1,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("session"), "{}", err.detail);

    let other_pane = config().replace(r#"pane = "w2:p1""#, r#"pane = "w2:p9""#);
    let err = init_with_home(
        &mut s,
        &admin(),
        &other_pane,
        Some(crate::skill_home()),
        T0 + 1,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("pane"), "{}", err.detail);

    let other_kind = config().replace(r#"kind = "codex""#, r#"kind = "claude""#);
    let err = init_with_home(
        &mut s,
        &admin(),
        &other_kind,
        Some(crate::skill_home()),
        T0 + 1,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("kind"), "{}", err.detail);

    assert_eq!(
        s.read(|tx| tx.get_project("harbor"))
            .unwrap()
            .unwrap()
            .config_version,
        1
    );
}

#[test]
fn a_lane_profile_follows_the_config() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();

    let changed = config().replace(r#"profile = "opencode""#, r#"profile = "codex""#);
    init_with_home(
        &mut s,
        &admin(),
        &changed,
        Some(crate::skill_home()),
        T0 + 1,
    )
    .unwrap();
    let heat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(heat.profile, "codex");
}

#[test]
fn a_lane_may_not_take_the_seat_id_of_another_role() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), T0).unwrap();

    // A new lane named "captain" would take the captain's seat id.
    let captain_lane = r#"
[[lanes]]
name = "captain"
duty = "a lane called captain"
owns = ["src/captain/"]
kind = "opencode"
profile = "opencode"
"#;
    let clash = config_with(
        &format!("{HEAT}{captain_lane}"),
        "\"minimax-cn/MiniMax-M3.1-Flash\"",
    );
    let err =
        init_with_home(&mut s, &admin(), &clash, Some(crate::skill_home()), T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("captain"), "{}", err.detail);

    // The captain is untouched.
    let captain = s.read(|tx| tx.get_seat("harbor:captain")).unwrap().unwrap();
    assert_eq!(captain.role, SeatRole::Captain);
    assert_eq!(captain.profile, "codex");
}
