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

/// The first line of stdout: a `--check` run prints its headline, then the
/// seats it starts (T-261).
fn headline(o: &Output) -> String {
    stdout(o).lines().next().unwrap_or_default().to_string()
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
    assert_eq!(headline(&o), "config ok: project p, 1 lanes, 1 reviewers");
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
    assert_eq!(headline(&o), "config ok: project p, 0 lanes, 0 reviewers");
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
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[conventions]\ngate = \"make gate\"\n",
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
    // With no store yet there is nothing to collide with, and the check says
    // what it skipped.
    let o = check_cmd(&dir, &db, &config_for(&dir, "p", &repo), None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(stderr(&o).trim(), "cross-project check skipped: no store");
    assert!(!db.exists(), "a check creates no store");
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
    assert_eq!(headline(&o), "config ok: project q, 0 lanes, 0 reviewers");
}

/// `gear admin init <config>` as the user's own shell with no `GEAR_DB`: the
/// default store of `home` (T-256 F1).
fn init_without_db(home: &Path, config: &Path) -> Output {
    for agent in gear_app::skill::SkillAgent::ALL {
        gear_app::skill::install(*agent, home).expect("install the skill");
    }
    Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["admin", "init", config.to_str().expect("a utf-8 path")])
        .env("HOME", home)
        .env("GEAR_ADMIN", "1")
        .env_remove("GEAR_DB")
        .env_remove("GEAR_CTX")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear")
}

/// T-256 (review F1): with no `GEAR_DB`, `--check` reads the default store the
/// registration would use, so a second project on one repository is refused in
/// the user's own shell too.
#[test]
fn check_reads_the_default_store_when_gear_db_is_unset() {
    let dir = scratch("check-default");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");
    // T-316: an ordinary repository, outside the temp directory — a fixture
    // below it registers only into a store the caller names with `GEAR_DB`.
    // `elsewhere` is only ever `--check`ed, which registers nothing.
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("the other repo");

    // `p` is registered into the default store of the temporary HOME.
    let o = init_without_db(&home, &config_for(&dir, "p", &repo));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let db = home.join(".local/state/gear/gear.db");
    assert!(db.exists(), "the registration made the default store");

    // With no GEAR_DB, `--check` of `q` on the same repository still sees `p`.
    let o = check_cmd_without_db(&home, &config_for(&dir, "q", &repo));
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.starts_with("ERROR BAD_FORMAT: "), "{err}");
    let shown = std::fs::canonicalize(&repo).expect("the repo").to_string_lossy().into_owned();
    assert!(err.contains(&format!("project p also works in {shown}")), "{err}");
    assert!(!err.contains("skipped"), "the check read the default store: {err}");

    // On another repository the same config passes, and still says nothing of a
    // skipped check.
    let o = check_cmd_without_db(&home, &config_for(&dir, "q", &elsewhere));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stderr(&o).is_empty(), "{}", stderr(&o));
}

/// A two-lane config for project `p` on `repo`, with a lane chain, a review
/// chain and a lane overriding its model (T-261).
fn config_two_lanes(dir: &Path, repo: &Path) -> PathBuf {
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 2\ntemps = 1\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\", \"Cargo.toml\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"keel\"\nduty = \"everything\"\nowns = [\"docs/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\nmodels = [\"mine/model\"]\n\
             \n[temps]\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[models]\nlanes = [\n  {{ model = \"role/model\", cli = \"codex\" }},\n  {{ model = \"role/fallback\", cli = \"codex\" }},\n]\nreview = [\"review/model\", \"review/fallback\"]\n\
             \n[conventions]\ngate = \"make test\"\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    path
}

/// T-261: `--check` says which lanes the config starts, with the model each
/// one runs — its own first `models` entry, else the role chain's first — and
/// then which reviewers.
#[test]
fn check_names_every_lane_and_the_reviewers_with_their_models() {
    let dir = scratch("check-seats");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = config_two_lanes(&dir, &repo);
    let db = dir.join("gear.db");

    let o = check_cmd(&dir, &db, &path, None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "config ok: project p, 2 lanes, 1 reviewers\n\
         \x20 lane heat: codex role/model · owns src/, Cargo.toml\n\
         \x20 lane keel: codex mine/model · owns docs/\n\
         \x20 reviewers: 1 × codex review/model\n"
    );
    assert!(!db.exists(), "a check writes nothing");
}

/// T-261: the JSON form carries the same lane list and the reviewers' model.
#[test]
fn check_json_carries_the_lane_list_and_review_model() {
    let dir = scratch("check-seats-json");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = config_two_lanes(&dir, &repo);
    let db = dir.join("gear.db");

    let o = check_cmd(&dir, &db, &path, None, false, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let parsed: serde_json::Value = serde_json::from_str(stdout(&o).trim()).expect("json");
    assert_eq!(
        parsed["lane_list"],
        serde_json::json!([
            { "name": "heat", "kind": "codex", "model": "role/model" },
            { "name": "keel", "kind": "codex", "model": "mine/model" },
        ])
    );
    assert_eq!(parsed["review_model"], serde_json::json!("review/model"));
}

/// T-261: a config with neither `[temps]` nor a model chain still says who it
/// starts: no reviewers, and every model left to the seat's own CLI ("-").
#[test]
fn check_says_none_without_temps_and_dashes_without_models() {
    let dir = scratch("check-no-temps");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\
             \n[conventions]\ngate = \"make test\"\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    let db = dir.join("gear.db");

    let o = check_cmd(&dir, &db, &path, None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "config ok: project p, 1 lanes, 0 reviewers\n\
         \x20 lane heat: codex - · owns src/\n\
         \x20 reviewers: none\n"
    );

    let o = check_cmd(&dir, &db, &path, None, false, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let parsed: serde_json::Value = serde_json::from_str(stdout(&o).trim()).expect("json");
    assert_eq!(
        parsed["lane_list"],
        serde_json::json!([{ "name": "heat", "kind": "codex", "model": "-" }])
    );
    assert_eq!(parsed["review_model"], serde_json::json!("-"));
}

/// T-261: a successful `init` names the same seats and models as `--check`.
#[test]
fn init_names_the_lanes_and_reviewers_after_registering() {
    let dir = scratch("init-seats");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = config_two_lanes(&dir, &repo);
    let db = dir.join("gear.db");

    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let printed = stdout(&o);
    assert!(printed.contains("project p (config v1)"), "{printed}");
    assert!(printed.contains("  lane heat: codex role/model · owns src/, Cargo.toml"), "{printed}");
    assert!(printed.contains("  lane keel: codex mine/model · owns docs/"), "{printed}");
    assert!(printed.contains("  reviewers: 1 × codex review/model"), "{printed}");
    assert_eq!(config_version(&db), 1, "the project was registered");
}

/// A config for project `p` on `repo` that names Herdr session `session`
/// (T-269).
fn config_in_session(dir: &Path, repo: &Path, session: &str) -> PathBuf {
    let path = dir.join(format!("gear-{session}.toml"));
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = {session}\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[conventions]\ngate = \"make test\"\n",
            session = serde_json::json!(session).to_string(),
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    path
}

/// `gear admin init [--check] <config>` from a shell inside the Herdr session
/// at `socket`, over an injected `HOME` — with no `GEAR_DB` unless `db` names
/// one (T-269).
fn init_from_session(
    home: &Path,
    socket: &Path,
    config: &Path,
    check: bool,
    db: Option<&Path>,
) -> Output {
    for agent in gear_app::skill::SkillAgent::ALL {
        gear_app::skill::install(*agent, home).expect("install the skill");
    }
    let mut args = vec!["admin", "init"];
    if check {
        args.push("--check");
    }
    args.push(config.to_str().expect("a utf-8 path"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(args)
        .env("HOME", home)
        .env("GEAR_ADMIN", "1")
        .env("HERDR_SOCKET_PATH", socket)
        .env_remove("HERDR_PANE_ID")
        .env_remove("GEAR_CTX");
    match db {
        Some(db) => cmd.env("GEAR_DB", db),
        None => cmd.env_remove("GEAR_DB"),
    };
    cmd.output().expect("run gear")
}

/// T-269: one store per Herdr session since v0.4.5, so a config naming another
/// session would register into this session's store, where its engine never
/// reaches the panes — refused by `--check` and by `init`, before either says
/// or writes anything.
#[test]
fn init_refuses_a_config_for_another_herdr_session() {
    let dir = scratch("session");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");
    // T-316: a repository outside the temp directory, since a fixture below
    // it registers only into a store the caller names with `GEAR_DB`. This
    // test is about the session checks, and about the default store.
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let alpha = home.join(".config/herdr/sessions/alpha/herdr.sock");
    let refused = "project p names herdr_session beta, but this shell runs in session alpha: \
                   its store and engine are this session's; run init from a pane of session beta, \
                   or fix herdr_session";
    let beta = config_in_session(&dir, &repo, "beta");

    // `--check` refuses it before printing "config ok".
    let o = init_from_session(&home, &alpha, &beta, true, None);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.starts_with("ERROR BAD_FORMAT: "), "{err}");
    assert!(err.contains(refused), "{err}");
    assert!(!stdout(&o).contains("config ok"), "no headline: {}", stdout(&o));

    // A real init writes nothing at all.
    let o = init_from_session(&home, &alpha, &beta, false, None);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains(refused), "{}", stderr(&o));
    assert!(
        !home.join(".local/state/gear/alpha/gear.db").exists(),
        "nothing was registered"
    );

    // The session the config names passes, and registers.
    let named = config_in_session(&dir, &repo, "alpha");
    let o = init_from_session(&home, &alpha, &named, true, None);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(headline(&o), "config ok: project p, 0 lanes, 0 reviewers");
    let o = init_from_session(&home, &alpha, &named, false, None);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        home.join(".local/state/gear/alpha/gear.db").exists(),
        "registered into alpha's own store"
    );

    // The default session belongs to nobody in particular: it passes either
    // way, whatever the config names.
    let default_socket = home.join(".config/herdr/herdr.sock");
    let db = dir.join("gear.db");
    let o = init_from_session(&home, &default_socket, &beta, true, Some(&db));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(headline(&o), "config ok: project p, 0 lanes, 0 reviewers");
    let o = init_from_session(&home, &default_socket, &beta, false, Some(&db));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));

    // A store the caller names with `GEAR_DB` is not a session's: nothing is
    // checked, so a named session's socket passes too.
    let explicit = dir.join("explicit.db");
    let o = init_from_session(&home, &alpha, &beta, true, Some(&explicit));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let o = init_from_session(&home, &alpha, &beta, false, Some(&explicit));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(config_version(&explicit), 1, "registered into the named store");
}

/// T-316: a repository under the system temp directory is a fixture (M4.2
/// #35: a leftover fixture project with empty tokens appeared in the user's
/// board, its captain seat bound to the real captain's pane). A fixture
/// registers only into a store the caller names itself with `GEAR_DB`, never
/// into this session's store — the default one or a named session's. `--check`
/// is untouched, and every repository outside the temp directory registers as
/// it did before.
#[test]
fn init_refuses_a_fixture_into_the_sessions_store() {
    let dir = scratch("fixture");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the home");
    // A repository under the temp directory: the fixtures of the acceptance.
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let default_socket = home.join(".config/herdr/herdr.sock");
    let shared = home.join(".local/state/gear/gear.db");

    // `--check` still validates it: it writes nothing, so it is no threat.
    let o = init_from_session(&home, &default_socket, &config(&dir, &repo), true, None);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(headline(&o), "config ok: project p, 0 lanes, 0 reviewers");

    // A real init into the default store of this session is refused.
    let o = init_from_session(&home, &default_socket, &config(&dir, &repo), false, None);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.starts_with("ERROR BAD_FORMAT: "), "{err}");
    assert!(err.contains("lives under the temp directory"), "{err}");
    assert!(
        err.contains("a fixture registers only into a store named with GEAR_DB"),
        "{err}"
    );
    assert!(
        err.contains(&format!("never into this session's store {}", shared.display())),
        "{err}"
    );
    assert!(!shared.exists(), "nothing was registered");

    // So is one into a named session's store.
    let alpha = home.join(".config/herdr/sessions/alpha/herdr.sock");
    let alpha_store = home.join(".local/state/gear/alpha/gear.db");
    let o = init_from_session(&home, &alpha, &config_in_session(&dir, &repo, "alpha"), false, None);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(
        err.contains(&format!("never into this session's store {}", alpha_store.display())),
        "{err}"
    );
    assert!(!alpha_store.exists(), "nothing was registered");

    // A store the caller names with `GEAR_DB` is not this session's: the same
    // fixture registers there as it always did.
    let named = dir.join("named.db");
    let o = init_from_session(&home, &default_socket, &config(&dir, &repo), false, Some(&named));
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(config_version(&named), 1, "registered into the named store");

    // A repository outside the temp directory is no fixture: it registers
    // into this session's default store. A home of its own, since the store
    // named above is already the default one for `home`'s session.
    let outside = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let outside_home = dir.join("outside-home");
    std::fs::create_dir_all(&outside_home).expect("the outside home");
    let outside_socket = outside_home.join(".config/herdr/herdr.sock");
    let o = init_from_session(
        &outside_home,
        &outside_socket,
        &config(&dir, &outside),
        false,
        None,
    );
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let outside_store = outside_home.join(".local/state/gear/gear.db");
    assert_eq!(config_version(&outside_store), 1, "an ordinary repository registers");
}

/// T-315: `--check --models` asks each seat's CLI for the models it lists and
/// warns on stderr about a configured model id that is not among them — a
/// warning, exit 0 — while a plain `--check` runs no CLI at all.
#[test]
fn check_models_warns_about_a_model_its_cli_does_not_list() {
    let dir = scratch("check-models");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
             kind = \"pi\"\nprofile = \"pi\"\nmodels = [\"tok/deep-2\"]\n\
             \n[conventions]\ngate = \"make test\"\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    // A PATH with one CLI on it: a `pi` that lists `tok/deep-1`, and leaves a
    // marker behind whenever it runs, so a run that asks no CLI is visible.
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("the bin directory");
    let pi = bin.join("pi");
    std::fs::write(
        &pi,
        "#!/bin/sh\nprintf 'ran' > \"$GEAR_PI_MARKER\"\nprintf 'provider model context\\ntok deep-1 600K\\n'\n",
    )
    .expect("the fake pi");
    let mut perms = std::fs::metadata(&pi).expect("the fake pi").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&pi, perms).expect("an executable fake pi");

    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("the test home");
    for agent in gear_app::skill::SkillAgent::ALL {
        gear_app::skill::install(*agent, &home).expect("install the skill");
    }
    let marker = dir.join("pi-ran");
    let db = dir.join("gear.db");
    let run = |args: &[&str]| -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
        cmd.args(args)
            .env("HOME", &home)
            .env("GEAR_DB", &db)
            .env("PATH", &bin)
            .env("GEAR_PI_MARKER", &marker)
            .env_remove("GEAR_ADMIN")
            .env_remove("GEAR_CTX")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_SOCKET_PATH");
        cmd.output().expect("run gear")
    };
    let config = path.to_str().expect("a utf-8 path");

    let o = run(&["admin", "init", "--check", "--models", config]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(
        stderr(&o)
            .contains("warning: lane heat: model tok/deep-2 is not among the models pi lists"),
        "{}",
        stderr(&o)
    );
    assert!(marker.exists(), "`--models` asked the CLI");
    std::fs::remove_file(&marker).expect("clear the marker");

    // The plain check stays offline: the same config, no CLI run at all.
    let o = run(&["admin", "init", "--check", config]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(!stderr(&o).contains("warning:"), "{}", stderr(&o));
    assert!(!marker.exists(), "a plain --check runs no CLI");
}

/// T-321: a real init records the canonical config path in the meta row the
/// home page reads (`config.path.<project>`), so a config file that drifts
/// from the applied config can be named.
#[test]
fn a_real_init_records_the_canonical_config_path() {
    let dir = scratch("config-path");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let path = config(&dir, &repo);

    // A spelling of the same file that is not canonical: `<dir>/nested/../gear.toml`.
    std::fs::create_dir_all(dir.join("nested")).expect("the nested directory");
    let dotty = dir.join("nested").join("..").join("gear.toml");
    let o = init(&dir, &db, &dotty, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));

    let mut store = Store::open(&db).expect("the store");
    let recorded = store
        .read(|tx| tx.get_meta(&gear_app::admin::config_path_key("p")))
        .expect("the meta row");
    let canonical = std::fs::canonicalize(&path).expect("the config exists");
    assert_eq!(
        recorded.as_deref(),
        Some(canonical.to_string_lossy().as_ref()),
        "the canonical config path"
    );
}

/// T-304: `--check` refuses a config whose lanes would run past the lane
/// quota — with no store there is no retired lane to leave out, so three lanes
/// under `lanes = 2` are refused and nothing is written.
#[test]
fn check_refuses_more_lanes_than_the_quota_allows() {
    let dir = scratch("check-quota");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 2\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/heat/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"keel\"\nduty = \"everything\"\nowns = [\"src/keel/\"]\n\
             kind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"sail\"\nduty = \"everything\"\nowns = [\"src/sail/\"]\n\
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
    assert!(
        checked.contains(
            "quota.lanes: 3 lanes would run but the quota allows 2 (a retired lane does not count)"
        ),
        "{checked}"
    );
    assert!(!db.exists(), "a check writes nothing");
}

/// A config for project `p` with one lane, whose `seat_branch` is `template`.
fn lane_config(dir: &Path, repo: &Path, template: &str) -> PathBuf {
    let path = dir.join("gear-lanes.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"thermal calculation\"\nowns = [\"src/heat/\"]\nkind = \"opencode\"\nprofile = \"opencode\"\n\
             \n[conventions]\ngate = \"make test\"\nseat_branch = {template:?}\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string()
        ),
    )
    .expect("the config");
    path
}

/// T-373: a re-applied config that changes `conventions.seat_branch` prints,
/// right after the project line, the worktree of every seat that has one: the
/// branch it was made for, the branch new agents get, and the command that
/// removes the old tree.
#[test]
fn a_changed_seat_branch_prints_the_worktree_it_leaves_behind() {
    let dir = scratch("stale");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let o = init(&dir, &db, &lane_config(&dir, &repo, "{project}/seat/{seat}"), None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    // The engine made the lane's worktree; the store knows where it is.
    {
        let mut store = Store::open(&db).expect("the store");
        store
            .write(|tx| tx.set_meta("engine.worktree_path.p:heat", "/work/p-heat"))
            .expect("the meta");
    }

    let o = init(&dir, &db, &lane_config(&dir, &repo, "{project}/home/{seat}"), None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let expected = "seat heat: its worktree /work/p-heat was made for branch p/seat/heat; \
                    new agents of this seat get a worktree on p/home/heat; \
                    remove the old one with `herdr worktree remove` once the seat runs in the new one";
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    let lines: Vec<&str> = out.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("project p (config v2)"))
        .unwrap_or_else(|| panic!("{out}"));
    assert_eq!(lines[at + 1], expected, "{out}");
    assert_eq!(config_version(&db), 2);
}

/// A config for project `p` on `repo` whose gate is `gate` — the one line the
/// change-list tests rewrite (T-388).
fn config_with_gate(dir: &Path, repo: &Path, gate: &str) -> PathBuf {
    let path = dir.join("gear.toml");
    std::fs::write(
        &path,
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
             \n[quota]\nlanes = 1\ntemps = 0\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
             \n[conventions]\ngate = {gate}\n",
            repo = serde_json::json!(repo.to_string_lossy()).to_string(),
            gate = serde_json::json!(gate).to_string()
        ),
    )
    .expect("the config");
    path
}

/// T-388: `--check` says what a re-init would change against the config the
/// store holds — a line diff after the "config ok" line, the registered lines
/// with `- ` and the file's with `+ ` — and still writes nothing.
#[test]
fn check_shows_what_a_re_init_would_change() {
    let dir = scratch("check-changes");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let path = config_with_gate(&dir, &repo, "make test");
    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let before = std::fs::metadata(&db).expect("the store").len();

    // The file changes one line: the check names it, the old one and the new.
    let path = config_with_gate(&dir, &repo, "make gate");
    let o = check_cmd(&dir, &db, &path, None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "config ok: project p, 0 lanes, 0 reviewers\n\
         changes against the registered config v1:\n\
         - gate = \"make test\"\n\
         + gate = \"make gate\"\n\
         \x20 reviewers: none\n"
    );
    assert_eq!(std::fs::metadata(&db).expect("the store").len(), before, "a check writes nothing");
    assert_eq!(config_version(&db), 1, "the registration stands");

    // The JSON form carries the same lines.
    let o = check_cmd(&dir, &db, &path, None, false, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let parsed: serde_json::Value = serde_json::from_str(stdout(&o).trim()).expect("json");
    assert_eq!(
        parsed["changes"],
        serde_json::json!(["- gate = \"make test\"", "+ gate = \"make gate\""])
    );
}

/// T-388: the config the store holds gives no change, and the check says so
/// instead of an empty diff.
#[test]
fn check_says_no_change_for_the_registered_config() {
    let dir = scratch("check-same");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let db = dir.join("gear.db");
    let path = config_with_gate(&dir, &repo, "make test");
    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));

    let o = check_cmd(&dir, &db, &path, None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "config ok: project p, 0 lanes, 0 reviewers\n\
         no change against the registered config v1\n\
         \x20 reviewers: none\n"
    );

    let o = check_cmd(&dir, &db, &path, None, false, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let parsed: serde_json::Value = serde_json::from_str(stdout(&o).trim()).expect("json");
    assert_eq!(parsed["changes"], serde_json::json!([]));
}

/// T-388: only a project of the file's own id has a registered config to
/// compare with. Another id — or no store at all — is a first registration:
/// the check says nothing of a change.
#[test]
fn check_says_nothing_of_a_change_for_a_first_registration() {
    let dir = scratch("check-first");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("the other repo");
    let db = dir.join("gear.db");

    // No store yet: no change line, and an empty list in the JSON form.
    let path = config_with_gate(&dir, &repo, "make test");
    let o = check_cmd(&dir, &db, &path, None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "config ok: project p, 0 lanes, 0 reviewers\n  reviewers: none\n"
    );
    let o = check_cmd(&dir, &db, &path, None, false, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let parsed: serde_json::Value = serde_json::from_str(stdout(&o).trim()).expect("json");
    assert_eq!(parsed["changes"], serde_json::json!([]));

    // The store holds `p`; a check of `q` on another repository finds no
    // registered config of its own id.
    let o = init(&dir, &db, &path, None, true);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    let o = check_cmd(&dir, &db, &config_for(&dir, "q", &elsewhere), None, false, false);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "config ok: project q, 0 lanes, 0 reviewers\n  reviewers: none\n"
    );
    assert_eq!(config_version(&db), 1, "the check registered nothing");
}
