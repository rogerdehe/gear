//! Releasing a review temp whose task is no longer under review (design/10,
//! plan T-17, from the T-5 review's R2-2).
//!
//! A temp reviewer holds a quota slot for as long as it lives. When the task it
//! was started for is no longer under review — cancelled, settled, or a newer
//! attempt was handed in — the reviewer is released on the next tick: its agent
//! exits (`Profile::exit_keys`), its worktree is removed through Herdr (the
//! branch is kept), and the slot is free again (`Released` does not occupy the
//! quota).
//!
//! The release is two steps so a restart cannot repeat the exit: the exit keys
//! put the seat in `Releasing`, and only a seat already in `Releasing` — or one
//! that never had an agent — is removed. One failing release is recorded and
//! the loop carries on with the next reviewer.

use gear_herdr::{HerdrAct, Outcome, Profile};
use gear_model::{AttemptState, GearResult, OpKind, SeatRole, SeatState, TaskState, Ts};
use gear_store::{BindingStatus, SeatRow, Store};

use crate::EngineCfg;

/// What the release step did in one tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseReport {
    /// The reviewer seats that were exited and removed.
    pub released: Vec<String>,
    /// The reviewer seats that could not be released (a human decides).
    pub failed: Vec<String>,
}

/// Release every reviewer of `project` whose task is no longer under review.
pub fn release_finished_reviews(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    project: &str,
    now: Ts,
) -> GearResult<ReleaseReport> {
    let mut report = ReleaseReport::default();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        if seat.role != SeatRole::Temp {
            continue;
        }
        let Some(task) = seat.task_id.clone() else {
            continue;
        };
        // Only a reviewer that is up, or one already on its way out, is
        // released. A Queued or Starting reviewer has no slot and no worktree
        // yet — releasing it would throw away the review it is waiting to do
        // (R1 of the T-17 review) — and a seat a human has to look at is left
        // alone.
        if !matches!(seat.state, SeatState::Active | SeatState::Releasing) {
            continue;
        }
        // Still doing its job: nothing to release.
        if seat.state == SeatState::Active && under_review(store, project, &seat, &task)? {
            continue;
        }
        match release_one(store, herdr, cfg, project, &seat, &task, now) {
            Ok(true) => report.released.push(seat.id.clone()),
            Ok(false) => {}
            // One failing reviewer does not stop the others.
            Err(_) => report.failed.push(seat.id.clone()),
        }
    }
    Ok(report)
}

/// Whether the reviewer still has work: its task is `Verifying`, its attempt is
/// the one the reviewer was started for, and that attempt is `Submitted`.
fn under_review(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    task: &str,
) -> GearResult<bool> {
    store.read(|tx| {
        let Some(row) = tx.get_task(project, task)? else {
            return Ok(false);
        };
        if row.state != TaskState::Verifying {
            return Ok(false);
        }
        let Some(latest) = tx.attempts_of_task(project, task)?.into_iter().last() else {
            return Ok(false);
        };
        if latest.state != AttemptState::Submitted {
            return Ok(false);
        }
        // A newer attempt handed in means this reviewer read an older one.
        if let Some(job) = tx.last_op(&seat.id, OpKind::Deliver)? {
            if job.attempt != Some(latest.n) {
                return Ok(false);
            }
        }
        Ok(true)
    })
}

/// Release one reviewer. `Ok(false)` = nothing to do (it is waiting for its
/// worktree to go), `Err` = the step failed and a human should look.
fn release_one(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    project: &str,
    seat: &SeatRow,
    task: &str,
    now: Ts,
) -> Result<bool, String> {
    let pane = binding_pane(store, seat)?;
    let profile = cfg.profiles.get(&seat.profile);

    // 1. the agent leaves, once. A seat already in Releasing has sent its keys.
    if seat.state == SeatState::Active {
        if let (Some(pane), Some(profile)) = (pane.as_deref(), profile) {
            for key in &profile.exit_keys {
                match herdr.send_keys(pane, &[key.clone()]) {
                    Outcome::Ok(()) => {}
                    Outcome::Refused { code, message } => {
                        return Err(format!("send_keys: refused ({code}): {message}"))
                    }
                    Outcome::Uncertain { reason } => return Err(format!("send_keys: {reason}")),
                }
            }
        }
        store
            .write(|tx| tx.set_seat_state(&seat.id, SeatState::Releasing, Some("review done"), now))
            .map_err(|e| e.detail)?;
        return Ok(false);
    }

    // 2. the worktree goes (the branch is kept), then the slot is free.
    if let Some(pane) = pane.as_deref() {
        if let Some(workspace) = workspace_of(pane) {
            match herdr.worktree_remove(&workspace) {
                // Already gone (a retry, or a manual removal) is fine.
                Outcome::Ok(()) => {}
                Outcome::Refused { code, .. }
                    if code == "not_found"
                        || code == "worktree_not_found"
                        || code == "workspace_not_found" => {}
                Outcome::Refused { code, message } => {
                    return Err(format!("worktree_remove: refused ({code}): {message}"))
                }
                Outcome::Uncertain { reason } => return Err(format!("worktree_remove: {reason}")),
            }
        }
    }
    store
        .write(|tx| {
            let from = tx.get_seat(&seat.id)?.map(|seat| seat.state).unwrap_or(SeatState::Queued);
            let _ = from;
            tx.set_seat_state(
                &seat.id,
                SeatState::Released,
                Some(&format!("review of {task} done")),
                now,
            )
        })
        .map_err(|e| e.detail)?;
    let _ = project;
    Ok(true)
}

/// The pane of the reviewer's active binding.
fn binding_pane(store: &mut Store, seat: &SeatRow) -> Result<Option<String>, String> {
    let (id, generation) = (seat.id.clone(), seat.generation);
    store
        .read(|tx| {
            Ok(tx
                .get_binding(&id, generation)?
                .filter(|binding| binding.status == BindingStatus::Active)
                .map(|binding| binding.pane_id))
        })
        .map_err(|e| e.detail)
}

/// The Herdr workspace of a pane: its id up to the `:` (`w7:p1` → `w7`).
fn workspace_of(pane: &str) -> Option<String> {
    pane.split(':').next().filter(|id| !id.is_empty()).map(str::to_string)
}

/// The profile of a seat, when the engine knows it.
#[allow(dead_code)]
fn profile_of<'a>(cfg: &'a EngineCfg, seat: &SeatRow) -> Option<&'a Profile> {
    cfg.profiles.get(&seat.profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_store::{testing, ProjectRow, TaskRow};

    const T0: i64 = 1_800_000_000;

    const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 2

[captain]
pane = "w9:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "codex"
profile = "codex"
"#;

    fn cfg() -> EngineCfg {
        let mut cfg = EngineCfg { db_path: "/state/gear.db".into(), ..Default::default() };
        for kind in ["codex", "opencode", "claude"] {
            cfg.profiles.insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        cfg
    }

    /// A project with one task in `state` and a reviewer bound to it on `pane`.
    fn world(state: TaskState, pane: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: CONFIG.into(),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 2,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: T0,
                })?;
                tx.insert_task(&TaskRow {
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    updated: T0,
                })?;
                testing::seed_temp_seat(tx, "p", "r-1", "T-1", pane, "codex", T0)?;
                Ok(())
            })
            .unwrap();
        store
    }

    fn exits(herdr: &FakeHerdr) -> usize {
        herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::SendKeys { .. }))
            .count()
    }

    fn removes(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::WorktreeRemove { workspace } => Some(workspace.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_cancelled_task_releases_its_reviewer() {
        // The review worktree the fake makes: pane wt1:p1, workspace wt1.
        let mut herdr = FakeHerdr::new();
        let worktree = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        herdr.add_agent(&worktree.pane_id, "codex", gear_herdr::AgentStatus::Idle);
        let mut store = world(TaskState::Cancelled, &worktree.pane_id);

        let report = release_finished_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();

        // Nothing is released on the tick that exits the agent: the next tick
        // removes the worktree, so a restart can never send the keys twice.
        assert!(report.released.is_empty(), "{report:?}");
        assert_eq!(exits(&herdr), 1, "one exit");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Releasing);

        let report = release_finished_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 20).unwrap();

        assert_eq!(report.released, vec!["p:r-1".to_string()]);
        assert_eq!(removes(&herdr), vec!["wt1".to_string()]);
        assert_eq!(exits(&herdr), 1, "the exit is not repeated");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Released, "the slot is free");
        // A third tick does nothing.
        let report = release_finished_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 30).unwrap();
        assert!(report.released.is_empty());
        assert_eq!(removes(&herdr).len(), 1);
    }

    #[test]
    fn a_restart_between_exit_and_remove_finishes_without_a_second_exit() {
        let mut herdr = FakeHerdr::new();
        let worktree = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        herdr.add_agent(&worktree.pane_id, "codex", gear_herdr::AgentStatus::Idle);
        let mut store = world(TaskState::Cancelled, &worktree.pane_id);
        // The engine died right after the exit keys were sent.
        store
            .write(|tx| tx.set_seat_state("p:r-1", SeatState::Releasing, Some("review done"), T0 + 5))
            .unwrap();

        let report = release_finished_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();

        assert_eq!(report.released, vec!["p:r-1".to_string()]);
        assert_eq!(exits(&herdr), 0, "no second exit");
        assert_eq!(removes(&herdr), vec!["wt1".to_string()]);
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Released);
    }

    #[test]
    fn a_failing_remove_does_not_stop_the_other_reviews() {
        let mut herdr = FakeHerdr::new();
        let first = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        let second = match herdr.worktree_create("/repo", "review/t2", "abc1234", "p-t2") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        let mut store = world(TaskState::Cancelled, &first.pane_id);
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    project: "p".into(),
                    id: "T-2".into(),
                    rank: 2,
                    state: TaskState::Cancelled,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    updated: T0,
                })?;
                testing::seed_temp_seat(tx, "p", "r-2", "T-2", &second.pane_id, "codex", T0)?;
                // Both already sent their exit keys: the remove is the first call.
                tx.set_seat_state("p:r-1", SeatState::Releasing, Some("review done"), T0 + 5)?;
                tx.set_seat_state("p:r-2", SeatState::Releasing, Some("review done"), T0 + 5)
            })
            .unwrap();
        // The first removal fails; the second must still happen.
        herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse { code: "boom".into() });

        let report = release_finished_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();

        assert_eq!(report.failed, vec!["p:r-1".to_string()], "{report:?}");
        assert_eq!(report.released, vec!["p:r-2".to_string()], "{report:?}");
        let failed = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(failed.state, SeatState::Releasing, "the next tick tries again");
        let released = store.read(|tx| tx.get_seat("p:r-2")).unwrap().unwrap();
        assert_eq!(released.state, SeatState::Released);
    }

    #[test]
    fn a_queued_or_starting_reviewer_is_never_released() {
        // R1 of the review: a reviewer waiting for a slot (Queued) or one that
        // is starting holds no worktree, and releasing it would throw away the
        // review it is about to do — even when its task is already cancelled.
        let mut herdr = FakeHerdr::new();
        let mut store = world(TaskState::Cancelled, "w1:p1");
        for state in [SeatState::Queued, SeatState::Starting] {
            store
                .write(|tx| tx.set_seat_state("p:r-1", state, Some("waiting"), T0))
                .unwrap();
            let report =
                release_finished_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();
            assert!(report.released.is_empty(), "{state:?}: {report:?}");
            assert!(report.failed.is_empty(), "{state:?}: {report:?}");
            assert_eq!(exits(&herdr), 0, "{state:?}: no exit");
            assert!(removes(&herdr).is_empty(), "{state:?}: no removal");
            let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
            assert_eq!(seat.state, state, "left exactly as it was");
        }
    }

    #[test]
    fn a_reviewer_whose_task_is_still_under_review_is_left_alone() {
        let mut herdr = FakeHerdr::new();
        let worktree = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        herdr.add_agent(&worktree.pane_id, "codex", gear_herdr::AgentStatus::Idle);
        // The task is Verifying with a submitted attempt: the reviewer works.
        let mut store = world(TaskState::Verifying, &worktree.pane_id);
        store
            .write(|tx| {
                let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap_or(gear_store::AttemptRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    n: 1,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Submitted,
                    candidate: Some("abc1234".into()),
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                });
                attempt.state = AttemptState::Submitted;
                attempt.candidate = Some("abc1234".into());
                tx.insert_attempt(&attempt)?;
                Ok(())
            })
            .unwrap();

        let report = release_finished_reviews(&mut store, &mut herdr, &cfg(), "p", T0 + 10).unwrap();

        assert!(report.released.is_empty());
        assert_eq!(exits(&herdr), 0);
        assert!(removes(&herdr).is_empty());
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);
    }
}
