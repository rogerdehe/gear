//! The voyage report (design §7.5) and the work/waiting time of a voyage
//! (§5.4).
//!
//! `gear report voyage [--json] [--all]` is the captain's landing report: the
//! voyage's own lines, its `done:` sentence clause by clause, its confirmed
//! goal changes, its time and tokens, the ★ decisions in full (the rest
//! counted per kind), the friction reports with their outcomes, and the tasks
//! that took more than one round.
//!
//! §5.4 keeps working and waiting apart: a spent quota stops lanes for
//! hours, so the wall clock of a voyage says how much of it the fleet was
//! stopped.

use std::collections::BTreeMap;

use crate::identity::{require_seat, Fingerprint};
use crate::time::{UsageOverrides, VoyageWork};
use crate::Env;
use gear_model::{GearResult, SeatRole, TaskState, Ts};
use gear_store::{Store, Tx};

// ---------------------------------------------------------------- the report

/// The captain's landing report of a voyage (design §7.5).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Report {
    /// The project the report is about.
    pub project: String,
    /// The milestone the voyage sails on.
    pub milestone: String,
    /// The voyage's title.
    pub title: String,
    /// The voyage's own report lines (today's `VoyageReport::lines`).
    pub voyage_lines: Vec<String>,
    /// The `done:` sentence, clause by clause.
    pub done_clauses: Vec<String>,
    /// The work and waiting time of the voyage (§5.4).
    pub work: VoyageWork,
    /// The time and tokens of the voyage's tasks.
    pub time: crate::time::VoyageTime,
    /// The five tasks with the highest actual/estimate.
    pub slowest: Vec<TaskRatio>,
    /// The ★ decisions, in full.
    pub starred: Vec<DecisionLine>,
    /// The confirmed goal changes of the voyage (T-335): the user's decisions
    /// on its `done`, `bar` and `eta` (T-410, T-331).
    pub goal_changes: Vec<DecisionLine>,
    /// How many decisions of each kind there are besides the ★ ones.
    pub kind_counts: Vec<(String, usize)>,
    /// Every decision of the voyage (only with `--all`).
    pub all_decisions: Vec<DecisionLine>,
    /// The friction reports with their outcomes; the open ones are marked.
    pub frictions: Vec<FrictionLine>,
    /// The tasks that took more than one round.
    pub rework: Vec<String>,
}

/// One task of the "highest actual/estimate" list.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TaskRatio {
    pub id: String,
    pub actual_min: Option<i64>,
    pub estimate_min: Option<i64>,
    pub ratio: Option<f64>,
}

/// One decision of the log, as the report shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DecisionLine {
    pub kind: String,
    pub what: String,
    pub why: Option<String>,
    pub alt: Option<String>,
    pub changes: Option<String>,
    pub at: String,
    pub star: bool,
    pub source: String,
}

/// One friction report, as the report shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FrictionLine {
    pub id: String,
    pub task: String,
    pub attempt: i64,
    pub role: String,
    pub body: String,
    /// The captain's outcome; None while the report is open.
    pub outcome: Option<String>,
}

/// Build the voyage report of the caller's project (the captain's view).
///
/// The caller must be the captain of the project (`Forbidden` otherwise).
/// Read-only: nothing is written.
pub fn build(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    milestone: &str,
    all: bool,
    now: Ts,
) -> GearResult<Report> {
    build_with(store, env, fp, milestone, all, now, &UsageOverrides::default())
}

/// [`build`] with the counted usage of the open rounds (T-207).
///
/// The overrides are the counts of the rounds that are still running: they
/// stand in for the numbers the store has not written yet.
pub fn build_with(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    milestone: &str,
    all: bool,
    now: Ts,
    overrides: &UsageOverrides,
) -> GearResult<Report> {
    store.read(|tx| {
        let (ctx, _seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Captain]),
            Some(&[gear_model::SeatState::Active]),
        )?;
        report_with(tx, &ctx.project, milestone, all, now, overrides)
    })
}

/// The report of a project, read from the store.
pub fn report(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    all: bool,
    now: Ts,
) -> GearResult<Report> {
    report_with(tx, project, milestone, all, now, &UsageOverrides::default())
}

/// [`report`] with the counted usage of the open rounds (T-207).
pub fn report_with(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    all: bool,
    now: Ts,
    overrides: &UsageOverrides,
) -> GearResult<Report> {
    let voyage = tx.get_voyage(project, milestone)?;
    let voyage = voyage.ok_or_else(|| {
        gear_model::GearError::new(
            gear_model::ErrorCode::NoTarget,
            format!("no voyage {milestone} in project {project}"),
        )
    })?;
    // The voyage's own report lines.
    let voyage_lines = gear_board::voyage::reports_including_finished(tx, project, now)?
        .into_iter()
        .find(|report| report.milestone == milestone)
        .map(|report| report.lines())
        .unwrap_or_default();
    // The `done:` sentence, clause by clause.
    let done_clauses = clauses_of(voyage.done.as_deref().unwrap_or_default());
    // The time and tokens of the voyage's tasks.
    let time = crate::time::report_with(
        tx,
        project,
        &crate::time::Filter { task: None, voyage: Some(milestone.to_string()) },
        now,
        overrides,
    )?;
    let voyage_time = time
        .voyages
        .iter()
        .find(|voyage| voyage.milestone.as_deref() == Some(milestone))
        .cloned()
        .unwrap_or_default();
    // The five tasks with the highest actual/estimate.
    let mut slowest: Vec<TaskRatio> = time
        .tasks
        .iter()
        .map(|task| TaskRatio {
            id: task.id.clone(),
            actual_min: task.actual_min,
            estimate_min: task.estimate_min,
            ratio: task.ratio(),
        })
        .filter(|task| task.ratio.is_some())
        .collect();
    slowest.sort_by(|a, b| {
        b.ratio
            .unwrap_or(0.0)
            .partial_cmp(&a.ratio.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    slowest.truncate(5);
    // The decisions of the voyage: the ★ ones in full, the rest counted per
    // kind (§7.5).
    let decisions: Vec<DecisionLine> = tx
        .decisions_of(project, milestone)?
        .into_iter()
        .map(|row| DecisionLine {
            kind: row.kind,
            what: row.what,
            why: row.why,
            alt: row.alt,
            changes: row.changes,
            at: row.at,
            star: row.star,
            source: row.source,
        })
        .collect();
    let starred: Vec<DecisionLine> = decisions.iter().filter(|d| d.star).cloned().collect();
    // The confirmed goal changes: the decisions of kind `user` with source
    // `gear`, in the order they appeared (T-335).
    let goal_changes: Vec<DecisionLine> = decisions
        .iter()
        .filter(|d| d.kind == "user" && d.source == "gear")
        .cloned()
        .collect();
    let mut kind_counts: BTreeMap<String, usize> = BTreeMap::new();
    for decision in decisions.iter().filter(|d| !d.star) {
        *kind_counts.entry(decision.kind.clone()).or_default() += 1;
    }
    // The friction reports with their outcomes; the open ones are marked.
    // Only this voyage's tasks: the friction of an earlier voyage is not
    // this voyage's (design §7.5).
    let voyage_tasks: Vec<String> = tx
        .tasks_of_project(project)?
        .into_iter()
        .filter(|task| task.milestone.as_deref() == Some(milestone))
        .map(|task| task.id)
        .collect();
    let frictions: Vec<FrictionLine> = tx
        .frictions_of_project(project)?
        .into_iter()
        .filter(|row| voyage_tasks.iter().any(|task| task == &row.task))
        .map(|row| FrictionLine {
            id: row.id,
            task: row.task,
            attempt: row.attempt,
            role: row.role,
            body: row.body,
            outcome: row.outcome,
        })
        .collect();
    // The tasks that took more than one round.
    let rework: Vec<String> = time
        .tasks
        .iter()
        .filter(|task| task.rework_rounds > 0)
        .map(|task| task.id.clone())
        .collect();
    Ok(Report {
        project: project.to_string(),
        milestone: milestone.to_string(),
        title: voyage.title,
        voyage_lines,
        done_clauses,
        work: crate::time::work_of_with(tx, project, milestone, now, overrides)?,
        time: voyage_time,
        slowest,
        starred,
        goal_changes,
        kind_counts: kind_counts.into_iter().collect(),
        all_decisions: if all { decisions } else { Vec::new() },
        frictions,
        rework,
    })
}

/// The `done:` sentence, clause by clause.
///
/// The sentence is a numbered list in one line: `(1) …; (2) …`. A clause
/// starts with its number, so the clauses are split there.
fn clauses_of(done: &str) -> Vec<String> {
    // The places a clause starts: a "(<number>)" at the beginning of the
    // text or right after a "; ".
    let bytes = done.as_bytes();
    let mut starts: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b')' {
                let before = done[..i].trim_end();
                if before.is_empty() || before.ends_with(';') {
                    starts.push(i);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    if starts.is_empty() {
        let whole = done.trim();
        return if whole.is_empty() { Vec::new() } else { vec![whole.to_string()] };
    }
    let mut clauses: Vec<String> = Vec::new();
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(bytes.len());
        let clause = done[*start..end].trim().trim_end_matches(';').trim().to_string();
        if !clause.is_empty() {
            clauses.push(clause);
        }
    }
    clauses
}

// ---------------------------------------------------------------- rendering

/// The report as the captain reads it (design §7.5): the seven parts in order.
pub fn render(report: &Report) -> String {
    let mut lines: Vec<String> = Vec::new();
    // 1. The voyage's own report lines.
    lines.extend(report.voyage_lines.iter().cloned());
    lines.push(String::new());
    // 2. The `done:` sentence, clause by clause.
    lines.push(format!("Done ({})", report.milestone));
    for clause in &report.done_clauses {
        lines.push(format!("  {clause}"));
    }
    lines.push(String::new());
    // 3. The confirmed goal changes of the voyage (T-335).
    lines.push(format!("Goal changes ({})", report.milestone));
    if report.goal_changes.is_empty() {
        lines.push("  no goal change".to_string());
    }
    for change in &report.goal_changes {
        lines.push(format!("  {}", decision_line(change)));
    }
    lines.push(String::new());
    // 4. Time and tokens: the wall clock (§5.4), the work and waiting time
    // per lane, and the five tasks with the highest actual/estimate.
    lines.push(crate::time::voyage_work_line(&report.work));
    lines.push(format!(
        "Time and tokens · {}",
        crate::time::voyage_tokens_line(&report.time)
    ));
    for lane in &report.work.lanes {
        lines.push(format!("  {}", crate::time::lane_line(lane)));
        // The lane's own tokens and cost of this voyage (design §7.5).
        lines.push(format!("  lane {} · {}", lane.lane, lane.tokens.line()));
    }
    if !report.slowest.is_empty() {
        lines.push("  slowest tasks (actual/estimate)".to_string());
        for task in &report.slowest {
            lines.push(format!(
                "    {} · actual {} · estimate {} · ratio {:.2}",
                task.id,
                minutes(task.actual_min),
                minutes(task.estimate_min),
                task.ratio.unwrap_or(0.0)
            ));
        }
    }
    lines.push(String::new());
    // 5. The ★ decisions in full; the rest counted per kind.
    lines.push(format!("Decisions ({})", report.milestone));
    if report.starred.is_empty() {
        lines.push("  no ★ decision".to_string());
    }
    for decision in &report.starred {
        lines.push(format!("  {}", decision_line(decision)));
    }
    if !report.kind_counts.is_empty() {
        let counts: Vec<String> = report
            .kind_counts
            .iter()
            .map(|(kind, count)| format!("{kind} {count}"))
            .collect();
        lines.push(format!("  others: {}", counts.join(" · ")));
    }
    for decision in &report.all_decisions {
        lines.push(format!("  {}", decision_line(decision)));
    }
    lines.push(String::new());
    // 6. The friction reports with their outcomes; the open ones are marked.
    lines.push("Friction".to_string());
    if report.frictions.is_empty() {
        lines.push("  no friction report".to_string());
    }
    for friction in &report.frictions {
        lines.push(format!(
            "  {} {} attempt {} ({}) · {}",
            friction.id,
            friction.task,
            friction.attempt,
            friction.role,
            match &friction.outcome {
                Some(outcome) => format!("closed: {outcome}"),
                None => "open".to_string(),
            }
        ));
        lines.push(format!("    {}", friction.body));
    }
    lines.push(String::new());
    // 7. The tasks that took more than one round.
    lines.push("Rework".to_string());
    if report.rework.is_empty() {
        lines.push("  no task took more than one round".to_string());
    }
    for task in &report.rework {
        lines.push(format!("  {task}"));
    }
    lines.join("\n")
}

/// One decision as the report shows it.
fn decision_line(decision: &DecisionLine) -> String {
    let mut line = format!(
        "{}{} · {}",
        if decision.star { "★ " } else { "" },
        decision.kind,
        decision.what
    );
    if !decision.at.is_empty() {
        line.push_str(&format!(" · {}", decision.at));
    }
    if let Some(why) = &decision.why {
        line.push_str(&format!(" | why: {why}"));
    }
    if let Some(alt) = &decision.alt {
        line.push_str(&format!(" | else: {alt}"));
    }
    if let Some(changes) = &decision.changes {
        line.push_str(&format!(" | changes: {changes}"));
    }
    line
}

/// A duration in minutes, or "-" when it is not known.
fn minutes(value: Option<i64>) -> String {
    value.map(|value| format!("{value}m")).unwrap_or_else(|| "-".to_string())
}

/// The tasks of a voyage that are done, for the report's own use.
#[allow(dead_code)]
fn done_tasks(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<usize> {
    Ok(tx
        .tasks_of_project(project)?
        .iter()
        .filter(|task| {
            task.milestone.as_deref() == Some(milestone) && task.state == TaskState::Done
        })
        .count())
}
