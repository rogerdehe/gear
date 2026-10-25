//! Contract for gear_app::admin::init (T-57).

use gear_app::admin::init_with_home;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, SeatState};
use gear_store::Store;

const T0: i64 = 1_800_000_000;

const CONFIG: &str = r#"
[project]
id = "harbor"
herdr_session = "marina"
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
fn init_registers_project_and_queued_seats() {
    let mut s = Store::open_in_memory().unwrap();
    let r = init_with_home(&mut s, &admin(), CONFIG,Some(crate::skill_home()), T0).unwrap();
    assert_eq!((r.project.as_str(), r.config_version), ("harbor", 1));
    assert_eq!(r.seats, vec!["harbor:captain", "harbor:heat", "harbor:pricing"]);
    let p = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!((p.herdr_session.as_str(), p.config_toml.as_str(), p.quota_lanes, p.plan_version), ("marina", CONFIG, 2, 0));
    let seats = s.read(|tx| tx.seats_of_project("harbor")).unwrap();
    assert_eq!(seats.len(), 3);
    for seat in seats.iter().filter(|s| s.role == SeatRole::Lane) {
        assert_eq!((seat.state, seat.generation), (SeatState::Queued, 0), "{}", seat.id);
    }
    let cap = s.read(|tx| tx.get_seat("harbor:captain")).unwrap().unwrap();
    assert_eq!((cap.role, cap.profile.as_str(), cap.state, cap.generation), (SeatRole::Captain, "codex", SeatState::Active, 1));
    let b = s.read(|tx| tx.get_binding("harbor:captain", 1)).unwrap().unwrap();
    assert_eq!((b.pane_id.as_str(), b.kind.as_str(), b.status), ("w2:p1", "codex", gear_store::BindingStatus::Active));
}

#[test]
fn only_the_user_and_only_valid_new_configs() {
    let mut s = Store::open_in_memory().unwrap();
    let agent = Env { ctx: Some(GearCtx { project: "harbor".into(), seat: "heat".into(), generation: 1 }), pane_id: None, admin: true };
    assert_eq!(init_with_home(&mut s, &agent, CONFIG,Some(crate::skill_home()), T0).unwrap_err().code, ErrorCode::Forbidden, "an agent never inits");
    assert_eq!(init_with_home(&mut s, &Env::default(), CONFIG,Some(crate::skill_home()), T0).unwrap_err().code, ErrorCode::Forbidden, "GEAR_ADMIN not set");
    let overlap = CONFIG.replace(r#"owns = ["src/pricing/"]"#, r#"owns = ["src/heat/x.rs"]"#);
    let e = init_with_home(&mut s, &admin(), &overlap,Some(crate::skill_home()), T0).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert!(e.detail.contains("lanes[1].owns"), "{}", e.detail);
    assert!(s.read(|tx| tx.get_project("harbor")).unwrap().is_none(), "nothing written");
    init_with_home(&mut s, &admin(), CONFIG,Some(crate::skill_home()), T0).unwrap();
    // T-6: a re-init is a config change, not a second registration: the new
    // config is the next config version.
    let again = init_with_home(&mut s, &admin(), CONFIG,Some(crate::skill_home()), T0 + 1).unwrap();
    assert_eq!(again.config_version, 2);
}

/// A home with the GEAR skill installed for every kind, in a fresh temp dir.
fn home_with_skills() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for agent in gear_app::skill::SkillAgent::ALL {
        gear_app::skill::install(*agent, home.path()).unwrap();
    }
    home
}

/// T-83 (trial G06): `init` refuses a config that uses a kind whose GEAR skill
/// is not installed, naming the command — and writes nothing.
#[test]
fn a_kind_without_its_skill_is_refused() {
    let mut s = Store::open_in_memory().unwrap();
    let home = tempfile::tempdir().unwrap(); // no skill at all
    let error = init_with_home(&mut s, &admin(), CONFIG, Some(home.path()), T0).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("GEAR skill is not installed"), "{}", error.detail);
    assert!(
        error.detail.contains("gear admin install-skill --agent codex"),
        "the fix is named: {}",
        error.detail
    );
    assert!(s.read(|tx| tx.get_project("harbor")).unwrap().is_none(), "nothing written");

    // With the skill there, the same config registers.
    let home = home_with_skills();
    init_with_home(&mut s, &admin(), CONFIG, Some(home.path()), T0).unwrap();
    assert_eq!(s.read(|tx| tx.get_project("harbor")).unwrap().unwrap().config_version, 1);
}

/// Every seat role is checked, and a re-init that adds a kind without its
/// skill is refused with the project left as it was.
#[test]
fn a_lane_or_reviewer_kind_without_its_skill_is_refused_on_re_init() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    init_with_home(&mut s, &admin(), CONFIG, Some(home.path()), T0).unwrap();

    // A profile for a kind GEAR has no skill install for at all.
    let custom = "[profiles.custom]\nkind = \"gemini\"\nunattended_args = [\"--yolo\"]\n";
    // The captain keeps codex (installed); a lane, and then the reviewer
    // (temps), move to that kind.
    for bad in [
        format!(
            "{CONFIG}\n{custom}"
        )
        .replace(
            "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"opencode\"\nprofile = \"opencode\"",
            "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"gemini\"\nprofile = \"custom\"",
        ),
        // The reviewer (temps) too.
        format!("{CONFIG}\n{custom}\n[temps]\nkind = \"gemini\"\nprofile = \"custom\"\n"),
    ] {
        let error = init_with_home(&mut s, &admin(), &bad, Some(home.path()), T0 + 1).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
        assert!(error.detail.contains("gemini"), "{}", error.detail);
        // The project is exactly as it was: version 1, the old config text.
        let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
        assert_eq!((project.config_version, project.config_toml.as_str()), (1, CONFIG));
    }
}
