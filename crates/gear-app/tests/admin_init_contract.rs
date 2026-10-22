//! Contract for gear_app::admin::init (T-57).

use gear_app::admin::init;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, SeatState};
use gear_store::Store;

const T0: i64 = 1_800_000_000;

const CONFIG: &str = r#"
[project]
id = "oxford"
herdr_session = "selyra"
repo = "/repo"

[quota]
lanes = 2
temps = 1

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[[lanes]]
name = "pricing"
duty = "price engine"
owns = ["src/pricing/"]
kind = "opencode"
profile = "opencode"
"#;

fn admin() -> Env {
    Env { ctx: None, pane_id: None, admin: true }
}

#[test]
#[ignore = "T-57: un-ignored by the captain when T-57 merges"]
fn init_registers_project_and_queued_seats() {
    let mut s = Store::open_in_memory().unwrap();
    let r = init(&mut s, &admin(), CONFIG, T0).unwrap();
    assert_eq!((r.project.as_str(), r.config_version), ("oxford", 1));
    assert_eq!(r.seats, vec!["oxford:captain", "oxford:heat", "oxford:pricing"]);
    let p = s.read(|tx| tx.get_project("oxford")).unwrap().unwrap();
    assert_eq!((p.herdr_session.as_str(), p.config_toml.as_str(), p.quota_lanes, p.plan_version), ("selyra", CONFIG, 2, 0));
    let seats = s.read(|tx| tx.seats_of_project("oxford")).unwrap();
    assert_eq!(seats.len(), 3);
    for seat in &seats {
        assert_eq!((seat.state, seat.generation), (SeatState::Queued, 0), "{}", seat.id);
    }
    let cap = s.read(|tx| tx.get_seat("oxford:captain")).unwrap().unwrap();
    assert_eq!((cap.role, cap.profile.as_str()), (SeatRole::Captain, "codex"));
}

#[test]
#[ignore = "T-57: un-ignored by the captain when T-57 merges"]
fn only_the_user_and_only_valid_new_configs() {
    let mut s = Store::open_in_memory().unwrap();
    let agent = Env { ctx: Some(GearCtx { project: "oxford".into(), seat: "heat".into(), generation: 1 }), pane_id: None, admin: true };
    assert_eq!(init(&mut s, &agent, CONFIG, T0).unwrap_err().code, ErrorCode::Forbidden, "an agent never inits");
    assert_eq!(init(&mut s, &Env::default(), CONFIG, T0).unwrap_err().code, ErrorCode::Forbidden, "GEAR_ADMIN not set");
    let overlap = CONFIG.replace(r#"owns = ["src/pricing/"]"#, r#"owns = ["src/heat/x.rs"]"#);
    let e = init(&mut s, &admin(), &overlap, T0).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert!(e.detail.contains("lanes[1].owns"), "{}", e.detail);
    assert!(s.read(|tx| tx.get_project("oxford")).unwrap().is_none(), "nothing written");
    init(&mut s, &admin(), CONFIG, T0).unwrap();
    assert_eq!(init(&mut s, &admin(), CONFIG, T0 + 1).unwrap_err().code, ErrorCode::BadFormat, "already registered");
}
