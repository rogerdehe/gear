//! How fast each model works (M7.3): the measured work time of the attempts
//! it ran, so a plan can calibrate an estimate to the model that will do it.
//!
//! Read-only: the store is read, nothing is written. The dev time is
//! [`crate::time::dev_of`], the same computation `gear report time` uses — a
//! second one would drift from it.

use std::collections::BTreeMap;

use gear_model::{GearResult, TaskState, Ts};
use gear_store::Tx;

/// The measured speed of one model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSpeed {
    pub model: String,
    /// The attempts measured: every one with a model and a work time, whether
    /// or not it had an estimate.
    pub samples: u32,
    /// The median dev time of those attempts, in minutes.
    pub median_work_min: u32,
    /// The median work time over estimate, over the attempts that had an
    /// estimate; 0 when none did.
    pub median_ratio: f64,
}

/// The window the attempts are taken from: 90 days.
const WINDOW_S: i64 = 90 * 24 * 3600;

/// The speed of every model the store has measured (M7.3): over every
/// project's attempts from the last 90 days that were handed in, the dev time
/// and the estimate the plan gave them; a captain's own done tasks are
/// measured from merge to merge (or from the voyage's departure for the first).
/// Sorted by samples, most first.
pub fn measured(tx: &Tx<'_>, now: Ts) -> GearResult<Vec<ModelSpeed>> {
    let cutoff = now - WINDOW_S;
    let mut by_model: BTreeMap<String, (Vec<i64>, Vec<f64>)> = BTreeMap::new();
    for project in tx.project_ids()? {
        let departures: BTreeMap<String, Ts> = tx
            .voyages_of_project(&project)?
            .into_iter()
            .filter_map(|voyage| voyage.departure.map(|at| (voyage.milestone, at)))
            .collect();
        // The captain's own done tasks, per voyage: merge → merge.
        let mut own: BTreeMap<String, Vec<(Ts, String, Option<i64>)>> = BTreeMap::new();
        for task in tx.tasks_of_project(&project)? {
            let times = tx.attempt_times_of_task(&project, &task.id)?;
            let attempts = tx.attempts_of_task(&project, &task.id)?;
            if task.lane.is_none() && task.state == TaskState::Done {
                // The attempt whose merge time is the task's: the latest one.
                let mut best: Option<(i64, Ts)> = None;
                for (n, time) in &times {
                    if let Some(merged) = time.merged {
                        if best.is_none_or(|(_, at)| merged > at) {
                            best = Some((*n, merged));
                        }
                    }
                }
                let Some((n, merged)) = best else {
                    continue;
                };
                if merged < cutoff {
                    continue;
                }
                let Some(attempt) = attempts.iter().find(|attempt| attempt.n == n) else {
                    continue;
                };
                let Some(model) = model_of(tx, &project, &task.id, n, &attempt.seat)? else {
                    continue;
                };
                let estimate = estimate_of(&task, n);
                let Some(milestone) = task.milestone.clone() else {
                    continue;
                };
                own.entry(milestone)
                    .or_default()
                    .push((merged, model, estimate));
                continue;
            }
            for (n, time) in &times {
                let Some(handed_in) = time.handed_in else {
                    continue; // not handed in: no work time to measure
                };
                if handed_in < cutoff {
                    continue;
                }
                let Some(dev) = crate::time::dev_of(time) else {
                    continue;
                };
                let Some(attempt) = attempts.iter().find(|attempt| attempt.n == *n) else {
                    continue;
                };
                let Some(model) = model_of(tx, &project, &task.id, *n, &attempt.seat)? else {
                    continue;
                };
                add(&mut by_model, model, dev, estimate_of(&task, *n));
            }
        }
        for (milestone, mut rows) in own {
            rows.sort_by_key(|(merged, _, _)| *merged);
            let mut previous = departures.get(&milestone).copied();
            for (merged, model, estimate) in rows {
                if let Some(previous) = previous {
                    if merged >= previous {
                        add(&mut by_model, model, (merged - previous) / 60, estimate);
                    }
                }
                previous = Some(merged);
            }
        }
    }
    let mut speeds: Vec<ModelSpeed> = by_model
        .into_iter()
        .map(|(model, (work, ratios))| ModelSpeed {
            model,
            samples: work.len() as u32,
            median_work_min: median_minutes(&work),
            median_ratio: median_ratio(&ratios),
        })
        .collect();
    speeds.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.model.cmp(&b.model)));
    Ok(speeds)
}

/// The meta key of an attempt's model: `attempt.model:<project>:<task>:<n>`
/// (T-701's `crate::tasks::attempt_model_key`, kept here until it lands).
fn attempt_model_key(project: &str, task: &str, n: i64) -> String {
    format!("attempt.model:{project}:{task}:{n}")
}

/// The model an attempt ran on: its `attempt.model:` meta, else the seat's
/// current `engine.model.<seat>`, else none (the attempt is skipped).
fn model_of(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    n: i64,
    seat: &str,
) -> GearResult<Option<String>> {
    if let Some(model) = tx
        .get_meta(&attempt_model_key(project, task, n))?
        .filter(|model| !model.is_empty())
    {
        return Ok(Some(model));
    }
    Ok(tx
        .get_meta(&format!("engine.model.{seat}"))?
        .filter(|model| !model.is_empty()))
}

/// The estimate of an attempt: the task's `estimate_min` for its first
/// attempt, `rework_estimate_min` for a later one.
fn estimate_of(task: &gear_store::TaskRow, n: i64) -> Option<i64> {
    if n == 1 {
        task.estimate_min
    } else {
        task.rework_estimate_min
    }
}

/// Add one measured attempt to its model: the work time always, the ratio
/// only when the plan gave an estimate.
fn add(
    by_model: &mut BTreeMap<String, (Vec<i64>, Vec<f64>)>,
    model: String,
    work_min: i64,
    estimate_min: Option<i64>,
) {
    let entry = by_model.entry(model).or_default();
    entry.0.push(work_min);
    if let Some(estimate) = estimate_min.filter(|estimate| *estimate > 0) {
        entry.1.push(work_min as f64 / estimate as f64);
    }
}

/// The median of the work times, in minutes.
fn median_minutes(work: &[i64]) -> u32 {
    let mut sorted: Vec<i64> = work.iter().copied().filter(|min| *min >= 0).collect();
    if sorted.is_empty() {
        return 0;
    }
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    let median = if sorted.len() % 2 == 1 {
        sorted[middle] as f64
    } else {
        (sorted[middle - 1] + sorted[middle]) as f64 / 2.0
    };
    median.round() as u32
}

/// The median of the ratios; 0 when there is none.
fn median_ratio(ratios: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = ratios.iter().copied().filter(|r| r.is_finite()).collect();
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    }
}

/// The estimate calibrated to `model` (M7.3): `estimate_min × median_ratio`,
/// rounded, only when that model has at least 5 samples with a ratio.
pub fn calibrated(speeds: &[ModelSpeed], model: &str, estimate_min: u32) -> Option<u32> {
    let speed = speeds.iter().find(|speed| speed.model == model)?;
    if speed.samples < 5 || speed.median_ratio <= 0.0 {
        return None;
    }
    Some((estimate_min as f64 * speed.median_ratio).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::{AttemptState, SeatRole};
    use gear_store::{testing, AttemptRow, AttemptStep, Store};

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    fn world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", NOW)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    SeatRole::Lane,
                    "w1:p2",
                    "opencode",
                    NOW,
                )?;
                Ok(())
            })
            .unwrap();
        store
    }

    fn task(
        id: &str,
        lane: Option<&str>,
        state: TaskState,
        estimate: Option<i64>,
    ) -> gear_store::TaskRow {
        gear_store::TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: id.into(),
            rank: 1,
            state,
            priority: "must".into(),
            lane: lane.map(str::to_string),
            milestone: Some("M1".into()),
            required: false,
            contract_version: 1,
            deps: vec![],
            followup_of: None,
            wait_on: None,
            wait_cond: None,
            rework_note: None,
            estimate_min: estimate,
            issue: None,
            updated: NOW,
        }
    }

    /// An attempt `n` of `task` on `seat`, handed in after `work_min` minutes
    /// of work, with `model` recorded for it.
    fn attempt(store: &mut Store, task: &str, n: i64, seat: &str, work_min: i64, model: &str) {
        store
            .write(|tx| {
                tx.insert_attempt(&AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: task.into(),
                    n,
                    seat: seat.into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Accepted,
                    candidate: Some(format!("c{n}")),
                    report: None,
                    safe_point: None,
                    started: NOW - DAY,
                    updated: NOW - DAY,
                })?;
                tx.set_attempt_time("p", task, n, AttemptStep::Started, NOW - DAY)?;
                tx.set_attempt_time(
                    "p",
                    task,
                    n,
                    AttemptStep::HandedIn,
                    NOW - DAY + work_min * 60,
                )?;
                tx.set_meta(&attempt_model_key("p", task, n), model)
            })
            .unwrap();
    }

    #[test]
    fn three_models_are_measured_with_their_samples_medians_and_ratio() {
        let mut store = world();
        store
            .write(|tx| {
                tx.insert_task(&task("T-1", Some("heat"), TaskState::Done, Some(10)))?;
                tx.insert_task(&task("T-2", Some("heat"), TaskState::Done, Some(20)))?;
                tx.insert_task(&task("T-3", Some("heat"), TaskState::Done, Some(10)))?;
                tx.insert_task(&task("T-4", Some("heat"), TaskState::Done, Some(5)))?;
                tx.insert_task(&task("T-5", Some("heat"), TaskState::Done, Some(5)))?;
                tx.insert_task(&task("T-6", Some("heat"), TaskState::Done, Some(5)))?;
                Ok(())
            })
            .unwrap();
        attempt(&mut store, "T-1", 1, "p:heat", 10, "model-a");
        attempt(&mut store, "T-2", 1, "p:heat", 20, "model-a");
        attempt(&mut store, "T-3", 1, "p:heat", 30, "model-b");
        attempt(&mut store, "T-4", 1, "p:heat", 5, "model-c");
        attempt(&mut store, "T-5", 1, "p:heat", 5, "model-c");
        attempt(&mut store, "T-6", 1, "p:heat", 5, "model-c");

        let speeds = store.read(|tx| measured(tx, NOW)).unwrap();
        assert_eq!(
            speeds.iter().map(|s| s.model.as_str()).collect::<Vec<_>>(),
            vec!["model-c", "model-a", "model-b"],
            "sorted by samples, most first"
        );
        let c = &speeds[0];
        assert_eq!((c.samples, c.median_work_min), (3, 5));
        assert!((c.median_ratio - 1.0).abs() < 1e-9);
        let a = &speeds[1];
        assert_eq!((a.samples, a.median_work_min), (2, 15));
        assert!((a.median_ratio - 1.0).abs() < 1e-9);
        let b = &speeds[2];
        assert_eq!((b.samples, b.median_work_min), (1, 30));
        assert!((b.median_ratio - 3.0).abs() < 1e-9);
    }

    #[test]
    fn an_attempt_without_an_estimate_counts_for_the_work_time_only() {
        let mut store = world();
        store
            .write(|tx| tx.insert_task(&task("T-1", Some("heat"), TaskState::Done, None)))
            .unwrap();
        attempt(&mut store, "T-1", 1, "p:heat", 12, "model-a");
        let speeds = store.read(|tx| measured(tx, NOW)).unwrap();
        assert_eq!(speeds.len(), 1);
        assert_eq!((speeds[0].samples, speeds[0].median_work_min), (1, 12));
        assert_eq!(speeds[0].median_ratio, 0.0);
    }

    #[test]
    fn a_captain_alone_voyages_done_tasks_are_measured_from_merge_to_merge() {
        let mut store = world();
        store
            .write(|tx| {
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p1",
                    "claude",
                    NOW,
                )?;
                tx.upsert_voyage(&gear_store::VoyageRow {
                    project: "p".into(),
                    milestone: "M1".into(),
                    title: "v".into(),
                    done: Some("done".into()),
                    bar: Some("FASTEST".into()),
                    eta: Some("4h".into()),
                    departure: Some(NOW - DAY),
                    departure_tasks: Some(2),
                    departure_estimate_min: Some(60),
                    updated: NOW - DAY,
                })?;
                tx.insert_task(&task("T-1", None, TaskState::Done, Some(10)))?;
                tx.insert_task(&task("T-2", None, TaskState::Done, Some(10)))?;
                Ok(())
            })
            .unwrap();
        // The first captain-done task: 10 minutes after the departure; the
        // second: 10 minutes after the first.
        for (task, n, merged) in [("T-1", 1, NOW - DAY + 600), ("T-2", 1, NOW - DAY + 1200)] {
            store
                .write(|tx| {
                    tx.insert_attempt(&AttemptRow {
                        session_ref: None,
                        session_cli: None,
                        session_action: None,
                        sent_at: None,
                        project: "p".into(),
                        task: task.into(),
                        n,
                        seat: "p:captain".into(),
                        generation: 1,
                        contract_version: 1,
                        adopted_version: 1,
                        pending_update: None,
                        state: AttemptState::Accepted,
                        candidate: Some(format!("c{task}")),
                        report: None,
                        safe_point: None,
                        started: merged,
                        updated: merged,
                    })?;
                    tx.set_attempt_time("p", task, n, AttemptStep::Merged, merged)?;
                    tx.set_meta(&attempt_model_key("p", task, n), "claude-opus")
                })
                .unwrap();
        }

        let speeds = store.read(|tx| measured(tx, NOW)).unwrap();
        assert_eq!(speeds.len(), 1, "{speeds:?}");
        assert_eq!(speeds[0].model, "claude-opus");
        assert_eq!((speeds[0].samples, speeds[0].median_work_min), (2, 10));
        assert!((speeds[0].median_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn calibrated_is_none_under_five_samples_and_scales_at_five() {
        let four = ModelSpeed {
            model: "m".into(),
            samples: 4,
            median_work_min: 10,
            median_ratio: 1.5,
        };
        assert_eq!(calibrated(&[four.clone()], "m", 10), None);
        let five = ModelSpeed { samples: 5, ..four };
        assert_eq!(calibrated(&[five], "m", 10), Some(15));
        assert_eq!(
            calibrated(&[], "m", 10),
            None,
            "an unknown model is not calibrated"
        );
    }
}
