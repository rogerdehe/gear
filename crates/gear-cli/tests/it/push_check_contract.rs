//! Contract for `gear push-check <remote> <url>` (T-74): the pre-push hook's
//! command.
//!
//! It is the command a git pre-push hook runs, so it is exercised as a child
//! process: a push to a destination the project registered goes through, a
//! push to any other is refused with the refusal on stderr, one urgent item
//! for the captain and exit 1 — and a push outside a GEAR seat is never
//! blocked.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gear_model::SeatRole;
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

/// The destination the project registered, and the URLs that name it and
/// another one.
const PUSH_DESTINATION: &str = "github.com/owner/repo";
const PUSH_ALLOWED: &str = "https://github.com/owner/repo.git";
const PUSH_REFUSED: &str = "https://github.com/other/repo.git";

/// A scratch directory, unique per tag and process.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gear-push-check-{}-{tag}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A git repository: the hook finds the repo through git.
fn git_repo(dir: &Path) {
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git init");
    assert!(init.success(), "git init in {}", dir.display());
}

/// A store with a project, its seats and the one frozen destination.
fn store_with_project(db: &Path, repo: &Path) -> Store {
    let mut store = Store::open(db).expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            let mut project = tx.get_project("p")?.expect("the project");
            project.config_toml = format!(
                "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {:?}\n\
                 \n[quota]\nlanes = 4\ntemps = 2\n\
                 \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n",
                repo.to_string_lossy()
            );
            tx.update_project(&project)?;
            tx.set_push_destinations("p", "", &[PUSH_DESTINATION.to_string()], T0)?;
            Ok(())
        })
        .expect("the world");
    store
}

/// `gear push-check` as the hook runs it: the seat's environment, the store,
/// and no Herdr around (the hook must not depend on one to refuse a push).
fn push_check(home: &Path, db: &Path, cwd: &Path, url: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["push-check", "origin", url])
        .env("HOME", home)
        .env("GEAR_DB", db)
        .env("GEAR_CTX", "p:heat:1")
        .env_remove("HERDR_PANE_ID")
        .env("HERDR_SOCKET_PATH", home.join("no-herdr.sock"))
        .current_dir(cwd)
        .output()
        .expect("run gear")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr.clone()).into_owned()
}

#[test]
fn a_push_to_a_registered_destination_is_allowed() {
    let dir = scratch("allowed");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    git_repo(&repo);
    let db = dir.join("gear.db");
    drop(store_with_project(&db, &repo));

    let out = push_check(&dir, &db, &repo, PUSH_ALLOWED);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "", "an allowed push says nothing");

    // Nothing was said to the captain.
    let mut store = Store::open(&db).expect("the store");
    let mail = store
        .read(|tx| tx.actionable_for("p", "p:captain", T0 + 10))
        .expect("the captain's inbox");
    assert!(mail.is_empty(), "{mail:?}");
}

#[test]
fn a_push_to_an_unregistered_destination_is_refused() {
    let dir = scratch("refused");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("the repo");
    git_repo(&repo);
    let db = dir.join("gear.db");
    drop(store_with_project(&db, &repo));

    let out = push_check(&dir, &db, &repo, PUSH_REFUSED);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("ERROR FORBIDDEN"), "{err}");
    assert!(err.contains("github.com/other/repo"), "{err}");
    assert!(err.contains("git push origin"), "{err}");

    // One urgent item for the captain.
    let mut store = Store::open(&db).expect("the store");
    let mail = store
        .read(|tx| tx.actionable_for("p", "p:captain", T0 + 10))
        .expect("the captain's inbox");
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert!(mail[0].urgent, "{mail:?}");
    assert_eq!(mail[0].recipient, "p:captain");
    assert!(mail[0].body.contains("github.com/other/repo"), "{:?}", mail[0]);
}

#[test]
fn a_push_outside_a_gear_seat_is_never_blocked() {
    let dir = scratch("outside");
    let db = dir.join("gear.db");
    let out = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["push-check", "origin", PUSH_REFUSED])
        .env("HOME", &dir)
        .env("GEAR_DB", &db)
        .env_remove("GEAR_CTX")
        .env_remove("HERDR_PANE_ID")
        .env("HERDR_SOCKET_PATH", dir.join("no-herdr.sock"))
        .current_dir(&dir)
        .output()
        .expect("run gear");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    // The hook did not even open the store: a user's own push is none of
    // GEAR's business.
    assert!(!db.exists(), "{}", db.display());
}
