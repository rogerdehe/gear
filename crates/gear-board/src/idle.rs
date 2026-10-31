//! Why a lane is idle (design §8.2, §13.6).
//!
//! One function for the board, `gear queue` and the imbalance item: the
//! reason a lane has nothing to do, in the order of the table of §8.2. The
//! first fact that fits wins:
//!
//! | reason | when | shown |
//! |---|---|---|
//! | `Quota { until }` | the seat is quota-parked | `quota until 15:12` |
//! | `Parked { until }` | the seat is parked | `parked until …` |
//! | `Held { reason }` | the seat is held | `held: …` |
//! | `Waiting { on }` | the lane waits for a task to be merged (§5.3) | `waiting for T-m (lane, stage)` |
//! | `DepWait { task, on }` | the next task of the lane waits for a dependency | `next T-x waits for T-y (lane, stage)` |
//! | `Undelivered { task }` | the next task is ready and was not delivered | `next T-x is ready but not delivered` |
//! | `NoWork` | nothing is left in the lane's queue | `nothing left planned` |
//!
//! A lane that works has no reason (`None`).

use gear_model::{GearResult, MsgKind, SeatRole, SeatState, TaskState, Ts};
use gear_store::{MessageRow, SeatRow, TaskRow, Tx};

use crate::state_word;

/// How long an unanswered instruction of the captain keeps a lane busy (T-323):
/// half an hour. A lane that never answers comes back to the idle and stall
/// rules after it.
pub const HELP_LIMIT_S: i64 = 1800;

/// A lane that helps on an unanswered instruction of the captain (T-323, S2
/// "work that is not a task"): the lane holds no attempt, but the captain
/// asked it for something and it has not answered yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Helping {
    /// The id of the instruction (`M-<n>`).
    pub message: String,
    /// The lane the instruction is about, when it is another lane's.
    pub lane: Option<String>,
    /// The first words of the instruction.
    pub words: String,
    /// When the instruction was sent.
    pub since: Ts,
}

impl Helping {
    /// The text of the help: `helping <lane>: <words>`.
    pub fn text(&self) -> String {
        match &self.lane {
            Some(lane) => format!("helping {lane}: {}", self.words),
            None => format!("helping the captain: {}", self.words),
        }
    }
}

/// Why a lane is idle (§8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdleReason {
    /// The seat is quota-parked: the provider's window is spent.
    Quota {
        /// When the provider's window starts again.
        until: Ts,
    },
    /// The seat is parked (the plan parked it).
    Parked {
        /// When the seat is let go again; None when the store does not say.
        until: Option<Ts>,
    },
    /// The seat is held (the plan holds it).
    Held {
        /// Why the seat is held.
        reason: String,
    },
    /// The lane waits for a task to be merged (§5.3).
    Waiting {
        /// The task the lane waits for.
        on: String,
    },
    /// The next task of the lane waits for a dependency.
    DepWait {
        /// The task that waits.
        task: String,
        /// The dependency it waits for.
        on: String,
    },
    /// The next task of the lane is ready and was never delivered.
    Undelivered {
        /// The task that waits for the lane.
        task: String,
    },
    /// Nothing is left in the lane's queue.
    NoWork,
}

/// The reason the lane is idle; None while it works (§13.6).
///
/// `lane` is the name of the lane (the seat is `<project>:<lane>`); a lane
/// that is given as its seat id is read as it is.
pub fn idle_reason(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    now: Ts,
) -> GearResult<Option<IdleReason>> {
    let seat_id = seat_id_of(project, lane);
    let seat = tx.get_seat(&seat_id)?;

    // The seat's own state comes first: a parked lane is idle whatever its
    // queue says.
    if let Some(seat) = seat.as_ref() {
        match seat.state {
            SeatState::Parked => {
                let quota = is_quota_park(tx, seat)?;
                let until = if quota {
                    Some(quota_until(tx, seat)?)
                } else {
                    park_until(seat)
                };
                // The quota park lasts until the provider's window starts
                // again (§9.2): after that the engine continues the lane, and
                // the seat is only parked until it does.
                match until {
                    Some(until) if quota && until > now => {
                        return Ok(Some(IdleReason::Quota { until }))
                    }
                    until => return Ok(Some(IdleReason::Parked { until })),
                }
            }
            SeatState::Held => {
                return Ok(Some(IdleReason::Held {
                    reason: held_reason(seat),
                }));
            }
            _ => {}
        }
    }

    // The lane's queue: its tasks, in the order of the plan.
    let queue = queue_of(tx, project, lane)?;

    // A declared wait (§5.3): the lane waits for a task to be merged. The
    // attempt of the task stays Started, so this comes before the working
    // check.
    if let Some(task) = queue.iter().find(|task| task.wait_on.is_some()) {
        return Ok(Some(IdleReason::Waiting {
            on: task.wait_on.clone().unwrap_or_default(),
        }));
    }

    // The lane works: it has an attempt in flight.
    if is_working(tx, &seat_id)? {
        return Ok(None);
    }

    // The lane helps on an unanswered instruction of the captain (T-323): it
    // has no attempt, but it is not idle either.
    if let Some(seat) = seat.as_ref() {
        if helping(tx, project, seat, now)?.is_some() {
            return Ok(None);
        }
    }

    // The next task of the queue: the first one that is queued. A task that
    // was handed in waits for its review in the background and does not hide
    // the work that is left (the engine gives the lane its next task while
    // the review runs).
    let Some(next) = queue.iter().find(|task| is_queued(task.state)) else {
        return Ok(Some(IdleReason::NoWork));
    };

    // A dependency of it that is not done yet.
    for dep in &next.deps {
        let done = tx
            .get_task(project, dep)?
            .map(|task| task.state == TaskState::Done)
            .unwrap_or(false);
        if !done {
            return Ok(Some(IdleReason::DepWait {
                task: next.id.clone(),
                on: dep.clone(),
            }));
        }
    }

    // The task is queued and nobody gave it to the lane.
    Ok(Some(IdleReason::Undelivered {
        task: next.id.clone(),
    }))
}

/// The instruction of the captain the lane still has to answer, if any
/// (T-323).
///
/// None unless the seat is a lane, holds no attempt in flight and the project
/// has a captain; then the newest unanswered captain instruction to the seat
/// within [`HELP_LIMIT_S`] is the help. A reply of the lane — any message of
/// it with `reply_to` the instruction — ends it.
pub fn helping(tx: &Tx<'_>, project: &str, seat: &SeatRow, now: Ts) -> GearResult<Option<Helping>> {
    if seat.role != SeatRole::Lane || !tx.in_progress_attempts_of_seat(&seat.id)?.is_empty() {
        return Ok(None);
    }
    let Some(captain) = tx.captain_of(project)? else {
        return Ok(None);
    };
    let mut instructions: Vec<MessageRow> = tx
        .messages_to_since(project, &seat.id, now - HELP_LIMIT_S)?
        .into_iter()
        .filter(|message| message.kind == MsgKind::Instruction)
        .filter(|message| message.sender == captain.id)
        .collect();
    instructions.sort_by(|a, b| (b.created, &b.id).cmp(&(a.created, &a.id)));
    for instruction in instructions {
        if has_reply(tx, project, &seat.id, &instruction)? {
            continue;
        }
        let lane = match instruction.task.as_deref() {
            Some(task) => tx.get_task(project, task)?.and_then(|task| task.lane),
            None => None,
        };
        let lane = lane.filter(|lane| *lane != seat.name);
        return Ok(Some(Helping {
            message: instruction.id.clone(),
            lane,
            words: words_of(&instruction.body),
            since: instruction.created,
        }));
    }
    Ok(None)
}

/// Did the lane answer the instruction?
fn has_reply(
    tx: &Tx<'_>,
    project: &str,
    seat_id: &str,
    instruction: &MessageRow,
) -> GearResult<bool> {
    Ok(tx
        .messages_from_since(project, seat_id, instruction.created)?
        .into_iter()
        .any(|message| message.reply_to.as_deref() == Some(instruction.id.as_str())))
}

/// The first six words of the body, and "…" when there are more.
fn words_of(body: &str) -> String {
    let words: Vec<&str> = body.split_whitespace().collect();
    let head: Vec<&str> = words.iter().take(6).copied().collect();
    let mut text = head.join(" ");
    if words.len() > head.len() {
        text.push_str(" …");
    }
    text
}

/// The text of the reason (§8.2).
pub fn describe(reason: &IdleReason, tx: &Tx<'_>, project: &str) -> String {
    match reason {
        IdleReason::Quota { until } => format!("quota until {}", clock(*until)),
        IdleReason::Parked { until } => match until {
            Some(until) => format!("parked until {}", clock(*until)),
            // The store does not say when the seat is let go.
            None => "parked until …".to_string(),
        },
        IdleReason::Held { reason } => format!("held: {reason}"),
        IdleReason::Waiting { on } => {
            format!("waiting for {on} ({})", lane_and_stage(tx, project, on))
        }
        IdleReason::DepWait { task, on } => {
            format!(
                "next {task} waits for {on} ({})",
                lane_and_stage(tx, project, on)
            )
        }
        IdleReason::Undelivered { task } => format!("next {task} is ready but not delivered"),
        IdleReason::NoWork => "nothing left planned".to_string(),
    }
}

/// The seat of a lane: `<project>:<lane>`, or the seat id itself.
fn seat_id_of(project: &str, lane: &str) -> String {
    if lane.contains(':') {
        lane.to_string()
    } else {
        format!("{project}:{lane}")
    }
}

/// The tasks of a lane, in the order of the plan. A task that is not in the
/// plan yet (a draft) is not in the queue.
fn queue_of(tx: &Tx<'_>, project: &str, lane: &str) -> GearResult<Vec<TaskRow>> {
    let mut queue: Vec<TaskRow> = tx
        .tasks_of_project(project)?
        .into_iter()
        .filter(|task| task.state != TaskState::Draft)
        .filter(|task| task.lane.as_deref() == Some(lane))
        .collect();
    queue.sort_by_key(|task| (task.rank, task.id.clone()));
    Ok(queue)
}

/// Is the task queued? A task that was handed in (it waits for its review in
/// the background) is not: the lane may take its next task meanwhile.
fn is_queued(state: TaskState) -> bool {
    matches!(state, TaskState::Ready | TaskState::Waiting)
}

/// Does the lane work? It has an attempt in flight.
fn is_working(tx: &Tx<'_>, seat_id: &str) -> GearResult<bool> {
    Ok(tx
        .open_attempts_of_seat(seat_id)?
        .into_iter()
        .any(|attempt| {
            matches!(
                attempt.state,
                gear_model::AttemptState::Delivered
                    | gear_model::AttemptState::Started
                    | gear_model::AttemptState::Stopped
                    | gear_model::AttemptState::Unknown
            )
        }))
}

/// Is the seat parked on quota?
///
/// The engine parks a lane on quota and says so in the seat's reason
/// (`quota: <model>; resume at <time>`); the store's pause of the seat
/// (§5.4) says it too.
fn is_quota_park(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<bool> {
    if seat
        .state_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("quota:"))
    {
        return Ok(true);
    }
    Ok(tx
        .pauses_of_project(&seat.project)?
        .into_iter()
        .any(|pause| pause.seat == seat.id && pause.kind == "quota" && pause.end_at.is_none()))
}

/// When the provider's window starts again.
///
/// The pause of the seat (§5.4) is the fact of the store; the engine's own
/// deadline (`engine.resume.<seat>`) is the fact of today, and the reason the
/// seat recorded carries it as well.
fn quota_until(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Ts> {
    if let Some(pause) = tx
        .pauses_of_project(&seat.project)?
        .into_iter()
        .find(|pause| pause.seat == seat.id && pause.kind == "quota" && pause.end_at.is_none())
    {
        if let Some(until) = pause.end_at {
            return Ok(until);
        }
    }
    if let Some(until) = tx
        .get_meta(&format!("engine.resume.{}", seat.id))?
        .and_then(|value| value.trim().parse::<Ts>().ok())
    {
        return Ok(until);
    }
    if let Some(until) = seat
        .state_reason
        .as_deref()
        .and_then(|reason| reason.split("resume at ").nth(1))
        .and_then(|until| until.trim().parse::<Ts>().ok())
    {
        return Ok(until);
    }
    // Nothing says when the provider is back: the seat's own state since is
    // the best the board has.
    Ok(seat.state_since)
}

/// When the seat is let go again.
///
/// The plan parks a lane with `until=<RFC 3339>` (or `until-task=T-n`); the
/// time of the first is read, the second is not a time.
fn park_until(seat: &SeatRow) -> Option<Ts> {
    let reason = seat.state_reason.as_deref()?;
    let until = reason.strip_prefix("until=")?;
    rfc3339(until)
}

/// Why the seat is held: the plan holds it with a reason.
fn held_reason(seat: &SeatRow) -> String {
    let reason = seat.state_reason.as_deref().unwrap_or_default();
    match reason.strip_prefix("hold: ") {
        Some(reason) => reason.to_string(),
        None if reason.is_empty() => "held".to_string(),
        None => reason.to_string(),
    }
}

/// The lane and the stage of a task: `(hull, queued)`.
fn lane_and_stage(tx: &Tx<'_>, project: &str, task: &str) -> String {
    match tx.get_task(project, task) {
        Ok(Some(row)) => {
            let lane = row.lane.unwrap_or_else(|| "-".to_string());
            format!("{}, {}", lane, state_word(row.state))
        }
        _ => "-".to_string(),
    }
}

/// The time of day, as the user's clock sees it.
pub fn clock(until: Ts) -> String {
    let seconds = until as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let local = unsafe { libc::localtime_r(&seconds, &mut parts) };
    let (hour, minute) = if local.is_null() {
        // No local time: the clock of the machine is not readable.
        (((until / 3600) % 24), ((until % 3600) / 60))
    } else {
        (parts.tm_hour as i64, parts.tm_min as i64)
    };
    format!("{hour:02}:{minute:02}")
}

/// The English three-letter names of the months.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The day and the time of day, as the user's clock sees it (`Oct 30 00:35`).
pub fn day_clock(until: Ts) -> String {
    let seconds = until as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let local = unsafe { libc::localtime_r(&seconds, &mut parts) };
    let (month, day, hour, minute) = if local.is_null() {
        // No local time: the clock of the machine is not readable.
        let (month, day) = civil_from_days(until.div_euclid(86_400));
        (month, day, (until / 3600) % 24, (until % 3600) / 60)
    } else {
        (
            parts.tm_mon as i64 + 1,
            parts.tm_mday as i64,
            parts.tm_hour as i64,
            parts.tm_min as i64,
        )
    };
    let month = MONTHS[(month.clamp(1, 12) - 1) as usize];
    format!("{month} {day} {hour:02}:{minute:02}")
}

/// The local date, as the user's clock sees it (`Oct 30`): a quota window that
/// resets in more than a day names its day, not its minute (T-360).
pub fn month_day(until: Ts) -> String {
    let seconds = until as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let local = unsafe { libc::localtime_r(&seconds, &mut parts) };
    let (month, day) = if local.is_null() {
        // No local time: the clock of the machine is not readable.
        civil_from_days(until.div_euclid(86_400))
    } else {
        (parts.tm_mon as i64 + 1, parts.tm_mday as i64)
    };
    let month = MONTHS[(month.clamp(1, 12) - 1) as usize];
    format!("{month} {day}")
}

/// The month and the day of a count of days since 1970-01-01, in UTC.
///
/// The fallback of [`day_clock`], when the machine's local time is not
/// readable.
fn civil_from_days(days: i64) -> (i64, i64) {
    let days = days + 719_468;
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (month, day)
}

/// An RFC 3339 time (`2026-10-27T10:00:00Z`) in epoch seconds.
pub(crate) fn rfc3339(text: &str) -> Option<Ts> {
    let text = text.trim();
    if text.len() < 19 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: i64 = text.get(5..7)?.parse().ok()?;
    let day: i64 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    let rest = &text[19..];
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.chars().take_while(|c| c.is_ascii_digit()).count();
            &fraction[digits..]
        }
        None => rest,
    };
    let offset = if rest.eq_ignore_ascii_case("z") {
        0
    } else if rest.starts_with('+') || rest.starts_with('-') {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let hours: i64 = rest.get(1..3)?.parse().ok()?;
        let minutes: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (hours * 3600 + minutes * 60)
    } else {
        0
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// The days since the epoch of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_position = (month + 9) % 12;
    let day_of_year = (153 * month_position + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
