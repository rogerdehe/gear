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

use gear_herdr::{HerdrAct, Outcome};
use gear_model::state::require_transition;
use gear_model::{AttemptState, GearResult, OpKind, OpState, SeatRole, SeatState, TaskState, Ts};
use gear_store::{BindingStatus, SeatRow, Store};

/// What the release step did in one tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseReport {
    /// The reviewer seats that were exited and removed.
    pub released: Vec<String>,
    /// The reviewer seats that could not be released (a human decides).
    pub failed: Vec<String>,
}

/// Release every reviewer of `project` whose task is no longer under review.
///
/// The step of the tick that owns the life cycle of a reviewer seat
/// (design §3.6):
///
/// 1. the reviewers whose verdict is in go to `Standby` — the agent stays
///    alive, the pane stays open, the job binding ends;
/// 2. a reviewer whose job is gone (the task was cancelled, a newer attempt
///    was handed in) leaves;
/// 3. a reviewer that is still starting and whose job is gone is cancelled:
///    its start op is voided, its pane is closed, the seat is free;
/// 4. the Standby reviewers that the rule of §3.6 releases are released.
pub fn release_finished_reviews(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    now: Ts,
) -> GearResult<ReleaseReport> {
    let mut report = ReleaseReport::default();

    // The verdict ends the job, not the seat: the reviewer waits for the
    // next one (the rules below apply to it right away).
    crate::review::standby_after_verdict(store, project, now)?;

    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        if seat.role != SeatRole::Temp {
            continue;
        }
        let pending_key = format!("engine.review_pending.{}", seat.id);
        let pending = store.read(|tx| tx.get_meta(&pending_key))?.filter(|task| !task.is_empty());
        if seat.state != SeatState::Starting {
            if let Some(task) = pending.as_deref() {
                if !under_review(store, project, &seat, task)? {
                    store.write(|tx| {
                        tx.set_meta(&pending_key, "")?;
                        if seat.state == SeatState::Queued {
                            tx.set_seat_state(&seat.id, SeatState::Starting, Some("cancelled reservation"), now)?;
                            tx.set_seat_state(&seat.id, SeatState::Released, Some("cancelled reservation"), now)?;
                        }
                        Ok(())
                    })?;
                }
            }
        }
        match seat.state {
            // A reviewer that is on its way out (the captain released it, or
            // the rule of §3.6 did): the pane goes, the binding ends, the
            // seat is free — with a job or without one.
            SeatState::Releasing => {
                match release_one(store, herdr, project, &seat, seat.task_id.as_deref(), now) {
                    Ok(true) => report.released.push(seat.id.clone()),
                    Ok(false) => {}
                    // One failing reviewer does not stop the others.
                    Err(_) => report.failed.push(seat.id.clone()),
                }
            }
            // A reviewer that is up and whose job is gone: it leaves.
            SeatState::Active => {
                let Some(task) = seat.task_id.clone() else {
                    continue;
                };
                // Still doing its job: nothing to release.
                if under_review(store, project, &seat, &task)? {
                    continue;
                }
                match release_one(store, herdr, project, &seat, Some(&task), now) {
                    Ok(true) => report.released.push(seat.id.clone()),
                    Ok(false) => {}
                    Err(_) => report.failed.push(seat.id.clone()),
                }
            }
            // A reviewer that is still starting and whose job is gone: the
            // start is cancelled (design §3.6, B03-05). A Queued reviewer
            // waits for a slot and a seat a human has to look at is left
            // alone.
            SeatState::Starting => {
                let Some(task) = seat.task_id.clone().or(pending) else {
                    continue;
                };
                if under_review(store, project, &seat, &task)? {
                    continue;
                }
                match cancel_start(store, herdr, project, &seat, now) {
                    Ok(true) => report.released.push(seat.id.clone()),
                    Ok(false) => {}
                    Err(_) => report.failed.push(seat.id.clone()),
                }
            }
            _ => {}
        }
    }

    // The Standby reviewers beyond W are released (§3.6).
    let idle = release_idle_standby(store, herdr, project, now)?;
    report.released.extend(idle.released);
    report.failed.extend(idle.failed);
    Ok(report)
}

/// The Standby reviewers that the rule of design §3.6 releases.
///
/// With W = the lane seats that work on a task (an attempt in `Started`), the
/// Standby reviewers beyond W are released — the one idle longest first: it
/// has waited longest for a review that is not coming. With W = 0 every
/// Standby reviewer is released at once: no lane works, so no hand-in can
/// come (a hand-in that does come starts a reviewer again in seconds).
pub fn release_idle_standby(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    now: Ts,
) -> GearResult<ReleaseReport> {
    let mut report = ReleaseReport::default();
    let mut standby: Vec<SeatRow> = store.read(|tx| {
        Ok(tx
            .seats_of_project(project)?
            .into_iter()
            .filter(|seat| seat.role == SeatRole::Temp && seat.state == SeatState::Standby)
            .collect())
    })?;
    if standby.is_empty() {
        return Ok(report);
    }
    // The idle longest goes first: the seat that has been in Standby since
    // the longest.
    standby.sort_by_key(|seat| seat.state_since);
    let working = working_lanes(store, project)?;
    let excess = standby.len().saturating_sub(working.max(0) as usize);
    // The sweep touches only the seats that were already Standby before this
    // tick: a seat that just got its verdict waits for the next review, and
    // it stays until the next tick (the captain's decision of M-2603).
    let waiting: Vec<SeatRow> = standby
        .into_iter()
        .filter(|seat| seat.state_since < now)
        .collect();
    for seat in waiting.into_iter().take(excess) {
        // A standby reviewer has no job: it is idle, its agent alive.
        match release_one(store, herdr, project, &seat, None, now) {
            Ok(true) => report.released.push(seat.id.clone()),
            Ok(false) => {}
            Err(_) => report.failed.push(seat.id.clone()),
        }
    }
    Ok(report)
}

/// The number of lane seats that work on a task now (design §3.6's W): a
/// lane with an attempt in `Started`. A lane that waits, that hands in or
/// that is idle does not count: no review of its work is coming.
fn working_lanes(store: &mut Store, project: &str) -> GearResult<i64> {
    store.read(|tx| {
        let mut working = 0;
        for seat in tx.seats_of_project(project)? {
            if seat.role != SeatRole::Lane {
                continue;
            }
            let attempts = tx.open_attempts_of_seat(&seat.id)?;
            if attempts
                .iter()
                .any(|attempt| attempt.state == AttemptState::Started)
            {
                working += 1;
            }
        }
        Ok(working)
    })
}

/// Cancel the start of a reviewer whose job is gone (design §3.6, B03-05):
/// the ops of the seat that are still live are voided, its pane is closed, its
/// binding ends and the seat is free.
///
/// The transition `Starting → Released` is not in the model's table yet (§3.6
/// is new): the captain's own `- release <reviewer>` states its transition
/// the same way (`gear-app/src/plan.rs`).
fn cancel_start(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    seat: &SeatRow,
    now: Ts,
) -> Result<bool, String> {
    // 1. nothing goes to a reviewer that has no job: the ops of the seat that
    //    are still live for that job are voided (the captain's decision of
    //    M-2603: the `Intended` and the `Submitted` ones — an op in flight
    //    ends on its own).
    let job = seat.task_id.clone().or(store.read(|tx| tx.get_meta(&format!("engine.review_pending.{}", seat.id))).map_err(|e| e.detail)?.filter(|s| !s.is_empty()));
    let live = store
        .read(|tx| tx.slot_ops(&seat.id))
        .map_err(|e| e.detail)?;
    for op in live {
        if !matches!(op.state, OpState::Intended | OpState::Submitted) {
            continue;
        }
        if op.task.as_deref() != job.as_deref() {
            continue;
        }
        store
            .write(|tx| {
                tx.set_op_state(
                    project,
                    &op.id,
                    OpState::Void,
                    Some("the review it was started for is gone"),
                    now,
                )
            })
            .map_err(|e| e.detail)?;
    }

    // 2. the pane goes: the agent that was starting has nothing to do.
    if let Some(pane) = bound_pane(store, seat)?.as_deref() {
        match herdr.pane_close(pane) {
            Outcome::Ok(()) => {}
            Outcome::Refused { code, .. } if code == "pane_not_found" => {}
            Outcome::Refused { code, message } => {
                return Err(format!("pane_close: refused ({code}): {message}"))
            }
            Outcome::Uncertain { reason } => return Err(format!("pane_close: {reason}")),
        }
    }

    // 3. the seat is free: its binding ends with the start, and it waits for
    //    the next review as a free seat.
    store
        .write(|tx| {
            if let Some(binding) = tx.get_binding(&seat.id, seat.generation)? {
                if binding.status != BindingStatus::Void {
                    tx.set_binding_status(&seat.id, seat.generation, BindingStatus::Void)?;
                }
            }
            tx.set_meta(&format!("engine.review_pending.{}", seat.id), "")?;
            // Starting → Released (design §3.6, B03-05).
            require_transition(
                SeatState::can_transition(SeatRole::Temp, SeatState::Starting, SeatState::Released),
                "seat",
                SeatState::Starting.as_str(),
                SeatState::Released.as_str(),
            )?;
            tx.set_seat_state(
                &seat.id,
                SeatState::Released,
                Some("the review it was started for is gone"),
                now,
            )
        })
        .map_err(|e| e.detail)?;
    Ok(true)
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
        if seat.task_id.as_deref() == Some(task) {
            if let Some(job) = tx.last_op(&seat.id, OpKind::Deliver)? {
                if job.task.as_deref() == Some(task) && job.attempt != Some(latest.n) {
                    return Ok(false);
                }
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
///
/// `task` is the job the seat held, if any: a standby reviewer has none.
pub(crate) fn release_one(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    seat: &SeatRow,
    task: Option<&str>,
    now: Ts,
) -> Result<bool, String> {
    let pane = binding_pane(store, seat)?;

    // 1. the agent leaves, once: its pane is closed. Exit keys differ per
    //    agent and fail quietly (codex needs two ctrl+c, grok only `/exit`,
    //    L-63); closing the pane ends any agent. The worktree stays — the next
    //    job reopens it. A pane already gone is fine.
    //
    //    A standby reviewer is released the same way (design §3.6): it is
    //    idle, its agent alive, and the rule that releases it closes its pane
    //    like any other release.
    if matches!(seat.state, SeatState::Active | SeatState::Standby) {
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
        let reason = match task {
            Some(_) => "review done",
            None => "idle reviewer released",
        };
        // The transition the seat makes on its way out (design §3.6): the
        // model knows it.
        require_transition(
            SeatState::can_transition(SeatRole::Temp, seat.state, SeatState::Releasing),
            "seat",
            seat.state.as_str(),
            SeatState::Releasing.as_str(),
        )
        .map_err(|e| e.detail)?;
        store
            .write(|tx| tx.set_seat_state(&seat.id, SeatState::Releasing, Some(reason), now))
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
                Some(&match task {
                    Some(task) => format!("review of {task} done"),
                    None => "idle reviewer released".to_string(),
                }),
                now,
            )
        })
        .map_err(|e| e.detail)?;
    let _ = project;
    Ok(true)
}

/// Release the reviewer seats that run **no reviewer** (B02-04).
///
/// A temp seat that is parked (`Unknown`) and holds no live agent — its
/// binding ended, or its pane is empty — runs nothing, but it holds a temp
/// slot for ever: a parked seat is never retried (design/10), so no review
/// can start and the count of temps in use grows past the quota (the trial
/// saw `temps 4/2` with no reviewer running). Such a seat is released here:
/// the slot is free and the next job takes it.
///
/// A seat whose agent still runs is left alone: that is a reviewer a human has
/// to look at, not a dead seat. A pane that cannot be read is not an absence
/// (design/08): such a seat keeps its slot.
pub fn release_dead_temps(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut released = Vec::new();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        // Only a parked reviewer: a seat that is up, starting or on its way
        // out runs (or is about to run) a review.
        if seat.role != SeatRole::Temp || seat.state != SeatState::Unknown {
            continue;
        }
        if runs_a_reviewer(store, herdr, &seat)? {
            continue;
        }
        release_dead(store, herdr, &seat, now)?;
        released.push(seat.id.clone());
    }
    Ok(released)
}

/// Whether the seat's reviewer is still there: its binding ended (nothing runs
/// in it), or its pane still holds an agent. A pane that cannot be read is an
/// observation failure, not an absence: the seat keeps its slot.
pub(crate) fn runs_a_reviewer(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
) -> GearResult<bool> {
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    let Some(binding) = binding else { return Ok(false) };
    if binding.status == BindingStatus::Void {
        return Ok(false);
    }
    match herdr.agent_get(&binding.pane_id) {
        Ok(None) => Ok(false),
        Ok(Some(_)) => Ok(true),
        Err(_) => Ok(true),
    }
}

/// Free a dead reviewer: its binding ends, its pane is closed and the seat is
/// `Released` — the state that holds no temp slot (design/10). Its worktree
/// stays: the next job on this seat reopens it (17 §8a).
fn release_dead(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
    now: Ts,
) -> GearResult<()> {
    let pane = store
        .read(|tx| tx.get_binding(&seat.id, seat.generation))?
        .map(|binding| binding.pane_id);
    if let Some(pane) = pane.as_deref() {
        // The pane holds no agent (that is why the seat is dead): closing it
        // ends the shell the seat leaves behind. A pane that is already gone
        // is fine; a refused close does not keep the seat alive.
        match herdr.pane_close(pane) {
            Outcome::Ok(()) => {}
            Outcome::Refused { .. } | Outcome::Uncertain { .. } => {}
        }
    }
    store
        .write(|tx| {
            if let Some(binding) = tx.get_binding(&seat.id, seat.generation)? {
                if binding.status != BindingStatus::Void {
                    tx.set_binding_status(&seat.id, seat.generation, BindingStatus::Void)?;
                }
            }
            require_transition(
                SeatState::can_transition(SeatRole::Temp, SeatState::Unknown, SeatState::Released),
                "seat",
                SeatState::Unknown.as_str(),
                SeatState::Released.as_str(),
            )?;
            tx.set_seat_state(
                &seat.id,
                SeatState::Released,
                Some("no reviewer runs in it"),
                now,
            )
        })
}

/// Finish the retirement of a lane (design/10, plan T-65).
///
/// A `Retiring` lane whose work is at a safe point — every open attempt handed
/// in, nothing in progress, its agent idle or already gone — says goodbye to
/// its agent (the profile's `exit_command`, else its `exit_keys`), closes its
/// pane and becomes `Retired`, in that order and once. A lane still working
/// (an attempt that is not handed in, or an agent mid-turn) is finished after
/// its current hand-in.
///
/// The worktree is kept: its build cache is the lane's, and the design removes
/// it only when the lane's results are all merged (design/10), which is not
/// this step.
pub fn retire_finished_lanes(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    observed: &[crate::observe::SeatObs],
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut retired = Vec::new();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        if seat.role != SeatRole::Lane || seat.state != SeatState::Retiring {
            continue;
        }
        // Busy: an attempt that is not handed in finishes first; a hand-in is
        // the lane's safe point (design/10: submitted ones continue through
        // acceptance without the lane).
        let open = store.read(|tx| tx.open_attempts_of_seat(&seat.id))?;
        if !open.iter().all(|attempt| attempt.state == AttemptState::Submitted) {
            continue;
        }
        // A lane mid-turn is not cut off; one whose agent is already gone has
        // nothing left to say goodbye to.
        let settled = observed
            .iter()
            .find(|obs| obs.seat.id == seat.id)
            .map(|obs| {
                obs.idle || matches!(obs.presence, crate::observe::Presence::Missing)
            })
            .unwrap_or(false);
        if !settled {
            continue;
        }
        if let Err(_reason) = retire_one(store, herdr, &seat, profiles, now) {
            continue;
        }
        retired.push(seat.id.clone());
    }
    Ok(retired)
}

/// Take one lane out: exit, close its pane, Retired.
fn retire_one(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    now: Ts,
) -> Result<(), String> {
    let pane = binding_pane(store, seat)?;

    // 1. the agent leaves: the profile's own goodbye (a command for the CLIs
    //    whose exit is a slash command, keys for the others). A pane that is
    //    already gone is fine. T-134: an unnamed agent refuses a prompt, so it
    //    carries its seat name first.
    if let Some(pane) = pane.as_deref() {
        crate::names::ensure(herdr, pane, &crate::names::desired(seat));
        if let Some(profile) = profiles.get(&seat.profile) {
            if let Some(command) = profile.exit_command.as_deref() {
                // Design §10.1: the command goes to the agent's name, never to
                // the pane id — a pane whose agent is gone is a shell.
                if let Some(name) = crate::names::live_name(herdr, pane) {
                    let _ = herdr.prompt(&name, command);
                }
            } else if !profile.exit_keys.is_empty() {
                let _ = herdr.send_keys(pane, &profile.exit_keys);
            }
        }

        // 2. the pane goes: a closed pane ends any agent that ignored its exit.
        match herdr.pane_close(pane) {
            Outcome::Ok(()) => {}
            Outcome::Refused { code, .. } if code == "pane_not_found" => {}
            Outcome::Refused { code, message } => {
                return Err(format!("pane_close: refused ({code}): {message}"))
            }
            Outcome::Uncertain { reason } => return Err(format!("pane_close: {reason}")),
        }
    }

    // 3. the seat is stayed: its binding ends with it, so a later re-register
    //    opens a fresh pane (the seat's worktree and cache stay on disk).
    store
        .write(|tx| {
            if let Some(binding) = tx.get_binding(&seat.id, seat.generation)? {
                if binding.status != BindingStatus::Void {
                    tx.set_binding_status(&seat.id, seat.generation, BindingStatus::Void)?;
                }
            }
            tx.set_seat_state(&seat.id, SeatState::Retired, Some("retired"), now)
        })
        .map_err(|e| e.detail)?;
    Ok(())
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

/// The pane of the seat's binding, whatever its status: a seat that is still
/// starting has a pending binding, and its pane is the one that has to go.
fn bound_pane(store: &mut Store, seat: &SeatRow) -> Result<Option<String>, String> {
    let (id, generation) = (seat.id.clone(), seat.generation);
    store
        .read(|tx| {
            Ok(tx
                .get_binding(&id, generation)?
                .map(|binding| binding.pane_id))
        })
        .map_err(|e| e.detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_store::{testing, NewOp, ProjectRow, TaskRow};

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
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
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
                    issue: None,
                    updated: T0,
                })?;
                testing::seed_temp_seat(tx, "p", "r-1", "T-1", pane, profile, T0)?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// A project with a lane that works on `T-2` (its attempt is `Started`,
    /// so the lane counts for W — design §3.6) and a reviewer seat on `T-1`,
    /// a task that is under review.
    fn world_with_a_working_lane() -> (Store, SeatRow) {
        let mut store = Store::open_in_memory().unwrap();
        let seat = store
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
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: TaskState::Verifying,
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
                    issue: None,
                    updated: T0,
                })?;
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-2".into(),
                    rank: 2,
                    state: TaskState::Ready,
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
                    issue: None,
                    updated: T0,
                })?;
                let lane =
                    testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p1", "opencode", T0)?;
                // The lane works: its attempt is `Started` (W = 1).
                gear_app::tasks::start_attempt(tx, "p", "T-2", &lane.id, 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-2", 1, T0)?;
                let seat = testing::seed_temp_seat(tx, "p", "r-1", "T-1", "w2:p2", "codex", T0)?;
                Ok(seat)
            })
            .unwrap();
        (store, seat)
    }

    /// The attempt of `T-1` is handed in and judged by `seat`: the reviewer
    /// handed in its verdict.
    fn judge(store: &mut Store, seat: &str) {
        store
            .write(|tx| {
                tx.insert_attempt(&gear_store::AttemptRow {
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
                    state: AttemptState::Submitted,
                    candidate: Some("abc1234".into()),
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                })?;
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    passed: true,
                    findings: "ok".into(),
                    by_seat: seat.into(),
                    created: T0 + 5,
                })
            })
            .unwrap();
    }

    /// The reviewer works on attempt 2 of `T-1`, and the verdict of attempt 1
    /// is already in: the seat judged the first attempt, the lane reworked,
    /// and the seat reviews the second attempt now.
    fn reviewer_of_the_second_attempt(store: &mut Store, seat: &SeatRow) {
        store
            .write(|tx| {
                tx.insert_attempt(&gear_store::AttemptRow {
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
                    state: AttemptState::Submitted,
                    candidate: Some("def5678".into()),
                    report: None,
                    safe_point: None,
                    started: T0 + 10,
                    updated: T0 + 10,
                })?;
                // The packet of the seat names the attempt it judges now.
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    kind: OpKind::Deliver,
                    task: Some("T-1".into()),
                    attempt: Some(2),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "text": "the packet of attempt 2" }),
                    created: T0 + 10,
                })?;
                Ok(())
            })
            .unwrap();
    }

    /// The verdict of attempt 2 of `T-1`, handed in by `seat`.
    fn judge_attempt_two(store: &mut Store, seat: &str) {
        store
            .write(|tx| {
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 2,
                    passed: true,
                    findings: "ok".into(),
                    by_seat: seat.into(),
                    created: T0 + 20,
                })
            })
            .unwrap();
    }

    /// A project with the lane `heat` on `pane`, the seat in `state`, and one
    /// task `T-1` the lane may work on.
    fn lane_world(pane: &str, state: SeatState) -> (Store, SeatRow) {
        let mut store = Store::open_in_memory().unwrap();
        let seat = store
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
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: TaskState::Ready,
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
                    issue: None,
                    updated: T0,
                })?;
                let seat = testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, pane, "opencode", T0)?;
                if state != SeatState::Active {
                    tx.set_seat_state(&seat.id, state, Some("retiring for the test"), T0)?;
                }
                Ok(seat)
            })
            .unwrap();
        (store, seat)
    }

    /// The lane as this tick observed it: up and idle.
    fn idle_obs(seat: &SeatRow) -> Vec<crate::observe::SeatObs> {
        vec![crate::observe::SeatObs {
            seat: seat.clone(),
            presence: crate::observe::Presence::Present,
            status: Some(gear_herdr::AgentStatus::Idle),
            flags: std::collections::BTreeSet::new(),
            idle: true,
            screen: None,
            session: None,
        }]
    }

    fn profiles() -> std::collections::BTreeMap<String, gear_herdr::Profile> {
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert("opencode".into(), gear_herdr::builtin("opencode").unwrap());
        profiles
    }

    /// T-65: a Retiring lane idle at a safe point says goodbye, its pane is
    /// closed and it becomes Retired — in that order, once.
    #[test]
    fn a_retiring_lane_is_exited_closed_and_retired_in_order() {
        let (mut store, seat) = lane_world("w1:p1", SeatState::Retiring);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);

        let retired =
            retire_finished_lanes(&mut store, &mut herdr, "p", &idle_obs(&seat), &profiles(), T0 + 10)
                .unwrap();

        assert_eq!(retired, vec![seat.id.clone()], "the lane is retired");
        let calls: Vec<&str> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::SendKeys { .. } | Call::Prompt { .. } => Some("exit"),
                Call::PaneClose { .. } => Some("close"),
                _ => None,
            })
            .collect();
        assert_eq!(calls, vec!["exit", "close"], "exit, then the closed pane");
        assert_eq!(
            store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap().state,
            SeatState::Retired
        );
        // Once: the next tick has nothing left to do.
        let before = herdr.calls.len();
        assert!(retire_finished_lanes(
            &mut store,
            &mut herdr,
            "p",
            &idle_obs(&seat),
            &profiles(),
            T0 + 20
        )
        .unwrap()
        .is_empty());
        assert_eq!(herdr.calls.len(), before, "no second exit or close");
    }

    /// T-65: a lane still working is finished after its current hand-in.
    #[test]
    fn a_retiring_lane_with_an_open_attempt_is_not_closed() {
        let (mut store, seat) = lane_world("w1:p1", SeatState::Active);
        // The lane is working when the captain retires it: the attempt goes on
        // and the lane is finished after its hand-in.
        store
            .write(|tx| {
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
                tx.set_seat_state(&seat.id, SeatState::Retiring, Some("retired by the plan"), T0 + 1)
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Working);

        assert!(retire_finished_lanes(
            &mut store,
            &mut herdr,
            "p",
            &idle_obs(&seat),
            &profiles(),
            T0 + 10
        )
        .unwrap()
        .is_empty(), "a working attempt keeps the lane");
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
        assert_eq!(
            store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap().state,
            SeatState::Retiring
        );

        // Handed in: the lane is at its safe point and goes.
        store
            .write(|tx| {
                gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "report", T0 + 5)
            })
            .unwrap();
        assert_eq!(
            retire_finished_lanes(&mut store, &mut herdr, "p", &idle_obs(&seat), &profiles(), T0 + 20)
                .unwrap(),
            vec![seat.id.clone()]
        );
        assert_eq!(
            store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap().state,
            SeatState::Retired
        );
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
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
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
                    issue: None,
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
    fn a_queued_reviewer_is_never_released() {
        // R1 of the review: a reviewer waiting for a slot (Queued) holds no
        // worktree, and releasing it would throw away the review it is waiting
        // to do — even when its task is already cancelled. A reviewer that is
        // *starting* is cancelled instead (design §3.6, B03-05).
        let mut herdr = FakeHerdr::new();
        let mut store = world(TaskState::Cancelled, "w1:p1", "codex");
        store
            .write(|tx| tx.set_seat_state("p:r-1", SeatState::Queued, Some("waiting"), T0))
            .unwrap();

        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        assert!(report.released.is_empty(), "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(exits(&herdr), 0, "no exit");
        assert!(removes(&herdr).is_empty(), "no removal");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Queued, "left exactly as it was");
    }

    /// T-143 (design §3.6, B03-05): a reviewer that is still starting and
    /// whose job is gone is cancelled: its start op is voided, its pane is
    /// closed, the seat is free.
    #[test]
    fn a_starting_reviewer_whose_job_is_gone_is_cancelled() {
        let mut herdr = FakeHerdr::new();
        let mut store = world(TaskState::Ready, "w2:p2", "codex");
        // The reviewer is starting its review, and the packet is on its way.
        store
            .write(|tx| {
                tx.set_seat_state("p:r-1", SeatState::Starting, Some("starting"), T0)?;
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:r-1".into(),
                    generation: 1,
                    kind: OpKind::Deliver,
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "text": "the packet" }),
                    created: T0,
                })?;
                // The packet of this job was sent: it must not confirm
                // anything either.
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:r-1".into(),
                    generation: 1,
                    kind: OpKind::Doorbell,
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({}),
                    created: T0,
                })?;
                Ok(())
            })
            .unwrap();
        // The doorbell op is sent, the deliver op is not.
        let deliver = store
            .read(|tx| tx.last_op("p:r-1", OpKind::Deliver))
            .unwrap()
            .unwrap();
        let doorbell = store
            .read(|tx| tx.last_op("p:r-1", OpKind::Doorbell))
            .unwrap()
            .unwrap();
        store
            .write(|tx| tx.set_op_state("p", &doorbell.id, OpState::Submitted, None, T0))
            .unwrap();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);

        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        assert_eq!(report.released, vec!["p:r-1".to_string()], "{report:?}");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Released, "the seat is free");
        assert_eq!(exits(&herdr), 1, "its pane is closed");
        assert!(removes(&herdr).is_empty(), "its worktree is kept");
        let op = store.read(|tx| tx.get_op("p", &deliver.id)).unwrap().unwrap();
        assert_eq!(op.state, OpState::Void, "its start op is voided");
        let op = store.read(|tx| tx.get_op("p", &doorbell.id)).unwrap().unwrap();
        assert_eq!(op.state, OpState::Void, "the op it sent is voided too");
        let binding = store.read(|tx| tx.get_binding("p:r-1", 1)).unwrap().unwrap();
        assert_eq!(binding.status, BindingStatus::Void, "its binding ends");
    }

    #[test]
    fn a_cancelled_pending_review_releases_its_starting_seat() {
        let mut herdr = FakeHerdr::new();
        let mut store = world(TaskState::Ready, "w2:p2", "codex");
        store.write(|tx| {
            tx.set_seat_state("p:r-1", SeatState::Starting, Some("starting"), T0)?;
            tx.clear_seat_task("p:r-1")?;
            tx.set_meta("engine.review_pending.p:r-1", "T-1")
        }).unwrap();
        herdr.add_shell("w2:p2");
        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();
        assert_eq!(report.released, vec!["p:r-1"]);
        assert_eq!(store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap().state, SeatState::Released);
        assert_eq!(store.read(|tx| tx.get_meta("engine.review_pending.p:r-1")).unwrap().as_deref(), Some(""));
    }

    #[test]
    fn cancelled_reservation_is_cleared_before_onboarding() {
        let mut herdr = FakeHerdr::new();
        let mut store = world(TaskState::Ready, "w2:p2", "codex");
        store.write(|tx| {
            tx.set_seat_state("p:r-1", SeatState::Released, None, T0)?;
            tx.clear_seat_task("p:r-1")?;
            tx.set_meta("engine.review_pending.p:r-1", "T-1")
        }).unwrap();
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();
        assert_eq!(store.read(|tx| tx.get_meta("engine.review_pending.p:r-1")).unwrap().as_deref(), Some(""));

        store.write(|tx| {
            tx.set_seat_state("p:r-1", SeatState::Queued, None, T0)?;
            tx.set_meta("engine.review_pending.p:r-1", "T-1")
        }).unwrap();
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 11).unwrap();
        assert_eq!(store.read(|tx| tx.get_meta("engine.review_pending.p:r-1")).unwrap().as_deref(), Some(""));
        assert_eq!(store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap().state, SeatState::Released);
    }

    /// T-143 (design §3.6): a reviewer that is still starting and whose job
    /// is alive goes on starting.
    #[test]
    fn a_starting_reviewer_whose_job_is_alive_keeps_starting() {
        let mut herdr = FakeHerdr::new();
        let mut store = world(TaskState::Verifying, "w2:p2", "codex");
        store
            .write(|tx| {
                tx.set_seat_state("p:r-1", SeatState::Starting, Some("starting"), T0)?;
                tx.insert_attempt(&gear_store::AttemptRow {
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
                    state: AttemptState::Submitted,
                    candidate: Some("abc1234".into()),
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                })
            })
            .unwrap();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);

        let report = release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        assert!(report.released.is_empty(), "{report:?}");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Starting, "it goes on");
        assert_eq!(exits(&herdr), 0, "its pane stays open");
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
        // reviewer's job is done (L-58). It does not leave: it waits for the
        // next review, its agent alive and its pane open (design §3.6).
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
        // Its verdict ends its job (L-58): the reviewer waits for the next
        // one, its agent alive and its pane open (design §3.6).
        let parked = crate::review::standby_after_verdict(&mut store, "p", T0 + 30).unwrap();
        assert_eq!(parked, vec!["p:r-1".to_string()], "the reviewer waits for the next review");
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Standby, "the reviewer waits for the next review");
        assert_eq!(seat.task_id, None, "its job binding ended");
        assert_eq!(exits(&herdr), 0, "its pane stays open");

        // No lane works (W = 0), so the idle reviewer is released at once.
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 40).unwrap();
        assert_eq!(exits(&herdr), 1, "the idle reviewer is released");
    }

    /// T-143 (design §3.6): after its verdict a reviewer seat goes to
    /// `Standby`: the agent stays alive, the pane stays open, and the job
    /// binding ends.
    #[test]
    fn a_verdict_puts_the_reviewer_on_standby_with_its_pane_open() {
        let (mut store, seat) = world_with_a_working_lane();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        judge(&mut store, &seat.id);

        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        let row = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(row.state, SeatState::Standby, "the reviewer waits for the next review");
        assert_eq!(row.task_id, None, "its job binding ended");
        assert_eq!(exits(&herdr), 0, "its pane stays open");
        assert!(removes(&herdr).is_empty(), "its worktree is kept");
        let binding = store.read(|tx| tx.get_binding(&seat.id, 1)).unwrap().unwrap();
        assert_eq!(binding.status, BindingStatus::Active, "its agent is still there");
    }

    /// F1 of the T-143 review: the verdict of an earlier attempt does not
    /// end the review that runs now. A reviewer that works on attempt 2 of a
    /// task whose attempt 1 was judged stays bound, Active and its pane open
    /// until the verdict of attempt 2 is in.
    #[test]
    fn an_earlier_verdict_does_not_end_the_review_that_runs_now() {
        let (mut store, seat) = world_with_a_working_lane();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        // The first attempt was judged (it failed), the lane reworked, and the
        // seat reviews the second attempt now.
        judge(&mut store, &seat.id);
        reviewer_of_the_second_attempt(&mut store, &seat);

        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 30).unwrap();

        let row = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(row.state, SeatState::Active, "the review of attempt 2 goes on");
        assert_eq!(row.task_id.as_deref(), Some("T-1"), "the seat keeps its job");
        assert_eq!(exits(&herdr), 0, "its pane stays open");

        // The verdict of attempt 2 ends the job: the reviewer waits for the
        // next one.
        judge_attempt_two(&mut store, &seat.id);
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 40).unwrap();

        let row = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(row.state, SeatState::Standby, "the reviewer waits for the next review");
        assert_eq!(row.task_id, None, "its job binding ended");
        assert_eq!(exits(&herdr), 0, "its pane stays open");
    }

    /// F1 of the T-143 review: a reviewer that judged attempt 1 and whose
    /// lane has already started attempt 2 is done — the verdict of attempt 1
    /// is the verdict of its job, even though a newer attempt exists.
    #[test]
    fn the_verdict_of_the_job_ends_it_even_when_a_newer_attempt_exists() {
        let (mut store, seat) = world_with_a_working_lane();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        // The seat judged attempt 1 (its packet names attempt 1) and the
        // writer has already started attempt 2.
        judge(&mut store, &seat.id);
        store
            .write(|tx| {
                tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    kind: OpKind::Deliver,
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "text": "the packet of attempt 1" }),
                    created: T0,
                })?;
                tx.insert_attempt(&gear_store::AttemptRow {
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
                    state: AttemptState::Started,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: T0 + 10,
                    updated: T0 + 10,
                })
            })
            .unwrap();

        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 30).unwrap();

        let row = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(row.state, SeatState::Standby, "its job is done");
        assert_eq!(row.task_id, None, "its job binding ended");
        assert_eq!(exits(&herdr), 0, "its pane stays open");
    }

    /// The captain's decision of M-2603: the release sweep touches only the
    /// seats that were already Standby before this tick. A reviewer that just
    /// got its verdict stays Standby with its pane open until the next tick.
    #[test]
    fn a_reviewer_that_just_got_its_verdict_waits_for_the_next_tick() {
        let mut store = world(TaskState::Verifying, "w2:p2", "codex");
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        // The attempt is judged: the reviewer goes to Standby in this tick.
        store
            .write(|tx| {
                tx.insert_attempt(&gear_store::AttemptRow {
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
                    state: AttemptState::Submitted,
                    candidate: Some("abc1234".into()),
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                })?;
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    passed: true,
                    findings: "ok".into(),
                    by_seat: "p:r-1".into(),
                    created: T0 + 5,
                })
            })
            .unwrap();

        // No lane works (W = 0), and the reviewer got its verdict in this
        // very tick: it waits.
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Standby, "it waits for the next review");
        assert_eq!(seat.task_id, None, "its job binding ended");
        assert_eq!(exits(&herdr), 0, "its pane stays open");

        // The next tick releases it: it has been waiting since the last one.
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 20).unwrap();

        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Releasing, "the idle reviewer goes");
        assert_eq!(exits(&herdr), 1, "its pane is closed");
    }

    /// T-143 (design §3.6): with one lane working (W = 1) and two Standby
    /// reviewers, the one that waits longest is released; the other stays.
    #[test]
    fn with_one_lane_working_the_longest_waiting_standby_reviewer_is_released() {
        let (mut store, seat) = world_with_a_working_lane();
        store
            .write(|tx| {
                let second = testing::seed_temp_seat(tx, "p", "r-2", "T-1", "w3:p3", "codex", T0)?;
                tx.set_seat_state(&second.id, SeatState::Standby, Some("review done"), T0 + 1)?;
                tx.set_seat_state(&seat.id, SeatState::Standby, Some("review done"), T0)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w3:p3", "codex", gear_herdr::AgentStatus::Idle);

        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();

        // The one that waits longest goes first: its pane is closed and it is
        // on its way out.
        let first = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(first.state, SeatState::Releasing, "the longest waiting reviewer goes");
        assert_eq!(exits(&herdr), 1, "its pane is closed");
        let second = store.read(|tx| tx.get_seat("p:r-2")).unwrap().unwrap();
        assert_eq!(second.state, SeatState::Standby, "the other one waits");

        // The next tick frees the seat.
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 20).unwrap();
        let first = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(first.state, SeatState::Released, "the seat is free");
        assert_eq!(exits(&herdr), 1, "the close is not repeated");
        assert!(removes(&herdr).is_empty(), "the worktree is kept");
    }

    /// T-143 (design §3.6): with no lane working (W = 0) every Standby
    /// reviewer is released at once.
    #[test]
    fn with_no_lane_working_every_standby_reviewer_is_released() {
        let mut store = world(TaskState::Verifying, "w2:p2", "codex");
        store
            .write(|tx| {
                let second = testing::seed_temp_seat(tx, "p", "r-2", "T-1", "w3:p3", "codex", T0)?;
                tx.set_seat_state(&second.id, SeatState::Standby, Some("review done"), T0)?;
                tx.set_seat_state("p:r-1", SeatState::Standby, Some("review done"), T0)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w3:p3", "codex", gear_herdr::AgentStatus::Idle);

        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 10).unwrap();
        release_finished_reviews(&mut store, &mut herdr, "p", T0 + 20).unwrap();

        for name in ["p:r-1", "p:r-2"] {
            let seat = store.read(|tx| tx.get_seat(name)).unwrap().unwrap();
            assert_eq!(seat.state, SeatState::Released, "{name} is free");
        }
        assert_eq!(exits(&herdr), 2, "both panes are closed");
    }

    /// B02-04: a parked reviewer that runs no reviewer is released — the slot
    /// is free — while a parked one whose agent still runs (and a pane that
    /// cannot be read) keeps its slot.
    #[test]
    fn a_parked_reviewer_without_an_agent_is_released_and_a_live_one_is_not() {
        let mut store = world(TaskState::Verifying, "w2:p2", "codex");
        let mut herdr = FakeHerdr::new();
        // The seat is parked; its pane is an empty shell.
        store
            .write(|tx| {
                tx.set_seat_state("p:r-1", SeatState::Unknown, Some("worktree_open"), T0)?;
                Ok(())
            })
            .unwrap();
        herdr.add_shell("w2:p2");

        let released = release_dead_temps(&mut store, &mut herdr, "p", T0 + 10).unwrap();
        assert_eq!(released, vec!["p:r-1".to_string()]);
        let seat = store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Released);
        let binding = store.read(|tx| tx.get_binding("p:r-1", 1)).unwrap().unwrap();
        assert_eq!(binding.status, BindingStatus::Void, "its binding ends with it");

        // The same seat with an agent in its pane and a live binding: it is a
        // reviewer, not a dead seat — it keeps its slot.
        store
            .write(|tx| {
                tx.set_binding_status("p:r-1", 1, BindingStatus::Active)?;
                tx.set_seat_state("p:r-1", SeatState::Unknown, Some("worktree_open"), T0)?;
                Ok(())
            })
            .unwrap();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        assert!(release_dead_temps(&mut store, &mut herdr, "p", T0 + 20).unwrap().is_empty());
        assert_eq!(
            store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap().state,
            SeatState::Unknown
        );

        // A pane that cannot be read is an observation failure, not an
        // absence: the seat keeps its slot (design/08).
        herdr.observe_fails = true;
        assert!(release_dead_temps(&mut store, &mut herdr, "p", T0 + 30).unwrap().is_empty());
        assert_eq!(
            store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap().state,
            SeatState::Unknown
        );
    }
}
