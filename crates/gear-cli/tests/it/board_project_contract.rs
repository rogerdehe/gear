//! `gear board --project <id>` (T-249): the user's own board tab runs in a pane
//! bound to no seat, so the board has no caller and would show every project in
//! the store — and the first one on screen. The flag names the project, and a
//! typo is refused before the board starts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

/// A scratch directory and a store holding the projects `a` and `b`.
fn world(tag: &str) -> (PathBuf, PathBuf) {
    let dir = crate::scratch::temp(&format!("gear-board-{tag}"));
    let db = dir.join("gear.db");
    let mut store = Store::open(&db).expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, "a", "sess-a", T0)?;
            testing::seed_project(tx, "b", "sess-b", T0)
        })
        .expect("two projects");
    drop(store);
    (dir, db)
}

/// `gear board …` as a caller's process with no seat: no `GEAR_CTX`, no Herdr
/// pane, so `cmd_board` has no caller to take a project from.
fn board(dir: &Path, db: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(args)
        .env("HOME", dir)
        .env("GEAR_DB", db)
        .env_remove("GEAR_CTX")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH");
    cmd.output().expect("run gear board")
}

/// The project ids a `gear board --json` printed, in order.
fn projects_of(out: &Output) -> Vec<String> {
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let doc: serde_json::Value = serde_json::from_str(&text).expect("the board as JSON");
    doc["projects"]
        .as_array()
        .expect("the projects")
        .iter()
        .map(|p| p["project"].as_str().expect("a project id").to_string())
        .collect()
}

#[test]
fn the_flag_shows_only_the_project_it_names() {
    let (dir, db) = world("named");
    let projects = projects_of(&board(&dir, &db, &["board", "--project", "b", "--json"]));
    assert_eq!(projects, vec!["b"], "the board showed {projects:?}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_flag_wins_over_the_callers_own_project() {
    let (dir, db) = world("wins");
    // The caller's seat is project `a`; the flag asks for `b`.
    let out = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["board", "--project", "b", "--json"])
        .env("HOME", &dir)
        .env("GEAR_DB", &db)
        .env("GEAR_CTX", "a:captain:1")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear board");
    let projects = projects_of(&out);
    assert_eq!(projects, vec!["b"], "the board showed {projects:?}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unknown_project_is_refused_and_the_registered_ones_are_named() {
    let (dir, db) = world("unknown");
    let out = board(&dir, &db, &["board", "--project", "zz", "--json"]);
    assert!(!out.status.success(), "an unknown project must fail");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no project zz in this store"),
        "the error names the typo: {stderr}"
    );
    assert!(
        stderr.contains("registered: a, b"),
        "the error lists the registered projects: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn without_the_flag_a_caller_with_no_seat_sees_every_project() {
    let (dir, db) = world("every");
    let projects = projects_of(&board(&dir, &db, &["board", "--json"]));
    assert_eq!(projects, vec!["a", "b"], "the board showed {projects:?}");

    std::fs::remove_dir_all(&dir).ok();
}
