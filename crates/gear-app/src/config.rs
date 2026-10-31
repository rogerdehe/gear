//! Project config (design/01, conventions 17 §1–§2, §11). Pure: parse TOML
//! text, apply defaults, validate what can be checked without Herdr or git.
//! Checks that need the outside world (session exists, repo/branch exist,
//! profile verified) live in `gear admin init`, not here.
//!
//! The one exception is [`latest`]: it reads the config a project is
//! registered with, so the engine and every command read the same version.
//!
//! A project may be made of several git repositories (T-52): the root the
//! project is registered with, plus a `[[repos]]` entry per child repo below
//! it. Each lane works in one repo — [`ProjectConfig::repo_of_lane`] — and
//! that repo's own integration branch and gate carry its work. A config
//! without `[[repos]]` is a single-repo project and behaves exactly as
//! before.

use gear_model::{ErrorCode, GearError, GearResult, Ts};
use gear_store::Store;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub quota: Quota,
    pub captain: CaptainSection,
    #[serde(default)]
    pub lanes: Vec<LaneSection>,
    /// The project's other repos, below `project.repo` (T-52). Empty = a
    /// single-repo project: every lane works in the root.
    #[serde(default)]
    pub repos: Vec<RepoSection>,
    #[serde(default)]
    pub temps: Option<TempsSection>,
    #[serde(default)]
    pub conventions: Conventions,
    #[serde(default)]
    pub timing: Timing,
    /// The departure time of a voyage (T-448); absent = today's behaviour.
    #[serde(default)]
    pub schedule: Option<ScheduleSection>,
    /// API-equivalent USD per million tokens, keyed by provider/model.
    #[serde(default)]
    pub prices: BTreeMap<String, Price>,
    /// Model chains per role, agreed at kickoff (17 §0). Empty = not managed.
    #[serde(default)]
    pub models: Models,
    /// Per-kind profile overrides, kept as raw TOML; merged by the engine (04).
    #[serde(default)]
    pub profiles: BTreeMap<String, toml::Table>,
    /// The GitHub mode of the project root (T-79). Absent = local-only: issues
    /// are not read and no pull request is opened. A `[[repos]]` entry needs
    /// its own section: it never inherits this one.
    #[serde(default)]
    pub github: Option<GithubSection>,
}

impl ProjectConfig {
    /// The project's root repo (T-52): the checkout the project is
    /// registered with, with the project's own branches and gate.
    pub fn root_repo(&self) -> Repo {
        Repo {
            name: String::new(),
            path: self.project.repo.clone(),
            integration: self.conventions.integration.clone(),
            release: self.conventions.release.clone(),
            gate: self.conventions.gate.clone(),
            github: self.github.as_ref().and_then(GithubSection::resolved),
        }
    }

    /// The repo a lane works in (T-52): the `[[repos]]` entry its `repo`
    /// names, with that repo's own branches and gate (its own, or the
    /// project's `[conventions]`).
    ///
    /// A lane without a `repo`, a lane that is not in the config and a lane
    /// naming a repo that is not there all work in the project root.
    pub fn repo_of_lane(&self, lane: &str) -> Repo {
        let bound = self
            .lanes
            .iter()
            .find(|section| section.name == lane)
            .and_then(|section| section.repo.as_deref());
        match bound.and_then(|name| self.repos.iter().find(|repo| repo.name == name)) {
            Some(section) => self.repo_of_section(section),
            None => self.root_repo(),
        }
    }

    /// A task keeps the repository, base and gate fixed at creation, even if
    /// its lane later moves. Legacy tasks without a snapshot use the lane.
    pub fn repo_of_task(&self, task: &gear_store::TaskRow) -> Repo {
        let Some(path) = task.repo.as_deref() else {
            return self.repo_of_lane(task.lane.as_deref().unwrap_or_default());
        };
        let mut repo = self
            .repos()
            .into_iter()
            .find(|r| r.path == path)
            .unwrap_or_else(|| {
                // A removed repo is still a valid historical target. Do not use a
                // different repo's GitHub settings or worker key for it.
                let mut hash = 0xcbf29ce484222325u64;
                for b in path.bytes() {
                    hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
                }
                let mut fallback = self.root_repo();
                fallback.name = format!("snapshot-{hash:x}");
                fallback.path = path.to_string();
                fallback.github = None;
                fallback
            });
        if let Some(base) = &task.base {
            repo.integration = base.clone();
        }
        if let Some(gate) = &task.gate {
            repo.gate = gate.clone();
        }
        repo
    }

    /// Every repo of the project (T-52): the project root first, then the
    /// `[[repos]]` entries in config order. The root's name is `""`.
    pub fn repos(&self) -> Vec<Repo> {
        let mut repos = vec![self.root_repo()];
        for section in &self.repos {
            repos.push(self.repo_of_section(section));
        }
        repos
    }

    /// One `[[repos]]` entry, resolved: its absolute path and its effective
    /// branches and gate.
    fn repo_of_section(&self, section: &RepoSection) -> Repo {
        Repo {
            name: section.name.clone(),
            path: join_path(&self.project.repo, &section.path),
            integration: section
                .integration
                .clone()
                .unwrap_or_else(|| self.conventions.integration.clone()),
            release: section
                .release
                .clone()
                .unwrap_or_else(|| self.conventions.release.clone()),
            gate: section
                .gate
                .clone()
                .unwrap_or_else(|| self.conventions.gate.clone()),
            // A repo's own `[github]`, and only that (T-79): a child repo
            // without one is local-only. It never inherits the project's
            // section, which names another repository — a GitHub mode that
            // pointed a child at the root's repo would write in the wrong
            // place.
            github: section.github.as_ref().and_then(GithubSection::resolved),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    pub id: String,
    pub herdr_session: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quota {
    pub lanes: u32,
    pub temps: u32,
    #[serde(default)]
    pub includes_captain: bool,
    /// The quota sources of §9.1: the providers whose quota the engine
    /// reads. Empty = the quota comes from the screen alone.
    #[serde(default)]
    pub sources: Vec<QuotaSource>,
}

/// One quota source of §9.1: a provider whose quota the engine reads.
///
/// ```toml
/// [[quota.sources]]
/// provider = "minimax-cn"            # the provider part of the model ids
/// reader = "minimax"                 # minimax | opencode-go | codex
/// key_file = "~/.pi/agent/auth.json" # the file the key is in
/// key_path = "minimax-cn.key"        # the dotted path of the key in it
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaSource {
    /// The provider part of the model ids this source is about.
    pub provider: String,
    /// The reader that knows the provider's API: `minimax`, `opencode-go`
    /// or `codex`.
    pub reader: String,
    /// The file the API key is in (`~` is the home folder). The `codex`
    /// reader reads the rollout of a session and needs no key.
    #[serde(default)]
    pub key_file: String,
    /// The dotted path of the key inside that file.
    #[serde(default)]
    pub key_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptainSection {
    pub pane: String,
    pub kind: String,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSection {
    pub name: String,
    pub duty: String,
    pub owns: Vec<String>,
    #[serde(default)]
    pub not: Vec<String>,
    pub kind: String,
    pub profile: String,
    /// Overrides `models.lanes` for this lane; empty = use the role chain.
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    /// The `[[repos]]` entry this lane works in (T-52); absent = the project
    /// root.
    #[serde(default)]
    pub repo: Option<String>,
    /// This lane's charter (T-469): how its agent works on the model it runs,
    /// written at kickoff for that model. Absent = the lane rules.
    #[serde(default)]
    pub charter: Option<String>,
}

/// One repo of a multi-repo project (T-52): a child repository below
/// `project.repo`, with its own branches and gate.
///
/// `integration`, `release` and `gate` are optional: a repo without them
/// uses the project's `[conventions]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoSection {
    /// The repo's name: `[a-z][a-z0-9-]*`. A lane binds to it with `repo =`.
    pub name: String,
    /// The repo's path, relative to `project.repo`: never absolute, never
    /// leaving `project.repo` with `..`.
    pub path: String,
    /// The repo's integration branch; absent = `[conventions].integration`.
    #[serde(default)]
    pub integration: Option<String>,
    /// The repo's release branch; absent = `[conventions].release`.
    #[serde(default)]
    pub release: Option<String>,
    /// The gate that runs in this repo before a merge; absent =
    /// `[conventions].gate`.
    #[serde(default)]
    pub gate: Option<String>,
    /// This repo's own GitHub mode (T-79); absent = local-only. It does not
    /// inherit the project's section: that one names the root's repository.
    #[serde(default)]
    pub github: Option<GithubSection>,
}

/// The GitHub mode of a project root or a repo (T-79): `repo =
/// "owner/name"`, the labels that select issues, and how a merge reaches
/// GitHub. Absent = local-only; each level names its own repository.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GithubSection {
    /// The GitHub repository, `owner/name`. Required when the section is
    /// present: a GitHub mode without a repo names no place to read or write.
    pub repo: Option<String>,
    /// Which issues become tasks.
    pub issues: IssuesSection,
    /// How an accepted task reaches the integration branch: `push` (local, as
    /// today) or `pr` (through a pull request the forge merges).
    pub merge: MergeMode,
    /// The method a pull-request merge uses.
    pub merge_method: MergeMethod,
}

impl GithubSection {
    /// The resolved mode; `None` when the section names no repo (an invalid
    /// config, refused by `validate`; a reader fails closed).
    pub fn resolved(&self) -> Option<Github> {
        let repo = self.repo.clone()?;
        Some(Github {
            repo,
            issues_labels: self.issues.labels.clone(),
            merge: self.merge,
            merge_method: self.merge_method,
        })
    }
}

/// The labels that select issues (`[github.issues] labels = [...]`). Empty =
/// every open issue.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IssuesSection {
    pub labels: Vec<String>,
}

/// How an accepted task reaches the integration branch (T-79).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeMode {
    /// Local, as today: the engine merges and pushes (the default).
    #[default]
    Push,
    /// Through a pull request the forge opens and merges.
    Pr,
}

impl MergeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMode::Push => "push",
            MergeMode::Pr => "pr",
        }
    }
}

/// The method a pull-request merge uses (`squash` by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    #[default]
    Squash,
    Merge,
}

impl MergeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Squash => "squash",
            MergeMethod::Merge => "merge",
        }
    }
}

/// The GitHub mode of a repo, resolved (T-79): the repo's own `[github]` — or
/// the project's, for the project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Github {
    /// `owner/name`.
    pub repo: String,
    /// Only issues with these labels; empty = all open issues.
    pub issues_labels: Vec<String>,
    pub merge: MergeMode,
    pub merge_method: MergeMethod,
}

/// A repo of the project, resolved (T-52): the project root or one of the
/// `[[repos]]` entries, with the path, branches and gate the engine uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The repo's name; "" for the project root.
    pub name: String,
    /// The repo's path: the project's own path for the root, `project.repo`
    /// joined with the entry's path for a `[[repos]]` entry.
    pub path: String,
    /// The repo's integration branch.
    pub integration: String,
    /// The repo's release branch.
    pub release: String,
    /// The gate that runs in this repo before a merge.
    pub gate: String,
    /// The GitHub mode of this repo (T-79); `None` = local-only.
    pub github: Option<Github>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TempsSection {
    pub kind: String,
    pub profile: String,
    /// The reviewers' charter (T-469): how a reviewer works on its model.
    /// Absent = the reviewer rules.
    #[serde(default)]
    pub charter: Option<String>,
}

/// The default cap on a task's planned time (T-279): no task is planned for
/// longer than this, so none keeps a lane unattended for hours before its
/// hand-in is reviewed.
pub const MAX_TASK_MIN: u32 = 60;

/// The default age, in minutes, past which a task's branch on the remote is
/// named to the captain: a branch of a voyage is meant to be merged (or its
/// task cancelled) within this time; one that lives longer means its work is
/// stuck, and the captain decides what to do.
pub const BRANCH_MAX_AGE_MIN: u32 = 120;

/// The default number of task branches each lane may hold on the remote at
/// once: two hand-ins out and one sent back. More than lanes × this means
/// branches rot, and the captain is told to clean up.
pub const BRANCH_MAX_PER_LANE: u32 = 3;

/// The default no-push clock in minutes: a lane working this long without a
/// push to its task branch is asked why.
pub const PUSH_INTERVAL_MIN: u32 = 20;

/// The default minutes a lane has to answer that question to the captain;
/// no answer in time is reported to the captain as a lane in trouble.
pub const PUSH_REPLY_MIN: u32 = 10;

/// Defaults: integration "dev", release "main", staging "" (none), review_max_rounds 3,
/// max_task_min 60, the branches GEAR itself uses (T-252), gate "" (none: a
/// project names its own gate, T-255).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Conventions {
    pub integration: String,
    pub release: String,
    pub staging: String,
    pub review_max_rounds: u32,
    /// The cap on a task's planned time in minutes (T-279): a plan whose
    /// estimate is over it is refused, so the captain cuts the work into
    /// tasks each with its own test cycle. Default 60.
    pub max_task_min: u32,
    /// A task's branch on the remote older than this many minutes is named to
    /// the captain (the branch sweep in the engine). Default 120.
    pub branch_max_age_min: u32,
    /// The open voyages' task branches on the remote may number at most
    /// lanes × this; more are named to the captain. Default 3.
    pub branch_max_per_lane: u32,
    /// A lane working this many minutes without a push is asked to tell the
    /// captain why. Default 20.
    pub push_interval_min: u32,
    /// The minutes the lane has for that answer; without it the captain is
    /// told the lane is in trouble. Default 10.
    pub push_reply_min: u32,
    /// What the engine runs in its integration worktree after a merge, before
    /// it pushes (T-12). No default (T-255): GEAR cannot know another stack's
    /// build + test command, so the project names its own — a `[[repos]]`
    /// entry may name its gate instead.
    pub gate: String,
    /// The branch a task's work goes on (T-252): a template with `{project}`,
    /// `{lane}` and `{n}` filled in. Default `{lane}/t{n}`.
    pub task_branch: String,
    /// The branch a task is reviewed on (T-252), expanded like
    /// `task_branch`. Default `review/t{n}`.
    pub review_branch: String,
    /// The branch of a seat's own worktree (T-252), with `{project}` and
    /// `{seat}` filled in: the merge worktree is seat `merge`, so Herdr names
    /// the worktree `<project>-<seat>`. Default `{project}/{seat}`.
    pub seat_branch: String,
    /// The git remote a hand-in is checked against (T-257): the branch is
    /// fetched from it and `<remote>/<branch>` is the ref the check reads.
    /// Default `origin`.
    pub remote: String,
    /// The folder a voyage keeps its plan, reports and reviews in (T-257), a
    /// template with `{project}` and `{milestone}` filled in. Default
    /// `gear/voyages/{milestone}` (one folder per voyage).
    pub voyage_dir: String,
    /// The message of a task's merge commit (T-299), a template with
    /// `{project}`, `{task}` and `{source}` filled in. Default
    /// `merge({task}): {source}`.
    pub merge_message: String,
    /// The message of a reviewer's commit (T-299), a template with `{project}`,
    /// `{task}`, `{attempt}` and `{what}` filled in. Default
    /// `review({task}): {what}`.
    pub review_message: String,
    /// The message of the captain's plan commit (T-299), a template with
    /// `{project}`, `{milestone}` and `{what}` filled in. Default
    /// `{milestone}: {what}`.
    pub plan_message: String,
    /// Where a seat's temp files go (T-413), a path (`~` is the home folder):
    /// each seat gets `<scratch_dir>/<project>/<seat>` as its `TMPDIR`. Absent
    /// = the seat's own worktree holds them, `<worktree>/.gear/tmp`.
    pub scratch_dir: Option<String>,
}

impl Default for Conventions {
    fn default() -> Self {
        Self {
            integration: "dev".to_string(),
            release: "main".to_string(),
            staging: String::new(),
            review_max_rounds: 3,
            max_task_min: MAX_TASK_MIN,
            branch_max_age_min: BRANCH_MAX_AGE_MIN,
            branch_max_per_lane: BRANCH_MAX_PER_LANE,
            push_interval_min: PUSH_INTERVAL_MIN,
            push_reply_min: PUSH_REPLY_MIN,
            gate: String::new(),
            task_branch: gear_model::branch::TASK_BRANCH.to_string(),
            review_branch: gear_model::branch::REVIEW_BRANCH.to_string(),
            seat_branch: gear_model::branch::SEAT_BRANCH.to_string(),
            remote: gear_model::layout::REMOTE.to_string(),
            voyage_dir: gear_model::layout::VOYAGE_DIR.to_string(),
            merge_message: gear_model::commit::MERGE_MESSAGE.to_string(),
            review_message: gear_model::commit::REVIEW_MESSAGE.to_string(),
            plan_message: gear_model::commit::PLAN_MESSAGE.to_string(),
            scratch_dir: None,
        }
    }
}

/// The finding for a project that does not name its gate (T-255).
const GATE_MISSING: &str = "name the command the merge gate runs in the integration worktree; it must exit non-zero on any failure (e.g. `make gate`, `go test ./...`)";

/// What to do when an agent stops on a provider limit (17 §0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuotaPolicy {
    /// Switch to the next model in the chain; back after the reset.
    #[default]
    Fallback,
    /// Park until the reset, then wake on the same model.
    Wait,
}

/// What happens to a lane's agent while it is parked on a long quota wait
/// (T-325): keep the agent, or release it until the reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WaitPolicy {
    /// The agent stays; it waits out the quota in its pane.
    #[default]
    Keep,
    /// The agent exits until the reset; its worktree and session are kept.
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct OnQuota {
    pub lanes: QuotaPolicy,
    pub review: QuotaPolicy,
}

/// Ordered model chains (primary first, then fallbacks), named by full
/// provider/model id. Defaults: all chains empty, both policies Fallback,
/// quota_reset_h 5.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Models {
    pub lanes: Vec<ModelEntry>,
    pub review: Vec<ModelEntry>,
    pub captain: Vec<ModelEntry>,
    pub on_quota: OnQuota,
    /// What a lane parked on a long quota wait does with its agent (T-325).
    pub on_wait: WaitPolicy,
    pub quota_reset_h: u32,
    /// How a model is found in its CLI's model picker and recognised on the
    /// status bar, keyed by the model id (T-33): `[models.switch."<id>"]`
    /// with `picker` and `bar` (and an optional `variant`).
    #[serde(default)]
    pub switch: BTreeMap<String, ModelSwitch>,
}

/// One model of a chain (T-35).
///
/// The **plain form** is the model id alone: the seat's own CLI runs it, with
/// that CLI's own way of taking the model (`"minimax-cn/MiniMax-M3.1-Flash"`).
///
/// The **entry form** names the CLI that runs the model and the arguments it
/// starts with:
///
/// ```toml
/// { model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=medium"] }
/// ```
///
/// A chain of entries may cross CLIs: when the next model needs another CLI,
/// the engine closes the agent and starts that CLI in the same pane (T-35).
/// The reasoning effort of codex is a property of the entry, never a constant
/// in the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEntry {
    /// The plain form: the model id only.
    Plain(String),
    /// The entry form: the model, the CLI that runs it and its start args.
    Entry(ModelSpec),
}

/// The entry form of a model: the model id, the CLI that runs it and the
/// arguments that CLI starts with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// The model id, as the CLI names it (`gpt-6.1-sol`).
    pub model: String,
    /// The CLI (the Herdr agent kind) that runs the model: `codex`, `claude`…
    /// Without it the seat's own CLI runs the model.
    #[serde(default)]
    pub cli: Option<String>,
    /// The arguments the CLI starts with, after the model flag: the reasoning
    /// effort of codex, the effort of Claude Code…
    #[serde(default)]
    pub args: Vec<String>,
    /// Model context window in thousands of tokens, when configured.
    #[serde(default)]
    pub context_window_k: Option<u64>,
}

impl ModelEntry {
    /// The model id, trimmed.
    pub fn model(&self) -> &str {
        match self {
            ModelEntry::Plain(model) => model.trim(),
            ModelEntry::Entry(spec) => spec.model.trim(),
        }
    }

    /// The CLI that runs the model, when the entry names one.
    pub fn cli(&self) -> Option<&str> {
        match self {
            ModelEntry::Plain(_) => None,
            ModelEntry::Entry(spec) => spec
                .cli
                .as_deref()
                .map(str::trim)
                .filter(|cli| !cli.is_empty()),
        }
    }

    /// The arguments the CLI starts with, after the model flag.
    pub fn args(&self) -> &[String] {
        match self {
            ModelEntry::Plain(_) => &[],
            ModelEntry::Entry(spec) => &spec.args,
        }
    }

    pub fn context_window_k(&self) -> Option<u64> {
        match self {
            ModelEntry::Plain(_) => None,
            ModelEntry::Entry(spec) => spec.context_window_k,
        }
    }

    /// The entry as one line, for a log or a message: the model id, and the
    /// CLI it runs on when the entry names one.
    pub fn describe(&self) -> String {
        match self.cli() {
            Some(cli) => format!("{} ({cli})", self.model()),
            None => self.model().to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for ModelEntry {
    /// A model id (the plain form) or a model entry (the entry form). The
    /// error names the field, so a wrong config is fixed without guessing.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ModelEntryVisitor)
    }
}

/// Reads one chain entry: a string, or a table with `model`, `cli` and
/// `args`.
struct ModelEntryVisitor;

impl<'de> serde::de::Visitor<'de> for ModelEntryVisitor {
    type Value = ModelEntry;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "a model id (\"provider/model\") or a model entry \
             ({ model = \"…\", cli = \"…\", args = [ … ] })",
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(ModelEntry::Plain(value.to_string()))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut model: Option<String> = None;
        let mut cli: Option<String> = None;
        let mut args: Vec<String> = Vec::new();
        let mut context_window_k = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "model" => {
                    if model.is_some() {
                        return Err(serde::de::Error::duplicate_field("model"));
                    }
                    model = Some(map.next_value()?);
                }
                "cli" => {
                    if cli.is_some() {
                        return Err(serde::de::Error::duplicate_field("cli"));
                    }
                    cli = Some(map.next_value()?);
                }
                "args" => args = map.next_value()?,
                "context_window_k" => context_window_k = Some(map.next_value()?),
                other => {
                    return Err(serde::de::Error::unknown_field(
                        other,
                        &["model", "cli", "args", "context_window_k"],
                    ))
                }
            }
        }
        let model = model.ok_or_else(|| serde::de::Error::missing_field("model"))?;
        Ok(ModelEntry::Entry(ModelSpec {
            model,
            cli,
            args,
            context_window_k,
        }))
    }
}

/// The texts an in-place model switch needs (T-33): what to type into the
/// picker (specific enough to pick the wanted entry, L-29) and what the status
/// bar must show afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSwitch {
    pub picker: String,
    pub bar: String,
    /// The answer to a "Select variant" dialog; default `high`.
    #[serde(default)]
    pub variant: Option<String>,
}

impl Default for Models {
    fn default() -> Self {
        Self {
            lanes: Vec::new(),
            review: Vec::new(),
            captain: Vec::new(),
            on_quota: OnQuota::default(),
            on_wait: WaitPolicy::default(),
            quota_reset_h: 5,
            switch: BTreeMap::new(),
        }
    }
}

/// Per-million-token rates for one provider/model.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Defaults: tick_s 4, cooldown_s 90, sample_min 20, review_timer_min 45.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Timing {
    pub tick_s: u32,
    pub cooldown_s: u32,
    pub sample_min: u32,
    pub review_timer_min: u32,
    /// Promote and remind open captain items once per this many minutes.
    pub captain_item_stale_min: u32,
    /// Working with no attempt in development while ready work waits.
    pub lane_busy_idle_min: u32,
    pub delivery_grace_s: u32,
    /// How many minutes a hand-in may be behind the integration branch and
    /// still be accepted (T-221): a base that moved while the lane merged and
    /// ran its suite is not a stale base. `0` = a hand-in must contain the
    /// current tip (the strict rule).
    pub handin_base_window_min: u32,
    /// How long an at-once agent switch lets the outgoing agent wrap up and
    /// commit before the engine exits it anyway (T-465); default 10 minutes.
    pub switch_grace_min: u32,
    /// Whether an agent switch (a new CLI) happens at once instead of at the
    /// seat's next safe point (T-465); default false = the next safe point.
    pub switch_at_once: bool,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            tick_s: 4,
            cooldown_s: 90,
            sample_min: 20,
            review_timer_min: 45,
            captain_item_stale_min: 15,
            lane_busy_idle_min: 10,
            delivery_grace_s: 180,
            handin_base_window_min: 30,
            switch_grace_min: 10,
            switch_at_once: false,
        }
    }
}

/// A voyage that departs at an agreed time (T-448): before it, the engine
/// starts no lane and delivers nothing; at it, it starts them by itself.
///
/// ```toml
/// [schedule]
/// depart_at = "2026-10-30T14:00:00-05:00"  # RFC3339
/// hours = "09:00-18:00"                    # local time
/// notice_min = 15
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScheduleSection {
    /// The moment the voyage departs (RFC3339); absent = as soon as ready.
    pub depart_at: Option<String>,
    /// The working hours of the day (`"HH:MM-HH:MM"`, local time).
    pub hours: Option<String>,
    /// How long before the departure the captain is told (T-450).
    pub notice_min: u32,
}

impl Default for ScheduleSection {
    fn default() -> Self {
        Self {
            depart_at: None,
            hours: None,
            notice_min: 15,
        }
    }
}

/// The moment a voyage departs (T-448): `Some(depart_at)` while `now` is
/// before it, `None` at and after it, and when the config names none. A
/// `depart_at` that does not parse is refused by `validate`, so a reader that
/// skipped the check fails closed (no departure time, today's behaviour).
pub fn departs_at(config: &ProjectConfig, now: Ts) -> Option<Ts> {
    let at = crate::plan::parse_rfc3339(config.schedule.as_ref()?.depart_at.as_deref()?)?;
    (now < at).then_some(at)
}

/// The working hours of `[schedule].hours` as (start, end) in minutes past
/// local midnight. `None` when the section names no hours or they do not
/// parse (the config check refuses them).
fn hours_window(config: &ProjectConfig) -> Option<(u32, u32)> {
    let text = config.schedule.as_ref()?.hours.as_deref()?;
    let (start, end) = text.trim().split_once('-')?;
    let start = minutes_of_day(start)?;
    let end = minutes_of_day(end)?;
    (start != end).then_some((start, end))
}

/// Whether `minute` (past local midnight) is inside the working hours. An
/// overnight window (`start > end`) works from its start to its end on the
/// next day.
fn inside_hours(window: (u32, u32), minute: u32) -> bool {
    let (start, end) = window;
    if start < end {
        start <= minute && minute < end
    } else {
        minute >= start || minute < end
    }
}

/// The local time at `minute` past midnight, `day_offset` days after the day
/// of `now`. `None` when the machine's local time is not readable.
fn local_at(now: Ts, day_offset: i32, minute: u32) -> Option<Ts> {
    let stamp = now as libc::time_t;
    let mut parts = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers address valid storage; localtime_r initializes tm
    // on success and returns null on failure.
    if unsafe { libc::localtime_r(&stamp, parts.as_mut_ptr()) }.is_null() {
        return None;
    }
    let mut target = unsafe { parts.assume_init() };
    target.tm_mday += day_offset;
    target.tm_hour = (minute / 60) as i32;
    target.tm_min = (minute % 60) as i32;
    target.tm_sec = 0;
    target.tm_isdst = -1;
    let time = unsafe { libc::mktime(&mut target) } as Ts;
    (time != -1).then_some(time)
}

/// The minute of the day `now` is, in local time. `None` when the machine's
/// local time is not readable.
fn local_minute(now: Ts) -> Option<u32> {
    let stamp = now as libc::time_t;
    let mut parts = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: as in `local_at`.
    if unsafe { libc::localtime_r(&stamp, parts.as_mut_ptr()) }.is_null() {
        return None;
    }
    let parts = unsafe { parts.assume_init() };
    Some(parts.tm_hour as u32 * 60 + parts.tm_min as u32)
}

/// The end of the working period that is current or next (T-450): the hour the
/// fleet stops, from `[schedule].hours` and the local time of `now`.
pub fn hours_end(config: &ProjectConfig, now: Ts) -> Option<Ts> {
    let window = hours_window(config)?;
    let minute = local_minute(now)?;
    let day = if minute < window.1 { 0 } else { 1 };
    local_at(now, day, window.1)
}

/// The start of the next working period, while `now` is outside the working
/// hours (T-450): when the fleet comes back from `[schedule].hours`. `None`
/// without hours, and while `now` is inside them.
pub fn off_hours_until(config: &ProjectConfig, now: Ts) -> Option<Ts> {
    let window = hours_window(config)?;
    let minute = local_minute(now)?;
    if inside_hours(window, minute) {
        return None;
    }
    if window.0 < window.1 {
        let day = if minute < window.0 { 0 } else { 1 };
        local_at(now, day, window.0)
    } else {
        // Overnight: outside is between the end and the start, so the start is
        // later today.
        local_at(now, 0, window.0)
    }
}

/// The start of the next working period after `now`, inside or outside the
/// working hours (T-450): the time the notice tells a seat to continue at.
pub fn hours_next_start(config: &ProjectConfig, now: Ts) -> Option<Ts> {
    let window = hours_window(config)?;
    let minute = local_minute(now)?;
    if window.0 < window.1 {
        let day = if minute < window.0 { 0 } else { 1 };
        local_at(now, day, window.0)
    } else if minute >= window.0 {
        local_at(now, 1, window.0)
    } else {
        local_at(now, 0, window.0)
    }
}

/// `"HH:MM-HH:MM"` with a start different from its end, both real times of
/// the day.
fn valid_hours(text: &str) -> bool {
    let Some((start, end)) = text.trim().split_once('-') else {
        return false;
    };
    let Some(start) = minutes_of_day(start) else {
        return false;
    };
    let Some(end) = minutes_of_day(end) else {
        return false;
    };
    start != end
}

/// `"HH:MM"` as minutes past midnight; `None` when it is not a time of day.
fn minutes_of_day(text: &str) -> Option<u32> {
    let (hours, minutes) = text.split_once(':')?;
    if hours.len() != 2 || minutes.len() != 2 {
        return None;
    }
    if !hours.bytes().all(|byte| byte.is_ascii_digit())
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    (hours <= 23 && minutes <= 59).then_some(hours * 60 + minutes)
}

/// One validation finding. `field` is a dotted path such as `lanes[1].owns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub field: String,
    pub problem: String,
}

/// Parse TOML text. Syntax errors, unknown keys, wrong types and missing
/// required keys → `ErrorCode::BadFormat` with the TOML error message as detail.
pub fn parse(text: &str) -> GearResult<ProjectConfig> {
    toml::from_str::<ProjectConfig>(text)
        .map_err(|e| GearError::new(ErrorCode::BadFormat, e.to_string()))
}

/// The config a project is registered with: the text the store holds for its
/// **latest** config version, parsed. A project that is not registered →
/// `NoTarget`; stored text that does not parse → `BadFormat`.
///
/// The config is read from the store every time, so a config change (T-6) is
/// what the next reader sees — there is no second copy anywhere. A caller that
/// also needs the seats the config is read against (the engine's onboarding)
/// reads both in **one** transaction and parses outside it, so a config change
/// can never be seen half-applied (review R-63).
pub fn latest(store: &mut Store, project: &str) -> GearResult<ProjectConfig> {
    let row = store.read(|tx| tx.get_project(project))?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no project {project} in the store"),
        )
    })?;
    parse(&row.config_toml)
}

/// The project's `[conventions]` for a reader that only needs the branch names
/// the engine cuts (T-252): the defaults when the project's config text is
/// empty (a store written before a config was stored) or does not parse. A
/// name is never a gate — the merge is checked on the candidate, not on the
/// branch it came from — so a reader here never stops a hand-in over it.
pub fn conventions_or_default(store: &mut Store, project: &str) -> GearResult<Conventions> {
    let row = store.read(|tx| tx.get_project(project))?;
    Ok(row
        .filter(|row| !row.config_toml.trim().is_empty())
        .and_then(|row| parse(&row.config_toml).ok())
        .map(|config| config.conventions)
        .unwrap_or_default())
}

/// The profile table of a project (T-31): `base` (the built-ins) with the
/// project's `[profiles.<name>]` on top — a field left out keeps the value the
/// profile already has. A name that is not a built-in starts from the
/// defaults, with the kind it is named after.
///
/// A profile block that does not parse (an unknown field, a wrong type) is
/// `BadFormat`, naming the profile.
pub fn profile_table(
    base: &BTreeMap<String, gear_herdr::Profile>,
    config: &ProjectConfig,
) -> GearResult<BTreeMap<String, gear_herdr::Profile>> {
    let mut table = base.clone();
    for (name, raw) in &config.profiles {
        let patch: gear_herdr::ProfilePatch =
            toml::Value::Table(raw.clone()).try_into().map_err(|e| {
                GearError::new(
                    ErrorCode::BadFormat,
                    format!("profile {name} is not a valid profile: {e}"),
                )
            })?;
        // A name that is not built in starts from the operational defaults
        // (16 lines, two confirmation rounds), not from an all-zero profile
        // (review R2).
        let current = table
            .get(name)
            .cloned()
            .unwrap_or_else(gear_herdr::Profile::for_new);
        let mut profile = current.patched(&patch, name);
        // A profile the user configured is one GEAR may drive: they wrote the
        // kind, the args and the exit keys (T-31). `gear admin init` refuses
        // the ones that cannot run unattended, so the trust is checked there.
        profile.verified = true;
        table.insert(name.clone(), profile);
    }
    Ok(table)
}

/// All findings, in field order (project, quota, captain, lanes in order,
/// repos in order, conventions, timing, models); empty = valid. Rules:
/// - `project.id`, every lane `name`: `[a-z][a-z0-9-]*` (`gear_model::valid_name`
///   is NOT this rule — check it here);
/// - `<project.id>-<lane.name>` and `<project.id>-captain` ≤ 32 chars (Herdr agent names);
/// - lane names unique; lane `duty` non-empty after trim; `owns` non-empty;
/// - `owns` entries of two different lanes must not overlap: equal, or one is a
///   path prefix of the other on a `/` boundary (`src/a/` vs `src/a/b.rs` overlap;
///   `src/a` vs `src/ab` do not) — only between lanes of the **same** repo
///   (T-52): the same paths in two repos are two different files;
/// - the lane quota (`quota.lanes`) is not checked here: only the seats know
///   which lanes are retired (T-304), so `admin::lane_quota_problem` does it;
/// - every `[[quota.sources]]`: a provider name and a reader the engine knows
///   (`minimax`, `opencode-go`, `codex`);
/// - every `[[repos]]` name: `[a-z][a-z0-9-]*` and unique;
/// - every `[[repos]]` path: relative to `project.repo` (not absolute, no `..`);
/// - every `[[repos]]` gate: not empty and no TODO; its integration and
///   release: non-empty and different (its own, else `[conventions]`'s);
/// - a gate: no default (T-255), so `conventions.gate` must be named unless
///   every repo the project runs work in has its own — a repo without one (the
///   root among them, when a lane works there or it is the whole project) runs
///   `conventions.gate`; `conventions.gate` is not left as a TODO either;
/// - a gate (`conventions.gate` or a repo's own) that pipes into another
///   command without `pipefail` (T-242): the pipeline's exit status is its last
///   command's, so a failure in an earlier stage can pass;
/// - a lane's `repo` names one of the `[[repos]]` entries;
/// - `conventions.integration` and `release` non-empty and different; `staging`
///   if non-empty differs from both;
/// - `conventions.remote` non-empty and without whitespace, and
///   `conventions.voyage_dir` names `{milestone}` (T-257);
/// - a seat branch that git cannot hold together with a task or review branch
///   (T-372): a ref and a directory of one name cannot both exist, so the
///   seat branch of a lane, of the merge seat and of a reviewer must not be a
///   task or review branch, nor a folder of one;
/// - `timing.cooldown_s > timing.tick_s`;
/// - `models.lanes`, `models.review`, `models.captain` and each lane's `models`
///   hold no blank and no repeated entry (compared trimmed; an entry is the
///   model id together with the CLI it names);
/// - a non-empty `models.review` holds at least one model different from every
///   lane's primary (`models[0]`, else `models.lanes[0]`), so the reviewer
///   never runs the model it reviews;
/// - `models.quota_reset_h ≥ 1`;
/// - an entry whose CLI is `claude` (its own, else the kind of the seat that
///   runs it) names a bare model id or alias, never `provider/model` (T-235).
/// Empty chains are valid: models are not managed by GEAR then.
/// The finding for a gate that pipes without `pipefail` (T-242).
const GATE_PIPE: &str = "the gate pipes into another command, so its exit status is the last command's and a failure can pass: drop the pipe or start with `set -o pipefail;`";

/// A gate that pipes into another command reports only that command's exit
/// status: a failure in an earlier stage passes (T-242). `||` is not a pipe,
/// and `pipefail` makes the pipeline report the failing stage.
fn gate_pipes_without_pipefail(gate: &str) -> bool {
    if gate.contains("pipefail") {
        return false;
    }
    let bytes = gate.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'|' {
            if bytes.get(index + 1) == Some(&b'|') {
                index += 2;
                continue;
            }
            return true;
        }
        index += 1;
    }
    false
}

pub fn validate(cfg: &ProjectConfig) -> Vec<ConfigIssue> {
    let mut issues = Vec::new();

    if cfg.timing.delivery_grace_s == 0 {
        issues.push(issue(
            "timing.delivery_grace_s",
            "must be at least one second".to_string(),
        ));
    }
    // A cap of zero would refuse every task (T-279): the captain plans each
    // task's time, and no estimate is under it.
    if cfg.conventions.max_task_min == 0 {
        issues.push(issue(
            "conventions.max_task_min",
            format!("must be at least one minute (the default is {MAX_TASK_MIN})"),
        ));
    }
    if cfg.conventions.branch_max_age_min == 0 {
        issues.push(issue(
            "conventions.branch_max_age_min",
            format!("must be at least one minute (the default is {BRANCH_MAX_AGE_MIN})"),
        ));
    }
    if cfg.conventions.branch_max_per_lane == 0 {
        issues.push(issue(
            "conventions.branch_max_per_lane",
            format!("must be at least one (the default is {BRANCH_MAX_PER_LANE})"),
        ));
    }
    if cfg.conventions.push_interval_min == 0 {
        issues.push(issue(
            "conventions.push_interval_min",
            format!("must be at least one minute (the default is {PUSH_INTERVAL_MIN})"),
        ));
    }
    if cfg.conventions.push_reply_min == 0 {
        issues.push(issue(
            "conventions.push_reply_min",
            format!("must be at least one minute (the default is {PUSH_REPLY_MIN})"),
        ));
    }
    for (model, price) in &cfg.prices {
        if !model
            .split_once('/')
            .is_some_and(|(provider, name)| !provider.trim().is_empty() && !name.trim().is_empty())
        {
            issues.push(issue(
                &format!("prices.{model}"),
                "must name a provider/model".to_string(),
            ));
        }
        for (field, value) in [
            ("input", price.input),
            ("output", price.output),
            ("cache_read", price.cache_read),
            ("cache_write", price.cache_write),
        ] {
            if !value.is_finite() || value < 0.0 {
                issues.push(issue(
                    &format!("prices.{model}.{field}"),
                    "must be a finite non-negative rate".to_string(),
                ));
            }
        }
    }

    // ---- the gate runs before every merge in the integration worktree, and
    // it is the project's own command: GEAR ships no default (T-255), because
    // a default that fits one stack's build leaves another project with a
    // gate that cannot run. A `[[repos]]` entry may name its own gate; the
    // root repo, and every entry without one, run `conventions.gate`, so when
    // that is empty the project must still name a gate for them. The root
    // counts only when it is the whole project, or when a lane works in it
    // (a lane without `repo`, or naming a repo that is not there): with no
    // work of its own it never runs a gate (M-4220).
    if cfg.conventions.gate.contains("TODO") {
        issues.push(issue(
            "conventions.gate",
            "conventions.gate must be your build + test command (it runs before every merge)"
                .to_string(),
        ));
    }
    let gate_missing = cfg.conventions.gate.trim().is_empty();
    let repo_without_own_gate = cfg.repos.iter().any(|repo| {
        repo.gate
            .as_deref()
            .is_none_or(|gate| gate.trim().is_empty())
    });
    let root_has_work = cfg.repos.is_empty()
        || cfg.lanes.iter().any(|lane| {
            lane.repo
                .as_deref()
                .is_none_or(|name| !cfg.repos.iter().any(|repo| repo.name == name))
        });
    if gate_missing && (root_has_work || repo_without_own_gate) {
        issues.push(issue("conventions.gate", GATE_MISSING.to_string()));
    }
    if gate_pipes_without_pipefail(&cfg.conventions.gate) {
        issues.push(issue("conventions.gate", GATE_PIPE.to_string()));
    }

    // ---- quota policy: a fallback needs a next model (L-74). A one-model
    // chain with `fallback` is really "park for quota_reset_h hours" — say so
    // at kickoff instead of finding out when every lane stops.
    for (field, chain, policy) in [
        (
            "models.lanes",
            &cfg.models.lanes,
            &cfg.models.on_quota.lanes,
        ),
        (
            "models.review",
            &cfg.models.review,
            &cfg.models.on_quota.review,
        ),
    ] {
        if *policy == QuotaPolicy::Fallback && chain.len() == 1 {
            issues.push(issue(
                field,
                format!(
                    "on_quota is \"fallback\" but {field} has no next model: a quota stop would park for {}h. Add a fallback model, or set on_quota to \"wait\"",
                    cfg.models.quota_reset_h
                ),
            ));
        }
    }

    // ---- quota sources (§9.1): the engine reads the quota of these
    // providers; a reader it does not know is a finding, not a silent skip.
    for source in &cfg.quota.sources {
        let provider = source.provider.trim();
        if provider.is_empty() {
            issues.push(issue(
                "quota.sources.provider",
                "a quota source needs the provider it is about".to_string(),
            ));
            continue;
        }
        if gear_usage::QuotaReader::from_name(&source.reader).is_none() {
            issues.push(issue(
                &format!("quota.sources.{provider}.reader"),
                format!(
                    "the reader {:?} is not one of them: minimax, opencode-go, codex",
                    source.reader
                ),
            ));
        }
    }

    // ---- project
    if !is_name(&cfg.project.id) {
        issues.push(issue(
            "project.id",
            format!("project id {:?} must match [a-z][a-z0-9-]*", cfg.project.id),
        ));
    }
    if !agent_name_fits(&cfg.project.id, "captain") {
        issues.push(issue(
            "project.id",
            format!(
                "agent name {:?} is longer than 32 characters",
                agent_name(&cfg.project.id, "captain")
            ),
        ));
    }

    // ---- quota: not checked here. The quota counts the lanes that can run,
    // which means the seats, retired ones excluded (T-304); `validate` sees only
    // the config, so `admin::lane_quota_problem` makes that finding.

    // ---- captain: nothing to check here (pane, kind and profile are checked
    // against Herdr by `gear admin init`).

    // ---- lanes, in order
    let mut first_lane_with_name: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, lane) in cfg.lanes.iter().enumerate() {
        let field = format!("lanes[{index}].name");
        if !is_name(&lane.name) {
            issues.push(issue(
                &field,
                format!("lane name {:?} must match [a-z][a-z0-9-]*", lane.name),
            ));
        }
        if !agent_name_fits(&cfg.project.id, &lane.name) {
            issues.push(issue(
                &field,
                format!(
                    "agent name {:?} is longer than 32 characters",
                    agent_name(&cfg.project.id, &lane.name)
                ),
            ));
        }
        match first_lane_with_name.get(lane.name.as_str()) {
            Some(first) => issues.push(issue(
                &field,
                format!(
                    "lane name {:?} is already used by lanes[{first}]",
                    lane.name
                ),
            )),
            None => {
                first_lane_with_name.insert(lane.name.as_str(), index);
            }
        }

        if lane.duty.trim().is_empty() {
            issues.push(issue(
                &format!("lanes[{index}].duty"),
                "a lane must state its duty (conventions 17 §1)".to_string(),
            ));
        }

        // The lane's repo must be one of the project's repos (T-52).
        if let Some(repo) = &lane.repo {
            if !cfg.repos.iter().any(|entry| &entry.name == repo) {
                issues.push(issue(
                    &format!("lanes[{index}].repo"),
                    format!("the lane's repo {repo:?} is not one of the project's repos"),
                ));
            }
        }

        let owns_field = format!("lanes[{index}].owns");
        let owns: Vec<&str> = lane.owns.iter().map(|entry| entry.trim()).collect();
        if owns.is_empty() {
            issues.push(issue(
                &owns_field,
                "a lane must state what it owns (conventions 17 §1)".to_string(),
            ));
        } else if owns.iter().any(|entry| entry.is_empty()) {
            issues.push(issue(
                &owns_field,
                "an owns entry is empty after trimming".to_string(),
            ));
        } else if let Some(other) = cfg.lanes[..index].iter().find(|earlier| {
            // Two lanes overlap only when they work in the same repo
            // (T-52): the same paths in two repos are two different
            // files, and a lane's `owns` is relative to its repo.
            earlier.repo == lane.repo && owns_overlap(&earlier.owns, &lane.owns)
        }) {
            issues.push(issue(
                &owns_field,
                format!("overlaps the paths owned by lane {:?}", other.name),
            ));
        }

        // A charter (T-469) that is present must say something: a blank one
        // would replace the lane rules with nothing, so it is a finding.
        if let Some(charter) = &lane.charter {
            if charter.trim().is_empty() {
                issues.push(issue(
                    &format!("lanes[{index}].charter"),
                    "a charter is empty after trimming: leave it out to keep the lane rules"
                        .to_string(),
                ));
            }
        }
    }

    // ---- the reviewers' charter (T-469), the same rule.
    if let Some(temps) = &cfg.temps {
        if let Some(charter) = &temps.charter {
            if charter.trim().is_empty() {
                issues.push(issue(
                    "temps.charter",
                    "a charter is empty after trimming: leave it out to keep the reviewer rules"
                        .to_string(),
                ));
            }
        }
    }

    // ---- repos, in order (T-52): every entry names a repo below
    // `project.repo`, with its own integration branch, release branch and
    // gate. A repo without branches or a gate uses the project's
    // `[conventions]`.
    let mut first_repo_with_name: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, repo) in cfg.repos.iter().enumerate() {
        let name_field = format!("repos[{index}].name");
        if !is_name(&repo.name) {
            issues.push(issue(
                &name_field,
                format!("repo name {:?} must match [a-z][a-z0-9-]*", repo.name),
            ));
        }
        match first_repo_with_name.get(repo.name.as_str()) {
            Some(first) => issues.push(issue(
                &name_field,
                format!(
                    "repo name {:?} is already used by repos[{first}]",
                    repo.name
                ),
            )),
            None => {
                first_repo_with_name.insert(repo.name.as_str(), index);
            }
        }

        // The path is relative to `project.repo`: the engine opens the repo
        // there, so an absolute path or one that leaves the project would
        // point the work at another checkout.
        let path_field = format!("repos[{index}].path");
        let path = repo.path.trim();
        if path.is_empty() {
            issues.push(issue(
                &path_field,
                "a repo must state its path, relative to project.repo".to_string(),
            ));
        } else if path.starts_with('/') {
            issues.push(issue(
                &path_field,
                format!("the repo path {path:?} must be relative to project.repo, not absolute"),
            ));
        } else if path.split('/').any(|part| part == "..") {
            issues.push(issue(
                &path_field,
                format!("the repo path {path:?} must not leave project.repo with \"..\""),
            ));
        }

        // The repo's own branches, else the project's.
        let repo_integration = repo
            .integration
            .as_deref()
            .unwrap_or(&cfg.conventions.integration)
            .trim();
        let repo_release = repo
            .release
            .as_deref()
            .unwrap_or(&cfg.conventions.release)
            .trim();
        if repo_integration.is_empty() {
            issues.push(issue(
                &format!("repos[{index}].integration"),
                "the integration branch must not be empty".to_string(),
            ));
        }
        if repo_release.is_empty() {
            issues.push(issue(
                &format!("repos[{index}].release"),
                "the release branch must not be empty".to_string(),
            ));
        }
        if !repo_integration.is_empty()
            && !repo_release.is_empty()
            && repo_integration == repo_release
        {
            issues.push(issue(
                &format!("repos[{index}].release"),
                format!(
                    "the release branch must differ from the integration branch {repo_integration:?}"
                ),
            ));
        }

        // The gate runs before every merge in that repo: it must be the
        // repo's own build + test command. A repo without one runs the
        // project's gate, which is checked above.
        if let Some(gate) = &repo.gate {
            if gate.trim().is_empty() || gate.contains("TODO") {
                issues.push(issue(
                    &format!("repos[{index}].gate"),
                    "the gate must be the repo's own build + test command (it runs before every merge in that repo)".to_string(),
                ));
            }
            if gate_pipes_without_pipefail(gate) {
                issues.push(issue(
                    &format!("repos.{}.gate", repo.name),
                    GATE_PIPE.to_string(),
                ));
            }
        }
    }

    // ---- conventions
    let conventions = &cfg.conventions;
    let integration = conventions.integration.trim();
    let release = conventions.release.trim();
    let staging = conventions.staging.trim();
    if integration.is_empty() {
        issues.push(issue(
            "conventions.integration",
            "the integration branch must not be empty".to_string(),
        ));
    }
    if release.is_empty() {
        issues.push(issue(
            "conventions.release",
            "the release branch must not be empty".to_string(),
        ));
    }
    if !integration.is_empty() && !release.is_empty() && integration == release {
        issues.push(issue(
            "conventions.release",
            format!("the release branch must differ from the integration branch {integration:?}"),
        ));
    }
    // A blank staging branch means "no staging branch".
    if !staging.is_empty() {
        let clash = if staging == integration {
            Some(integration)
        } else if staging == release {
            Some(release)
        } else {
            None
        };
        if let Some(other) = clash {
            issues.push(issue(
                "conventions.staging",
                format!("the staging branch must differ from {other:?}"),
            ));
        }
    }

    // ---- the remote and the voyage folder (T-257): a remote with a blank
    // or a space in it is no remote git can fetch from, and a voyage folder
    // without `{milestone}` keeps every voyage in one place.
    let remote = conventions.remote.trim();
    if remote.is_empty() || remote.chars().any(char::is_whitespace) {
        issues.push(issue(
            "conventions.remote",
            "the remote must be one git remote name, without spaces (e.g. origin, upstream)"
                .to_string(),
        ));
    }
    if !conventions.voyage_dir.contains("{milestone}") {
        issues.push(issue(
            "conventions.voyage_dir",
            "the voyage folder must name {milestone} (one folder per voyage, e.g. gear/voyages/{milestone})"
                .to_string(),
        ));
    }

    // The commit-message templates (T-299): a team with commit rules (ticket
    // keys, Conventional Commits) names its own; GEAR's are only the defaults.
    // Each is checked here — before a merge or a hand-in writes one — the way a
    // branch template is: the placeholders it may name are the ones the writer
    // fills in.
    message_template_issues(
        "conventions.merge_message",
        &conventions.merge_message,
        Some("task"),
        &["project", "task", "source"],
        &mut issues,
    );
    message_template_issues(
        "conventions.review_message",
        &conventions.review_message,
        Some("task"),
        &["project", "task", "attempt", "what"],
        &mut issues,
    );
    message_template_issues(
        "conventions.plan_message",
        &conventions.plan_message,
        None,
        &["project", "milestone", "what"],
        &mut issues,
    );

    // The branch templates (T-252): the branch names GEAR cuts and
    // checks. A template is expanded with its placeholders; anything else stays
    // a literal brace and git refuses the branch, so each is checked here — on
    // a sample — rather than when a lane cannot push.
    let templates: [(&str, &str, &str, &[&str]); 3] = [
        // the field, the template, the placeholder it must name, the ones it
        // may name
        (
            "conventions.task_branch",
            &conventions.task_branch,
            "n",
            &["project", "lane", "n"],
        ),
        (
            "conventions.review_branch",
            &conventions.review_branch,
            "n",
            &["project", "lane", "n"],
        ),
        (
            "conventions.seat_branch",
            &conventions.seat_branch,
            "seat",
            &["project", "seat"],
        ),
    ];
    let mut usable = [false; 3];
    for (index, (field, template, required, allowed)) in templates.iter().enumerate() {
        let sample = if *required == "seat" {
            gear_model::branch::expand_seat(template, SAMPLE_PROJECT, SAMPLE_SEAT)
        } else {
            gear_model::branch::expand(template, SAMPLE_PROJECT, SAMPLE_LANE, SAMPLE_TASK)
        };
        usable[index] =
            branch_template_issues(field, template, required, allowed, &sample, &mut issues);
    }
    if usable[0] && usable[1] {
        let task = gear_model::branch::expand(
            &conventions.task_branch,
            SAMPLE_PROJECT,
            SAMPLE_LANE,
            SAMPLE_TASK,
        );
        let review = gear_model::branch::expand(
            &conventions.review_branch,
            SAMPLE_PROJECT,
            SAMPLE_LANE,
            SAMPLE_TASK,
        );
        if task == review {
            issues.push(issue(
                "conventions.review_branch",
                format!(
                    "the review branch {review:?} is the same name as the task branch: a task needs a branch for its work and another for its review"
                ),
            ));
        }
    }

    // ---- a seat branch is a ref, and a lane's task or review branch lives
    // below it when one is a folder of the other (T-372): git cannot hold the
    // ref `prometheus/money` and the branch `prometheus/money/t54` together, so
    // the lane would have to delete its own seat branch to cut a task branch.
    // Checked on a sample per lane, for the lane's own seat and for the two
    // seats every project has: the merge seat and a reviewer's.
    let project = &cfg.project.id;
    for lane in &cfg.lanes {
        let task =
            gear_model::branch::expand(&conventions.task_branch, project, &lane.name, SAMPLE_TASK);
        let review = gear_model::branch::expand(
            &conventions.review_branch,
            project,
            &lane.name,
            SAMPLE_TASK,
        );
        for seat in [lane.name.as_str(), MERGE_SEAT, REVIEW_SEAT] {
            let seat_branch =
                gear_model::branch::expand_seat(&conventions.seat_branch, project, seat);
            for (kind, branch) in [("task", &task), ("review", &review)] {
                if one_branch_holds_the_other(&seat_branch, branch) {
                    issues.push(issue(
                        "conventions.seat_branch",
                        format!(
                            "the seat branch {seat_branch} and the {kind} branch {branch} cannot both exist in git (a ref and a directory of the same name): give the seat branches their own folder, e.g. seat_branch = \"{{project}}/home/{{seat}}\""
                        ),
                    ));
                }
            }
        }
    }

    // ---- timing
    if cfg.timing.cooldown_s <= cfg.timing.tick_s {
        issues.push(issue(
            "timing.cooldown_s",
            format!(
                "the cooldown ({} s) must be longer than the tick ({} s)",
                cfg.timing.cooldown_s, cfg.timing.tick_s
            ),
        ));
    }

    if cfg.timing.captain_item_stale_min == 0 {
        issues.push(issue(
            "timing.captain_item_stale_min",
            "must be at least one minute".to_string(),
        ));
    }

    if cfg.timing.lane_busy_idle_min == 0 {
        issues.push(issue(
            "timing.lane_busy_idle_min",
            "must be at least one minute".to_string(),
        ));
    }

    // ---- the schedule (T-448): a departure time must be an RFC3339 moment
    // the engine can compare with the clock, and the hours of the day must be
    // two real times with a start different from the end.
    if let Some(schedule) = &cfg.schedule {
        if let Some(depart_at) = &schedule.depart_at {
            if crate::plan::parse_rfc3339(depart_at).is_none() {
                issues.push(issue(
                    "schedule.depart_at",
                    format!(
                        "must be an RFC3339 moment (e.g. 2026-10-30T14:00:00-05:00), got {depart_at:?}"
                    ),
                ));
            }
        }
        if let Some(hours) = &schedule.hours {
            if !valid_hours(hours) {
                issues.push(issue(
                    "schedule.hours",
                    format!(
                        "must be two HH:MM times with a different start and end (e.g. \"09:00-18:00\"), got {hours:?}"
                    ),
                ));
            }
        }
    }

    // ---- models (17 §0): empty chains mean models are not managed
    let models = &cfg.models;
    for (field, chain) in [
        ("models.lanes", &models.lanes),
        ("models.review", &models.review),
        ("models.captain", &models.captain),
    ] {
        if let Some(problem) = chain_problem(chain) {
            issues.push(issue(field, problem));
        }
    }
    for (index, lane) in cfg.lanes.iter().enumerate() {
        if let Some(problem) = chain_problem(&lane.models) {
            issues.push(issue(&format!("lanes[{index}].models"), problem));
        }
    }

    // The reviewer must not run the model it reviews.
    let review_models: Vec<&str> = models
        .review
        .iter()
        .map(|entry| entry.model())
        .filter(|model| !model.is_empty())
        .collect();
    if !review_models.is_empty() {
        let independent = review_models.iter().any(|review_model| {
            cfg.lanes
                .iter()
                .all(|lane| match lane_primary(lane, &models.lanes) {
                    Some(primary) => primary.model() != *review_model,
                    None => true,
                })
        });
        if !independent {
            issues.push(issue(
                "models.review",
                "the review chain must hold a model that no lane runs as its primary".to_string(),
            ));
        }
    }

    if models.quota_reset_h < 1 {
        issues.push(issue(
            "models.quota_reset_h",
            "the quota reset window must be at least one hour".to_string(),
        ));
    }

    // ---- the claude CLI takes a bare model id or alias, never a
    // `provider/model` one (T-235): `anthropic/claude-sonnet-5-5` is refused
    // by the CLI itself, which costs a review round to find out. The CLI of
    // an entry is its own, else the kind of the seat that runs it.
    let lane_kinds: Vec<&str> = cfg.lanes.iter().map(|lane| lane.kind.trim()).collect();
    let temps_kinds: Vec<&str> = cfg.temps.iter().map(|temps| temps.kind.trim()).collect();
    claude_model_issues("models.lanes", &models.lanes, &lane_kinds, &mut issues);
    claude_model_issues("models.review", &models.review, &temps_kinds, &mut issues);
    claude_model_issues(
        "models.captain",
        &models.captain,
        &[cfg.captain.kind.trim()],
        &mut issues,
    );
    for (index, lane) in cfg.lanes.iter().enumerate() {
        claude_model_issues(
            &format!("lanes[{index}].models"),
            &lane.models,
            &[lane.kind.trim()],
            &mut issues,
        );
    }

    // ---- the GitHub mode (T-79): absent = local-only, so there is nothing
    // to check then; present = it must name a repo.
    if let Some(github) = &cfg.github {
        github_issues(github, "github", &mut issues);
    }
    for (index, repo) in cfg.repos.iter().enumerate() {
        if let Some(github) = &repo.github {
            github_issues(github, &format!("repos[{index}].github"), &mut issues);
        }
    }

    issues
}

/// A `[github]` section's findings: it names a GitHub repository as
/// `owner/name`, and no issue label is blank.
fn github_issues(section: &GithubSection, field: &str, issues: &mut Vec<ConfigIssue>) {
    match section.repo.as_deref() {
        None => issues.push(issue(
            &format!("{field}.repo"),
            "a [github] section needs `repo = \"owner/name\"` (absent it is local-only, so remove the section)".to_string(),
        )),
        Some(repo) if !is_github_repo(repo) => issues.push(issue(
            &format!("{field}.repo"),
            format!("the GitHub repo {repo:?} must be `owner/name`"),
        )),
        Some(_) => {}
    }
    for (index, label) in section.issues.labels.iter().enumerate() {
        if label.trim().is_empty() {
            issues.push(issue(
                &format!("{field}.issues.labels[{index}]"),
                "an issue label is empty after trimming".to_string(),
            ));
        }
    }
}

/// `owner/name`: one slash, both sides non-empty, no whitespace and no URL
/// punctuation (a GitHub repository, not a URL).
fn is_github_repo(repo: &str) -> bool {
    let Some((owner, name)) = repo.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !name.contains('/')
        && !owner.contains('/')
        && repo
            .chars()
            .all(|c| !c.is_whitespace() && !matches!(c, '@' | ':' | '\\'))
}

// ---------------------------------------------------------------- helpers

fn issue(field: &str, problem: String) -> ConfigIssue {
    ConfigIssue {
        field: field.to_string(),
        problem,
    }
}

/// The sample a branch template is expanded with to check it (T-252): one
/// project, one lane, one seat, one task.
const SAMPLE_PROJECT: &str = "p";
const SAMPLE_LANE: &str = "l";
const SAMPLE_SEAT: &str = "s";
const SAMPLE_TASK: &str = "T-1";

/// The placeholders `template` names, filled in or not.
fn placeholders(template: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        names.push(&after[..close]);
        rest = &after[close + 1..];
    }
    names
}

/// Check one commit-message template (T-299): it must not be empty, it must
/// name `required` when the message needs it, and it may name only `allowed`.
/// Returns whether the template is usable.
fn message_template_issues(
    field: &str,
    template: &str,
    required: Option<&str>,
    allowed: &[&str],
    issues: &mut Vec<ConfigIssue>,
) -> bool {
    let filled: Vec<String> = allowed.iter().map(|name| format!("{{{name}}}")).collect();
    if template.trim().is_empty() {
        issues.push(issue(
            field,
            format!(
                "the commit message template is empty: name one; GEAR fills in {}",
                filled.join(", ")
            ),
        ));
        return false;
    }
    let names = placeholders(template);
    if let Some(unknown) = names.iter().find(|name| !allowed.contains(name)) {
        issues.push(issue(
            field,
            format!(
                "the template {template:?} names {{{unknown}}}: GEAR fills in {}",
                filled.join(", ")
            ),
        ));
        return false;
    }
    if let Some(required) = required {
        if !names.contains(&required) {
            issues.push(issue(
                field,
                format!(
                    "the template {template:?} has no {{{required}}}, so it does not name the task: GEAR fills in {}",
                    filled.join(", ")
                ),
            ));
            return false;
        }
    }
    true
}

/// Check one branch template (T-252): it must name `required`, it may name only
/// `allowed`, and its sample expansion must be a branch name git takes. Returns
/// whether the template is usable.
fn branch_template_issues(
    field: &str,
    template: &str,
    required: &str,
    allowed: &[&str],
    sample: &str,
    issues: &mut Vec<ConfigIssue>,
) -> bool {
    let filled: Vec<String> = allowed.iter().map(|name| format!("{{{name}}}")).collect();
    let names = placeholders(template);
    if let Some(unknown) = names.iter().find(|name| !allowed.contains(name)) {
        issues.push(issue(
            field,
            format!(
                "the template {template:?} names {{{unknown}}}: GEAR fills in {}",
                filled.join(", ")
            ),
        ));
        return false;
    }
    if !names.contains(&required) {
        issues.push(issue(
            field,
            format!(
                "the template {template:?} has no {{{required}}}: each one needs its own branch"
            ),
        ));
        return false;
    }
    if let Some(problem) = branch_name_problem(sample) {
        issues.push(issue(
            field,
            format!("the template {template:?} names the branch {sample:?}: {problem}"),
        ));
        return false;
    }
    true
}

/// The seat names every project has (T-372): the merge worktree and a
/// reviewer's.
const MERGE_SEAT: &str = "merge";
const REVIEW_SEAT: &str = "review-1";

/// Two branch names git cannot hold side by side (T-372): the same name, or one
/// as a folder of the other (`p/heat` and `p/heat/t1`), because a ref and a
/// directory of one name cannot both exist.
fn one_branch_holds_the_other(a: &str, b: &str) -> bool {
    a == b
        || b.strip_prefix(a).is_some_and(|rest| rest.starts_with('/'))
        || a.strip_prefix(b).is_some_and(|rest| rest.starts_with('/'))
}

/// What is wrong with `name` as a branch name, or `None` when git takes it
/// (T-252): letters, digits, '.', '_', '-' and '/' only, no leading '/', '-'
/// or '.', no ".." or "//", and no trailing '/' or ".lock".
fn branch_name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("a branch name must not be empty".to_string());
    }
    if name.starts_with(['/', '-', '.']) {
        return Some("a branch name must not start with '/', '-' or '.'".to_string());
    }
    if name.contains("..") || name.contains("//") {
        return Some("a branch name must not contain \"..\" or \"//\"".to_string());
    }
    if name.ends_with('/') {
        return Some("a branch name must not end with '/'".to_string());
    }
    if name.ends_with(".lock") {
        return Some("a branch name must not end with \".lock\"".to_string());
    }
    match name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')))
    {
        Some(bad) => Some(format!(
            "a branch name holds only letters, digits, '.', '_', '-' and '/', not {bad:?}"
        )),
        None => None,
    }
}

/// Project and lane names: `[a-z][a-z0-9-]*` (conventions 17 §2).
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The Herdr agent name of a seat: `<project>-<seat>` (17 §2/§3). The engine
/// gives a cleared name back with it (T-134) and `agent.start` uses it.
pub fn agent_name(project: &str, seat: &str) -> String {
    format!("{project}-{seat}")
}

/// The absolute path of a repo (T-52): the project's own path joined with the
/// repo's path, which is relative to it. `./` in front and a trailing `/` on
/// the project path are the same path, not two.
fn join_path(base: &str, relative: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = if base.is_empty() { "/" } else { base };
    let relative = relative.trim();
    let relative = relative.trim_start_matches("./");
    let relative = relative.trim_matches('/');
    if relative.is_empty() || relative == "." {
        return base.to_string();
    }
    if base == "/" {
        return format!("/{relative}");
    }
    format!("{base}/{relative}")
}

/// Herdr agent names are at most 32 characters long.
fn agent_name_fits(project: &str, seat: &str) -> bool {
    agent_name(project, seat).len() <= 32
}

/// A model chain problem: no entry may be blank and no entry may repeat
/// (both compared after trimming; an entry is the model id together with the
/// CLI it names). Empty chains are fine.
fn chain_problem(chain: &[ModelEntry]) -> Option<String> {
    let mut seen: Vec<(String, Option<String>)> = Vec::new();
    for entry in chain {
        let model = entry.model();
        if model.is_empty() {
            return Some("a model chain entry is empty after trimming".to_string());
        }
        let key = (model.to_string(), entry.cli().map(str::to_string));
        if seen.contains(&key) {
            return Some(format!("the model {model:?} appears twice in one chain"));
        }
        seen.push(key);
    }
    None
}

/// T-235: the claude CLI takes a bare model id or alias
/// (`claude-sonnet-5-5`), never a `provider/model` one. An entry's CLI is the
/// one it names, else the kind of the seat that runs it (a lane's kind, the
/// temps seat's, the captain's); a chain shared by several seats is checked
/// against every one of their kinds.
fn claude_model_issues(
    field: &str,
    chain: &[ModelEntry],
    kinds: &[&str],
    issues: &mut Vec<ConfigIssue>,
) {
    for entry in chain {
        let claude = match entry.cli() {
            Some(cli) => cli == "claude",
            None => kinds.iter().any(|kind| *kind == "claude"),
        };
        let model = entry.model();
        if claude && model.contains('/') {
            issues.push(issue(
                field,
                format!(
                    "the claude CLI takes a bare model id such as claude-sonnet-5-5, not {model}"
                ),
            ));
        }
    }
}

/// The chain a lane runs on (T-35): its own entries when it names any, else
/// the project's `models.lanes`. The engine starts a seat on the CLI the
/// chain's entry names, so this is also what says which CLIs a lane can run
/// (T-83).
pub fn lane_chain<'a>(config: &'a ProjectConfig, lane: &'a LaneSection) -> &'a [ModelEntry] {
    if lane.models.is_empty() {
        &config.models.lanes
    } else {
        &lane.models
    }
}

/// The primary model of a lane: its own first entry, else the lane chain's.
fn lane_primary<'a>(
    lane: &'a LaneSection,
    lanes_chain: &'a [ModelEntry],
) -> Option<&'a ModelEntry> {
    lane.models.first().or_else(|| lanes_chain.first())
}

/// Two path lists overlap when a path is equal to another or one is its
/// prefix on a `/` boundary: `src/a/` and `src/a/b.rs` overlap,
/// `src/a` and `src/ab` do not. Entries are compared after trimming.
fn owns_overlap(earlier: &[String], later: &[String]) -> bool {
    earlier
        .iter()
        .any(|a| later.iter().any(|b| paths_overlap(a.trim(), b.trim())))
}

fn paths_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if let Some(rest) = b.strip_prefix(a) {
        return rest.starts_with('/') || a.ends_with('/');
    }
    if let Some(rest) = a.strip_prefix(b) {
        return rest.starts_with('/') || b.ends_with('/');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_lane_rule() {
        for good in ["heat", "a", "a-b", "lane-2", "x9"] {
            assert!(is_name(good), "{good}");
        }
        for bad in ["", "Heat", "9harbor", "a b", "a_b", "-a", "heat!"] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn agent_names_are_at_most_32_characters() {
        assert!(agent_name_fits("harbor", "heat"));
        assert!(agent_name_fits("harbor", "captain"));
        assert!(!agent_name_fits(
            "harbor",
            "a-very-long-lane-name-for-harbor"
        ));
    }

    #[test]
    fn paths_overlap_only_on_path_boundaries() {
        assert!(paths_overlap("src/heat/", "src/heat/model.rs"));
        assert!(paths_overlap("src/heat/model.rs", "src/heat/"));
        assert!(paths_overlap("tests/heat/", "tests/heat/"));
        assert!(paths_overlap("src/a", "src/a/b.rs"));
        assert!(!paths_overlap("src/a", "src/ab"));
        assert!(!paths_overlap("src/heat/", "tests/heat/"));
        assert!(!paths_overlap("src/heat/", "src/pricing/"));
    }

    /// A repo's path is relative to the project's own path: the joined path
    /// is the same path however it is written.
    #[test]
    fn a_repo_path_is_joined_to_the_project_path() {
        assert_eq!(join_path("/repo", "crates/api"), "/repo/crates/api");
        assert_eq!(join_path("/repo", "./crates/api"), "/repo/crates/api");
        assert_eq!(join_path("/repo", "crates/api/"), "/repo/crates/api");
        assert_eq!(join_path("/repo/", "crates/api"), "/repo/crates/api");
        assert_eq!(join_path("/repo", ""), "/repo");
        assert_eq!(join_path("/repo", "."), "/repo");
        assert_eq!(join_path("/", "crates/api"), "/crates/api");
    }
}
