//! Shared voyage measurements and the five-line captain/board report (T-39).
//! Pace uses completed tasks with both an estimate and a recorded span. Unknown
//! history stays unknown; remaining work is never treated as measured work.

use gear_model::{GearResult, TaskState, Ts};
use gear_store::{TaskRow, Tx};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoyageReport {
    pub milestone: String,
    pub title: String,
    pub done: Option<String>,
    pub eta: Option<String>,
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
    pub slowest: Option<(String, i64)>,
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
        vec![
            format!("Voyage {} {} · done: {}", self.milestone, self.title,
                self.done.as_deref().unwrap_or("-")),
            format!("  elapsed {} / eta {}", minutes(self.elapsed_min), self.eta.as_deref().unwrap_or("-")),
            format!("  pace {pace} · projected finish {} from departure", minutes(self.projected_min)),
            format!("  scope now {} / departure {}", minutes(self.scope_min), minutes(self.departure_scope_min)),
            format!("  rework {} {} · slowest {slowest}", self.rework_rounds,
                if self.rework_rounds == 1 { "round" } else { "rounds" }),
        ]
    }
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
            elapsed_min: voyage.departure.map(|start| (now - start).max(0) / 60),
            measured_actual_min: 0, measured_estimate_min: 0,
            remaining_estimate_min: Some(0), projected_min: None, projected_at: None,
            scope_min: Some(0), departure_scope_min: voyage.departure_estimate_min,
            rework_rounds: 0, slowest: None,
        };
        for task in scope {
            let estimate = estimate_of(tx, project, task)?;
            r.scope_min = add(r.scope_min, estimate);
            let rounds = tx.attempt_times_of_task(project, &task.id)?;
            r.rework_rounds += rounds.len().saturating_sub(1);
            let start = rounds.first().and_then(|(_, t)| t.started);
            let end = if task.state == TaskState::Done {
                rounds.iter().filter_map(|(_, t)| t.last()).max()
            } else {
                Some(now)
            };
            let actual = match (start, end) {
                (Some(start), Some(end)) if end >= start => Some((end - start) / 60),
                _ => None,
            };
            if task.state == TaskState::Done {
                if let (Some(actual), Some(estimate)) = (actual, estimate.filter(|e| *e > 0)) {
                    r.measured_actual_min += actual;
                    r.measured_estimate_min += estimate;
                }
            } else {
                r.remaining_estimate_min = add(r.remaining_estimate_min, estimate);
            }
            if let Some(actual) = actual {
                if r.slowest.as_ref().is_none_or(|(_, longest)| actual > *longest) {
                    r.slowest = Some((task.id.clone(), actual));
                }
            }
        }
        if let (Some(elapsed), Some(remaining), Some(pace)) =
            (r.elapsed_min, r.remaining_estimate_min, r.pace())
        {
            r.projected_min = Some(elapsed + (remaining as f64 * pace).ceil() as i64);
            r.projected_at = voyage.departure.zip(r.projected_min).map(|(start, min)| start + min * 60);
        }
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
