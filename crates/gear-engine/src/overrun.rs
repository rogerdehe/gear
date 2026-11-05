//! Round clocks and the overrun question (design §5.1–§5.2).
use crate::EngineCfg;
use gear_herdr::HerdrAct;
use gear_model::{AttemptState, GearResult, MsgKind, MsgState, SeatState, TaskState, Ts};
use gear_store::{NewMessage, RoundKey, RoundRow, Store, Tx};
use serde_json::{json, Value};

fn key(round: &RoundRow) -> String {
    format!(
        "engine.overrun.{}.{}.{}.{}",
        round.project, round.task, round.attempt, round.role
    )
}
fn round_key(round: &RoundRow) -> RoundKey {
    RoundKey {
        project: round.project.clone(),
        task: round.task.clone(),
        attempt: round.attempt,
        role: round.role.clone(),
    }
}
fn text(task: &str, attempt: i64, contract: i64, budget: i64, active: i64) -> String {
    format!("GEAR: {task} has reached its estimate ({budget} min; {} min so far).\nAnswer with one command, then keep working:\ngear send --kind progress --task {task} --attempt {attempt} --contract {contract} --left <minutes> --blocker \"<what stops you, or none>\" \"<done so far; what is left>\"", active / 60)
}

/// The budget of a stage check (T-591): a direction round of a feature member
/// is a light look at the candidate, not the task's work, so it gets five
/// minutes and not the task's estimate.
const STAGE_CHECK_BUDGET_MIN: i64 = 5;

/// The one reminder a stage check gets (T-591). It says what the check is — a
/// stage check of a feature, whose full review comes once, on the feature's
/// integration task — so the reviewer reports what it has and keeps going.
fn stage_check_text(task: &str, feature: &str) -> String {
    format!(
        "GEAR: {task} is a stage check of feature {feature}, and {STAGE_CHECK_BUDGET_MIN} minutes have passed. Report what you found now. If you have a lead that needs more time, reply with one line saying why and keep going; the full review comes on the feature's integration task."
    )
}

/// The feature whose stage check this round runs (T-591): a direction round of
/// a task marked `feature: F-<n>` that is not the feature's own integration
/// task (the carrier is never marked). None for every other round.
fn stage_check(tx: &Tx<'_>, round: &RoundRow) -> GearResult<Option<String>> {
    if round.role != "direction" {
        return Ok(None);
    }
    tx.task_feature(&round.project, &round.task)
}

/// The stage check's reply (T-591): a progress line its seat sent after the
/// reminder — "one line saying why and keep going". It is not an answer with
/// `--left`, so it neither asks for a captain item nor moves the clock: it
/// only silences the reminder for this round.
fn stage_check_reply(
    tx: &Tx<'_>,
    round: &RoundRow,
    after: Ts,
    until: Ts,
) -> GearResult<Option<gear_store::MessageRow>> {
    let mut messages = tx.progress_of_attempt(&round.project, &round.task, round.attempt)?;
    messages.retain(|m| m.sender == round.seat && m.created >= after && m.created <= until);
    messages.sort_by_key(|m| {
        (
            m.created,
            m.id.trim_start_matches("M-").parse::<u64>().unwrap_or(0),
        )
    });
    Ok(messages.pop())
}

/// Union of the seat's pauses in a time window: overlapping quota and wait
/// pauses count only once.
fn paused(tx: &Tx<'_>, round: &RoundRow, start: Ts, end: Ts) -> GearResult<i64> {
    let mut intervals: Vec<(Ts, Ts)> = tx
        .pauses_of_project(&round.project)?
        .into_iter()
        .filter(|p| p.seat == round.seat)
        .map(|p| (p.start_at.max(start), p.end_at.unwrap_or(end).min(end)))
        .filter(|(a, b)| b > a)
        .collect();
    intervals.sort_unstable();
    let mut total = 0;
    let mut current: Option<(Ts, Ts)> = None;
    for (a, b) in intervals {
        match current {
            Some((x, y)) if a <= y => current = Some((x, y.max(b))),
            Some((x, y)) => {
                total += y - x;
                current = Some((a, b));
            }
            None => current = Some((a, b)),
        }
    }
    if let Some((a, b)) = current {
        total += b - a;
    }
    Ok(total)
}
fn active_at(tx: &Tx<'_>, round: &RoundRow, start: Ts, end: Ts) -> GearResult<i64> {
    Ok((end - start - paused(tx, round, start, end)?).max(0))
}

fn answers(tx: &Tx<'_>, round: &RoundRow) -> GearResult<Vec<gear_store::MessageRow>> {
    let mut messages = tx.progress_of_attempt(&round.project, &round.task, round.attempt)?;
    messages.retain(|m| {
        m.sender == round.seat
            && m.meta
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .and_then(|v| v.get("left").and_then(Value::as_i64))
                .is_some()
    });
    messages.sort_by_key(|m| {
        (
            m.created,
            m.id.trim_start_matches("M-").parse::<u64>().unwrap_or(0),
        )
    });
    Ok(messages)
}

fn item(tx: &Tx<'_>, round: &RoundRow, body: &str, urgent: bool, now: Ts) -> GearResult<()> {
    let thread = format!("overrun:{}:{}", round.task, round.role);
    let Some(captain) = tx.captain_of(&round.project)? else {
        return Ok(());
    };
    let existing = tx
        .thread_messages(&round.project, &thread)?
        .into_iter()
        .find(|m| {
            m.recipient == captain.id
                && m.kind == MsgKind::System
                && m.attempt == Some(round.attempt)
                && m.sender == round.seat
        });
    if let Some(existing) = existing {
        tx.update_message_body(&round.project, &existing.id, body)?;
        tx.set_message_urgent(&round.project, &existing.id, urgent)?;
        if existing.state == MsgState::Resolved {
            tx.update_message_state(
                &round.project,
                &existing.id,
                MsgState::Open,
                None,
                None,
                None,
                now,
            )?;
        }
    } else {
        tx.insert_message(&NewMessage {
            project: round.project.clone(),
            thread,
            task: Some(round.task.clone()),
            attempt: Some(round.attempt),
            sender: round.seat.clone(),
            sender_gen: round.generation,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent,
            body: body.into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
            meta: None,
        })?;
    }
    Ok(())
}

/// Account one round at `now` (T-406): its clock, its pause, and the
/// overrun question. Also called when a round is closed (T-458, `rounds`),
/// so a round's clock reaches its end and not only the last tick.
pub(crate) fn process(tx: &Tx<'_>, round: &RoundRow, now: Ts) -> GearResult<bool> {
    // How many rounds a tick looked at (T-406's tests).
    #[cfg(test)]
    PROCESSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some(attempt) = tx.get_attempt(&round.project, &round.task, round.attempt)? else {
        return Ok(false);
    };
    let Some(task) = tx.get_task(&round.project, &round.task)? else {
        return Ok(false);
    };
    let times = tx.attempt_times(&round.project, &round.task, round.attempt)?;
    let review = round.role == "review";
    // T-591: a direction round of a feature member is a stage check. It is a
    // light look under the feature's reviewer and not the task's work, so it
    // is measured in its own five minutes, from the check's own start: the
    // attempt's start (and its hand-in) belong to the work, not to the check
    // that reads its candidate.
    let stage_check = stage_check(tx, round)?;
    // T-458: a round reopened for a parked attempt that goes on counts from
    // the window's own start, never from the attempt's first start (the time
    // the lane worked on another task is not this round's).
    let start = if review || stage_check.is_some() {
        Some(round.start_at)
    } else {
        times.started.map(|started| started.max(round.start_at))
    };
    let Some(start) = start else { return Ok(false) };
    let verdict = if review {
        tx.get_review_verdict(&round.project, &round.task, round.attempt)?
            .map(|v| v.created)
    } else {
        None
    };
    // A review round ends when its attempt is settled by anyone (T-381): a
    // captain's verdict writes neither a review verdict nor an `end_at`, so the
    // round would stay live for ever, its active time would grow and its
    // "no answer" item would come back every 15 minutes. A review round is thus
    // live only while its attempt is still handed in and its task is still with
    // the reviewer; once it is not, the round stops when it was settled.
    let settled =
        review && (attempt.state != AttemptState::Submitted || task.state != TaskState::Verifying);
    let stop = if stage_check.is_some() {
        // A stage check reads a candidate that is already handed in: the
        // hand-in is not its end. Its round ends when the check's job ends
        // (`rounds`), never here.
        None
    } else if review {
        match verdict {
            Some(at) => Some(at),
            None if settled => Some(times.settled.unwrap_or(now)),
            None => None,
        }
    } else {
        times.handed_in
    };
    let end = stop
        .unwrap_or(now)
        .min(round.end_at.unwrap_or(now))
        .min(now)
        .max(start);
    let pause = paused(tx, round, start, end)?;
    let total_pause = pause.max(round.paused_s);
    if total_pause > round.paused_s {
        tx.add_round_pause(&round_key(round), total_pause - round.paused_s)?;
    }
    let active = (end - start - total_pause).max(0);
    tx.set_round_active(&round_key(round), active)?;
    let live = if let Some(_feature) = &stage_check {
        // T-591: a stage check is never ended by this step, and this step
        // never sets a result for it or judges it on course. Its round is open
        // until its check's job ends (`rounds`), and it is reminded once.
        round.end_at.is_none()
    } else if review {
        verdict.is_none() && round.end_at.is_none() && !settled
    } else {
        matches!(attempt.state, AttemptState::Started | AttemptState::Stopped) && stop.is_none()
    };
    let Some(seat) = tx.get_seat(&round.seat)? else {
        return Ok(false);
    };
    let budget = match &stage_check {
        Some(_) => STAGE_CHECK_BUDGET_MIN,
        None => tx.round_budget_min(
            &round.project,
            &round.task,
            round.attempt,
            &round.role,
            round.model.as_deref(),
        )?,
    };
    let state_key = key(round);
    let mut state = tx.get_meta(&state_key)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or_else(|| json!({"next_active":budget*60,"asked_at":null,"asked_active":0,"answer_id":null,"waiting":false,"no_answer":false}));
    let previous_budget = state
        .get("budget")
        .and_then(Value::as_i64)
        .unwrap_or(budget);
    if budget > previous_budget {
        let next = state["next_active"].as_i64().unwrap_or(0).max(budget * 60);
        state["next_active"] = json!(next);
    }
    state["budget"] = json!(budget);
    let asked_at = state
        .get("asked_at")
        .and_then(Value::as_i64)
        .filter(|_| !settled);
    let mut changed = false;
    // T-591: a stage check's reply is not an answer. The block below writes
    // the captain an overrun item and moves the clock; a stage check does
    // neither — its reply only silences its reminder, below.
    if let (None, Some(asked)) = (&stage_check, asked_at) {
        let last_id = state["answer_id"]
            .as_str()
            .and_then(|id| id.trim_start_matches("M-").parse::<u64>().ok())
            .unwrap_or(0);
        if let Some(answer) = answers(tx, round)?.into_iter().rev().find(|m| {
            m.created >= asked
                && m.created >= start
                && m.created <= end
                && m.id.trim_start_matches("M-").parse::<u64>().unwrap_or(0) > last_id
        }) {
            let meta: Value =
                serde_json::from_str(answer.meta.as_deref().unwrap_or("{}")).unwrap_or_default();
            let left = meta.get("left").and_then(Value::as_i64).unwrap_or(0).max(0);
            let blocker = meta
                .get("blocker")
                .and_then(Value::as_str)
                .unwrap_or("none");
            let answered_active = active_at(tx, round, start, answer.created.min(now).max(start))?;
            let body = format!(
                "{}: budget {} min; active {} min; left {} min; blocker: {}\n{}",
                round.task,
                budget,
                active / 60,
                left,
                blocker,
                answer.body
            );
            item(
                tx,
                round,
                &body,
                !blocker.trim().eq_ignore_ascii_case("none"),
                now,
            )?;
            state["answer_id"] = json!(answer.id);
            state["next_active"] = json!((answered_active + left.max(10) * 60).max(budget * 60));
            state["waiting"] = json!(false);
            state["no_answer"] = json!(false);
            for question in tx.thread_messages(
                &round.project,
                &format!(
                    "overrun-question:{}:{}:{}",
                    round.task, round.attempt, round.role
                ),
            )? {
                if matches!(question.state, MsgState::Open | MsgState::Waiting) {
                    tx.update_message_state(
                        &round.project,
                        &question.id,
                        MsgState::Resolved,
                        Some("progress received"),
                        None,
                        None,
                        now,
                    )?;
                }
            }
            crate::stall::resolve(tx, &round.project, &round.seat, now, None)?;
            changed = true;
        }
    }
    if !live {
        for question in tx.thread_messages(
            &round.project,
            &format!(
                "overrun-question:{}:{}:{}",
                round.task, round.attempt, round.role
            ),
        )? {
            if matches!(question.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(
                    &round.project,
                    &question.id,
                    MsgState::Resolved,
                    Some("round finished"),
                    None,
                    None,
                    now,
                )?;
            }
        }
        // The round is over because its attempt was settled: its open overrun
        // item is answered by that settlement and is not asked again (T-381).
        if settled {
            for old in tx.thread_messages(
                &round.project,
                &format!("overrun:{}:{}", round.task, round.role),
            )? {
                if old.attempt == Some(round.attempt)
                    && matches!(old.state, MsgState::Open | MsgState::Waiting)
                {
                    tx.update_message_state(
                        &round.project,
                        &old.id,
                        MsgState::Resolved,
                        Some("the attempt was settled"),
                        None,
                        None,
                        now,
                    )?;
                    changed = true;
                }
            }
        }
        tx.set_meta(&state_key, &state.to_string())?;
        return Ok(changed);
    }
    let waiting = state["waiting"].as_bool().unwrap_or(false);
    let asked_active = state["asked_active"].as_i64().unwrap_or(0);
    let paused_now = tx
        .pauses_of_project(&round.project)?
        .iter()
        .any(|p| p.seat == round.seat && p.start_at <= now && p.end_at.is_none());
    let askable = !paused_now
        && task.wait_on.is_none()
        && !matches!(seat.state, SeatState::Parked | SeatState::Held);
    // T-591: a stage check gets one interrupting prompt when its five minutes
    // pass, and nothing else from this step: the round is never ended here, no
    // result is set for it, it is never judged on course, and no answer is
    // asked for. A progress reply with a reason — "why and keep going" —
    // silences the reminder for this round, and there is no second reminder.
    if let Some(feature) = &stage_check {
        let next = state["next_active"].as_i64().unwrap_or(budget * 60);
        let reminded = state["stage_reminded"].as_bool().unwrap_or(false);
        if reminded {
            let asked = state["asked_at"].as_i64().unwrap_or(0);
            if stage_check_reply(tx, round, asked, end)?.is_some() {
                for prompt in tx.thread_messages(
                    &round.project,
                    &format!(
                        "overrun-question:{}:{}:{}",
                        round.task, round.attempt, round.role
                    ),
                )? {
                    if matches!(prompt.state, MsgState::Open | MsgState::Waiting) {
                        tx.update_message_state(
                            &round.project,
                            &prompt.id,
                            MsgState::Resolved,
                            Some("progress received"),
                            None,
                            None,
                            now,
                        )?;
                        changed = true;
                    }
                }
            }
        } else if active >= next && askable {
            tx.insert_message(&NewMessage {
                project: round.project.clone(),
                thread: format!(
                    "overrun-question:{}:{}:{}",
                    round.task, round.attempt, round.role
                ),
                task: Some(round.task.clone()),
                attempt: Some(round.attempt),
                sender: "engine".into(),
                sender_gen: 0,
                recipient: round.seat.clone(),
                kind: MsgKind::Instruction,
                urgent: true,
                body: stage_check_text(&round.task, feature),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
                meta: None,
            })?;
            state["stage_reminded"] = json!(true);
            state["asked_at"] = json!(now);
            state["waiting"] = json!(false);
            changed = true;
        }
        tx.set_meta(&state_key, &state.to_string())?;
        return Ok(changed);
    }
    if waiting
        && task.wait_on.is_none()
        && !state["no_answer"].as_bool().unwrap_or(false)
        && active >= asked_active + 600
    {
        item(
            tx,
            round,
            &format!(
                "{}: budget {} min; active {} min; no answer",
                round.task,
                budget,
                active / 60
            ),
            true,
            now,
        )?;
        state["no_answer"] = json!(true);
        state["waiting"] = json!(false);
        state["next_active"] = json!(active + 900);
        changed = true;
    }
    // Read after the no answer item moved it: the next question waits for the
    // new time, never for the one this tick found.
    let next = state["next_active"].as_i64().unwrap_or(budget * 60);
    if !state["waiting"].as_bool().unwrap_or(false) && active >= next && askable {
        let question = text(
            &round.task,
            round.attempt,
            attempt.adopted_version,
            budget,
            active,
        );
        for old in tx.thread_messages(
            &round.project,
            &format!(
                "overrun-question:{}:{}:{}",
                round.task, round.attempt, round.role
            ),
        )? {
            if matches!(old.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(
                    &round.project,
                    &old.id,
                    MsgState::Resolved,
                    Some("a new question replaces it"),
                    None,
                    None,
                    now,
                )?;
            }
        }
        tx.insert_message(&NewMessage {
            project: round.project.clone(),
            thread: format!(
                "overrun-question:{}:{}:{}",
                round.task, round.attempt, round.role
            ),
            task: Some(round.task.clone()),
            attempt: Some(round.attempt),
            sender: "engine".into(),
            sender_gen: 0,
            recipient: round.seat.clone(),
            kind: MsgKind::Instruction,
            urgent: true,
            body: question,
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
            meta: None,
        })?;
        state["asked_at"] = json!(now);
        state["asked_active"] = json!(active);
        state["waiting"] = json!(true);
        state["no_answer"] = json!(false);
        changed = true;
    }
    tx.set_meta(&state_key, &state.to_string())?;
    Ok(changed)
}

/// Mirror quota parking into pause rows before calculating any round clock.
fn reconcile_quota_pauses(tx: &Tx<'_>, project: &str) -> GearResult<()> {
    let pauses = tx.pauses_of_project(project)?;
    for seat in tx.seats_of_project(project)? {
        let open = pauses
            .iter()
            .any(|p| p.seat == seat.id && p.kind == "quota" && p.end_at.is_none());
        if seat.state == SeatState::Parked {
            if !open {
                tx.open_pause(project, &seat.id, "quota", "quota parked", seat.state_since)?;
            }
        } else if open {
            tx.close_pause(project, &seat.id, "quota", seat.state_since)?;
        }
    }
    Ok(())
}

/// The rounds a tick looked at: the counter behind T-406's tests.
#[cfg(test)]
static PROCESSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn tick(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    _cfg: &EngineCfg,
    now: Ts,
) -> GearResult<()> {
    let projects = store.read(|tx| tx.active_project_ids())?;
    for project in projects {
        store.write(|tx| {
            reconcile_quota_pauses(tx, &project)?;
            let mut changed = false;
            // Only the rounds that can still change (T-406): the open ones and
            // the ended ones whose overrun item is still owed an answer.
            for round in tx.rounds_to_process(&project)? {
                changed |= process(tx, &round, now)?;
            }
            Ok(changed)
        })?;
        let _ = herdr;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_model::{SeatRole, TaskState};
    use gear_store::{testing, AttemptStep, ContractRow, NewRound, TaskRow};
    const T: i64 = 1_800_000_000;
    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "codex", T)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(10),
                issue: None,
                updated: T,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-1".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T,
                mechanical: None,
            })?;
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                },
                seat: "p:heat".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })?;
            Ok(())
        })
        .unwrap();
        s
    }
    fn tick_at(s: &mut Store, at: i64) {
        tick(s, &mut FakeHerdr::new(), &EngineCfg::default(), T + at).unwrap();
    }

    /// The world with attempt 1 handed in (Submitted) and its task Verifying:
    /// what a review round runs on.
    fn submitted() -> Store {
        let mut s = world();
        s.write(|tx| {
            gear_app::tasks::submit_result(
                tx,
                "p",
                "T-1",
                1,
                1,
                "abc1234",
                "Report: ops/reports/T-1.md",
                T + 1,
            )
        })
        .unwrap();
        s
    }
    fn messages(s: &mut Store, thread: &str) -> Vec<gear_store::MessageRow> {
        s.read(|tx| tx.thread_messages("p", thread)).unwrap()
    }
    fn processed() -> usize {
        PROCESSED.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn round_of(s: &mut Store, role: &str) -> gear_store::RoundRow {
        s.read(|tx| tx.rounds_of_task("p", "T-1"))
            .unwrap()
            .into_iter()
            .find(|round| round.attempt == 1 && round.role == role)
            .unwrap_or_else(|| panic!("no {role} round"))
    }

    /// A feature member under its stage check (T-591): T-1 is marked
    /// `feature: F-1` and a direction round reads its candidate from a temp
    /// review seat. The dev round of `world` is still there and is not a
    /// stage check.
    fn stage_check_world() -> Store {
        let mut s = world();
        s.write(|tx| {
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T)?;
            tx.set_task_feature("p", "T-1", "F-1")?;
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "direction".into(),
                },
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })?;
            Ok(())
        })
        .unwrap();
        s
    }

    /// The one line a stage check's reviewer sends instead of a result: why it
    /// needs more time, with no `--left` fields (`meta` None).
    fn reviewer_line(s: &mut Store, at: i64, body: &str) {
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-1:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:review-1".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Progress,
                urgent: false,
                body: body.into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: T + at,
                meta: None,
            })
            .map(|_| ())
        })
        .unwrap();
    }

    /// T-591: a member's direction round is a stage check, and five minutes of
    /// it earns one interrupting prompt — the same message the overrun question
    /// uses today — and never a second one. The engine ends nothing, sets no
    /// result and judges nothing.
    #[test]
    fn a_stage_check_is_reminded_once_at_five_minutes() {
        let mut s = stage_check_world();
        tick_at(&mut s, 299);
        assert!(messages(&mut s, "overrun-question:T-1:1:direction").is_empty());
        tick_at(&mut s, 300);
        let prompts = messages(&mut s, "overrun-question:T-1:1:direction");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].urgent);
        assert_eq!(prompts[0].kind, MsgKind::Instruction);
        assert_eq!(prompts[0].recipient, "p:review-1");
        assert_eq!(
            prompts[0].body,
            "GEAR: T-1 is a stage check of feature F-1, and 5 minutes have passed. \
             Report what you found now. If you have a lead that needs more time, reply \
             with one line saying why and keep going; the full review comes on the \
             feature's integration task."
        );
        // No second reminder, however long the check goes on, and no captain
        // item: the check blocks nothing.
        for at in [301, 600, 1200, 3600] {
            tick_at(&mut s, at);
        }
        let prompts = messages(&mut s, "overrun-question:T-1:1:direction");
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].state, MsgState::Open);
        assert!(messages(&mut s, "overrun:T-1:direction").is_empty());
        // The round stays open, with no result and no verdict written for it.
        let round = round_of(&mut s, "direction");
        assert_eq!(round.end_at, None);
        assert_eq!(round.active_s, 3600);
        assert!(s
            .read(|tx| tx.direction_results_of_attempt("p", "T-1", 1))
            .unwrap()
            .is_empty());
    }

    /// T-591: the reviewer's one line, sent after the reminder, silences it for
    /// that round — no second reminder and no captain item for it.
    #[test]
    fn a_stage_check_reply_silences_the_reminder() {
        let mut s = stage_check_world();
        tick_at(&mut s, 300);
        assert_eq!(
            messages(&mut s, "overrun-question:T-1:1:direction").len(),
            1
        );
        reviewer_line(
            &mut s,
            320,
            "the goal names two behaviours the contract does not; reading the plan",
        );
        tick_at(&mut s, 321);
        let prompts = messages(&mut s, "overrun-question:T-1:1:direction");
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].state, MsgState::Resolved);
        assert_eq!(prompts[0].result.as_deref(), Some("progress received"));
        // Silenced: no second reminder at 10, 30 or 90 minutes, and the reply
        // is no answer of the overrun question's kind (no captain item).
        for at in [600, 660, 1800, 5400, 7200] {
            tick_at(&mut s, at);
        }
        assert_eq!(
            messages(&mut s, "overrun-question:T-1:1:direction").len(),
            1
        );
        assert!(messages(&mut s, "overrun:T-1:direction").is_empty());
        assert_eq!(round_of(&mut s, "direction").end_at, None);
    }

    /// T-591: the five minutes are the stage check's own. A normal round of the
    /// same feature member keeps its budget: the dev round its estimate, the
    /// review round the measured review budget.
    #[test]
    fn a_normal_round_keeps_its_estimate_budget() {
        let mut s = stage_check_world();
        tick_at(&mut s, 599);
        assert!(messages(&mut s, "overrun-question:T-1:1:dev").is_empty());
        tick_at(&mut s, 600);
        let dev = messages(&mut s, "overrun-question:T-1:1:dev");
        assert_eq!(dev.len(), 1);
        assert_eq!(dev[0].body, text("T-1", 1, 1, 10, 600));

        let mut s = submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T)?;
            tx.set_task_feature("p", "T-1", "F-1")?;
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "review".into(),
                },
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })
        })
        .unwrap();
        tick_at(&mut s, 1799);
        assert!(messages(&mut s, "overrun-question:T-1:1:review").is_empty());
        tick_at(&mut s, 1800);
        let review = messages(&mut s, "overrun-question:T-1:1:review");
        assert_eq!(review.len(), 1);
        assert_eq!(review[0].body, text("T-1", 1, 1, 30, 1800));
    }

    fn answer(s: &mut Store, at: i64, left: i64, blocker: &str) {
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-1:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Progress,
                urgent: false,
                body: "work update".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: T + at,
                meta: Some(json!({"left":left,"blocker":blocker}).to_string()),
            })
            .map(|_| ())
        })
        .unwrap();
    }
    #[test]
    fn question_answer_and_urgency_are_scheduled_from_answer() {
        let mut s = world();
        tick_at(&mut s, 599);
        assert!(messages(&mut s, "overrun-question:T-1:1:dev").is_empty());
        tick_at(&mut s, 600);
        tick_at(&mut s, 600);
        let q = messages(&mut s, "overrun-question:T-1:1:dev");
        assert_eq!(q.len(), 1);
        assert!(q[0].urgent);
        assert_eq!(q[0].kind, MsgKind::Instruction);
        assert_eq!(q[0].body, text("T-1", 1, 1, 10, 600));
        answer(&mut s, 660, 15, "waiting for API");
        tick_at(&mut s, 660);
        let items = messages(&mut s, "overrun:T-1:dev");
        assert_eq!(items.len(), 1);
        assert!(items[0].urgent);
        answer(&mut s, 700, 15, "none");
        tick_at(&mut s, 700);
        let items = messages(&mut s, "overrun:T-1:dev");
        assert_eq!(items.len(), 1);
        assert!(!items[0].urgent);
        tick_at(&mut s, 1599);
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:dev").len(), 1);
        tick_at(&mut s, 1600);
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:dev").len(), 2);
    }
    #[test]
    fn missing_answer_escalates_after_active_ten_and_repeats_after_fifteen() {
        let mut s = world();
        tick_at(&mut s, 600);
        s.write(|tx| tx.open_pause("p", "p:heat", "wait", "blocked", T + 700))
            .unwrap();
        tick_at(&mut s, 2000);
        assert!(messages(&mut s, "overrun:T-1:dev").is_empty());
        s.write(|tx| tx.close_pause("p", "p:heat", "wait", T + 2000))
            .unwrap();
        tick_at(&mut s, 2499);
        assert!(messages(&mut s, "overrun:T-1:dev").is_empty());
        tick_at(&mut s, 2500);
        assert!(messages(&mut s, "overrun:T-1:dev")[0].urgent);
        tick_at(&mut s, 3399);
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:dev").len(), 1);
        tick_at(&mut s, 3400);
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:dev").len(), 2);
    }
    #[test]
    fn pauses_are_unioned_and_clock_stops_at_handin() {
        let mut s = world();
        s.write(|tx| {
            tx.open_pause("p", "p:heat", "wait", "wait", T + 100)?;
            tx.open_pause("p", "p:heat", "quota", "quota", T + 200)?;
            tx.close_pause("p", "p:heat", "wait", T + 300)?;
            tx.close_pause("p", "p:heat", "quota", T + 400)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, T + 500)?;
            Ok(())
        })
        .unwrap();
        tick_at(&mut s, 2000);
        let round = s
            .read(|tx| tx.rounds_of_task("p", "T-1"))
            .unwrap()
            .remove(0);
        assert_eq!(round.paused_s, 300);
        assert_eq!(round.active_s, 200);
        assert!(messages(&mut s, "overrun-question:T-1:1:dev").is_empty());
    }
    #[test]
    fn review_budget_uses_model_history_and_stops_at_verdict() {
        let mut s = submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T)?;
            for (n, seconds) in [(2, 1200), (3, 1800), (4, 2400)] {
                let key = RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: n,
                    role: "review".into(),
                };
                tx.insert_round(&NewRound {
                    key: key.clone(),
                    seat: "p:review-1".into(),
                    generation: 1,
                    cli: "codex".into(),
                    model: Some("model-a".into()),
                    start_at: T - 5000,
                })?;
                tx.set_round_active(&key, seconds)?;
                tx.close_round(&key, T - 1000 + n)?;
            }
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "review".into(),
                },
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: Some("model-a".into()),
                start_at: T + 100,
            })?;
            Ok(())
        })
        .unwrap();
        tick_at(&mut s, 2799);
        assert!(messages(&mut s, "overrun-question:T-1:1:review").is_empty());
        tick_at(&mut s, 2800);
        assert_eq!(
            messages(&mut s, "overrun-question:T-1:1:review")[0].body,
            text("T-1", 1, 1, 45, 2700)
        );
        s.write(|tx| {
            tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed: true,
                findings: String::new(),
                by_seat: "p:review-1".into(),
                created: T + 3000,
            })
        })
        .unwrap();
        tick_at(&mut s, 4000);
        let review = s
            .read(|tx| tx.rounds_of_task("p", "T-1"))
            .unwrap()
            .into_iter()
            .find(|r| r.attempt == 1 && r.role == "review")
            .unwrap();
        assert_eq!(review.active_s, 2900);
    }

    #[test]
    fn actual_quota_park_pauses_the_clock_and_no_answer_timer() {
        let mut s = world();
        tick_at(&mut s, 600);
        s.write(|tx| tx.set_seat_state("p:heat", SeatState::Parked, Some("quota"), T + 700))
            .unwrap();
        tick_at(&mut s, 2000);
        let round = s
            .read(|tx| tx.rounds_of_task("p", "T-1"))
            .unwrap()
            .remove(0);
        assert_eq!((round.active_s, round.paused_s), (700, 1300));
        assert!(messages(&mut s, "overrun:T-1:dev").is_empty());
        s.write(|tx| tx.set_seat_state("p:heat", SeatState::Active, Some("quota reset"), T + 2100))
            .unwrap();
        tick_at(&mut s, 2200);
        let round = s
            .read(|tx| tx.rounds_of_task("p", "T-1"))
            .unwrap()
            .remove(0);
        assert_eq!((round.active_s, round.paused_s), (800, 1400));
        assert!(messages(&mut s, "overrun:T-1:dev").is_empty());
        tick_at(&mut s, 2600);
        assert!(messages(&mut s, "overrun:T-1:dev")[0].urgent);
    }

    #[test]
    fn progress_before_handin_is_processed_on_the_finished_round() {
        let mut s = world();
        tick_at(&mut s, 600);
        answer(&mut s, 601, 0, "none");
        s.write(|tx| tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, T + 602))
            .unwrap();
        tick_at(&mut s, 603);
        tick_at(&mut s, 604);
        let items = messages(&mut s, "overrun:T-1:dev");
        assert_eq!(items.len(), 1);
        assert!(!items[0].urgent);
        assert!(items[0].body.contains("work update"));
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:dev").len(), 1);
    }
    #[test]
    fn progress_after_handin_is_not_an_answer_for_the_closed_round() {
        let mut s = world();
        tick_at(&mut s, 600);
        s.write(|tx| tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, T + 601))
            .unwrap();
        answer(&mut s, 602, 0, "none");
        tick_at(&mut s, 603);
        assert!(messages(&mut s, "overrun:T-1:dev").is_empty());
    }
    #[test]
    fn progress_before_review_verdict_is_processed_on_the_finished_round() {
        let mut s = submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T)?;
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "review".into(),
                },
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })
        })
        .unwrap();
        tick_at(&mut s, 1800);
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-1:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:review-1".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Progress,
                urgent: false,
                body: "review update".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: T + 1801,
                meta: Some(json!({"left":0,"blocker":"none"}).to_string()),
            })?;
            tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed: true,
                findings: String::new(),
                by_seat: "p:review-1".into(),
                created: T + 1802,
            })
        })
        .unwrap();
        tick_at(&mut s, 1803);
        tick_at(&mut s, 1804);
        let items = messages(&mut s, "overrun:T-1:review");
        assert_eq!(items.len(), 1);
        assert!(!items[0].urgent);
        assert!(items[0].body.contains("review update"));
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:review").len(), 1);
    }

    #[test]
    fn a_settled_attempt_ends_the_review_round_and_resolves_its_item_once() {
        let mut s = submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T)?;
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "review".into(),
                },
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })
        })
        .unwrap();
        // The round asks at its budget (30 min) and, unanswered, writes its item.
        tick_at(&mut s, 1800);
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:review").len(), 1);
        tick_at(&mut s, 2400);
        let items = messages(&mut s, "overrun:T-1:review");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, MsgState::Open);
        assert!(items[0].urgent);
        // The captain's fail verdict settles the attempt: no review verdict, no end_at.
        s.write(|tx| {
            gear_app::tasks::settle(
                tx,
                &gear_app::tasks::Verdict {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: "abc1234".into(),
                    pass: false,
                    evidence: None,
                    rework: Some("the rows drift right".into()),
                    integrated_by: None,
                    by_seat: "p:captain".into(),
                },
                T + 2401,
            )
        })
        .unwrap();
        tick_at(&mut s, 2500);
        let items = messages(&mut s, "overrun:T-1:review");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, MsgState::Resolved);
        assert_eq!(items[0].result.as_deref(), Some("the attempt was settled"));
        assert_eq!(items[0].resolved, Some(T + 2500));
        let round = s
            .read(|tx| tx.rounds_of_task("p", "T-1"))
            .unwrap()
            .into_iter()
            .find(|r| r.attempt == 1 && r.role == "review")
            .unwrap();
        assert_eq!(round.active_s, 2401);
        // Later ticks ask nothing and write nothing: the item stays resolved once.
        tick_at(&mut s, 6000);
        tick_at(&mut s, 9000);
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:review").len(), 1);
        let items = messages(&mut s, "overrun:T-1:review");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, MsgState::Resolved);
        assert_eq!(items[0].resolved, Some(T + 2500));
    }

    #[test]
    fn a_queued_answer_after_settlement_writes_no_overrun_item() {
        let mut s = submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T)?;
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "review".into(),
                },
                seat: "p:review-1".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })
        })
        .unwrap();
        // The round asks at its budget (30 min).
        tick_at(&mut s, 1800);
        assert_eq!(messages(&mut s, "overrun-question:T-1:1:review").len(), 1);
        // The reviewer's answer is queued at T+1801 and the captain settles the
        // attempt at T+1802, before the next tick can process the answer.
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-1:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:review-1".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Progress,
                urgent: false,
                body: "review update".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: T + 1801,
                meta: Some(json!({"left":0,"blocker":"none"}).to_string()),
            })?;
            gear_app::tasks::settle(
                tx,
                &gear_app::tasks::Verdict {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    contract_version: 1,
                    candidate: "abc1234".into(),
                    pass: false,
                    evidence: None,
                    rework: Some("the rows drift right".into()),
                    integrated_by: None,
                    by_seat: "p:captain".into(),
                },
                T + 1802,
            )
        })
        .unwrap();
        // The settlement already stopped the round: the queued answer writes no
        // item on this tick or any later one (F1 of the T-381 review).
        for at in [1803, 1804, 6000, 9000] {
            tick_at(&mut s, at);
            assert!(
                messages(&mut s, "overrun:T-1:review").is_empty(),
                "no item after the settlement, tick {at}"
            );
        }
        // The round's question is closed by the settlement, as before.
        let questions = messages(&mut s, "overrun-question:T-1:1:review");
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].state, MsgState::Resolved);
    }

    #[test]
    fn a_tick_looks_only_at_the_rounds_that_can_still_change() {
        let mut s = world();
        s.write(|tx| {
            // 300 rounds that ended long ago are never looked at again.
            for n in 0..300 {
                let key = RoundKey {
                    project: "p".into(),
                    task: format!("T-old-{n}"),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "codex".into(),
                    model: None,
                    start_at: T - 9000,
                })?;
                tx.close_round(&key, T - 8000)?;
            }
            // A second open round beside the dev round the world starts with.
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 2,
                    role: "dev".into(),
                },
                seat: "p:heat".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })?;
            Ok(())
        })
        .unwrap();
        PROCESSED.store(0, std::sync::atomic::Ordering::Relaxed);
        tick_at(&mut s, 600);
        assert_eq!(processed(), 2, "only the two open rounds");
    }

    #[test]
    fn an_ended_round_is_looked_at_while_its_overrun_item_is_open() {
        let mut s = world();
        let ended = RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "review".into(),
        };
        s.write(|tx| {
            tx.insert_round(&NewRound {
                key: ended.clone(),
                seat: "p:heat".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T - 900,
            })?;
            tx.close_round(&ended, T - 800)?;
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "overrun:T-1:review".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::System,
                urgent: true,
                body: "T-1: budget 10 min; active 10 min; no answer".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T - 700,
                meta: None,
            })
            .map(|_| ())
        })
        .unwrap();
        PROCESSED.store(0, std::sync::atomic::Ordering::Relaxed);
        tick_at(&mut s, 600);
        assert_eq!(
            processed(),
            2,
            "the dev round and the ended review round that still owes an answer"
        );
        // Its item is answered by that look (T-381): once resolved, the ended
        // round is no longer looked at.
        let items = messages(&mut s, "overrun:T-1:review");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, MsgState::Resolved);
        PROCESSED.store(0, std::sync::atomic::Ordering::Relaxed);
        tick_at(&mut s, 700);
        assert_eq!(processed(), 1, "only the dev round is left");
    }

    /// F1 of the T-406 review: the selection must read the project's messages
    /// once, not once per ended round. Thousands of messages and hundreds of
    /// ended rounds are a normal project's history, so the selection is read by
    /// index — `messages_thread` for the answered threads, the rounds key for
    /// the rounds.
    ///
    /// T-520: the check is the query plan, not the clock. A timing assertion
    /// here failed under the gate's parallel load (F-293); the plan is the fact
    /// that does not move with the machine's load.
    #[test]
    fn a_long_history_is_selected_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gear.db");
        let mut s = Store::open(&path).unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "codex", T)?;
            // The world of the other tests: a lane working T-1 in an open round.
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(10),
                issue: None,
                updated: T,
            })?;
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
            tx.insert_round(&NewRound {
                key: RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                },
                seat: "p:heat".into(),
                generation: 1,
                cli: "codex".into(),
                model: None,
                start_at: T,
            })?;
            for n in 0..300 {
                let key = RoundKey {
                    project: "p".into(),
                    task: format!("T-old-{n}"),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "codex".into(),
                    model: None,
                    start_at: T - 9000,
                })?;
                tx.close_round(&key, T - 8000)?;
            }
            for n in 0..10_000 {
                let task = format!("T-old-{}", n % 300);
                tx.insert_message(&NewMessage {
                    project: "p".into(),
                    thread: format!("{task}:1"),
                    task: Some(task),
                    attempt: Some(1),
                    sender: "p:heat".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Progress,
                    urgent: false,
                    body: "work update".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Info,
                    created: T - 9000 + n,
                    meta: None,
                })?;
            }
            Ok(())
        })
        .unwrap();

        // The plan of the selection, as the store asks it: the rounds by their
        // key, the answered threads by `messages_thread` — no full scan of
        // either table.
        let conn = rusqlite::Connection::open(&path).unwrap();
        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT project FROM rounds
                 WHERE project=?1
                   AND (end_at IS NULL OR ('overrun:'||task||':'||role, attempt) IN (
                     SELECT m.thread, m.attempt FROM messages m
                      WHERE m.project=?1 AND m.kind='system'
                        AND m.state IN ('open', 'waiting')))
                 ORDER BY start_at, task, role",
            )
            .unwrap()
            .query_map(["p"], |row| row.get::<_, String>(3))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        let text = plan.join(" | ");
        assert!(
            text.contains("SEARCH rounds USING"),
            "the rounds are not read by an index: {text}"
        );
        assert!(
            text.contains("SEARCH m USING"),
            "the answered threads are not read by an index: {text}"
        );
        assert!(
            !text.contains("SCAN rounds")
                && !text.contains("SCAN m")
                && !text.contains("SCAN messages"),
            "the selection scans a table: {text}"
        );

        // And it still selects the one round that is open.
        let rounds = s.read(|tx| tx.rounds_to_process("p")).unwrap();
        assert_eq!(rounds.len(), 1, "only the open round is selected");
    }
}
