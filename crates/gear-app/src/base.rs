//! The hand-in gate (T-19): a result may only be handed in from a branch that
//! contains the project's current integration branch.
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
    let worktree = std::env::current_dir().map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot read the working directory for the hand-in check: {e}"),
        )
    })?;
    Ok(BaseGate { worktree, integration })
}

/// The task branch of a lane: `<lane>/t<n>` (conventions 17 §3).
pub fn task_branch(lane: &str, task: &str) -> String {
    format!("{lane}/{}", task.to_lowercase().replace('-', ""))
}

/// Refuse unless `candidate` is on `origin/<branch>` (T-83): M2.1's T-60 was
/// reviewed from an unpushed branch and could not be merged, and the captain
/// fixed it by hand. The branch is fetched first, so the check is about the
/// remote as it is now.
pub fn require_pushed(worktree: &Path, branch: &str, candidate: &str) -> GearResult<()> {
    let where_ = worktree.display();
    let reference = format!("refs/remotes/origin/{branch}");
    // Fetch the branch itself: a branch pushed since the lane's last fetch
    // must be seen. A fetch that fails is a refusal, never a pass: the local
    // remote-tracking ref may be stale (the branch deleted on the remote), and
    // an unseen branch is one the merge worker could not fetch either.
    let fetched = Command::new("git")
        .args(["fetch", "origin", &format!("{branch}:refs/remotes/origin/{branch}")])
        .current_dir(worktree)
        .output()
        .map_err(|error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot fetch origin/{branch} in {where_}, so the hand-in cannot be checked: {error}"),
            )
        })?;
    if !fetched.status.success() {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot fetch origin/{branch} in {where_}, so the hand-in cannot be checked: push the task branch first (`git push origin {branch}`): {}",
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
            "{candidate} is not on {reference}: push the task branch before handing in (`git push origin {branch}`)"
        ),
    ))
}

/// Refuse unless `candidate` contains the current `origin/<integration>`:
/// `git fetch origin`, then `git merge-base --is-ancestor origin/<integration>
/// <candidate>` in `worktree`. Nothing is written here; the caller runs this
/// before its transaction, so a stale hand-in stores nothing at all.
pub fn require_current_base(worktree: &Path, integration: &str, candidate: &str) -> GearResult<()> {
    let where_ = worktree.display();
    // 1. the fetch: the configured source explicitly, into its own tracking ref.
    //    A plain `git fetch origin` follows `remote.origin.fetch`, which a lane's
    //    clone may restrict to another branch — the fetch would succeed and leave
    //    `origin/<integration>` stale (review F1). A remote branch that does not
    //    exist makes this fetch fail, and the hand-in is refused.
    let reference = format!("refs/remotes/origin/{integration}");
    let refspec = format!("+refs/heads/{integration}:{reference}");
    let fetched = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["fetch", "--quiet", "origin", &refspec])
        .output();
    match fetched {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "cannot fetch origin/{integration} in {where_}, so the hand-in cannot be checked: {}",
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
    let short = format!("origin/{integration}");
    let checked = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["merge-base", "--is-ancestor", &reference, candidate])
        .output();
    match checked {
        // 0: the reference is an ancestor of the candidate — current.
        Ok(out) if out.status.success() => Ok(()),
        // 1: it is not — the hand-in is stale. The instruction names the remote
        //    ref: a lane's local branch may lag behind the fetch (review F3).
        Ok(out) if out.status.code() == Some(1) => Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{candidate} does not contain the current {short}: merge {short} into your branch, build, then hand in again"
            ),
        )),
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
