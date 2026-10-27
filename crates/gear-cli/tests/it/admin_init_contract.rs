//! Contract for `gear admin init` (T-86): the user's own shell, or the
//! project's own captain — any other seat, and any caller without
//! `GEAR_ADMIN=1`, is refused.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gear_store::Store;

/// A scratch directory, unique per tag and process.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gear-init-{}-{tag}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A config for project `p`, whose repository is `repo`.
fn config(dir: &Path, repo: &Path) -> PathBuf {
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    path
}

/// `gear admin init <config>` as a caller's process. No Herdr: `init` must not
/// start an engine on a test store (L-54).
fn init(home: &Path, db: &Path, config: &Path, ctx: Option<&str>, admin: bool) -> Output {
    // T-83: `init` refuses a kind whose GEAR skill is missing; these tests are
    // about the caller checks, so the skill is installed in the test home.
    for agent in gear_app::skill::SkillAgent::ALL {
        gear_app::skill::install(*agent, home).expect("install the skill");
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(["admin", "init", config.to_str().expect("a utf-8 path")])
        .env("HOME", home)
        .env("GEAR_DB", db)
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH");
    if admin {
        cmd.env("GEAR_ADMIN", "1");
    } else {
        cmd.env_remove("GEAR_ADMIN");
    }
    match ctx {
        Some(ctx) => cmd.env("GEAR_CTX", ctx),
        None => cmd.env_remove("GEAR_CTX"),
    };
    cmd.output().expect("run gear")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr.clone()).into_owned()
}

/// The config version the store holds for `p`; 0 when it is not registered.
fn config_version(db: &Path) -> i64 {
    let mut store = Store::open(db).expect("the store");
    store
        .read(|tx| Ok(tx.get_project("p")?.map(|row| row.config_version).unwrap_or(0)))
        .expect("the project")
}

/// T-100: a project that is not registered yet is created from the user's own
/// shell only. An agent — even the captain of that very project — may re-apply
/// a registered project's config (the test below), never create one.
#[test]
fn a_first_registration_is_the_users_own() {
    let dir = scratch("first");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = config(&dir, &repo);

    // The user's own shell registers it.
    let db = dir.join("gear.db");
    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(config_version(&db), 1, "the user registered the project");

    // An agent may not create one, whatever GEAR_ADMIN says.
    let other = scratch("first-agent");
    let other_repo = other.join("repo");
    std::fs::create_dir_all(&other_repo).expect("the repo");
    let other_db = other.join("gear.db");
    let o = init(&other, &other_db, &config(&other, &other_repo), Some("p:captain:1"), true);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.starts_with("ERROR FORBIDDEN: "), "{err}");
    assert!(err.contains("user's own shell"), "{err}");
    assert_eq!(config_version(&other_db), 0, "nothing was registered");

    // And a plain shell pane with no agent in it is still the user (no Herdr
    // here means the pane cannot be read: the answer is no, and it says why).
    let o = init(&other, &other_db, &config(&other, &other_repo), None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
}

/// T-104: a captain's re-init is a seat claim on a registered project, and it
/// is proven against the caller's real pane. Without a reachable Herdr the
/// claim cannot be proven, so it is refused and nothing changes — the
/// successful captain re-apply (with a fake Herdr for its pane) is covered by
/// `gear_app::admin::init_checked`'s own test.
#[test]
fn a_captains_re_init_needs_its_pane_proven() {
    let dir = scratch("reinit");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    // The user registered it once.
    let o = init(&dir, &db, &config(&dir, &repo), None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));

    let o = init(&dir, &db, &config(&dir, &repo), Some("p:captain:1"), true);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("ERROR NOT_REGISTERED"), "{err}");
    assert!(err.contains("herdr"), "the refusal says the pane cannot be read: {err}");
    assert_eq!(config_version(&db), 1, "nothing changed");
}

/// PR landing mode can be registered through the real initialization command.
#[test]
fn a_pull_request_merge_mode_is_registered() {
    let dir = scratch("pr-mode");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[github]\nrepo = \"owner/name\"\nmerge = \"pr\"\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(config_version(&db), 1, "the project was registered");
    let mut store = Store::open(&db).expect("the store");
    let cfg = gear_app::config::latest(&mut store, "p").expect("the registered config");
    assert_eq!(cfg.root_repo().github.unwrap().merge, gear_app::config::MergeMode::Pr);
}

#[test]
fn a_lane_may_not_register_the_project() {
    let dir = scratch("lane");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let o = init(&dir, &db, &config(&dir, &repo), Some("p:heat:1"), true);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("ERROR FORBIDDEN"), "{err}");
    assert!(err.contains("captain"), "the refusal names the seat to use: {err}");
    assert_eq!(config_version(&db), 0, "nothing was registered");
}

#[test]
fn another_projects_captain_may_not_register_this_one() {
    let dir = scratch("other");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let o = init(&dir, &db, &config(&dir, &repo), Some("other:captain:1"), true);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("ERROR FORBIDDEN"), "{}", stderr(&o));
    assert_eq!(config_version(&db), 0);
}

#[test]
fn a_stale_captain_generation_may_not_register() {
    let dir = scratch("stale");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let o = init(&dir, &db, &config(&dir, &repo), None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    // `init` bound the captain's seat at generation 1; 2 is not it. The claim
    // cannot be proven at all here (no Herdr), and the generation is never
    // reached — either way the registration stands.
    let o = init(&dir, &db, &config(&dir, &repo), Some("p:captain:2"), true);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("ERROR NOT_REGISTERED"), "{}", stderr(&o));
    assert_eq!(config_version(&db), 1, "the first registration stands");
}

#[test]
fn the_users_own_shell_still_registers() {
    let dir = scratch("user");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let o = init(&dir, &db, &config(&dir, &repo), None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(config_version(&db), 1);
}

#[test]
fn without_the_users_consent_nobody_registers() {
    let dir = scratch("no-admin");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    for ctx in [None, Some("p:captain:1")] {
        let o = init(&dir, &db, &config(&dir, &repo), ctx, false);
        assert_eq!(o.status.code(), Some(1), "{ctx:?}: {}", stderr(&o));
        let err = stderr(&o);
        assert!(err.contains("ERROR FORBIDDEN"), "{ctx:?}: {err}");
        assert!(err.contains("GEAR_ADMIN=1"), "{ctx:?}: {err}");
        assert_eq!(config_version(&db), 0, "{ctx:?}");
    }
}

/// T-83 (trial G06): a config whose kind has no GEAR skill installed is
/// refused, naming the command, and nothing is registered.
#[test]
fn a_kind_without_its_skill_is_refused() {
    let home = scratch("noskill");
    let db = scratch("noskill-db");
    let repo = scratch("noskill-repo");
    let config = config(&scratch("noskill-cfg"), &repo);

    // No `gear admin install-skill` here: the home has no skill at all.
    let out = {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
        cmd.args(["admin", "init", config.to_str().unwrap()])
            .env("HOME", &home)
            .env("GEAR_DB", db.join("gear.db"))
            .env("GEAR_ADMIN", "1")
            .env_remove("GEAR_CTX")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_SOCKET_PATH");
        cmd.output().expect("run gear")
    };

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let message = stderr(&out);
    assert!(message.contains("BAD_FORMAT"), "{message}");
    assert!(message.contains("GEAR skill is not installed"), "{message}");
    assert!(message.contains("gear admin install-skill --agent codex"), "{message}");
    assert_eq!(config_version(&db.join("gear.db")), 0, "nothing was registered");
}

/// T-83 (round 2, F1) through the real binary: the review's reproduction —
/// codex seats whose lane chain runs OpenCode, with only codex's skill
/// installed — is refused, and the same config registers once OpenCode's skill
/// is there.
#[test]
fn a_model_chain_cli_without_its_skill_is_refused() {
    use gear_app::skill::{install, SkillAgent};

    let home = scratch("chain-home");
    install(SkillAgent::Codex, &home).expect("install codex");
    let db = scratch("chain-db").join("gear.db");
    let repo = scratch("chain-repo");
    let dir = scratch("chain-cfg");
    let config = dir.join("gear.toml");
    std::fs::write(
        &config,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\
             \n[models]\nlanes = [\n  {{ model = \"test-model\", cli = \"opencode\" }},\n  {{ model = \"fallback\", cli = \"codex\" }},\n]\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");

    let run = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
        cmd.args(["admin", "init", config.to_str().unwrap()])
            .env("HOME", &home)
            .env("GEAR_DB", &db)
            .env("GEAR_ADMIN", "1")
            .env_remove("GEAR_CTX")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_SOCKET_PATH");
        cmd.output().expect("run gear")
    };

    let out = run();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let message = stderr(&out);
    assert!(message.contains("BAD_FORMAT"), "{message}");
    assert!(message.contains("opencode"), "{message}");
    assert!(message.contains("gear admin install-skill --agent opencode"), "{message}");
    assert_eq!(config_version(&db), 0, "nothing was registered");

    install(SkillAgent::OpenCode, &home).expect("install opencode");
    let out = run();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(config_version(&db), 1, "the same config registers");
}
