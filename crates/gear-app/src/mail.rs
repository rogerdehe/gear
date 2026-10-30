//! Messages: send, reply, inbox batch selection (design/07).

use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, SeatRole, SeatState, TaskState, Ts};
use gear_store::{MessageRow, NewMessage, RequestOutcome, Store, Tx};
use std::collections::BTreeMap;

/// Upper bound for the thread sequence search, so a broken database cannot
/// spin forever.
const MAX_THREAD_SEQ: u32 = 10_000;

/// How long a wait on something outside GEAR lasts before the lane is asked
/// to push again (T-440): the engine ends the wait at this time.
pub const EXTERNAL_WAIT_MIN: i64 = 120;

/// The time an external wait ends (T-440): the engine reads it, the lane's
/// wait is written with it.
pub fn external_wait_key(project: &str, task: &str) -> String {
    format!("engine.wait_until.{project}.{task}")
}

/// Unhandled messages of one thread that are shown in the batch.
const SHOWN_PER_THREAD: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendArgs {
    pub kind: MsgKind,
    pub body: String,
    /// Required for Result/Blocked (design/03): exact task, attempt, contract.
    pub task: Option<String>,
    pub attempt: Option<i64>,
    pub contract: Option<i64>,
    /// Result only: the commit (or other checkable ref) handed in.
    pub candidate: Option<String>,
    /// Review only (T-2): the verdict a temp reviewer hands in.
    pub verdict: Option<crate::review::Outcome>,
    /// Review only (T-23): the ids of the findings that block.
    pub blocking: Option<String>,
    /// Result only (T-19): the hand-in gate — the candidate must contain the
    /// project's current integration branch. `None` skips the check (tests and
    /// callers that are not a lane hand-in).
    pub base: Option<crate::base::BaseGate>,
    /// Upstream task to ask (must be a registered dependency) — slice 2.
    pub about: Option<String>,
    /// Captain only: seat name to instruct.
    pub to: Option<String>,
    pub reply_to: Option<String>,
    pub urgent: bool,
    /// Progress only (design §5.2): the minutes the lane says are left.
    pub left: Option<i64>,
    /// Progress only (design §5.2): what stops the lane ("none" when nothing
    /// does).
    pub blocker: Option<String>,
    /// Blocked only (design §5.3): the task the lane waits for.
    pub on: Option<String>,
    /// Result/Review only (design §6.1): what cost time that the captain
    /// could fix.
    pub friction: Option<String>,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SendResult {
    /// The message's project (T-130: the caller types the doorbell itself).
    #[serde(default)]
    pub project: String,
    pub msg_id: String,
    pub thread: String,
    /// Seat id, e.g. `p:captain`.
    pub recipient: String,
    /// `true` when the stored result was replayed for a repeated request_id.
    pub replayed: bool,
}

/// `gear send`. One transaction via `run_request`.
///
/// Slice 1 scope:
/// - sender must be an Active/Standby/Retiring lane/temp, or the captain;
/// - default recipient = project captain; `--to` only for the captain and
///   only with kind `instruction` (design/07:21);
///   `--reply-to M-x` → the sender of M-x;
/// - Progress/Result/passing Review → Info; Question/Instruction/failing Review → Open;
///   Reply-kind is produced by `reply()`, not by `send`;
/// - thread: reply_to inherits; otherwise new thread `<task or seat>:<seq>` (seq = next unused number for that prefix, design/07);
/// - Result (T-45): `task`, `attempt`, `contract` and `candidate` required (else
///   BadFormat); in the same transaction `tasks::submit_result(…, body as report)`
///   — its errors abort the send; the captain gets a silent information record.
/// - Review (T-2): `task`, `attempt` and `verdict` required (else BadFormat);
///   only a temp seat bound to that task may send one (Forbidden/NoTarget);
///   in the same transaction `review::submit_review(…)` stores the verdict on
///   the attempt's review record — it settles nothing; the item goes to the
///   captain with the verdict in its first line (a fail is actionable, a pass is information).
/// - Ack side effects (tasks, handover) arrive later in slice 2 and must
///   return BadFormat until then rather than silently storing.
/// - A progress answer (design §5.2) carries the minutes the lane says are
///   left and what stops it, in the message's meta.
/// - A declared wait (design §5.3) — the `blocked` kind — works: the lane
///   names the task it waits for, the wait is written on its task and the
///   attempt goes on.
/// - Messages to a reviewer seat carry the task (design §3.8): a reviewer
///   seat is bound to a task for life, so the messages it writes carry that
///   task, and an instruction the captain addresses to it without `--task`
///   takes its `task_id`; an idle reviewer has no job, and the instruction
///   is refused ("<reviewer> has no review job; nothing would show").
/// - A hand-in is answered (design §10.7, row B03-04/10): the lane is told
///   that its work is in and that the next task comes by itself. The line is
///   printed by `gear send` — the CLI owns the reply a hand-in gets.
/// - An instruction the captain needs confirmed is sent with `--ack` (design
///   §10.7, row B7): it is stored with `needs_ack`, its body ends with the
///   `--kind ack` line the lane runs to confirm it, and the captain gets an
///   item that stays `waiting` for ten minutes, so an
///   unacknowledged instruction comes back into the captain's inbox.
pub fn send(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    args: &SendArgs,
    now: Ts,
) -> GearResult<SendResult> {
    send_inner(store, env, fp, args, now, false, false)
}

/// Send an instruction the captain needs confirmed (design §10.7, row B7):
/// the stored message asks for an ack, the lane is told how to give it, and
/// the captain gets an item that returns if the ack does not come.
pub fn send_with_ack(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    args: &SendArgs,
    now: Ts,
) -> GearResult<SendResult> {
    send_inner(store, env, fp, args, now, false, true)
}

/// Hand in a direction check without submitting the lane's attempt.
pub fn send_direction(store: &mut Store, env: &Env, fp: Option<&Fingerprint>, args: &SendArgs, now: Ts) -> GearResult<SendResult> {
    send_inner(store, env, fp, args, now, true, false)
}

fn send_inner(store: &mut Store, env: &Env, fp: Option<&Fingerprint>, args: &SendArgs, now: Ts, direction: bool, ack: bool) -> GearResult<SendResult> {
    // A request that already committed replays its receipt before the hand-in
    // gate: the hand-in happened, only the receipt was lost, and an outage
    // must not report it as failed (T-19 review F2). The caller is the live
    // seat — identity is checked in the same read transaction as the receipt
    // (review round 2, F6), so a stale generation or another occupant cannot
    // replay it.
    if let Some(receipt) = stored_receipt(store, env, fp, args, direction, ack)? {
        return Ok(receipt);
    }
    // The hand-in gate (T-19) runs before the transaction: a candidate that
    // does not contain the current integration branch is refused and nothing —
    // no message, no attempt change — is stored.
    if args.kind == MsgKind::Result && !direction {
        if let Some(gate) = &args.base {
            let candidate = args.candidate.as_deref().ok_or_else(|| {
                GearError::new(ErrorCode::BadFormat, "a result needs `--candidate`")
            })?;
            // The remote the hand-in is checked against is the project's
            // `[conventions].remote` (T-257): `origin` unless it names its own.
            let conventions = match env.ctx.as_ref() {
                Some(ctx) => crate::config::conventions_or_default(store, &ctx.project)?,
                None => crate::config::Conventions::default(),
            };
            crate::base::require_current_base(
                &gate.worktree,
                &conventions.remote,
                &gate.integration,
                candidate,
                gate.window_min,
            )?;
            // A rework keeps the work the reviewer read (design §10.7, row
            // B03-08): the new candidate carries the previous attempt's
            // candidate. The refusal names that commit. It comes before the
            // push check: a branch that lost the reviewed work is refused for
            // that, not for where it is pushed.
            if let (Some(ctx), Some(task), Some(attempt)) =
                (env.ctx.as_ref(), args.task.as_deref(), args.attempt)
            {
                let previous = store
                    .read(|tx| crate::tasks::rework_base_candidate(tx, &ctx.project, task, attempt))?;
                if let Some(previous) = previous {
                    crate::tasks::require_rework_contains(&gate.worktree, &previous, candidate)?;
                }
            }
            // And it must be on the task's branch on the remote (T-83): a
            // hand-in nobody can fetch is not a hand-in (M2.1: T-60). The
            // branch is the project's template (T-252), expanded like the
            // lane's, and the remote is the project's (T-257).
            if let Some(task) = args.task.as_deref() {
                if let Some(ctx) = env.ctx.as_ref() {
                    if let Some(lane) = store
                        .read(|tx| tx.get_task(&ctx.project, task))
                        .ok()
                        .flatten()
                        .and_then(|row| row.lane)
                    {
                        let branch =
                            crate::base::task_branch(&conventions, &ctx.project, &lane, task);
                        crate::base::require_pushed(
                            &gate.worktree,
                            &conventions.remote,
                            &branch,
                            candidate,
                        )?;
                    }
                }
            }
        }
    }
    store.write(|tx| {
        // The caller must be a seat that may write (design/03).
        let (ctx, seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Lane, SeatRole::Temp, SeatRole::Captain]),
            Some(&[SeatState::Active, SeatState::Standby, SeatState::Retiring]),
        )?;

        // Slice 1 kinds, plus `result` (T-45): a result names the exact
        // attempt/contract/candidate and submits it in this transaction.
        // An ack answers an instruction that asks for one (T-320); a reply is
        // written by `gear reply`.
        match args.kind {
            MsgKind::Progress | MsgKind::Question | MsgKind::Instruction => {}
            MsgKind::Ack => {
                // An ack answers an instruction that asks for one: without
                // `--reply-to` there is nothing to answer.
                if args.reply_to.is_none() {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        "an ack answers an instruction that asks for one: --reply-to M-<n>",
                    ));
                }
            }
            MsgKind::Result => {
                if args.task.is_none() || args.attempt.is_none() || (!direction && (
                    args.contract.is_none() || args.candidate.is_none()))
                {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        "a result needs `--task`, `--attempt`, `--contract` and `--candidate`",
                    ));
                }
            }
            MsgKind::Review => {
                // A review names the exact attempt it is about and carries a
                // verdict (T-2); the body holds the findings.
                if args.task.is_none() || args.attempt.is_none() || args.verdict.is_none() {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        "a review needs `--task`, `--attempt` and `--verdict pass|fail`",
                    ));
                }
            }
            MsgKind::Blocked => {
                // A declared wait (design §5.3): the lane names the task it
                // waits for — or `external` — and the attempt the wait
                // belongs to.
                if args.task.is_none()
                    || args.attempt.is_none()
                    || args.contract.is_none()
                    || args.on.is_none()
                {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        "a blocked message needs `--task`, `--attempt`, `--contract` and `--on T-<n>|external`",
                    ));
                }
            }
            other => {
                return Err(GearError::new(
                    ErrorCode::BadFormat,
                    format!("kind {} is not available yet", other.as_str()),
                ))
            }
        }
        // `--ack` is the captain's way of keeping an instruction open until
        // the lane confirms it (design §10.7, row B7): only an instruction it
        // addresses to a seat can ask for that.
        if ack && (args.kind != MsgKind::Instruction
            || args.to.is_none()
            || seat.role != SeatRole::Captain)
        {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                "`--ack` belongs to a captain's instruction with `--to`",
            ));
        }
        if direction && (args.kind != MsgKind::Result || args.body.trim().is_empty() || args.to.is_some() || args.reply_to.is_some() || args.candidate.is_some() || args.contract.is_some() || args.verdict.is_some()) {
            return Err(GearError::new(ErrorCode::BadFormat, "a direction result needs `--kind result`, `--task`, `--attempt`, and nonempty `--direction` text"));
        }
        if args.verdict.is_some() && args.kind != MsgKind::Review {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "`--verdict` belongs to a review, not to {}",
                    args.kind.as_str()
                ),
            ));
        }
        if args.about.is_some() {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                "`--about` needs registered task dependencies (slice 2)",
            ));
        }
        // The fields belong to their message: the minutes left and the
        // blocker to a progress answer (§5.2), the task waited for to a
        // declared wait (§5.3).
        if (args.left.is_some() || args.blocker.is_some()) && args.kind != MsgKind::Progress {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "`--left` and `--blocker` belong to a progress message, not to {}",
                    args.kind.as_str()
                ),
            ));
        }
        if args.on.is_some() && args.kind != MsgKind::Blocked {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "`--on` belongs to a blocked message, not to {}",
                    args.kind.as_str()
                ),
            ));
        }
        if args.friction.is_some()
            && !matches!(args.kind, MsgKind::Result | MsgKind::Review)
        {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "`--friction` belongs to a hand-in or a review, not to {}",
                    args.kind.as_str()
                ),
            ));
        }
        if args.to.is_some() && args.reply_to.is_some() {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                "`--to` and `--reply-to` cannot be combined",
            ));
        }
        if args.to.is_some() && args.kind != MsgKind::Instruction {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "`--to` addresses a seat with an instruction, not with {}",
                    args.kind.as_str()
                ),
            ));
        }
        // A verdict is recorded for the captain: it is not addressed to
        // a seat and it does not continue another thread.
        if args.kind == MsgKind::Review && (args.to.is_some() || args.reply_to.is_some()) {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                "a review goes to the captain: no `--to`, no `--reply-to`",
            ));
        }

        // The task the message is about (design §3.8). A reviewer seat is
        // bound to a task for life: the messages it writes carry that task,
        // so the captain's answer comes back to the job it belongs to and
        // shows on the reviewer's home page.
        let mut task = args.task.clone();
        if task.is_none() && seat.role == SeatRole::Temp {
            task = seat.task_id.clone();
        }

        // Recipient: the sender of the answered message, a seat the captain
        // addresses, or the project captain.
        let mut acked: Option<String> = None;
        let (recipient, inherited_thread) = if let Some(reply_to) = &args.reply_to {
            let original = tx.get_message(&ctx.project, reply_to)?.ok_or_else(|| {
                GearError::new(
                    ErrorCode::NoTarget,
                    format!("no message {reply_to} in project {}", ctx.project),
                )
            })?;
            // An ack answers an instruction that asks for one, addressed to
            // the seat that gives it (T-320).
            if args.kind == MsgKind::Ack {
                if original.kind != MsgKind::Instruction
                    || !original.needs_ack
                    || original.recipient != seat.id
                {
                    return Err(GearError::new(
                        ErrorCode::BadFormat,
                        format!(
                            "an ack answers an instruction that asks for one: --reply-to {reply_to}"
                        ),
                    ));
                }
                acked = Some(original.id.clone());
            }
            (original.sender, Some(original.thread))
        } else if let Some(to) = &args.to {
            if seat.role != SeatRole::Captain {
                return Err(GearError::new(
                    ErrorCode::Forbidden,
                    "only the captain may address a seat directly",
                ));
            }
            let seat_id = format!("{}:{}", ctx.project, to);
            let target = tx.get_seat(&seat_id)?.ok_or_else(|| {
                GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id} in this project"))
            })?;
            // A message to a reviewer seat carries the task it is about
            // (design §3.8): without it nothing would show on the reviewer's
            // home page. An idle reviewer has no job and no task, so the
            // instruction would be invisible — it is refused, with the
            // reason.
            if task.is_none() && target.role == SeatRole::Temp {
                match target.task_id.clone() {
                    Some(job) => task = Some(job),
                    None => {
                        return Err(GearError::new(
                            ErrorCode::BadFormat,
                            format!("{to} has no review job; nothing would show"),
                        ))
                    }
                }
            }
            (seat_id, None)
        } else {
            let captain = tx.captain_of(&ctx.project)?.ok_or_else(|| {
                GearError::new(
                    ErrorCode::NoTarget,
                    format!("project {} has no captain", ctx.project),
                )
            })?;
            (captain.id, None)
        };

        let state = match args.kind {
            MsgKind::Progress | MsgKind::Result | MsgKind::Blocked | MsgKind::Ack => {
                MsgState::Info
            }
            MsgKind::Review if args.verdict == Some(crate::review::Outcome::Pass) => MsgState::Info,
            _ => MsgState::Open,
        };

        // The review the verdict belongs to (a review names its task, attempt
        // and verdict; the body holds the findings). It is built inside the
        // request closure, after the replay check: a repeated request id
        // replays the stored result and never touches the reviewer's file
        // again (T-21, review F2). The key keeps the original argument.
        let args_key = request_key(&ctx.project, &ctx.seat, ctx.generation, args, direction, ack);

        let outcome = tx.run_request(&args.request_id, &args_key, now, |tx| {
            // The review the verdict belongs to: the findings are resolved
            // here, after the replay check. A path to a file in the
            // reviewer's cwd is read into the store; a missing file fails
            // before anything is written (T-21). Only a temp reviewer's
            // findings are read: a lane or the captain is refused as a
            // reviewer, and that is the error they should see.
            let review = match (args.kind, args.verdict) {
                (MsgKind::Review, Some(verdict)) => Some(crate::review::Review {
                    project: ctx.project.clone(),
                    task: task.clone().unwrap_or_default(),
                    attempt: args.attempt.unwrap_or_default(),
                    outcome: verdict,
                    findings: if seat.role == SeatRole::Temp {
                        crate::review::findings_text(&args.body)?
                    } else {
                        args.body.clone()
                    },
                    // The ids the reviewer named as blocking (T-23).
                    blocking: args.blocking.clone().unwrap_or_default(),
                    by_seat: seat.id.clone(),
                }),
                _ => None,
            };

            // The body of a review is the findings; the captain reads the
            // verdict in the first line of the item (design/07: the inbox
            // shows the body).
            let body = match &review {
                Some(review) => format!("{}\n{}", crate::review::headline(review), review.findings),
                None => args.body.clone(),
            };

            // A result submits the attempt before the message is stored; an
            // error here aborts the whole send, so nothing is kept.
            let mut direction_sha = None;
            if direction {
                let task_id = task.as_deref().unwrap_or_default();
                let attempt_n = args.attempt.unwrap_or_default();
                let job = tx.get_meta(&format!("engine.direction.{}.{}.{}", ctx.project, task_id, attempt_n))?
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
                let valid = seat.role == SeatRole::Temp && seat.task_id.as_deref() == Some(task_id)
                    && job.as_ref().is_some_and(|j| j["seat"].as_str() == Some(seat.id.as_str())
                        && j["generation"].as_i64() == Some(ctx.generation) && j["state"].as_str() == Some("running"));
                if !valid { return Err(GearError::new(ErrorCode::Forbidden, "this seat does not hold the direction job for that attempt")); }
                let attempt = tx.get_attempt(&ctx.project, task_id, attempt_n)?.ok_or_else(|| GearError::new(ErrorCode::NoTarget, "direction attempt not found"))?;
                if !matches!(attempt.state, AttemptState::Started | AttemptState::Submitted) { return Err(GearError::new(ErrorCode::BadTransition, "direction attempt is neither Started nor Submitted")); }
                direction_sha = job.and_then(|j| j["sha"].as_str().filter(|s| !s.is_empty()).map(str::to_owned));
                if direction_sha.is_none() { return Err(GearError::new(ErrorCode::BadFormat, "direction job has no checked sha")); }
            } else if args.kind == MsgKind::Result {
                let result_task = task.as_deref().ok_or_else(|| GearError::new(ErrorCode::BadFormat, "a result needs `--task`"))?;
                let result_attempt = args.attempt.ok_or_else(|| GearError::new(ErrorCode::BadFormat, "a result needs `--attempt`"))?;
                let owner = tx.get_attempt(&ctx.project, result_task, result_attempt)?.ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no attempt {result_attempt} of task {result_task}")))?;
                if owner.seat != seat.id || (owner.state == AttemptState::Delivered && owner.generation != ctx.generation) {
                    return Err(GearError::new(ErrorCode::Forbidden, format!("attempt {result_attempt} of task {result_task} is not owned by this seat and generation")));
                }
                // The friction report of the round (design §6.1): when the
                // round passed its budget, the report is required (§6.2) and
                // the hand-in is refused without it.
                let role = if args.attempt.unwrap_or_default() >= 2 {
                    "rework"
                } else {
                    "dev"
                };
                let required = check_friction(
                    tx,
                    &ctx.project,
                    task.as_deref().unwrap_or_default(),
                    args.attempt.unwrap_or_default(),
                    role,
                    args.friction.as_deref(),
                    now,
                )?;
                let handed_in = crate::tasks::submit_result(
                    tx,
                    &ctx.project,
                    task.as_deref().ok_or_else(|| {
                        GearError::new(ErrorCode::BadFormat, "a result needs `--task`")
                    })?,
                    args.attempt.ok_or_else(|| {
                        GearError::new(ErrorCode::BadFormat, "a result needs `--attempt`")
                    })?,
                    args.contract.ok_or_else(|| {
                        GearError::new(ErrorCode::BadFormat, "a result needs `--contract`")
                    })?,
                    args.candidate.as_deref().ok_or_else(|| {
                        GearError::new(ErrorCode::BadFormat, "a result needs `--candidate`")
                    })?,
                    &args.body,
                    now,
                );
                // T-266: a hand-in that fails because the lane's work was
                // already sent back is answered with the rework that waits
                // for it, not with the state error the engine hit. Every
                // other failure keeps its own text.
                if let Err(error) = handed_in {
                    if let Some(rework) = rework_waiting(
                        tx,
                        &ctx.project,
                        &seat.id,
                        result_task,
                        result_attempt,
                    )? {
                        return Err(rework);
                    }
                    return Err(error);
                }
                store_friction(
                    tx,
                    &ctx.project,
                    task.as_deref().unwrap_or_default(),
                    args.attempt.unwrap_or_default(),
                    role,
                    &seat.id,
                    args.friction.as_deref(),
                    required,
                )?;
            }

            // A review stores the verdict on the attempt's review record
            // before the item reaches the captain (T-2). It settles nothing:
            // the task and the attempt keep their state.
            if let Some(review) = &review {
                // The reviewer's own friction report (§6.1): the review round
                // has a budget too (§5.1).
                let required = check_friction(
                    tx,
                    &ctx.project,
                    &review.task,
                    review.attempt,
                    "review",
                    args.friction.as_deref(),
                    now,
                )?;
                crate::review::submit_review(tx, review, now)?;
                store_friction(
                    tx,
                    &ctx.project,
                    &review.task,
                    review.attempt,
                    "review",
                    &seat.id,
                    args.friction.as_deref(),
                    required,
                )?;
            }

            // A declared wait (design §5.3): the lane's task waits for
            // another task to land, or for something outside GEAR. The
            // attempt stays Started and the lane ends its turn; the wait
            // itself is on the task.
            if args.kind == MsgKind::Blocked {
                declare_wait(
                    tx,
                    &ctx.project,
                    &seat,
                    args.task.as_deref().unwrap_or_default(),
                    args.attempt.unwrap_or_default(),
                    args.contract.unwrap_or_default(),
                    args.on.as_deref().unwrap_or_default(),
                    &body,
                    now,
                )?;
            }

            // The thread of a new message is numbered here, after the replay
            // check: a replay returns the stored result and numbers nothing.
            let thread = match &inherited_thread {
                Some(thread) => thread.clone(),
                None => {
                    let prefix = task.clone().unwrap_or_else(|| ctx.seat.clone());
                    let seq = next_thread_seq(tx, &ctx.project, &recipient, now, &prefix)?;
                    format!("{prefix}:{seq}")
                }
            };
            let message = NewMessage {
                meta: if let Some(sha) = direction_sha { Some(serde_json::json!({"direction": true, "sha": sha}).to_string()) } else { progress_meta(args.left, args.blocker.as_deref())? },
                project: ctx.project.clone(),
                thread: thread.clone(),
                task: task.clone(),
                attempt: args.attempt,
                sender: seat.id.clone(),
                sender_gen: ctx.generation,
                recipient: recipient.clone(),
                kind: args.kind,
                urgent: args.urgent,
                body: body.clone(),
                reply_to: args.reply_to.clone(),
                needs_ack: ack,
                state,
                created: now,
            };
            let stored = tx.insert_message(&message)?;

            // An instruction the captain needs confirmed (design §10.7, row
            // B7): the lane is told how to acknowledge it, and the captain
            // gets an item that waits ten minutes for that ack, so an
            // instruction nobody read comes back into the inbox.
            if ack {
                let body = format!(
                    "{}\nAcknowledge it: gear send --kind ack --reply-to {} \"<what you will do>\"",
                    stored.body, stored.id
                );
                tx.update_message_body(&ctx.project, &stored.id, &body)?;
                let seat_name = args.to.clone().unwrap_or_default();
                let preview: String = stored.body.chars().take(60).collect();
                let item = NewMessage {
                    meta: None,
                    project: ctx.project.clone(),
                    thread: format!("ack:{}", stored.id),
                    task: stored.task.clone(),
                    attempt: stored.attempt,
                    sender: seat.id.clone(),
                    sender_gen: ctx.generation,
                    recipient: seat.id.clone(),
                    kind: MsgKind::System,
                    urgent: false,
                    body: format!(
                        "{seat_name} has not acknowledged {} yet: {preview}",
                        stored.id
                    ),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: now,
                };
                let item = tx.insert_message(&item)?;
                tx.update_message_state(
                    &ctx.project,
                    &item.id,
                    MsgState::Waiting,
                    None,
                    None,
                    Some(now + 600),
                    now,
                )?;
            }

            // The ack closes the captain's reminder for the instruction it
            // answers: the item stays as the record, resolved with the ack.
            if let Some(original) = &acked {
                for item in tx.thread_messages(&ctx.project, &format!("ack:{original}"))? {
                    if matches!(item.state, MsgState::Open | MsgState::Waiting) {
                        tx.update_message_state(
                            &ctx.project,
                            &item.id,
                            MsgState::Resolved,
                            Some(&format!("acknowledged in {}", stored.id)),
                            None,
                            None,
                            now,
                        )?;
                    }
                }
            }
            Ok(serde_json::json!({
                "project": stored.project,
                "msg_id": stored.id,
                "thread": stored.thread,
                "recipient": stored.recipient,
            }))
        })?;

        let (value, replayed) = match outcome {
            RequestOutcome::Executed(value) => (value, false),
            RequestOutcome::Replayed(value) => (value, true),
        };
        let project = value["project"].as_str().unwrap_or_default().to_string();
        let msg_id = value["msg_id"].as_str().unwrap_or_default().to_string();
        let thread = value["thread"].as_str().unwrap_or_default().to_string();
        let recipient = value["recipient"].as_str().unwrap_or_default().to_string();
        Ok(SendResult { project, msg_id, thread, recipient, replayed })
    })
}

/// The receipt of a request that already committed, if the caller is the live
/// seat that sent it: the seat, generation, binding and fingerprint are checked
/// in the **same read transaction** that reads the stored request (review
/// round 2, F6). No receipt, no `GEAR_CTX` or a different args key → None (the
/// write path decides, as before); a caller that is no longer the seat gets
/// its `NotRegistered` / `StaleGeneration` / `Forbidden` here.
fn stored_receipt(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    args: &SendArgs,
    direction: bool,
    ack: bool,
) -> GearResult<Option<SendResult>> {
    if env.ctx.is_none() {
        return Ok(None);
    }
    store.read(|tx| {
        let Some((stored_hash, stored)) = tx.get_request(&args.request_id)? else {
            return Ok(None);
        };
        let (ctx, _seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Lane, SeatRole::Temp, SeatRole::Captain]),
            Some(&[SeatState::Active, SeatState::Standby, SeatState::Retiring]),
        )?;
        let key = request_key(&ctx.project, &ctx.seat, ctx.generation, args, direction, ack);
        if stored_hash != gear_store::args_hash(&key) {
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_str(&stored).map_err(|e| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("stored send result for {} is broken: {e}", args.request_id),
            )
        })?;
        Ok(Some(SendResult {
            project: env.ctx.as_ref().map(|ctx| ctx.project.clone()).unwrap_or_default(),
            msg_id: value["msg_id"].as_str().unwrap_or_default().to_string(),
            thread: value["thread"].as_str().unwrap_or_default().to_string(),
            recipient: value["recipient"].as_str().unwrap_or_default().to_string(),
            replayed: true,
        }))
    })
}

/// The args key of a send: the same seat and generation with the same
/// arguments is the same request (design/13), whether it is checked before the
/// transaction (to replay a committed receipt) or inside it.
fn request_key(
    project: &str,
    seat: &str,
    generation: i64,
    args: &SendArgs,
    direction: bool,
    ack: bool,
) -> String {
    let key = format!(
        "{project}|{seat}:{generation}|send|kind={}|body={:?}|task={:?}|attempt={:?}|contract={:?}|candidate={:?}|verdict={:?}|about={:?}|to={:?}|reply_to={:?}|urgent={}|left={:?}|blocker={:?}|on={:?}|friction={:?}",
        args.kind.as_str(),
        args.body,
        args.task,
        args.attempt,
        args.contract,
        args.candidate,
        args.verdict.map(|verdict| verdict.as_str()),
        args.about,
        args.to,
        args.reply_to,
        args.urgent,
        args.left,
        args.blocker,
        args.on,
        args.friction
    );
    let key = if direction { format!("{key}|direction=true") } else { key };
    if ack { format!("{key}|ack=true") } else { key }
}

/// The next unused sequence number of a thread with this prefix.
///
/// The threads that are still unhandled are read in one query and give the
/// highest number in use; the numbers a resolved thread already took are then
/// skipped one by one, so a number is never reused and two questions never
/// end up in the same thread.
fn next_thread_seq(
    tx: &Tx<'_>,
    project: &str,
    recipient: &str,
    now: Ts,
    prefix: &str,
) -> GearResult<u32> {
    let unhandled = tx.actionable_for(project, recipient, now)?;
    let mut seq = unhandled
        .iter()
        .filter_map(|message| thread_seq(&message.thread, prefix))
        .max()
        .unwrap_or(0);
    loop {
        seq += 1;
        if seq > MAX_THREAD_SEQ {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("too many threads named {prefix}:* in project {project}"),
            ));
        }
        if tx.thread_messages(project, &format!("{prefix}:{seq}"))?.is_empty() {
            return Ok(seq);
        }
    }
}

/// The sequence number of `thread`, when the thread belongs to `prefix`.
fn thread_seq(thread: &str, prefix: &str) -> Option<u32> {
    thread.strip_prefix(prefix)?.strip_prefix(':')?.parse().ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    pub thread: String,
    pub urgent: bool,
    /// Unhandled messages in this thread (max 3 shown), oldest first.
    pub open: Vec<MessageRow>,
    /// How many more unhandled messages exist beyond `open`.
    pub more: usize,
    /// Latest reply/decision in the thread, for context.
    pub last_reply: Option<MessageRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    pub max_threads: usize,
    pub per_sender: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        BatchLimits { max_threads: 6, per_sender: 2 }
    }
}

/// Pick the next batch for `recipient` (design/07):
/// 1. candidates = Open + due Waiting;
/// 2. if any non-urgent thread exists, reserve one slot for the oldest one;
/// 3. fill the rest: urgent threads first, then oldest first; at most
///    `per_sender` threads per sender (the reserved one counts);
/// Returns threads in display order. Reading never changes state.
///
/// A thread counts once for the sender of its oldest unhandled message, and
/// `max_threads = 0` returns no thread at all.
pub fn select_batch(
    tx: &Tx<'_>,
    project: &str,
    recipient: &str,
    now: Ts,
    limits: BatchLimits,
) -> GearResult<Vec<ThreadView>> {
    // 1. candidates: open messages and waiting messages that are due.
    let candidates = tx.actionable_for(project, recipient, now)?;

    // Group them by thread, keeping the order of the oldest message.
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<MessageRow>> = BTreeMap::new();
    for message in candidates {
        if !groups.contains_key(&message.thread) {
            order.push(message.thread.clone());
        }
        groups.entry(message.thread.clone()).or_default().push(message);
    }

    // One view per thread.
    let mut views: Vec<ThreadView> = Vec::new();
    let mut oldest: Vec<Ts> = Vec::new();
    let mut senders: Vec<String> = Vec::new();
    for thread in &order {
        let messages = groups.get(thread).cloned().unwrap_or_default();
        let shown: Vec<MessageRow> = messages.iter().take(SHOWN_PER_THREAD).cloned().collect();
        let more = messages.len() - shown.len();
        let urgent = messages.iter().any(|m| m.urgent);
        // The latest reply **to this seat** is context, not an unhandled item
        // (T-103): a reply the captain wrote to the engine, to himself or to
        // another seat is not the recipient's business, and a lane must not
        // read it as if it were.
        let last_reply = tx
            .thread_messages(project, thread)?
            .into_iter()
            .filter(|m| m.kind == MsgKind::Reply && m.recipient == recipient)
            .max_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        views.push(ThreadView {
            thread: thread.clone(),
            urgent,
            open: shown,
            more,
            last_reply,
        });
        oldest.push(messages.first().map(|m| m.created).unwrap_or_default());
        senders.push(messages.first().map(|m| m.sender.clone()).unwrap_or_default());
    }

    // 2. one slot is reserved for the oldest normal thread.
    let mut selected: Vec<usize> = Vec::new();
    let mut per_sender: BTreeMap<String, usize> = BTreeMap::new();
    if limits.max_threads > 0 && limits.per_sender > 0 {
        if let Some(index) = views.iter().position(|view| !view.urgent) {
            selected.push(index);
            *per_sender.entry(senders[index].clone()).or_insert(0) += 1;
        }
    }

    // 3. the rest: urgent threads first, then oldest first, per sender capped.
    let mut rest: Vec<usize> = (0..views.len()).filter(|i| !selected.contains(i)).collect();
    rest.sort_by_key(|index| (if views[*index].urgent { 0 } else { 1 }, oldest[*index]));
    for index in rest {
        if selected.len() >= limits.max_threads {
            break;
        }
        let sender = senders[index].clone();
        let taken = per_sender.get(&sender).copied().unwrap_or(0);
        if taken >= limits.per_sender {
            continue;
        }
        selected.push(index);
        *per_sender.entry(sender).or_insert(0) += 1;
    }

    // Display order: urgent threads first, then oldest first.
    selected.sort_by_key(|index| (if views[*index].urgent { 0 } else { 1 }, oldest[*index]));
    Ok(selected.into_iter().map(|index| views[index].clone()).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    Resolve,
    Wait,
    Note,
    /// Open an item that was resolved by mistake (design §10.7, row
    /// B03-07). The item goes back to the captain's inbox; nothing is sent
    /// to the seat that asked.
    Reopen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyArgs {
    pub ids: Vec<String>,
    pub text: String,
    pub mode: ReplyMode,
    pub wait_on: Option<String>,
    pub wait_until: Option<Ts>,
    /// Only adjusts urgency; with no explicit mode the CLI passes Note.
    pub downgrade: bool,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplyResult {
    /// (message id, new state as string)
    pub handled: Vec<(String, String)>,
    /// Reply messages created, one per (original sender, thread).
    pub replies: Vec<String>,
    pub replayed: bool,
}

/// May this item be handled in this mode? (design §10.7, row B03-07): a
/// reopen works on a resolved item — one resolved by mistake — and every
/// other mode works on an open or waiting one; only a resolve may close an
/// informational record.
fn handlable(message: &MessageRow, mode: ReplyMode) -> bool {
    match mode {
        ReplyMode::Reopen => message.state == MsgState::Resolved,
        _ => {
            message.state == MsgState::Open
                || message.state == MsgState::Waiting
                || (message.state == MsgState::Info && mode == ReplyMode::Resolve)
        }
    }
}

/// The friction report of a round (design §6.1): what cost the lane or the
/// reviewer time that the captain could fix.
///
/// When the round is over its budget the report is required (§6.2): the
/// hand-in is refused with `FRICTION_REQUIRED` and the numbers that say so.
/// Nothing is written when it refuses. Returns whether the report was
/// required.
fn check_friction(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
    role: &str,
    friction: Option<&str>,
    now: Ts,
) -> GearResult<bool> {
    let rounds = tx.rounds_of_task(project, task)?;
    let round = rounds
        .iter()
        .find(|round| round.attempt == attempt && round.role == role);
    let model = round.and_then(|round| round.model.as_deref());
    let budget = tx.round_budget_min(project, task, attempt, role, model)?;
    // The round's own clock (design §5.1). A review is charged from the
    // reviewer's own start — the review round's start — and never from the
    // writer's check-in: the reviewer did not spend the writer's time. The
    // writer's attempt carries the writer's times, so reading them here would
    // charge the reviewer for the writer's hour (T-193).
    let started = if role == "review" {
        round.map(|round| round.start_at)
    } else {
        tx.attempt_times(project, task, attempt)
            .ok()
            .and_then(|times| times.started)
            .or_else(|| {
                tx.get_attempt(project, task, attempt)
                    .ok()
                    .flatten()
                    .map(|attempt| attempt.started)
            })
    };
    // The active time of the round: now − its start − its own pauses.
    let paused = round.map(|round| round.paused_s).unwrap_or(0);
    let active_s = match started {
        Some(started) => (now - started - paused).max(0),
        None => 0,
    };
    // The budget is in minutes and the active time in seconds: a round that is
    // one second over its budget is over (§6.2). The minutes are for the
    // message only.
    let over = active_s > budget * 60;
    if over && friction.is_none() {
        return Err(GearError::new(
            ErrorCode::FrictionRequired,
            format!(
                "{task} took {} min against {budget} min. Add --friction \"<what made it slow>\" and send again.",
                (active_s + 59) / 60
            ),
        ));
    }
    Ok(over)
}

/// Store the friction report of a round (design §6.1): what cost time that
/// the captain could fix, with the round's own facts (§6.3).
fn store_friction(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
    role: &str,
    seat: &str,
    friction: Option<&str>,
    required: bool,
) -> GearResult<()> {
    let Some(body) = friction else { return Ok(()) };
    tx.insert_friction(&gear_store::NewFriction {
        project: project.to_string(),
        task: task.to_string(),
        attempt,
        role: role.to_string(),
        seat: seat.to_string(),
        body: body.to_string(),
        required,
    })?;
    Ok(())
}

/// The structured fields of a progress answer (design §5.2): the minutes the
/// lane says are left and what stops it. `None` when the answer carries
/// neither.
fn progress_meta(left: Option<i64>, blocker: Option<&str>) -> GearResult<Option<String>> {
    if left.is_none() && blocker.is_none() {
        return Ok(None);
    }
    let mut fields = serde_json::Map::new();
    if let Some(left) = left {
        fields.insert("left".to_string(), serde_json::Value::from(left));
    }
    if let Some(blocker) = blocker {
        fields.insert(
            "blocker".to_string(),
            serde_json::Value::String(blocker.to_string()),
        );
    }
    Ok(Some(serde_json::Value::Object(fields).to_string()))
}

/// T-266: the rework that waits for a lane whose hand-in failed.
///
/// After a fail verdict the engine opens the next attempt on the lane's seat
/// and delivers its packet only when the lane is idle. A lane still in its
/// turn hands in the old attempt again and reads a state error; this finds
/// the newer attempt (same seat, not handed in yet) so the refusal can say
/// what actually waits. `None` when there is no such attempt: every other
/// failure keeps its own text.
fn rework_waiting(
    tx: &Tx<'_>,
    project: &str,
    seat: &str,
    task: &str,
    named: i64,
) -> GearResult<Option<GearError>> {
    let mut newer = None;
    for attempt in tx.attempts_of_task(project, task)? {
        if attempt.n <= named || attempt.seat != seat {
            continue;
        }
        // Not handed in yet: the rework has not reached the reviewer.
        if matches!(
            attempt.state,
            AttemptState::Delivered | AttemptState::Started | AttemptState::Stopped | AttemptState::Unknown
        ) {
            newer = Some(attempt.n);
        }
    }
    Ok(newer.map(|n| {
        GearError::new(
            ErrorCode::BadTransition,
            format!(
                "a rework of {task} (attempt {n}) is waiting for you: end your turn now and its \
                 packet arrives; do not hand in attempt {named} again"
            ),
        )
    }))
}

/// A declared wait (design §5.3): the lane's task cannot finish until another
/// task lands, or until something outside GEAR is done (a CI run, evidence
/// the captain must fix) (T-440).
///
/// The wait is written on the task (`wait_on`, `wait_cond`); the attempt
/// stays Started, so the lane's round goes on when the wait ends. The message
/// itself is the captain's info item: the wait is a fact, not an action.
///
/// `on` is the task it waits for, or `external`: then the body says what the
/// lane waits for (required), the wait lasts `EXTERNAL_WAIT_MIN` and the
/// captain gets one info item naming it.
///
/// Accepted only when the task it waits for exists and is not Done: a wait
/// for a task that is already done would never end.
fn declare_wait(
    tx: &Tx<'_>,
    project: &str,
    seat: &gear_store::SeatRow,
    task: &str,
    attempt: i64,
    contract: i64,
    on: &str,
    body: &str,
    now: Ts,
) -> GearResult<()> {
    let attempt_row = tx.get_attempt(project, task, attempt)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no attempt {attempt} of task {task} in project {project}"),
        )
    })?;
    if attempt_row.seat != seat.id {
        return Err(GearError::new(
            ErrorCode::Forbidden,
            format!(
                "attempt {attempt} of task {task} belongs to {}, not to {}",
                attempt_row.seat, seat.id
            ),
        ));
    }
    // The contract the lane names must be the one the attempt works to and
    // the one the task carries now — the same two checks a hand-in makes. A
    // captain who changed the contract (and ended the wait with it) must not
    // have the lane's old command park the task on an obsolete dependency.
    if contract != attempt_row.adopted_version {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "attempt {attempt} of task {task} works to contract {}, not {contract}",
                attempt_row.adopted_version
            ),
        ));
    }
    let task_row = tx.get_task(project, task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {task} in project {project}"),
        )
    })?;
    if contract != task_row.contract_version {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{task} is on contract v{}, not the v{contract} this message names: \
                 run `gear` and `gear --show {task}` to work to v{}, then declare the wait again",
                task_row.contract_version, task_row.contract_version
            ),
        ));
    }
    if attempt_row.state != AttemptState::Started {
        return Err(GearError::new(
            ErrorCode::BadTransition,
            format!(
                "attempt {attempt} of task {task} is {}, not started: a wait is declared while the work goes on",
                attempt_row.state.as_str()
            ),
        ));
    }
    // Waiting on something outside GEAR (T-440): the body says what for, and
    // the wait lasts no longer than EXTERNAL_WAIT_MIN — the engine ends it,
    // so the no-push clock cannot be silenced for good.
    if on == "external" {
        let reason = body.trim();
        if reason.is_empty() {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                "a wait on `external` needs the reason as the message body: what you wait for",
            ));
        }
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} has no captain"),
            )
        })?;
        tx.set_task_wait(project, task, on, "external")?;
        tx.open_pause(project, &seat.id, "wait", &format!("waiting for: {reason}"), now)?;
        tx.set_meta(
            &external_wait_key(project, task),
            &(now + EXTERNAL_WAIT_MIN * 60).to_string(),
        )?;
        tx.insert_message(&NewMessage {
            project: project.into(),
            thread: format!("wait-external:{task}:{attempt}"),
            task: Some(task.into()),
            attempt: Some(attempt),
            sender: "engine".into(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: false,
            body: format!(
                "{} waits on {task} for: {reason} (at most {EXTERNAL_WAIT_MIN} min)",
                seat.name
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Info,
            created: now,
            meta: None,
        })?;
        return Ok(());
    }
    let waited = tx.get_task(project, on)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {on} in project {project} to wait for"),
        )
    })?;
    if waited.state == TaskState::Done {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("{on} is done: there is nothing to wait for"),
        ));
    }
    tx.set_task_wait(project, task, on, "merged")?;
    tx.open_pause(project, &seat.id, "wait", &format!("waiting for {on}"), now)?;
    Ok(())
}

/// `gear send` (captain only). Each id must be in this project, addressed to
/// the captain, Open or Waiting (Info may only be resolved). Resolve → Resolved; Wait → Waiting until
/// wait_until (required; wait_on is an optional label); Note → state unchanged,
/// with the text kept on the item and sent to nobody (T-103: a note is the
/// captain's record, not a message); Reopen → a Resolved item goes back to
/// Open (design §10.7, row B03-07: an item resolved by mistake is reopened),
/// and nothing is sent to anybody.
///
/// Resolve and Wait write one Reply (state Info) per (live sender, thread) into
/// that sender's inbox for actionable items — only when the sender is a seat of
/// this project that is still running **at the generation that asked**
/// (T-103): the engine has no seat to read an answer, and a reviewer seat
/// reused for a later job must not read the note on an earlier one. Info and
/// Result records never send an answer, and neither does an item the captain
/// itself raised: its text stays the item's record (T-119). Never changes task
/// state.
///
/// Slice 1 rules:
/// - a repeated id is handled once; `handled` lists every id once;
/// - one answer per (sender, thread): every thread a sender has listed gets
///   an answer of its own, so nothing is answered silently;
/// - `Wait` needs `wait_until`, the recheck time that makes the message due
///   again; `wait_on` is only a label.
pub fn reply(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    args: &ReplyArgs,
    now: Ts,
) -> GearResult<ReplyResult> {
    store.write(|tx| {
        // Only the captain answers, and only in a state a captain can hold.
        let (ctx, seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Captain]),
            Some(&[SeatState::Active]),
        )?;
        let captain_id = seat.id.clone();

        // A wait without a recheck time would never become due again.
        if args.mode == ReplyMode::Wait && args.wait_until.is_none() {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                "a wait needs `wait_until`, the time it is checked again",
            ));
        }

        let args_key = format!(
            "{}|{}:{}|reply|ids={:?}|mode={:?}|text={:?}|wait_on={:?}|wait_until={:?}|downgrade={}",
            ctx.project,
            ctx.seat,
            ctx.generation,
            args.ids,
            args.mode,
            args.text,
            args.wait_on,
            args.wait_until,
            args.downgrade
        );

        let outcome = tx.run_request(&args.request_id, &args_key, now, |tx| {
            // Check every id before writing anything: a rejected id leaves
            // the other messages of the same call untouched. The check runs
            // after the replay check, so a repeated request returns the
            // stored result instead of meeting a closed message.
            let mut originals: Vec<MessageRow> = Vec::new();
            let mut seen: Vec<String> = Vec::new();
            for id in &args.ids {
                // A repeated id is handled once; the first place keeps it.
                if seen.iter().any(|known| known == id) {
                    continue;
                }
                seen.push(id.clone());
                let message = tx.get_message(&ctx.project, id)?.ok_or_else(|| {
                    GearError::new(
                        ErrorCode::NoTarget,
                        format!("no message {id} in project {}", ctx.project),
                    )
                })?;
                if message.recipient != captain_id {
                    return Err(GearError::new(
                        ErrorCode::Forbidden,
                        format!(
                            "message {id} is addressed to {}, not to the captain",
                            message.recipient
                        ),
                    ));
                }
                if !handlable(&message, args.mode) {
                    return Err(GearError::new(
                        ErrorCode::BadTransition,
                        format!("message {id} is {}, it cannot be handled again", message.state.as_str()),
                    ));
                }
                if crate::station::is_station(&message) && args.mode != ReplyMode::Note {
                    return Err(GearError::new(ErrorCode::BadFormat, format!(
                        "station {} needs a plan # Decisions line: station {}: keep course|time-box|cut|move",
                        message.id, message.id,
                    )));
                }
                // A review fail (or a failed merge) is settled where it can
                // be: by a plan line — a `# Verdicts` line, or a `# Staff`
                // `- remerge T-<n>` line for a failed merge (B5). Resolving it
                // by reply would close the item and settle nothing (T-90).
                if args.mode == ReplyMode::Resolve {
                    if let Some(hint) = crate::tasks::verdict_hint(tx, &ctx.project, &message)? {
                        let mut detail = format!(
                            "message {} is settled by a plan {} line, not by a reply: next: {}",
                            message.id,
                            crate::home::verdict_section(&hint),
                            crate::home::verdict_command(&hint),
                        );
                        // A failed merge is retried with `- remerge T-<n>` (B5);
                        // a rework is the other way round, and the line for it
                        // is named too.
                        if hint.remerge.is_some() {
                            detail.push_str(&format!(
                                " (a rework is a `# Verdicts` line: {})",
                                hint.line
                            ));
                        }
                        return Err(GearError::new(ErrorCode::BadFormat, detail));
                    }
                }
                originals.push(message);
            }

            let mut handled: Vec<(String, String)> = Vec::new();
            let mut replies: Vec<String> = Vec::new();
            // One answer per (sender, thread).
            let mut answered: Vec<(String, String)> = Vec::new();
            for original in &originals {
                let new_state = match args.mode {
                    ReplyMode::Resolve => MsgState::Resolved,
                    ReplyMode::Wait => MsgState::Waiting,
                    ReplyMode::Note => original.state,
                    // A resolved item resolved by mistake goes back to the
                    // captain's inbox (design §10.7, row B03-07). The model's
                    // transition table (gear-model, deck's file) knows no
                    // Resolved → Open, so the reopen states it here: nothing
                    // else reopens, and nothing else moves.
                    ReplyMode::Reopen => MsgState::Open,
                };
                if new_state != original.state {
                    let allowed = match args.mode {
                        ReplyMode::Reopen => original.state == MsgState::Resolved,
                        _ => MsgState::can_transition(original.state, new_state),
                    };
                    gear_model::state::require_transition(
                        allowed,
                        "reply",
                        original.state.as_str(),
                        new_state.as_str(),
                    )?;
                }
                // The text is the item's own record. A note keeps the state
                // and the wait fields it had; resolve and wait set their own
                // (T-103).
                let (wait_on, wait_until) = match args.mode {
                    ReplyMode::Note | ReplyMode::Reopen => (original.wait_on.clone(), original.wait_until),
                    _ => (args.wait_on.clone(), args.wait_until),
                };
                tx.update_message_state(
                    &ctx.project,
                    &original.id,
                    new_state,
                    Some(&args.text),
                    wait_on.as_deref(),
                    wait_until,
                    now,
                )?;
                handled.push((original.id.clone(), new_state.as_str().to_string()));

                // A note is the captain's bookkeeping: stored on the item just
                // above, sent to nobody (T-103). A reopen is the same: the
                // item is back in the inbox, the captain answers it there.
                if args.mode == ReplyMode::Note || args.mode == ReplyMode::Reopen {
                    continue;
                }

                // An item the captain raised itself has no other seat to
                // answer: the text is already the item's record above, and a
                // reply addressed to the captain is the same seat — no new
                // fact, and the engine would ring the captain's own doorbell
                // for nothing (T-119).
                if original.sender == captain_id {
                    continue;
                }

                // Information is bookkeeping, not a conversation. Legacy
                // open Result records must not echo resolve text either.
                if original.state == MsgState::Info || original.kind == MsgKind::Result {
                    continue;
                }

                // Only a live seat of the generation that asked gets the
                // answer (T-103): never the engine, never a released or
                // retired seat, never a seat reused for another job. A
                // Retiring lane is still live: it finishes the attempt it
                // holds, so an answer to its question must reach it (F1 of the
                // review).
                let live = tx.get_seat(&original.sender)?.filter(|seat| {
                    seat.generation == original.sender_gen
                        && matches!(
                            seat.state,
                            SeatState::Starting
                                | SeatState::Active
                                | SeatState::Standby
                                | SeatState::Retiring
                        )
                });
                if live.is_none() {
                    continue;
                }

                // One answer per thread of the sender, in the thread of the message.
                let key = (original.sender.clone(), original.thread.clone());
                if !answered.contains(&key) {
                    answered.push(key);
                    let answer = NewMessage {
                        meta: None,
                        project: ctx.project.clone(),
                        thread: original.thread.clone(),
                        task: original.task.clone(),
                        attempt: original.attempt,
                        sender: captain_id.clone(),
                        sender_gen: ctx.generation,
                        recipient: original.sender.clone(),
                        kind: MsgKind::Reply,
                        urgent: false,
                        body: args.text.clone(),
                        reply_to: Some(original.id.clone()),
                        needs_ack: false,
                        state: MsgState::Info,
                        created: now,
                    };
                    let stored = tx.insert_message(&answer)?;
                    replies.push(stored.id);
                }
            }
            Ok(serde_json::json!({
                "handled": handled,
                "replies": replies,
            }))
        })?;

        let (value, replayed) = match outcome {
            RequestOutcome::Executed(value) => (value, false),
            RequestOutcome::Replayed(value) => (value, true),
        };
        let handled: Vec<(String, String)> =
            serde_json::from_value(value["handled"].clone()).map_err(|e| {
                GearError::new(ErrorCode::BadFormat, format!("stored reply result is broken: {e}"))
            })?;
        let replies: Vec<String> =
            serde_json::from_value(value["replies"].clone()).map_err(|e| {
                GearError::new(ErrorCode::BadFormat, format!("stored reply result is broken: {e}"))
            })?;
        Ok(ReplyResult { handled, replies, replayed })
    })
}
