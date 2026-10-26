//! Friction reports (design §6): what cost a lane or a reviewer time that
//! the captain could fix, and what the captain did about it.
//!
//! A report is written at the hand-in (`--friction`, design §6.1) and is
//! required when the round passed its budget (§6.2). The captain reads them
//! with `gear friction` and closes them with `gear friction close F-<n> -m
//! "<outcome>"` (§6.4).

use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, Ts};
use gear_store::{FrictionRow, Store};

/// The outcomes a closed report may carry (design §6.4): fixed (what),
/// lesson (which), not worth it (why).
const OUTCOMES: [&str; 3] = ["fixed", "lesson", "not worth it"];

/// The friction reports of a project: the open ones, or every one of them
/// (`all`), each with the round's own facts (design §6.3).
pub fn list(store: &mut Store, project: &str, all: bool) -> GearResult<String> {
    let rows = store.read(|tx| {
        if all {
            tx.frictions_of_project(project)
        } else {
            tx.open_frictions(project)
        }
    })?;
    if rows.is_empty() {
        return Ok(if all {
            "no friction reports".to_string()
        } else {
            "no open friction reports".to_string()
        });
    }
    let mut text = String::new();
    for row in &rows {
        let facts = store.read(|tx| facts_of(tx, row))?;
        text.push_str(&format!(
            "{} {} attempt {} ({}){}{}\n",
            row.id,
            row.task,
            row.attempt,
            row.role,
            if row.required { " · required" } else { "" },
            match (&row.closed_at, &row.outcome) {
                (Some(_), Some(outcome)) => format!(" · closed: {outcome}"),
                _ => " · open".to_string(),
            }
        ));
        text.push_str(&format!(
            "  budget {} min · active {} min · paused {} min · {} round(s) so far · tokens {} in / {} out\n",
            facts.budget,
            facts.active,
            facts.paused,
            facts.rounds,
            facts.input,
            facts.output
        ));
        text.push_str(&format!("  {}\n", row.body));
    }
    Ok(text.trim_end().to_string())
}

/// Close a report (design §6.4): what the captain did about it — fixed
/// (what), lesson (which) or not worth it (why).
pub fn close(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    project: &str,
    id: &str,
    outcome: &str,
    now: Ts,
) -> GearResult<String> {
    store.write(|tx| {
        // The captain closes a report (design §6.4).
        require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Captain]),
            Some(&[SeatState::Active]),
        )?;
        if !OUTCOMES.iter().any(|kind| outcome.starts_with(kind)) {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "the outcome is one of {}: fixed (what), lesson (which), not worth it (why)",
                    OUTCOMES.join(", ")
                ),
            ));
        }
        tx.close_friction(project, id, outcome, now)?;
        Ok(format!("closed {id}: {outcome}"))
    })
}

/// The round's own facts, next to the report (design §6.3): its budget, the
/// time it was active, the time it was paused, the rounds so far and its
/// tokens.
struct Facts {
    budget: i64,
    active: i64,
    paused: i64,
    rounds: usize,
    input: i64,
    output: i64,
}

fn facts_of(tx: &gear_store::Tx<'_>, row: &FrictionRow) -> GearResult<Facts> {
    let rounds = tx.rounds_of_task(&row.project, &row.task)?;
    let round = rounds
        .iter()
        .find(|round| round.attempt == row.attempt && round.role == row.role);
    let model = round.and_then(|round| round.model.as_deref());
    let budget = tx.round_budget_min(&row.project, &row.task, row.attempt, &row.role, model)?;
    Ok(Facts {
        budget,
        active: round.map(|round| round.active_s / 60).unwrap_or(0),
        paused: round.map(|round| round.paused_s / 60).unwrap_or(0),
        rounds: rounds.len(),
        input: round.and_then(|round| round.input).unwrap_or(0),
        output: round.and_then(|round| round.output).unwrap_or(0),
    })
}
