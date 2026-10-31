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
    Env {
        ctx: None,
        pane_id: None,
        admin: true,
    }
}

#[test]
fn init_registers_project_and_queued_seats() {
    let mut s = Store::open_in_memory().unwrap();
    let r = init_with_home(
        &mut s,
        &admin(),
        CONFIG,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    assert_eq!((r.project.as_str(), r.config_version), ("harbor", 1));
    assert_eq!(
        r.seats,
        vec!["harbor:captain", "harbor:heat", "harbor:pricing"]
    );
    let p = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(
        (
            p.herdr_session.as_str(),
            p.config_toml.as_str(),
            p.quota_lanes,
            p.plan_version
        ),
        ("marina", CONFIG, 2, 0)
    );
    let seats = s.read(|tx| tx.seats_of_project("harbor")).unwrap();
    assert_eq!(seats.len(), 3);
    for seat in seats.iter().filter(|s| s.role == SeatRole::Lane) {
        assert_eq!(
            (seat.state, seat.generation),
            (SeatState::Queued, 0),
            "{}",
            seat.id
        );
    }
    let cap = s.read(|tx| tx.get_seat("harbor:captain")).unwrap().unwrap();
    assert_eq!(
        (cap.role, cap.profile.as_str(), cap.state, cap.generation),
        (SeatRole::Captain, "codex", SeatState::Active, 1)
    );
    let b = s
        .read(|tx| tx.get_binding("harbor:captain", 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        (b.pane_id.as_str(), b.kind.as_str(), b.status),
        ("w2:p1", "codex", gear_store::BindingStatus::Active)
    );
}

#[test]
fn only_the_user_and_only_valid_new_configs() {
    let mut s = Store::open_in_memory().unwrap();
    let agent = Env {
        ctx: Some(GearCtx {
            project: "harbor".into(),
            seat: "heat".into(),
            generation: 1,
        }),
        pane_id: None,
        admin: true,
    };
    assert_eq!(
        init_with_home(&mut s, &agent, CONFIG, Some(crate::skill_home()), false, T0)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden,
        "an agent never inits"
    );
    assert_eq!(
        init_with_home(
            &mut s,
            &Env::default(),
            CONFIG,
            Some(crate::skill_home()),
            false,
            T0
        )
        .unwrap_err()
        .code,
        ErrorCode::Forbidden,
        "GEAR_ADMIN not set"
    );
    let overlap = CONFIG.replace(r#"owns = ["src/pricing/"]"#, r#"owns = ["src/heat/x.rs"]"#);
    let e = init_with_home(
        &mut s,
        &admin(),
        &overlap,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert!(e.detail.contains("lanes[1].owns"), "{}", e.detail);
    assert!(
        s.read(|tx| tx.get_project("harbor")).unwrap().is_none(),
        "nothing written"
    );
    init_with_home(
        &mut s,
        &admin(),
        CONFIG,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    // T-6: a re-init is a config change, not a second registration: the new
    // config is the next config version.
    let again = init_with_home(
        &mut s,
        &admin(),
        CONFIG,
        Some(crate::skill_home()),
        false,
        T0 + 1,
    )
    .unwrap();
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
    let r = init_with_home(&mut s, &admin(), &config, Some(home.path()), false, T0).unwrap();
    assert_eq!(r.config_version, 1);
    let seat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(seat.profile, "pi");
    // The reviewer of kind pi is accepted too: the init needs its skill and
    // its profile, and it got both.
    let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert!(
        project.config_toml.contains("[temps]"),
        "{}",
        project.config_toml
    );
}

/// T-495: `init` accepts a lane of kind `omp` (oh-my-pi): its built-in
/// profile runs unattended (`--auto-approve`) and its skill is installed in
/// `~/.omp/agent/skills`.
#[test]
fn a_lane_of_kind_omp_is_accepted() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    let config = CONFIG.replace(
        "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"opencode\"\nprofile = \"opencode\"",
        "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"omp\"\nprofile = \"omp\"",
    );
    assert_ne!(config, CONFIG, "the config changed");
    let r = init_with_home(&mut s, &admin(), &config, Some(home.path()), false, T0).unwrap();
    assert_eq!(r.config_version, 1);
    let seat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(seat.profile, "omp");
    let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert!(
        project.config_toml.contains("kind = \"omp\""),
        "{}",
        project.config_toml
    );
}

/// T-497: `init` accepts a lane of kind `agy` (the built-in profile runs
/// unattended with `--dangerously-skip-permissions`, and its skill installs
/// into `~/.gemini/antigravity-cli/skills`).
#[test]
fn a_lane_of_kind_agy_is_accepted() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    let config = CONFIG.replace(
        "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"opencode\"\nprofile = \"opencode\"",
        "[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"agy\"\nprofile = \"agy\"",
    );
    assert_ne!(config, CONFIG, "the config changed");
    let r = init_with_home(&mut s, &admin(), &config, Some(home.path()), false, T0).unwrap();
    assert_eq!(r.config_version, 1);
    let seat = s.read(|tx| tx.get_seat("harbor:heat")).unwrap().unwrap();
    assert_eq!(seat.profile, "agy");
    let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert!(
        project.config_toml.contains("kind = \"agy\""),
        "{}",
        project.config_toml
    );
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
    let error =
        init_with_home(&mut s, &admin(), &config, Some(home.path()), false, T0).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error.detail.contains("gear admin install-skill --agent pi"),
        "the fix is named: {}",
        error.detail
    );
    assert!(
        s.read(|tx| tx.get_project("harbor")).unwrap().is_none(),
        "nothing written"
    );
}

/// T-83 (trial G06): `init` refuses a config that uses a kind whose GEAR skill
/// is not installed, naming the command — and writes nothing.
#[test]
fn a_kind_without_its_skill_is_refused() {
    let mut s = Store::open_in_memory().unwrap();
    let home = tempfile::tempdir().unwrap(); // no skill at all
    let error = init_with_home(&mut s, &admin(), CONFIG, Some(home.path()), false, T0).unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error.detail.contains("GEAR skill is not installed"),
        "{}",
        error.detail
    );
    assert!(
        error
            .detail
            .contains("gear admin install-skill --agent codex"),
        "the fix is named: {}",
        error.detail
    );
    assert!(
        s.read(|tx| tx.get_project("harbor")).unwrap().is_none(),
        "nothing written"
    );

    // With the skill there, the same config registers.
    let home = home_with_skills();
    init_with_home(&mut s, &admin(), CONFIG, Some(home.path()), false, T0).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_project("harbor"))
            .unwrap()
            .unwrap()
            .config_version,
        1
    );
}

/// Every seat role is checked, and a re-init that adds a kind without its
/// skill is refused with the project left as it was.
#[test]
fn a_lane_or_reviewer_kind_without_its_skill_is_refused_on_re_init() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    init_with_home(&mut s, &admin(), CONFIG, Some(home.path()), false, T0).unwrap();

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
        let error = init_with_home(&mut s, &admin(), &bad, Some(home.path()), false, T0 + 1).unwrap_err();
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

    let error = init_with_home(
        &mut s,
        &admin(),
        CODEX_CHAIN_CONFIG,
        Some(home.path()),
        false,
        T0,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("opencode"), "{}", error.detail);
    assert!(
        error
            .detail
            .contains("gear admin install-skill --agent opencode"),
        "the fix is named: {}",
        error.detail
    );
    assert!(
        s.read(|tx| tx.get_project("harbor")).unwrap().is_none(),
        "nothing written"
    );

    // The missing skill installed: the same config registers.
    gear_app::skill::install(SkillAgent::OpenCode, home.path()).unwrap();
    let r = init_with_home(
        &mut s,
        &admin(),
        CODEX_CHAIN_CONFIG,
        Some(home.path()),
        false,
        T0,
    )
    .unwrap();
    assert_eq!(r.config_version, 1);

    // A re-init with a fallback on a third CLI without its skill: refused, and
    // the registered config is the old one, at the old version.
    let with_grok = CODEX_CHAIN_CONFIG.replace(
        "  { model = \"fallback\", cli = \"codex\" },",
        "  { model = \"fallback\", cli = \"codex\" },\n  { model = \"third\", cli = \"grok\" },",
    );
    assert_ne!(with_grok, CODEX_CHAIN_CONFIG, "the config changed");
    let error = init_with_home(
        &mut s,
        &admin(),
        &with_grok,
        Some(home.path()),
        false,
        T0 + 1,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error
            .detail
            .contains("gear admin install-skill --agent grok"),
        "{}",
        error.detail
    );
    let project = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(
        (project.config_version, project.config_toml.as_str()),
        (1, CODEX_CHAIN_CONFIG),
        "a refused re-init changes nothing"
    );

    // And with it, the same config is a config change.
    gear_app::skill::install(SkillAgent::Grok, home.path()).unwrap();
    let r = init_with_home(
        &mut s,
        &admin(),
        &with_grok,
        Some(home.path()),
        false,
        T0 + 2,
    )
    .unwrap();
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
        let error =
            init_with_home(&mut s, &admin(), &config, Some(home.path()), false, T0).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat, "{kind}: {}", error.detail);
        assert!(
            error
                .detail
                .contains(&format!("gear admin install-skill --agent {kind}")),
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
    init_with_home(
        &mut s,
        &admin(),
        CONFIG,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
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
        false,
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
    let items = s
        .read(|tx| tx.thread_messages("harbor", "identity:w1:p2"))
        .unwrap();
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
        false,
        T0 + 2,
    )
    .unwrap();
    assert_eq!(done.config_version, 2);
}

/// The contract's config with another project id and repository (T-256), plus
/// `branches` lines in its `[conventions]` section.
fn project_on(id: &str, repo: &std::path::Path, branches: &str) -> String {
    project_on_with(id, repo, branches, "")
}

/// The contract's config with another project id and repository (T-302), plus
/// `conventions` lines in its `[conventions]` section and whole `tail` sections
/// after it.
fn project_on_with(id: &str, repo: &std::path::Path, conventions: &str, tail: &str) -> String {
    let path = serde_json::json!(repo.to_string_lossy()).to_string();
    let text = CONFIG
        .replace("id = \"harbor\"", &format!("id = {id:?}"))
        .replace("repo = \"/repo\"", &format!("repo = {path}"))
        .replace("[conventions]\n", &format!("[conventions]\n{conventions}"));
    format!("{text}\n{tail}\n")
}

/// T-256: two projects on one repository cannot cut the same branch names. A
/// project whose three templates name `{project}` may share it, and one on
/// another repository keeps the defaults.
#[test]
fn a_second_project_on_one_repo_needs_project_in_its_branch_templates() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let shown = |path: &std::path::Path| {
        std::fs::canonicalize(path)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    let alpha = project_on("alpha", &repo, "");
    init_with_home(&mut s, &admin(), &alpha, Some(home.path()), false, T0).unwrap();

    // beta on alpha's repository with the default templates is refused,
    // naming alpha and the repository.
    let beta = project_on("beta", &repo, "");
    let e = init_with_home(&mut s, &admin(), &beta, Some(home.path()), false, T0 + 1).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert_eq!(
        e.detail,
        format!(
            "project alpha also works in {}: put {{project}} in conventions.task_branch, review_branch and seat_branch so the two projects' branches cannot collide",
            shown(&repo)
        )
    );
    assert!(
        s.read(|tx| tx.get_project("beta")).unwrap().is_none(),
        "nothing written"
    );

    // With `{project}` in all three templates and in the voyage folder (T-302)
    // it registers.
    let templated = project_on(
        "beta",
        &repo,
        "task_branch = \"{project}/{lane}/t{n}\"\nreview_branch = \"{project}/review/t{n}\"\nseat_branch = \"{project}/home/{seat}\"\nvoyage_dir = \"gear/{project}/voyages/{milestone}\"\n",
    );
    let r = init_with_home(
        &mut s,
        &admin(),
        &templated,
        Some(home.path()),
        false,
        T0 + 2,
    )
    .unwrap();
    assert_eq!((r.project.as_str(), r.config_version), ("beta", 1));

    // A third project on another repository keeps the defaults.
    let gamma = project_on("gamma", &elsewhere, "");
    let r = init_with_home(&mut s, &admin(), &gamma, Some(home.path()), false, T0 + 3).unwrap();
    assert_eq!((r.project.as_str(), r.config_version), ("gamma", 1));

    // A re-registration is checked too: gamma moving onto alpha's repository
    // with the default templates is refused and changes nothing.
    let moved = project_on("gamma", &repo, "");
    let e = init_with_home(&mut s, &admin(), &moved, Some(home.path()), false, T0 + 4).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert!(
        e.detail.contains("project alpha also works in"),
        "{}",
        e.detail
    );
    let row = s.read(|tx| tx.get_project("gamma")).unwrap().unwrap();
    assert_eq!(
        row.config_version, 1,
        "a refused re-registration writes nothing"
    );
    assert!(row.config_toml.contains("elsewhere"), "{}", row.config_toml);
}

/// The third lane of the quota test (T-304), appended to a config.
const SAIL: &str = r#"
[[lanes]]
name = "sail"
duty = "sail trim"
owns = ["src/sail/"]
kind = "opencode"
profile = "opencode"
"#;

/// The contract's config with a third lane, so that three lanes run under a
/// quota of two.
fn three_lanes() -> String {
    CONFIG.replace("[conventions]\n", &format!("{SAIL}\n[conventions]\n"))
}

/// T-304: the lane quota counts the lanes that would run, so a first
/// registration of three lanes under `lanes = 2` is refused — a first
/// registration has no retired lane to leave out.
#[test]
fn a_first_registration_over_the_lane_quota_is_refused() {
    let mut s = Store::open_in_memory().unwrap();
    let e = init_with_home(
        &mut s,
        &admin(),
        &three_lanes(),
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert_eq!(
        e.detail,
        "quota.lanes: 3 lanes would run but the quota allows 2 (a retired lane does not count)"
    );
    assert!(
        s.read(|tx| tx.get_project("harbor")).unwrap().is_none(),
        "nothing written"
    );
    assert!(s
        .read(|tx| tx.seats_of_project("harbor"))
        .unwrap()
        .is_empty());
}

/// T-304: a retired lane keeps its seat but takes no quota slot, so a re-apply
/// may list it together with new lanes that fill the quota.
#[test]
fn a_retired_lane_takes_no_quota_slot_on_re_apply() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(
        &mut s,
        &admin(),
        CONFIG,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    // pricing is reclaimed: its seat stays, retired.
    s.write(|tx| {
        tx.set_seat_state(
            "harbor:pricing",
            SeatState::Retired,
            Some("reclaimed"),
            T0 + 1,
        )
    })
    .unwrap();

    // The new config lists heat, pricing and sail under the same quota of two:
    // heat and sail run, pricing does not count.
    let r = init_with_home(
        &mut s,
        &admin(),
        &three_lanes(),
        Some(crate::skill_home()),
        false,
        T0 + 2,
    )
    .unwrap();
    assert_eq!(r.config_version, 2);
    let seats = s.read(|tx| tx.seats_of_project("harbor")).unwrap();
    let pricing = seats.iter().find(|seat| seat.name == "pricing").unwrap();
    assert_eq!(
        pricing.state,
        SeatState::Retired,
        "a retired lane stays retired"
    );
    assert!(seats
        .iter()
        .any(|seat| seat.name == "sail" && seat.state == SeatState::Queued));
}

/// The templates that let two projects share a repository (T-256). T-372: the
/// seat branch keeps its own folder, so it is no ref above the task branches.
const TEMPLATED: &str = "task_branch = \"{project}/{lane}/t{n}\"\nreview_branch = \"{project}/review/t{n}\"\nseat_branch = \"{project}/home/{seat}\"\n";

/// T-302: two projects on one repository keep their voyages apart. With
/// `{project}` in all three branch templates but the default voyage folder
/// (`gear/voyages/{milestone}`, no project) they would overwrite each other's
/// plan, reports and reviews, so the second is refused; a `voyage_dir` naming
/// `{project}` lets it in.
#[test]
fn a_second_project_on_one_repo_needs_its_own_voyage_folder() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let shown = std::fs::canonicalize(&repo)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let alpha = project_on("alpha", &repo, TEMPLATED);
    init_with_home(&mut s, &admin(), &alpha, Some(home.path()), false, T0).unwrap();

    // The branches are kept apart, the voyages are not: refused.
    let beta = project_on("beta", &repo, TEMPLATED);
    let e = init_with_home(&mut s, &admin(), &beta, Some(home.path()), false, T0 + 1).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert_eq!(
        e.detail,
        format!(
            "project alpha also works in {shown} and keeps its voyages in the same folder gear/voyages/{{milestone}}: put {{project}} in conventions.voyage_dir so the two projects' plans, reports and reviews cannot overwrite each other"
        )
    );
    assert!(
        s.read(|tx| tx.get_project("beta")).unwrap().is_none(),
        "nothing written"
    );

    // A voyage folder that names the project registers.
    let kept_apart =
        format!("{TEMPLATED}voyage_dir = \"gear/{{project}}/voyages/{{milestone}}\"\n");
    let beta = project_on("beta", &repo, &kept_apart);
    let r = init_with_home(&mut s, &admin(), &beta, Some(home.path()), false, T0 + 2).unwrap();
    assert_eq!((r.project.as_str(), r.config_version), ("beta", 1));
}

/// T-302: two projects on one GitHub repository take the same issues unless
/// each selects them with labels of its own — whatever their local paths. The
/// repository is compared ASCII case-insensitively, an empty label list takes
/// every open issue, and one shared label puts the issue in both queues.
#[test]
fn two_projects_on_one_github_repo_need_their_own_issue_labels() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let github = |repo: &str, labels: &str| {
        format!("[github]\nrepo = {repo:?}\n\n[github.issues]\nlabels = [{labels}]\n")
    };
    let alpha = project_on_with("alpha", &first, "", &github("o/r", "\"a\""));
    init_with_home(&mut s, &admin(), &alpha, Some(home.path()), false, T0).unwrap();

    // Another local repository, the same GitHub one in another case, and
    // labels of its own: it registers.
    let beta = project_on_with("beta", &second, "", &github("O/R", "\"b\""));
    let r = init_with_home(&mut s, &admin(), &beta, Some(home.path()), false, T0 + 1).unwrap();
    assert_eq!((r.project.as_str(), r.config_version), ("beta", 1));

    // A shared label would make one issue a task of both projects: refused,
    // naming the label and alpha.
    let gamma = project_on_with("gamma", &first, "", &github("o/r", "\"a\", \"c\""));
    let e = init_with_home(&mut s, &admin(), &gamma, Some(home.path()), false, T0 + 2).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert_eq!(
        e.detail,
        "project alpha also takes issues labelled a from GitHub o/r: give each project labels of its own"
    );
    assert!(
        s.read(|tx| tx.get_project("gamma")).unwrap().is_none(),
        "nothing written"
    );

    // No labels at all takes every open issue, alpha's too: refused.
    let delta = project_on_with("delta", &second, "", &github("o/r", ""));
    let e = init_with_home(&mut s, &admin(), &delta, Some(home.path()), false, T0 + 3).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert_eq!(
        e.detail,
        "project alpha also takes issues from GitHub o/r: each project needs its own non-empty [github.issues] labels"
    );
    assert!(
        s.read(|tx| tx.get_project("delta")).unwrap().is_none(),
        "nothing written"
    );
}

/// T-302: every shared label is named in the refusal, in this config's order.
#[test]
fn shared_issue_labels_are_named_together() {
    let mut s = Store::open_in_memory().unwrap();
    let home = home_with_skills();
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let github = |labels: &str| {
        format!("[github]\nrepo = \"o/r\"\n\n[github.issues]\nlabels = [{labels}]\n")
    };
    let alpha = project_on_with("alpha", &first, "", &github("\"a\", \"z\""));
    init_with_home(&mut s, &admin(), &alpha, Some(home.path()), false, T0).unwrap();
    let beta = project_on_with("beta", &second, "", &github("\"z\", \"a\", \"q\""));
    let e = init_with_home(&mut s, &admin(), &beta, Some(home.path()), false, T0 + 1).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat);
    assert_eq!(
        e.detail,
        "project alpha also takes issues labelled z, a from GitHub o/r: give each project labels of its own"
    );
}

/// `CONFIG` with `[conventions] seat_branch` set to `template`.
fn with_seat_branch(template: &str) -> String {
    CONFIG.replace(
        "gate = \"make test\"",
        &format!("gate = \"make test\"\nseat_branch = \"{template}\""),
    )
}

/// T-373: a re-applied config whose `conventions.seat_branch` changed names
/// the worktree of every seat that has one in meta — the branch it was made
/// for, the branch new agents of the seat get, and the command that takes the
/// old tree away. Nothing is removed.
#[test]
fn a_changed_seat_branch_names_the_worktree_it_leaves_behind() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(
        &mut s,
        &admin(),
        &with_seat_branch("{project}/seat/{seat}"),
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    // Only one lane has a worktree so far: the other one and the captain say
    // nothing.
    s.write(|tx| tx.set_meta("engine.worktree_path.harbor:heat", "/work/harbor-heat"))
        .unwrap();

    let r = init_with_home(
        &mut s,
        &admin(),
        &with_seat_branch("{project}/home/{seat}"),
        Some(crate::skill_home()),
        false,
        T0 + 1,
    )
    .unwrap();
    assert_eq!(r.config_version, 2);
    assert_eq!(
        r.stale_worktrees,
        vec![
            "seat heat: its worktree /work/harbor-heat was made for branch harbor/seat/heat; \
             new agents of this seat get a worktree on harbor/home/heat; \
             remove the old one with `herdr worktree remove` once the seat runs in the new one"
                .to_string()
        ]
    );
    // Nothing is removed: the seat keeps its worktree until it runs in the new
    // one.
    let path = s
        .read(|tx| tx.get_meta("engine.worktree_path.harbor:heat"))
        .unwrap();
    assert_eq!(path.as_deref(), Some("/work/harbor-heat"));

    // An unchanged `seat_branch` leaves nothing to say.
    let r = init_with_home(
        &mut s,
        &admin(),
        &with_seat_branch("{project}/home/{seat}"),
        Some(crate::skill_home()),
        false,
        T0 + 2,
    )
    .unwrap();
    assert_eq!(r.config_version, 3);
    assert!(r.stale_worktrees.is_empty());
}
