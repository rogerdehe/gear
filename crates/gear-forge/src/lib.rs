//! The forge layer (T-78): where a project's issues and pull requests live.
//!
//! A project keeps its plan and its work in GEAR. A project that works on
//! GitHub also has issues that become tasks and pull requests that carry a
//! task's claim, its hand-in and its verdict. This crate is that edge and
//! nothing else — the one place that talks to a forge, so the engine never
//! calls an external tool itself.
//!
//! Two implementations of [`Forge`]:
//!
//! - [`GitHub`] drives the `gh` CLI: an explicit repo, JSON output, no
//!   prompts, every call bounded by a timeout.
//! - [`Local`] is the forge of a project that has none: every call succeeds
//!   and does nothing, so a caller never has to branch on the mode.
//!
//! The marker lines follow BMAD-Team's ORCHESTRATE protocol (see [`marker`]):
//! plain HTML comments in a pull request's body and comments, which a human
//! and another agent can both read, and which no markdown renderer shows.
//!
//! An error is always typed ([`Error`]) and always the forge's fault or the
//! answer's: a call that fails is logged by the caller and retried there; a
//! forge outage never stops local work.

pub mod github;
pub mod local;
pub mod marker;
pub mod sync;

pub use github::GitHub;
pub use local::Local;
pub use sync::{sync, sync_with};

use serde::Deserialize;

/// One issue of a repo.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Issue {
    /// The issue's number in its repo (`#12` → 12).
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// The issue's labels, by name.
    #[serde(default, deserialize_with = "label_names")]
    pub labels: Vec<String>,
}

/// Which issues to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    /// Only issues that carry every one of these labels; empty = all open
    /// issues.
    pub labels: Vec<String>,
    /// At most this many; 0 = the forge's own default.
    pub limit: u32,
}

/// A pull request, and the repo it lives in: a comment or a merge names the
/// number and the repo, never the branch it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    /// `owner/name`, as the forge names a repo.
    pub repo: String,
    pub number: u64,
    pub url: String,
}

impl PrRef {
    /// The pull request a forge did not open: [`Local`]'s answer, and what a
    /// caller sees when there is no forge at all.
    pub fn none(repo: &str) -> PrRef {
        PrRef {
            repo: repo.to_string(),
            number: 0,
            url: String::new(),
        }
    }

    /// Was this pull request actually opened?
    pub fn is_open(&self) -> bool {
        self.number > 0
    }
}

/// How a pull request is merged into its base branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    /// One commit on the base branch (the default for a claim).
    Squash,
    /// The pull request's own commits, kept.
    Merge,
}

impl MergeMethod {
    /// The `gh pr merge` flag for this method.
    pub fn flag(self) -> &'static str {
        match self {
            MergeMethod::Squash => "--squash",
            MergeMethod::Merge => "--merge",
        }
    }
}

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeResult {
    /// `true` when the forge merged it; a forge that answers "already merged"
    /// is not a merge.
    pub merged: bool,
    /// What the forge said, for the op's record.
    pub message: String,
}

/// The state of a pull request, read from the forge (design §11.3).
///
/// The pull request is the truth about a merge whose outcome is unknown: it
/// says whether it is merged, with which commit, and which head it points
/// at.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct PrState {
    /// The state the forge names: `OPEN`, `MERGED`, `CLOSED`…
    #[serde(default)]
    pub state: String,
    /// The commit the merge created, when the pull request is merged.
    #[serde(rename = "mergeCommit", default, deserialize_with = "merge_commit")]
    pub merge_commit: Option<String>,
    /// The sha the pull request's head points at.
    #[serde(rename = "headRefOid", default)]
    pub head_ref_oid: Option<String>,
}

impl PrState {
    /// Is the pull request merged?
    pub fn is_merged(&self) -> bool {
        self.state.eq_ignore_ascii_case("MERGED")
    }

    /// Is the pull request still open?
    pub fn is_open(&self) -> bool {
        self.state.eq_ignore_ascii_case("OPEN")
    }
}

/// What a forge call can fail with. Every variant names where it came from:
/// the tool, the answer, or the time it took.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The forge tool could not be started at all (not installed, not
    /// executable).
    #[error("the forge tool could not run: {0}")]
    Tool(String),
    /// The call ran past its bound and was killed.
    #[error("the forge call ran past its {0:?} bound")]
    Timeout(std::time::Duration),
    /// The tool ran and failed: it says why.
    #[error("the forge refused: {0}")]
    Failed(String),
    /// The tool answered something this crate cannot read.
    #[error("the forge answered something unreadable: {0}")]
    Unreadable(String),
}

pub type ForgeResult<T> = Result<T, Error>;

/// A project's forge: its issues and the pull requests that carry its work.
///
/// Every method is a bounded call that either answers or returns a typed
/// [`Error`]; none of them loops or prompts.
pub trait Forge {
    /// The open issues of `repo`, newest first, filtered by label.
    fn issues(&self, repo: &str, filter: &Filter) -> ForgeResult<Vec<Issue>>;

    /// Open the draft pull request that claims an issue for `branch`: the
    /// claim opener of a task (see [`marker::claim`]).
    fn open_claim(
        &self,
        repo: &str,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> ForgeResult<PrRef>;

    /// Comment on a pull request.
    fn comment(&self, pr: &PrRef, body: &str) -> ForgeResult<()>;

    /// Merge a pull request, with the head the gate checked: the forge merges
    /// exactly that commit (`--match-head-commit`), never another head that
    /// landed in between (design §11.2).
    fn merge(&self, pr: &PrRef, method: MergeMethod, head: &str) -> ForgeResult<MergeResult>;

    /// The state of a pull request (design §11.3): what a merge whose outcome
    /// is unknown asks the forge.
    fn pr_state(&self, pr: &PrRef) -> ForgeResult<PrState>;

    /// The open pull request of a branch, if there is one (design §11.3): a
    /// claim that was interrupted is adopted by its branch.
    fn pr_by_head(&self, repo: &str, branch: &str) -> ForgeResult<Option<PrRef>>;

    /// Close an issue, with one closing comment.
    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> ForgeResult<()>;
}

/// The commit `gh` names: an object `{"oid": "…"}`. An answer that is a
/// plain sha is read too.
fn merge_commit<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Commit {
        Oid { oid: String },
        Sha(String),
    }
    let commit = Option::<Commit>::deserialize(deserializer)?;
    Ok(match commit {
        Some(Commit::Oid { oid }) => Some(oid),
        Some(Commit::Sha(sha)) => Some(sha),
        None => None,
    })
}

/// `gh --json labels` answers an array of objects (`[{"name": "bug", …}]`);
/// a plain array of names is accepted too, so a recorded answer of either
/// shape reads the same.
fn label_names<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Label {
        Name(String),
        Named { name: String },
    }
    let labels = Vec::<Label>::deserialize(deserializer)?;
    Ok(labels
        .into_iter()
        .map(|label| match label {
            Label::Name(name) => name,
            Label::Named { name } => name,
        })
        .collect())
}
