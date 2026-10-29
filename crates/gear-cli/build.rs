//! The git sha this binary is built from (T-398).
//!
//! The engine prints it on its start line, so a log shows which build ran: a
//! rebuilt binary replaces the file on disk while the running engine keeps the
//! old one (2026-10-29: an hour on a replaced pre-fix binary, nothing in the
//! log said so).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A new commit changes the sha without any source file changing.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    // "unknown" when git is absent (a source tarball, a bare machine): the
    // start line then says it was built without a repository.
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GEAR_BUILD_SHA={sha}");
}
