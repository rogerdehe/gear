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
/// directory and the project's `[conventions].integration` (17 §11, default
/// `dev`). No `GEAR_CTX`, or a config that cannot be read → `BadFormat`: a
/// hand-in is never accepted unchecked.
pub fn gate_for(store: &mut Store, env: &Env) -> GearResult<BaseGate> {
    let ctx = env.ctx.as_ref().ok_or_else(|| {
        GearError::new(
            ErrorCode::BadFormat,
            "a hand-in needs GEAR_CTX: it names the project whose integration branch the candidate must contain",
        )
    })?;
    let config = crate::config::latest(store, &ctx.project)?;
    let worktree = std::env::current_dir().map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot read the working directory for the hand-in check: {e}"),
        )
    })?;
    Ok(BaseGate { worktree, integration: config.conventions.integration })
}

/// Refuse unless `candidate` contains the current `origin/<integration>`:
/// `git fetch origin`, then `git merge-base --is-ancestor origin/<integration>
/// <candidate>` in `worktree`. Nothing is written here; the caller runs this
/// before its transaction, so a stale hand-in stores nothing at all.
pub fn require_current_base(worktree: &Path, integration: &str, candidate: &str) -> GearResult<()> {
    let where_ = worktree.display();
    // 1. the fetch: without it the comparison would be against a stale ref.
    let fetched = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["fetch", "--quiet", "origin"])
        .output();
    match fetched {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "cannot fetch origin in {where_}, so the hand-in cannot be checked: {}",
                    tail(&out.stderr)
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

    // 2. the candidate must contain the integration branch as origin has it.
    let reference = format!("origin/{integration}");
    let checked = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["merge-base", "--is-ancestor", &reference, candidate])
        .output();
    match checked {
        // 0: the reference is an ancestor of the candidate — current.
        Ok(out) if out.status.success() => Ok(()),
        // 1: it is not — the hand-in is stale.
        Ok(out) if out.status.code() == Some(1) => Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{candidate} does not contain the current {reference}: merge {integration} into your branch, build, then hand in again"
            ),
        )),
        // Anything else (unknown revision, not a repository, …): unverifiable.
        Ok(out) => Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "cannot check {candidate} against {reference} in {where_}: {}",
                tail(&out.stderr)
            ),
        )),
        Err(e) => Err(GearError::new(
            ErrorCode::BadFormat,
            format!("cannot run git in {where_}, so the hand-in cannot be checked: {e}"),
        )),
    }
}

/// The last non-empty line of a command's stderr, for a short detail.
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| "no output".to_string())
}
