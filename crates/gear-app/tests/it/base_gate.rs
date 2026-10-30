//! Contract for the hand-in gate (T-19): a result is refused unless the
//! candidate contains the project's current `origin/<integration>`.
//!
//! The git side is exercised against scratch repositories with a bare origin;
//! the store side proves that a refused hand-in stores nothing.

use gear_app::base::{require_current_base, BaseGate};
use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::tasks::{on_delivery_confirmed, settle, start_attempt, submit_result, Verdict};
use gear_app::Env;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};
use std::path::{Path, PathBuf};
use std::process::Command;

const T0: i64 = 1_800_000_000;
const LANE_PANE: &str = "w1:p2";

// ---------------------------------------------------------------- scratch git

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "gate")
        .env("GIT_AUTHOR_EMAIL", "gate@example.com")
        .env("GIT_COMMITTER_NAME", "gate")
        .env("GIT_COMMITTER_EMAIL", "gate@example.com")
        .output()
        .unwrap_or_else(|e| panic!("cannot run git: {e}"));
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, name: &str, text: &str) -> String {
    std::fs::write(dir.join(name), text).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", text]);
    git(dir, &["rev-parse", "HEAD"])
}

/// The wall clock in seconds since the epoch.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `git` with a commit time: the gate reads commit times (T-221), so a
/// fixture can place a commit in the past.
fn git_at(dir: &Path, args: &[&str], when: i64) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "gate")
        .env("GIT_AUTHOR_EMAIL", "gate@example.com")
        .env("GIT_COMMITTER_NAME", "gate")
        .env("GIT_COMMITTER_EMAIL", "gate@example.com")
        .env("GIT_AUTHOR_DATE", format!("@{when}"))
        .env("GIT_COMMITTER_DATE", format!("@{when}"))
        .output()
        .unwrap_or_else(|e| panic!("cannot run git: {e}"));
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A commit whose committer time is `minutes_ago` minutes ago.
fn commit_minutes_ago(dir: &Path, name: &str, text: &str, minutes_ago: i64) -> String {
    let when = now_secs() - minutes_ago * 60;
    std::fs::write(dir.join(name), text).unwrap();
    git_at(dir, &["add", name], when);
    git_at(dir, &["commit", "-q", "-m", text], when);
    git(dir, &["rev-parse", "HEAD"])
}

/// A scratch directory, removed when it goes.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        // Unique across threads of this process and across runs.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir()
            .join(format!("gear-base-{tag}-{}-{nanos}-{seq}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Scratch { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A bare origin with two commits on `dev`, and a clone for the lane.
struct Repos {
    _root: Scratch,
    /// The bare repository every clone calls `origin`.
    origin: PathBuf,
    /// The repo that pushes to the bare origin (the "captain's" side).
    seed: PathBuf,
    lane: PathBuf,
    /// The commit before the current `origin/dev` tip.
    older: String,
}

/// Publish `branch` of `source` in the bare origin's refs, the way the remote
/// would learn it: the origin fetches the objects *and* writes the ref in one
/// process — nothing here pushes (T-108). `rev-parse` plus `update-ref` were two
/// more git processes for the same ref (T-128).
fn publish(origin: &Path, source: &Path, from: &str, branch: &str) {
    let refspec = format!("+{from}:refs/heads/{branch}");
    git(origin, &["fetch", "-q", source.to_str().unwrap(), &refspec]);
}

/// Delete `branch` in the bare origin's refs (a ref update, not a push).
fn unpublish(origin: &Path, branch: &str) {
    git(origin, &["update-ref", "-d", &format!("refs/heads/{branch}")]);
}

/// The repositories every test starts from, built **once per test file**
/// (T-128): one `git init`, two commits and a clone per test cost more than the
/// gate under test. A test copies these trees — a file-system walk, no process
/// — and points its copies at its own origin.
struct Template {
    /// The bare origin, on `dev` with two commits.
    origin: PathBuf,
    /// The repo the origin fetches `dev` from (the "captain's" side).
    seed: PathBuf,
    /// The lane's clone.
    lane: PathBuf,
    /// The commit before the current `origin/dev` tip.
    older: String,
    /// The trees above live here for as long as the tests use them: the
    /// directory goes with the last test holding it (T-412).
    _root: Scratch,
}

fn template() -> std::sync::Arc<Template> {
    use std::sync::{Arc, Mutex, OnceLock, Weak};
    static TEMPLATE: OnceLock<Mutex<Weak<Template>>> = OnceLock::new();
    let mut slot = TEMPLATE.get_or_init(|| Mutex::new(Weak::new())).lock().unwrap();
    if let Some(template) = slot.upgrade() {
        return template;
    }
    let template = Arc::new({
        let root = Scratch::new("gate-template");
        let origin = root.path.join("origin.git");
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "-q", "--bare", "--initial-branch=dev"]);
        let url = origin.to_str().unwrap().to_string();
        let seed = root.path.join("seed");
        git(&root.path, &["clone", "-q", &url, seed.to_str().unwrap()]);
        commit(&seed, "a.txt", "one");
        commit(&seed, "b.txt", "two");
        publish(&origin, &seed, "refs/heads/dev", "dev");
        let lane = root.path.join("lane");
        git(&root.path, &["clone", "-q", &url, lane.to_str().unwrap()]);
        let older = git(&lane, &["rev-parse", "origin/dev~1"]);
        Template { origin, seed, lane, older, _root: root }
    });
    *slot = Arc::downgrade(&template);
    template
}

fn repos() -> Repos {
    let template = template();
    let root = Scratch::new("gate");
    let origin = root.path.join("origin.git");
    crate::copy_tree(&template.origin, &origin);
    let seed = root.path.join("seed");
    crate::copy_tree(&template.seed, &seed);
    let lane = root.path.join("lane");
    crate::copy_tree(&template.lane, &lane);
    // The seed publishes to *this* test's origin, so `advance_dev` moves only it.
    crate::point_at(&seed, &origin);
    crate::point_at(&lane, &origin);
    Repos { _root: root, origin, seed, lane, older: template.older.clone() }
}

impl Repos {
    /// A candidate on `branch`, published in the origin: a hand-in must be on
    /// `origin/<lane>/t<n>` (T-83), so every fixture hands in a published one.
    /// The origin fetches it from the lane — the fixtures never push (T-108).
    fn candidate(&self, branch: &str, from: &str, text: &str) -> String {
        git(&self.lane, &["switch", "-q", "-c", branch, from]);
        let commit = commit(&self.lane, &format!("{}.txt", branch.replace('/', "-")), text);
        publish(&self.origin, &self.lane, &format!("refs/heads/{branch}"), branch);
        commit
    }

    /// A commit on top of `from`, published as `branch` — a rework that builds
    /// on the earlier work.
    fn candidate_building_on(&self, branch: &str, from: &str, text: &str) -> String {
        let work = format!("work-{branch}");
        git(&self.lane, &["switch", "-q", "-c", &work, from]);
        let commit = commit(&self.lane, &format!("{}.txt", work.replace('/', "-")), text);
        publish(&self.origin, &self.lane, &format!("refs/heads/{work}"), branch);
        commit
    }

    /// Advance `dev` on the remote (a commit the origin fetches from the seed).
    fn advance_dev(&self, text: &str) {
        git(&self.seed, &["switch", "-q", "dev"]);
        commit(&self.seed, &format!("{}.txt", text.replace(' ', "-")), text);
        publish(&self.origin, &self.seed, "refs/heads/dev", "dev");
    }
}

/// Whether `a` is an ancestor of `b`: `git merge-base --is-ancestor`, whose
/// exit code says yes (0) or no (1).
fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(["merge-base", "--is-ancestor", a, b])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// A `dev` history whose commits have the given ages in minutes: the commits
/// are made in the seed, published as `dev` and fetched by the lane. The ages
/// are what the gate reads (the committer time, T-221).
fn dev_with_ages(r: &Repos, ages: &[i64]) -> Vec<String> {
    git(&r.seed, &["switch", "-q", "--orphan", "fresh"]);
    let mut shas = Vec::new();
    for (index, age) in ages.iter().enumerate() {
        shas.push(commit_minutes_ago(
            &r.seed,
            &format!("d{index}.txt"),
            &format!("dev {index}"),
            *age,
        ));
    }
    publish(&r.origin, &r.seed, "refs/heads/fresh", "dev");
    git(
        &r.lane,
        &["fetch", "-q", "origin", "+refs/heads/dev:refs/remotes/origin/dev"],
    );
    shas
}

// ---------------------------------------------------------------- the gate

#[test]
fn a_candidate_that_contains_the_integration_branch_is_accepted() {
    let r = repos();
    let candidate = r.candidate("heat/t1", "origin/dev", "my work");
    require_current_base(&r.lane, "origin", "dev", &candidate, 30).unwrap();
}

/// T-221: a base that moved during the lane's own suite is not a stale base.
/// A candidate that is behind the tip, but based on a commit of `dev` that is
/// only ten minutes old and merges cleanly into the tip, is accepted.
#[test]
fn a_candidate_behind_a_recent_base_that_merges_clean_is_accepted() {
    let r = repos();
    // dev: a base an hour old, a commit ten minutes old, and the tip now.
    let shas = dev_with_ages(&r, &[60, 10, 0]);
    // The lane worked on the ten-minute-old commit; the tip landed meanwhile.
    let candidate = r.candidate("heat/t1", &shas[1], "my work");

    // The candidate does not contain the tip: it is accepted because of the
    // window, not because it is current.
    assert!(
        !is_ancestor(&r.lane, "origin/dev", &candidate),
        "the tip is not in the candidate"
    );
    require_current_base(&r.lane, "origin", "dev", &candidate, 30).unwrap();
}

/// T-221: a base that is old is a stale base: a candidate that is only based
/// on a forty-minute-old commit of `dev` is refused, as before.
#[test]
fn a_candidate_behind_the_integration_branch_is_refused() {
    let r = repos();
    // dev: a base an hour old, a commit forty minutes old, and the tip now.
    let shas = dev_with_ages(&r, &[60, 40, 0]);
    // The lane worked on the forty-minute-old commit.
    let stale = r.candidate("data/stale", &shas[1], "old work");

    let err = require_current_base(&r.lane, "origin", "dev", &stale, 30).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("merge origin/dev into your branch, build, then hand in again"),
        "{}",
        err.detail
    );
}

/// T-221: a recent base is not enough — the tip must merge into the candidate
/// without a conflict. A candidate that changed the same file as the tip is
/// refused.
#[test]
fn a_recent_base_with_a_conflicting_change_is_refused() {
    let r = repos();
    // dev: a base an hour old, a commit ten minutes old, and the tip now.
    let shas = dev_with_ages(&r, &[60, 10, 0]);
    // The tip changed the file the ten-minute-old commit made...
    std::fs::write(r.seed.join("d1.txt"), "tip").unwrap();
    git_at(&r.seed, &["add", "d1.txt"], now_secs());
    git_at(&r.seed, &["commit", "-q", "-m", "tip"], now_secs());
    publish(&r.origin, &r.seed, "refs/heads/fresh", "dev");
    git(
        &r.lane,
        &["fetch", "-q", "origin", "+refs/heads/dev:refs/remotes/origin/dev"],
    );
    // ...and the lane changed it differently.
    git(&r.lane, &["switch", "-q", "-c", "heat/t1", &shas[1]]);
    std::fs::write(r.lane.join("d1.txt"), "mine").unwrap();
    git(&r.lane, &["add", "d1.txt"]);
    git(&r.lane, &["commit", "-q", "-m", "mine"]);
    let candidate = git(&r.lane, &["rev-parse", "HEAD"]);
    // The candidate does carry the ten-minute-old commit: the refusal is the
    // conflict, not an old base.
    assert!(is_ancestor(&r.lane, &shas[1], &candidate));

    let err = require_current_base(&r.lane, "origin", "dev", &candidate, 30).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("merge origin/dev into your branch, build, then hand in again"),
        "{}",
        err.detail
    );
}

/// T-221: the window is a setting, not a rule — with no window (0) a
/// candidate that is behind the tip is refused, as it was.
#[test]
fn a_candidate_behind_the_integration_branch_is_refused_without_a_window() {
    let r = repos();
    // The candidate is based on the commit before the current origin/dev tip.
    let candidate = r.candidate("data/stale", &r.older, "old work");

    let err = require_current_base(&r.lane, "origin", "dev", &candidate, 0).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(
        err.detail.contains("merge origin/dev into your branch, build, then hand in again"),
        "{}",
        err.detail
    );
}

#[test]
fn a_failing_fetch_is_refused_not_ignored() {
    let r = repos();
    let candidate = r.candidate("heat/t1", "origin/dev", "my work");
    git(&r.lane, &["remote", "set-url", "origin", "/nonexistent/nowhere.git"]);
    let err = require_current_base(&r.lane, "origin", "dev", &candidate, 30).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/dev"), "{}", err.detail);
    // The reason, not only git's boilerplate last line (review F4).
    assert!(
        err.detail.contains("nowhere.git") || err.detail.contains("does not appear to be a git repository"),
        "{}",
        err.detail
    );
}

#[test]
fn a_candidate_git_cannot_place_is_refused() {
    let r = repos();
    let err = require_current_base(&r.lane, "origin", "dev", "no-such-commit", 30).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot check"), "{}", err.detail);
}

/// T-257: the remote is the project's, not always `origin`. The gate fetches
/// the integration branch from the remote it is given and reads that remote's
/// tracking ref; a remote that is not there is a refusal, never a pass.
#[test]
fn the_integration_branch_is_fetched_through_the_projects_remote() {
    let r = repos();
    let candidate = r.candidate("heat/t1", "origin/dev", "my work");
    // The lane knows its remote as `upstream`, as a project naming
    // `remote = "upstream"` would have it.
    git(&r.lane, &["remote", "rename", "origin", "upstream"]);

    require_current_base(&r.lane, "upstream", "dev", &candidate, 30).unwrap();
    assert!(
        r.lane.join(".git/refs/remotes/upstream/dev").exists(),
        "the fetch wrote the given remote's tracking ref"
    );

    // A remote that is not there names no remote branch: the hand-in is
    // refused, never accepted unchecked.
    let err = require_current_base(&r.lane, "origin", "dev", &candidate, 30).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/dev"), "{}", err.detail);
}

// ---------------------------------------------------------------- the hand-in

/// Project `p` with a started attempt on T-1, ready to be handed in.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        tx.insert_task(&TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: "T-1".into(),
            rank: 1,
            state: TaskState::Ready,
            priority: "must".into(),
            lane: Some("heat".into()),
            milestone: None,
            required: false,
            contract_version: 1,
            deps: vec![],
            followup_of: None,
            wait_on: None,
            wait_cond: None,
            rework_note: None,
            estimate_min: None,
             issue: None,updated: T0,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "g".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
            mechanical: None,
        })?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, T0)
    })
    .unwrap();
    s
}

fn lane() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }),
            pane_id: Some(LANE_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None },
    )
}

fn result(candidate: &str, gate: Option<BaseGate>) -> SendArgs {
    SendArgs {
        kind: MsgKind::Result,
        body: "Report: ops/reports/T-1.md".into(),
        task: Some("T-1".into()),
        attempt: Some(1),
        contract: Some(1),
        candidate: Some(candidate.into()),
        verdict: None,
        blocking: None,
        base: gate,
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        request_id: "r-1".into(),
    }
}

#[test]
fn a_stale_hand_in_stores_nothing() {
    let r = repos();
    let stale = r.candidate("data/stale", &r.older, "old work");
    let mut s = world();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 0 });

    let err = mail::send(&mut s, &env, Some(&fp), &result(&stale, gate), T0 + 5).unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat);
    // Nothing was written: no message, the attempt is still being worked on.
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate), (AttemptState::Started, None));
}

/// T-83: a hand-in from a branch nobody pushed is refused — M2.1's T-60 was
/// reviewed from an unpushed branch and could not be merged, and the captain
/// fixed it by hand.
#[test]
fn an_unpushed_candidate_is_refused() {
    let r = repos();
    // The candidate exists in the lane's clone only.
    git(&r.lane, &["switch", "-q", "-c", "heat/t1", "origin/dev"]);
    let unpushed = commit(&r.lane, "work.txt", "my work");
    let mut s = world();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });

    let err = mail::send(&mut s, &env, Some(&fp), &result(&unpushed, gate), T0 + 5).unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/heat/t1"), "{}", err.detail);
    assert!(err.detail.contains("push the task branch first"), "{}", err.detail);
    assert!(err.detail.contains("heat/t1"), "{}", err.detail);
    // A candidate that is not on a branch that *does* exist is the other
    // refusal: the branch is there, this commit is not on it.
    publish(&r.origin, &r.lane, "refs/remotes/origin/dev", "heat/t1");
    let other = commit(&r.lane, "other.txt", "somewhere else");
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });
    let err = mail::send(&mut s, &env, Some(&fp), &result(&other, gate), T0 + 5).unwrap_err();
    assert!(err.detail.contains("is not on refs/remotes/origin/heat/t1"), "{}", err.detail);
    // Nothing was written: no message, the attempt is still being worked on.
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate), (AttemptState::Started, None));
}

/// §10.7 row B03-08: a rework hand-in whose candidate does not contain the
/// candidate of the attempt before it is refused, naming that commit.
#[test]
fn a_rework_off_the_earlier_candidate_is_refused() {
    let r = repos();
    let first = r.candidate("heat/t1", "origin/dev", "my work");
    // The rework: cut from dev again, so it does not carry the first candidate.
    let second = r.candidate("heat/t1b", "origin/dev", "other work");
    let mut s = world();
    // Attempt 1 hands the first candidate in and is failed: the task is ready
    // for a rework.
    s.write(|tx| {
        submit_result(tx, "p", "T-1", 1, 1, &first, "report", T0 + 2)?;
        settle(
            tx,
            &Verdict {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                contract_version: 1,
                candidate: first.clone(),
                pass: false,
                evidence: None,
                rework: Some("edge cases".into()),
                integrated_by: None,
                by_seat: "p:captain".into(),
            },
            T0 + 3,
        )?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 4)?;
        on_delivery_confirmed(tx, "p", "T-1", 2, T0 + 4)
    })
    .unwrap();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });
    let mut args = result(&second, gate);
    args.attempt = Some(2);

    let err = mail::send(&mut s, &env, Some(&fp), &args, T0 + 5).unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains(&first), "the refusal names that commit: {}", err.detail);
    // Nothing is stored: the attempt is still being worked on.
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate), (AttemptState::Started, None));
}

/// The other half of B03-08: a rework that keeps the work the reviewer read
/// (its candidate carries the earlier one) is handed in as usual.
#[test]
fn a_rework_that_keeps_the_earlier_candidate_is_accepted() {
    let r = repos();
    let first = r.candidate("heat/t1", "origin/dev", "my work");
    // The rework builds on the first candidate, so it carries it.
    let second = r.candidate_building_on("heat/t1", &first, "the fix");
    let mut s = world();
    s.write(|tx| {
        submit_result(tx, "p", "T-1", 1, 1, &first, "report", T0 + 2)?;
        settle(
            tx,
            &Verdict {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                contract_version: 1,
                candidate: first.clone(),
                pass: false,
                evidence: None,
                rework: Some("edge cases".into()),
                integrated_by: None,
                by_seat: "p:captain".into(),
            },
            T0 + 3,
        )?;
        start_attempt(tx, "p", "T-1", "p:heat", 1, T0 + 4)?;
        on_delivery_confirmed(tx, "p", "T-1", 2, T0 + 4)
    })
    .unwrap();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });
    let mut args = result(&second, gate);
    args.attempt = Some(2);

    let sent = mail::send(&mut s, &env, Some(&fp), &args, T0 + 5).unwrap();

    assert_eq!(sent.recipient, "p:captain");
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap().unwrap();
    assert_eq!(
        (attempt.state, attempt.candidate.as_deref()),
        (AttemptState::Submitted, Some(second.as_str()))
    );
}

#[test]
fn a_current_candidate_is_accepted() {
    let r = repos();
    let current = r.candidate("heat/t1", "origin/dev", "my work");
    let mut s = world();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });

    let sent = mail::send(&mut s, &env, Some(&fp), &result(&current, gate), T0 + 5).unwrap();

    assert_eq!(sent.recipient, "p:captain");
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate.as_deref()), (AttemptState::Submitted, Some(current.as_str())));
    assert_eq!(s.read(|tx| tx.thread_messages("p", "T-1:1")).unwrap().len(), 1);
}

// ---------------------------------------------------------------- R1 fixes

/// F1: a lane clone whose `remote.origin.fetch` is restricted to another branch
/// fetches `origin/dev` explicitly, so a stale candidate cannot pass.
#[test]
fn a_restricted_fetch_refspec_cannot_hide_a_stale_candidate() {
    let r = repos();
    // The candidate is based on the dev the lane has cached.
    let cached = git(&r.lane, &["rev-parse", "origin/dev"]);
    let stale = r.candidate("data/stale", &cached, "old work");
    // The remote advances on dev and carries another branch the lane fetches.
    r.advance_dev("new work");
    publish(&r.origin, &r.seed, "refs/heads/dev", "other");
    git(
        &r.lane,
        &["config", "remote.origin.fetch", "+refs/heads/other:refs/remotes/origin/other"],
    );

    // The strict rule (no window): the candidate does not contain the tip the
    // remote has, so it is refused (T-221: with a window a base that moved
    // only just is accepted).
    let err = require_current_base(&r.lane, "origin", "dev", &stale, 0).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("merge origin/dev into your branch"), "{}", err.detail);
}

/// F1: a deleted remote integration branch is refused, not verified against a
/// cached ref.
#[test]
fn a_deleted_remote_integration_branch_is_refused() {
    let r = repos();
    let candidate = r.candidate("heat/t1", "origin/dev", "my work");
    unpublish(&r.origin, "dev");

    let err = require_current_base(&r.lane, "origin", "dev", &candidate, 30).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/dev"), "{}", err.detail);
}

/// F2: a committed request replays its receipt even when the remote is now
/// unreachable; the attempt and the message stay exactly as they were.
#[test]
fn a_committed_hand_in_replays_its_receipt_after_a_fetch_failure() {
    let r = repos();
    let current = r.candidate("heat/t1", "origin/dev", "my work");
    let mut s = world();
    let (env, fp) = lane();
    let gate = || Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });

    let first = mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 5).unwrap();
    assert!(!first.replayed);

    // The remote goes away; the identical request (same id) must still replay.
    git(&r.lane, &["remote", "set-url", "origin", "/nonexistent/nowhere.git"]);
    let again = mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 6).unwrap();
    assert!(again.replayed, "{again:?}");
    assert_eq!(again.msg_id, first.msg_id);
    assert_eq!(again.thread, first.thread);

    // Nothing was stored twice, and the attempt is still the one submitted.
    assert_eq!(s.read(|tx| tx.thread_messages("p", "T-1:1")).unwrap().len(), 1);
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Submitted);
}

// ---------------------------------------------------------------- round 2

/// F5: a failed fetch whose diagnostic is long and multibyte is truncated on a
/// character boundary — an ordinary BAD_FORMAT with the reason, never a panic.
#[test]
fn a_long_multibyte_fetch_failure_is_a_bad_format_not_a_panic() {
    let r = repos();
    let candidate = r.candidate("heat/t1", "origin/dev", "my work");
    // A valid pathname shape (every component below NAME_MAX) that does not
    // exist, long enough that the fatal line passes the excerpt limit and the
    // limit falls inside a multibyte character.
    let remote = format!(
        "/missing/{}/{}/{}.git",
        "a".repeat(151),
        "b".repeat(150),
        "界".repeat(60)
    );
    git(&r.lane, &["remote", "set-url", "origin", &remote]);

    let err = require_current_base(&r.lane, "origin", "dev", &candidate, 30).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/dev"), "{}", err.detail);
    assert!(
        err.detail.contains("does not appear to be a git repository")
            || err.detail.contains("Could not read from remote repository"),
        "the reason survives: {}",
        err.detail
    );
}

/// F6: a replay needs the live seat. A caller without a fingerprint, from a
/// stale generation or with a void binding cannot replay a committed request —
/// and gets no second message.
#[test]
fn a_replay_needs_the_live_seat() {
    let r = repos();
    let current = r.candidate("heat/t1", "origin/dev", "my work");

    // (a) no fingerprint: the pane hosts no agent, so there is no seat.
    let mut s = world();
    let (env, fp) = lane();
    let gate = || Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });
    let first = mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 5).unwrap();
    let err = mail::send(&mut s, &env, None, &result(&current, gate()), T0 + 6).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered, "{err:?}");

    // (b) a stale generation: the seat moved on, the old context may not replay.
    let mut s = world();
    mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 5).unwrap();
    s.write(|tx| tx.set_seat_generation("p:heat", 2)).unwrap();
    let err = mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 6).unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleGeneration, "{err:?}");

    // (c) a void binding: the pane no longer belongs to this generation.
    let mut s = world();
    mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 5).unwrap();
    s.write(|tx| tx.set_binding_status("p:heat", 1, gear_store::BindingStatus::Void)).unwrap();
    let err = mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 6).unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleGeneration, "{err:?}");

    // Nothing was stored twice anywhere; the committed message is still one.
    assert_eq!(s.read(|tx| tx.thread_messages("p", "T-1:1")).unwrap().len(), 1);
    assert_eq!(first.msg_id, "M-1");
}

// ---------------------------------------------------------------- the repos

/// A project whose config holds a repository below its root, and one lane in
/// it (`heat`) and one in the root (`pricing`).
const WITH_REPOS: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"
repo = "api"

[[lanes]]
name = "pricing"
duty = "price engine"
owns = ["src/pricing/"]
kind = "opencode"
profile = "opencode"

[[repos]]
name = "api"
path = "crates/api"
integration = "feat/api"
release = "main"
gate = "cargo test -p api"

[conventions]
integration = "dev"
release = "main"
"#;

/// A store with the project above, and the seat of the lane `heat`.
fn world_with_repos() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        tx.insert_project(&gear_store::ProjectRow {
            id: "p".into(),
            herdr_session: "sess".into(),
            config_toml: WITH_REPOS.to_string(),
            config_version: 1,
            quota_lanes: 2,
            quota_temps: 0,
            quota_includes_captain: false,
            plan_version: 0,
            created: T0,
        })?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)
    })
    .unwrap();
    s
}

/// The environment of a seat: the lane `heat` or the lane `pricing`.
fn seat_env(seat: &str) -> Env {
    Env {
        ctx: Some(GearCtx { project: "p".into(), seat: seat.into(), generation: 1 }),
        pane_id: Some(LANE_PANE.into()),
        admin: false,
    }
}

/// T-54: a hand-in is checked against the integration branch of the repo the
/// lane works in: the child repo's own branch, not the root's.
#[test]
fn a_hand_in_uses_the_integration_branch_of_the_lanes_own_repo() {
    let mut s = world_with_repos();

    let gate = gear_app::base::gate_for(&mut s, &seat_env("heat")).unwrap();
    assert_eq!(gate.integration, "feat/api", "the child repo's own branch");

    // A lane in the root is checked against the root's branch.
    let gate = gear_app::base::gate_for(&mut s, &seat_env("pricing")).unwrap();
    assert_eq!(gate.integration, "dev", "the root's branch");
}

/// F2 of the review: a task branch deleted on the remote after the lane
/// fetched it is refused — a stale remote-tracking ref never accepts a
/// candidate the merge worker could not fetch.
#[test]
fn a_branch_deleted_on_the_remote_is_refused() {
    let r = repos();
    let candidate = r.candidate("heat/t1", "origin/dev", "my work");
    git(&r.lane, &["fetch", "-q", "origin"]);
    unpublish(&r.origin, "heat/t1");
    let mut s = world();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 30 });

    let err = mail::send(&mut s, &env, Some(&fp), &result(&candidate, gate), T0 + 5).unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/heat/t1"), "{}", err.detail);
    assert!(err.detail.contains("push the task branch first"), "{}", err.detail);
    assert!(err.detail.contains("heat/t1"), "{}", err.detail);
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate), (AttemptState::Started, None));
}

// ---------------------------------------------------------------- a review

const REVIEW_PANE: &str = "w1:p3";

/// `world()` plus the temp reviewer of T-1: only a temp seat hands in a
/// verdict.
fn review_world() -> Store {
    let mut s = world();
    s.write(|tx| testing::seed_temp_seat(tx, "p", "r-1", "T-1", REVIEW_PANE, "opencode", T0).map(|_| ()))
        .unwrap();
    s
}

fn reviewer() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "r-1".into(), generation: 1 }),
            pane_id: Some(REVIEW_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: REVIEW_PANE.into(), kind: Some("opencode".into()), session_ref: None },
    )
}

/// A verdict with the gate the CLI builds for a review (T-438): its presence
/// is what marks a reviewer handing in from its worktree.
fn review(gate: Option<BaseGate>) -> SendArgs {
    SendArgs {
        kind: MsgKind::Review,
        body: "P1: the row drifts right".into(),
        task: Some("T-1".into()),
        attempt: Some(1),
        contract: None,
        candidate: None,
        verdict: Some(gear_app::review::Outcome::Pass),
        blocking: None,
        base: gate,
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        request_id: "rev-1".into(),
    }
}

/// Run `f` with the process working directory at `dir`: the review check reads
/// the reviewer's worktree, and its HEAD, from the caller's directory (T-438).
/// The lock keeps two such tests from fighting over one process-wide setting.
fn in_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let out = f();
    std::env::set_current_dir(before).unwrap();
    out
}

/// T-438: a verdict whose review branch is on a *second* remote but not on the
/// project's is refused — T-265 and T-375 checked the remote they pushed to,
/// handed in, and the merge could not find `<origin>/<review branch>`, so the
/// captain pushed and remerged by hand.
#[test]
fn a_review_pushed_to_another_remote_is_refused() {
    let r = repos();
    // The reviewer's worktree: the review branch, committed.
    git(&r.lane, &["switch", "-q", "-c", "review/t1", "origin/dev"]);
    commit(&r.lane, "review-T-1.md", "review(T-1): pass");
    // It pushed to the second remote its worktree has, and checked that one.
    let backup = r._root.path.join("backup.git");
    git(&r._root.path, &["init", "-q", "--bare", "--initial-branch=dev", backup.to_str().unwrap()]);
    publish(&backup, &r.lane, "refs/heads/review/t1", "review/t1");
    git(&r.lane, &["remote", "add", "backup", backup.to_str().unwrap()]);
    git(&r.lane, &["fetch", "-q", "backup"]);

    let mut s = review_world();
    let (env, fp) = reviewer();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 0 });

    let err = in_dir(&r.lane, || {
        mail::send(&mut s, &env, Some(&fp), &review(gate), T0 + 5)
    })
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/review/t1"), "{}", err.detail);
    // The refusal names the exact push of the review branch.
    assert!(err.detail.contains("`git push origin review/t1`"), "{}", err.detail);
    // Nothing was stored: no verdict, nothing in the captain's inbox.
    assert!(s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().is_none());
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
}

/// T-438: the same verdict on the project's own remote is taken.
#[test]
fn a_review_pushed_to_the_projects_remote_is_taken() {
    let r = repos();
    git(&r.lane, &["switch", "-q", "-c", "review/t1", "origin/dev"]);
    commit(&r.lane, "review-T-1.md", "review(T-1): pass");
    publish(&r.origin, &r.lane, "refs/heads/review/t1", "review/t1");

    let mut s = review_world();
    let (env, fp) = reviewer();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 0 });

    in_dir(&r.lane, || {
        mail::send(&mut s, &env, Some(&fp), &review(gate), T0 + 5)
    })
    .unwrap();

    let record = s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().unwrap();
    assert!(record.passed);
    assert_eq!(record.by_seat, "p:r-1");
}

/// T-438: the check is the reviewer's own commit, not its branch's old tip: a
/// branch that is on the remote but behind the reviewer's HEAD is refused.
#[test]
fn a_review_branch_behind_the_reviewers_head_is_refused() {
    let r = repos();
    git(&r.lane, &["switch", "-q", "-c", "review/t1", "origin/dev"]);
    commit(&r.lane, "review-T-1.md", "the review");
    publish(&r.origin, &r.lane, "refs/heads/review/t1", "review/t1");
    // A second commit the reviewer made after pushing, then not pushed.
    let head = commit(&r.lane, "fixes.md", "and a fix");

    let mut s = review_world();
    let (env, fp) = reviewer();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into(), window_min: 0 });

    let err = in_dir(&r.lane, || {
        mail::send(&mut s, &env, Some(&fp), &review(gate), T0 + 5)
    })
    .unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains(&format!("{head} is not on refs/remotes/origin/review/t1")), "{}", err.detail);
    assert!(s.read(|tx| tx.get_review_verdict("p", "T-1", 1)).unwrap().is_none());
}
