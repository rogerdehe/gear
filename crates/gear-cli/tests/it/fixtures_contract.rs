//! Contract for `scripts/fixtures/make.sh` (T-87): the two fixture projects of
//! the live acceptance. The script builds them from nothing and offline, and
//! can rebuild them over themselves; the test checks the layout the acceptance
//! runs on — the repositories, their own local origins, their integration
//! branches — and that every gate passes.
//!
//! The checks live in three tests (T-186): one per fixture, plus the default
//! run the acceptance uses. Each test builds its fixture once and reuses it
//! for every check, so nextest runs the three side by side and no test pays
//! for the other project's build. Every assertion of the former single test is
//! kept, spread over the parts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory, unique per tag and process.
fn scratch(tag: &str) -> PathBuf {
    crate::scratch::temp(&format!("gear-fixtures-{tag}"))
}

/// Run the fixture script for `dir`, as the acceptance runs it. `which` says
/// which project to build: `multi`, `running` or `both` — and nothing at all,
/// which is the default the acceptance runs.
fn make(dir: &Path, which: Option<&str>) -> Output {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("scripts/fixtures/make.sh");
    let mut cmd = Command::new("sh");
    cmd.arg(script).arg(dir);
    if let Some(which) = which {
        cmd.arg(which);
    }
    cmd.output().expect("run make.sh")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Run one fixture's gate; it must pass, quickly.
fn gate(dir: &Path) -> String {
    let started = std::time::Instant::now();
    let out = Command::new("sh")
        .arg("gate.sh")
        .current_dir(dir)
        .output()
        .expect("run the gate");
    assert!(
        out.status.success(),
        "the gate of {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the gate of {} took {:?}: a gate must run in seconds",
        dir.display(),
        started.elapsed()
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The remote of a repo, and the assertion that it is a local path: no
/// fixture may reach a network.
fn local_remote(repo: &Path) -> String {
    let url = git(repo, &["remote", "get-url", "origin"]);
    assert!(
        Path::new(&url).is_absolute(),
        "{} pushes to {url}, which is not a local path",
        repo.display()
    );
    assert!(
        !url.contains("://") && !url.contains('@'),
        "{} pushes to {url}, which is not a local path",
        repo.display()
    );
    url
}

/// The multi-repo fixture: a parent on `dev` with two children of their own,
/// every gate passing, and nothing of an earlier run left over.
#[test]
fn the_multi_fixture_builds_replaces_and_passes_their_gates() {
    let dir = scratch("multi");
    // What an earlier run left where the fixture will be built.
    std::fs::create_dir_all(dir.join("multi")).expect("a stale project");
    std::fs::write(dir.join("multi/STALE"), "gone").expect("a stale file");

    let first = make(&dir, Some("multi"));
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    // The parent: a repo on dev with a local origin.
    let multi = dir.join("multi");
    assert!(
        multi.join(".git").exists(),
        "the parent is not a repository"
    );
    assert_eq!(git(&multi, &["branch", "--show-current"]), "dev");
    local_remote(&multi);
    assert_eq!(
        git(&multi, &["status", "--porcelain"]),
        "",
        "the parent has uncommitted work"
    );
    // The run replaces the project: nothing of the earlier run is left.
    assert!(
        !multi.join("STALE").exists(),
        "the run did not replace multi/"
    );

    // Its two children: own repositories, on their own integration branch,
    // each with its own local origin, inside gitignored paths of the parent.
    for name in ["web", "api"] {
        let child = multi.join(name);
        assert!(child.join(".git").exists(), "{name} is not a repository");
        assert_eq!(git(&child, &["branch", "--show-current"]), "feat", "{name}");
        let remote = local_remote(&child);
        assert!(remote.contains(name), "{name}'s origin is {remote}");
        assert_eq!(
            git(&child, &["status", "--porcelain"]),
            "",
            "{name} has uncommitted work"
        );
    }
    // The parent ignores the children: each is its own repository.
    let status = git(&multi, &["status", "--porcelain", "--ignored"]);
    for name in ["web", "api"] {
        assert!(
            status.contains(&format!("!! {name}/")),
            "the parent does not ignore {name}/: {status}"
        );
    }

    // Every gate passes, and the children's are their own.
    gate(&multi);
    gate(&multi.join("web"));
    gate(&multi.join("api"));

    // The notes carry the config of the multi-repo fixture.
    let notes = std::fs::read_to_string(dir.join("FIXTURE.md")).expect("FIXTURE.md");
    assert!(notes.contains("[[repos]]"), "FIXTURE.md: {notes}");
    assert!(
        notes.contains("integration = \"feat\""),
        "FIXTURE.md: {notes}"
    );
}

/// The fixture of the project that already runs: its own ways, and the
/// exemplar next to the bad pattern — and nothing of an earlier run left over.
#[test]
fn the_running_fixture_builds_replaces_and_passes_its_gate() {
    let dir = scratch("running");
    // What an earlier run left where the fixture will be built.
    std::fs::create_dir_all(dir.join("running")).expect("a stale project");
    std::fs::write(dir.join("running/STALE"), "gone").expect("a stale file");

    let first = make(&dir, Some("running"));
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let running = dir.join("running");
    assert_eq!(git(&running, &["branch", "--show-current"]), "main");
    // T-98: the project that already runs has its own local origin, on `main`.
    let running_origin = local_remote(&running);
    assert!(
        running_origin.ends_with("origins/running.git"),
        "running/'s origin is {running_origin}"
    );
    assert!(
        git(
            &dir.join("origins/running.git"),
            &["for-each-ref", "--format=%(refname)"]
        )
        .contains("refs/heads/main"),
        "the running fixture's main was not pushed"
    );
    for path in [
        ".github/workflows/ci.yml",
        "CLAUDE.md",
        "AGENTS.md",
        ".claude/skills/release-check/SKILL.md",
        "docs/OLD-PLAN.md",
        "docs/architecture-2019.md",
        "docs/notes.md",
        "src/good.py",
        "src/bad.py",
        "Makefile",
        "gate.sh",
    ] {
        assert!(running.join(path).exists(), "running/ misses {path}");
    }
    let claude = std::fs::read_to_string(running.join("CLAUDE.md")).expect("CLAUDE.md");
    let agents = std::fs::read_to_string(running.join("AGENTS.md")).expect("AGENTS.md");
    assert!(claude.contains("straight to `main`"), "CLAUDE.md: {claude}");
    assert!(
        agents.contains("one change per branch"),
        "AGENTS.md: {agents}"
    );
    // The run replaces the project: nothing of the earlier run is left.
    assert!(
        !running.join("STALE").exists(),
        "the run did not replace running/"
    );
    gate(&running);
}

/// The default run builds both projects, as the acceptance runs it, and
/// replaces what an earlier run left there.
#[test]
fn the_default_run_builds_both_projects_and_replaces_what_was_there() {
    let dir = scratch("both");
    // What an earlier run left where the projects will be built.
    std::fs::create_dir_all(dir.join("multi")).expect("a stale project");
    std::fs::create_dir_all(dir.join("running")).expect("a stale project");
    std::fs::write(dir.join("multi/STALE"), "gone").expect("a stale file");
    std::fs::write(dir.join("running/STALE"), "gone").expect("a stale file");

    let first = make(&dir, None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let multi = dir.join("multi");
    let running = dir.join("running");
    assert!(
        multi.join(".git").exists(),
        "the parent is not a repository"
    );
    assert!(running.join(".git").exists(), "running is not a repository");
    assert_eq!(git(&multi, &["branch", "--show-current"]), "dev");
    assert_eq!(git(&running, &["branch", "--show-current"]), "main");
    assert!(
        !multi.join("STALE").exists(),
        "the run did not replace multi/"
    );
    assert!(
        !running.join("STALE").exists(),
        "the run did not replace running/"
    );
    gate(&multi);
    gate(&running);
}
