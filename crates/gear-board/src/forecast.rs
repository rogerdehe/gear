//! Each lane's queue, played forward (T-349, M5 S4; the board's ENDS and
//! TO HANDLE columns).
//!
//! A lane is free again when it hands a task in; its next task starts when the
//! lane is free *and* every task it depends on has merged (hand-in plus review
//! and merge); the gap between the two is waiting. Durations are the estimate
//! left times the lane's measured pace. So the board can say when each lane
//! ends and how long it will sit idle.
//!
//! This is the prototype's `fn forecast` (gear/voyages/M5/board/prototype.py,
//! lines 282-316) and nothing else. The engine's run-dry forecast
//! (`crates/gear-engine/src/feed.rs`) is not this: it multiplies a ready count
//! by a lane's mean seconds per attempt and knows no dependency — and the
//! engine depends on this crate, so the board cannot call it anyway.

use std::collections::BTreeMap;

use gear_model::{GearResult, TaskState};
use gear_store::Tx;

use crate::graph::Stage;

/// How long a review and the merge take when the voyage measured nothing
/// (minutes): the prototype's `REVIEW_MERGE`.
pub const REVIEW_MERGE_MIN: i64 = 15;

/// A dependency no job of the queue speaks for: it never merges (minutes), the
/// prototype's `10 ** 6`.
const NO_DEP: i64 = 1_000_000;

/// The fewest minutes a job is given, whatever its estimate and time say.
const LEAST_LEFT: i64 = 10;

/// How many times the lanes are played forward: dependencies cross lanes, so
/// one pass cannot see a lane that comes later (the prototype's `range(4)`).
const PASSES: usize = 4;

/// One job of a lane's queue, as the board's model has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// `T-<n>`.
    pub id: String,
    /// The lane the job sits on.
    pub lane: String,
    /// Where the job stands (§8.1's stage); only its stage decides what the
    /// forecast does with it.
    pub stage: Stage,
    /// The estimate in minutes; 0 when unknown.
    pub est_min: i64,
    /// The time it took in minutes; 0 when unknown.
    pub actual_min: i64,
    /// The tasks it depends on.
    pub deps: Vec<String>,
}

/// One lane at the end of its queue: when it ends, what it waits on the way
/// there and how fast it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneEnd {
    /// The lane's name.
    pub lane: String,
    /// When the lane is done with its queue: minutes from now.
    pub end_min: i64,
    /// The idle stretches on the way there: how many minutes, and the task it
    /// waits to merge.
    pub waits: Vec<(i64, String)>,
    /// The lane's measured pace (actual minutes over estimate minutes of the
    /// jobs it merged), between 0.5 and 3.0; 1.0 when it merged nothing.
    pub pace: f64,
}

/// Each lane's queue played forward: one [`LaneEnd`] per lane, in `lanes`
/// order.
///
/// `review_merge_min` is how long a hand-in needs to reach the integration
/// branch; [`review_merge_min`] measures it for a voyage.
pub fn forecast(jobs: &[Job], lanes: &[String], review_merge_min: i64) -> Vec<LaneEnd> {
    let paces: Vec<f64> = lanes.iter().map(|lane| pace_of(jobs, lane)).collect();
    // When each job is on the integration branch, in minutes from now. The
    // jobs already on their way there know it; the rest learn it as their lane
    // plays its queue forward.
    let mut merged: BTreeMap<String, i64> = BTreeMap::new();
    for job in jobs {
        match job.stage {
            // Already on the integration branch.
            Stage::Merged => merged.insert(job.id.clone(), 0),
            // Handed in: it merges after review and merge.
            Stage::Handed | Stage::Review | Stage::Verdict => {
                merged.insert(job.id.clone(), review_merge_min)
            }
            // Passed: only the merge is left.
            Stage::Passed | Stage::Merging => merged.insert(job.id.clone(), 5),
            _ => None,
        };
    }
    let mut ends: Vec<LaneEnd> = lanes
        .iter()
        .map(|lane| LaneEnd {
            lane: lane.clone(),
            end_min: 0,
            waits: Vec::new(),
            pace: 1.0,
        })
        .collect();
    for _ in 0..PASSES {
        for (i, lane) in lanes.iter().enumerate() {
            let pace = paces[i];
            let mut free = 0;
            let mut waits: Vec<(i64, String)> = Vec::new();
            let mut last = 0;
            // What the lane holds now, then what it has still to start: each
            // group in queue order.
            let holds = jobs.iter().filter(|job| {
                job.lane == *lane
                    && matches!(
                        job.stage,
                        Stage::Working
                            | Stage::Asking
                            | Stage::Delivered
                            | Stage::Rework
                            | Stage::SentBack
                    )
            });
            let waits_to_start = jobs.iter().filter(|job| {
                job.lane == *lane && matches!(job.stage, Stage::Queued | Stage::Deps)
            });
            for job in holds.chain(waits_to_start) {
                let mut start = free;
                if matches!(job.stage, Stage::Queued | Stage::Deps) {
                    let (ready, blocker) = ready(&merged, &job.deps);
                    if ready > free {
                        if let Some(blocker) = blocker {
                            waits.push((ready - free, blocker));
                        }
                        start = ready;
                    }
                }
                let left = if job.actual_min > 0 {
                    (job.est_min - job.actual_min).max(LEAST_LEFT)
                } else {
                    job.est_min
                };
                free = start + (left as f64 * pace).floor() as i64;
                merged.insert(job.id.clone(), free + review_merge_min);
                last = free + review_merge_min;
            }
            // A task already with a reviewer or a merger ends the lane too.
            let reviewing = jobs
                .iter()
                .filter(|job| {
                    job.lane == *lane
                        && matches!(
                            job.stage,
                            Stage::Handed
                                | Stage::Review
                                | Stage::Verdict
                                | Stage::Passed
                                | Stage::Merging
                        )
                })
                .filter_map(|job| merged.get(&job.id).copied())
                .max();
            ends[i] = LaneEnd {
                lane: lane.clone(),
                end_min: last.max(reviewing.unwrap_or(last)),
                waits,
                pace,
            };
        }
    }
    ends
}

/// The lane's measured pace: the minutes its merged jobs took over the minutes
/// they were estimated at, between 0.5 and 3.0; 1.0 when it merged nothing (so
/// a lane whose estimate was 0 or missing cannot divide by it).
fn pace_of(jobs: &[Job], lane: &str) -> f64 {
    let merged: Vec<&Job> = jobs
        .iter()
        .filter(|job| job.lane == lane && job.stage == Stage::Merged)
        .collect();
    if merged.is_empty() {
        return 1.0;
    }
    let actual: i64 = merged.iter().map(|job| job.actual_min).sum();
    let est: i64 = merged.iter().map(|job| job.est_min).sum();
    (actual as f64 / est.max(1) as f64).clamp(0.5, 3.0)
}

/// When the job can start because of what it depends on: the latest merge
/// among its dependencies, and the first dependency that merges that late, so
/// the board can name the one it waits on.
///
/// A dependency no job speaks for never merges ([`NO_DEP`]); a job with no
/// dependency is ready at once.
fn ready(merged: &BTreeMap<String, i64>, deps: &[String]) -> (i64, Option<String>) {
    let mut ready = 0;
    let mut blocker: Option<String> = None;
    for dep in deps.iter().filter(|dep| !dep.is_empty()) {
        let at = merged.get(dep).copied().unwrap_or(NO_DEP);
        // The first dependency with the latest merge names the wait; a tie
        // keeps the one already found.
        if blocker.is_none() || at > ready {
            ready = at;
            blocker = Some(dep.clone());
        }
    }
    (ready, blocker)
}

/// How long a hand-in takes to reach the integration branch, in whole minutes,
/// measured over the voyage's merged tasks: the mean of their newest attempt's
/// merge time minus its hand-in time. [`REVIEW_MERGE_MIN`] when no task of the
/// voyage has both times.
pub fn review_merge_min(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<i64> {
    let mut total = 0;
    let mut count = 0;
    for task in tx.tasks_of_project(project)? {
        if task.state != TaskState::Done || task.milestone.as_deref() != Some(milestone) {
            continue;
        }
        // The newest attempt that was both handed in and merged: a task that
        // was reworked has earlier attempts that never reached the branch.
        let newest = tx
            .attempt_times_of_task(project, &task.id)?
            .into_iter()
            .filter(|(_, times)| times.handed_in.is_some() && times.merged.is_some())
            .max_by_key(|(n, _)| *n);
        let Some((_, times)) = newest else { continue };
        total += times.merged.unwrap_or_default() - times.handed_in.unwrap_or_default();
        count += 1;
    }
    if count == 0 {
        return Ok(REVIEW_MERGE_MIN);
    }
    Ok((total / count) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The twelve jobs of the prototype's M5 voyage (`VOYS[0]["T"]`), with the
    /// stages the prototype's stage names stand for.
    fn m5() -> Vec<Job> {
        let job = |id: &str, lane: &str, stage: Stage, est: i64, actual: i64, deps: &[&str]| Job {
            id: id.to_string(),
            lane: lane.to_string(),
            stage,
            est_min: est,
            actual_min: actual,
            deps: deps.iter().map(|dep| dep.to_string()).collect(),
        };
        vec![
            job("T-257", "hull", Stage::Merged, 20, 18, &[]),
            job("T-258", "mast", Stage::Merged, 40, 44, &[]),
            job("T-259", "keel", Stage::Merged, 30, 27, &[]),
            job("T-260", "hull", Stage::Merged, 30, 41, &["T-257"]),
            job("T-261", "mast", Stage::Merged, 45, 52, &["T-258"]),
            job("T-262", "keel", Stage::Review, 40, 35, &[]),
            job("T-263", "hull", Stage::Rework, 25, 49, &["T-260"]),
            job("T-264", "mast", Stage::Working, 40, 22, &["T-261"]),
            job("T-268", "keel", Stage::Passed, 15, 12, &[]),
            job("T-265", "keel", Stage::Deps, 35, 0, &["T-262"]),
            job("T-266", "mast", Stage::Queued, 60, 0, &["T-261"]),
            job("T-267", "hull", Stage::Queued, 15, 0, &["T-263", "T-266"]),
        ]
    }

    fn lane<'a>(ends: &'a [LaneEnd], name: &str) -> &'a LaneEnd {
        ends.iter()
            .find(|end| end.lane == name)
            .unwrap_or_else(|| panic!("no lane {name}"))
    }

    #[test]
    fn the_m5_voyage_ends_when_the_prototype_says() {
        let lanes = vec!["hull".to_string(), "mast".to_string(), "keel".to_string()];
        let ends = forecast(&m5(), &lanes, REVIEW_MERGE_MIN);

        // One per lane, in the lanes' order.
        assert_eq!(
            ends.iter().map(|end| end.lane.as_str()).collect::<Vec<_>>(),
            vec!["hull", "mast", "keel"]
        );

        // hull: its rework takes 49 of 25 minutes at pace 1.18, and its last
        // task waits 91 minutes for mast's T-266 to merge.
        let hull = lane(&ends, "hull");
        assert_eq!(hull.end_min, 134);
        assert_eq!(hull.waits, vec![(91, "T-266".to_string())]);
        assert!(
            (hull.pace - 1.18).abs() < 1e-9,
            "hull's pace is {}",
            hull.pace
        );

        // mast: nothing blocks it, and its pace is 96 minutes over 85.
        let mast = lane(&ends, "mast");
        assert_eq!(mast.end_min, 102);
        assert!(mast.waits.is_empty(), "mast waits on {:?}", mast.waits);
        assert!(
            (mast.pace - 96.0 / 85.0).abs() < 1e-9,
            "mast's pace is {}",
            mast.pace
        );

        // keel: T-262 is in review, so T-265 waits 15 minutes for it.
        let keel = lane(&ends, "keel");
        assert_eq!(keel.end_min, 61);
        assert_eq!(keel.waits, vec![(15, "T-262".to_string())]);
        assert!(
            (keel.pace - 0.9).abs() < 1e-9,
            "keel's pace is {}",
            keel.pace
        );
    }

    #[test]
    fn a_lane_with_nothing_planned_ends_at_once() {
        // A lane no job speaks for ends at once, whatever the others do.
        let ends = forecast(
            &m5(),
            &["hull".to_string(), "temp".to_string()],
            REVIEW_MERGE_MIN,
        );
        assert_eq!(
            ends[1],
            LaneEnd {
                lane: "temp".to_string(),
                end_min: 0,
                waits: Vec::new(),
                pace: 1.0,
            }
        );
        // One end per lane and no more: mast's jobs are not hull's.
        assert_eq!(ends.len(), 2);
    }

    #[test]
    fn a_dependency_nobody_speaks_for_never_merges() {
        let jobs = vec![Job {
            id: "T-1".to_string(),
            lane: "hull".to_string(),
            stage: Stage::Queued,
            est_min: 10,
            actual_min: 0,
            deps: vec!["T-404".to_string()],
        }];
        let ends = forecast(&jobs, &["hull".to_string()], REVIEW_MERGE_MIN);
        assert_eq!(ends[0].waits, vec![(NO_DEP, "T-404".to_string())]);
        assert_eq!(ends[0].end_min, NO_DEP + 10 + REVIEW_MERGE_MIN);
    }

    #[test]
    fn the_pace_stays_between_a_half_and_three() {
        let job = |id: &str, est: i64, actual: i64| Job {
            id: id.to_string(),
            lane: "hull".to_string(),
            stage: Stage::Merged,
            est_min: est,
            actual_min: actual,
            deps: Vec::new(),
        };
        // 300 of 10 minutes would be a pace of 30; the clamp holds it at 3.
        assert_eq!(
            forecast(&[job("T-1", 10, 300)], &["hull".to_string()], 15)[0].pace,
            3.0
        );
        // 1 of 300 would be 0.003; the clamp holds it at 0.5.
        assert_eq!(
            forecast(&[job("T-1", 300, 1)], &["hull".to_string()], 15)[0].pace,
            0.5
        );
        // A lane whose estimates are 0 divides by one, not by zero.
        assert_eq!(
            forecast(&[job("T-1", 0, 4)], &["hull".to_string()], 15)[0].pace,
            3.0
        );
    }
}
