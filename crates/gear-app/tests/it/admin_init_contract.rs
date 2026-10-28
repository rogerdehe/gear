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

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
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

/// T-113: `init` accepts a lane and a reviewer of kind `pi`: pi is a kind
/// GEAR knows (its built-in profile and its skill), and the built-in profile
/// may run unattended (`--approve`).
#[test]
fn a_lane_or_reviewer_of_kind_pi_is_accepted() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    let config = CONFIG
        .replace(
            "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"opencode\"\nprofile = \"opencode\"",
            "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"pi\"\nprofile = \"pi\"",
        )
        + "\n[temps]\nkind = \"pi\"\nprofile = \"pi\"\n";
    assert_ne!(config, CONFIG, "the config changed");
    let r = init_with_home(&mut s, &admin(), &config, Some(home.path()), T0).unwrap();
    assert_eq!(r.config_version, 1);
    let seat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(seat.profile, "pi");
    // The reviewer of kind pi is accepted too: the init needs its skill and
    // its profile, and it got both.
    let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert!(project.config_toml.contains("[temps]"), "{}", project.config_toml);
}

/// A kind GEAR knows but whose skill is not installed is refused like any
/// other (T-113): pi's skill lives in `~/.pi/agent/skills`.
#[test]
fn a_pi_kind_without_its_skill_is_refused() {
    use gear_app::skill::SkillAgent;
    let mut s = Store::open_in_memory().unwrap();
    // The captain's and the other lane's skills are there; pi's is not.
    let home = home_with(&[SkillAgent::Codex, SkillAgent::OpenCode]);
    let config = CONFIG.replace(
        "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"opencode\"\nprofile = \"opencode\"",
        "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"pi\"\nprofile = \"pi\"",
    );
    let error = init_with_home(&mut s, &admin(), &config, Some(home.path()), T0).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error.detail.contains("gear admin install-skill --agent pi"),
        "the fix is named: {}",
        error.detail
    );
    assert!(s.read(|tx| tx.get_project("harbor")).unwrap().is_none(), "nothing written");
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

/// A fresh home with only these kinds' GEAR skills installed.
fn home_with(kinds: &[gear_app::skill::SkillAgent]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for agent in kinds {
        gear_app::skill::install(*agent, home.path()).unwrap();
    }
    home
}

/// The review's reproduction (T-83 round 2, F1): codex seats whose lane chain
/// runs its primary model on OpenCode. The guard checks the CLI the chain
/// names, not only the seat's profile.
const CODEX_CHAIN_CONFIG: &str = r#"
[project]
id = "harbor"
herdr_session = "marina"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "codex"
profile = "codex"

[models]
lanes = [
  { model = "test-model", cli = "opencode" },
  { model = "fallback", cli = "codex" },
]

[conventions]
gate = "make test"
"#;

/// A model entry may run its model on another CLI than its seat's profile (T-35)
/// — a primary one, and a fallback — and that CLI's GEAR skill must be there
/// too: the review's codex-seats/OpenCode-model config is refused, the same
/// config registers once the skill is installed, and a re-init that names a
/// third CLI is refused with the project untouched.
#[test]
fn a_model_chain_cli_without_its_skill_is_refused() {
    use gear_app::skill::SkillAgent;

    let mut s = Store::open_in_memory().unwrap();
    let home = home_with(&[SkillAgent::Codex]);

    let error =
        init_with_home(&mut s, &admin(), CODEX_CHAIN_CONFIG, Some(home.path()), T0).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("opencode"), "{}", error.detail);
    assert!(
        error.detail.contains("gear admin install-skill --agent opencode"),
        "the fix is named: {}",
        error.detail
    );
    assert!(
        s.read(|tx| tx.get_project("harbor")).unwrap().is_none(),
        "nothing written"
    );

    // The missing skill installed: the same config registers.
    gear_app::skill::install(SkillAgent::OpenCode, home.path()).unwrap();
    let r = init_with_home(&mut s, &admin(), CODEX_CHAIN_CONFIG, Some(home.path()), T0).unwrap();
    assert_eq!(r.config_version, 1);

    // A re-init with a fallback on a third CLI without its skill: refused, and
    // the registered config is the old one, at the old version.
    let with_grok = CODEX_CHAIN_CONFIG.replace(
        "  { model = \"fallback\", cli = \"codex\" },",
        "  { model = \"fallback\", cli = \"codex\" },\n  { model = \"third\", cli = \"grok\" },",
    );
    assert_ne!(with_grok, CODEX_CHAIN_CONFIG, "the config changed");
    let error = init_with_home(&mut s, &admin(), &with_grok, Some(home.path()), T0 + 1).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("gear admin install-skill --agent grok"), "{}", error.detail);
    let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(
        (project.config_version, project.config_toml.as_str()),
        (1, CODEX_CHAIN_CONFIG),
        "a refused re-init changes nothing"
    );

    // And with it, the same config is a config change.
    gear_app::skill::install(SkillAgent::Grok, home.path()).unwrap();
    let r = init_with_home(&mut s, &admin(), &with_grok, Some(home.path()), T0 + 2).unwrap();
    assert_eq!(r.config_version, 2);
}

/// Role and per-lane chains are checked as well: a lane's own `models`
/// (overriding `models.lanes`), the review chain, and the captain chain.
#[test]
fn per_lane_and_role_chains_are_checked() {
    use gear_app::skill::SkillAgent;

    let home = home_with(&[SkillAgent::Codex, SkillAgent::OpenCode]);
    // The lane's own chain, which overrides `models.lanes`: grok has no skill.
    let own = CODEX_CHAIN_CONFIG.replace(
        "kind = \"codex\"\nprofile = \"codex\"\n\n[models]",
        "kind = \"codex\"\nprofile = \"codex\"\nmodels = [{ model = \"own\", cli = \"grok\" }]\n\n[models]",
    );
    // The review chain names another CLI than the seats do.
    let review = CODEX_CHAIN_CONFIG.replace(
        "[models]\nlanes = [",
        "[models]\nreview = [{ model = \"reviewer-model\", cli = \"claude\" }, { model = \"reviewer-model-2\", cli = \"codex\" }]\nlanes = [",
    );
    // The captain chain too.
    let captain = CODEX_CHAIN_CONFIG.replace(
        "[models]\nlanes = [",
        "[models]\ncaptain = [{ model = \"captain-model\", cli = \"grok\" }, { model = \"captain-model-2\", cli = \"codex\" }]\nlanes = [",
    );

    for (config, kind) in [(own, "grok"), (review, "claude"), (captain, "grok")] {
        assert_ne!(config, CODEX_CHAIN_CONFIG, "the config changed: {kind}");
        let mut s = Store::open_in_memory().unwrap();
        let error = init_with_home(&mut s, &admin(), &config, Some(home.path()), T0).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat, "{kind}: {}", error.detail);
        assert!(
            error.detail.contains(&format!("gear admin install-skill --agent {kind}")),
            "{kind}: {}",
            error.detail
        );
        assert!(
            s.read(|tx| tx.get_project("harbor")).unwrap().is_none(),
            "{kind}: nothing written"
        );
    }
}

/// T-104 (review F3): a lane that sets the captain's context and its own pane
/// id cannot change a registered project's config — the config stays, and the
/// captain gets one urgent item naming the lane's real pane. The captain's own
/// pane still re-applies a configuration.
#[test]
fn a_forged_captain_claim_cannot_re_init() {
    use gear_herdr::{fake::FakeHerdr, AgentStatus};

    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), CONFIG, Some(crate::skill_home()), T0).unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w2:p1", "codex", AgentStatus::Idle); // the captain's pane
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Idle); // the lane's pane
    const CAP: [u32; 2] = [60_001, 60_000];
    const LANE: [u32; 2] = [50_001, 50_000];
    herdr.set_pane_processes("w2:p1", Some(CAP[1]), &[]);
    herdr.set_pane_processes("w1:p2", Some(LANE[1]), &[]);

    // The captain's context with the lane's own (genuine) pane id.
    let claimed = Env {
        ctx: Some(GearCtx {
            project: "harbor".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some("w1:p2".into()),
        admin: true,
    };
    let changed = CONFIG.replace("lanes = 2", "lanes = 3");
    let error = gear_app::admin::init_checked(
        &mut s,
        &claimed,
        &changed,
        Some(&mut herdr),
        &LANE,
        Some(crate::skill_home()),
        T0 + 1,
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::NotRegistered, "{}", error.detail);
    let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(
        (project.config_version, project.config_toml.as_str()),
        (1, CONFIG),
        "a refused claim changes nothing"
    );
    let items = s.read(|tx| tx.thread_messages("harbor", "identity:w1:p2")).unwrap();
    assert_eq!(items.len(), 1, "one urgent item: {items:?}");
    assert!(items[0].urgent);
    assert!(items[0].body.contains("w1:p2"), "{}", items[0].body);

    // The captain in its own pane may re-apply the config.
    let captain = Env {
        ctx: Some(GearCtx {
            project: "harbor".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some("w2:p1".into()),
        admin: true,
    };
    let done = gear_app::admin::init_checked(
        &mut s,
        &captain,
        &changed,
        Some(&mut herdr),
        &CAP,
        Some(crate::skill_home()),
        T0 + 2,
    )
    .unwrap();
    assert_eq!(done.config_version, 2);
}
