//! The hand-in gate (T-19): a result may only be handed in from a branch that
//! contains the project's current integration branch.
//!
//! T-221 relaxes that rule for a base that moved *during* the lane's own
//! work: M4's T-175 was refused twice because `dev` advanced while the lane
//! merged and ran its 12-minute suite, and merges land every ten minutes — a
//! lane could race `dev` forever. A candidate that does not contain the
//! current tip is accepted when it is based on a commit of the integration
//! branch that is at most `timing.handin_base_window_min` minutes old and the
//! tip merges into the candidate without a conflict. The merge gate still runs
//! the full suite on the merged result.
//!
//! Two reworks (T-13, T-15) came only from hand-ins that had not merged `dev`:
//! the candidate was fine on its own base and could not pass the merge gate.
//! A mechanical check beats a written rule, so [`require_current_base`] fetches
//! and compares before anything is written. It never accepts unchecked: a
//! fetch that fails (offline, no remote) or a candidate git cannot place is a
//! `BadFormat`, not a pass.
//!
//! Git runs **outside** any store transaction, in the lane's own worktree (the
//! caller's working directory), like the read-only Herdr calls of identity.

use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult};
use gear_store::Store;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a hand-in must prove: the worktree to check in and the integration
/// branch it must contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseGate {
    pub worktree: PathBuf,
    pub integration: String,
    /// How many minutes a hand-in may be behind the integration branch and
    /// still be accepted (T-221): `timing.handin_base_window_min`.
    pub window_min: u32,
}

/// The gate for a `gear send --kind result` in this process: the lane's working
/// directory and the integration branch of the repo the lane works in (T-54):
/// the repo the lane is bound to with `repo =`, else the project's root, with
/// that repo's own integration branch (`[conventions].integration` for the
/// root). No `GEAR_CTX`, or a config that cannot be read → `BadFormat`: a
/// hand-in is never accepted unchecked.
pub fn gate_for(store: &mut Store, env: &Env) -> GearResult<BaseGate> {
    let ctx = env.ctx.as_ref().ok_or_else(|| {
        GearError::new(
            ErrorCode::BadFormat,
            "a hand-in needs GEAR_CTX: it names the project whose integration branch the candidate must contain",
        )
    })?;
    let config = crate::config::latest(store, &ctx.project)?;
    // The lane's own repo: a lane in a child repo is checked against that
    // repo's integration branch, not the root's (T-54).
    let integration = config.repo_of_lane(&ctx.seat).integration;
    let window_min = config.timing.handin_base_window_min;
    let worktree = std::env::current_dir().map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot read the working directory for the hand-in check: {e}"),
        )
    })?;
    Ok(BaseGate {
        worktree,
        integration,
        window_min,
    })
}

/// The task branch of a lane (T-252): the project's
/// `[conventions].task_branch` template with `{project}`, `{lane}` and `{n}`
/// filled in — `<lane>/t<n>` by default (conventions 17 §3).
pub fn task_branch(conventions: &crate::config::Conventions, project: &str, lane: &str, task: &str) -> String {
    gear_model::branch::expand(&conventions.task_branch, project, lane, task)
}

/// Refuse unless `candidate` is on `<remote>/<branch>` (T-83): M2.1's T-60 was
/// reviewed from an unpushed branch and could not be merged, and the captain
/// fixed it by hand. The branch is fetched first, so the check is about the
/// remote as it is now. `remote` is the project's `[conventions].remote`
/// (T-257): `origin` unless the project names its own.
pub fn require_pushed(
    worktree: &Path,
    remote: &str,
    branch: &str,
    candidate: &str,
) -> GearResult<()> {
    let where_ = worktree.display();
    let reference = format!("refs/remotes/{remote}/{branch}");
    // Fetch the branch itself: a branch pushed since the lane's last fetch
    // must be seen. A fetch that fails is a refusal, never a pass: the local
    // remote-tracking ref may be stale (the branch deleted on the remote), and
    // an unseen branch is one the merge worker could not fetch either.
    let fetched = Command::new("git")
        .args(["fetch", remote, &format!("{branch}:refs/remotes/{remote}/{branch}")])
        .current_dir(worktree)
        .output()
        .map_err(|error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot fetch {remote}/{branch} in {where_}, so the hand-in cannot be checked: {error}"),
            )
        })?;
    if !fetched.status.success() {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot fetch {remote}/{branch} in {where_}, so the hand-in cannot be checked: push the task branch first (`git push {remote} {branch}`): {}",
                String::from_utf8_lossy(&fetched.stderr).trim()
            ),
        ));
    }
    let pushed = Command::new("git")
        .args(["merge-base", "--is-ancestor", candidate, &reference])
        .current_dir(worktree)
        .output()
        .map_err(|error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot check {candidate} against {reference} in {where_}: {error}"),
            )
        })?;
    if pushed.status.success() {
        return Ok(());
    }
    Err(GearError::new(
        ErrorCode::BadFormat,
        format!(
            "{candidate} is not on {reference}: push the task branch before handing in (`git push {remote} {branch}`)"
        ),
    ))
}

/// Whether `candidate` contains `base`: `git merge-base --is-ancestor base
/// candidate` in `worktree`.
///
/// Git that cannot place a commit is an error, never a "no": a check that
/// cannot answer must not pass silently (T-19's rule).
pub fn contains(worktree: &Path, base: &str, candidate: &str) -> GearResult<bool> {
    let where_ = worktree.display();
    let checked = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["merge-base", "--is-ancestor", base, candidate])
        .output()
        .map_err(|error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot check whether {candidate} contains {base} in {where_}: {error}"),
            )
        })?;
    match checked.status.code() {
        // 0: the base is an ancestor of the candidate — the candidate carries it.
        Some(0) => Ok(true),
        // 1: the base is not an ancestor of the candidate — it does not carry it.
        Some(1) => Ok(false),
        _ => Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot check whether {candidate} contains {base} in {where_}: {}",
                excerpt(&checked.stderr)
            ),
        )),
    }
}

/// Fetch the integration branch and name the ref that holds it (design
/// §10.7, row B03-01): the answer about a candidate must be about the remote
/// as it is now, so the branch is fetched first — a local tracking ref may be
/// stale.
pub fn fetch_integration(repo_path: &str, remote: &str, integration: &str) -> GearResult<String> {
    let reference = format!("refs/remotes/{remote}/{integration}");
    let fetched = Command::new("git")
        .args([
            "fetch",
            remote,
            &format!("+refs/heads/{integration}:{reference}"),
        ])
        .current_dir(repo_path)
        .output()
        .map_err(|error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "cannot fetch {remote}/{integration} in {repo_path} to tell whether the work is merged: {error}"
                ),
            )
        })?;
    if !fetched.status.success() {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot fetch {remote}/{integration} in {repo_path} to tell whether the work is merged: {}",
                excerpt(&fetched.stderr)
            ),
        ));
    }
    Ok(reference)
}

/// Refuse unless `candidate` contains the current `<remote>/<integration>`:
/// `git fetch <remote>`, then `git merge-base --is-ancestor
/// <remote>/<integration> <candidate>` in `worktree`. `remote` is the
/// project's `[conventions].remote` (T-257). Nothing is written here; the
/// caller runs this before its transaction, so a stale hand-in stores nothing
/// at all.
///
/// `window_min` is the window in which a base that moved during the lane's
/// own work is still accepted (T-221): a candidate that does not contain the
/// current tip is accepted when it is based on a commit of the integration
/// branch that is at most `window_min` minutes old (committer time) and the
/// tip merges into the candidate without a conflict. `0` is the strict rule:
/// only a candidate that contains the current tip is accepted.
pub fn require_current_base(
    worktree: &Path,
    remote: &str,
    integration: &str,
    candidate: &str,
    window_min: u32,
) -> GearResult<()> {
    let where_ = worktree.display();
    // 1. the fetch: the configured source explicitly, into its own tracking ref.
    //    A plain `git fetch <remote>` follows `remote.<remote>.fetch`, which a
    //    lane's clone may restrict to another branch — the fetch would succeed
    //    and leave `<remote>/<integration>` stale (review F1). A remote branch
    //    that does not exist makes this fetch fail, and the hand-in is refused.
    let reference = format!("refs/remotes/{remote}/{integration}");
    let refspec = format!("+refs/heads/{integration}:{reference}");
    let fetched = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["fetch", "--quiet", remote, &refspec])
        .output();
    match fetched {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "cannot fetch {remote}/{integration} in {where_}, so the hand-in cannot be checked: {}",
                    excerpt(&out.stderr)
                ),
            ))
        }
        Err(e) => {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("cannot run git in {where_}, so the hand-in cannot be checked: {e}"),
            ))
        }
    }

    // 2. the candidate must contain the refreshed integration branch.
    let short = format!("{remote}/{integration}");
    let checked = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["merge-base", "--is-ancestor", &reference, candidate])
        .output();
    match checked {
        // 0: the reference is an ancestor of the candidate — current.
        Ok(out) if out.status.success() => Ok(()),
        // 1: it is not — the hand-in is behind the tip. The instruction names
        //    the remote ref: a lane's local branch may lag behind the fetch
        //    (review F3).
        //
        //    The tip may have moved while the lane merged and ran its suite
        //    (T-221): a base that is only just behind is not a stale base.
        //    Such a candidate is accepted when it is based on a commit of the
        //    integration branch that is at most `window_min` minutes old and
        //    the tip merges into it without a conflict — the merge gate still
        //    runs the full suite on the merged result.
        Ok(out) if out.status.code() == Some(1) => {
            if window_min > 0
                && based_on_recent_base(worktree, &reference, candidate, window_min)?
                && merges_clean(worktree, &reference, candidate)?
            {
                return Ok(());
            }
            Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "{candidate} does not contain the current {short}: merge {short} into your branch, build, then hand in again"
                ),
            ))
        }
        // Anything else (unknown revision, not a repository, …): unverifiable.
        Ok(out) => Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot check {candidate} against {short} in {where_}: {}",
                excerpt(&out.stderr)
            ),
        )),
        Err(e) => Err(GearError::new(
            ErrorCode::BadFormat,
            format!("cannot run git in {where_}, so the hand-in cannot be checked: {e}"),
        )),
    }
}

/// Whether the candidate is based on a commit of the integration branch that
/// is at most `window_min` minutes old (committer time): the base moved
/// during the lane's own work, but only just (T-221).
///
/// The commits are the ones of the integration branch that git itself finds
/// in the window (`--since`), so the list is short: a branch that lands a
/// merge every ten minutes has a handful.
fn based_on_recent_base(
    worktree: &Path,
    reference: &str,
    candidate: &str,
    window_min: u32,
) -> GearResult<bool> {
    let where_ = worktree.display();
    let now = now_secs();
    let since = now - i64::from(window_min) * 60;
    let listed = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args([
            "log",
            "--format=%H %ct",
            &format!("--since=@{since}"),
            reference,
        ])
        .output()
        .map_err(|error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot read the commits of {reference} in {where_}: {error}"),
            )
        })?;
    if !listed.status.success() {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot read the commits of {reference} in {where_}: {}",
                excerpt(&listed.stderr)
            ),
        ));
    }
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(sha), Some(when)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(when) = when.parse::<i64>() else {
            continue;
        };
        // "At most `window_min` minutes older than now": a commit of the
        // future (a skewed clock) is not older than now, and is inside the
        // window too.
        if now - when > i64::from(window_min) * 60 {
            continue;
        }
        if contains(worktree, sha, candidate)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the tip of the integration branch merges into the candidate
/// without a conflict: `git merge-tree --write-tree` says so (git 2.38 and
/// later). It writes the merged tree's objects and touches neither the index
/// nor the worktree.
fn merges_clean(worktree: &Path, reference: &str, candidate: &str) -> GearResult<bool> {
    let where_ = worktree.display();
    let merged = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["merge-tree", "--write-tree", reference, candidate])
        .output()
        .map_err(|error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot run git in {where_}, so the hand-in cannot be checked: {error}"),
            )
        })?;
    match merged.status.code() {
        // 0: the merge is clean — the suite runs on the merged result.
        Some(0) => Ok(true),
        // 1: the merge conflicts — the tip does not fit into the candidate.
        Some(1) => Ok(false),
        // Anything else (an unknown revision, a git without `--write-tree`):
        // unverifiable, never a pass.
        _ => Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot check whether {candidate} merges with {reference} in {where_}: {}",
                excerpt(&merged.stderr)
            ),
        )),
    }
}

/// The wall clock in seconds since the epoch: the clock the commit times are
/// compared with.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A bounded excerpt of a command's stderr: the `fatal:`/`error:` lines (the
/// reason), else the last lines — never only the boilerplate last sentence
/// (review F4). At most four lines, at most ~400 characters.
fn excerpt(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let mut picked: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .collect();
    if picked.is_empty() {
        picked = lines.iter().rev().take(2).copied().collect::<Vec<_>>();
        picked.reverse();
    }
    picked.truncate(4);
    let joined = picked.join(" / ");
    if joined.is_empty() {
        "no output".to_string()
    } else if joined.chars().count() > 400 {
        // Head and tail, cut on characters and never inside one: a long
        // multibyte path must not panic the check, and the reason at the end of
        // git's fatal line must survive the cut (review round 2, F5).
        let chars: Vec<char> = joined.chars().collect();
        let head: String = chars[..300].iter().collect();
        let tail: String = chars[chars.len() - 100..].iter().collect();
        format!("{head}…{tail}")
    } else {
        joined
    }
}
