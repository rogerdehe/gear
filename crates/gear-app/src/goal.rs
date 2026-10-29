//! The goal of a departed voyage belongs to the user (T-331): after departure
//! its `title`, `done:`, `bar:` and stations are locked, and an apply that
//! changes any of them writes nothing. `eta:` is not locked.
//!
//! A voyage whose words were never written (it departed before its first
//! block) may be described once, as T-22 allows: the lock starts with the
//! stored `done:`. A departed voyage without stations may be given its first
//! list once, for the same reason.

use crate::draft::{milestone_fields, milestone_parts, milestone_stations, Draft};
use gear_model::{ErrorCode, GearError, GearResult};
use gear_store::{Tx, VoyageStation};

/// One field of a departed voyage's goal that this apply would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalChange {
    pub milestone: String,
    pub field: &'static str,
    pub before: String,
    pub after: String,
}

/// The changes this draft makes to the goal of a voyage that has departed and
/// already has its words, in block order: `title`, `done`, `bar` and
/// `stations`. Fields are compared collapsed (whitespace runs read as one
/// space), so a whitespace-only difference is no change; an absent block field
/// reads as "".
pub fn changes(tx: &Tx<'_>, project: &str, draft: &Draft) -> GearResult<Vec<GoalChange>> {
    let mut changes: Vec<GoalChange> = Vec::new();
    for (header, body) in &draft.milestones {
        let (milestone, title) = milestone_parts(header);
        let Some(stored) = tx.get_voyage(project, milestone)? else {
            continue;
        };
        if stored.departure.is_none() {
            continue;
        }
        let described = stored.done.as_deref().map(str::trim).unwrap_or("");
        if described.is_empty() {
            // A voyage whose words were never written may be described once.
            continue;
        }
        let (done, bar, _eta) = milestone_fields(body);
        let stations = milestone_stations(body).map_err(|e| {
            GearError::new(ErrorCode::BadFormat, format!("milestone {milestone}: {e}"))
        })?;
        let mut differ = |field: &'static str, before: String, after: String| {
            if crate::decision::collapsed(&before) != crate::decision::collapsed(&after) {
                changes.push(GoalChange {
                    milestone: milestone.to_string(),
                    field,
                    before,
                    after,
                });
            }
        };
        differ("title", stored.title, title.to_string());
        differ(
            "done",
            stored.done.unwrap_or_default(),
            done.unwrap_or_default(),
        );
        differ("bar", stored.bar.unwrap_or_default(), bar.unwrap_or_default());
        let stored_stations = tx.voyage_stations(project, milestone)?;
        if !stored_stations.is_empty() {
            differ(
                "stations",
                stations_text(&stored_stations),
                stations_text(&stations),
            );
        }
    }
    Ok(changes)
}

/// Refuse the apply when it changes the goal of a departed voyage: `Err` names
/// the voyage and each changed field, else `Ok` with the empty list.
pub fn check(tx: &Tx<'_>, project: &str, draft: &Draft) -> GearResult<Vec<GoalChange>> {
    let changes = changes(tx, project, draft)?;
    let Some(first) = changes.first() else {
        return Ok(changes);
    };
    let milestone = first.milestone.clone();
    let mut text = format!(
        "voyage {milestone} has departed: its goal belongs to the user and is locked. This apply changes it:"
    );
    for change in changes.iter().filter(|change| change.milestone == milestone) {
        text.push_str(&format!(
            "\n  {}: «{}» → «{}»",
            change.field, change.before, change.after
        ));
    }
    text.push_str("\nPut the stored words back.");
    Err(GearError::new(ErrorCode::GoalLocked, text))
}

/// Stations as one text to compare and show: `<id>: <needs>`, joined by "; ".
fn stations_text(stations: &[VoyageStation]) -> String {
    stations
        .iter()
        .map(|station| format!("{}: {}", station.id, station.needs))
        .collect::<Vec<String>>()
        .join("; ")
}
