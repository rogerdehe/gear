//! `gear` home page (design/13). Read-only for business state; records one
//! ViewRecord of exactly what was displayed (evidence for op confirmation).

use crate::identity::{require_seat, Fingerprint};
use crate::mail;
use crate::Env;
use gear_model::{GearResult, SeatRole, TaskState, Ts};
use gear_store::{Store, Tx, ViewRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// Text printed to the agent (≈120 lines max).
    pub text: String,
    /// What was shown; written by [`record_view`] only after the text was printed.
    pub view: ViewRecord,
}

/// Build the home page for the caller's seat.
///
/// Slice 1 content:
/// - identity line: seat, role, generation;
/// - captain: the inbox batch from `mail::select_batch` (thread id, message ids,
///   original bodies, "+N more"), then unshown Reply/Instruction messages;
/// - lane/temp: unshown Reply/Instruction messages addressed to it (originals);
/// - lane/temp (T-51): a "current task" line for the seat's open attempt
///   (`open_attempts_of_seat`, first): `task <T-n> attempt <n> contract v<adopted>
///   — <goal of that contract version>`; the view records `shown_task`,
///   `shown_attempt`, `shown_contract` (the engine confirms deliveries from them);
///   no open attempt → no line, those fields None;
/// - footer: suggested next command.
/// "Unshown" = created after the seat's latest view that listed it.
pub fn build(store: &mut Store, env: &Env, fp: Option<&Fingerprint>, now: Ts) -> GearResult<Home> {
    let (text, view) = store.read(|tx| {
        let (ctx, seat) = require_seat(tx, env, fp, None, None)?;
        let seat_id = seat.id.clone();

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!(
            "seat {seat_id} · {} · generation {}",
            seat.role.as_str(),
            ctx.generation
        ));

        // The captain's snapshot: the plan at a glance, before the inbox
        // (T-9). The model is the board's, so the two views never disagree.
        if seat.role == SeatRole::Captain {
            lines.push(String::new());
            lines.extend(snapshot(tx, &ctx.project, now)?);
        }

        // A message counts as shown when a recorded view of this seat and
        // generation lists its id. The ids decide, not the timestamps: a
        // message created in the same second as the view must not be lost,
        // and a message no view ever listed stays unshown.
        let mut already_shown: Vec<String> = Vec::new();
        for view in tx.views_since(&seat_id, ctx.generation, 0)? {
            already_shown.extend(view.shown_msgs);
        }

        let mut shown_msgs: Vec<String> = Vec::new();
        let mut shown_task: Option<String> = None;
        let mut shown_attempt: Option<i64> = None;
        let mut shown_contract: Option<i64> = None;

        // A lane or temp sees the task of its open attempt (design/13: the
        // current task comes right after the identity line).
        if seat.role != SeatRole::Captain {
            if let Some(attempt) = tx.open_attempts_of_seat(&seat_id)?.first() {
                let goal = tx
                    .get_contract(&ctx.project, &attempt.task, attempt.adopted_version)?
                    .map(|contract| contract.goal)
                    .unwrap_or_default();
                lines.push(format!(
                    "task {} attempt {} contract v{} — {}",
                    attempt.task, attempt.n, attempt.adopted_version, goal
                ));
                shown_task = Some(attempt.task.clone());
                shown_attempt = Some(attempt.n);
                shown_contract = Some(attempt.adopted_version);
            }
        }

        if seat.role == SeatRole::Captain {
            // The inbox batch: thread, message ids, the original bodies.
            let batch = mail::select_batch(tx, &ctx.project, &seat_id, now, mail::BatchLimits::default())?;
            if !batch.is_empty() {
                lines.push(String::new());
                lines.push(format!("INBOX ({} threads)", batch.len()));
                for thread in &batch {
                    let ids: Vec<&str> = thread.open.iter().map(|message| message.id.as_str()).collect();
                    let urgent = if thread.urgent { " [urgent]" } else { "" };
                    lines.push(format!("  {}{urgent} {}", thread.thread, ids.join(" ")));
                    for message in &thread.open {
                        lines.push(format!("    {}", message.body));
                        shown_msgs.push(message.id.clone());
                    }
                    if thread.more > 0 {
                        lines.push(format!("    +{} more", thread.more));
                    }
                }
            }
        }

        // The replies and instructions the seat has not seen yet. A message
        // the batch above already printed is not printed again.
        let unshown: Vec<_> = tx
            .messages_to_since(&ctx.project, &seat_id, 0)?
            .into_iter()
            .filter(|message| {
                !already_shown.contains(&message.id) && !shown_msgs.contains(&message.id)
            })
            .collect();
        if !unshown.is_empty() {
            lines.push(String::new());
            lines.push(format!("UNSHOWN ({})", unshown.len()));
            for message in &unshown {
                lines.push(format!("  {} {}  {}", message.id, message.thread, message.body));
                shown_msgs.push(message.id.clone());
            }
        }

        // The command that helps the seat move on.
        lines.push(String::new());
        lines.push(match seat.role {
            SeatRole::Captain => match shown_msgs.first() {
                Some(first) => format!("next: gear reply {first} --resolve"),
                None => "next: gear".to_string(),
            },
            _ => "next: gear send --kind progress".to_string(),
        });

        let view = ViewRecord {
            project: ctx.project.clone(),
            seat: seat_id,
            generation: ctx.generation,
            ts: now,
            shown_msgs,
            shown_ops: Vec::new(),
            shown_task,
            shown_attempt,
            shown_contract,
        };
        Ok((lines.join("\n"), view))
    })?;
    Ok(Home { text, view })
}

/// Persist the view. The CLI calls this **after** printing succeeded; if
/// printing fails the view is not recorded (design/13).
pub fn record_view(store: &mut Store, view: &ViewRecord) -> GearResult<()> {
    store.write(|tx| tx.insert_view(view))
}

/// The most lanes the snapshot lists; the rest is one line of count, so the
/// snapshot stays under 15 lines.
const SNAPSHOT_LANES: usize = 10;

/// The captain's snapshot: the totals, one line per lane (the task it is on
/// and how much is queued), the deliveries in flight and the temp slots.
///
/// The model is the board's ([`gear_board::project`]), so the two views of
/// the plan never disagree. Read-only, like the rest of the page.
fn snapshot(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<String>> {
    let plan = gear_board::project(tx, project, now)?;
    let mut lines = vec![format!(
        "SNAPSHOT {} tasks · {} done · {} in progress · {} waiting",
        plan.totals.tasks, plan.totals.done, plan.totals.in_progress, plan.totals.waiting
    )];
    for lane in plan.lanes.iter().take(SNAPSHOT_LANES) {
        lines.push(lane_line(lane));
    }
    if plan.lanes.len() > SNAPSHOT_LANES {
        lines.push(format!("+{} more lanes", plan.lanes.len() - SNAPSHOT_LANES));
    }
    lines.push(format!(
        "deliveries not confirmed: {} · temps {}/{}",
        plan.unconfirmed_deliveries, plan.temps.in_use, plan.temps.quota
    ));
    Ok(lines)
}

/// One lane of the snapshot: the task it is on and how much is queued.
fn lane_line(lane: &gear_board::LaneBoard) -> String {
    // A lane is on the task in progress; a lane whose work is in review is
    // not idle either, so a review counts when nothing runs. A lane with a
    // task waiting for a question is on that one, too.
    let current = lane
        .tasks
        .iter()
        .find(|task| task.state == TaskState::Running)
        .or_else(|| lane.tasks.iter().find(|task| task.state == TaskState::Waiting))
        .or_else(|| lane.tasks.iter().find(|task| task.state == TaskState::Verifying));
    let current = match current {
        Some(task) => format!("{} {}", task.id, gear_board::state_word(task.state)),
        None => "idle".to_string(),
    };
    let queued = lane.tasks.iter().filter(|task| task.state == TaskState::Ready).count();
    format!("  {} · {} · {} queued", lane.lane, current, queued)
}
