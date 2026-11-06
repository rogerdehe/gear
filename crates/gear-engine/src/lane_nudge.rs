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
        "Continue {task}: your last turn stopped (a server error such as \"model at capacity\", a dropped connection, or a pause). Go on where you stopped; when done, hand in with gear send --kind result, as the task packet says."
    )
}

/// The key of the idle mark of one attempt (T-206): the first tick the lane's
/// agent was idle on.
fn idle_key(seat: &str, task: &str, attempt: i64) -> String {
    format!("engine.lanenudge.{seat}.{task}.{attempt}")
}

/// T-610: an engine nudge to a lane closes itself. The stopped-lane nudge is
/// a delivery op; a lane may not resolve it and the captain may not either, so
/// the engine voids it with the reason when the attempt it names has been
/// handed in, has a newer attempt, or its task is terminal. A nudge for a
/// still-open attempt stays.
pub fn resolve_nudges(store: &mut Store, project: &str, now: Ts) -> GearResult<()> {
    let open = store.read(|tx| {
        tx.ops_of_kind_in_states(
            project,
            OpKind::Deliver,
            &[
                gear_model::OpState::Intended,
                gear_model::OpState::Executing,
                gear_model::OpState::Submitted,
                gear_model::OpState::Unknown,
            ],
        )
    })?;
    for op in open {
        if op.payload.get("nudge").is_none() {
            continue;
        }
        let (Some(task), Some(attempt)) = (op.task.as_deref(), op.attempt) else {
            continue;
        };
        let reason = store.read(|tx| crate::quiet::attempt_closed(tx, project, task, attempt))?;
        let Some(reason) = reason else {
            continue;
        };
        store.write(|tx| {
            tx.set_op_state(
                project,
                &op.id,
                gear_model::OpState::Void,
                Some(&reason),
                now,
            )
        })?;
    }
    Ok(())
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
    resolve_nudges(store, project, now)?;
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
            // T-523: this attempt's deliveries, read by (seat, kind, task,
            // attempt) — not every delivery the seat ever had.
            let deliveries = store.read(|tx| {
                tx.ops_of_seat_kind_task(&seat.id, OpKind::Deliver, &attempt.task, attempt.attempt)
            })?;
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
    let ops = store.read(|tx| tx.ops_of_seat_kind_task(seat, OpKind::Deliver, task, attempt))?;
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
        .ops_of_seat_kind_task(seat, OpKind::Deliver, task, attempt)?
        .into_iter()
        .filter(|op| op.payload.get("redeliver").is_some())
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
    let ops = tx.ops_of_seat_kind_task(seat, OpKind::Deliver, task, attempt)?;
    Ok(ops
        .iter()
        .filter(|op| op.payload.get("nudge").is_some())
        .count() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::{MsgKind, MsgState, TaskState};
    use gear_store::{testing, AttemptStep, ContractRow, NewMessage, TaskRow};

    const T: i64 = 1_800_000_000;
    const PANE: &str = "w1:p2";

    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T)?;
            tx.insert_task(&TaskRow {
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
                updated: T,
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-1".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T,
                mechanical: None,
            })?;
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T)?;
            Ok(())
        })
        .unwrap();
        s
    }

    fn nudge(s: &mut Store, at: i64) -> String {
        s.write(|tx| {
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": "continue", "nudge": 1 }),
                created: at,
            })?;
            Ok(op.id)
        })
        .unwrap()
    }

    fn state(s: &mut Store, id: &str) -> gear_model::OpState {
        s.read(|tx| {
            Ok(tx
                .ops_of_seat_kind("p:heat", OpKind::Deliver)?
                .into_iter()
                .find(|op| op.id == id)
                .unwrap()
                .state)
        })
        .unwrap()
    }

    /// T-610: the stopped-lane nudge closes itself when the attempt it names
    /// was handed in; a nudge for a still-open attempt stays.
    #[test]
    fn a_stopped_lane_nudge_closes_on_the_hand_in_of_its_attempt() {
        let mut s = world();
        let id = nudge(&mut s, T + 100);
        resolve_nudges(&mut s, "p", T + 200).unwrap();
        assert_eq!(state(&mut s, &id), gear_model::OpState::Intended);
        s.write(|tx| {
            let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
            attempt.state = AttemptState::Submitted;
            tx.update_attempt(&attempt)
        })
        .unwrap();
        resolve_nudges(&mut s, "p", T + 300).unwrap();
        assert_eq!(state(&mut s, &id), gear_model::OpState::Void);
        let result = s
            .read(|tx| {
                Ok(tx
                    .ops_of_seat_kind("p:heat", OpKind::Deliver)?
                    .into_iter()
                    .find(|op| op.id == id)
                    .unwrap()
                    .result)
            })
            .unwrap();
        assert!(
            result
                .as_deref()
                .is_some_and(|r| r.contains("was handed in")),
            "{result:?}"
        );
    }

    /// T-610: a nudge for a still-open attempt stays.
    #[test]
    fn a_stopped_lane_nudge_for_a_still_open_attempt_stays() {
        let mut s = world();
        let id = nudge(&mut s, T + 100);
        resolve_nudges(&mut s, "p", T + 200).unwrap();
        assert_eq!(state(&mut s, &id), gear_model::OpState::Intended);
    }

    /// T-610: the stopped-lane nudge closes when its task is cancelled.
    #[test]
    fn a_stopped_lane_nudge_closes_on_a_cancel_of_its_task() {
        let mut s = world();
        let id = nudge(&mut s, T + 100);
        s.write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.state = TaskState::Cancelled;
            tx.update_task(&task)
        })
        .unwrap();
        resolve_nudges(&mut s, "p", T + 200).unwrap();
        assert_eq!(state(&mut s, &id), gear_model::OpState::Void);
    }

    /// T-610: the stopped-lane nudge closes when a newer attempt exists.
    #[test]
    fn a_stopped_lane_nudge_closes_on_a_newer_attempt() {
        let mut s = world();
        let id = nudge(&mut s, T + 100);
        s.write(|tx| {
            tx.insert_attempt(&gear_store::AttemptRow {
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
                started: T + 150,
                updated: T + 150,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })
        })
        .unwrap();
        resolve_nudges(&mut s, "p", T + 200).unwrap();
        assert_eq!(state(&mut s, &id), gear_model::OpState::Void);
    }

    /// The lane's hand-in answered the captain's item: the item closes.
    #[test]
    fn the_captains_item_closes_when_the_lane_hands_in() {
        let mut s = world();
        let item = s
            .write(|tx| {
                tx.insert_message(&NewMessage {
                    project: "p".into(),
                    thread: "quiet:p:heat".into(),
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: MsgKind::System,
                    urgent: true,
                    body: "no word".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: T + 100,
                    meta: None,
                })
            })
            .unwrap();
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-1:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Result,
                urgent: false,
                body: "done".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: T + 150,
                meta: None,
            })
            .map(|_| ())
        })
        .unwrap();
        // The quiet module owns the captain item; the lane nudge's own
        // resolution is the op. Here the quiet item is resolved by the same
        // rule, so call the quiet resolver.
        s.write(|tx| crate::quiet::resolve_nudges(tx, "p", T + 200))
            .unwrap();
        let item = s
            .read(|tx| tx.thread_messages("p", "quiet:p:heat"))
            .unwrap()
            .into_iter()
            .find(|m| m.id == item.id)
            .unwrap();
        assert_eq!(item.state, MsgState::Resolved);
    }
}
