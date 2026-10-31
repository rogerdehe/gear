//! Plan draft format (design/05 "Draft format"). Pure: text in, `Draft` out.
//! Strict: any unknown section, field or value rejects the whole draft with
//! `ErrorCode::BadFormat` and a detail starting `line <n>: `. Semantic checks
//! that need the store (deps exist, acyclic, verdict matches the attempt) are
//! `plan apply`'s job, not this parser's.

use gear_model::{ErrorCode, GearError, GearResult};
use std::collections::BTreeSet;

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
    /// The voyage station the task belongs to (M5 S3): `station=S2`. The
    /// station must be one the task's milestone block names.
    pub station: Option<String>,
    pub required: bool,
    pub goal: Option<String>,
    pub accept: Option<String>,
    pub evidence_files: Vec<String>,
    /// minutes
    pub evidence_due: Option<u32>,
    /// The planned time of the task in minutes (T-38): `estimate: 90`.
    /// A NEW block must carry it; an existing task may keep the one it has.
    pub estimate: Option<u32>,
    /// `deps: -` → empty
    pub deps: Vec<String>,
    /// only `ready` or `cancelled`
    pub state: Option<String>,
    pub followup_of: Option<String>,
    pub reopen: Option<String>,
    /// The GitHub issue a task came from (T-79): `issue: #12`.
    pub issue: Option<u64>,
    /// The reason the task is mechanical (T-272): `mechanical: delete three
    /// dead fns`. The gate alone proves the change, so the review is skipped.
    pub mechanical: Option<String>,
    /// The reason the task is unrelated to the lane's earlier work (T-481):
    /// `fresh: a new parser`. The lane starts a new conversation instead of
    /// keeping the one it has.
    pub fresh: Option<String>,
}

/// One `# Staff` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaffItem {
    /// `- temp R-4 task=T-45 profile=opencode-review [resume]`
    Temp {
        line: usize,
        id: String,
        task: String,
        profile: Option<String>,
        resume: bool,
    },
    /// `- park heat until=<RFC3339>` | `- park media until-task=T-12`
    Park {
        line: usize,
        lane: String,
        until: Option<String>,
        until_task: Option<String>,
    },
    /// `- hold wire reason=<text>`
    Hold {
        line: usize,
        lane: String,
        reason: String,
    },
    /// `- retire prompts`
    Retire { line: usize, lane: String },
    /// `- retry heat`: a parked (NeedsCheck) or lost (Unknown) lane goes back
    /// to Queued, so the engine onboards it again (T-82).
    Retry { line: usize, lane: String },
    /// `- release heat`: a held lane goes back to Active (its agent continues)
    /// or Queued (its agent is gone, the engine onboards it again) (T-133).
    /// `- release review-1`: a reviewer seat in Standby is released (T-140).
    Release { line: usize, name: String },
    /// `- rebind heat repo=backend owns=[src/, tests/]`.
    Rebind {
        line: usize,
        lane: String,
        repo: String,
        owns: Vec<String>,
    },
    /// `- rebind heat cancel`: withdraw a pending rebind (T-305).
    RebindCancel { line: usize, lane: String },
    /// `- remerge T-<n>`: retry the task's failed merge (T-92).
    Remerge { line: usize, task: String },
    /// `- drop <milestone>`: remove a milestone with no task and no history
    /// (T-473).
    Drop { line: usize, milestone: String },
}

/// `- T-29 attempt=2 contract=3 candidate=abc1234 pass evidence=<path>`
/// `- T-33 attempt=1 contract=1 candidate=def5678 fail rework="<text>"`
/// `- T-34 attempt=1 contract=1 candidate=abc1234 fail rework="<text>" estimate=25`
/// `- T-35 attempt=1 contract=1 candidate=abc1234 pass evidence=<path> why="<text>"`
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
    /// The minutes the fail verdict asks for (design §5.1): the budget of the
    /// rework round. `estimate=25`.
    pub estimate: Option<i64>,
    /// Why the captain passes over a failed review (design §7.4): `why="…"`.
    /// Required for that case.
    pub why: Option<String>,
    /// The task whose merge landed this work (design §10.7, row B03-01):
    /// `integrated-by=T-m`.
    pub integrated_by: Option<String>,
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
    /// (title line after `# Milestone ` or `# Voyage `, body)
    pub milestones: Vec<(String, String)>,
    pub decisions: Vec<String>,
    /// in rank order
    pub queue: Vec<QueueItem>,
    pub staff: Vec<StaffItem>,
    pub verdicts: Vec<VerdictLine>,
    pub reviews: Vec<ReviewLine>,
    /// The user asked for this goal, bar or eta change and confirmed it
    /// (T-410): `gear plan apply --confirmed`. The parser never sets it.
    pub confirmed: bool,
}

/// The id and title of a `# Milestone <id>: <title>` (or `# Voyage <id>:
/// <title>`) block, from the header text (the part after `# Milestone `). The id is the first word without its
/// trailing colon, as [`parse`] reads it.
pub fn milestone_parts(header: &str) -> (&str, &str) {
    let id = header
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(':');
    let title = header[id.len()..].trim_start_matches(':').trim();
    (id, title)
}

/// The fields a milestone block must carry (T-22), read from its body: `done:`
/// (the one checkable sentence), `bar:` (the quality line of this phase) and
/// `eta:` (the planned duration), one line each. Absent ones are None.
pub fn milestone_fields(body: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut done = None;
    let mut bar = None;
    let mut eta = None;
    for line in body.lines() {
        let line = line.trim();
        let (name, value) = match line.split_once(':') {
            Some((name, value)) => (name.trim(), value.trim()),
            None => continue,
        };
        let value = value.to_string();
        match name {
            "done" => done = Some(value),
            "bar" => bar = Some(value),
            "eta" => eta = Some(value),
            _ => {}
        }
    }
    (done, bar, eta)
}

/// The `depart:` line of a milestone block (T-473): `now` is the only value;
/// a timed departure is `[schedule] depart_at` in the config, not a second
/// way. A block without the line does not depart.
pub fn milestone_depart(body: &str) -> Option<String> {
    for line in body.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("depart:") {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// The stations a milestone block names (M5 S3), in plan order: every trimmed
/// line `station <id>: <what reaching it needs>`. The id is non-empty and has
/// no whitespace, the needs are non-empty; a repeated id, an empty id or empty
/// needs rejects the block, naming the line.
pub fn milestone_stations(body: &str) -> Result<Vec<gear_store::VoyageStation>, String> {
    let mut stations: Vec<gear_store::VoyageStation> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("station ") else {
            continue;
        };
        let (id, needs) = match rest.split_once(':') {
            Some((id, needs)) => (id.trim(), needs.trim()),
            None => ("", ""),
        };
        let refused = || format!("station <id>: <what reaching it needs>, got {line:?}");
        if id.is_empty() || id.split_whitespace().count() != 1 || needs.is_empty() {
            return Err(refused());
        }
        if stations.iter().any(|station| station.id == id) {
            return Err(refused());
        }
        stations.push(gear_store::VoyageStation {
            id: id.to_string(),
            needs: needs.to_string(),
        });
    }
    Ok(stations)
}

/// Parse a draft. Header `<!-- gear-plan project=<id> base=<n> -->` on the
/// first non-empty line is required. Sections may be absent; known sections
/// only (`Goal`, `Milestone …`/`Voyage …`, `Decisions`, `Queue`, `Staff`,
/// `Verdicts`, `Review`). Queue field lines are `<name>: <value>`; a value runs to the end
/// of the line.
pub fn parse(text: &str) -> GearResult<Draft> {
    let lines: Vec<&str> = text.lines().collect();

    // The header is the first non-empty line; nothing precedes it.
    let first = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            bad(
                1,
                "the draft is empty; the `<!-- gear-plan … -->` header is missing",
            )
        })?;
    let (project, base) = parse_header(lines[first].trim()).ok_or_else(|| {
        bad(
            first + 1,
            "the first line must be `<!-- gear-plan project=<id> base=<n> -->`",
        )
    })?;

    let mut draft = Draft {
        project,
        base,
        goal: String::new(),
        milestones: Vec::new(),
        decisions: Vec::new(),
        queue: Vec::new(),
        staff: Vec::new(),
        verdicts: Vec::new(),
        reviews: Vec::new(),
        confirmed: false,
    };

    let mut index = first + 1;
    let mut seen_sections: Vec<String> = Vec::new();
    let mut seen_milestones: Vec<String> = Vec::new();
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.is_empty() {
            index += 1;
            continue;
        }
        let line_no = index + 1;
        let section = trimmed.strip_prefix("# ").ok_or_else(|| {
            bad(
                line_no,
                format!("expected a section header, got {trimmed:?}"),
            )
        })?;
        let (name, rest) = match section.split_once(char::is_whitespace) {
            Some((name, rest)) => (name, rest.trim()),
            None => (section, ""),
        };
        match name {
            "Goal" => {
                check_once(&mut seen_sections, "Goal", line_no)?;
                index += 1;
                draft.goal = collect_body(&lines, &mut index);
            }
            "Milestone" | "Voyage" => {
                let id = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(':')
                    .to_string();
                if id.is_empty() {
                    return Err(bad(
                        line_no,
                        "a voyage header needs an id, e.g. `# Voyage M2: …`",
                    ));
                }
                if seen_milestones.contains(&id) {
                    return Err(bad(line_no, format!("milestone {id} appears twice")));
                }
                seen_milestones.push(id);
                index += 1;
                let body = collect_body(&lines, &mut index);
                draft.milestones.push((rest.to_string(), body));
            }
            "Decisions" => {
                check_once(&mut seen_sections, "Decisions", line_no)?;
                index += 1;
                loop {
                    match section_line(&lines, &mut index) {
                        Some((text, line_no)) => {
                            let entry = text.strip_prefix("- ").ok_or_else(|| {
                                bad(
                                    line_no,
                                    format!("a decision line starts with `- `, got {text:?}"),
                                )
                            })?;
                            draft.decisions.push(entry.trim().to_string());
                        }
                        None => break,
                    }
                }
            }
            "Queue" => {
                check_once(&mut seen_sections, "Queue", line_no)?;
                index += 1;
                parse_queue(&lines, &mut index, &mut draft.queue)?;
            }
            "Staff" => {
                check_once(&mut seen_sections, "Staff", line_no)?;
                index += 1;
                loop {
                    match section_line(&lines, &mut index) {
                        Some((text, line_no)) => draft.staff.push(parse_staff(text, line_no)?),
                        None => break,
                    }
                }
            }
            "Verdicts" => {
                check_once(&mut seen_sections, "Verdicts", line_no)?;
                index += 1;
                loop {
                    match section_line(&lines, &mut index) {
                        Some((text, line_no)) => draft.verdicts.push(parse_verdict(text, line_no)?),
                        None => break,
                    }
                }
            }
            "Review" => {
                check_once(&mut seen_sections, "Review", line_no)?;
                index += 1;
                loop {
                    match section_line(&lines, &mut index) {
                        Some((text, line_no)) => draft.reviews.push(parse_review(text, line_no)?),
                        None => break,
                    }
                }
            }
            other => return Err(bad(index + 1, format!("unknown section `# {other}`"))),
        }
    }

    Ok(draft)
}

// ---------------------------------------------------------------- helpers

fn bad(line: usize, detail: impl std::fmt::Display) -> GearError {
    GearError::new(ErrorCode::BadFormat, format!("line {line}: {detail}"))
}

/// `<!-- gear-plan project=<id> base=<n> -->`.
fn parse_header(line: &str) -> Option<(String, i64)> {
    let inner = line.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    let mut tokens = inner.split_whitespace();
    if tokens.next()? != "gear-plan" {
        return None;
    }
    let mut project = None;
    let mut base = None;
    for token in tokens {
        if let Some(value) = token.strip_prefix("project=") {
            if project.is_some() {
                return None;
            }
            project = Some(value.to_string());
        } else {
            let value = token.strip_prefix("base=")?;
            if base.is_some() {
                return None;
            }
            base = Some(value.parse::<i64>().ok()?);
        }
    }
    Some((project?, base?))
}

/// The lines of a free-text section (Goal, Milestone/Voyage), joined and trimmed.
fn collect_body(lines: &[&str], index: &mut usize) -> String {
    let mut body: Vec<&str> = Vec::new();
    while *index < lines.len() && !is_section(lines[*index]) {
        let line = lines[*index].trim();
        if !line.is_empty() {
            body.push(line);
        }
        *index += 1;
    }
    body.join("\n").trim().to_string()
}

/// The next non-empty line of the current section with its 1-based line
/// number, if the section has not ended; `*index` then points at the next
/// line to read.
fn section_line<'a>(lines: &'a [&'a str], index: &mut usize) -> Option<(&'a str, usize)> {
    while *index < lines.len() {
        if is_section(lines[*index]) {
            return None;
        }
        let line_no = *index + 1;
        let line = lines[*index].trim();
        *index += 1;
        if !line.is_empty() {
            return Some((line, line_no));
        }
    }
    None
}

fn is_section(line: &str) -> bool {
    line.trim().starts_with("# ")
}

/// A known section may appear only once in a draft.
fn check_once(seen: &mut Vec<String>, name: &str, line: usize) -> GearResult<()> {
    if seen.iter().any(|known| known == name) {
        return Err(bad(line, format!("the `# {name}` section appears twice")));
    }
    seen.push(name.to_string());
    Ok(())
}

fn parse_priority(token: &str) -> Option<Priority> {
    match token.strip_prefix('[')?.strip_suffix(']')? {
        "now" => Some(Priority::Now),
        "must" => Some(Priority::Must),
        "should" => Some(Priority::Should),
        "could" => Some(Priority::Could),
        "wont" => Some(Priority::Wont),
        _ => None,
    }
}

/// An issue number as `#<n>`: ASCII digits, positive, and small enough for
/// the store's integer (T-79). `#0`, `#+12` and a number that does not fit
/// are not issue numbers.
fn parse_issue(value: &str) -> Option<u64> {
    let digits = value.strip_prefix('#')?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let number: u64 = digits.parse().ok()?;
    (1..=i64::MAX as u64).contains(&number).then_some(number)
}

impl Priority {
    /// The plan's word for a priority: `now`, `must`, …
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::Now => "now",
            Priority::Must => "must",
            Priority::Should => "should",
            Priority::Could => "could",
            Priority::Wont => "wont",
        }
    }

    /// The priority a GitHub label names (T-79): `priority/now`,
    /// `priority/must`, … `None` for any other label.
    pub fn from_label(label: &str) -> Option<Priority> {
        match label.trim().strip_prefix("priority/")? {
            "now" => Some(Priority::Now),
            "must" => Some(Priority::Must),
            "should" => Some(Priority::Should),
            "could" => Some(Priority::Could),
            "wont" => Some(Priority::Wont),
            _ => None,
        }
    }

    /// The label a priority is written as (the inverse of `from_label`).
    pub fn label(self) -> String {
        format!("priority/{}", self.as_str())
    }
}

fn is_task_id(token: &str) -> bool {
    match token.strip_prefix("T-") {
        Some(number) => !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

fn parse_queue(lines: &[&str], index: &mut usize, queue: &mut Vec<QueueItem>) -> GearResult<()> {
    let mut current: Option<QueueItem> = None;
    let mut seen: Vec<&'static str> = Vec::new();
    loop {
        let (text, line_no) = match section_line(lines, index) {
            Some(found) => found,
            None => break,
        };
        if let Some(header) = text.strip_prefix("## ") {
            if let Some(item) = current.take() {
                queue.push(item);
            }
            seen.clear();
            current = Some(parse_block_header(header, line_no)?);
            continue;
        }
        let item = current
            .as_mut()
            .ok_or_else(|| bad(line_no, "a queue field appears before any queue block"))?;
        parse_queue_field(item, text, line_no, &mut seen)?;
    }
    if let Some(item) = current.take() {
        queue.push(item);
    }
    Ok(())
}

/// `## T-31 [now] lane=heat milestone=M2 required` or `## NEW [should] lane=heat`.
fn parse_block_header(header: &str, line: usize) -> GearResult<QueueItem> {
    let mut tokens = header.split_whitespace();
    let id = tokens
        .next()
        .ok_or_else(|| bad(line, "a queue block header needs `T-<n>` or `NEW`"))?;
    let id = if id == "NEW" {
        None
    } else if is_task_id(id) {
        Some(id.to_string())
    } else {
        return Err(bad(
            line,
            format!("a queue block header needs `T-<n>` or `NEW`, got {id:?}"),
        ));
    };
    let priority_token = tokens
        .next()
        .ok_or_else(|| bad(line, "a queue block header needs a priority in brackets"))?;
    let priority = parse_priority(priority_token).ok_or_else(|| {
        bad(
            line,
            format!("unknown priority {priority_token:?}, expected [now|must|should|could|wont]"),
        )
    })?;
    let mut lane = None;
    let mut milestone = None;
    let mut station = None;
    let mut required = false;
    for token in tokens {
        if token == "required" {
            if required {
                return Err(bad(line, "repeated `required` in one queue block header"));
            }
            required = true;
        } else if let Some(value) = token.strip_prefix("lane=") {
            if lane.replace(value.to_string()).is_some() {
                return Err(bad(line, "repeated `lane=` in one queue block header"));
            }
        } else if let Some(value) = token.strip_prefix("milestone=") {
            if milestone.replace(value.to_string()).is_some() {
                return Err(bad(line, "repeated `milestone=` in one queue block header"));
            }
        } else if let Some(value) = token.strip_prefix("station=") {
            if value.is_empty() {
                return Err(bad(line, "`station=` needs a station id"));
            }
            if station.replace(value.to_string()).is_some() {
                return Err(bad(line, "repeated `station=` in one queue block header"));
            }
        } else {
            return Err(bad(
                line,
                format!("unknown queue block header field {token:?}"),
            ));
        }
    }
    Ok(QueueItem {
        line,
        id,
        priority,
        lane,
        milestone,
        station,
        required,
        goal: None,
        accept: None,
        evidence_files: Vec::new(),
        evidence_due: None,
        estimate: None,
        deps: Vec::new(),
        state: None,
        followup_of: None,
        reopen: None,
        issue: None,
        mechanical: None,
        fresh: None,
    })
}

/// One `<name>: <value>` line inside a queue block.
fn parse_queue_field(
    item: &mut QueueItem,
    text: &str,
    line: usize,
    seen: &mut Vec<&'static str>,
) -> GearResult<()> {
    let (name, value) = text.split_once(':').ok_or_else(|| {
        bad(
            line,
            format!("a queue field is `<name>: <value>`, got {text:?}"),
        )
    })?;
    let name = name.trim();
    let value = value.trim();
    let field: &'static str = match name {
        "goal" => "goal",
        "accept" => "accept",
        "evidence-files" => "evidence-files",
        "evidence-due" => "evidence-due",
        "estimate" => "estimate",
        "deps" => "deps",
        "state" => "state",
        "followup-of" => "followup-of",
        "reopen" => "reopen",
        "issue" => "issue",
        "mechanical" => "mechanical",
        "fresh" => "fresh",
        other => return Err(bad(line, format!("unknown queue field {other:?}"))),
    };
    if seen.contains(&field) {
        return Err(bad(
            line,
            format!("repeated queue field `{field}` in one block"),
        ));
    }
    seen.push(field);
    match field {
        "goal" => item.goal = Some(value.to_string()),
        "accept" => item.accept = Some(value.to_string()),
        "evidence-files" => item.evidence_files = comma_list(value),
        "evidence-due" => {
            item.evidence_due = Some(value.parse::<u32>().map_err(|_| {
                bad(
                    line,
                    format!("`evidence-due` is a number of minutes, got {value:?}"),
                )
            })?)
        }
        "estimate" => {
            item.estimate = Some(value.parse::<u32>().map_err(|_| {
                bad(
                    line,
                    format!("`estimate` is a number of minutes, got {value:?}"),
                )
            })?)
        }
        "deps" => {
            item.deps = if value == "-" {
                Vec::new()
            } else {
                comma_list(value)
            }
        }
        "state" => {
            if value != "ready" && value != "hold" && value != "cancelled" {
                return Err(bad(
                    line,
                    format!("`state` is `ready`, `hold` or `cancelled`, got {value:?}"),
                ));
            }
            item.state = Some(value.to_string());
        }
        "followup-of" => {
            if !is_task_id(value) {
                return Err(bad(
                    line,
                    format!("`followup-of` needs a task id, got {value:?}"),
                ));
            }
            item.followup_of = Some(value.to_string());
        }
        "reopen" => item.reopen = Some(value.to_string()),
        "issue" => {
            item.issue = Some(parse_issue(value).ok_or_else(|| {
                bad(
                    line,
                    format!("`issue` is a GitHub issue number as `#<n>`, got {value:?}"),
                )
            })?)
        }
        "mechanical" => {
            if value.is_empty() {
                return Err(bad(
                    line,
                    "`mechanical` needs the reason the gate alone proves this change",
                ));
            }
            item.mechanical = Some(value.to_string());
        }
        "fresh" => {
            if value.is_empty() {
                return Err(bad(
                    line,
                    "`fresh` needs the reason the task is unrelated to the lane's earlier work",
                ));
            }
            item.fresh = Some(value.to_string());
        }
        _ => unreachable!("the field was matched above"),
    }
    Ok(())
}

fn comma_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_string())
        .collect()
}

/// `- temp …`, `- park …`, `- hold …`, `- retire …`, `- remerge T-<n>`.
fn parse_staff(text: &str, line: usize) -> GearResult<StaffItem> {
    let rest = text
        .strip_prefix("- ")
        .ok_or_else(|| bad(line, format!("a staff line starts with `- `, got {text:?}")))?;
    let mut tokens = rest.split_whitespace();
    let verb = tokens
        .next()
        .ok_or_else(|| bad(line, "a staff line needs a verb"))?;
    match verb {
        "temp" => {
            let id = tokens
                .next()
                .ok_or_else(|| bad(line, "`temp` needs a worker id such as `R-4`"))?;
            let mut task = None;
            let mut profile = None;
            let mut resume = false;
            for token in tokens {
                if token == "resume" {
                    if resume {
                        return Err(bad(line, "repeated `resume` in one `temp` line"));
                    }
                    resume = true;
                } else if let Some(value) = token.strip_prefix("task=") {
                    if !is_task_id(value) {
                        return Err(bad(line, format!("`task=` needs a task id, got {value:?}")));
                    }
                    if task.replace(value.to_string()).is_some() {
                        return Err(bad(line, "repeated `task=` in one `temp` line"));
                    }
                } else if let Some(value) = token.strip_prefix("profile=") {
                    if profile.replace(value.to_string()).is_some() {
                        return Err(bad(line, "repeated `profile=` in one `temp` line"));
                    }
                } else {
                    return Err(bad(line, format!("unknown `temp` field {token:?}")));
                }
            }
            let task = task.ok_or_else(|| bad(line, "`temp` needs `task=`"))?;
            Ok(StaffItem::Temp { line, id: id.to_string(), task, profile, resume })
        }
        "park" => {
            let lane = tokens
                .next()
                .ok_or_else(|| bad(line, "`park` needs a lane"))?;
            let mut until = None;
            let mut until_task = None;
            for token in tokens {
                if let Some(value) = token.strip_prefix("until=") {
                    if until.is_some() || until_task.is_some() {
                        return Err(bad(line, "`park` takes exactly one of `until=` and `until-task=`"));
                    }
                    until = Some(value.to_string());
                } else if let Some(value) = token.strip_prefix("until-task=") {
                    if !is_task_id(value) {
                        return Err(bad(line, format!("`until-task=` needs a task id, got {value:?}")));
                    }
                    if until.is_some() || until_task.is_some() {
                        return Err(bad(line, "`park` takes exactly one of `until=` and `until-task=`"));
                    }
                    until_task = Some(value.to_string());
                } else {
                    return Err(bad(line, format!("unknown `park` field {token:?}")));
                }
            }
            if until.is_none() && until_task.is_none() {
                return Err(bad(line, "`park` needs `until=` or `until-task=`"));
            }
            Ok(StaffItem::Park { line, lane: lane.to_string(), until, until_task })
        }
        "hold" => {
            // `hold <lane> reason=<text to the end of the line>`.
            let after = rest["hold".len()..].trim_start();
            let (lane, tail) = after
                .split_once(char::is_whitespace)
                .ok_or_else(|| bad(line, "`hold` needs a lane and `reason=`"))?;
            let reason = tail
                .trim_start()
                .strip_prefix("reason=")
                .ok_or_else(|| bad(line, "`hold` needs `reason=<text>`"))?;
            if reason.trim().is_empty() {
                return Err(bad(line, "`hold` needs a non-empty `reason=`"));
            }
            Ok(StaffItem::Hold {
                line,
                lane: lane.to_string(),
                reason: reason.trim().to_string(),
            })
        }
        "retire" => {
            let lane = tokens
                .next()
                .ok_or_else(|| bad(line, "`retire` needs a lane"))?;
            if tokens.next().is_some() {
                return Err(bad(line, "`retire` takes only a lane"));
            }
            Ok(StaffItem::Retire { line, lane: lane.to_string() })
        }
        "retry" => {
            let lane = tokens
                .next()
                .ok_or_else(|| bad(line, "`retry` needs a lane"))?;
            if tokens.next().is_some() {
                return Err(bad(line, "`retry` takes only a lane"));
            }
            Ok(StaffItem::Retry { line, lane: lane.to_string() })
        }
        "release" => {
            let name = tokens
                .next()
                .ok_or_else(|| bad(line, "`release` needs a lane or a reviewer"))?;
            if tokens.next().is_some() {
                return Err(bad(line, "`release` takes only a lane or a reviewer"));
            }
            Ok(StaffItem::Release { line, name: name.to_string() })
        }
        "rebind" => {
            let lane = tokens.next().ok_or_else(|| bad(line, "`rebind` needs a lane"))?;
            // `- rebind <lane> cancel` withdraws a pending request (T-305).
            let rest = tokens.clone().collect::<Vec<_>>();
            if rest.first() == Some(&"cancel") {
                tokens.next();
                if tokens.next().is_some() {
                    return Err(bad(line, "`rebind ... cancel` takes only a lane"));
                }
                return Ok(StaffItem::RebindCancel { line, lane: lane.into() });
            }
            let repo = tokens.next().and_then(|s| s.strip_prefix("repo="))
                .filter(|s| !s.is_empty()).ok_or_else(|| bad(line, "`rebind` needs `repo=<name>`"))?;
            let remaining = tokens.collect::<Vec<_>>().join(" ");
            let list = remaining.strip_prefix("owns=[").and_then(|s| s.strip_suffix(']'))
                .ok_or_else(|| bad(line, "`rebind` needs `owns=[<path>, ...]`"))?;
            let owns: Vec<String> = list.split(',').map(|s| s.trim().to_string()).collect();
            if owns.iter().any(|s| s.is_empty() || s.chars().any(char::is_whitespace) || s.contains(['[', ']'])) {
                return Err(bad(line, "`rebind` needs a nonempty comma-separated list of paths"));
            }
            Ok(StaffItem::Rebind { line, lane: lane.into(), repo: repo.into(), owns })
        }
        "remerge" => {
            let task = tokens
                .next()
                .ok_or_else(|| bad(line, "`remerge` needs a task id"))?;
            if !is_task_id(task) {
                return Err(bad(line, format!("`remerge` needs a task id, got {task:?}")));
            }
            if tokens.next().is_some() {
                return Err(bad(line, "`remerge` takes only a task id"));
            }
            Ok(StaffItem::Remerge { line, task: task.to_string() })
        }
        "drop" => {
            let milestone = tokens
                .next()
                .ok_or_else(|| bad(line, "`drop` needs a milestone id"))?;
            if milestone.is_empty() || milestone.chars().any(char::is_whitespace) {
                return Err(bad(
                    line,
                    format!("`drop` needs one milestone id, got {milestone:?}"),
                ));
            }
            if tokens.next().is_some() {
                return Err(bad(line, "`drop` takes only a milestone id"));
            }
            Ok(StaffItem::Drop { line, milestone: milestone.to_string() })
        }
        other => Err(bad(
            line,
            format!("unknown staff verb {other:?}, expected temp, park, hold, release, retire, retry, rebind, remerge or drop"),
        )),
    }
}

/// `- T-29 attempt=2 contract=3 candidate=abc1234 pass evidence=<path>`
fn parse_verdict(text: &str, line: usize) -> GearResult<VerdictLine> {
    let rest = text.strip_prefix("- ").ok_or_else(|| {
        bad(
            line,
            format!("a verdict line starts with `- `, got {text:?}"),
        )
    })?;
    let mut tokens = rest.split_whitespace();
    let task = tokens
        .next()
        .ok_or_else(|| bad(line, "a verdict line needs a task id"))?;
    if !is_task_id(task) {
        return Err(bad(
            line,
            format!("a verdict line needs a task id, got {task:?}"),
        ));
    }
    let attempt = number_field(tokens.next(), "attempt", line)?;
    let contract = number_field(tokens.next(), "contract", line)?;
    let candidate = tokens
        .next()
        .and_then(|token| token.strip_prefix("candidate="))
        .ok_or_else(|| bad(line, "a verdict line needs `candidate=<commit>`"))?;
    if candidate.is_empty() {
        return Err(bad(line, "a verdict line needs a non-empty `candidate=`"));
    }
    let outcome = tokens
        .next()
        .ok_or_else(|| bad(line, "a verdict line ends with `pass …` or `fail …`"))?;
    let pass = match outcome {
        "pass" => true,
        "fail" => false,
        other => {
            return Err(bad(
                line,
                format!("a verdict is `pass` or `fail`, got {other:?}"),
            ))
        }
    };

    // The tail runs to the end of the line: `evidence=<path>`, `estimate=<min>`,
    // `integrated-by=T-m` and the quoted value (`rework="<text>"` or
    // `why="<text>"`).
    let tail = rest
        .splitn(6, char::is_whitespace)
        .nth(5)
        .unwrap_or("")
        .trim();
    let (fields, quoted) = parse_verdict_tail(tail, line)?;
    let mut evidence = None;
    let mut rework = None;
    let mut estimate = None;
    let mut why = None;
    let mut integrated_by = None;
    for (key, value) in fields {
        match key.as_str() {
            "evidence" => {
                let value = value.trim();
                if value.is_empty() {
                    return Err(bad(line, "a `pass` verdict needs a non-empty `evidence=`"));
                }
                evidence = Some(value.to_string());
            }
            "estimate" => {
                let minutes = value.trim().parse::<i64>().map_err(|_| {
                    bad(
                        line,
                        format!("`estimate=` takes a number of minutes, got {value:?}"),
                    )
                })?;
                estimate = Some(minutes);
            }
            "integrated-by" => {
                let task = value.trim();
                if !is_task_id(task) {
                    return Err(bad(
                        line,
                        format!("`integrated-by=` takes a task id, got {task:?}"),
                    ));
                }
                integrated_by = Some(task.to_string());
            }
            other => {
                return Err(bad(line, format!("unexpected verdict field {other:?}")));
            }
        }
    }
    // The quoted value: the rework text or the why.
    if let Some((key, value)) = quoted {
        match key.as_str() {
            "rework" => {
                if value.trim().is_empty() {
                    return Err(bad(
                        line,
                        "a `fail` verdict needs a non-empty rework reason",
                    ));
                }
                rework = Some(value.trim().to_string());
            }
            "why" => {
                if value.trim().is_empty() {
                    return Err(bad(line, "`why=\"<text>\"` takes a non-empty text"));
                }
                why = Some(value.trim().to_string());
            }
            other => {
                return Err(bad(line, format!("unexpected verdict field {other:?}")));
            }
        }
    }

    // The fields that belong to the other verdict are refused first: they say
    // what the captain meant to write.
    if !pass && why.is_some() {
        return Err(bad(
            line,
            "`why=` belongs to a pass over a failed review: a fail carries its rework text",
        ));
    }
    if !pass && integrated_by.is_some() {
        return Err(bad(
            line,
            "`integrated-by=` belongs to a pass: only a pass can be landed by another task's merge",
        ));
    }
    if pass && estimate.is_some() {
        return Err(bad(
            line,
            "a `pass` verdict must not carry `estimate=`: the minutes a rework asks for belong to a fail",
        ));
    }
    if pass && evidence.is_none() {
        return Err(bad(line, "a `pass` verdict needs `evidence=<path>`"));
    }
    if pass && rework.is_some() {
        return Err(bad(line, "a `pass` verdict must not carry `rework=`"));
    }
    if !pass && rework.is_none() {
        return Err(bad(line, "a `fail` verdict needs `rework=\"<text>\"`"));
    }
    if !pass && evidence.is_some() {
        return Err(bad(line, "a `fail` verdict must not carry `evidence=`"));
    }
    if !pass && estimate.map(|min| min <= 0).unwrap_or(false) {
        return Err(bad(line, "`estimate=` takes a positive number of minutes"));
    }

    Ok(VerdictLine {
        line,
        task: task.to_string(),
        attempt,
        contract,
        candidate: candidate.to_string(),
        pass,
        evidence,
        rework,
        estimate,
        why,
        integrated_by,
    })
}

/// The tail of a verdict line: the `key=value` fields and the one quoted
/// value (the rework text or the why).
///
/// The captain's rule (design §10.7, T-141's rework): the tail carries at
/// most one quoted value, and it is the **last** field — it runs from its
/// first quote to its closing quote, so the prose may contain quotes of its
/// own. The `key=value` fields (`estimate=`, `integrated-by=`, `evidence=`)
/// come before it; one after it is refused, and a value without a closing
/// quote is refused too.
fn parse_verdict_tail(
    tail: &str,
    line: usize,
) -> GearResult<(Vec<(String, String)>, Option<(String, String)>)> {
    let tail = tail.trim();
    let Some(open) = tail.find('"') else {
        // No quoted value: the tail is `key=value` fields.
        return Ok((key_value_fields(tail, line)?, None));
    };
    // The quoted value belongs to the last key of the head: its value starts
    // at the quote.
    let mut fields = key_value_fields(tail[..open].trim_end(), line)?;
    let quoted_key = match fields.last() {
        Some((key, value)) if value.is_empty() => {
            let key = key.clone();
            fields.pop();
            key
        }
        _ => {
            return Err(bad(
                line,
                "a verdict line's quoted value needs its key: `rework=\"…\"` or `why=\"…\"`",
            ))
        }
    };
    // The closing quote: the first quote that ends the value. The prose
    // between the quotes may contain quotes.
    let rest = &tail[open + 1..];
    let closing = find_closing_quote(rest, line)?;
    let value = rest[..closing].to_string();
    let after = rest[closing + 1..].trim();
    if !after.is_empty() {
        // The order is the captain's rule: the quoted value is the last field.
        return Err(bad(
            line,
            format!("a verdict line's quoted value is the last field: {after:?} comes after it"),
        ));
    }
    Ok((fields, Some((quoted_key, value))))
}

/// The `key=value` fields of a verdict line's tail.
fn key_value_fields(head: &str, line: usize) -> GearResult<Vec<(String, String)>> {
    let mut fields = Vec::new();
    for token in head.split(char::is_whitespace) {
        if token.is_empty() {
            continue;
        }
        let Some((key, value)) = token.split_once('=') else {
            return Err(bad(line, format!("unexpected verdict text {token:?}")));
        };
        if key.is_empty() {
            return Err(bad(line, format!("unexpected verdict text {token:?}")));
        }
        fields.push((key.to_string(), value.to_string()));
    }
    Ok(fields)
}

/// The closing quote of a quoted value: the **last** quote on the line (the
/// captain's rule). The prose between the quotes may hold quotes of its own
/// (T-141, round 2, F1): a literal `estimate=25` inside the text is text, not
/// the start of a field.
///
/// A line without a closing quote is refused.
fn find_closing_quote(rest: &str, line: usize) -> GearResult<usize> {
    rest.rfind('"')
        .ok_or_else(|| bad(line, "a verdict line's quoted value has no closing quote"))
}

fn number_field(token: Option<&str>, name: &str, line: usize) -> GearResult<i64> {
    let token = token.ok_or_else(|| bad(line, format!("a verdict line needs `{name}=<n>`")))?;
    let value = token.strip_prefix(&format!("{name}=")).ok_or_else(|| {
        bad(
            line,
            format!("a verdict line needs `{name}=<n>`, got {token:?}"),
        )
    })?;
    value
        .parse::<i64>()
        .map_err(|_| bad(line, format!("`{name}=` takes a number, got {value:?}")))
}

/// `V-9 keep plan=12: <text>` (design/11: `keep` or `change`).
fn parse_review(text: &str, line: usize) -> GearResult<ReviewLine> {
    let (head, rest) = text
        .split_once(':')
        .ok_or_else(|| bad(line, "a review line is `<id> keep|change plan=<n>: <text>`"))?;
    let mut tokens = head.split_whitespace();
    let id = tokens
        .next()
        .ok_or_else(|| bad(line, "a review line needs a review id such as `V-9`"))?;
    let decision = tokens
        .next()
        .ok_or_else(|| bad(line, "a review line needs `keep` or `change`"))?;
    if decision != "keep" && decision != "change" {
        return Err(bad(
            line,
            format!("a review decision is `keep` or `change`, got {decision:?}"),
        ));
    }
    let plan = tokens
        .next()
        .and_then(|token| token.strip_prefix("plan="))
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| bad(line, "a review line needs `plan=<n>`"))?;
    if tokens.next().is_some() {
        return Err(bad(line, "unexpected text before the `:` of a review line"));
    }
    Ok(ReviewLine {
        line,
        id: id.to_string(),
        plan,
        text: rest.trim().to_string(),
    })
}

/// Rewrite a draft after a successful apply, so the file stays the current
/// plan of its voyage (T-34):
///
/// - the header's `base=` becomes the applied version;
/// - every `## NEW` block header becomes `## T-<id>`, in draft order (the ids
///   the apply assigned);
/// - the `# Verdicts` section is dropped: verdicts live in the store, and a
///   re-applied file must settle nothing;
/// - the `# Staff` lines the apply used are dropped (B5): a staff line is an
///   instruction, and an instruction that was carried out must not be carried
///   out again (a `- remerge T-<n>` line would be refused the second time).
///   A `# Staff` section that keeps no line is dropped with them: an empty
///   section is noise, and a second one would be refused by the parser.
///
/// Everything else — comments, sections, field lines, spacing — is kept as the
/// captain wrote it, so the file is edited in place and re-applied as is.
///
/// `blocks` are the `# Milestone` blocks the apply wrote back from the store
/// (T-333): each goes in with an empty line after it, before the first section
/// that is neither `# Goal` nor a milestone block — the file keeps the goal and
/// the voyages first, and the rest of the plan follows them. A file with no such
/// section gets them at its end, after an empty line. With no block the text is
/// rewritten as before.
pub fn rewrite_applied(
    text: &str,
    version: i64,
    created: &[String],
    used_staff: &[usize],
    blocks: &[String],
) -> String {
    let source: Vec<&str> = text.lines().collect();
    // The staff lines the apply used, by their number in the text.
    let used: BTreeSet<usize> = used_staff.iter().copied().collect();
    // The `# Staff` section, and whether it keeps a line of its own.
    let staff_header = source
        .iter()
        .position(|line| is_section_named(line.trim(), "Staff"));
    let staff_empty = staff_header.is_some_and(|header| {
        source
            .iter()
            .enumerate()
            .skip(header + 1)
            .take_while(|(_, line)| !line.trim().starts_with("# "))
            .filter(|(_, line)| !line.trim().is_empty())
            .all(|(index, _)| used.contains(&(index + 1)))
    });
    let mut lines: Vec<String> = Vec::new();
    let mut ids = created.iter();
    let mut dropping = false;
    for (index, line) in source.iter().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim();
        if dropping {
            // The section ends at the next `# ` header (which is kept) or at
            // the end of the file.
            if trimmed.starts_with("# ") {
                dropping = false;
            } else {
                continue;
            }
        }
        if is_section_named(trimmed, "Verdicts") {
            dropping = true;
            continue;
        }
        // The staff lines the apply used are gone (B5), and a section that
        // keeps nothing goes with them.
        if used.contains(&line_no) {
            continue;
        }
        if staff_empty && staff_header == Some(index) {
            dropping = true;
            continue;
        }
        if trimmed
            .strip_prefix("## ")
            .and_then(|rest| rest.split_whitespace().next())
            == Some("NEW")
        {
            if let Some(id) = ids.next() {
                let indent = &line[..line.len() - line.trim_start().len()];
                let rest = trimmed["## NEW".len()..].trim_start();
                let header = if rest.is_empty() {
                    format!("{indent}## {id}")
                } else {
                    format!("{indent}## {id} {rest}")
                };
                lines.push(header);
                continue;
            }
        }
        lines.push(line.to_string());
    }
    // The dropped section may have been the last one: do not leave the blank
    // lines that separated it behind.
    if dropping {
        while lines
            .last()
            .map(|line| line.trim().is_empty())
            .unwrap_or(false)
        {
            lines.pop();
        }
    }
    // The header is the first non-empty line; its `base=` is the version just
    // applied.
    if let Some(header) = lines.iter_mut().find(|line| !line.trim().is_empty()) {
        let trimmed = header.trim();
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            let parts: Vec<String> = trimmed
                .split_whitespace()
                .map(|token| {
                    if token.starts_with("base=") {
                        format!("base={version}")
                    } else {
                        token.to_string()
                    }
                })
                .collect();
            *header = parts.join(" ");
        }
    }
    // The blocks the store had to write back (T-333).
    if !blocks.is_empty() {
        let first_section = lines.iter().position(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("# ")
                && !is_section_named(trimmed, "Goal")
                && !trimmed.starts_with("# Milestone ")
                && !trimmed.starts_with("# Voyage ")
        });
        match first_section {
            Some(at) => {
                let mut written: Vec<String> = Vec::new();
                for block in blocks {
                    written.extend(block.lines().map(str::to_string));
                    written.push(String::new());
                }
                lines.splice(at..at, written);
            }
            None => {
                // No later section: the blocks close the file, after an empty
                // line.
                if lines
                    .last()
                    .map(|line| !line.trim().is_empty())
                    .unwrap_or(false)
                {
                    lines.push(String::new());
                }
                for block in blocks {
                    lines.extend(block.lines().map(str::to_string));
                }
            }
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Is this line the `# <name>` section header (not `# <name>s`, not a
/// value line)?
fn is_section_named(line: &str, name: &str) -> bool {
    let Some(rest) = line.strip_prefix("# ") else {
        return false;
    };
    rest == name || rest.starts_with(&format!("{name} "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "<!-- gear-plan project=p base=0 -->\n";

    fn detail_of(text: &str) -> String {
        let err = parse(text).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat);
        err.detail
    }

    #[test]
    fn an_applied_draft_is_rewritten_in_place() {
        let text = format!(
            "{HEADER}# Goal\nthe goal\n\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: A\naccept: a\ndeps: -\n\n## NEW [should] lane=heat milestone=M1\ngoal: B\naccept: b\ndeps: T-1\n\n# Verdicts\n- T-9 attempt=1 contract=1 candidate=abc pass evidence=x\n\n# Staff\n- hold heat reason=test\n"
        );
        let rewritten =
            rewrite_applied(&text, 7, &["T-1".to_string(), "T-2".to_string()], &[], &[]);
        assert!(
            rewritten.starts_with("<!-- gear-plan project=p base=7 -->"),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("## T-1 [must] lane=heat milestone=M1"),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("## T-2 [should] lane=heat milestone=M1"),
            "{rewritten}"
        );
        assert!(!rewritten.contains("NEW"), "{rewritten}");
        assert!(!rewritten.contains("# Verdicts"), "{rewritten}");
        assert!(!rewritten.contains("T-9 attempt"), "{rewritten}");
        // Everything else is untouched, including the sections after the
        // dropped one.
        assert!(rewritten.contains("# Goal\nthe goal"), "{rewritten}");
        assert!(
            rewritten.contains("goal: A\naccept: a\ndeps: -"),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("# Staff\n- hold heat reason=test"),
            "{rewritten}"
        );
        // And the result still parses as a draft with those ids.
        let again = parse(&rewritten).unwrap();
        assert_eq!(again.base, 7);
        assert_eq!(
            again
                .queue
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>(),
            vec![Some("T-1".into()), Some("T-2".into())]
        );
        assert!(again.verdicts.is_empty(), "the verdicts are gone");
    }

    #[test]
    fn the_staff_lines_the_apply_used_are_gone() {
        // B5: a staff line is an instruction, and an instruction that was
        // carried out must not be carried out again (a `- remerge T-<n>` line
        // would be refused the second time).
        let text = format!(
            "{HEADER}# Staff\n- remerge T-12\n- hold heat reason=test\n\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n"
        );
        // The remerge line was used (line 3), the hold line was not.
        let rewritten = rewrite_applied(&text, 2, &[], &[3], &[]);
        assert!(!rewritten.contains("remerge"), "{rewritten}");
        assert!(rewritten.contains("- hold heat reason=test"), "{rewritten}");
        assert!(
            rewritten.contains("# Staff"),
            "the section keeps a line: {rewritten}"
        );

        // A section that keeps nothing goes with its lines: no empty section
        // and no second one.
        let rewritten = rewrite_applied(&text, 2, &[], &[3, 4], &[]);
        assert!(!rewritten.contains("# Staff"), "{rewritten}");
        assert!(!rewritten.contains("remerge"), "{rewritten}");
        assert!(!rewritten.contains("hold heat"), "{rewritten}");
        // The file still parses as a draft.
        parse(&rewritten).unwrap();
    }

    #[test]
    fn a_verdicts_section_at_the_end_leaves_no_tail() {
        let text = format!("{HEADER}# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n\n# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc fail rework=\"x\"\n");
        let rewritten = rewrite_applied(&text, 2, &["T-1".to_string()], &[], &[]);
        assert!(rewritten.ends_with("deps: -\n"), "{rewritten:?}");
        assert!(!rewritten.contains("Verdicts"), "{rewritten}");
    }

    #[test]
    fn a_written_back_milestone_block_goes_before_the_first_later_section() {
        // T-333: the file carries the `# Milestone` block of every milestone its
        // tasks name, and a block the store wrote back sits with the goal and
        // the voyages, before the rest of the plan.
        let text = format!(
            "{HEADER}# Goal\nthe goal\n\n# Milestone M1: one\ndone: m1\nbar: b1\neta: 1h\n\n# Queue\n## NEW [must] lane=heat milestone=M2\ngoal: A\naccept: a\ndeps: -\n\n# Decisions\n- a note\n"
        );
        let block = "# Milestone M2: two\ndone: m2\nbar: b2\neta: 2h\nstation S1: the first step needs it\n";
        let rewritten = rewrite_applied(&text, 3, &["T-1".to_string()], &[], &[block.to_string()]);
        let os = rewritten
            .find("# Milestone M2: two")
            .expect("the block is there");
        let first = rewritten
            .find("# Milestone M1: one")
            .expect("the M1 block is there");
        let queue = rewritten.find("# Queue").expect("the queue is there");
        let decisions = rewritten
            .find("# Decisions")
            .expect("the decisions are there");
        assert!(
            first < os,
            "the block joins the goal and the voyages: {rewritten}"
        );
        assert!(
            os < queue,
            "the block goes before the first later section: {rewritten}"
        );
        assert!(queue < decisions, "{rewritten}");
        assert!(
            rewritten.contains("station S1: the first step needs it\n\n# Queue"),
            "the block keeps its station lines and an empty line after it: {rewritten}"
        );
        // The file still parses, and M2 is one of its milestones.
        let again = parse(&rewritten).unwrap();
        assert_eq!(again.milestones.len(), 2, "{rewritten}");
    }

    #[test]
    fn a_written_back_block_closes_a_file_with_no_later_section() {
        // T-333: with no section after the goal and the milestones, the block
        // is appended at the end, after an empty line.
        let text = format!(
            "{HEADER}# Goal\nthe goal\n\n# Milestone M1: one\ndone: m1\nbar: b1\neta: 1h\n"
        );
        let block = "# Milestone M2: two\ndone: m2\nbar: b2\neta: 2h\n";
        let rewritten = rewrite_applied(&text, 2, &[], &[], &[block.to_string()]);
        assert!(
            rewritten.ends_with("eta: 1h\n\n# Milestone M2: two\ndone: m2\nbar: b2\neta: 2h\n"),
            "{rewritten:?}"
        );
        let again = parse(&rewritten).unwrap();
        assert_eq!(again.milestones.len(), 2, "{rewritten}");
    }

    #[test]
    fn pass_with_rework_after_evidence_is_rejected() {
        let text = format!(
            "{HEADER}# Verdicts\n- T-1 attempt=1 contract=1 candidate=abc pass evidence=x rework=\"y\"\n"
        );
        let detail = detail_of(&text);
        assert!(detail.starts_with("line 3: "), "{detail}");
    }

    #[test]
    fn repeated_section_is_rejected() {
        let text = format!("{HEADER}# Goal\none\n\n# Goal\ntwo\n");
        let detail = detail_of(&text);
        assert!(detail.starts_with("line 5: "), "{detail}");
    }

    #[test]
    fn repeated_milestone_id_is_rejected() {
        let text = format!("{HEADER}# Milestone M2: A\na\n\n# Milestone M2: B\nb\n");
        let detail = detail_of(&text);
        assert!(detail.starts_with("line 5: "), "{detail}");
    }

    #[test]
    fn a_voyage_section_is_read_as_a_milestone() {
        let body = "done: users log in\nbar: block only a broken login\neta: 8h\n";
        let voyage = format!("{HEADER}# Voyage M2: A\n{body}");
        let milestone = format!("{HEADER}# Milestone M2: A\n{body}");
        let from_voyage = parse(&voyage).unwrap();
        let from_milestone = parse(&milestone).unwrap();
        assert_eq!(from_voyage.milestones, from_milestone.milestones);
        assert_eq!(
            from_voyage.milestones,
            vec![("M2: A".to_string(), body.trim().to_string())]
        );
        let (done, bar, eta) = milestone_fields(&from_voyage.milestones[0].1);
        assert_eq!(done.as_deref(), Some("users log in"));
        assert_eq!(bar.as_deref(), Some("block only a broken login"));
        assert_eq!(eta.as_deref(), Some("8h"));
    }

    #[test]
    fn a_voyage_and_a_milestone_with_one_id_are_refused() {
        let text = format!("{HEADER}# Voyage M2: A\na\n\n# Milestone M2: B\nb\n");
        let detail = detail_of(&text);
        assert!(detail.contains("milestone M2 appears twice"), "{detail}");
    }
}
