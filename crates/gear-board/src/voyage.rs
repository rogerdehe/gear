//! Shared voyage measurements and the five-line captain/board report (T-39).
//! Pace uses completed tasks with both an estimate and a recorded span. Unknown
//! history stays unknown; remaining work is never treated as measured work.

use std::collections::BTreeMap;

use gear_model::{GearResult, TaskState, Ts};
use gear_store::{TaskRow, Tx};

#[derive(Debug, Clone, PartialEq)]
pub struct VoyageReport {
    pub milestone: String,
    pub title: String,
    pub done: Option<String>,
    pub eta: Option<String>,
    /// When the voyage departs, while it waits for its departure time (T-449):
    /// `Some` only before it, `None` at and after it and without `[schedule]`.
    pub departs_at: Option<Ts>,
    pub elapsed_min: Option<i64>,
    pub measured_actual_min: i64,
    pub measured_estimate_min: i64,
    pub remaining_estimate_min: Option<i64>,
    /// Minutes from departure, not minutes from now.
    pub projected_min: Option<i64>,
    pub projected_at: Option<Ts>,
    pub scope_min: Option<i64>,
    pub departure_scope_min: Option<i64>,
    pub rework_rounds: usize,
    /// The slowest task of the voyage: its id and the minutes it took.
    pub slowest: Option<(String, i64)>,
    /// The lane that sets the finish (the slowest one, §10.7 row B4): its
    /// name and the minutes of work it has left.
    pub slowest_lane: Option<(String, i64)>,
    /// The lanes of the voyage, with their own pace and what is left.
    pub lanes: Vec<LaneProjection>,
    /// The minutes of estimate merged per hour of the voyage (the burn-up).
    pub burn_up: Vec<i64>,
}

/// One lane of the voyage (design §10.7 row B4).
#[derive(Debug, Clone, PartialEq)]
pub struct LaneProjection {
    /// The name of the lane.
    pub lane: String,
    /// The minutes the lane measured on its finished work.
    pub done_min: i64,
    /// The estimate of the lane's finished work (its pace is measured against it).
    pub done_estimate_min: i64,
    /// The estimates of the lane's open work.
    pub left_min: i64,
    /// Every open task of the lane has an estimate: an unknown one leaves
    /// the lane's finish unknown (it is never counted as zero work).
    pub left_known: bool,
    /// The tasks the lane has finished.
    pub done_count: usize,
    /// The tasks the lane still has.
    pub left_count: usize,
    /// The lane's own pace: measured minutes per estimated minute.
    pub pace: Option<f64>,
    /// The minutes the lane needs at its own pace.
    pub projected_min: Option<i64>,
}

impl VoyageReport {
    pub fn pace(&self) -> Option<f64> {
        (self.measured_estimate_min > 0).then(|| {
            self.measured_actual_min as f64 / self.measured_estimate_min as f64
        })
    }

    /// Exactly five lines per active milestone, shared by every text surface.
    pub fn lines(&self) -> Vec<String> {
        let pace = self.pace().map(|p| format!("{p:.2}x")).unwrap_or_else(|| "-".into());
        let slowest = self.slowest.as_ref().map(|(id, min)| format!("{id} {min}m"))
            .unwrap_or_else(|| "-".into());
        // The departure time says why nothing moves while the voyage waits
        // for it (T-449); without one the line is unchanged.
        let departs = self.departs_at
            .map(|at| format!(" · departs at {}", crate::idle::clock(at)))
            .unwrap_or_default();
        vec![
            format!("Voyage {} {} · done: {}{departs}", self.milestone, self.title,
                self.done.as_deref().unwrap_or("-")),
            format!("  elapsed {} / eta {}", minutes(self.elapsed_min), self.eta.as_deref().unwrap_or("-")),
            format!("  pace {pace} · projected finish {} from departure", minutes(self.projected_min)),
            format!("  scope now {} / departure {}", minutes(self.scope_min), minutes(self.departure_scope_min)),
            format!("  rework {} {} · slowest {slowest}", self.rework_rounds,
                if self.rework_rounds == 1 { "round" } else { "rounds" }),
        ]
    }
}

/// The moment the project's voyage departs (T-449): `Some(depart_at)` while
/// `now` is before the stored config's `[schedule] depart_at`, else `None`.
/// The same rule as `gear_app::config::departs_at`; gear-board reads the
/// stored config itself because gear-app already depends on gear-board.
fn departs_at(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Option<Ts>> {
    let Some(row) = tx.get_project(project)? else { return Ok(None) };
    let Ok(config) = row.config_toml.parse::<toml::Value>() else { return Ok(None) };
    let Some(at) = config
        .get("schedule")
        .and_then(|schedule| schedule.get("depart_at"))
        .and_then(toml::Value::as_str)
    else {
        return Ok(None);
    };
    let Some(at) = crate::idle::rfc3339(at) else { return Ok(None) };
    Ok((now < at).then_some(at))
}

pub fn reports(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<VoyageReport>> {
    build_reports(tx, project, now, false)
}

/// Also measure finished voyages, so a station can include the final third
/// completion. Consumers decide which historical voyages they are watching.
pub fn reports_including_finished(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<VoyageReport>> {
    build_reports(tx, project, now, true)
}

fn build_reports(tx: &Tx<'_>, project: &str, now: Ts, include_finished: bool) -> GearResult<Vec<VoyageReport>> {
    let tasks = tx.tasks_of_project(project)?;
    let departs_at = departs_at(tx, project, now)?;
    let mut reports = Vec::new();
    for voyage in tx.voyages_of_project(project)? {
        let scope: Vec<_> = tasks.iter().filter(|t| {
            t.milestone.as_deref() == Some(&voyage.milestone)
                && !matches!(t.state, TaskState::Draft | TaskState::Cancelled | TaskState::Failed)
        }).collect();
        if !include_finished && !scope.iter().any(|t| !t.state.is_terminal()) {
            continue;
        }
        let mut r = VoyageReport {
            milestone: voyage.milestone, title: voyage.title, done: voyage.done, eta: voyage.eta,
            departs_at,
            elapsed_min: voyage.departure.map(|start| (now - start).max(0) / 60),
            measured_actual_min: 0, measured_estimate_min: 0,
            remaining_estimate_min: Some(0), projected_min: None, projected_at: None,
            scope_min: Some(0), departure_scope_min: voyage.departure_estimate_min,
            rework_rounds: 0, slowest: None, slowest_lane: None,
            lanes: Vec::new(), burn_up: Vec::new(),
        };
        // Every lane of the voyage, with its own pace (§10.7 row B4).
        let mut lanes: BTreeMap<String, LaneProjection> = BTreeMap::new();
        // The burn-up: the minutes of estimate merged per hour of the voyage.
        let mut burn_up: Vec<i64> = Vec::new();
        // A lane's throughput (T-390): its own minutes run from the earliest
        // `started` of any attempt of its voyage tasks to the latest hand-in of
        // its done tasks. Review and merge run while the lane is already on its
        // next task, so the span of a task up to its merge is not the lane's
        // own time and would count the same minutes twice.
        let mut throughput: BTreeMap<String, (Option<Ts>, Option<Ts>)> = BTreeMap::new();
        for task in scope {
            let estimate = estimate_of(tx, project, task)?;
            r.scope_min = add(r.scope_min, estimate);
            let rounds = tx.attempt_times_of_task(project, &task.id)?;
            r.rework_rounds += rounds.len().saturating_sub(1);
            let start = rounds.first().and_then(|(_, t)| t.started);
            let merged = rounds.iter().filter_map(|(_, t)| t.merged).max();
            let end = if task.state == TaskState::Done {
                rounds.iter().filter_map(|(_, t)| t.last()).max()
            } else {
                Some(now)
            };
            let actual = match (start, end) {
                (Some(start), Some(end)) if end >= start => Some((end - start) / 60),
                _ => None,
            };
            let name = task.lane.clone().unwrap_or_else(|| "-".to_string());
            let lane = lanes.entry(name.clone()).or_insert_with(|| LaneProjection {
                lane: name.clone(),
                done_min: 0,
                done_estimate_min: 0,
                left_min: 0,
                left_known: true,
                done_count: 0,
                left_count: 0,
                pace: None,
                projected_min: None,
            });
            if let Some(actual) = actual {
                if r.slowest.as_ref().is_none_or(|(_, longest)| actual > *longest) {
                    r.slowest = Some((task.id.clone(), actual));
                }
            }
            // The lane's own window: every attempt of every voyage task opens
            // it, a done task's hand-in (or its last step when it was never
            // handed in) closes it.
            let (lane_started, lane_closed) = throughput.entry(name.clone()).or_default();
            if let Some(started) = rounds.iter().filter_map(|(_, t)| t.started).min() {
                *lane_started = Some(lane_started.map_or(started, |first| first.min(started)));
            }
            if task.state == TaskState::Done {
                let closed = rounds.iter().filter_map(|(_, t)| t.handed_in).max()
                    .or_else(|| rounds.iter().filter_map(|(_, t)| t.last()).max());
                if let Some(closed) = closed {
                    *lane_closed = Some(lane_closed.map_or(closed, |last| last.max(closed)));
                }
                if let (Some(_), Some(estimate)) = (actual, estimate.filter(|e| *e > 0)) {
                    r.measured_estimate_min += estimate;
                    lane.done_estimate_min += estimate;
                    lane.done_count += 1;
                }
                // The work that was merged in that hour of the voyage.
                if let (Some(merged), Some(departure)) = (merged, voyage.departure) {
                    let hour = ((merged - departure) / 3600).max(0) as usize;
                    while burn_up.len() <= hour {
                        burn_up.push(0);
                    }
                    burn_up[hour] += estimate.unwrap_or(0);
                }
            } else {
                r.remaining_estimate_min = add(r.remaining_estimate_min, estimate);
                lane.left_min += estimate.unwrap_or(0);
                lane.left_count += 1;
                // An open task without an estimate is unknown work: it is
                // never counted as zero (F2).
                if estimate.is_none() {
                    lane.left_known = false;
                }
            }
        }
        // Every lane's own pace, and what it needs at that pace. A lane that
        // measured nothing has no pace: its projection stays unknown.
        for (name, lane) in lanes.iter_mut() {
            let (started, closed) = throughput.get(name).copied().unwrap_or_default();
            lane.done_min = match (started, closed) {
                (Some(started), Some(closed)) if closed >= started => (closed - started) / 60,
                _ => 0,
            };
            lane.pace = (lane.done_estimate_min > 0)
                .then(|| lane.done_min as f64 / lane.done_estimate_min as f64);
            lane.projected_min = lane
                .pace
                .filter(|_| lane.left_known)
                .map(|pace| ((lane.left_min as f64) * pace).ceil() as i64);
        }
        r.lanes = lanes.into_values().collect();
        // The voyage's measured time is the lanes' own time, not the sum of
        // their tasks' spans: overlapping review and merge is not double work.
        r.measured_actual_min = r.lanes.iter().map(|lane| lane.done_min).sum();
        // The slowest lane sets the finish and is named (§10.7 row B4). The
        // finish is known only when every lane that has open work has a pace
        // and known estimates: an unmeasured lane is never dropped and never
        // borrows another's pace (F2).
        let finish_known = r
            .lanes
            .iter()
            .all(|lane| lane.left_count == 0 || (lane.pace.is_some() && lane.left_known));
        r.slowest_lane = if finish_known {
            r.lanes
                .iter()
                .filter_map(|lane| lane.projected_min.map(|min| (lane.lane.clone(), min)))
                .max_by_key(|(_, min)| *min)
        } else {
            None
        };
        if let (Some(elapsed), Some(slowest)) =
            (r.elapsed_min, r.slowest_lane.as_ref().map(|(_, min)| *min))
        {
            r.projected_min = Some(elapsed + slowest);
            r.projected_at = voyage.departure.zip(r.projected_min).map(|(start, min)| start + min * 60);
        }
        // The burn-up is cumulative: the scope done so far, hour by hour (the
        // prototype's BURN = [0, 0, 95, 170, 260, 330]). The hours without
        // progress stay in the series (F3).
        if let Some(departure) = voyage.departure {
            let hour = ((now - departure) / 3600).max(0) as usize;
            while burn_up.len() <= hour {
                burn_up.push(0);
            }
        }
        let mut done_so_far = 0;
        for minutes in burn_up.iter_mut() {
            done_so_far += *minutes;
            *minutes = done_so_far;
        }
        r.burn_up = burn_up;
        reports.push(r);
    }
    Ok(reports)
}

fn add(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    Some(a? + b?)
}

pub fn minutes(value: Option<i64>) -> String {
    value.map(|v| format!("{v}m")).unwrap_or_else(|| "-".into())
}

/// Estimate in minutes, with the M2 legacy goal fallback shared with T-38.
pub fn estimate_of(tx: &Tx<'_>, project: &str, task: &TaskRow) -> GearResult<Option<i64>> {
    if let Some(minutes) = task.estimate_min {
        return Ok(Some(minutes));
    }
    Ok(tx.get_contract(project, &task.id, task.contract_version)?
        .and_then(|contract| estimate_from_goal(&contract.goal)))
}

pub fn estimate_from_goal(goal: &str) -> Option<i64> {
    let rest = goal.split_once("Estimate:")?.1.trim_start();
    let number: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let value: f64 = number.parse().ok()?;
    let unit: String = rest[number.len()..].trim_start().chars()
        .take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_lowercase();
    let minutes = match unit.as_str() {
        "h" | "hr" | "hrs" | "hour" | "hours" => value * 60.0,
        "m" | "min" | "mins" | "minute" | "minutes" => value,
        _ => return None,
    };
    Some(minutes.round() as i64)
}
