//! T-571: a finished voyage sends nothing.
//!
//! A voyage is finished when none of its tasks is open (the user, 2026-11-05:
//! Lawrence's voyage M1 finished on 2026-11-04 — every task done or cancelled,
//! every lane retired — yet its new captain was rung again and again for two
//! branch warnings from 2026-11-03, each doorbell carrying M1's goal,
//! interrupting the work the user had given it).
//!
//! The items a voyage left open are resolved here, once, on the tick that finds
//! it finished: the branch sweep, the station of the voyage, the merge
//! failures and waits, the overrun asks, the stalled-voyage item, and the open
//! items of its tasks. The doorbell half is [`crate::doorbell::goal_line`] (no
//! goal of a finished voyage) and the captain filter in `tick` (no engine item
//! of a project with no open voyage).

use gear_model::{GearResult, MsgState, Ts};
use gear_store::{Store, Tx};
use std::collections::BTreeSet;

/// Why an item closes with its voyage: `voyage M1 is finished`.
pub fn reason(voyage: &str) -> String {
    format!("voyage {voyage} is finished")
}

/// The project-level threads a finished voyage leaves behind (T-571): the
/// branch sweep (named after the tick), the merge failures and waits (named
/// after the task), the overrun asks and the stalled-voyage item. They belong
/// to no single task, so they close only when no voyage of the project is open
/// any more.
const PROJECT_THREADS: [&str; 6] = [
    "branches:",
    "merge-failed:",
    "merge-lost:",
    "merge-wait:",
    "overrun",
    "stalled",
];

/// A finished voyage and the tasks of it.
struct FinishedRow {
    milestone: String,
    tasks: BTreeSet<String>,
}

/// The voyages of this project that are finished: departed, with tasks, and
/// none of their tasks open (the rule `tasks_live_of_project` already uses,
/// T-529: a voyage with a task that is not terminal is open).
fn finished_voyages(tx: &Tx<'_>, project: &str) -> GearResult<Vec<FinishedRow>> {
    let open: BTreeSet<String> = tx
        .tasks_live_of_project(project)?
        .into_iter()
        .filter_map(|task| task.milestone)
        .collect();
    let mut finished = Vec::new();
    for voyage in tx.voyages_of_project(project)? {
        if voyage.departure.is_none() || open.contains(&voyage.milestone) {
            continue;
        }
        let tasks: BTreeSet<String> = tx
            .tasks_of_voyage(project, &voyage.milestone)?
            .into_iter()
            .map(|task| task.id)
            .collect();
        if tasks.is_empty() {
            continue;
        }
        finished.push(FinishedRow {
            milestone: voyage.milestone,
            tasks,
        });
    }
    Ok(finished)
}

/// The finished voyages of a project, read once (T-571): the sweep that closes
/// their items and the doorbell that names none of them use the same reading.
pub struct Sweep {
    rows: Vec<FinishedRow>,
    /// A voyage of the project still has open work: its project-level threads
    /// (a branch sweep, a merge wait) may be about that one, so they stay.
    any_open: bool,
}

impl Sweep {
    pub fn read(tx: &Tx<'_>, project: &str) -> GearResult<Sweep> {
        let rows = finished_voyages(tx, project)?;
        let any_open = !tx
            .tasks_live_of_project(project)?
            .into_iter()
            .filter_map(|task| task.milestone)
            .collect::<BTreeSet<String>>()
            .is_empty();
        Ok(Sweep { rows, any_open })
    }

    pub fn empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Why this item belongs to a finished voyage, if it does.
    pub fn why(&self, message: &gear_store::MessageRow) -> Option<String> {
        why(message, &self.rows, self.any_open)
    }
}

/// Resolve the open engine items a finished voyage left behind, and return
/// their ids. Runs every tick; a finished voyage whose items are already
/// resolved writes nothing.
pub fn close(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<String>> {
    let sweep = store.read(|tx| Sweep::read(tx, project))?;
    if sweep.empty() {
        return Ok(Vec::new());
    }
    store.write(|tx| {
        let mut closed = Vec::new();
        for message in tx.open_messages_of_project(project)? {
            let Some(reason) = sweep.why(&message) else {
                continue;
            };
            tx.update_message_state(
                project,
                &message.id,
                MsgState::Resolved,
                Some(&reason),
                None,
                None,
                now,
            )?;
            closed.push(message.id);
        }
        Ok(closed)
    })
}

/// Why this item belongs to a finished voyage, if it does.
fn why(
    message: &gear_store::MessageRow,
    finished: &[FinishedRow],
    any_open: bool,
) -> Option<String> {
    // The item of a task of a finished voyage: its attempt's threads, its merge
    // failures and waits, its overrun ask.
    if let Some(task) = &message.task {
        if let Some(row) = finished.iter().find(|row| row.tasks.contains(task)) {
            return Some(reason(&row.milestone));
        }
    }
    // The station of the voyage itself.
    if let Some(voyage) = message.thread.strip_prefix("station:") {
        if finished.iter().any(|row| row.milestone == voyage) {
            return Some(reason(voyage));
        }
    }
    if any_open {
        return None;
    }
    // A project-level thread, with no voyage of the project open any more: the
    // newest finished voyage is the one it belonged to.
    if PROJECT_THREADS
        .iter()
        .any(|prefix| message.thread.starts_with(prefix))
    {
        return finished.last().map(|row| reason(&row.milestone));
    }
    None
}
