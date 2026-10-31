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
            // The block the captain has to write: the title the store keeps, or
            // the placeholder, since an empty or a repeated id reads like a real
            // title (T-333, the captain's answer).
            let title = stored
                .as_ref()
                .map(|voyage| voyage.title.trim())
                .filter(|title| !title.is_empty())
                .unwrap_or("<title>");
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

// ------------------------------------------------------- the captain's goal check

/// One line of the captain's goal check (T-461): `- done (<n>): met |
/// evidence: <what proves it>` or `- done (<n>): open | <why>`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoneCheck {
    /// The clause number, 1-based.
    pub n: usize,
    /// Whether the clause is met.
    pub met: bool,
    /// The clause of the voyage's `done:` sentence.
    pub text: String,
    /// What proves the clause met (`met` lines).
    pub evidence: Option<String>,
    /// Why the clause is open (`open` lines).
    pub why: Option<String>,
}

/// Whether a `# Decisions` line is a goal-check line (T-461) rather than a
/// decision: `- done (<n>): …`.
pub fn is_done_line(line: &str) -> bool {
    line.trim_start().starts_with("done (")
}

/// Parse one goal-check line (T-461), without its leading `- `.
pub fn parse_done_line(line: &str) -> Result<DoneCheck, String> {
    let line = line.trim();
    let rest = line
        .strip_prefix("done (")
        .ok_or_else(|| format!("a goal-check line is `done (<n>): met | evidence: …` or `done (<n>): open | <why>`, got {line:?}"))?;
    let (number, rest) = rest.split_once(')').ok_or_else(|| {
        format!("a goal-check line needs its clause number in `done (<n>)`, got {line:?}")
    })?;
    let n: usize = number
        .trim()
        .parse()
        .ok()
        .filter(|n: &usize| *n > 0)
        .ok_or_else(|| format!("`done (<n>)` takes a positive clause number, got {number:?}"))?;
    let rest = rest
        .trim_start()
        .strip_prefix(':')
        .ok_or_else(|| format!("a goal-check line needs `: met …` or `: open …`, got {line:?}"))?
        .trim();
    if let Some(after) = rest.strip_prefix("met") {
        let after = after.trim();
        let evidence = if after.is_empty() {
            None
        } else {
            let value = after
                .strip_prefix('|')
                .map(str::trim_start)
                .and_then(|value| value.strip_prefix("evidence:"))
                .map(str::trim)
                .ok_or_else(|| {
                    format!("a met clause is `met | evidence: <what proves it>`, got {line:?}")
                })?;
            if value.is_empty() {
                return Err(format!(
                    "`evidence:` needs what proves the clause, got {line:?}"
                ));
            }
            Some(value.to_string())
        };
        return Ok(DoneCheck {
            n,
            met: true,
            text: String::new(),
            evidence,
            why: None,
        });
    }
    if let Some(after) = rest.strip_prefix("open") {
        let after = after.trim();
        let why = after
            .strip_prefix('|')
            .map(str::trim)
            .filter(|why| !why.is_empty())
            .ok_or_else(|| format!("an open clause is `open | <why>`, got {line:?}"))?;
        return Ok(DoneCheck {
            n,
            met: false,
            text: String::new(),
            evidence: None,
            why: Some(why.to_string()),
        });
    }
    Err(format!(
        "a goal-check line is `met …` or `open …`, got {line:?}"
    ))
}

/// The goal-check lines of a draft (T-461), in clause order; a malformed line
/// or a repeated clause number is refused.
pub fn done_checks(decisions: &[String]) -> GearResult<Vec<DoneCheck>> {
    let mut checks: Vec<DoneCheck> = Vec::new();
    for line in decisions {
        if !is_done_line(line) {
            continue;
        }
        let check = parse_done_line(line)
            .map_err(|e| GearError::new(ErrorCode::BadFormat, format!("# Decisions: {e}")))?;
        if checks.iter().any(|known| known.n == check.n) {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "# Decisions: clause ({}) appears twice in the goal check",
                    check.n
                ),
            ));
        }
        checks.push(check);
    }
    checks.sort_by_key(|check| check.n);
    Ok(checks)
}

/// Write the captain's goal check into the meta the board reads (T-461):
/// `goal-check:<project>:<milestone>` is a JSON array of `{"met": bool,
/// "text": str, "evidence": str, "why": str}`; the board reads `met` and
/// `text`, the report also shows the evidence or why.
pub fn record_goal_check(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    draft: &Draft,
) -> GearResult<()> {
    let checks = done_checks(&draft.decisions)?;
    if checks.is_empty() {
        return Ok(());
    }
    let voyage = tx.get_voyage(project, milestone)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("the goal check names no voyage: {milestone} is not a voyage of {project}"),
        )
    })?;
    let clauses = crate::voyage::clauses_of(voyage.done.as_deref().unwrap_or_default());
    if clauses.is_empty() {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("milestone {milestone} has no done sentence to check"),
        ));
    }
    let numbers: Vec<usize> = checks.iter().map(|check| check.n).collect();
    let expected: Vec<usize> = (1..=clauses.len()).collect();
    if numbers != expected {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "the goal check of {milestone} needs one line per done clause ({} clauses), got {}",
                clauses.len(),
                numbers
                    .iter()
                    .map(|n| format!("({n})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let stored: Vec<serde_json::Value> = checks
        .iter()
        .zip(clauses.iter())
        .map(|(check, clause)| {
            let mut item = serde_json::json!({
                "met": check.met,
                "text": clause,
            });
            if let Some(evidence) = &check.evidence {
                item["evidence"] = serde_json::Value::String(evidence.clone());
            }
            if let Some(why) = &check.why {
                item["why"] = serde_json::Value::String(why.clone());
            }
            item
        })
        .collect();
    let raw = serde_json::to_string(&stored).map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot serialize the goal check: {e}"),
        )
    })?;
    tx.set_meta(&format!("goal-check:{project}:{milestone}"), &raw)?;
    Ok(())
}

/// The goal check the board reads (T-461): the meta's clauses, or the
/// voyage's own done clauses, every one open, when no check is written.
pub fn goal_check(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    done: Option<&str>,
) -> GearResult<Vec<DoneCheck>> {
    if let Some(raw) = tx.get_meta(&format!("goal-check:{project}:{milestone}"))? {
        if let Some(checks) = parse_goal_check(&raw) {
            return Ok(checks);
        }
    }
    Ok(crate::voyage::clauses_of(done.unwrap_or_default())
        .into_iter()
        .enumerate()
        .map(|(index, text)| DoneCheck {
            n: index + 1,
            met: false,
            text,
            evidence: None,
            why: None,
        })
        .collect())
}

/// The meta's goal check, as the board's parser reads it plus the evidence and
/// why the report shows.
fn parse_goal_check(raw: &str) -> Option<Vec<DoneCheck>> {
    let clauses: Vec<serde_json::Value> = serde_json::from_str(raw).ok()?;
    let mut checks = Vec::with_capacity(clauses.len());
    for (index, clause) in clauses.into_iter().enumerate() {
        let met = clause.get("met")?.as_bool()?;
        let text = clause.get("text")?.as_str()?.to_string();
        checks.push(DoneCheck {
            n: index + 1,
            met,
            text,
            evidence: clause
                .get("evidence")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            why: clause
                .get("why")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    Some(checks)
}
