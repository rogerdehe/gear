//! `gear` home page (design/13). Read-only for business state; records one
//! ViewRecord of exactly what was displayed (evidence for op confirmation).

use crate::identity::{require_seat, Fingerprint};
use crate::mail;
use crate::Env;
use gear_model::{GearResult, SeatRole, Ts};
use gear_store::{Store, ViewRecord};

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

        // What the seat saw the last time: everything created after that
        // view is unshown.
        let views = tx.views_since(&seat_id, ctx.generation, 0)?;
        let since = views.last().map(|view| view.ts).unwrap_or(0);

        let mut shown_msgs: Vec<String> = Vec::new();

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

        // The replies and instructions the seat has not seen yet.
        let unshown = tx.messages_to_since(&ctx.project, &seat_id, since)?;
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
            shown_task: None,
            shown_attempt: None,
            shown_contract: None,
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
