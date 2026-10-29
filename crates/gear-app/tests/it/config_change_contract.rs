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
    Env { ctx: None, pane_id: None, admin: true }
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
    init_with_home(&mut s, &admin(), &config(),Some(crate::skill_home()), None, T0).unwrap();

    let changed = config_with(HEAT, "\"deepseek/deepseek-flash\"");
    let r = init_with_home(&mut s, &admin(), &changed,Some(crate::skill_home()), None, T0 + 1).unwrap();
    assert_eq!(r.config_version, 2);

    // The store holds the new config as the current version.
    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 2);
    assert_eq!(row.config_toml, changed);

    // And it hands the new models to whoever reads the config.
    let current = config::latest(&mut s, "harbor").unwrap();
    let lanes: Vec<&str> = current.models.lanes.iter().map(|entry| entry.model()).collect();
    assert_eq!(lanes, vec!["deepseek/deepseek-flash"]);
    let review: Vec<&str> = current.models.review.iter().map(|entry| entry.model()).collect();
    assert_eq!(review, vec!["gpt-6.1-sol"]);
}

#[test]
fn a_new_lane_becomes_a_queued_seat() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(),Some(crate::skill_home()), None, T0).unwrap();
    // The lane that is already there is running: a config change must not
    // touch its seat.
    s.write(|tx| {
        tx.set_seat_generation("harbor:heat", 1)?;
        tx.set_seat_state("harbor:heat", SeatState::Active, Some("onboarded"), T0)
    })
    .unwrap();

    let more = config_with(&format!("{HEAT}{PRICING}"), "\"minimax-cn/MiniMax-M3.1-Flash\"");
    let r = init_with_home(&mut s, &admin(), &more,Some(crate::skill_home()), None, T0 + 1).unwrap();
    assert_eq!(r.config_version, 2);
    assert!(r.seats.contains(&"harbor:pricing".to_string()), "{:?}", r.seats);

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
    init_with_home(&mut s, &admin(), &registered,Some(crate::skill_home()), None, T0).unwrap();

    // The lane "heat" is not in the new config any more.
    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    let err = init_with_home(&mut s, &admin(), &fewer,Some(crate::skill_home()), None, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("heat"), "{}", err.detail);

    // Nothing was written: the config version and the seats are as they were.
    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 1);
    assert_eq!(row.config_toml, registered);
    assert!(s.read(|tx| tx.get_seat("harbor:pricing")).unwrap().is_none());
}

/// A retired lane may leave the config: a new voyage starts with new lanes.
#[test]
fn a_retired_lane_may_be_dropped() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(),Some(crate::skill_home()), None, T0).unwrap();
    s.write(|tx| tx.set_seat_state("harbor:heat", SeatState::Retired, Some("reclaimed"), T0 + 1)).unwrap();

    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    init_with_home(&mut s, &admin(), &fewer,Some(crate::skill_home()), None, T0 + 2).unwrap();

    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 2);
    assert!(s.read(|tx| tx.get_seat("harbor:pricing")).unwrap().is_some());
}

/// T-247: a retired lane that is in the config stays retired through a
/// re-apply — a config change starts no agent — and a task that names it is
/// what brings it back.
#[test]
fn a_retired_lane_stays_retired_until_a_task_names_it() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), None, T0).unwrap();
    s.write(|tx| {
        tx.set_seat_generation("harbor:heat", 1)?;
        tx.set_seat_state("harbor:heat", SeatState::Retired, Some("reclaimed"), T0 + 1)
    })
    .unwrap();

    // The lane "heat" is in the new config again: a re-apply alone (here the
    // model chain changed) does not register it again.
    let again = config_with(HEAT, "\"deepseek/deepseek-flash\"");
    init_with_home(&mut s, &admin(), &again, Some(crate::skill_home()), None, T0 + 2).unwrap();
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
    assert_eq!(heat.state, SeatState::Queued, "the engine onboards it again");
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
    init_with_home(&mut s, &admin(), &config(), Some(crate::skill_home()), None, T0).unwrap();
    s.write(|tx| {
        tx.set_seat_generation("harbor:heat", 1)?;
        tx.set_seat_state("harbor:heat", SeatState::Retired, Some("reclaimed"), T0 + 1)
    })
    .unwrap();

    // The retired lane leaves the config: a new voyage starts with new lanes.
    let fewer = config_with(PRICING, "\"minimax-cn/MiniMax-M3.1-Flash\"");
    init_with_home(&mut s, &admin(), &fewer, Some(crate::skill_home()), None, T0 + 2).unwrap();
    assert_eq!(s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap().state, SeatState::Retired);

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
    init_with_home(&mut s, &admin(), &registered,Some(crate::skill_home()), None, T0).unwrap();

    // The new lane owns a path the lane "heat" owns: the config is invalid.
    let broken = config_with(
        &format!("{HEAT}{}\n", PRICING.replace("src/pricing/", "src/heat/x.rs")),
        "\"minimax-cn/MiniMax-M3.1-Flash\"",
    );
    let err = init_with_home(&mut s, &admin(), &broken,Some(crate::skill_home()), None, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("lanes[1].owns"), "{}", err.detail);

    let row = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(row.config_version, 1);
    assert_eq!(row.config_toml, registered);
}

#[test]
fn the_session_and_the_bound_captain_do_not_change_mid_run() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(),Some(crate::skill_home()), None, T0).unwrap();

    let other_session = config().replace(r#"herdr_session = "marina""#, r#"herdr_session = "other""#);
    let err = init_with_home(&mut s, &admin(), &other_session,Some(crate::skill_home()), None, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("session"), "{}", err.detail);

    let other_pane = config().replace(r#"pane = "w2:p1""#, r#"pane = "w2:p9""#);
    let err = init_with_home(&mut s, &admin(), &other_pane,Some(crate::skill_home()), None, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("pane"), "{}", err.detail);

    let other_kind = config().replace(r#"kind = "codex""#, r#"kind = "claude""#);
    let err = init_with_home(&mut s, &admin(), &other_kind,Some(crate::skill_home()), None, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("kind"), "{}", err.detail);

    assert_eq!(s.read(|tx| tx.get_project("harbor")).unwrap().unwrap().config_version, 1);
}

#[test]
fn a_lane_profile_follows_the_config() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(),Some(crate::skill_home()), None, T0).unwrap();

    let changed = config().replace(r#"profile = "opencode""#, r#"profile = "codex""#);
    init_with_home(&mut s, &admin(), &changed,Some(crate::skill_home()), None, T0 + 1).unwrap();
    let heat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(heat.profile, "codex");
}

#[test]
fn a_lane_may_not_take_the_seat_id_of_another_role() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(),Some(crate::skill_home()), None, T0).unwrap();

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
    let err = init_with_home(&mut s, &admin(), &clash,Some(crate::skill_home()), None, T0 + 1).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("captain"), "{}", err.detail);

    // The captain is untouched.
    let captain = s.read(|tx| tx.get_seat("harbor:captain")).unwrap().unwrap();
    assert_eq!(captain.role, SeatRole::Captain);
    assert_eq!(captain.profile, "codex");
}

/// T-304: a Ready task on a retired lane brings it back only while the lane
/// quota has a free slot; the quota counts the lane seats that are not Retired.
#[test]
fn a_full_lane_quota_leaves_a_retired_lane_retired() {
    const SAIL: &str = r#"
[[lanes]]
name = "sail"
duty = "sail trim"
owns = ["src/sail/"]
kind = "opencode"
profile = "opencode"
"#;
    let mut s = Store::open_in_memory().unwrap();
    let three = config_with(
        &format!("{HEAT}{PRICING}{SAIL}"),
        "\"minimax-cn/MiniMax-M3.1-Flash\"",
    );
    init_with_home(&mut s, &admin(), &three, Some(crate::skill_home()), None, T0).unwrap();
    // sail is reclaimed, then the captain lowers the quota to two: heat and
    // pricing fill it, sail is retired and does not count (T-304).
    s.write(|tx| tx.set_seat_state("harbor:sail", SeatState::Retired, Some("reclaimed"), T0 + 1))
        .unwrap();
    let two = three.replace("lanes = 3", "lanes = 2");
    init_with_home(&mut s, &admin(), &two, Some(crate::skill_home()), None, T0 + 2).unwrap();

    // A NEW task names sail: the quota is full, so it stays retired and the
    // apply says why.
    let applied = apply_task_on(&mut s, "sail", 0, "r1");
    assert!(
        !applied.staffed.iter().any(|line| line == "start sail"),
        "{:?}",
        applied.staffed
    );
    assert!(
        applied.warnings.contains(&"lane sail stays retired: 2 lanes run and the quota allows 2; retire another lane or raise [quota] lanes".to_string()),
        "{:?}",
        applied.warnings
    );
    let sail = s.read(|tx| tx.get_seat("harbor:sail")).unwrap().unwrap();
    assert_eq!(sail.state, SeatState::Retired);
    assert_eq!(sail.state_reason.as_deref(), Some("reclaimed"));

    // Another lane retires, so a slot is free: the Ready task on sail brings
    // it back.
    s.write(|tx| {
        tx.set_seat_state("harbor:pricing", SeatState::Retired, Some("reclaimed"), T0 + 4)
    })
    .unwrap();
    let applied = apply_task_on(&mut s, "heat", 1, "r2");
    assert!(
        applied.staffed.contains(&"start sail".to_string()),
        "{:?}",
        applied.staffed
    );
    let sail = s.read(|tx| tx.get_seat("harbor:sail")).unwrap().unwrap();
    assert_eq!(sail.state, SeatState::Queued, "the engine onboards it again");
    assert_eq!(sail.state_reason.as_deref(), Some("a task names it again"));
}

/// T-304 review F1: two Ready tasks on two retired lanes and one free slot
/// revive **one** lane, not both — the count advances within the apply.
#[test]
fn two_ready_tasks_revive_only_one_lane_when_one_slot_is_free() {
    const SAIL: &str = r#"
[[lanes]]
name = "sail"
duty = "sail trim"
owns = ["src/sail/"]
kind = "opencode"
profile = "opencode"
"#;
    let mut s = Store::open_in_memory().unwrap();
    let three = config_with(
        &format!("{HEAT}{PRICING}{SAIL}"),
        "\"minimax-cn/MiniMax-M3.1-Flash\"",
    );
    init_with_home(&mut s, &admin(), &three, Some(crate::skill_home()), None, T0).unwrap();
    // pricing and sail are reclaimed; the captain lowers the quota to two, so
    // heat fills one slot and exactly one slot is free.
    s.write(|tx| {
        tx.set_seat_state("harbor:pricing", SeatState::Retired, Some("reclaimed"), T0 + 1)?;
        tx.set_seat_state("harbor:sail", SeatState::Retired, Some("reclaimed"), T0 + 1)
    })
    .unwrap();
    let two = three.replace("lanes = 3", "lanes = 2");
    init_with_home(&mut s, &admin(), &two, Some(crate::skill_home()), None, T0 + 2).unwrap();

    // One apply names both: one comes back, the other stays retired.
    let text = "<!-- gear-plan project=harbor base=0 -->\n# Queue\n\
## NEW [must] lane=pricing\ngoal: G\naccept: a\nestimate: 30\ndeps: -\n\
## NEW [must] lane=sail\ngoal: G\naccept: a\nestimate: 30\ndeps: -\n";
    let draft = parse(text).unwrap();
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
    let applied = apply(&mut s, &env, Some(&fp), None, &draft, text, "r1", T0 + 3).unwrap();

    let started: Vec<&String> = applied
        .staffed
        .iter()
        .filter(|line| line.starts_with("start "))
        .collect();
    assert_eq!(started.len(), 1, "{:?}", applied.staffed);
    let warning = format!(
        "lane {} stays retired: 2 lanes run and the quota allows 2; retire another lane or raise [quota] lanes",
        if started[0] == "start pricing" { "sail" } else { "pricing" }
    );
    assert!(applied.warnings.contains(&warning), "{:?}", applied.warnings);

    // At most the quota runs, and exactly one of the two is still Retired.
    let seats = s.read(|tx| tx.seats_of_project("harbor")).unwrap();
    let running = seats
        .iter()
        .filter(|seat| seat.role == SeatRole::Lane && seat.state != SeatState::Retired)
        .count();
    assert!(running <= 2, "{running} lanes run under a quota of 2");
    let retired: Vec<&str> = seats
        .iter()
        .filter(|seat| seat.role == SeatRole::Lane && seat.state == SeatState::Retired)
        .map(|seat| seat.name.as_str())
        .collect();
    assert_eq!(retired.len(), 1, "{retired:?}");
}
