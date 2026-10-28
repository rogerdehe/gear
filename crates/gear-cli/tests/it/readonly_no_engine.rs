//! Read-only commands start no engine and open no store (the review of T-10,
//! P1).
//!
//! `gear admin kickoff` prints the machine's inventory. Before the fix it
//! went through `ensure_engine`, which created `engine.lock`, started
//! `gear run` and so opened (and migrated) a database — for a command that
//! must not write anything, from a point where the project may not exist.

use std::process::Command;

use crate::scratch;

/// A fresh directory for a store that must stay untouched.
fn scratch_dir(tag: &str) -> std::path::PathBuf {
    scratch::home(tag)
}

#[test]
fn kickoff_starts_no_engine_and_opens_no_store() {
    let dir = scratch_dir("kickoff");
    let db = dir.join("gear.db");

    // An empty PATH: no agent CLI is found, so the inventory is quick. What
    // this test is about — the engine and the store — does not depend on it.
    let empty = dir.join("bin");
    std::fs::create_dir_all(&empty).expect("an empty bin");

    let out = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["admin", "kickoff"])
        .env("PATH", &empty)
        // The store of this call is the scratch one: never the production
        // store (T-41).
        .env("GEAR_DB", &db)
        .env("HOME", &dir)
        .env_remove("GEAR_CTX")
        .env_remove("HERDR_PANE_ID")
        .env_remove("GEAR_ADMIN")
        .output()
        .expect("run gear");

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("GEAR kickoff"),
        "the kickoff printed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Nothing was started and nothing was opened: no engine.lock, no
    // engine.log, no database.
    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .expect("the directory")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert!(
        entries == vec!["bin".to_string()],
        "kickoff created {entries:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
