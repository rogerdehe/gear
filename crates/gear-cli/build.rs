//! The git sha this binary is built from (T-398).
//!
//! The engine prints it on its start line, so a log shows which build ran: a
//! rebuilt binary replaces the file on disk while the running engine keeps the
//! old one (2026-10-29: an hour on a replaced pre-fix binary, nothing in the
//! log said so).

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // The crate's own sources: a rebuild after a source change must read the
    // sha again, not keep the one of the build before it.
    println!("cargo:rerun-if-changed=src");
    // A new commit changes the sha without any source file changing. `.git/HEAD`
    // alone does not say so: it holds a branch name, and a commit moves that
    // branch's ref, not HEAD (rework F1). The paths are asked of git, because a
    // linked worktree keeps its HEAD and refs outside the source tree.
    for path in git_paths() {
        if Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // "unknown" when git is absent (a source tarball, a bare machine): the
    // start line then says it was built without a repository.
    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GEAR_BUILD_SHA={sha}");
}

/// The files that move when HEAD moves: git's own HEAD, the ref HEAD names and
/// the packed refs, as git resolves them (a linked worktree keeps them
/// elsewhere).
fn git_paths() -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(path) = git(&["rev-parse", "--git-path", "HEAD"]) {
        paths.push(path);
    }
    // A detached HEAD names no branch; then HEAD alone is the file that moves.
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &branch]) {
            paths.push(path);
        }
    }
    if let Some(path) = git(&["rev-parse", "--git-path", "packed-refs"]) {
        paths.push(path);
    }
    paths
}

/// The first line of a git command's output, when the command succeeds.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}
