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
        match release_one(store, herdr, project, &seat, &task, now) {
            Ok(true) => report.released.push(seat.id.clone()),
            Ok(false) => {}
            // One failing reviewer does not stop the others.
            Err(_) => report.failed.push(seat.id.clone()),
        }
    }
    Ok(report)
}

/// Whether the reviewer still has work: its task is `Verifying`, its attempt is
/// the one the reviewer was started for, that attempt is `Submitted`, and the
/// reviewer has not handed in its verdict yet.
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
        // Its verdict is in: the reviewer's job is done, whoever settles it
        // and whenever (L-58) — the slot is not held for the settlement.
        if tx.get_review_verdict(project, task, latest.n)?.is_some() {
            return Ok(false);
        }
        Ok(true)
    })
}

/// Release one reviewer. `Ok(false)` = nothing to do (it is waiting for its
/// worktree to go), `Err` = the step failed and a human should look.
fn release_one(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    seat: &SeatRow,
    task: &str,
    now: Ts,
) -> Result<bool, String> {
    let pane = binding_pane(store, seat)?;

    // 1. the agent leaves, once: its pane is closed. Exit keys differ per
    //    agent and fail quietly (codex needs two ctrl+c, grok only `/exit`,
    //    L-63); closing the pane ends any agent. The worktree stays — the next
    //    job reopens it. A pane already gone is fine.
    if seat.state == SeatState::Active {
        if let Some(pane) = pane.as_deref() {
            // A finished reviewer's pane is always closed (herdr `pane_close`),
            // grok included: a reviewer lives for one job, there is no session
            // to resume, and a closed pane cannot leave a stuck slot.
            match herdr.pane_close(pane) {
                Outcome::Ok(()) => {}
                Outcome::Refused { code, .. } if code == "pane_not_found" => {}
                Outcome::Refused { code, message } => {
                    return Err(format!("pane_close: refused ({code}): {message}"))
                }
                Outcome::Uncertain { reason } => return Err(format!("pane_close: {reason}")),
            }
        }
        store
            .write(|tx| tx.set_seat_state(&seat.id, SeatState::Releasing, Some("review done"), now))
            .map_err(|e| e.detail)?;
        return Ok(false);
    }

    // 2. the old agent has to be *gone* before the seat is free: an exit key
    //    is accepted long before the process leaves, and a seat that is freed
    //    too early hands its pane — and its worktree — to the next job while
    //    the old reviewer is still running in it (F5 of the T-18 review).
    //    An observation failure is not an absence (design/08): the seat stays
    //    `Releasing` and the next tick looks again.
    if let Some(pane) = pane.as_deref() {
        match herdr.agent_get(pane) {
            Ok(None) => {
                // The pane is a shell again: the seat is free.
            }
            Ok(Some(_)) => {
                // The old reviewer is still there (its exit was lost, or an
                // older engine sent exit keys it ignored): close its pane
                // again; the next tick looks once more.
                if let Outcome::Refused { code, message } = herdr.pane_close(pane) {
                    if code != "pane_not_found" {
                        return Err(format!("pane_close: refused ({code}): {message}"));
                    }
                }
                return Ok(false);
            }
            Err(error) => {
                // The pane could not be looked at: never call that "gone". The
                // seat stays on its way out and the next tick looks again.
                let _ = store.write(|tx| {
                    tx.set_seat_state(
                        &seat.id,
                        SeatState::Releasing,
                        Some(&format!("agent_get failed: {}", error.0)),
                        now,
                    )
                });
                return Ok(false);
            }
        }
    }

    // 3. the seat is free again. Its worktree and its build cache stay: the
    //    next job on this seat reopens them (conventions 17 §8a), so a review
    //    never builds the workspace from nothing and a release removes nothing.
    //    The closed pane's binding ends here, so the next job opens a new pane.
    store
        .write(|tx| {
            if let Some(binding) = tx.get_binding(&seat.id, seat.generation)? {
                if binding.status != BindingStatus::Void {
                    tx.set_binding_status(&seat.id, seat.generation, BindingStatus::Void)?;
                }
            }
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

    /// A project with one task in `state` and a reviewer bound to it on `pane`.
    fn world(state: TaskState, pane: &str, profile: &str) -> Store {
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
                    estimate_min: None,
                    updated: T0,
                })?;
                testing::seed_temp_seat(tx, "p", "r-1", "T-1", pane, profile, T0)?;
                Ok(())
            })
            .unwrap();
        store
    }

    fn exits(herdr: &FakeHerdr) -> usize {
        herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::PaneClose { .. }))
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
        let mut store = world(TaskState::Cancelled, &worktree.pane_id, "codex");

        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        // Nothing is released on the tick that closes the pane: the next tick
        // frees the seat, so a restart never closes it twice.
        assert!(report.released.is_empty(), "{report:?}");
        assert_eq!(exits(&herdr), 1, "the pane is closed once");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Releasing);

        // The pane is gone: the seat is free, its binding ends (the next job
        // opens a new pane on the same worktree).
        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 30).unwrap();

        assert_eq!(report.released, vec!["p:r-1".to_string()]);
        assert!(removes(&herdr).is_empty(), "the seat's worktree is kept (T-18)");
        assert_eq!(exits(&herdr), 1, "the close is not repeated");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Released, "the slot is free");
        let binding = store.read(|tx| tx.get_binding("p:r-1", seat.generation)).unwrap().unwrap();
        assert_eq!(binding.status, BindingStatus::Void, "the closed pane is not reused");
        // A further tick does nothing.
        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 40).unwrap();
        assert!(report.released.is_empty());
    }

    #[test]
    fn a_restart_between_exit_and_release_finishes_without_a_second_exit() {
        let mut herdr = FakeHerdr::new();
        let worktree = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        herdr.add_agent(&worktree.pane_id, "codex", gear_herdr::AgentStatus::Idle);
        let mut store = world(TaskState::Cancelled, &worktree.pane_id, "codex");
        // The engine died right after the exit keys were sent.
        store
            .write(|tx| tx.set_seat_state("p:r-1", SeatState::Releasing, Some("review done"), T0 + 5))
            .unwrap();

        // The agent is still there (an older engine's exit keys were
        // ignored): its pane is closed, the seat waits for the next look.
        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();
        assert!(report.released.is_empty(), "{report:?}");
        assert_eq!(exits(&herdr), 1, "the pane is closed");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Releasing);

        // The pane is gone: the seat is free.
        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 20).unwrap();

        assert_eq!(report.released, vec!["p:r-1".to_string()]);
        assert_eq!(exits(&herdr), 1, "no second close");
        assert!(removes(&herdr).is_empty(), "the seat's worktree is kept (T-18)");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Released);
    }

    #[test]
    fn an_observation_failure_is_not_an_absence() {
        // design/08: a pane that cannot be read is not a pane without an
        // agent. The seat stays on its way out and the next tick looks again.
        let mut herdr = FakeHerdr::new();
        let worktree = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        herdr.add_agent(&worktree.pane_id, "codex", gear_herdr::AgentStatus::Idle);
        let mut store = world(TaskState::Cancelled, &worktree.pane_id, "codex");
        store
            .write(|tx| tx.set_seat_state("p:r-1", SeatState::Releasing, Some("review done"), T0 + 5))
            .unwrap();
        herdr.observe_fails = true;

        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        assert!(report.released.is_empty(), "{report:?}");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Releasing, "not released on a guess");
    }

    #[test]
    fn two_reviewers_are_released_one_by_one() {
        let mut herdr = FakeHerdr::new();
        let first = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        let second = match herdr.worktree_create("/repo", "review/t2", "abc1234", "p-t2") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        herdr.add_agent(&first.pane_id, "codex", gear_herdr::AgentStatus::Idle);
        herdr.add_agent(&second.pane_id, "codex", gear_herdr::AgentStatus::Idle);
        let mut store = world(TaskState::Cancelled, &first.pane_id, "codex");
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
                    estimate_min: None,
                    updated: T0,
                })?;
                testing::seed_temp_seat(tx, "p", "r-2", "T-2", &second.pane_id, "codex", T0)?;
                // Both already sent their exit keys.
                tx.set_seat_state("p:r-1", SeatState::Releasing, Some("review done"), T0 + 5)?;
                tx.set_seat_state("p:r-2", SeatState::Releasing, Some("review done"), T0 + 5)
            })
            .unwrap();

        // Only the first reviewer's agent has left: the other seat waits.
        herdr.exit_agent(&first.pane_id);
        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        assert_eq!(report.released, vec!["p:r-1".to_string()], "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(removes(&herdr).is_empty(), "no seat's worktree is removed (T-18)");
        let first_seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(first_seat.state, SeatState::Released);
        let second_seat = store.read(|tx| tx.get_seat("p:r-2")).unwrap().unwrap();
        assert_eq!(second_seat.state, SeatState::Releasing, "its agent was still there");
        assert_eq!(exits(&herdr), 1, "its pane is closed");

        // Its pane is gone: it is released too.
        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 20).unwrap();
        assert_eq!(report.released, vec!["p:r-2".to_string()], "{report:?}");
        let second_seat = store.read(|tx| tx.get_seat("p:r-2")).unwrap().unwrap();
        assert_eq!(second_seat.state, SeatState::Released);
    }

    #[test]
    fn a_grok_reviewers_pane_is_closed() {
        // T-32 rework: a finished reviewer's pane is closed whatever its CLI
        // is — grok included. A reviewer lives for one job, there is no session
        // to resume, and a closed pane cannot leave a stuck slot.
        let mut herdr = FakeHerdr::new();
        let worktree = match herdr.worktree_create("/repo", "review/t1", "abc1234", "p-t1") {
            Outcome::Ok(worktree) => worktree,
            other => panic!("{other:?}"),
        };
        herdr.add_agent(&worktree.pane_id, "grok", gear_herdr::AgentStatus::Idle);
        let mut store = world(TaskState::Cancelled, &worktree.pane_id, "grok");

        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        assert!(report.released.is_empty(), "{report:?}");
        assert!(
            herdr.calls.iter().any(|call| matches!(call, Call::PaneClose { pane } if pane == &worktree.pane_id)),
            "the pane is closed: {:?}",
            herdr.calls
        );
        assert!(herdr.prompts_to(&worktree.pane_id).is_empty(), "no exit command is sent");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Releasing);
    }

    #[test]
    fn a_queued_or_starting_reviewer_is_never_released() {
        // R1 of the review: a reviewer waiting for a slot (Queued) or one that
        // is starting holds no worktree, and releasing it would throw away the
        // review it is about to do — even when its task is already cancelled.
        let mut herdr = FakeHerdr::new();
        let mut store = world(TaskState::Cancelled, "w1:p1", "codex");
        for state in [SeatState::Queued, SeatState::Starting] {
            store
                .write(|tx| tx.set_seat_state("p:r-1", state, Some("waiting"), T0))
                .unwrap();
            let report =
                release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();
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
        let mut store = world(TaskState::Verifying, &worktree.pane_id, "codex");
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

        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        assert!(report.released.is_empty());
        assert_eq!(exits(&herdr), 0);
        assert!(removes(&herdr).is_empty());
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);

        // Its verdict comes in while the task still waits for settlement: the
        // reviewer's job is done and it leaves at once (L-58).
        store
            .write(|tx| {
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    passed: true,
                    findings: "ok".into(),
                    by_seat: "p:r-1".into(),
                    created: T0 + 20,
                })
            })
            .unwrap();
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 30).unwrap();
        assert_eq!(exits(&herdr), 1, "the reviewer is told to exit once its verdict is in");
    }
}
