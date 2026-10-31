//! `gear admin stop --hold` (T-394): a stop the user asked for stays stopped.
//!
//! Every `gear` call runs `ensure_engine`, which starts an engine whenever none
//! holds the lock, so a stopped engine came back seconds later — the user's
//! "关都关不掉". A hold file in the engine dir keeps it down until
//! `gear admin start` lifts it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::scratch;

/// A `gear` child process on the scratch store `dir/gear.db`, as the user's
/// own shell: no seat, no admin flag and no Herdr around it.
fn gear(dir: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(args)
        // Not in the repository: `project_session_socket` must not find a real
        // Herdr session here (L-54).
        .current_dir(dir)
        .env("HOME", dir)
        .env("GEAR_DB", dir.join("gear.db"))
        .env_remove("GEAR_CTX")
        .env_remove("GEAR_ADMIN")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH");
    cmd.output().expect("run gear")
}

fn scratch_dir(tag: &str) -> PathBuf {
    scratch::home(tag)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn stop_with_hold_writes_the_hold_when_no_engine_runs() {
    let dir = scratch_dir("hold-stop");
    let out = gear(&dir, &["admin", "stop", "--hold"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("engine not running"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("engine held: no gear call starts it until `gear admin start`"),
        "{}",
        stdout(&out)
    );
    let hold = std::fs::read_to_string(dir.join("engine.hold")).expect("the hold file");
    assert!(hold.contains("held by gear admin stop --hold"), "{hold}");
    assert!(
        hold.trim().len() > "held by gear admin stop --hold".len(),
        "the hold holds the time too: {hold}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_plain_stop_writes_no_hold() {
    let dir = scratch_dir("hold-plain");
    let out = gear(&dir, &["admin", "stop"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("engine not running"),
        "{}",
        stdout(&out)
    );
    assert!(
        !dir.join("engine.hold").exists(),
        "a plain stop holds nothing"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_held_engine_stays_down_and_the_call_says_so() {
    let dir = scratch_dir("hold-down");
    let out = gear(&dir, &["admin", "stop", "--hold"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // `gear home` is a call that starts the engine when its lock is free.
    let out = gear(&dir, &[]);
    assert!(
        stderr(&out).contains(
            "gear: the engine is held (gear admin stop --hold); `gear admin start` resumes it"
        ),
        "{}",
        stderr(&out)
    );
    // The lock file is the one the stop took and released; a running engine
    // writes its pid there.
    let lock = std::fs::read_to_string(dir.join("engine.lock")).unwrap_or_default();
    assert!(
        lock.trim().is_empty(),
        "no engine wrote its pid, so none started: {lock}"
    );
    assert!(!dir.join("engine.log").exists(), "no engine was spawned");
    assert!(dir.join("engine.hold").exists(), "the hold stays");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn start_lifts_the_hold() {
    let dir = scratch_dir("hold-start");
    let out = gear(&dir, &["admin", "stop", "--hold"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        dir.join("engine.hold").exists(),
        "the hold is there before the start"
    );

    let out = gear(&dir, &["admin", "start"]);
    assert!(
        !dir.join("engine.hold").exists(),
        "start lifted the hold: {}",
        stderr(&out)
    );
    // With no hold it only starts (here: no engine, since this store is no
    // Herdr session's store — L-54), and it does not fail the user.
    let out = gear(&dir, &["admin", "start"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!dir.join("engine.hold").exists(), "a start writes no hold");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn agents_hold_and_start_nothing() {
    let dir = scratch_dir("hold-agents");
    for args in [
        ["admin", "stop", "--hold"].as_slice(),
        ["admin", "start"].as_slice(),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_gear"))
            .args(args)
            .current_dir(&dir)
            .env("HOME", &dir)
            .env("GEAR_DB", dir.join("gear.db"))
            .env("GEAR_CTX", "gear:mast:1")
            .env_remove("GEAR_ADMIN")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_SOCKET_PATH")
            .output()
            .expect("run gear");
        assert!(!out.status.success(), "an agent may not run {args:?}");
        assert!(stderr(&out).contains("agents do not"), "{}", stderr(&out));
    }
    assert!(!dir.join("engine.hold").exists(), "an agent wrote no hold");
    assert!(
        !dir.join("engine.lock").exists(),
        "an agent started no engine"
    );
    std::fs::remove_dir_all(&dir).ok();
}
