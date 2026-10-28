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
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\n[conventions]\ngate = \"make test\"\n",
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

/// T-100: a project that is not registered yet is created by its captain (a
/// caller with no seat yet), never by another project's seat.
#[test]
fn a_first_registration_is_the_captains_not_another_seats() {
    let dir = scratch("first");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = config(&dir, &repo);

    // The user's own shell registers it.
    let db = dir.join("gear.db");
    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(config_version(&db), 1, "the user registered the project");

    // Another project's seat may not create one, whatever GEAR_ADMIN says.
    let other = scratch("first-agent");
    let other_repo = other.join("repo");
    std::fs::create_dir_all(&other_repo).expect("the repo");
    let other_db = other.join("gear.db");
    let o = init(&other, &other_db, &config(&other, &other_repo), Some("p:captain:1"), true);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.starts_with("ERROR FORBIDDEN: "), "{err}");
    assert!(err.contains("never registers another"), "{err}");
    assert_eq!(config_version(&other_db), 0, "nothing was registered");

    // A captain with no seat yet registers it.
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
             \n[github]\nrepo = \"owner/name\"\nmerge = \"pr\"\n\n[conventions]\ngate = \"make test\"\n",
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
             \n[models]\nlanes = [\n  {{ model = \"test-model\", cli = \"opencode\" }},\n  {{ model = \"fallback\", cli = \"codex\" }},\n]\n\n[conventions]\ngate = \"make test\"\n",
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

/// `gear admin init --check <config>` as a caller's process (T-228). The
/// skills are installed in the test home, since a config's kinds are checked
/// like `init` checks them.
fn check_cmd(
    home: &Path,
    db: &Path,
    config: &Path,
    ctx: Option<&str>,
    admin: bool,
    json: bool,
) -> Output {
    for agent in gear_app::skill::SkillAgent::ALL {
        gear_app::skill::install(*agent, home).expect("install the skill");
    }
    let mut args = vec!["admin", "init", "--check"];
    if json {
        args.push("--json");
    }
    args.push(config.to_str().expect("a utf-8 path"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(args)
        .env("HOME", home)
        .env("GEAR_DB", db)
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH");
    match ctx {
        Some(ctx) => cmd.env("GEAR_CTX", ctx),
        None => cmd.env_remove("GEAR_CTX"),
    };
    if admin {
        cmd.env("GEAR_ADMIN", "1");
    } else {
        cmd.env_remove("GEAR_ADMIN");
    }
    cmd.output().expect("run gear")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Every path under `root`, relative and sorted: a snapshot of the tree.
fn tree(root: &Path) -> Vec<PathBuf> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(path.strip_prefix(base).expect("under the root").to_path_buf());
            if path.is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// `gear admin init --check <config>` with no `GEAR_DB`: the ordinary shell of
/// a machine that has the skills installed and no project store yet.
fn check_cmd_without_db(home: &Path, config: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["admin", "init", "--check", config.to_str().expect("a utf-8 path")])
        .env("HOME", home)
        .env_remove("GEAR_DB")
        .env_remove("GEAR_ADMIN")
        .env_remove("GEAR_CTX")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear")
}

/// T-228: `--check` validates a config and changes nothing — it needs no
/// `GEAR_ADMIN`, opens no store and starts nothing.
#[test]
fn check_validates_a_config_and_opens_no_store() {
    let dir = scratch("check");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 1\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\
             \n[temps]\nkind = \"codex\"\nprofile = \"codex\"\n\n[conventions]\ngate = \"make test\"\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    let db = dir.join("gear.db");

    // No GEAR_ADMIN=1: a check needs nobody's consent, because it changes
    // nothing.
    let o = check_cmd(&dir, &db, &path, None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "config ok: project p, 1 lanes, 1 reviewers");
    assert!(!db.exists(), "no store was opened");

    // The JSON form names the same three facts.
    let o = check_cmd(&dir, &db, &path, None, false, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let parsed: serde_json::Value = serde_json::from_str(stdout(&o).trim()).expect("json");
    assert_eq!(parsed["ok"], serde_json::json!(true));
    assert_eq!(parsed["project"], serde_json::json!("p"));
    assert_eq!(parsed["lanes"], serde_json::json!(1));
    assert_eq!(parsed["reviewers"], serde_json::json!(1));
    assert!(!db.exists(), "still no store");

    // A seat's pane may check too: no caller check, so no store is read to
    // prove who is asking.
    let o = check_cmd(&dir, &db, &path, Some("other:heat:1"), false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(!db.exists(), "still no store");
}

/// T-228 (review F1): with no `GEAR_DB`, a check leaves the default store's
/// directories uncreated too — a valid config and a malformed one alike.
#[test]
fn check_creates_no_default_store_without_gear_db() {
    let dir = scratch("check-home");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");
    // The skills a config's kinds need, installed before the snapshot: they
    // are what a set-up machine already holds.
    for agent in gear_app::skill::SkillAgent::ALL {
        gear_app::skill::install(*agent, &home).expect("install the skill");
    }
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = config(&dir, &repo);

    let before = tree(&home);
    let o = check_cmd_without_db(&home, &path);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "config ok: project p, 0 lanes, 0 reviewers");
    assert_eq!(tree(&home), before, "a check creates no default store directory");

    // The error path changes nothing either.
    let bad = dir.join("bad.toml");
    std::fs::write(&bad, "this is not a config\n").expect("the malformed config");
    let o = check_cmd_without_db(&home, &bad);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("ERROR BAD_FORMAT"), "{}", stderr(&o));
    assert_eq!(tree(&home), before, "a failed check creates nothing either");
}

/// T-228: a config with two lanes owning the same path is refused with the
/// same issue text `init` gives, and nothing is registered.
#[test]
fn check_reports_the_issues_init_reports() {
    let dir = scratch("check-issues");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 2\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"keel\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\n[conventions]\ngate = \"make test\"\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    let db = dir.join("gear.db");

    let o = check_cmd(&dir, &db, &path, None, false, false);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let checked = stderr(&o);
    assert!(checked.contains("ERROR BAD_FORMAT"), "{checked}");
    assert!(checked.contains("lanes[1].owns"), "{checked}");
    assert!(!db.exists(), "a check writes nothing");

    // `init` on the same config says exactly the same thing.
    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert_eq!(checked, stderr(&o), "the same issue text");
    assert_eq!(config_version(&db), 0, "nothing was registered");
}

/// A config for project `id` on `repo`, with the default branch templates.
fn config_for(dir: &Path, id: &str, repo: &Path) -> PathBuf {
    let path = dir.join(format!("{id}.toml"));
    std::fs::write(
        &path,
        format!(
            "[project]\nid = {id}\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 0\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n",
            id = serde_json::json!(id).to_string(),
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    path
}

/// T-256: `gear admin init --check` refuses a config that would cut the same
/// branch names as a registered project on the same repository — the existing
/// store is only read, and a config on another repository still passes.
#[test]
fn check_refuses_a_second_project_on_the_same_repo() {
    let dir = scratch("check-clash");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("the other repo");
    let db = dir.join("gear.db");
    // The user registers `p` on the repository.
    let o = init(&dir, &db, &config_for(&dir, "p", &repo), None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let before = std::fs::metadata(&db).expect("the store").len();

    // `q` on the same repository with the default templates is refused,
    // naming `p` and the repository.
    let o = check_cmd(&dir, &db, &config_for(&dir, "q", &repo), None, false, false);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.starts_with("ERROR BAD_FORMAT: "), "{err}");
    let shown = std::fs::canonicalize(&repo).expect("the repo").to_string_lossy().into_owned();
    assert!(err.contains(&format!("project p also works in {shown}")), "{err}");
    assert!(err.contains("put {project} in conventions.task_branch"), "{err}");
    assert_eq!(std::fs::metadata(&db).expect("the store").len(), before, "a check writes nothing");

    // On another repository the same config passes.
    let o = check_cmd(&dir, &db, &config_for(&dir, "q", &elsewhere), None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stdout(&o).trim(), "config ok: project q, 0 lanes, 0 reviewers");
}
