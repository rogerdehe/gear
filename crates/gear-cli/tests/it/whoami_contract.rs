//! `gear whoami` (design §10.7, row B9): the pane the call runs in, the seat
//! bound to it, the store, and why a claim fails.
//!
//! It only reads: no engine, no write.

use std::process::Command;

use crate::scratch;

/// A fresh directory for a store that must stay untouched.
fn scratch_dir(tag: &str) -> std::path::PathBuf {
    scratch::home(tag)
}

#[test]
fn whoami_starts_no_engine_and_prints_the_store() {
    let dir = scratch_dir("whoami");
    let db = dir.join("gear.db");

    // An empty PATH: no agent CLI is found. What this test is about — the
    // engine and the store — does not depend on it.
    let empty = dir.join("bin");
    std::fs::create_dir_all(&empty).expect("an empty bin");

    let out = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["whoami"])
        .env("PATH", &empty)
        // The store of this call is the scratch one: never the production store
        // (T-41).
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
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("pane:"), "the whoami printed: {text}");
    assert!(text.contains("seat:"), "the whoami printed: {text}");
    assert!(
        text.contains(&db.display().to_string()),
        "the whoami names its store: {text}"
    );
    assert!(
        text.contains("reason:"),
        "a caller with no seat is told why: {text}"
    );

    // No engine was started: no engine.lock, no engine.log. (The store itself
    // is opened — the seat bound to the pane is read from it — so the database
    // file is there.)
    let mut entries: Vec<String> = std::fs::read_dir(&dir)
        .expect("the directory")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert!(
        entries == vec!["bin".to_string(), "gear.db".to_string()],
        "whoami created {entries:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
