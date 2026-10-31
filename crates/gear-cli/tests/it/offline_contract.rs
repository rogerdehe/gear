//! T-111: a test's git never leaves the machine.
//!
//! `.cargo/config.toml` allows the file transport only, so a clone or a fetch
//! over https or ssh is refused by git itself before a packet is sent — the
//! suite runs in a restricted network without the network.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn a_test_cannot_clone_over_the_network() {
    let dir = crate::scratch::temp("gear-offline-clone");
    let out = Command::new("git")
        .args(["clone", "https://example.com/owner/repo.git"])
        .arg(&dir)
        .output()
        .expect("git runs");
    assert!(!out.status.success(), "the https clone was not refused");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not allowed"), "{err}");
    assert!(
        !PathBuf::from(&dir).join(".git").exists(),
        "something was cloned"
    );
}
