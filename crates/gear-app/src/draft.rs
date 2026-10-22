//! Plan draft format (design/05 "Draft format"). Pure: text in, `Draft` out.
//! Strict: any unknown section, field or value rejects the whole draft with
//! `ErrorCode::BadFormat` and a detail starting `line <n>: `. Semantic checks
//! that need the store (deps exist, acyclic, verdict matches the attempt) are
//! `plan apply`'s job, not this parser's.

use gear_model::GearResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Now,
    Must,
    Should,
    Could,
    Wont,
}

/// `## T-31 [now] lane=heat milestone=M2 required` — `id` is None for `## NEW`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    pub line: usize,
    pub id: Option<String>,
    pub priority: Priority,
    pub lane: Option<String>,
    pub milestone: Option<String>,
    pub required: bool,
    pub goal: Option<String>,
    pub accept: Option<String>,
    pub evidence_files: Vec<String>,
    /// minutes
    pub evidence_due: Option<u32>,
    /// `deps: -` → empty
    pub deps: Vec<String>,
    /// only `ready` or `cancelled`
    pub state: Option<String>,
    pub followup_of: Option<String>,
    pub reopen: Option<String>,
}

/// One `# Staff` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaffItem {
    /// `- temp R-4 task=T-45 profile=opencode-review [resume]`
    Temp { line: usize, id: String, task: String, profile: Option<String>, resume: bool },
    /// `- park heat until=<RFC3339>` | `- park media until-task=T-12`
    Park { line: usize, lane: String, until: Option<String>, until_task: Option<String> },
    /// `- hold wire reason=<text>`
    Hold { line: usize, lane: String, reason: String },
    /// `- retire prompts`
    Retire { line: usize, lane: String },
}

/// `- T-29 attempt=2 contract=3 candidate=abc1234 pass evidence=<path>`
/// `- T-33 attempt=1 contract=1 candidate=def5678 fail rework="<text>"`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictLine {
    pub line: usize,
    pub task: String,
    pub attempt: i64,
    pub contract: i64,
    pub candidate: String,
    pub pass: bool,
    /// required for pass, absent for fail
    pub evidence: Option<String>,
    /// required for fail, absent for pass
    pub rework: Option<String>,
}

/// `V-9 keep plan=12: <text>; next=<RFC3339>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLine {
    pub line: usize,
    pub id: String,
    pub plan: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub project: String,
    pub base: i64,
    pub goal: String,
    /// (title line after `# Milestone `, body)
    pub milestones: Vec<(String, String)>,
    pub decisions: Vec<String>,
    /// in rank order
    pub queue: Vec<QueueItem>,
    pub staff: Vec<StaffItem>,
    pub verdicts: Vec<VerdictLine>,
    pub reviews: Vec<ReviewLine>,
}

/// Parse a draft. Header `<!-- gear-plan project=<id> base=<n> -->` on the
/// first non-empty line is required. Sections may be absent; known sections
/// only (`Goal`, `Milestone …`, `Decisions`, `Queue`, `Staff`, `Verdicts`,
/// `Review`). Queue field lines are `<name>: <value>`; a value runs to the end
/// of the line.
pub fn parse(text: &str) -> GearResult<Draft> {
    todo!("T-31")
}
