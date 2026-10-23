//! Contract for the hand-in gate (T-19): a result is refused unless the
//! candidate contains the project's current `origin/<integration>`.
//!
//! The git side is exercised against scratch repositories with a bare origin;
//! the store side proves that a refused hand-in stores nothing.

use gear_app::base::{require_current_base, BaseGate};
use gear_app::identity::Fingerprint;
use gear_app::mail::{self, SendArgs};
use gear_app::tasks::{on_delivery_confirmed, start_attempt};
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
    /// The repo that pushes to the bare origin (the "captain's" side).
    seed: PathBuf,
    lane: PathBuf,
    /// The commit before the current `origin/dev` tip.
    older: String,
}

fn repos() -> Repos {
    let root = Scratch::new("gate");
    let origin = root.path.join("origin.git");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "--initial-branch=dev"]);
    // Let a test delete dev on the remote (the bare clone's HEAD would forbid it).
    git(&origin, &["config", "receive.denyDeleteCurrent", "ignore"]);

    let seed = root.path.join("seed");
    std::fs::create_dir(&seed).unwrap();
    git(&seed, &["init", "-q", "--initial-branch=dev"]);
    let older = commit(&seed, "a.txt", "one");
    commit(&seed, "b.txt", "two");
    git(&seed, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&seed, &["push", "-q", "origin", "dev"]);

    let lane = root.path.join("lane");
    git(&root.path, &["clone", "-q", origin.to_str().unwrap(), lane.to_str().unwrap()]);
    Repos { _root: root, seed, lane, older }
}

impl Repos {
    /// A candidate commit on a new branch, based on `from`.
    fn candidate(&self, branch: &str, from: &str, text: &str) -> String {
        git(&self.lane, &["switch", "-q", "-c", branch, from]);
        commit(&self.lane, &format!("{}.txt", branch.replace('/', "-")), text)
    }

    /// Advance `dev` on the remote (a commit pushed from the seed clone).
    fn advance_dev(&self, text: &str) {
        git(&self.seed, &["switch", "-q", "dev"]);
        commit(&self.seed, &format!("{}.txt", text.replace(' ', "-")), text);
        git(&self.seed, &["push", "-q", "origin", "dev"]);
    }
}

// ---------------------------------------------------------------- the gate

#[test]
fn a_candidate_that_contains_the_integration_branch_is_accepted() {
    let r = repos();
    let candidate = r.candidate("data/t19", "origin/dev", "my work");
    require_current_base(&r.lane, "dev", &candidate).unwrap();
}

#[test]
fn a_candidate_behind_the_integration_branch_is_refused() {
    let r = repos();
    // Branched from the commit before the current origin/dev tip.
    let stale = r.candidate("data/stale", &r.older, "old work");
    let err = require_current_base(&r.lane, "dev", &stale).unwrap_err();
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
    let candidate = r.candidate("data/t19", "origin/dev", "my work");
    git(&r.lane, &["remote", "set-url", "origin", "/nonexistent/nowhere.git"]);
    let err = require_current_base(&r.lane, "dev", &candidate).unwrap_err();
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
    let err = require_current_base(&r.lane, "dev", "no-such-commit").unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot check"), "{}", err.detail);
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
            updated: T0,
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
        request_id: "r-1".into(),
    }
}

#[test]
fn a_stale_hand_in_stores_nothing() {
    let r = repos();
    let stale = r.candidate("data/stale", &r.older, "old work");
    let mut s = world();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into() });

    let err = mail::send(&mut s, &env, Some(&fp), &result(&stale, gate), T0 + 5).unwrap_err();

    assert_eq!(err.code, ErrorCode::BadFormat);
    // Nothing was written: no message, the attempt is still being worked on.
    assert!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().is_empty());
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate), (AttemptState::Started, None));
}

#[test]
fn a_current_candidate_is_accepted() {
    let r = repos();
    let current = r.candidate("data/t19", "origin/dev", "my work");
    let mut s = world();
    let (env, fp) = lane();
    let gate = Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into() });

    let sent = mail::send(&mut s, &env, Some(&fp), &result(&current, gate), T0 + 5).unwrap();

    assert_eq!(sent.recipient, "p:captain");
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((attempt.state, attempt.candidate.as_deref()), (AttemptState::Submitted, Some(current.as_str())));
    assert_eq!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 6)).unwrap().len(), 1);
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
    git(&r.seed, &["push", "-q", "origin", "dev:other"]);
    git(
        &r.lane,
        &["config", "remote.origin.fetch", "+refs/heads/other:refs/remotes/origin/other"],
    );

    let err = require_current_base(&r.lane, "dev", &stale).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("merge origin/dev into your branch"), "{}", err.detail);
}

/// F1: a deleted remote integration branch is refused, not verified against a
/// cached ref.
#[test]
fn a_deleted_remote_integration_branch_is_refused() {
    let r = repos();
    let candidate = r.candidate("data/t19", "origin/dev", "my work");
    git(&r.seed, &["push", "-q", "origin", "--delete", "dev"]);

    let err = require_current_base(&r.lane, "dev", &candidate).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("cannot fetch origin/dev"), "{}", err.detail);
}

/// F2: a committed request replays its receipt even when the remote is now
/// unreachable; the attempt and the message stay exactly as they were.
#[test]
fn a_committed_hand_in_replays_its_receipt_after_a_fetch_failure() {
    let r = repos();
    let current = r.candidate("data/t19", "origin/dev", "my work");
    let mut s = world();
    let (env, fp) = lane();
    let gate = || Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into() });

    let first = mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 5).unwrap();
    assert!(!first.replayed);

    // The remote goes away; the identical request (same id) must still replay.
    git(&r.lane, &["remote", "set-url", "origin", "/nonexistent/nowhere.git"]);
    let again = mail::send(&mut s, &env, Some(&fp), &result(&current, gate()), T0 + 6).unwrap();
    assert!(again.replayed, "{again:?}");
    assert_eq!(again.msg_id, first.msg_id);
    assert_eq!(again.thread, first.thread);

    // Nothing was stored twice, and the attempt is still the one submitted.
    assert_eq!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 7)).unwrap().len(), 1);
    let attempt = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(attempt.state, AttemptState::Submitted);
}

// ---------------------------------------------------------------- round 2

/// F5: a failed fetch whose diagnostic is long and multibyte is truncated on a
/// character boundary — an ordinary BAD_FORMAT with the reason, never a panic.
#[test]
fn a_long_multibyte_fetch_failure_is_a_bad_format_not_a_panic() {
    let r = repos();
    let candidate = r.candidate("data/t19", "origin/dev", "my work");
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

    let err = require_current_base(&r.lane, "dev", &candidate).unwrap_err();
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
    let current = r.candidate("data/t19", "origin/dev", "my work");

    // (a) no fingerprint: the pane hosts no agent, so there is no seat.
    let mut s = world();
    let (env, fp) = lane();
    let gate = || Some(BaseGate { worktree: r.lane.clone(), integration: "dev".into() });
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
    assert_eq!(s.read(|tx| tx.actionable_for("p", "p:captain", T0 + 7)).unwrap().len(), 1);
    assert_eq!(first.msg_id, "M-1");
}
