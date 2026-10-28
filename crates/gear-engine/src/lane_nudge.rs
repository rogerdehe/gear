//! A lane that stopped on a server error goes on by itself (T-206).
//!
//! The mirror of [`crate::review::nudge_stopped_reviewers`] for the lanes: a
//! lane whose agent has been idle for two minutes with an attempt in flight
//! gets one nudge into the same session, and the captain is told when the
//! nudges are spent.
//!
//! The screen text rules out only what a nudge cannot fix: a lane whose
//! screen shows a quota or a login is left to the quota and the auth paths
//! (the reviewer's rule), and a lane that waits for its provider is not stuck
//! (T-75).

use gear_model::{AttemptState, GearResult, OpKind, SeatRole, SeatState, Ts};
use gear_store::{NewOp, Store, Tx};

use crate::observe::SeatObs;

/// The nudges one attempt gets before the captain is told (T-206).
const LANE_NUDGE_BUDGET: i64 = 3;

/// How long a lane's agent must be idle before the lane is nudged (T-206).
const IDLE_BEFORE_NUDGE_S: i64 = 120;

/// The text a stopped lane is nudged with (T-206).
fn nudge_text(task: &str) -> String {
    format!(
        "Continue {task}: your last turn stopped (a server error such as \"model at capacity\", a dropped connection, or a pause). Go on where you stopped; when done, hand in with gear send --kind handin."
    )
}

/// The key of the idle mark of one attempt (T-206): the first tick the lane's
/// agent was idle on.
fn idle_key(seat: &str, task: &str, attempt: i64) -> String {
    format!("engine.lanenudge.{seat}.{task}.{attempt}")
}

/// Tell the lanes that stopped to go on.
///
/// A lane seat (role `Lane`) with an attempt in flight, that is neither
/// parked nor held and does not wait for its provider, whose agent has been
/// idle for [`IDLE_BEFORE_NUDGE_S`] seconds gets one nudge into the same
/// session. A working agent clears the idle mark: the next idle episode
/// starts over.
///
/// The nudges of one attempt are counted like the reviewer's
/// ([`crate::review::nudge_stopped_reviewers`]): a push or a progress message
/// after a nudge resets the count. Past [`LANE_NUDGE_BUDGET`] the captain
/// hears about it once, on the lane's stall thread.
pub fn nudge_stopped_lanes(
    store: &mut Store,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
) -> GearResult<(Vec<String>, Vec<String>)> {
    let mut nudged = Vec::new();
    let mut flagged = Vec::new();
    for obs in observed {
        let seat = &obs.seat;
        if seat.role != SeatRole::Lane {
            continue;
        }
        // A parked or a held lane is not stopped: the captain or the quota
        // path owns it.
        if matches!(seat.state, SeatState::Parked | SeatState::Held) {
            continue;
        }
        // A nudge cannot fix a quota or a login (the reviewer's path).
        if obs.flags.iter().any(|flag| {
            matches!(
                flag,
                gear_herdr::profile::ScreenFlag::Quota | gear_herdr::profile::ScreenFlag::Auth
            )
        }) {
            continue;
        }
        // A lane that waits for its provider is not stuck (T-75).
        if obs
            .flags
            .contains(&gear_herdr::profile::ScreenFlag::Backoff)
        {
            continue;
        }
        // A lane that waits for another task on purpose is not stopped (F4):
        // the wait keeps its attempt, and a nudge would wake work that waits.
        if store.read(|tx| crate::waits::waiting_seat(tx, &seat.id))? {
            continue;
        }
        let Some(attempt) = attempt_in_flight(store, project, &seat.id)? else {
            continue;
        };
        let key = idle_key(&seat.id, &attempt.task, attempt.attempt);
        if !obs.idle {
            // A working agent clears the idle mark.
            store.write(|tx| tx.set_meta(&key, ""))?;
            continue;
        }
        // The first idle tick of the episode is kept; the nudge follows two
        // minutes later.
        let idle_since = store
            .read(|tx| tx.get_meta(&key))?
            .and_then(|at| at.trim().parse::<Ts>().ok());
        let Some(idle_since) = idle_since else {
            store.write(|tx| tx.set_meta(&key, &now.to_string()))?;
            continue;
        };
        if now - idle_since < IDLE_BEFORE_NUDGE_S {
            continue;
        }
        if attempt.state == AttemptState::Delivered {
            // The delivery was never confirmed: the turn ended before the
            // lane took the packet. It is delivered again, once, the same
            // way (T-206).
            let deliveries: Vec<_> = store
                .read(|tx| tx.ops_of_seat_kind(&seat.id, OpKind::Deliver))?
                .into_iter()
                .filter(|op| {
                    op.task.as_deref() == Some(attempt.task.as_str())
                        && op.attempt == Some(attempt.attempt)
                })
                .collect();
            let Some(last) = deliveries.last() else {
                continue;
            };
            // Never clone a packet that was not sent: an op that is still
            // Intended (the captain is away) or Unknown may yet get
            // through (F2).
            if last.state != gear_model::OpState::Failed {
                continue;
            }
            // The retry is allowed once (F3): the marker says the first
            // failure may still be retried; the retry itself is pending.
            // The retry happens once: a delivery that already carries
            // the marker was delivered again (F2, F3).
            if deliveries
                .iter()
                .any(|op| op.payload.get("redeliver").is_some())
            {
                continue;
            }
            let Some(text) = last.payload.get("text").and_then(|text| text.as_str()) else {
                continue;
            };
            let text = text.to_string();
            store.write(|tx| {
                tx.insert_op(&NewOp {
                    project: project.to_string(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    kind: OpKind::Deliver,
                    task: Some(attempt.task.clone()),
                    attempt: Some(attempt.attempt),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "text": text, "redeliver": 1 }),
                    created: now,
                })?;
                tx.set_meta(
                    &retry_marker(&seat.id, &attempt.task, attempt.attempt),
                    "pending",
                )?;

                Ok(())
            })?;
            nudged.push(seat.id.clone());
            continue;
        }
        // Never two deliveries in flight (the reviewer's rule, F2): a nudge
        // that would join another one is not sent.
        if store.read(|tx| delivery_in_flight(tx, &seat.id, now))? {
            continue;
        }
        // The nudges this attempt got, and the progress after them.
        let used = nudges_used(
            store,
            project,
            &seat.id,
            &attempt.task,
            attempt.attempt,
            now,
        )?;
        if used >= LANE_NUDGE_BUDGET {
            // The captain hears about it, once.
            let fact = format!(
                "its agent has been idle for {IDLE_BEFORE_NUDGE_S}s and {used} nudges did not help"
            );
            let opened = store.write(|tx| crate::stall::alert_fact(tx, &seat.id, now, &fact))?;
            if opened {
                flagged.push(seat.id.clone());
            }
            continue;
        }
        // The nudge: the same session, a new turn.
        store.write(|tx| {
            tx.insert_op(&NewOp {
                project: project.to_string(),
                seat: seat.id.clone(),
                generation: seat.generation,
                kind: OpKind::Deliver,
                task: Some(attempt.task.clone()),
                attempt: Some(attempt.attempt),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({
                    "text": nudge_text(&attempt.task),
                    "nudge": used + 1,
                }),
                created: now,
            })?;
            // A full interval before the next nudge (F2): the nudges of one
            // idle episode are spread over minutes, not over seconds.
            tx.set_meta(&key, &now.to_string())?;
            Ok(())
        })?;
        nudged.push(seat.id.clone());
    }
    Ok((nudged, flagged))
}

/// The attempt of a seat that is in flight: the one that is `Started` (the
/// lane works on it) or the one that is `Delivered` (its delivery was never
/// confirmed: the turn ended before the lane took it).
#[derive(Debug, Clone)]
struct InFlight {
    task: String,
    attempt: i64,
    state: AttemptState,
}

fn attempt_in_flight(
    store: &mut Store,
    _project: &str,
    seat: &str,
) -> GearResult<Option<InFlight>> {
    let attempts = store.read(|tx| tx.open_attempts_of_seat(seat))?;
    for wanted in [AttemptState::Started, AttemptState::Delivered] {
        if let Some(attempt) = attempts.iter().find(|attempt| attempt.state == wanted) {
            return Ok(Some(InFlight {
                task: attempt.task.clone(),
                attempt: attempt.n,
                state: attempt.state,
            }));
        }
    }
    Ok(None)
}

/// The nudges one attempt got, like the reviewer's: a push or a progress
/// message after a nudge resets the count.
fn nudges_used(
    store: &mut Store,
    project: &str,
    seat: &str,
    task: &str,
    attempt: i64,
    now: Ts,
) -> GearResult<i64> {
    let ops: Vec<_> = store
        .read(|tx| tx.ops_of_seat_kind(seat, OpKind::Deliver))?
        .into_iter()
        .filter(|op| op.task.as_deref() == Some(task) && op.attempt == Some(attempt))
        .collect();
    let progress = store
        .read(|tx| tx.progress_of_attempt(project, task, attempt))?
        .into_iter()
        .filter(|message| message.sender == seat && message.created <= now)
        .map(|message| message.created)
        .max();
    Ok(match progress {
        Some(at) => ops
            .iter()
            .filter(|op| op.created > at && op.payload.get("nudge").is_some())
            .count() as i64,
        None => ops
            .last()
            .and_then(|op| op.payload.get("nudge"))
            .and_then(|n| n.as_i64())
            .unwrap_or(0),
    })
}

/// The marker of the one retry a lane's delivery may get (T-206, F3): the
/// first failure says `allowed` (the retry may still come), the retry itself
/// says `pending` (its own deadline opens the one stall item).
pub fn retry_marker(seat: &str, task: &str, attempt: i64) -> String {
    format!("engine.laneretry.{seat}.{task}.{attempt}")
}

/// May this attempt's delivery still be retried once (T-206, F3)?
pub fn retry_allowed(tx: &Tx<'_>, seat: &str, task: &str, attempt: i64) -> GearResult<bool> {
    Ok(tx.get_meta(&retry_marker(seat, task, attempt))?.as_deref() == Some("allowed"))
}

/// Is the one automatic retry of this delivery still ahead (T-218, F3)?
///
/// `allowed` means the retry may still come, `pending` means it is on its
/// way. The cleared empty value means the one retry is spent: the fact
/// stands and the captain hears about it (T-218).
pub fn retry_pending(tx: &Tx<'_>, seat: &str, task: &str, attempt: i64) -> GearResult<bool> {
    Ok(matches!(
        tx.get_meta(&retry_marker(seat, task, attempt))?.as_deref(),
        Some("allowed") | Some("pending")
    ))
}

/// May this seat's delivery still be retried once (T-206, F3)? The marker
/// says the first failure may come again: the tick holds the captain's ring
/// back until the retry has had its chance. The cleared empty value is not
/// permission (T-218): the one retry is spent.
pub fn may_retry(tx: &Tx<'_>, seat: &str) -> GearResult<bool> {
    let Some(failed) = crate::delivery::failed(tx, seat)? else {
        return Ok(false);
    };
    retry_pending(tx, seat, &failed.task, failed.attempt)
}

/// May this delivery still get the one automatic retry of the lane's own path
/// (T-218, F3)? Only a lane seat whose attempt waits for its check-in
/// (`Delivered`) is delivered again by [`nudge_stopped_lanes`]; every other
/// seat (a reviewer, the captain) has no such retry, so its unconfirmed
/// delivery reaches the captain at once.
pub fn retry_eligible(
    tx: &Tx<'_>,
    project: &str,
    seat: &str,
    task: &str,
    attempt: i64,
) -> GearResult<bool> {
    let Some(row) = tx.get_seat(seat)? else {
        return Ok(false);
    };
    if row.role != SeatRole::Lane {
        return Ok(false);
    }
    Ok(tx
        .get_attempt(project, task, attempt)?
        .is_some_and(|row| row.state == AttemptState::Delivered))
}

/// Did the one automatic retry of this delivery already fail (T-218, F1)?
///
/// A retry that fails while it is sent (the agent of the pane was gone)
/// never reaches the confirmation window: the op is `Failed` at once. Such a
/// retry spends its marker — the fact stands and the captain hears about it.
pub fn retry_failed(tx: &Tx<'_>, seat: &str, task: &str, attempt: i64) -> GearResult<bool> {
    Ok(tx
        .ops_of_seat_kind(seat, OpKind::Deliver)?
        .into_iter()
        .filter(|op| {
            op.task.as_deref() == Some(task)
                && op.attempt == Some(attempt)
                && op.payload.get("redeliver").is_some()
        })
        .any(|op| op.state == gear_model::OpState::Failed))
}

/// Is a delivery of the seat still in flight (T-206, F2)? The reviewer's
/// rule: a submitted delivery that is older than the cooldown does not block
/// (it is exactly the case the nudge is for), anything else does.
pub fn delivery_in_flight(tx: &Tx<'_>, seat: &str, now: Ts) -> GearResult<bool> {
    Ok(tx
        .slot_ops(seat)?
        .into_iter()
        .any(|op| !(op.state == gear_model::OpState::Submitted && now - op.created >= 30)))
}

/// The nudge of a lane that stopped, as the reviewer's path counts it.
pub fn nudges_of(tx: &Tx<'_>, seat: &str, task: &str, attempt: i64) -> GearResult<i64> {
    let ops: Vec<_> = tx
        .ops_of_seat_kind(seat, OpKind::Deliver)?
        .into_iter()
        .filter(|op| op.task.as_deref() == Some(task) && op.attempt == Some(attempt))
        .collect();
    Ok(ops
        .iter()
        .filter(|op| op.payload.get("nudge").is_some())
        .count() as i64)
}
