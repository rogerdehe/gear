//! Contract for GEAR's pre-push check in a worktree (T-72): real git, a local
//! bare repository as the registered destination, the real `gear push-check`.
//!
//! The hook is installed per worktree; a push to the registered destination goes
//! through, a push anywhere else is refused by the hook, and the repository's own
//! `pre-push` hook still runs afterwards.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory, unique per tag and process.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gear-pushhook-{}-{tag}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// git with the fixture's identity, no signing and no user hooks.
fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@localhost")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@localhost")
        .output()
        .expect("run git")
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bin directory whose `gear` is the binary under test, so the hook — a
/// script that runs `gear push-check` — runs the real command.
fn gear_on_path(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("the bin directory");
    let gear = bin.join("gear");
    std::fs::write(&gear, format!("#!/bin/sh\nexec '{}' \"$@\"\n", env!("CARGO_BIN_EXE_gear")))
        .expect("the gear wrapper");
    let mut perms = std::fs::metadata(&gear).expect("the wrapper").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&gear, perms).expect("an executable wrapper");
    bin
}

/// Push from `dir` as the lane's own push: the agent's environment (its
/// context and store) and `PATH` holding the wrapper, so the hook finds `gear`.
fn push(dir: &Path, bin: &Path, db: &Path, remote: &str, reference: &str) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["push", remote, reference])
        .env("GEAR_DB", db)
        .env("GEAR_CTX", "p:heat:1")
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()))
        .output()
        .expect("run git push")
}

/// T-72, F2: a repository whose path holds an apostrophe still gets a valid
/// hook — the chained repository hook is a shell literal, quotes and all.
#[test]
fn a_quote_in_the_path_still_makes_a_valid_hook() {
    let dir = scratch("quote");
    let repo = dir.join("captain's-repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    ok(&repo, &["init", "-q", "-b", "dev"]);
    std::fs::write(repo.join("README.md"), "fixture\n").expect("a file");
    ok(&repo, &["add", "."]);
    ok(&repo, &["commit", "-qm", "base"]);
    let work = dir.join("work's");
    ok(&repo, &["worktree", "add", "-q", "-b", "heat/t1", work.to_str().unwrap()]);
    // The repository's own hook whose path holds the quote too.
    let own_dir = repo.join(".git").join("hooks");
    std::fs::create_dir_all(&own_dir).expect("the hooks directory");
    std::fs::write(own_dir.join("pre-push"), "#!/bin/sh\nexit 0\n").expect("the own hook");
    let mut perms = std::fs::metadata(own_dir.join("pre-push")).expect("the own hook").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(own_dir.join("pre-push"), perms).expect("an executable own hook");

    gear_engine::hooks::install(&work).expect("the hook installs");

    let hooks = ok(&work, &["config", "--worktree", "--get", "core.hooksPath"]);
    let script = std::fs::read_to_string(Path::new(hooks.trim()).join("pre-push")).expect("the hook");
    // The script is valid shell and its own-hook line is one literal.
    let out = Command::new("sh").arg("-n").arg(Path::new(hooks.trim()).join("pre-push")).output().expect("run sh");
    assert!(
        out.status.success(),
        "the generated hook is not valid shell: {}\n{script}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_push_to_the_registered_destination_goes_through_and_another_is_refused() {
    let dir = scratch("hook");
    // The registered destination, another bare repository, and a working repo
    // whose origin is the first.
    let registered = dir.join("registered.git");
    let other = dir.join("other.git");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    ok(&dir, &["init", "-q", "--bare", "-b", "dev", registered.to_str().unwrap()]);
    ok(&dir, &["init", "-q", "--bare", "-b", "dev", other.to_str().unwrap()]);
    ok(&repo, &["init", "-q", "-b", "dev"]);
    std::fs::write(repo.join("README.md"), "fixture\n").expect("a file");
    ok(&repo, &["add", "."]);
    ok(&repo, &["commit", "-qm", "base"]);
    ok(&repo, &["remote", "add", "origin", registered.to_str().unwrap()]);
    ok(&repo, &["remote", "add", "elsewhere", other.to_str().unwrap()]);

    // The worktree GEAR would make, with the hook installed in it.
    let work = dir.join("work");
    ok(&repo, &["worktree", "add", "-q", "-b", "heat/t1", work.to_str().unwrap()]);
    let bin = gear_on_path(&dir);
    gear_engine::hooks::install(&work).expect("the hook installs");
    assert!(gear_engine::hooks::installed(&work), "the hook is in place");

    // The integration branch is registered; a task branch goes through.
    // `gear push-check` compares against the project's frozen destinations, so
    // the check is driven by a store here: the hook asks the same command the
    // engine's merge does.
    let db = dir.join("gear.db");
    let store = seed(&db, &repo, &registered);
    drop(store);

    let allowed = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["push-check", "origin", registered.to_str().unwrap()])
        .env("GEAR_DB", &db)
        .env("GEAR_CTX", "p:heat:1")
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()))
        .current_dir(&work)
        .output()
        .expect("run gear");
    assert_eq!(
        allowed.status.code(),
        Some(0),
        "the registered destination is allowed: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );

    // A lane's own push: the hook runs the check. `elsewhere` is not registered.
    let refused = push(&work, &bin, &db, "elsewhere", "heat/t1");
    assert!(!refused.status.success(), "a push to an unregistered repo went through");
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(
        err.contains("push refused") || err.contains("FORBIDDEN") || err.contains("refused"),
        "the hook refused with: {err}"
    );
    // Nothing landed on the other repository.
    let other_refs = ok(&other, &["for-each-ref", "--format=%(refname)"]);
    assert!(!other_refs.contains("heat/t1"), "{other_refs}");

    // A push to the registered destination goes through — and the repository's
    // own pre-push hook, chained behind GEAR's, still runs.
    let marker = dir.join("own-hook-ran");
    let own = repo.join(".git").join("hooks").join("pre-push");
    std::fs::create_dir_all(own.parent().unwrap()).expect("the hooks directory");
    std::fs::write(
        &own,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    )
    .expect("the repository's own hook");
    let mut perms = std::fs::metadata(&own).expect("the own hook").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&own, perms).expect("an executable own hook");
    // Reinstall: the own hook must be chained, not lost.
    gear_engine::hooks::install(&work).expect("the hook installs again");

    let pushed = push(&work, &bin, &db, "origin", "heat/t1");
    assert!(
        pushed.status.success(),
        "a push to the registered destination was refused: {}",
        String::from_utf8_lossy(&pushed.stderr)
    );
    assert!(marker.exists(), "the repository's own pre-push hook did not run");
    let refs = ok(&registered, &["for-each-ref", "--format=%(refname)"]);
    assert!(refs.contains("heat/t1"), "{refs}");
}

/// A store whose project `p` registered `registered` as its only destination,
/// with a captain and a lane `heat` in the project root.
fn seed(db: &Path, repo: &Path, registered: &Path) -> gear_store::Store {
    use gear_model::SeatRole;
    use gear_store::testing;
    let mut store = gear_store::Store::open(db).expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", 1_800_000_000)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", 1_800_000_000)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", 1_800_000_000)?;
            let mut project = tx.get_project("p")?.expect("the project");
            project.config_toml = format!(
                "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {repo}\n\
                 \n[quota]\nlanes = 1\ntemps = 0\n\
                 \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n",
                repo = serde_json::json!(repo.to_string_lossy()).to_string()
            );
            tx.update_project(&project)?;
            tx.set_push_destinations(
                "p",
                "",
                // Frozen as `init` freezes them: the normalised destination.
                &[gear_app::push::normalise(&registered.to_string_lossy()).unwrap_or_default()],
                1_800_000_000,
            )?;
            Ok(())
        })
        .expect("the world");
    store
}
