//! Messages: send, reply, inbox batch selection (design/07).

use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{MessageRow, NewMessage, RequestOutcome, Store, Tx};
use std::collections::BTreeMap;

/// Upper bound for the thread sequence search, so a broken database cannot
/// spin forever.
const MAX_THREAD_SEQ: u32 = 10_000;

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
    /// Upstream task to ask (must be a registered dependency) — slice 2.
    pub about: Option<String>,
    /// Captain only: seat name to instruct.
    pub to: Option<String>,
    pub reply_to: Option<String>,
    pub urgent: bool,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SendResult {
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
/// - Progress → Info; Question/Blocked/Result/Instruction → Open;
///   Reply-kind is produced by `reply()`, not by `send`;
/// - thread: reply_to inherits; otherwise new thread `<task or seat>:<seq>` (seq = next unused number for that prefix, design/07);
/// - Result (T-45): `task`, `attempt`, `contract` and `candidate` required (else
///   BadFormat); in the same transaction `tasks::submit_result(…, body as report)`
///   — its errors abort the send; the message goes to the captain as usual.
/// - Review (T-2): `task`, `attempt` and `verdict` required (else BadFormat);
///   only a temp seat bound to that task may send one (Forbidden/NoTarget);
///   in the same transaction `review::submit_review(…)` stores the verdict on
///   the attempt's review record — it settles nothing; the item goes to the
///   captain with the verdict in its first line.
/// - Ack/Blocked side effects (tasks, handover) arrive later in slice 2 and
///   must return BadFormat until then rather than silently storing.
pub fn send(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    args: &SendArgs,
    now: Ts,
) -> GearResult<SendResult> {
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
        // Ack/blocked still belong to later tasks and a reply is written by
        // `gear reply`.
        match args.kind {
            MsgKind::Progress | MsgKind::Question | MsgKind::Instruction => {}
            MsgKind::Result => {
                if args.task.is_none()
                    || args.attempt.is_none()
                    || args.contract.is_none()
                    || args.candidate.is_none()
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
            other => {
                return Err(GearError::new(
                    ErrorCode::BadFormat,
                    format!("kind {} is not available yet", other.as_str()),
                ))
            }
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
        // A verdict is an action item for the captain: it is not addressed to
        // a seat and it does not continue another thread.
        if args.kind == MsgKind::Review && (args.to.is_some() || args.reply_to.is_some()) {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                "a review goes to the captain: no `--to`, no `--reply-to`",
            ));
        }

        // Recipient: the sender of the answered message, a seat the captain
        // addresses, or the project captain.
        let (recipient, inherited_thread) = if let Some(reply_to) = &args.reply_to {
            let original = tx.get_message(&ctx.project, reply_to)?.ok_or_else(|| {
                GearError::new(
                    ErrorCode::NoTarget,
                    format!("no message {reply_to} in project {}", ctx.project),
                )
            })?;
            (original.sender, Some(original.thread))
        } else if let Some(to) = &args.to {
            if seat.role != SeatRole::Captain {
                return Err(GearError::new(
                    ErrorCode::Forbidden,
                    "only the captain may address a seat directly",
                ));
            }
            let seat_id = format!("{}:{}", ctx.project, to);
            tx.get_seat(&seat_id)?.ok_or_else(|| {
                GearError::new(ErrorCode::NoTarget, format!("no seat {seat_id} in this project"))
            })?;
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
            MsgKind::Progress => MsgState::Info,
            _ => MsgState::Open,
        };

        // The review the verdict belongs to (a review names its task, attempt
        // and verdict; the body holds the findings). It is built inside the
        // request closure, after the replay check: a repeated request id
        // replays the stored result and never touches the reviewer's file
        // again (T-21, review F2).
        let args_key = format!(
            "{}|{}:{}|send|kind={}|body={:?}|task={:?}|attempt={:?}|contract={:?}|candidate={:?}|verdict={:?}|about={:?}|to={:?}|reply_to={:?}|urgent={}",
            ctx.project,
            ctx.seat,
            ctx.generation,
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
            args.urgent
        );

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
                    task: args.task.clone().unwrap_or_default(),
                    attempt: args.attempt.unwrap_or_default(),
                    outcome: verdict,
                    findings: if seat.role == SeatRole::Temp {
                        crate::review::findings_text(&args.body)?
                    } else {
                        args.body.clone()
                    },
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
            if args.kind == MsgKind::Result {
                crate::tasks::submit_result(
                    tx,
                    &ctx.project,
                    args.task.as_deref().ok_or_else(|| {
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
                )?;
            }

            // A review stores the verdict on the attempt's review record
            // before the item reaches the captain (T-2). It settles nothing:
            // the task and the attempt keep their state.
            if let Some(review) = &review {
                crate::review::submit_review(tx, review, now)?;
            }

            // The thread of a new message is numbered here, after the replay
            // check: a replay returns the stored result and numbers nothing.
            let thread = match &inherited_thread {
                Some(thread) => thread.clone(),
                None => {
                    let prefix = args.task.clone().unwrap_or_else(|| ctx.seat.clone());
                    let seq = next_thread_seq(tx, &ctx.project, &recipient, now, &prefix)?;
                    format!("{prefix}:{seq}")
                }
            };
            let message = NewMessage {
                project: ctx.project.clone(),
                thread: thread.clone(),
                task: args.task.clone(),
                attempt: args.attempt,
                sender: seat.id.clone(),
                sender_gen: ctx.generation,
                recipient: recipient.clone(),
                kind: args.kind,
                urgent: args.urgent,
                body: body.clone(),
                reply_to: args.reply_to.clone(),
                needs_ack: false,
                state,
                created: now,
            };
            let stored = tx.insert_message(&message)?;
            Ok(serde_json::json!({
                "msg_id": stored.id,
                "thread": stored.thread,
                "recipient": stored.recipient,
            }))
        })?;

        let (value, replayed) = match outcome {
            RequestOutcome::Executed(value) => (value, false),
            RequestOutcome::Replayed(value) => (value, true),
        };
        let msg_id = value["msg_id"].as_str().unwrap_or_default().to_string();
        let thread = value["thread"].as_str().unwrap_or_default().to_string();
        let recipient = value["recipient"].as_str().unwrap_or_default().to_string();
        Ok(SendResult { msg_id, thread, recipient, replayed })
    })
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
        // The latest reply in the thread is context, not an unhandled item.
        let last_reply = tx
            .thread_messages(project, thread)?
            .into_iter()
            .filter(|m| m.kind == MsgKind::Reply)
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

/// `gear reply` (captain only). Each id must be in this project, addressed to
/// the captain, Open or Waiting. Resolve → Resolved; Wait → Waiting until
/// wait_until (required; wait_on is an optional label); Note → state unchanged.
/// Writes one Reply (state Info) per (original sender, thread) into that
/// sender's inbox. Never changes task state.
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
                if message.state != MsgState::Open && message.state != MsgState::Waiting {
                    return Err(GearError::new(
                        ErrorCode::BadTransition,
                        format!("message {id} is {}, it cannot be handled again", message.state.as_str()),
                    ));
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
                };
                if new_state != original.state {
                    gear_model::state::require_transition(
                        MsgState::can_transition(original.state, new_state),
                        "reply",
                        original.state.as_str(),
                        new_state.as_str(),
                    )?;
                }
                if args.mode != ReplyMode::Note {
                    // A note changes nothing but the answer itself.
                    tx.update_message_state(
                        &ctx.project,
                        &original.id,
                        new_state,
                        Some(&args.text),
                        args.wait_on.as_deref(),
                        args.wait_until,
                        now,
                    )?;
                }
                handled.push((original.id.clone(), new_state.as_str().to_string()));

                // One answer per thread of the sender, in the thread of the message.
                let key = (original.sender.clone(), original.thread.clone());
                if !answered.contains(&key) {
                    answered.push(key);
                    let answer = NewMessage {
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
