//! A `gear` child process that cannot reach the real store (T-41).
//!
//! A lane runs the tests with `GEAR_DB` set — the production store — and the
//! store `gear` falls back to lives under `$HOME`. Every `gear` a test starts
//! gets no `GEAR_DB` and a scratch `HOME`, so neither the store it was given
//! nor the one it would fall back to is the real one.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch HOME, unique per tag and process.
pub fn home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gear-test-{}-{tag}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch home");
    dir
}

/// A `gear` child process that cannot reach the real store: no `GEAR_DB`, a
/// scratch `HOME`, and no Herdr around it (an engine only runs on the store
/// that is registered for a Herdr session).
pub fn gear(home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(args)
        .env("HOME", home)
        .env_remove("GEAR_CTX")
        .env_remove("GEAR_DB")
        .env_remove("GEAR_ADMIN")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH");
    cmd
}

/// The store `gear` uses under `home` when `GEAR_DB` is not set.
pub fn default_store(home: &Path) -> PathBuf {
    home.join(".local/state/gear/gear.db")
}
