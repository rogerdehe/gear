//! Onboarding a seat (design/03, /10): what the engine does to make a fresh
//! agent session the occupant of a seat. This module only *plans* the steps
//! (pure); executing them through Herdr is a later task.

use gear_herdr::Profile;
use gear_store::SeatRow;

/// Where a lane's long-lived worktree comes from (17 §3–§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStep {
    pub repo: String,
    /// `home/<lane>`
    pub branch: String,
    /// the project's integration branch
    pub base: String,
    /// `<project>-<lane>`
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardPlan {
    /// Lanes only; the captain and temps get their pane elsewhere.
    pub worktree: Option<WorktreeStep>,
    /// One shell line: `export GEAR_CTX=<project>:<seat>:<gen>; export GEAR_DB=<db>`.
    pub env_command: String,
    /// `<project>-<seat name>` (17 §2), at most 32 chars.
    pub agent_name: String,
    pub kind: String,
    /// The profile's `unattended_args`, then `resume_args` with `{session}`
    /// replaced when `resume` is given.
    pub args: Vec<String>,
}

/// Plan onboarding of `seat` as generation `generation` (the next one):
/// - Lane seats get a `WorktreeStep` (branch `home/<name>`, base
///   `integration`, label `<project>-<name>`); other roles `None`;
/// - `env_command` exactly `export GEAR_CTX=<project>:<name>:<generation>; export GEAR_DB=<db_path>`;
/// - `args` = `profile.unattended_args` (+ resume args when `resume` is Some and
///   the profile has them; a resume request for a profile without resume
///   support → Err with a reason);
/// - an unverified profile, or an agent name longer than 32 chars → Err.
pub fn plan_onboard(
    seat: &SeatRow,
    kind: &str,
    profile: &Profile,
    generation: i64,
    repo: &str,
    integration: &str,
    db_path: &str,
    resume: Option<&str>,
) -> Result<OnboardPlan, String> {
    let _ = (seat, kind, profile, generation, repo, integration, db_path, resume);
    todo!("T-58")
}
