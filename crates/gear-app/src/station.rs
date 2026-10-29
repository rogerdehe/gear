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
        let mut past_quarter = false;
        // The course station (design §10.7 row B10): besides the 25/50/75%
        // eta stations, the captain gets a station every 60 min of voyage
        // time. The hour counter is a meta, so a late engine tick does not
        // replay the hours it slept through.
        let mut course: Option<(String, i64)> = None;
        if left > 0 {
            let voyage = tx.get_voyage(project, &report.milestone)?.expect("report has a voyage");
            if let Some(departure) = voyage.departure {
                let hours = (now - departure).max(0) / 3600;
                let course_key = format!("{key}:course");
                let last = tx.get_meta(&course_key)?.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                if hours > last {
                    crossed.push(if hours - last == 1 {
                        format!("{} min", hours * 60)
                    } else {
                        format!("{}–{} min", (last + 1) * 60, hours * 60)
                    });
                    course = Some((course_key, hours));
                }
            }
            if let (Some(departure), Some(eta)) = (voyage.departure,
                report.eta.as_deref().and_then(eta_seconds))
            {
                // Every quarter the elapsed time has crossed rings, not only
                // 25/50/75%: a voyage past its eta keeps telling the captain
                // plainly that the eta is passed and by how much, until it
                // lands (T-414). Each quarter rings once.
                let elapsed = (now - departure).max(0);
                let max_quarter = i128::from(elapsed) * 4 / i128::from(eta);
                for quarter in 1..=max_quarter {
                    let marker = format!("{key}:eta:{quarter}");
                    if tx.get_meta(&marker)?.is_none() {
                        crossed.push(if quarter >= 4 {
                            past_quarter = true;
                            format!("{}% eta (past eta by {} min)", quarter * 25, (elapsed - eta) / 60)
                        } else {
                            format!("{}% eta", quarter * 25)
                        });
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
        // Every task of a watched voyage is done: the voyage must be landed by
        // the captain (report, freeze, ask about the lanes). Nothing else
        // would ever ring for it — M2 sat finished for 36 h (L-78).
        if left == 0 && done > 0 && tx.get_meta(&format!("{key}:land"))?.is_none() {
            let message = tx.insert_message(&NewMessage {
                meta: None,
                project: project.into(), thread: format!("station:{}", report.milestone),
                task: None, attempt: None, sender: "engine".into(), sender_gen: 0,
                recipient: captain.id.clone(), kind: MsgKind::System, urgent: true,
                body: with_goal(tx, project, &report.milestone, format!(
                    "Voyage {} is ready to land: all {done} tasks are done (elapsed {} / eta {}).\n\
1. Run `gear report voyage` and write the voyage report into # Decisions.\n\
2. Review it with the user first: the report, the ★ decisions (only those), the friction reports that led to a change. Write each correction as `- <date> user: <text>`.\n\
3. Only then ask about reclaiming the lanes and the release.\n\
Close this with `station <id>: land`.",
                    report.milestone, minutes(report.elapsed_min), report.eta.as_deref().unwrap_or("-")
                ))?,
                reply_to: None, needs_ack: false, state: MsgState::Open, created: now,
            })?;
            tx.set_meta(&format!("{key}:land"), &message.id)?;
            opened.push(message.id);
        }
        if crossed.is_empty() { continue; }
        let pace = report.pace().map(|p| format!("{p:.2}x")).unwrap_or_else(|| "-".into());
        // The lane that sets the finish is named (§10.7 row B4): the
        // slowest task alone does not say which lane holds the voyage back.
        let slowest = report
            .slowest_lane
            .as_ref()
            .map(|(lane, min)| format!("{lane} {min}m"))
            .unwrap_or_else(|| "-".into());
        // The per-lane projection (design §10.7 row B4): every lane's own
        // pace and what it has left. The slowest lane sets the finish and is
        // named; a lane without a measured pace says so instead of guessing.
        let lanes = if report.lanes.is_empty() {
            "-".to_string()
        } else {
            report.lanes.iter().map(|lane| {
                let projection = lane.projected_min.map(|m| format!("{m}m")).unwrap_or_else(|| "-".into());
                format!("{} {projection}", lane.lane)
            }).collect::<Vec<_>>().join(", ")
        };
        // The time left until the eta: the clock the captain watches.
        let time_left = match (report.eta.as_deref().and_then(eta_seconds), report.elapsed_min) {
            (Some(eta), Some(elapsed)) => format!("{}m", (((eta + 59) / 60) - elapsed).max(0)),
            _ => "-".into(),
        };
        let body = format!(
            "Station {} [{}] · elapsed {} / eta {} · time left {time_left} · pace {pace} · projected finish {} from departure · tasks left {left} ({}) · lanes {lanes} · slowest lane {slowest}",
            report.milestone, crossed.join(", "), minutes(report.elapsed_min),
            report.eta.as_deref().unwrap_or("-"), minutes(report.projected_min),
            minutes(report.remaining_estimate_min),
        );
        // Urgent when the projected finish is past the eta (row B10): the
        // captain must answer before the voyage is late.
        let past_eta = match (report.eta.as_deref().and_then(eta_seconds), report.projected_min) {
            (Some(eta), Some(projected)) => projected.saturating_mul(60) > eta,
            _ => false,
        };
        // A station that reports a crossed 100%+ quarter is urgent whatever
        // the projection says: the voyage is past its eta now.
        let body = if past_quarter {
            format!("{body}\nPast the eta: speed up (rebalance lanes, cut waiting, fix rework fast); the eta does not move.")
        } else {
            body
        };
        let message = tx.insert_message(&NewMessage {
            meta: None,
            project: project.into(), thread: format!("station:{}", report.milestone),
            task: None, attempt: None, sender: "engine".into(), sender_gen: 0,
            recipient: captain.id.clone(), kind: MsgKind::System, urgent: past_eta || past_quarter,
            body: with_goal(tx, project, &report.milestone, body)?, reply_to: None, needs_ack: false, state: MsgState::Open, created: now,
        })?;
        for marker in markers { tx.set_meta(&marker, &message.id)?; }
        if let Some((course_key, hours)) = course {
            tx.set_meta(&course_key, &hours.to_string())?;
        }
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
        let valid = ["keep course", "time-box", "cut", "move", "land"].iter().any(|action| {
            response.strip_prefix(action).is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        });
        if !valid { return Err(bad("station action must be keep course, time-box, cut, move or land")); }
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

/// The captain's message body: the voyage's goal line first, the text of
/// today second (T-336). Nothing changes when the voyage has no goal text.
fn with_goal(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
    today: String,
) -> GearResult<String> {
    Ok(match gear_board::stations::goal_text(tx, project, milestone)? {
        Some(goal) => format!("{goal}\n{today}"),
        None => today,
    })
}

fn bad(message: impl Into<String>) -> GearError {
    GearError::new(ErrorCode::BadFormat, message)
}

/// Positive relative ETA durations: 20h, 2.5 hours, 90m, 1h30m, 2d 4h.
/// Absolute dates/prose are not guessed; task-completion stations still work.
///
/// One or more `<number><unit>` groups and nothing else: `apply` refuses a
/// milestone whose `eta:` this does not read (T-387).
pub(crate) fn eta_seconds(text: &str) -> Option<i64> {
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
