//! Station reports (T-49): measured course checks, answered by a plan decision.
//! Checkpoint markers and the message are committed together. Simultaneous or
//! missed checkpoints coalesce into one immutable snapshot per voyage/tick.

use crate::draft::Draft;
use gear_model::{ErrorCode, GearError, GearResult, MsgKind, MsgState, TaskState, Ts};
use gear_store::{MessageRow, NewMessage, Tx};

pub fn is_station(message: &MessageRow) -> bool {
    message.sender == "engine" && message.kind == MsgKind::System
        && message.thread.starts_with("station:")
}

/// Return the ids opened this tick. Finished historical voyages are not
/// retroactively reported, but a watched voyage can send its final task station.
pub fn checkpoints(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<String>> {
    let Some(captain) = tx.captain_of(project)? else { return Ok(Vec::new()); };
    let tasks = tx.tasks_of_project(project)?;
    let mut opened = Vec::new();
    for report in gear_board::voyage::reports_including_finished(tx, project, now)? {
        let scope: Vec<_> = tasks.iter().filter(|task| {
            task.milestone.as_deref() == Some(&report.milestone)
                && !matches!(task.state, TaskState::Draft | TaskState::Cancelled | TaskState::Failed)
        }).collect();
        let done = scope.iter().filter(|t| t.state == TaskState::Done).count();
        let left = scope.len() - done;
        let key = format!("station:{project}:{}", report.milestone);
        if left == 0 && tx.get_meta(&format!("{key}:watch"))?.is_none() {
            continue;
        }
        if left > 0 { tx.set_meta(&format!("{key}:watch"), "1")?; }
        let mut crossed = Vec::new();
        let mut markers = Vec::new();
        if left > 0 {
            let voyage = tx.get_voyage(project, &report.milestone)?.expect("report has a voyage");
            if let (Some(departure), Some(eta)) = (voyage.departure,
                report.eta.as_deref().and_then(eta_seconds))
            {
                for quarter in 1..=3 {
                    let marker = format!("{key}:eta:{quarter}");
                    if i128::from((now - departure).max(0)) * 4 >= i128::from(eta) * quarter
                        && tx.get_meta(&marker)?.is_none()
                    {
                        crossed.push(format!("{}% eta", quarter * 25));
                        markers.push(marker);
                    }
                }
            }
        }
        // Each multiple of three is a station, even after a late engine tick.
        // Reopened tasks cannot replay already-recorded completion milestones.
        let completed_key = format!("{key}:done");
        let previous = tx.get_meta(&completed_key)?.and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
        let groups = done / 3;
        if groups > previous {
            crossed.push(if groups == previous + 1 {
                format!("{} done", groups * 3)
            } else {
                format!("{}–{} done", (previous + 1) * 3, groups * 3)
            });
        }
        if crossed.is_empty() { continue; }
        let pace = report.pace().map(|p| format!("{p:.2}x")).unwrap_or_else(|| "-".into());
        let slowest = report.slowest.as_ref().map(|(id, min)| format!("{id} {min}m"))
            .unwrap_or_else(|| "-".into());
        let body = format!(
            "Station {} [{}] · elapsed {} / eta {} · pace {pace} · projected finish {} from departure · tasks left {left} ({}) · slowest {slowest}",
            report.milestone, crossed.join(", "), minutes(report.elapsed_min),
            report.eta.as_deref().unwrap_or("-"), minutes(report.projected_min),
            minutes(report.remaining_estimate_min),
        );
        let message = tx.insert_message(&NewMessage {
            project: project.into(), thread: format!("station:{}", report.milestone),
            task: None, attempt: None, sender: "engine".into(), sender_gen: 0,
            recipient: captain.id.clone(), kind: MsgKind::System, urgent: false,
            body, reply_to: None, needs_ack: false, state: MsgState::Open, created: now,
        })?;
        for marker in markers { tx.set_meta(&marker, &message.id)?; }
        if groups > previous { tx.set_meta(&completed_key, &groups.to_string())?; }
        opened.push(message.id);
    }
    Ok(opened)
}

/// Only an explicit station decision closes the named action item. The plan
/// transaction makes a bad reference/action roll back every other plan write.
pub fn apply_decisions(tx: &Tx<'_>, project: &str, draft: &Draft, now: Ts) -> GearResult<()> {
    for decision in &draft.decisions {
        let Some(rest) = decision.strip_prefix("station ") else { continue; };
        let (id, response) = rest.split_once(':').ok_or_else(|| bad("station decision needs `station M-n: action`"))?;
        let (id, response) = (id.trim(), response.trim());
        let valid = ["keep course", "time-box", "cut", "move"].iter().any(|action| {
            response.strip_prefix(action).is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        });
        if !valid { return Err(bad("station action must be keep course, time-box, cut or move")); }
        let message = tx.get_message(project, id)?.ok_or_else(|| bad(format!("no station item {id}")))?;
        let captain = tx.captain_of(project)?.ok_or_else(|| bad("no captain for station decision"))?;
        if !is_station(&message) || message.recipient != captain.id {
            return Err(bad(format!("{id} is not this captain's station item")));
        }
        if message.state == MsgState::Resolved { continue; } // retained historical decision
        tx.update_message_state(project, id, MsgState::Resolved, Some(decision), None, None, now)?;
    }
    Ok(())
}

fn minutes(value: Option<i64>) -> String {
    value.map(|n| format!("{n}m")).unwrap_or_else(|| "-".into())
}

fn bad(message: impl Into<String>) -> GearError {
    GearError::new(ErrorCode::BadFormat, message)
}

/// Positive relative ETA durations: 20h, 2.5 hours, 90m, 1h30m, 2d 4h.
/// Absolute dates/prose are not guessed; task-completion stations still work.
fn eta_seconds(text: &str) -> Option<i64> {
    let mut rest = text.trim();
    let mut total = 0.0;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
        let number: f64 = rest[..end].parse().ok()?;
        rest = rest[end..].trim_start();
        let end = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let factor = match rest[..end].to_ascii_lowercase().as_str() {
            "d" | "day" | "days" => 86400.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            _ => return None,
        };
        total += number * factor;
        rest = rest[end..].trim_start();
    }
    (total.is_finite() && total >= 1.0 && total < i64::MAX as f64)
        .then(|| total.ceil() as i64)
}

#[cfg(test)]
mod tests {
    use super::eta_seconds;
    #[test]
    fn eta_duration_units_and_unknown_values() {
        for (input, expected) in [("20h",72000),("2.5 hours",9000),("90m",5400),
            ("1h30m",5400),("2d 4h",187200),("60s",60)] {
            assert_eq!(eta_seconds(input), Some(expected), "{input}");
        }
        for input in ["", "0h", "-1h", "NaNh", "2026-10-24", "soon", "2", "1h junk"] {
            assert_eq!(eta_seconds(input), None, "{input}");
        }
    }
}
