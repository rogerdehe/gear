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

use std::collections::BTreeMap;

use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, TaskState, Ts};
use gear_store::{AttemptTimes, Store, Tx};

/// The tokens and the cost of a round (design §4.3-§4.4).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct Tokens {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub reasoning: Option<i64>,
    /// The cost of the round in USD; None when it is not known.
    pub cost_usd: Option<f64>,
    /// `agent` | `prices` | `none` — where the cost came from.
    pub cost_source: Option<String>,
}

impl Tokens {
    /// The tokens of two rounds, added up.
    ///
    /// A round whose usage could not be read is **unknown**: the sum of a
    /// known round and an unknown one is unknown, never the known part alone
    /// (a number that was not read is not a zero, design §4.3).
    pub fn add(&mut self, other: &Tokens) {
        self.input = add(self.input, other.input);
        self.output = add(self.output, other.output);
        self.cache_read = add(self.cache_read, other.cache_read);
        self.cache_write = add(self.cache_write, other.cache_write);
        self.reasoning = add(self.reasoning, other.reasoning);
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            _ => None,
        };
        // Where the cost came from: the last known source, "mixed" when two
        // rounds disagree.
        if other.cost_usd.is_some() {
            self.cost_source = match (&self.cost_source, &other.cost_source) {
                (Some(a), Some(b)) if a == b => Some(a.clone()),
                (Some(_), Some(_)) => Some("mixed".to_string()),
                (Some(a), None) => Some(a.clone()),
                (None, Some(b)) => Some(b.clone()),
                (None, None) => None,
            };
        }
    }

    /// The tokens of a round that consumed nothing: a known zero, not an
    /// unknown.
    pub fn zero() -> Tokens {
        Tokens {
            input: Some(0),
            output: Some(0),
            cache_read: Some(0),
            cache_write: Some(0),
            reasoning: Some(0),
            cost_usd: Some(0.0),
            cost_source: None,
        }
    }

    /// The tokens in and out, as the report shows them.
    ///
    /// An unknown usage is shown as `tokens ? / cost ?`, never as zero: a log
    /// that cannot be read leaves the numbers unknown (design §4.3).
    pub fn line(&self) -> String {
        match (self.input, self.output) {
            (Some(input), Some(output)) => {
                let mut line = format!("tokens {input} in / {output} out");
                line.push_str(&self.cost_line(" · "));
                line
            }
            _ => {
                let mut line = "tokens ?".to_string();
                line.push_str(&self.cost_line(" / "));
                line
            }
        }
    }

    /// The cost as the report shows it: `$0.1234`, or `?` when it is not
    /// known.
    fn cost_line(&self, separator: &str) -> String {
        match self.cost_usd {
            Some(cost) => format!("{separator}cost ${cost:.4}"),
            None => format!("{separator}cost ?"),
        }
    }
}

impl From<&gear_store::RoundTokens> for Tokens {
    fn from(tokens: &gear_store::RoundTokens) -> Tokens {
        Tokens {
            input: tokens.input,
            output: tokens.output,
            cache_read: tokens.cache_read,
            cache_write: tokens.cache_write,
            reasoning: tokens.reasoning,
            cost_usd: tokens.cost_usd,
            cost_source: Some(tokens.cost_source.clone()),
        }
    }
}

/// The sum of two values; None when either of them is unknown.
fn add(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        _ => None,
    }
}

/// The counted usage of the open rounds (T-207).
///
/// The store keeps the counted tokens of a round that has ended. An **open**
/// round has none yet: the report counts it as it reads (design §4.3) and
/// passes the count in here as an override. The stored numbers of a closed
/// round are never replaced, and nothing is written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageOverrides {
    /// The count of each open round: (task, attempt, role) → tokens.
    rounds: BTreeMap<(String, i64, String), Tokens>,
}

impl UsageOverrides {
    /// The count of one round, when it was counted.
    pub fn get(&self, task: &str, attempt: i64, role: &str) -> Option<&Tokens> {
        self.rounds
            .get(&(task.to_string(), attempt, role.to_string()))
    }

    /// The count of one round: the override of its stored numbers.
    pub fn set(&mut self, task: &str, attempt: i64, role: &str, tokens: Tokens) {
        self.rounds
            .insert((task.to_string(), attempt, role.to_string()), tokens);
    }

    /// Whether no round was counted at all.
    pub fn is_empty(&self) -> bool {
        self.rounds.is_empty()
    }
}

/// One round of a task (one attempt) and what its steps took, in minutes.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
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
    /// The tokens and the cost of this round (design §4.3-§4.4): every
    /// stored round of the attempt, the reviewer's included (the review
    /// work belongs to the task it reviews, §4.2).
    pub tokens: Tokens,
    /// The stored rounds of the attempt, one per role: the lane's own
    /// (`dev`/`rework`) and the reviewer's (`review`).
    pub usage: Vec<RoundUsage>,
}

/// One stored round of a task (design §4.3-§4.4): its role and what it
/// consumed.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct RoundUsage {
    /// The attempt the round belongs to.
    pub attempt: i64,
    /// `dev` | `rework` | `review`.
    pub role: String,
    /// The tokens and the cost of the round.
    pub tokens: Tokens,
}

/// The time of one task.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
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
    /// The tokens and the cost of the task's rounds.
    pub tokens: Tokens,
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
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
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
    /// The tokens and the cost of the voyage's tasks.
    pub tokens: Tokens,
}

impl VoyageTime {
    /// Actual time over estimate; None without an estimate.
    pub fn ratio(&self) -> Option<f64> {
        ratio(self.actual_min, self.estimate_min)
    }
}

/// The time report of a project.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct Report {
    pub project: String,
    /// The tasks, in the order of the plan.
    pub tasks: Vec<TaskTime>,
    /// The voyages, in milestone order.
    pub voyages: Vec<VoyageTime>,
    /// The work and waiting time of the voyage the report is about (§5.4):
    /// present when the report names one voyage.
    pub work: Option<VoyageWork>,
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
    now: Ts,
) -> GearResult<Report> {
    build_with(store, env, fp, filter, now, &UsageOverrides::default())
}

/// [`build`] with the counted usage of the open rounds (T-207).
///
/// The overrides are the counts of the rounds that are still running: they
/// stand in for the numbers the store has not written yet.
pub fn build_with(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    filter: &Filter,
    now: Ts,
    overrides: &UsageOverrides,
) -> GearResult<Report> {
    store.read(|tx| {
        let (ctx, _seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Captain]),
            Some(&[SeatState::Active]),
        )?;
        report_with(tx, &ctx.project, filter, now, overrides)
    })
}

/// The report of a project, read from the store.
pub fn report(tx: &Tx<'_>, project: &str, filter: &Filter, now: Ts) -> GearResult<Report> {
    report_with(tx, project, filter, now, &UsageOverrides::default())
}

/// [`report`] with the counted usage of the open rounds (T-207).
pub fn report_with(
    tx: &Tx<'_>,
    project: &str,
    filter: &Filter,
    now: Ts,
    overrides: &UsageOverrides,
) -> GearResult<Report> {
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
        let mut rounds: Vec<Round> = tx
            .attempt_times_of_task(project, &task.id)?
            .into_iter()
            .map(|(n, times)| round(n, &times))
            .collect();
        // The tokens and the cost of every attempt's own round (design §4.3):
        // the lane's dev round (or its rework round) is the work of that
        // attempt; the reviewer's round is the reviewer's, not the task's.
        let tokens = tokens_of_attempts(tx, project, &task.id, &mut rounds, overrides)?;
        // A task with neither an estimate nor an attempt has nothing to
        // account for; it is not in the report.
        let estimate = estimate_of(tx, project, &task)?;
        if rounds.is_empty() && task.estimate_min.is_none() && estimate.is_none() {
            continue;
        }
        tasks.push(task_time(task, rounds, estimate, tokens));
    }

    let voyages = voyages_of(&tasks);
    // The work and waiting time of the voyage the report is about (§5.4).
    let work = match &filter.voyage {
        Some(voyage) => Some(work_of_with(tx, project, voyage, now, overrides)?),
        None => None,
    };
    Ok(Report {
        project: project.to_string(),
        tasks,
        voyages,
        work,
    })
}

/// The estimate of a task: the plan's `estimate:` field, or — for a task that
/// has none — the "Estimate: 2.5h" sentence in its goal (the rule the M2
/// plan used while the field did not exist yet).
fn estimate_of(tx: &Tx<'_>, project: &str, task: &gear_store::TaskRow) -> GearResult<Option<i64>> {
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
    tokens: Tokens,
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
        tokens,
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
        tokens: Tokens::default(),
        usage: Vec::new(),
    }
}

/// The tokens and the cost of every stored round of the task (design §4.3-§4.4):
/// the lane's own rounds (dev, rework) **and** the reviewer's round — the
/// review work belongs to the task it reviews (§4.2).
///
/// The rounds are filled in place: every attempt carries the usage of each of
/// its stored rounds, and the task's total is their sum.
fn tokens_of_attempts(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    rounds: &mut [Round],
    overrides: &UsageOverrides,
) -> GearResult<Tokens> {
    let stored = tx.rounds_of_task(project, task)?;
    let mut tokens = Tokens::zero();
    for round in rounds.iter_mut() {
        let mut usage: Vec<RoundUsage> = Vec::new();
        for row in stored.iter().filter(|row| row.attempt == round.n) {
            // The count of an open round stands in for the numbers the store
            // has not written yet (T-207); a closed round keeps its own.
            let round_tokens = match overrides.get(task, row.attempt, &row.role) {
                Some(counted) => counted.clone(),
                None => Tokens {
                    input: row.input,
                    output: row.output,
                    cache_read: row.cache_read,
                    cache_write: row.cache_write,
                    reasoning: row.reasoning,
                    cost_usd: row.cost_usd,
                    cost_source: row.cost_source.clone(),
                },
            };
            tokens.add(&round_tokens);
            usage.push(RoundUsage {
                attempt: row.attempt,
                role: row.role.clone(),
                tokens: round_tokens,
            });
        }
        round.usage = usage;
        round.tokens = tokens_of(&round.usage);
    }
    Ok(tokens)
}

/// The tokens and the cost of a list of stored rounds.
///
/// An empty list is a known zero: no round ran, so nothing was consumed.
fn tokens_of(usage: &[RoundUsage]) -> Tokens {
    let mut tokens = Tokens::zero();
    for round in usage {
        tokens.add(&round.tokens);
    }
    tokens
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
        let mut tokens = Tokens::zero();
        for task in &voyage.tasks {
            tokens.add(&task.tokens);
        }
        voyage.tokens = tokens;
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

/// The work and waiting time of one voyage (§5.4).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct VoyageWork {
    /// The milestone the voyage sails on.
    pub milestone: String,
    /// The wall clock of the voyage: departure → now (or landing), in minutes.
    pub elapsed_min: Option<i64>,
    /// The wall time every lane seat was in a quota pause at once, in minutes.
    pub fleet_stopped_min: i64,
    /// How many quota stops stopped the whole fleet.
    pub fleet_stops: usize,
    /// The wall clock without the quota stops.
    pub elapsed_without_quota_min: Option<i64>,
    /// Per lane: the work, the quota wait, the declared wait and the idle.
    pub lanes: Vec<LaneWork>,
}

/// The work and waiting time of one lane (§5.4).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct LaneWork {
    /// The lane's name.
    pub lane: String,
    /// The seconds its rounds were active (the work it did).
    pub work_s: i64,
    /// The seconds it waited for the quota.
    pub quota_wait_s: i64,
    /// The seconds it waited for a declared wait.
    pub declared_wait_s: i64,
    /// The rest of the voyage's elapsed time.
    pub idle_s: i64,
    /// The tokens and the cost of the lane's own rounds of this voyage
    /// (design §7.5: the time and the tokens per lane).
    pub tokens: Tokens,
}

/// The work and waiting time of a voyage, read from the store.
///
/// - **work** = the sum of the lane's rounds' `active_s`;
/// - **quota wait** and **declared wait** = the sums of the lane's pauses
///   (kind `quota` and kind `wait`);
/// - **idle** = the rest of the voyage's elapsed time;
/// - **fleet stopped by quota** = the wall time during which every lane seat
///   was in a quota pause at once (the union of the overlaps);
/// - **elapsed without quota stops** = elapsed − fleet stopped.
///
/// Read-only: nothing is written.
pub fn work_of(tx: &Tx<'_>, project: &str, milestone: &str, now: Ts) -> GearResult<VoyageWork> {
    work_of_with(tx, project, milestone, now, &UsageOverrides::default())
}

/// [`work_of`] with the counted usage of the open rounds (T-207): the tokens
/// of a lane's open round are counted as the report reads it.
pub fn work_of_with(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    now: Ts,
    overrides: &UsageOverrides,
) -> GearResult<VoyageWork> {
    let voyage = tx.get_voyage(project, milestone)?;
    // The voyage's window: from its departure until now (design §5.4).
    let departure = voyage.as_ref().and_then(|v| v.departure);
    let elapsed_s = departure.map(|departure| (now - departure).max(0));
    // The tasks of this voyage: the rounds of the other voyages are not this
    // voyage's work.
    let tasks = tx.tasks_of_project(project)?;
    let own: Vec<String> = tasks
        .iter()
        .filter(|task| task.milestone.as_deref() == Some(milestone))
        .filter(|task| task.state != TaskState::Draft)
        .map(|task| task.id.clone())
        .collect();
    // The lane seats of the project: the fleet is stopped when all of them
    // are parked at once.
    let seats: Vec<gear_store::SeatRow> = tx
        .seats_of_project(project)?
        .into_iter()
        .filter(|seat| seat.role == SeatRole::Lane)
        .collect();
    // The pauses of every lane seat, per kind.
    let pauses = tx.pauses_of_project(project)?;
    // The rounds of every lane seat: their active time is the work.
    let rounds = tx.rounds_of_project(project)?;
    // A lane that was retired before this voyage is not one of the fleet's
    // lanes: it is no line of the report, and it does not hold the fleet from
    // being stopped (gear/voyages/M4/reviews/T-171.md round 2, F10). A lane that was
    // retired during the voyage, but worked in it, still is one.
    let lanes: Vec<(String, String)> = seats
        .iter()
        .filter(|seat| {
            seat.state != SeatState::Retired
                || worked_in_window(&seat.id, &rounds, &pauses, departure, now)
        })
        .map(|seat| (seat.name.clone(), seat.id.clone()))
        .collect();

    let mut work = VoyageWork {
        milestone: milestone.to_string(),
        elapsed_min: elapsed_s.map(|s| s / 60),
        ..Default::default()
    };
    // The quota pauses of every lane seat, clipped to the voyage's window:
    // a pause of another voyage is not this voyage's stoppage.
    let mut quota_spans: Vec<(String, i64, i64)> = Vec::new();
    for (name, seat_id) in &lanes {
        let mut lane = LaneWork {
            lane: name.clone(),
            tokens: Tokens::zero(),
            ..Default::default()
        };
        for row in rounds.iter().filter(|row| &row.seat == seat_id) {
            // The reviewer's round is the reviewer's work, not the lane's.
            if row.role == "review" {
                continue;
            }
            // Only the rounds of this voyage's tasks count.
            if !own.iter().any(|task| task == &row.task) {
                continue;
            }
            lane.work_s += row.active_s;
        }
        // The lane's own tokens and cost of this voyage (design §7.5).
        for row in rounds.iter().filter(|row| &row.seat == seat_id) {
            if !own.iter().any(|task| task == &row.task) {
                continue;
            }
            // The count of an open round stands in for the numbers the store
            // has not written yet (T-207).
            let tokens = match overrides.get(&row.task, row.attempt, &row.role) {
                Some(counted) => counted.clone(),
                None => Tokens {
                    input: row.input,
                    output: row.output,
                    cache_read: row.cache_read,
                    cache_write: row.cache_write,
                    reasoning: row.reasoning,
                    cost_usd: row.cost_usd,
                    cost_source: row.cost_source.clone(),
                },
            };
            lane.tokens.add(&tokens);
        }
        for pause in pauses.iter().filter(|pause| &pause.seat == seat_id) {
            // A pause that is still open ends now (design §5.4): the lane is
            // still waiting.
            let end = pause.end_at.unwrap_or(now);
            let start = pause.start_at;
            // The pause is clipped to the voyage's window.
            let Some(departure) = departure else {
                continue;
            };
            let start = start.max(departure);
            let end = end.min(now).max(start);
            if end <= start {
                continue;
            }
            let seconds = end - start;
            match pause.kind.as_str() {
                "quota" => {
                    lane.quota_wait_s += seconds;
                    quota_spans.push((seat_id.clone(), start, end));
                }
                "wait" => lane.declared_wait_s += seconds,
                _ => {}
            }
        }
        // The idle time is the rest of the voyage's elapsed time.
        lane.idle_s = elapsed_s
            .map(|elapsed| elapsed - lane.work_s - lane.quota_wait_s - lane.declared_wait_s)
            .unwrap_or(0)
            .max(0);
        work.lanes.push(lane);
    }
    // The fleet is stopped while every lane seat is parked at once (design
    // §5.4): the overlap of the quota pauses of all the lanes, not the union
    // of one lane's pauses.
    let (stopped, stops) = fleet_stopped(&quota_spans, lanes.len());
    work.fleet_stopped_min = stopped / 60;
    work.fleet_stops = stops;
    work.elapsed_without_quota_min = elapsed_s.map(|elapsed| (elapsed - stopped).max(0) / 60);
    Ok(work)
}

/// Whether a lane seat has a round or a pause inside the voyage's window
/// (the departure until now). An open round or pause reaches now.
fn worked_in_window(
    seat_id: &str,
    rounds: &[gear_store::RoundRow],
    pauses: &[gear_store::PauseRow],
    departure: Option<Ts>,
    now: Ts,
) -> bool {
    let Some(departure) = departure else {
        return false;
    };
    let overlaps = |start: Ts, end: Option<Ts>| {
        let end = end.unwrap_or(now);
        start < now && end > departure
    };
    rounds
        .iter()
        .filter(|row| row.seat == seat_id)
        .any(|row| overlaps(row.start_at, row.end_at))
        || pauses
            .iter()
            .filter(|pause| pause.seat == seat_id)
            .any(|pause| overlaps(pause.start_at, pause.end_at))
}

/// The wall time during which **every** lane seat was in a quota pause at
/// once (design §5.4): the overlap of the lanes' quota pauses.
///
/// A lane with no pause at all keeps the fleet from ever being stopped, and
/// two lanes that pause at different times never stop it either.
///
/// Returns the seconds and the number of stops.
fn fleet_stopped(spans: &[(String, i64, i64)], lanes: usize) -> (i64, usize) {
    if spans.is_empty() || lanes == 0 {
        return (0, 0);
    }
    // The merged span of every seat: a seat's own pauses never overlap.
    let mut per_seat: std::collections::BTreeMap<&str, Vec<(i64, i64)>> =
        std::collections::BTreeMap::new();
    for (seat, start, end) in spans {
        per_seat
            .entry(seat.as_str())
            .or_default()
            .push((*start, *end));
    }
    for seat_spans in per_seat.values_mut() {
        seat_spans.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (start, end) in seat_spans.iter() {
            match merged.last_mut() {
                Some(last) if *start <= last.1 => last.1 = last.1.max(*end),
                _ => merged.push((*start, *end)),
            }
        }
        *seat_spans = merged;
    }
    // The stretches every seat is paused in: the boundaries of all the spans.
    let mut points: Vec<i64> = per_seat
        .values()
        .flat_map(|spans| spans.iter().flat_map(|(start, end)| [*start, *end]))
        .collect();
    points.sort_unstable();
    points.dedup();
    let mut seconds = 0;
    let mut stops = 0;
    for window in points.windows(2) {
        let (start, end) = (window[0], window[1]);
        // The seats paused for the whole stretch.
        let paused = per_seat
            .values()
            .filter(|spans| spans.iter().any(|(from, to)| *from <= start && end <= *to))
            .count();
        if paused == lanes {
            seconds += end - start;
            stops += 1;
        }
    }
    (seconds, stops)
}

/// The §5.4 line of one lane: the work, the waits and the idle.
pub fn lane_line(lane: &LaneWork) -> String {
    format!(
        "lane {} · work {} · quota wait {} · declared wait {} · idle {}",
        lane.lane,
        hours(lane.work_s),
        hours(lane.quota_wait_s),
        hours(lane.declared_wait_s),
        hours(lane.idle_s)
    )
}

/// The §5.4 line of the voyage: the wall clock, the quota stops and the rest.
pub fn voyage_work_line(work: &VoyageWork) -> String {
    format!(
        "Voyage {} · elapsed {} · fleet stopped by quota {} ({}) · elapsed without quota stops {}",
        work.milestone,
        hours(work.elapsed_min.unwrap_or(0) * 60),
        hours(work.fleet_stopped_min * 60),
        if work.fleet_stops == 1 {
            "1 stop".to_string()
        } else {
            format!("{} stops", work.fleet_stops)
        },
        hours(work.elapsed_without_quota_min.unwrap_or(0) * 60)
    )
}

/// A duration in hours and minutes: `2h05m`.
pub fn hours(seconds: i64) -> String {
    let seconds = seconds.max(0);
    format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
}

// ---------------------------------------------------------------- rendering

/// The tokens and the cost of a voyage, as the report shows them.
pub fn voyage_tokens_line(voyage: &VoyageTime) -> String {
    voyage.tokens.line()
}

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
        // Every stored round of the task, with what it consumed (design
        // §4.5): the lane's own round and the reviewer's, each apart.
        for usage in task.rounds.iter().flat_map(|round| round.usage.iter()) {
            lines.push(format!(
                "  {} attempt {} {} · {}",
                task.id,
                usage.attempt,
                usage.role,
                usage.tokens.line()
            ));
        }
    }
    lines.push(String::new());
    for voyage in &report.voyages {
        lines.push(voyage_line(voyage));
    }
    // The work and waiting time of the voyage the report is about (§5.4).
    if let Some(work) = &report.work {
        lines.push(String::new());
        lines.push(voyage_work_line(work));
        for lane in &work.lanes {
            lines.push(lane_line(lane));
        }
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
    line.push_str(&format!(" · {}", task.tokens.line()));
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
    line.push_str(&format!(" · {}", voyage.tokens.line()));
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
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Done,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: Some("M2".into()),
                required: true,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(20),
                issue: None,
                updated: T0,
            })?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: Some("abc".into()),
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
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
            .read(|tx| report(tx, "p", &Filter::default(), T0))
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
                project: "p".into(),
                task: "T-1".into(),
                n: 2,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: Some("def".into()),
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::Started, T0 + 30 * 60)?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::HandedIn, T0 + 45 * 60)?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::Verdict, T0 + 50 * 60)?;
            tx.set_attempt_time("p", "T-1", 2, AttemptStep::Settled, T0 + 55 * 60)?;
            Ok(())
        })
        .unwrap();
        let report = s
            .read(|tx| report(tx, "p", &Filter::default(), T0))
            .unwrap();
        let task = &report.tasks[0];
        assert_eq!(task.rework_rounds, 1);
        assert_eq!(
            task.rework_min,
            Some(25),
            "the rework round ran from its start to its settle"
        );
        // The dev time of the task is still the first round's.
        assert_eq!(task.dev(), Some(10));
    }

    /// The rule the M2 plan used while the `estimate:` field did not exist:
    /// the estimate of a task is the "Estimate: 2.5h" sentence in its goal.
    #[test]
    fn an_estimate_in_the_goal_is_read() {
        assert_eq!(
            estimate_from_goal("Fix the thing. Estimate: 2.5h"),
            Some(150)
        );
        assert_eq!(estimate_from_goal("Fix the thing. Estimate: 90m"), Some(90));
        assert_eq!(
            estimate_from_goal("Fix the thing. Estimate: 1 hour"),
            Some(60)
        );
        assert_eq!(estimate_from_goal("Fix the thing."), None);
        assert_eq!(estimate_from_goal("Fix the thing. Estimate: soon"), None);
    }
}
