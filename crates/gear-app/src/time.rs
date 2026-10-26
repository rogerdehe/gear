//! The time accounting of a project (T-38): where the time went.
//!
//! Every step of an attempt has its own time (the store keeps them, so no
//! step overwrites another), so the report can say what a task cost:
//!
//! - **dev time**: start → hand-in (the lane's work),
//! - **review time**: hand-in → verdict (the reviewer's work),
//! - **wait for the captain**: verdict → settle (the decision),
//! - **merge time**: settle → merge (the integration),
//! - **rework rounds and their time**: the rounds after the first,
//! - **the ratio** actual/estimate, so a plan that says 2.5 h can be compared
//!   with what the task really took.
//!
//! Dev, review and wait describe the first round; merge describes the latest
//! round with a recorded merge. Rework rounds are also counted and timed apart
//! as whole spans (including their integration when recorded).
//! The *actual* time of a task is its whole span: the first start until the
//! last step that happened (a rework and the waiting around it are part of
//! what the task cost).
//!
//! Read-only: the report changes nothing.

use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{AttemptTimes, Store, Tx};

/// One round of a task (one attempt) and what its steps took, in minutes.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct Round {
    /// The attempt number.
    pub n: i64,
    /// The time the round started (the lane began to work).
    pub start: Option<Ts>,
    /// The time of the last step of the round that happened.
    pub last: Option<Ts>,
    /// start → hand-in.
    pub dev: Option<i64>,
    /// hand-in → verdict.
    pub review: Option<i64>,
    /// verdict → settle.
    pub wait: Option<i64>,
    /// settle → merge.
    pub merge: Option<i64>,
    /// start → the last step of the round that happened.
    pub span: Option<i64>,
}

/// The time of one task.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct TaskTime {
    pub id: String,
    /// The voyage (milestone) the task sails on.
    pub milestone: Option<String>,
    /// The planned time in minutes (`estimate:` in the plan).
    pub estimate_min: Option<i64>,
    /// The rounds of the task, the dev round first.
    pub rounds: Vec<Round>,
    /// The rounds after the first.
    pub rework_rounds: i64,
    /// The time those rounds took; None when it is not fully known.
    pub rework_min: Option<i64>,
    /// The first start → the last step that happened.
    pub actual_min: Option<i64>,
}

impl TaskTime {
    /// The dev time: the dev round's start → hand-in.
    pub fn dev(&self) -> Option<i64> {
        self.rounds.first().and_then(|round| round.dev)
    }

    /// The review time: hand-in → verdict.
    pub fn review(&self) -> Option<i64> {
        self.rounds.first().and_then(|round| round.review)
    }

    /// The wait for the captain: verdict → settle.
    pub fn wait(&self) -> Option<i64> {
        self.rounds.first().and_then(|round| round.wait)
    }

    /// The latest recorded merge time: settle → merge, including after rework.
    pub fn merge(&self) -> Option<i64> {
        self.rounds.iter().rev().find_map(|round| round.merge)
    }

    /// Actual time over estimate; None without an estimate.
    pub fn ratio(&self) -> Option<f64> {
        ratio(self.actual_min, self.estimate_min)
    }
}

/// The time of a voyage (a milestone): the sum of its tasks.
///
/// Every duration is the sum of what is known: None when no task of the voyage
/// has it (a task from before the times existed has none).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct VoyageTime {
    /// The milestone; None for the tasks that sail on no voyage.
    pub milestone: Option<String>,
    pub tasks: Vec<TaskTime>,
    /// The planned time of the voyage's tasks, in minutes.
    pub estimate_min: Option<i64>,
    pub dev: Option<i64>,
    pub review: Option<i64>,
    pub wait: Option<i64>,
    pub merge: Option<i64>,
    /// The rework rounds of the voyage's tasks.
    pub rework_rounds: i64,
    /// Their time; None when it is not fully known.
    pub rework_min: Option<i64>,
    /// The time the voyage's tasks took.
    pub actual_min: Option<i64>,
}

impl VoyageTime {
    /// Actual time over estimate; None without an estimate.
    pub fn ratio(&self) -> Option<f64> {
        ratio(self.actual_min, self.estimate_min)
    }
}

/// The time report of a project.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct Report {
    pub project: String,
    /// The tasks, in the order of the plan.
    pub tasks: Vec<TaskTime>,
    /// The voyages, in milestone order.
    pub voyages: Vec<VoyageTime>,
}

/// What the report is about: one task, one voyage, or everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only this task.
    pub task: Option<String>,
    /// Only the tasks of this voyage (milestone).
    pub voyage: Option<String>,
}

/// Build the time report of the caller's project (the captain's view).
///
/// The caller must be the captain of the project (`Forbidden` otherwise).
/// Read-only: nothing is written.
pub fn build(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    filter: &Filter,
) -> GearResult<Report> {
    store.read(|tx| {
        let (ctx, _seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Captain]),
            Some(&[SeatState::Active]),
        )?;
        report(tx, &ctx.project, filter)
    })
}

/// The report of a project, read from the store.
pub fn report(tx: &Tx<'_>, project: &str, filter: &Filter) -> GearResult<Report> {
    // A task that does not exist is `NoTarget`, not an empty report: a typo
    // must not look like a project without work.
    if let Some(wanted) = &filter.task {
        if tx.get_task(project, wanted)?.is_none() {
            return Err(GearError::new(
                ErrorCode::NoTarget,
                format!("no task {wanted} in project {project}"),
            ));
        }
    }
    let mut tasks: Vec<TaskTime> = Vec::new();
    for task in tx.tasks_of_project(project)? {
        if let Some(wanted) = &filter.task {
            if &task.id != wanted {
                continue;
            }
        }
        if let Some(wanted) = &filter.voyage {
            if task.milestone.as_deref() != Some(wanted.as_str()) {
                continue;
            }
        }
        let rounds: Vec<Round> = tx
            .attempt_times_of_task(project, &task.id)?
            .into_iter()
            .map(|(n, times)| round(n, &times))
            .collect();
        // A task with neither an estimate nor an attempt has nothing to
        // account for; it is not in the report.
        let estimate = estimate_of(tx, project, &task)?;
        if rounds.is_empty() && task.estimate_min.is_none() && estimate.is_none() {
            continue;
        }
        tasks.push(task_time(task, rounds, estimate));
    }

    let voyages = voyages_of(&tasks);
    Ok(Report { project: project.to_string(), tasks, voyages })
}

/// The estimate of a task: the plan's `estimate:` field, or — for a task that
/// has none — the "Estimate: 2.5h" sentence in its goal (the rule the M2
/// plan used while the field did not exist yet).
fn estimate_of(
    tx: &Tx<'_>,
    project: &str,
    task: &gear_store::TaskRow,
) -> GearResult<Option<i64>> {
    gear_board::voyage::estimate_of(tx, project, task)
}

#[cfg(test)]
fn estimate_from_goal(goal: &str) -> Option<i64> {
    gear_board::voyage::estimate_from_goal(goal)
}

/// The time of one task: its rounds, the rework behind it and its span.
fn task_time(
    task: gear_store::TaskRow,
    rounds: Vec<Round>,
    estimate_min: Option<i64>,
) -> TaskTime {
    let rework: Vec<&Round> = rounds.iter().skip(1).collect();
    let rework_rounds = rework.len() as i64;
    // The time of the rework rounds: known only when every one of them was
    // timed (an attempt from before the times existed was not).
    let rework_min = if rework.is_empty() {
        Some(0)
    } else if rework.iter().all(|round| round.span.is_some()) {
        Some(rework.iter().filter_map(|round| round.span).sum())
    } else {
        None
    };
    // The actual time of the task: the first start until the last step that
    // happened in any round.
    let first_start = rounds.first().and_then(|round| round.start);
    let last = rounds.iter().filter_map(|round| round.last).max();
    let actual_min = match (first_start, last) {
        (Some(start), Some(end)) if end >= start => Some((end - start) / 60),
        _ => None,
    };
    TaskTime {
        id: task.id,
        milestone: task.milestone,
        estimate_min,
        rework_rounds,
        rework_min,
        actual_min,
        rounds,
    }
}

/// One round: the durations between its steps.
fn round(n: i64, times: &AttemptTimes) -> Round {
    let last = times.last();
    Round {
        n,
        start: times.started,
        last,
        dev: between(times.started, times.handed_in),
        review: between(times.handed_in, times.verdict),
        wait: between(times.verdict, times.settled),
        merge: between(times.settled, times.merged),
        span: between(times.started, last),
    }
}

/// The minutes between two steps, when both happened.
fn between(from: Option<Ts>, to: Option<Ts>) -> Option<i64> {
    match (from, to) {
        (Some(from), Some(to)) if to >= from => Some((to - from) / 60),
        _ => None,
    }
}

/// Actual over estimate.
fn ratio(actual: Option<i64>, estimate: Option<i64>) -> Option<f64> {
    match (actual, estimate) {
        (Some(actual), Some(estimate)) if estimate > 0 => Some(actual as f64 / estimate as f64),
        _ => None,
    }
}

/// The voyages of the tasks, in milestone order. The tasks that sail on no
/// voyage are one group of their own.
fn voyages_of(tasks: &[TaskTime]) -> Vec<VoyageTime> {
    let mut voyages: Vec<VoyageTime> = Vec::new();
    for task in tasks {
        let voyage = match voyages
            .iter_mut()
            .find(|voyage| voyage.milestone == task.milestone)
        {
            Some(voyage) => voyage,
            None => {
                voyages.push(VoyageTime {
                    milestone: task.milestone.clone(),
                    ..Default::default()
                });
                voyages.last_mut().expect("the voyage was just pushed")
            }
        };
        voyage.tasks.push(task.clone());
    }
    for voyage in &mut voyages {
        voyage.estimate_min = sum_known(voyage.tasks.iter().map(|task| task.estimate_min));
        voyage.dev = sum_known(voyage.tasks.iter().map(|task| task.dev()));
        voyage.review = sum_known(voyage.tasks.iter().map(|task| task.review()));
        voyage.wait = sum_known(voyage.tasks.iter().map(|task| task.wait()));
        voyage.merge = sum_known(voyage.tasks.iter().map(|task| task.merge()));
        voyage.actual_min = sum_known(voyage.tasks.iter().map(|task| task.actual_min));
        voyage.rework_rounds = voyage.tasks.iter().map(|task| task.rework_rounds).sum();
        // The voyage's rework time is known only when every task's is.
        voyage.rework_min = if voyage.tasks.iter().all(|task| task.rework_min.is_some()) {
            Some(voyage.tasks.iter().filter_map(|task| task.rework_min).sum())
        } else {
            None
        };
    }
    voyages.sort_by(|a, b| a.milestone.cmp(&b.milestone));
    voyages
}

/// The sum of the known values; None when nothing is known.
fn sum_known(values: impl Iterator<Item = Option<i64>>) -> Option<i64> {
    let mut sum = 0;
    let mut any = false;
    for value in values.flatten() {
        sum += value;
        any = true;
    }
    any.then_some(sum)
}

// ---------------------------------------------------------------- rendering

/// The report as the captain reads it: one line per task, one line per voyage.
pub fn render(report: &Report) -> String {
    let mut lines = vec![format!("Time report · project {}", report.project)];
    if report.tasks.is_empty() {
        lines.push("no task has an estimate or an attempt yet".to_string());
        return lines.join("\n");
    }
    lines.push(String::new());
    for task in &report.tasks {
        lines.push(task_line(task));
    }
    lines.push(String::new());
    for voyage in &report.voyages {
        lines.push(voyage_line(voyage));
    }
    lines.join("\n")
}

/// One task: what it cost against what was planned.
fn task_line(task: &TaskTime) -> String {
    let mut line = format!("{} · estimate {}", task.id, minutes(task.estimate_min));
    if task.rounds.is_empty() {
        line.push_str(" · no attempt yet");
        return line;
    }
    // An attempt from before the times existed: the rounds are counted, but
    // no step of them was timed.
    if task.rounds.iter().all(|round| round.start.is_none()) {
        line.push_str(&format!(" · rework {}", rounds_word(task.rework_rounds)));
        line.push_str(" · no time recorded");
        return line;
    }
    line.push_str(&format!(
        " · dev {} · review {} · wait {} · merge {}",
        minutes(task.dev()),
        minutes(task.review()),
        minutes(task.wait()),
        minutes(task.merge())
    ));
    line.push_str(&format!(" · rework {}", rounds_word(task.rework_rounds)));
    if task.rework_rounds > 0 {
        line.push_str(&format!(" ({})", minutes(task.rework_min)));
    }
    line.push_str(&format!(" · actual {}", minutes(task.actual_min)));
    if let Some(ratio) = task.ratio() {
        line.push_str(&format!(" · ratio {ratio:.2}"));
    }
    line
}

/// "0 rounds", "1 round", "2 rounds".
fn rounds_word(rounds: i64) -> String {
    if rounds == 1 {
        "1 round".to_string()
    } else {
        format!("{rounds} rounds")
    }
}

/// One voyage: what its tasks cost together.
fn voyage_line(voyage: &VoyageTime) -> String {
    let name = voyage.milestone.as_deref().unwrap_or("no voyage");
    let mut line = format!(
        "Voyage {name} · {} task{} · estimate {}",
        voyage.tasks.len(),
        if voyage.tasks.len() == 1 { "" } else { "s" },
        minutes(voyage.estimate_min)
    );
    line.push_str(&format!(
        " · dev {} · review {} · wait {} · merge {}",
        minutes(voyage.dev),
        minutes(voyage.review),
        minutes(voyage.wait),
        minutes(voyage.merge)
    ));
    if voyage.rework_rounds == 0 {
        line.push_str(" · rework 0 rounds");
    } else {
        line.push_str(&format!(
            " · rework {} ({})",
            rounds_word(voyage.rework_rounds),
            minutes(voyage.rework_min)
        ));
    }
    line.push_str(&format!(" · actual {}", minutes(voyage.actual_min)));
    if let Some(ratio) = voyage.ratio() {
        line.push_str(&format!(" · ratio {ratio:.2}"));
    }
    line
}

/// A duration in minutes, or "-" when it is not known.
fn minutes(value: Option<i64>) -> String {
    match value {
        Some(value) => format!("{value}m"),
        None => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::{AttemptState, TaskState};
    use gear_store::testing;
    use gear_store::{AttemptRow, AttemptStep, TaskRow};

    const T0: i64 = 1_800_000_000;

    /// A task with one round: 10 min of work, 5 min of review, 3 min of
    /// waiting for the captain and 2 min of merging.
    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(), id: "T-1".into(), rank: 1, state: TaskState::Done,
                priority: "must".into(), lane: Some("heat".into()), milestone: Some("M2".into()),
                required: true, contract_version: 1, deps: vec![], followup_of: None, wait_on: None,
                wait_cond: None, rework_note: None, estimate_min: Some(20),  issue: None,updated: T0,
            })?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(), task: "T-1".into(), n: 1, seat: "p:heat".into(), generation: 1,
                contract_version: 1, adopted_version: 1, pending_update: None,
                state: AttemptState::Accepted, candidate: Some("abc".into()), report: None,
                safe_point: None, started: T0, updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Delivered, T0)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0 + 60)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, T0 + 60 + 10 * 60)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Verdict, T0 + 60 + 15 * 60)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Settled, T0 + 60 + 18 * 60)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, T0 + 60 + 20 * 60)?;
            Ok(())
        })
        .unwrap();
        s
    }

    #[test]
    fn the_durations_are_the_steps_between_them() {
        let mut s = world();
        let report = s
            .read(|tx| report(tx, "p", &Filter::default()))
            .unwrap();
        let task = &report.tasks[0];
        assert_eq!(task.dev(), Some(10));
        assert_eq!(task.review(), Some(5));
        assert_eq!(task.wait(), Some(3));
        assert_eq!(task.merge(), Some(2));
        // The span: the first start until the merge.
        assert_eq!(task.actual_min, Some(20));
        assert_eq!(task.ratio(), Some(1.0));
    }

    #[test]
    fn a_rework_round_is_counted_apart() {
        let mut s = world();
        s.write(|tx| {
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(), task: "T-1".into(), n: 2, seat: "p:heat".into(), generation: 1,
                contract_version: 1, adopted_version: 1, pending_update: None,
                state: AttemptState::Accepted, candidate: Some("def".into()), report: None,
                safe_point: None, started: T0, updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::Started, T0 + 30 * 60)?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::HandedIn, T0 + 45 * 60)?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::Verdict, T0 + 50 * 60)?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::Settled, T0 + 55 * 60)?;
            Ok(())
        })
        .unwrap();
        let report = s
            .read(|tx| report(tx, "p", &Filter::default()))
            .unwrap();
        let task = &report.tasks[0];
        assert_eq!(task.rework_rounds, 1);
        assert_eq!(task.rework_min, Some(25), "the rework round ran from its start to its settle");
        // The dev time of the task is still the first round's.
        assert_eq!(task.dev(), Some(10));
    }

    /// The rule the M2 plan used while the `estimate:` field did not exist:
    /// the estimate of a task is the "Estimate: 2.5h" sentence in its goal.
    #[test]
    fn an_estimate_in_the_goal_is_read() {
        assert_eq!(estimate_from_goal("Fix the thing. Estimate: 2.5h"), Some(150));
        assert_eq!(estimate_from_goal("Fix the thing. Estimate: 90m"), Some(90));
        assert_eq!(estimate_from_goal("Fix the thing. Estimate: 1 hour"), Some(60));
        assert_eq!(estimate_from_goal("Fix the thing."), None);
        assert_eq!(estimate_from_goal("Fix the thing. Estimate: soon"), None);
    }
}
