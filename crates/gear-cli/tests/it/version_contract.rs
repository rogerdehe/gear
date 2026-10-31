//! Contract for `gear --version` (T-42): a new user asks which version they
//! have. The answer is the version of the workspace, and the question writes
//! nothing.

use std::path::PathBuf;
use std::process::Command;

/// A scratch home: the version writes nothing, and this is where "nothing" is
/// looked for.
fn scratch_home(tag: &str) -> PathBuf {
    crate::scratch::temp(&format!("gear-version-{tag}"))
}

/// A `gear` call that cannot reach the real store (T-41).
fn gear(home: &std::path::Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(args)
        .env("HOME", home)
        .env_remove("GEAR_CTX")
        .env_remove("GEAR_DB")
        .env_remove("HERDR_SOCKET_PATH");
    cmd
}

#[test]
fn version_prints_the_workspace_version() {
    let home = scratch_home("version");
    let out = gear(&home, &["--version"]).output().expect("run gear");

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.trim(), format!("gear {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn version_writes_nothing() {
    let home = scratch_home("version-nowrite");
    let out = gear(&home, &["--version"]).output().expect("run gear");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // No store, no directory: the question touched nothing in the home.
    let mut entries: Vec<String> = std::fs::read_dir(&home)
        .expect("the scratch home")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert!(entries.is_empty(), "the version wrote {entries:?}");
}
