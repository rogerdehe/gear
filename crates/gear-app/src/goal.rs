//! The milestone blocks a draft owes the voyage file (T-333).
//!
//! The words of a voyage live in its `# Milestone <id>:` block, and the store
//! keeps a copy: a draft may name a milestone in its `# Queue` without saying
//! anything about it (a task added to a voyage that already sails). Then the
//! file must get the block back — the file is the voyage's plan, and a reader
//! of it must find every milestone its tasks sail on (T-333).

use crate::draft::{milestone_parts, Draft};
use gear_model::{ErrorCode, GearError, GearResult};
use gear_store::Tx;

/// The `# Milestone` blocks this draft needs but does not carry (T-333), in
/// the order their milestones are first named by a queue item.
///
/// A milestone whose block is in the draft needs nothing. One the draft names
/// and does not describe gets its block from the store — the words an earlier
/// apply recorded, followed by its station lines — and a milestone the store
/// has no words for is refused: a task sails on a voyage the file describes,
/// with its `done:`, `bar:` and `eta:` lines (T-22).
///
/// A cancelled queue line is no task of the milestone any more, so it asks for
/// no block.
pub fn missing_blocks(tx: &Tx<'_>, project: &str, draft: &Draft) -> GearResult<Vec<String>> {
    let mut blocks: Vec<String> = Vec::new();
    let mut named: Vec<String> = Vec::new();
    for item in &draft.queue {
        if item.state.as_deref() == Some("cancelled") {
            continue;
        }
        let Some(milestone) = item.milestone.as_ref() else {
            continue;
        };
        if named.contains(milestone) {
            continue;
        }
        named.push(milestone.clone());
        // A block in the draft is the words themselves: nothing to write back.
        if draft
            .milestones
            .iter()
            .any(|(header, _)| milestone_parts(header).0 == milestone)
        {
            continue;
        }
        let stored = tx.get_voyage(project, milestone)?;
        let done = stored
            .as_ref()
            .and_then(|voyage| voyage.done.as_deref())
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if done.is_empty() {
            let task = item.id.clone().unwrap_or_else(|| "NEW".to_string());
            let title = stored
                .as_ref()
                .map(|voyage| voyage.title.trim())
                .unwrap_or_default();
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "the plan names milestone {milestone} ({task}) but has no `# Milestone {milestone}: {title}` block with its done:, bar: and eta: lines (T-22)"
                ),
            ));
        }
        let voyage = stored.expect("a done sentence comes from a stored voyage");
        let mut block = format!(
            "# Milestone {milestone}: {}\ndone: {}\nbar: {}\neta: {}\n",
            voyage.title,
            done,
            voyage.bar.as_deref().unwrap_or_default(),
            voyage.eta.as_deref().unwrap_or_default()
        );
        for station in tx.voyage_stations(project, milestone)? {
            block.push_str(&format!("station {}: {}\n", station.id, station.needs));
        }
        blocks.push(block);
    }
    Ok(blocks)
}
