//! Remind quiet lanes and interrupt a stuck working turn.
use crate::observe::{Presence, SeatObs};
use gear_herdr::{AgentStatus, HerdrAct, Outcome};
use gear_model::{AttemptState, GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{AttemptRow, NewMessage, SeatRow, Store, Tx};
use std::hash::{Hash, Hasher};

const LOOP_INTERVAL: i64 = 10 * 60;
const THINK_INTERVAL: i64 = 15 * 60;
/// How long a worktree fingerprint stands (T-521). The ask only follows
/// `THINK_INTERVAL` without a change, so one reading a minute is enough: the
/// three git commands per lane are the quiet step's steady cost on a large
/// repository, and a change seen half a minute late moves `since` by that much.
const FP_INTERVAL: i64 = 60;
fn ask_key(seat: &str) -> String {
    format!("engine.quiet.{seat}")
}
fn loop_key(seat: &str) -> String {
    format!("engine.quiet.loop.{seat}")
}
fn think_key(seat: &str) -> String {
    format!("engine.quiet.think.{seat}")
}
pub(crate) fn compact_key(seat: &str) -> String {
    format!("engine.quiet.compact.{seat}")
}

fn eligible(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Option<AttemptRow>> {
    if seat.role != SeatRole::Lane
        || !matches!(
            seat.state,
            SeatState::Active | SeatState::Standby | SeatState::Retiring
        )
        || tx
            .seats_in_backoff(&seat.project)?
            .iter()
            .any(|s| s == &seat.id)
        || crate::waits::waiting_seat(tx, &seat.id)?
    {
        return Ok(None);
    }
    Ok(tx
        .in_progress_attempts_of_seat(&seat.id)?
        .into_iter()
        .find(|a| a.state == AttemptState::Started))
}

// A loop can also occur while an assigned delivery or a stopped attempt is
// being processed, before a Started check-in has been recorded.
fn loop_attempt(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Option<AttemptRow>> {
    if seat.role != SeatRole::Lane
        || !matches!(
            seat.state,
            SeatState::Active | SeatState::Standby | SeatState::Retiring
        )
        || tx
            .seats_in_backoff(&seat.project)?
            .iter()
            .any(|s| s == &seat.id)
        || crate::waits::waiting_seat(tx, &seat.id)?
    {
        return Ok(None);
    }
    Ok(tx
        .in_progress_attempts_of_seat(&seat.id)?
        .into_iter()
        .next())
}

fn last_push(
    tx: &Tx<'_>,
    cfg: &crate::EngineCfg,
    tips: &mut crate::review::RepoTips,
    seat: &SeatRow,
    attempt: &AttemptRow,
) -> GearResult<(String, Ts)> {
    // The branch is the project's own (T-253, T-602): the branch the task
    // itself names when it names one (a member of a feature pushes there),
    // else the template it names the task branch with, `{lane}/t{n}` when it
    // says nothing.
    let conventions = crate::dispatch::conventions(tx, &seat.project);
    let branch =
        gear_app::base::task_branch_of(tx, &conventions, &seat.project, &seat.name, &attempt.task)?;
    // The project's own remote (T-262): the push clock reads the ref its
    // `[conventions].remote` names.
    let remote = &conventions.remote;
    let start = tx
        .attempt_times(&seat.project, &attempt.task, attempt.n)?
        .started
        .unwrap_or(attempt.started);
    let task = tx.get_task(&seat.project, &attempt.task)?;
    let project = tx.get_project(&seat.project)?;
    let repo = task.and_then(|t| t.repo).or_else(|| {
        project
            .and_then(|p| gear_app::config::parse(&p.config_toml).ok())
            .map(|c| c.repo_of_lane(&seat.name).path)
    });
    let pushed = repo.and_then(|repo| {
        // T-386: the branch's tip comes from one `for-each-ref` per repository
        // and tick, not from a `git log` per lane.
        crate::review::repo_tips(tips, cfg, &repo, remote)
            .get(&branch)
            .map(|(_, at)| *at)
    });
    Ok((branch, pushed.unwrap_or(start).max(start)))
}

/// The time the lane was asked why it has not pushed, while its answer is due.
fn reply_key(seat: &str) -> String {
    format!("engine.quiet.reply.{seat}")
}
/// Where the no-push clock restarts after a question was settled: the end of
/// the answer window, so a lane that explained itself gets another full
/// interval before it is asked again.
fn base_key(seat: &str) -> String {
    format!("engine.quiet.base.{seat}")
}

/// Ask a lane that has not pushed for `minutes` to tell the captain why,
/// within `reply` minutes.
fn notify(
    tx: &Tx<'_>,
    seat: &SeatRow,
    attempt: &AttemptRow,
    branch: &str,
    minutes: i64,
    reply: i64,
    now: Ts,
) -> GearResult<()> {
    let body = format!(
        "No push to {branch} for {minutes} min. Within {reply} min, tell the captain why: gear send --kind question --task {task} --attempt {n} \"no push for {minutes} min: <the reason: the task is complex because ..., or the packet is unclear: ..., or ...>\". Then commit and push what builds.",
        task = attempt.task,
        n = attempt.n,
    );
    tx.insert_message(&NewMessage {
        project: seat.project.clone(),
        thread: format!("quiet-lane:{}", seat.id),
        task: Some(attempt.task.clone()),
        attempt: Some(attempt.n),
        sender: "engine".into(),
        sender_gen: 0,
        recipient: seat.id.clone(),
        kind: MsgKind::Instruction,
        urgent: true,
        body,
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
        meta: None,
    })?;
    tx.set_meta(&reply_key(&seat.id), &now.to_string())?;
    tx.set_meta(&ask_key(&seat.id), &now.to_string())
}

/// Tell the captain that a lane neither pushed nor explained why in time.
fn alarm(
    tx: &Tx<'_>,
    seat: &SeatRow,
    attempt: &AttemptRow,
    branch: &str,
    minutes: i64,
    reply: i64,
    now: Ts,
) -> GearResult<()> {
    let Some(captain) = tx.captain_of(&seat.project)? else {
        return Ok(());
    };
    let body = format!(
        "lane {}: task {}: no push to {branch} for {minutes} min, and no word to the captain within {reply} min of being asked why. This lane is in trouble: look at its pane and act (answer its question, cut the task, or restart it).",
        seat.name, attempt.task
    );
    tx.insert_message(&NewMessage {
        project: seat.project.clone(),
        thread: format!("quiet:{}", seat.id),
        task: Some(attempt.task.clone()),
        attempt: Some(attempt.n),
        sender: "engine".into(),
        sender_gen: 0,
        recipient: captain.id,
        kind: MsgKind::System,
        urgent: true,
        body,
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
        meta: None,
    })?;
    Ok(())
}

/// The worktree's fingerprint: its HEAD, its dirty set and its diff. A git
/// that fails says nothing about the lane, so the seat is skipped this tick.
fn think_fp(path: &str) -> Option<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for args in [
        ["rev-parse", "HEAD"],
        ["status", "--porcelain"],
        ["diff", "HEAD"],
    ] {
        let output = std::process::Command::new("git")
            .args(["-C", path])
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        output.stdout.hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// The lane's session file is still growing, as fn loops reads it.
fn session_growing(tx: &Tx<'_>, seat: &SeatRow, now: Ts) -> GearResult<bool> {
    let Some(path) = tx
        .get_binding(&seat.id, seat.generation)?
        .and_then(|b| b.session_ref)
        .filter(|s| !s.is_empty())
    else {
        return Ok(false);
    };
    Ok(std::fs::metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as Ts)
        .is_some_and(|t| now.saturating_sub(t) < LOOP_INTERVAL))
}

fn think_notify(
    tx: &Tx<'_>,
    seat: &SeatRow,
    attempt: &AttemptRow,
    minutes: i64,
    now: Ts,
) -> GearResult<()> {
    let body = format!("Your worktree has not changed for {minutes} min while you work on {}. If a design question holds you, send gear send --kind question now with your default and go on with that default; otherwise write the code and push.", attempt.task);
    tx.insert_message(&NewMessage {
        project: seat.project.clone(),
        thread: format!("think-lane:{}", seat.id),
        task: Some(attempt.task.clone()),
        attempt: Some(attempt.n),
        sender: "engine".into(),
        sender_gen: 0,
        recipient: seat.id.clone(),
        kind: MsgKind::Instruction,
        urgent: true,
        body,
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
        meta: None,
    })?;
    if let Some(captain) = tx.captain_of(&seat.project)? {
        tx.insert_message(&NewMessage {
            project: seat.project.clone(),
            thread: format!("think:{}", seat.id),
            task: Some(attempt.task.clone()),
            attempt: Some(attempt.n),
            sender: "engine".into(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: false,
            body: format!(
                "lane {}: {}: working for {minutes} min with no change in its worktree",
                seat.name, attempt.task
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Info,
            created: now,
            meta: None,
        })?;
    }
    Ok(())
}

/// A 15-minute think clock (M4.1): a working lane whose session file grows
/// while its worktree does not change is asked to stop thinking. The
/// fingerprint and its clock are persisted per seat across engine restarts.
/// How long the whole fleet (captain, lanes, reviewers, no merge running)
/// sits idle in an unfinished voyage before GEAR reminds the captain, and how
/// long before it reminds it again.
const CAPTAIN_IDLE: i64 = 5 * 60;
const CAPTAIN_AGAIN: i64 = 10 * 60;

fn captain_idle_key(seat: &str) -> String {
    format!("engine.quiet.captain_idle.{seat}")
}
fn captain_told_key(seat: &str) -> String {
    format!("engine.quiet.captain_told.{seat}")
}

/// The land order the idle-voyage item was last rung for (T-634): the item is
/// not repeated while that order stays open.
fn sailing_land_key(project: &str, milestone: &str) -> String {
    format!("sailing-land:{project}:{milestone}")
}

/// Whether the voyage has landed: its landing order was answered `land`.
pub(crate) fn voyage_landed(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<bool> {
    let Some(id) = tx.get_meta(&format!("station:{project}:{milestone}:land"))? else {
        return Ok(false);
    };
    Ok(tx.get_message(project, &id)?.is_some_and(|message| {
        message.state == MsgState::Resolved
            && message.result.as_deref().is_some_and(|result| {
                result.split_once(':').is_some_and(|(_, action)| {
                    let action = action.trim();
                    action == "land" || action.starts_with("land ")
                })
            })
    }))
}

/// Whether the voyage's landing order is open: every task is done and the
/// captain has not landed it yet.
fn landing_open(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<bool> {
    let Some(id) = tx.get_meta(&format!("station:{project}:{milestone}:land"))? else {
        return Ok(false);
    };
    Ok(tx
        .get_message(project, &id)?
        .is_some_and(|message| message.state == MsgState::Open))
}

/// A voyage never stops on its own (the user, 2026-11-05: Carthage's captain
/// ended its turn at station S1 to wait for real sign-ins and test money from
/// the user, and nothing reminded it to go on). When the captain's agent has
/// sat idle for `CAPTAIN_IDLE` while its voyage still has open tasks, one item
/// tells it to go on; again after `CAPTAIN_AGAIN` if it is still idle.
pub fn captain_sailing(
    store: &mut Store,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
) -> GearResult<()> {
    // The user, 2026-11-05: an idle captain alone is no problem while its
    // lanes work; the captain AND every lane and reviewer idle is.
    let crew_busy = observed.iter().any(|o| {
        o.seat.project == project
            && o.seat.role != SeatRole::Captain
            && o.presence == Presence::Present
            && !o.idle
    });
    let merging = store.read(|tx| {
        Ok(!tx
            .ops_of_kind_in_states(
                project,
                gear_model::OpKind::Merge,
                &[gear_model::OpState::Executing],
            )?
            .is_empty())
    })?;
    for obs in observed.iter().filter(|o| {
        o.seat.project == project
            && o.seat.role == SeatRole::Captain
            && o.presence == Presence::Present
    }) {
        let seat = obs.seat.id.clone();
        store.write(|tx| {
            if !obs.idle || crew_busy || merging {
                tx.delete_meta(&captain_idle_key(&seat))?;
                return Ok(());
            }
            let since = match tx
                .get_meta(&captain_idle_key(&seat))?
                .and_then(|v| v.parse::<Ts>().ok())
            {
                Some(since) => since,
                None => {
                    tx.set_meta(&captain_idle_key(&seat), &now.to_string())?;
                    return Ok(());
                }
            };
            if now - since < CAPTAIN_IDLE {
                return Ok(());
            }
            // The user, 2026-11-05: a voyage that is not finished, by however
            // little, never lets its captain stop, unless it is paused. A
            // voyage is finished when it has landed.
            if tx.project_paused(project)?.is_some() {
                return Ok(());
            }
            let milestone = gear_app::plan::current_voyage(tx, project)?;
            if milestone.is_empty() || voyage_landed(tx, project, &milestone)? {
                return Ok(());
            }
            let left = tx
                .tasks_live_of_project(project)?
                .into_iter()
                .filter(|task| {
                    task.milestone.as_deref() == Some(milestone.as_str())
                        && !task.state.is_terminal()
                })
                .count();
            // Every task done: unfinished while its landing order waits for
            // an answer. A voyage with neither (an old one, landed before the
            // landing order existed) is finished and rings nobody (T-571).
            if left == 0 && !landing_open(tx, project, &milestone)? {
                return Ok(());
            }
            // T-634: every task done and a land order open. The item rings once
            // per open land order: while that order stays open it is not
            // repeated, however long the captain waits for the user.
            if left == 0 {
                let Some(order) = tx.get_meta(&format!("station:{project}:{milestone}:land"))?
                else {
                    return Ok(());
                };
                let key = sailing_land_key(project, &milestone);
                if tx.get_meta(&key)?.as_deref() == Some(order.as_str()) {
                    return Ok(());
                }
                tx.insert_message(&NewMessage {
                    meta: None,
                    project: project.into(),
                    thread: format!("sailing:{milestone}"),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: seat.clone(),
                    kind: MsgKind::System,
                    urgent: true,
                    body: format!(
                        "Voyage {milestone}: every task is done and its land order {order} is open. Land it (`gear` shows the order); if the land is refused for a decision only the user can make, tell the user and wait: this item is not repeated while that order stays open."
                    ),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: now,
                })?;
                tx.set_meta(&key, &order)?;
                return Ok(());
            }
            if tx
                .get_meta(&captain_told_key(&seat))?
                .and_then(|v| v.parse::<Ts>().ok())
                .is_some_and(|told| now - told < CAPTAIN_AGAIN)
            {
                return Ok(());
            }
            let thread = format!("sailing:{milestone}");
            if tx
                .thread_messages(project, &thread)?
                .iter()
                .any(|m| m.state == MsgState::Open)
            {
                return Ok(());
            }
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.into(),
                thread,
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: seat.clone(),
                kind: MsgKind::System,
                urgent: true,
                body: format!(
                    "Voyage {milestone} is not finished ({}), and you and every lane and reviewer have been idle for {} min. Nothing lets a voyage stop before it lands: GEAR asks you to finish the voyage, and nothing in it waits for the user to provide anything. A station is a mark, never a stop; what only the user can test is listed for the landing. The only reasons to stop are force majeure: the machine or GEAR is down, your own tokens are gone, or every lane is out of tokens or cannot start. Run `gear`, put every lane to work (or land the voyage when its goal is met), and resolve this item.",
                    if left == 0 { "every task done, not landed".to_string() } else { format!("{left} tasks left") },
                    (now - since) / 60
                ),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })?;
            tx.set_meta(&captain_told_key(&seat), &now.to_string())?;
            Ok(())
        })?;
    }
    Ok(())
}

pub fn thinks(store: &mut Store, project: &str, observed: &[SeatObs], now: Ts) -> GearResult<()> {
    for obs in observed
        .iter()
        .filter(|o| o.presence == Presence::Present && o.status == Some(AgentStatus::Working))
    {
        let candidate = store.read(|tx| {
            let Some(seat) = tx.get_seat(&obs.seat.id)? else {
                return Ok(None);
            };
            if seat.project != project || seat.generation != obs.seat.generation {
                return Ok(None);
            }
            let Some(attempt) = eligible(tx, &seat)? else {
                return Ok(None);
            };
            let Some(path) = tx
                .get_meta(&crate::onboard::worktree_path_key(&seat.id))?
                .filter(|p| !p.is_empty())
            else {
                return Ok(None);
            };
            let meta = tx
                .get_meta(&think_key(&seat.id))?
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
            Ok(Some((seat, attempt, path, meta)))
        })?;
        let Some((seat, attempt, path, meta)) = candidate else {
            continue;
        };
        // The last reading stands for a minute (T-521): same task, same
        // attempt, read less than `FP_INTERVAL` ago. Nothing moves in the
        // meantime, so the stored state is the state this tick would write.
        let current = meta.as_ref().filter(|m| {
            m["task"].as_str() == Some(attempt.task.as_str())
                && m["n"].as_i64() == Some(attempt.n)
                && m["fp_at"]
                    .as_i64()
                    .is_some_and(|at| now.saturating_sub(at) < FP_INTERVAL)
        });
        if current.is_some() {
            continue;
        }
        let Some(fp) = think_fp(&path) else { continue };
        store.write(|tx| {
            let live = meta.as_ref().filter(|m| m["task"].as_str() == Some(attempt.task.as_str())
                && m["n"].as_i64() == Some(attempt.n) && m["fp"].as_u64() == Some(fp));
            let since = live.and_then(|m| m["since"].as_i64()).unwrap_or(now);
            let mut asked = live.and_then(|m| m["asked"].as_i64());
            if now.saturating_sub(since) >= THINK_INTERVAL && asked.is_none_or(|a| now.saturating_sub(a) >= THINK_INTERVAL)
                && session_growing(tx, &seat, now)? {
                think_notify(tx, &seat, &attempt, now.saturating_sub(since) / 60, now)?;
                asked = Some(now);
            }
            tx.set_meta(&think_key(&seat.id), &serde_json::json!({"task":attempt.task,"n":attempt.n,"fp":fp,"fp_at":now,"since":since,"asked":asked}).to_string())
        })?;
    }
    Ok(())
}

/// T-610: an engine nudge to a lane closes itself. A lane may not resolve an
/// engine item and the captain may not either, so the engine resolves its own:
/// the no-push nudge when the attempt it names has been handed in, has a newer
/// attempt, or its task is terminal, and the captain's "no word" item when the
/// lane's hand-in answered it. A nudge for a still-open attempt stays.
pub fn resolve_nudges(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<()> {
    for message in tx.messages_in_threads_starting(project, "quiet-lane:")? {
        if !matches!(message.state, MsgState::Open | MsgState::Waiting) {
            continue;
        }
        let (Some(task), Some(attempt)) = (message.task.as_deref(), message.attempt) else {
            continue;
        };
        let Some(reason) = attempt_closed(tx, project, task, attempt)? else {
            continue;
        };
        tx.update_message_state(
            project,
            &message.id,
            MsgState::Resolved,
            Some(&reason),
            None,
            None,
            now,
        )?;
    }
    for message in tx.messages_in_threads_starting(project, "quiet:")? {
        if !matches!(message.state, MsgState::Open | MsgState::Waiting) {
            continue;
        }
        let (Some(task), Some(attempt)) = (message.task.as_deref(), message.attempt) else {
            continue;
        };
        let Some(lane) = message.thread.strip_prefix("quiet:") else {
            continue;
        };
        // The lane's hand-in answered it: a result from the lane after the
        // item was opened.
        let answered = tx
            .messages_from_since(project, lane, message.created)?
            .into_iter()
            .any(|m| {
                m.kind == MsgKind::Result
                    && m.task.as_deref() == Some(task)
                    && m.attempt == Some(attempt)
            });
        if !answered {
            continue;
        }
        tx.update_message_state(
            project,
            &message.id,
            MsgState::Resolved,
            Some("the lane handed in: its result answered the question"),
            None,
            None,
            now,
        )?;
    }
    Ok(())
}

/// The reason an attempt is closed, if it is: handed in (Submitted, Accepted,
/// Rejected), a newer attempt exists, or its task is terminal.
pub(crate) fn attempt_closed(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
) -> GearResult<Option<String>> {
    if let Some(row) = tx.get_task(project, task)? {
        if row.state.is_terminal() {
            return Ok(Some(format!(
                "{task} is {}: the nudge is moot",
                row.state.as_str()
            )));
        }
    }
    if tx
        .attempts_of_task(project, task)?
        .iter()
        .any(|a| a.n > attempt)
    {
        return Ok(Some(format!(
            "{task} has a newer attempt: the nudge is moot"
        )));
    }
    if let Some(row) = tx.get_attempt(project, task, attempt)? {
        if matches!(
            row.state,
            AttemptState::Submitted | AttemptState::Accepted | AttemptState::Rejected
        ) {
            return Ok(Some(format!(
                "{task} attempt {attempt} was handed in: the nudge is moot"
            )));
        }
    }
    Ok(None)
}

/// The no-push clock, persisted per seat across engine restarts: a lane that
/// has not pushed for `[conventions] push_interval_min` (20) is asked to tell
/// the captain why within `push_reply_min` (10). Its answer goes to the
/// captain as an item to read, and the clock restarts at the end of the
/// window; no answer in time is an urgent item to the captain.
pub fn tick(
    tx: &Tx<'_>,
    cfg: &crate::EngineCfg,
    tips: &mut crate::review::RepoTips,
    project: &str,
    now: Ts,
) -> GearResult<()> {
    let conventions = crate::dispatch::conventions(tx, project);
    let interval = i64::from(conventions.push_interval_min.max(1)) * 60;
    let reply = i64::from(conventions.push_reply_min.max(1)) * 60;
    let captain = tx.captain_of(project)?.map(|seat| seat.id);
    resolve_nudges(tx, project, now)?;
    for seat in tx.seats_of_project(project)? {
        let Some(attempt) = eligible(tx, &seat)? else {
            // A hold is not the lane's own time: while a lane seat is not
            // eligible the no-push clock stands still, so a released lane is
            // asked from the end of the hold, not all at once (T-417).
            if seat.role == SeatRole::Lane {
                tx.set_meta(&base_key(&seat.id), &now.to_string())?;
                tx.set_meta(&reply_key(&seat.id), "")?;
            }
            continue;
        };
        let (branch, push) = last_push(tx, cfg, tips, &seat, &attempt)?;
        let base = tx
            .get_meta(&base_key(&seat.id))?
            .and_then(|s| s.parse::<Ts>().ok())
            .unwrap_or(0);
        let asked = tx
            .get_meta(&reply_key(&seat.id))?
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<Ts>().ok());
        if let Some(asked) = asked {
            let settle = |tx: &Tx<'_>| -> GearResult<()> {
                tx.set_meta(&reply_key(&seat.id), "")?;
                tx.set_meta(&base_key(&seat.id), &(asked + reply).to_string())
            };
            if push > asked {
                settle(tx)?;
                continue;
            }
            let answers: Vec<_> = tx
                .messages_from_since(project, &seat.id, asked)?
                .into_iter()
                .filter(|m| captain.as_deref() == Some(m.recipient.as_str()))
                .collect();
            if !answers.is_empty() {
                // The answer is the captain's to read, whatever kind it was sent as.
                for answer in answers.iter().filter(|m| m.state == MsgState::Info) {
                    tx.update_message_state(
                        project,
                        &answer.id,
                        MsgState::Open,
                        None,
                        None,
                        None,
                        now,
                    )?;
                }
                settle(tx)?;
                continue;
            }
            // The answer window opens when the ask reached the lane: a notice
            // to a working lane waits for its next idle moment (T-454), and a
            // lane cannot answer an ask it has not seen.
            let reached = tx
                .ops_of_seat_kind(&seat.id, gear_model::OpKind::Doorbell)?
                .into_iter()
                .filter(|op| op.state == gear_model::OpState::Confirmed && op.created >= asked)
                .map(|op| op.updated)
                .min();
            if reached.is_some_and(|reached| now - reached >= reply) {
                alarm(
                    tx,
                    &seat,
                    &attempt,
                    &branch,
                    now.saturating_sub(push) / 60,
                    reply / 60,
                    now,
                )?;
                settle(tx)?;
            }
            continue;
        }
        if now.saturating_sub(push.max(base)) > interval {
            notify(
                tx,
                &seat,
                &attempt,
                &branch,
                now.saturating_sub(push) / 60,
                reply / 60,
                now,
            )?;
        }
    }
    Ok(())
}

/// Reserve the interruption before using Herdr: a crash or uncertain response
/// cannot send repeated escapes. Unreadable session files are not evidence.
pub fn loops(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &crate::EngineCfg,
    tips: &mut crate::review::RepoTips,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
) -> GearResult<()> {
    for obs in observed
        .iter()
        .filter(|o| o.presence == Presence::Present && o.status == Some(AgentStatus::Working))
    {
        let candidate = store.read(|tx| {
            let Some(seat) = tx.get_seat(&obs.seat.id)? else {
                return Ok(None);
            };
            if seat.project != project || seat.generation != obs.seat.generation {
                return Ok(None);
            }
            let Some(attempt) = loop_attempt(tx, &seat)? else {
                return Ok(None);
            };
            let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                return Ok(None);
            };
            let Some(path) = binding.session_ref.as_deref().filter(|s| !s.is_empty()) else {
                return Ok(None);
            };
            let modified = std::fs::metadata(path)
                .ok()
                .filter(|m| m.is_file())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as Ts);
            if modified.is_none_or(|t| now.saturating_sub(t) < LOOP_INTERVAL) {
                return Ok(None);
            }
            let last = tx
                .get_meta(&loop_key(&seat.id))?
                .and_then(|s| s.parse::<Ts>().ok());
            if last.is_some_and(|t| now.saturating_sub(t) < LOOP_INTERVAL) {
                return Ok(None);
            }
            let (branch, pushed) = last_push(tx, cfg, tips, &seat, &attempt)?;
            Ok(Some((
                seat,
                attempt,
                binding,
                branch,
                now.saturating_sub(pushed) / 60,
            )))
        })?;
        let Some((seat, attempt, binding, branch, minutes)) = candidate else {
            continue;
        };
        // Confirm the current occupant immediately before the escape.
        let current = herdr.agent_get(&binding.pane_id).ok().flatten();
        if !current.is_some_and(|a| {
            a.agent.as_deref() == Some(binding.kind.as_str())
                && a.agent_status == AgentStatus::Working
                && a.agent_session
                    .as_ref()
                    .is_none_or(|s| Some(s.value.as_str()) == binding.session_ref.as_deref())
        }) {
            continue;
        }
        let reserved = store.write(|tx| {
            let last = tx
                .get_meta(&loop_key(&seat.id))?
                .and_then(|s| s.parse::<Ts>().ok());
            if last.is_some_and(|t| now.saturating_sub(t) < LOOP_INTERVAL) {
                return Ok(None);
            }
            let Some(current) = tx.get_seat(&seat.id)? else {
                return Ok(None);
            };
            let same_attempt = loop_attempt(tx, &current)?
                .is_some_and(|a| a.task == attempt.task && a.n == attempt.n);
            let same_binding = tx
                .get_binding(&seat.id, current.generation)?
                .is_some_and(|b| {
                    b.generation == binding.generation
                        && b.pane_id == binding.pane_id
                        && b.session_ref == binding.session_ref
                });
            if !same_attempt || !same_binding {
                return Ok(None);
            }
            let already_compacting = tx
                .get_meta(&compact_key(&seat.id))?
                .is_some_and(|v| !v.is_empty());
            tx.set_meta(&loop_key(&seat.id), &now.to_string())?;
            if !already_compacting {
                tx.set_meta(&compact_key(&seat.id), "pending")?;
            }
            Ok(Some(already_compacting))
        })?;
        let Some(already_compacting) = reserved else {
            continue;
        };
        if matches!(
            herdr.send_keys(&binding.pane_id, &["esc".into()]),
            Outcome::Refused { .. }
        ) {
            if !already_compacting {
                store.write(|tx| tx.set_meta(&compact_key(&seat.id), ""))?;
            }
            continue;
        }
        store.write(|tx| {
            let reply = i64::from(
                crate::dispatch::conventions(tx, project)
                    .push_reply_min
                    .max(1),
            );
            notify(tx, &seat, &attempt, &branch, minutes, reply, now)
        })?;
    }
    thinks(store, project, observed, now)
}

/// A delivery can have been intended before the loop was detected. Drive its
/// compaction too; execution holds the op until the marker is cleared.
pub fn prepare_pending_deliveries(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &crate::EngineCfg,
    project: &str,
    now: Ts,
) -> GearResult<()> {
    let pending = store.read(|tx| {
        let mut work = Vec::new();
        for op in tx.ops_in_state(project, gear_model::OpState::Intended)? {
            if op.kind != gear_model::OpKind::Deliver
                || !tx
                    .get_meta(&compact_key(&op.seat))?
                    .is_some_and(|s| !s.is_empty())
            {
                continue;
            }
            let (Some(seat), Some(task), Some(attempt)) =
                (tx.get_seat(&op.seat)?, op.task, op.attempt)
            else {
                continue;
            };
            // Only the delivery's own task in a declared wait holds it: a
            // seat waiting on one task still takes the next task it was given.
            let active = tx
                .get_attempt(project, &task, attempt)?
                .is_some_and(|a| !a.state.is_terminal())
                && tx
                    .get_task(project, &task)?
                    .is_some_and(|t| !t.state.is_terminal() && t.wait_on.is_none());
            if active && seat.generation == op.generation && seat.role == SeatRole::Lane {
                work.push((seat, task, attempt));
            }
        }
        Ok(work)
    })?;
    for (seat, task, attempt) in pending {
        crate::session::prepare_lane(store, herdr, cfg, &seat, &task, attempt, now)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_model::TaskState;
    use gear_store::{testing, AttemptStep, ContractRow, TaskRow};
    use std::time::{Duration, UNIX_EPOCH};
    const T: i64 = 1_800_000_000;
    const PANE: &str = "w1:p2";

    /// The engine's own git (T-386: the push clock reads the branch tips
    /// through it). These tests read real repositories.
    fn cfg() -> crate::EngineCfg {
        crate::EngineCfg::default()
    }

    /// One read of the branch tips for this step (a tick shares one map, T-386).
    fn tick(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<()> {
        super::tick(
            tx,
            &cfg(),
            &mut crate::review::RepoTips::new(),
            project,
            now,
        )
    }

    fn loops(
        store: &mut Store,
        herdr: &mut dyn HerdrAct,
        project: &str,
        observed: &[SeatObs],
        now: Ts,
    ) -> GearResult<()> {
        super::loops(
            store,
            herdr,
            &cfg(),
            &mut crate::review::RepoTips::new(),
            project,
            observed,
            now,
        )
    }

    fn world(repo: &str) -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T)?;
            tx.insert_task(&TaskRow {
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
                estimate_min: None,
                issue: None,
                updated: T,
                repo: Some(repo.into()),
                base: None,
                gate: None,
                rework_estimate_min: None,
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
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T)?;
            Ok(())
        })
        .unwrap();
        s
    }

    fn git(repo: &std::path::Path, args: &[&str], date: Option<&str>) {
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(repo).args(args);
        if let Some(date) = date {
            command
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn messages(s: &mut Store, thread: &str) -> Vec<gear_store::MessageRow> {
        s.read(|tx| tx.thread_messages("p", thread)).unwrap()
    }

    fn repo_with_commit() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None);
        git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
            None,
        );
        git(dir.path(), &["config", "user.name", "test"], None);
        std::fs::write(dir.path().join("file"), "x").unwrap();
        git(dir.path(), &["add", "file"], None);
        git(
            dir.path(),
            &["commit", "-qm", "one"],
            Some("@1800001200 +0000"),
        );
        dir
    }

    /// The lane's own word to the captain, as `gear send` writes it.
    fn lane_says(s: &mut Store, kind: MsgKind, state: MsgState, at: Ts) {
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-1:q".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind,
                urgent: false,
                body: "no push: the task is complex".into(),
                reply_to: None,
                needs_ack: false,
                state,
                created: at,
                meta: None,
            })
            .map(|_| ())
        })
        .unwrap();
    }

    /// The ask reached the lane: a doorbell to its seat confirmed at `at`.
    fn rang(s: &mut Store, at: Ts) {
        s.write(|tx| {
            let op = tx.insert_op(&gear_store::NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: gear_model::OpKind::Doorbell,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: at,
            })?;
            tx.set_op_state("p", &op.id, gear_model::OpState::Confirmed, None, at)
        })
        .unwrap();
    }

    /// 20 min without a push: the lane is asked once to tell the captain why
    /// within 10 min; its answer becomes the captain's item; the clock then
    /// restarts at the end of the window, so the next question comes at 50.
    #[test]
    fn a_quiet_lane_is_asked_why_and_its_answer_restarts_the_clock() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1199)).unwrap();
        assert!(
            messages(&mut s, "quiet-lane:p:heat").is_empty(),
            "not before 20 min"
        );
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        let lane = messages(&mut s, "quiet-lane:p:heat");
        assert_eq!(lane.len(), 1);
        assert!(lane[0].urgent);
        assert!(lane[0].body.starts_with("No push to heat/t1 for 20 min. Within 10 min, tell the captain why: gear send --kind question --task T-1 --attempt 1"), "{}", lane[0].body);
        assert!(
            messages(&mut s, "quiet:p:heat").is_empty(),
            "the captain hears the lane's answer, not a notice"
        );
        s.write(|tx| tick(tx, "p", T + 1260)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1, "asked once");
        // The lane answers as progress (an info kind): it becomes an item.
        lane_says(&mut s, MsgKind::Progress, MsgState::Info, T + 1500);
        s.write(|tx| tick(tx, "p", T + 1510)).unwrap();
        let items = s
            .read(|tx| tx.actionable_for("p", "p:captain", T + 1510))
            .unwrap();
        assert!(
            items
                .iter()
                .any(|m| m.body == "no push: the task is complex"),
            "{items:?}"
        );
        // Asked at 20, window to 30: the next question is after 50.
        s.write(|tx| tick(tx, "p", T + 1201 + 600 + 1200)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        s.write(|tx| tick(tx, "p", T + 1201 + 600 + 1201)).unwrap();
        assert_eq!(
            messages(&mut s, "quiet-lane:p:heat").len(),
            2,
            "asked again after 50 min"
        );
        assert!(
            messages(&mut s, "quiet:p:heat").is_empty(),
            "no alarm: the lane answered"
        );
    }

    /// No answer within the window: the captain gets an urgent item that the
    /// lane is in trouble; a push, instead, settles the question.
    #[test]
    fn a_quiet_lane_that_does_not_answer_is_reported_and_a_push_settles_it() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        // A working lane gets the ask at its next idle moment (T-454): until
        // it has, no window runs and the captain is not alarmed.
        s.write(|tx| tick(tx, "p", T + 1201 + 1800)).unwrap();
        assert!(
            messages(&mut s, "quiet:p:heat").is_empty(),
            "an ask the lane has not seen raises no alarm"
        );
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        rang(&mut s, T + 1201);
        s.write(|tx| tick(tx, "p", T + 1201 + 599)).unwrap();
        assert!(
            messages(&mut s, "quiet:p:heat").is_empty(),
            "not before the window ends"
        );
        s.write(|tx| tick(tx, "p", T + 1201 + 600)).unwrap();
        let captain = messages(&mut s, "quiet:p:heat");
        assert_eq!(captain.len(), 1);
        assert!(captain[0].urgent && captain[0].state == MsgState::Open);
        assert!(
            captain[0]
                .body
                .contains("no word to the captain within 10 min"),
            "{}",
            captain[0].body
        );
        s.write(|tx| tick(tx, "p", T + 1201 + 660)).unwrap();
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 1, "told once");

        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        git(
            dir.path(),
            &["update-ref", "refs/remotes/origin/heat/t1", "HEAD"],
            None,
        );
        // The commit is dated T+1200 (before the question): a later push resets.
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", "two"],
            Some("@1800001400 +0000"),
        );
        git(
            dir.path(),
            &["update-ref", "refs/remotes/origin/heat/t1", "HEAD"],
            None,
        );
        s.write(|tx| tick(tx, "p", T + 1201 + 700)).unwrap();
        assert!(
            messages(&mut s, "quiet:p:heat").is_empty(),
            "a push settles the question"
        );
    }

    /// A hold is not counted: 3 h held with no push, then active, is not
    /// asked in the first tick and is asked once after the interval (T-417).
    #[test]
    fn a_hold_is_not_counted_by_the_no_push_clock() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| {
            tx.set_seat_state("p:heat", SeatState::Held, None, T)?;
            tick(tx, "p", T + 3 * 3600)
        })
        .unwrap();
        assert!(
            messages(&mut s, "quiet-lane:p:heat").is_empty(),
            "a held lane is not asked"
        );
        s.write(|tx| tx.set_seat_state("p:heat", SeatState::Active, None, T + 3 * 3600))
            .unwrap();
        s.write(|tx| tick(tx, "p", T + 3 * 3600 + 1199)).unwrap();
        assert!(
            messages(&mut s, "quiet-lane:p:heat").is_empty(),
            "the clock runs from the end of the hold"
        );
        s.write(|tx| tick(tx, "p", T + 3 * 3600 + 1201)).unwrap();
        assert_eq!(
            messages(&mut s, "quiet-lane:p:heat").len(),
            1,
            "asked once after the interval"
        );
    }

    /// A wait on something outside GEAR (T-440): the no-push question stays
    /// off the lane while the wait lasts, and comes back when it ends.
    #[test]
    fn an_external_wait_keeps_the_no_push_question_off_the_lane() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tx.set_task_wait("p", "T-1", "external", "external"))
            .unwrap();
        s.write(|tx| tick(tx, "p", T + 3 * 3600)).unwrap();
        assert!(
            messages(&mut s, "quiet-lane:p:heat").is_empty(),
            "a declared external wait is not a quiet lane"
        );
        assert!(
            messages(&mut s, "quiet:p:heat").is_empty(),
            "and the captain is not alarmed"
        );
        // The wait is over: the clock runs from the end of the wait, not from
        // the start of it.
        s.write(|tx| {
            tx.clear_task_wait("p", "T-1")?;
            tick(tx, "p", T + 3 * 3600)
        })
        .unwrap();
        s.write(|tx| tick(tx, "p", T + 3 * 3600 + 1199)).unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty());
        s.write(|tx| tick(tx, "p", T + 3 * 3600 + 1201)).unwrap();
        assert_eq!(
            messages(&mut s, "quiet-lane:p:heat").len(),
            1,
            "asked once the wait is over"
        );
    }

    /// The two times are the project's: 15 and 5 here.
    #[test]
    fn the_push_clock_follows_the_project_settings() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| {
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\n[quota]\nlanes = 1\ntemps = 0\n\n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\nkind = \"pi\"\nprofile = \"pi\"\n\n[conventions]\ngate = \"true\"\npush_interval_min = 15\npush_reply_min = 5\n".into();
            assert_eq!(gear_app::config::parse(&project.config_toml).map(|c| c.conventions.push_interval_min).ok(), Some(15), "{}", project.config_toml);
            tx.update_project(&project)
        }).unwrap();
        s.write(|tx| tick(tx, "p", T + 901)).unwrap();
        assert_eq!(
            messages(&mut s, "quiet-lane:p:heat").len(),
            1,
            "asked after 15 min"
        );
        rang(&mut s, T + 901);
        s.write(|tx| tick(tx, "p", T + 901 + 300)).unwrap();
        assert_eq!(
            messages(&mut s, "quiet:p:heat").len(),
            1,
            "reported after 5 more"
        );
    }

    #[test]
    fn parked_held_backoff_and_unstarted_lanes_are_quiet() {
        for state in [SeatState::Parked, SeatState::Held] {
            let mut s = world("/missing");
            s.write(|tx| {
                tx.set_seat_state("p:heat", state, None, T)?;
                tick(tx, "p", T + 1300)
            })
            .unwrap();
            assert!(messages(&mut s, "quiet-lane:p:heat").is_empty());
        }
        let mut s = world("/missing");
        s.write(|tx| {
            tx.set_seats_in_backoff("p", &["p:heat".into()])?;
            tick(tx, "p", T + 1300)
        })
        .unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty());
        s.write(|tx| {
            tx.set_seats_in_backoff("p", &[])?;
            let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
            attempt.state = AttemptState::Submitted;
            tx.update_attempt(&attempt)?;
            tick(tx, "p", T + 1300)
        })
        .unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty());
        let mut s = world("/missing");
        s.write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.wait_on = Some("T-2".into());
            tx.update_task(&task)?;
            tick(tx, "p", T + 1300)
        })
        .unwrap();
        assert!(
            messages(&mut s, "quiet-lane:p:heat").is_empty(),
            "declared task waits are not stalled work"
        );
    }

    /// A captain idle for 15 minutes while its voyage has open tasks is told
    /// to go on, once per 30 minutes; a busy captain or a finished voyage is
    /// not (Carthage, 2026-11-05: its captain stopped at station S1).
    #[test]
    fn an_idle_captain_of_a_sailing_voyage_is_told_to_go_on() {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T)?;
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M1".into(),
                title: "v".into(),
                done: Some("done".into()),
                bar: Some("FASTEST".into()),
                eta: Some("4h".into()),
                departure: Some(T),
                departure_tasks: Some(1),
                departure_estimate_min: Some(30),
                updated: T,
            })?;
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
                lane: Some("hull".into()),
                milestone: Some("M1".into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                updated: T,
            })?;
            Ok(())
        })
        .unwrap();
        let captain = |s: &mut Store, idle: bool| SeatObs {
            seat: s.read(|tx| tx.get_seat("p:captain")).unwrap().unwrap(),
            presence: Presence::Present,
            status: Some(if idle {
                AgentStatus::Idle
            } else {
                AgentStatus::Working
            }),
            flags: Default::default(),
            idle,
            screen: None,
            session: None,
        };
        let items = |s: &mut Store| s.read(|tx| tx.thread_messages("p", "sailing:M1")).unwrap();
        let obs = captain(&mut s, true);
        // A lane at work: an idle captain is no problem.
        s.write(|tx| {
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "opencode", T)
        })
        .unwrap();
        let lane = |s: &mut Store, idle: bool| SeatObs {
            seat: s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap(),
            presence: Presence::Present,
            status: Some(if idle {
                AgentStatus::Idle
            } else {
                AgentStatus::Working
            }),
            flags: Default::default(),
            idle,
            screen: None,
            session: None,
        };
        let working = lane(&mut s, false);
        captain_sailing(&mut s, "p", &[obs.clone(), working.clone()], T).unwrap();
        captain_sailing(&mut s, "p", &[obs.clone(), working], T + 20 * 60).unwrap();
        assert!(items(&mut s).is_empty(), "a working lane: no reminder");
        // The whole fleet idle: told after 5 minutes.
        let still = lane(&mut s, true);
        let obs_all = |o: &SeatObs| vec![o.clone(), still.clone()];
        captain_sailing(&mut s, "p", &obs_all(&obs), T).unwrap();
        captain_sailing(&mut s, "p", &obs_all(&obs), T + 4 * 60).unwrap();
        assert!(items(&mut s).is_empty(), "not yet 5 minutes idle");
        captain_sailing(&mut s, "p", &obs_all(&obs), T + 5 * 60).unwrap();
        let told = items(&mut s);
        assert_eq!(told.len(), 1);
        assert!(
            told[0].body.contains("Nothing lets a voyage stop"),
            "{}",
            told[0].body
        );
        assert!(told[0].urgent);
        let resolve = |s: &mut Store, id: &str, at: Ts| {
            s.write(|tx| tx.update_message_state("p", id, MsgState::Resolved, None, None, None, at))
                .unwrap()
        };
        // Resolved, still idle: not again before 10 minutes.
        resolve(&mut s, &told[0].id, T + 4 * 60);
        captain_sailing(&mut s, "p", &obs_all(&obs), T + 12 * 60).unwrap();
        assert_eq!(items(&mut s).len(), 1, "once per 10 minutes");
        // Every task done and its landing order open: not finished.
        s.write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.state = TaskState::Done;
            tx.update_task(&task)?;
            let land = tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "station:M1".into(),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: MsgKind::System,
                urgent: true,
                body: "ready to land".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T,
            })?;
            tx.set_meta("station:p:M1:land", &land.id)
        })
        .unwrap();
        captain_sailing(&mut s, "p", &obs_all(&obs), T + 15 * 60).unwrap();
        let told = items(&mut s);
        assert_eq!(told.len(), 2, "a voyage not landed is not finished");
        assert!(
            told[1]
                .body
                .contains("every task is done and its land order"),
            "{}",
            told[1].body
        );
        resolve(&mut s, &told[1].id, T + 16 * 60);
        // A paused project is left alone.
        s.write(|tx| tx.set_meta(&gear_store::Tx::project_paused_key("p"), "paused"))
            .unwrap();
        captain_sailing(&mut s, "p", &obs_all(&obs), T + 30 * 60).unwrap();
        assert_eq!(items(&mut s).len(), 2, "paused");
        // A working captain resets the clock.
        let busy = captain(&mut s, false);
        captain_sailing(&mut s, "p", &[busy], T + 31 * 60).unwrap();
        assert!(s
            .read(|tx| tx.get_meta(&captain_idle_key("p:captain")))
            .unwrap()
            .is_none());
    }

    // ---- T-634: the idle-voyage item rings once per open land order ------

    /// A project with voyage M1 and one task in `task_state`; when `land`, an
    /// open land order whose id is returned.
    fn sailing_world(task_state: TaskState, land: bool) -> (Store, Option<String>) {
        let mut s = Store::open_in_memory().unwrap();
        let mut order = None;
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "opencode", T)?;
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M1".into(),
                title: "v".into(),
                done: Some("done".into()),
                bar: Some("FASTEST".into()),
                eta: Some("4h".into()),
                departure: Some(T),
                departure_tasks: Some(1),
                departure_estimate_min: Some(30),
                updated: T,
            })?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: task_state,
                priority: "must".into(),
                lane: Some("hull".into()),
                milestone: Some("M1".into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                updated: T,
            })?;
            if land {
                let message = tx.insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "station:M1".into(),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: MsgKind::System,
                    urgent: true,
                    body: "ready to land".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: T,
                })?;
                tx.set_meta("station:p:M1:land", &message.id)?;
                order = Some(message.id);
            }
            Ok(())
        })
        .unwrap();
        (s, order)
    }

    fn idle_obs(s: &mut Store, seat: &str) -> SeatObs {
        SeatObs {
            seat: s.read(|tx| tx.get_seat(seat)).unwrap().unwrap(),
            presence: Presence::Present,
            status: Some(AgentStatus::Idle),
            flags: Default::default(),
            idle: true,
            screen: None,
            session: None,
        }
    }

    fn sailing_items(s: &mut Store) -> Vec<gear_store::MessageRow> {
        s.read(|tx| tx.thread_messages("p", "sailing:M1")).unwrap()
    }

    #[test]
    fn an_idle_voyage_with_an_open_land_order_rings_once() {
        let (mut s, order) = sailing_world(TaskState::Done, true);
        let order = order.expect("a land order");
        let all = vec![idle_obs(&mut s, "p:captain"), idle_obs(&mut s, "p:heat")];
        captain_sailing(&mut s, "p", &all, T).unwrap();
        captain_sailing(&mut s, "p", &all, T + 5 * 60).unwrap();
        let items = sailing_items(&mut s);
        assert_eq!(items.len(), 1, "{items:?}");
        assert!(
            items[0]
                .body
                .contains("every task is done and its land order"),
            "{}",
            items[0].body
        );
        assert!(items[0].body.contains(&order), "{}", items[0].body);
        // Thirty more minutes idle: not repeated while that order stays open.
        captain_sailing(&mut s, "p", &all, T + 35 * 60).unwrap();
        assert_eq!(sailing_items(&mut s).len(), 1, "one item per land order");

        // A new land order id rings once more.
        let next = s
            .write(|tx| {
                let message = tx.insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "station:M1".into(),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: MsgKind::System,
                    urgent: true,
                    body: "ready to land again".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: T + 36 * 60,
                })?;
                tx.set_meta("station:p:M1:land", &message.id)?;
                Ok(message.id)
            })
            .unwrap();
        captain_sailing(&mut s, "p", &all, T + 36 * 60).unwrap();
        let items = sailing_items(&mut s);
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(items[1].body.contains(&next), "{}", items[1].body);
        captain_sailing(&mut s, "p", &all, T + 66 * 60).unwrap();
        assert_eq!(sailing_items(&mut s).len(), 2, "once for the new order");
    }

    #[test]
    fn a_voyage_with_tasks_left_repeats_after_captain_again() {
        let (mut s, _) = sailing_world(TaskState::Ready, false);
        // A done task of the voyage is not a task left (Prometheus M5: 10
        // counted, 2 left).
        s.write(|tx| {
            let mut done = tx.get_task("p", "T-1")?.unwrap();
            done.id = "T-2".into();
            done.state = TaskState::Done;
            tx.insert_task(&done)
        })
        .unwrap();
        let all = vec![idle_obs(&mut s, "p:captain"), idle_obs(&mut s, "p:heat")];
        captain_sailing(&mut s, "p", &all, T).unwrap();
        captain_sailing(&mut s, "p", &all, T + 5 * 60).unwrap();
        let items = sailing_items(&mut s);
        assert_eq!(items.len(), 1, "{items:?}");
        assert!(items[0].body.contains("1 tasks left"), "{}", items[0].body);
        // Resolved, still idle: again after `CAPTAIN_AGAIN`.
        s.write(|tx| {
            tx.update_message_state(
                "p",
                &items[0].id,
                MsgState::Resolved,
                None,
                None,
                None,
                T + 6 * 60,
            )
        })
        .unwrap();
        captain_sailing(&mut s, "p", &all, T + 14 * 60).unwrap();
        assert_eq!(sailing_items(&mut s).len(), 1, "not yet 10 minutes");
        captain_sailing(&mut s, "p", &all, T + 16 * 60).unwrap();
        assert_eq!(sailing_items(&mut s).len(), 2, "repeated after 10 minutes");
    }

    fn observation(s: &mut Store) -> SeatObs {
        SeatObs {
            seat: s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap(),
            presence: Presence::Present,
            status: Some(AgentStatus::Working),
            flags: Default::default(),
            idle: false,
            screen: None,
            session: None,
        }
    }
    fn set_mtime(path: &std::path::Path, ts: i64) {
        std::fs::File::open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(ts as u64)),
            )
            .unwrap();
    }

    #[test]
    fn resumed_attempt_is_monitored_after_seat_generation_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "session").unwrap();
        set_mtime(&path, T + 100);
        let path = path.to_str().unwrap();
        let restarted = || {
            let mut s = world(dir.path().to_str().unwrap());
            s.write(|tx| {
                let mut binding = tx.get_binding("p:heat", 1)?.unwrap();
                binding.generation = 2;
                binding.session_ref = Some(path.into());
                tx.set_seat_generation("p:heat", 2)?;
                tx.insert_binding(&binding)?;
                assert_eq!(tx.get_attempt("p", "T-1", 1)?.unwrap().generation, 1);
                Ok(())
            })
            .unwrap();
            s
        };
        let mut s = restarted();
        s.write(|tx| tick(tx, "p", T + 1260)).unwrap();
        s.write(|tx| tick(tx, "p", T + 1261)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        // The captain hears the lane's answer, or the alarm when none comes.
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 0);

        let mut s = restarted();
        let mut h = FakeHerdr::new();
        h.add_agent_named(PANE, "pi", AgentStatus::Working, "p-heat");
        h.set_agent_session(PANE, path);
        let obs = observation(&mut s);
        loops(&mut s, &mut h, "p", &[obs.clone()], T + 760).unwrap();
        loops(&mut s, &mut h, "p", &[obs], T + 761).unwrap();
        assert_eq!(
            h.calls
                .iter()
                .filter(
                    |c| matches!(c, Call::SendKeys {pane, keys} if pane == PANE && keys == &["esc"])
                )
                .count(),
            1
        );
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        assert_eq!(
            messages(&mut s, "quiet:p:heat").len(),
            0,
            "the captain hears the answer or the alarm"
        );
    }

    #[test]
    fn stuck_working_session_interrupts_once_and_requests_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "session").unwrap();
        set_mtime(&path, T + 100);
        let mut s = world(dir.path().to_str().unwrap());
        let path = path.to_str().unwrap();
        s.write(|tx| tx.set_binding_session_ref("p:heat", 1, path))
            .unwrap();
        let mut h = FakeHerdr::new();
        h.add_agent_named(PANE, "pi", AgentStatus::Working, "p-heat");
        h.set_agent_session(PANE, path);
        let obs = observation(&mut s);
        loops(&mut s, &mut h, "p", &[obs.clone()], T + 100 + 660).unwrap();
        assert_eq!(
            h.calls
                .iter()
                .filter(
                    |c| matches!(c, Call::SendKeys {pane, keys} if pane == PANE && keys == &["esc"])
                )
                .count(),
            1
        );
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        assert_eq!(
            messages(&mut s, "quiet:p:heat").len(),
            0,
            "the captain hears the answer or the alarm"
        );
        assert_eq!(
            s.read(|tx| tx.get_meta(&compact_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("pending")
        );
        loops(&mut s, &mut h, "p", &[obs], T + 100 + 661).unwrap();
        assert_eq!(
            h.calls
                .iter()
                .filter(|c| matches!(c, Call::SendKeys {keys, ..} if keys == &["esc"]))
                .count(),
            1
        );
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
    }

    #[test]
    fn fresh_or_unreadable_session_does_not_interrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "session").unwrap();
        set_mtime(&path, T + 700);
        let mut s = world(dir.path().to_str().unwrap());
        let path = path.to_str().unwrap();
        s.write(|tx| tx.set_binding_session_ref("p:heat", 1, path))
            .unwrap();
        let mut h = FakeHerdr::new();
        h.add_agent_named(PANE, "pi", AgentStatus::Working, "p-heat");
        h.set_agent_session(PANE, path);
        let obs = observation(&mut s);
        loops(&mut s, &mut h, "p", &[obs], T + 760).unwrap();
        assert!(!h.calls.iter().any(|c| matches!(c, Call::SendKeys { .. })));
        assert!(messages(&mut s, "quiet:p:heat").is_empty());
    }

    struct Think {
        s: Store,
        session: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    fn thinking(mtime: i64) -> Think {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None);
        git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
            None,
        );
        git(dir.path(), &["config", "user.name", "test"], None);
        std::fs::write(dir.path().join("file"), "x").unwrap();
        git(dir.path(), &["add", "file"], None);
        git(
            dir.path(),
            &["commit", "-qm", "one"],
            Some("@1800001200 +0000"),
        );
        let session = dir.path().join("session.jsonl");
        std::fs::write(&session, "session").unwrap();
        set_mtime(&session, mtime);
        let worktree = dir.path().to_str().unwrap().to_string();
        let path = session.to_str().unwrap().to_string();
        let mut s = world(&worktree);
        s.write(|tx| {
            tx.set_meta(&crate::onboard::worktree_path_key("p:heat"), &worktree)?;
            tx.set_binding_session_ref("p:heat", 1, &path)
        })
        .unwrap();
        Think {
            s,
            session,
            _dir: dir,
        }
    }

    fn think(t: &mut Think, now: Ts) {
        let obs = observation(&mut t.s);
        thinks(&mut t.s, "p", &[obs], now).unwrap();
    }

    #[test]
    fn thinking_lane_is_asked_once_per_interval() {
        let mut t = thinking(T + 900);
        think(&mut t, T);
        think(&mut t, T + 960);
        let lane = messages(&mut t.s, "think-lane:p:heat");
        let captain = messages(&mut t.s, "think:p:heat");
        assert_eq!(lane.len(), 1);
        assert!(lane[0].urgent);
        assert_eq!(lane[0].kind, MsgKind::Instruction);
        assert_eq!(lane[0].body, "Your worktree has not changed for 16 min while you work on T-1. If a design question holds you, send gear send --kind question now with your default and go on with that default; otherwise write the code and push.");
        assert_eq!(captain.len(), 1);
        assert_eq!(captain[0].state, MsgState::Info);
        assert_eq!(
            captain[0].body,
            "lane heat: T-1: working for 16 min with no change in its worktree"
        );

        set_mtime(&t.session, T + 1500);
        think(&mut t, T + 1500);
        assert_eq!(
            messages(&mut t.s, "think-lane:p:heat").len(),
            1,
            "a second ask only 9 min later is too soon"
        );
        set_mtime(&t.session, T + 1860);
        think(&mut t, T + 1860);
        assert_eq!(
            messages(&mut t.s, "think-lane:p:heat").len(),
            2,
            "15 min after the first ask"
        );
        assert_eq!(messages(&mut t.s, "think:p:heat").len(), 2);
    }

    /// The think meta as it stands: the fingerprint, when it was read, and the
    /// clock it stands for.
    fn think_meta(t: &mut Think) -> serde_json::Value {
        let raw =
            t.s.read(|tx| tx.get_meta(&think_key("p:heat")))
                .unwrap()
                .expect("the seat has a think meta");
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn the_worktree_is_read_once_a_minute() {
        let mut t = thinking(T + 900);
        think(&mut t, T);
        let first = think_meta(&mut t);
        assert_eq!(first["fp_at"].as_i64(), Some(T));

        // A change 5 s later is not read: the fingerprint of the last reading
        // stands for a minute.
        std::fs::write(t._dir.path().join("file"), "edited").unwrap();
        think(&mut t, T + 5);
        let cached = think_meta(&mut t);
        assert_eq!(cached["fp"], first["fp"], "the fingerprint stands");
        assert_eq!(cached["fp_at"].as_i64(), Some(T), "no second reading");
        assert_eq!(cached["since"], first["since"], "the clock did not move");

        // A minute later the worktree is read again, and the edit resets the
        // clock — up to a minute late, as the packet allows.
        think(&mut t, T + 60);
        let read_again = think_meta(&mut t);
        assert_ne!(
            read_again["fp"], first["fp"],
            "a minute later the worktree is read again"
        );
        assert_eq!(read_again["fp_at"].as_i64(), Some(T + 60));
        assert_eq!(
            read_again["since"].as_i64(),
            Some(T + 60),
            "the edit resets it"
        );
        assert!(messages(&mut t.s, "think-lane:p:heat").is_empty());
    }

    #[test]
    fn a_moved_attempt_reads_the_worktree_at_once() {
        let mut t = thinking(T + 900);
        think(&mut t, T);
        let first = think_meta(&mut t);

        // The stored meta is for another attempt: it is not the state of this
        // one, so the fingerprint is read at once instead of standing.
        let mut stale = first.clone();
        stale["n"] = serde_json::json!(0);
        t.s.write(|tx| tx.set_meta(&think_key("p:heat"), &stale.to_string()))
            .unwrap();
        std::fs::write(t._dir.path().join("file"), "edited").unwrap();
        think(&mut t, T + 5);

        let read_again = think_meta(&mut t);
        assert_eq!(
            read_again["n"].as_i64(),
            Some(1),
            "the live attempt is stored"
        );
        assert_ne!(
            read_again["fp"], first["fp"],
            "a moved attempt reads the worktree at once"
        );
    }

    #[test]
    fn worktree_edit_resets_the_think_clock() {
        let mut t = thinking(T);
        think(&mut t, T);
        set_mtime(&t.session, T + 600);
        std::fs::write(t._dir.path().join("file"), "edited").unwrap();
        think(&mut t, T + 600);
        set_mtime(&t.session, T + 960);
        think(&mut t, T + 960);
        assert!(messages(&mut t.s, "think-lane:p:heat").is_empty());
        assert!(messages(&mut t.s, "think:p:heat").is_empty());
    }

    #[test]
    fn a_session_that_stopped_growing_is_left_to_the_loop_guard() {
        let mut t = thinking(T + 300);
        think(&mut t, T);
        think(&mut t, T + 960);
        assert!(messages(&mut t.s, "think-lane:p:heat").is_empty());
        assert!(messages(&mut t.s, "think:p:heat").is_empty());
    }

    /// T-610: the no-push nudge closes itself when the attempt it names was
    /// handed in; a nudge for a still-open attempt stays.
    #[test]
    fn a_no_push_nudge_closes_on_the_hand_in_of_its_attempt() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        // The lane hands in: the attempt is Submitted.
        s.write(|tx| {
            let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
            attempt.state = AttemptState::Submitted;
            tx.update_attempt(&attempt)
        })
        .unwrap();
        s.write(|tx| tick(tx, "p", T + 1300)).unwrap();
        let lane = messages(&mut s, "quiet-lane:p:heat");
        assert_eq!(lane.len(), 1);
        assert_eq!(lane[0].state, MsgState::Resolved);
        assert!(
            lane[0]
                .result
                .as_deref()
                .is_some_and(|r| r.contains("was handed in")),
            "{:?}",
            lane[0].result
        );
    }

    /// T-610: a nudge for a still-open attempt stays open.
    #[test]
    fn a_no_push_nudge_for_a_still_open_attempt_stays() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        s.write(|tx| tick(tx, "p", T + 1300)).unwrap();
        let lane = messages(&mut s, "quiet-lane:p:heat");
        assert_eq!(lane.len(), 1);
        assert_eq!(lane[0].state, MsgState::Open);
        assert_eq!(lane[0].result, None);
    }

    /// T-610: the no-push nudge closes when its task is cancelled.
    #[test]
    fn a_no_push_nudge_closes_on_a_cancel_of_its_task() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        s.write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.state = TaskState::Cancelled;
            tx.update_task(&task)
        })
        .unwrap();
        s.write(|tx| tick(tx, "p", T + 1300)).unwrap();
        let lane = messages(&mut s, "quiet-lane:p:heat");
        assert_eq!(lane.len(), 1);
        assert_eq!(lane[0].state, MsgState::Resolved);
        assert!(
            lane[0]
                .result
                .as_deref()
                .is_some_and(|r| r.contains("is cancelled")),
            "{:?}",
            lane[0].result
        );
    }

    /// T-610: the captain's "no word" item closes when the lane's hand-in
    /// answered it.
    #[test]
    fn the_captains_no_word_item_closes_when_the_lane_hands_in() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        rang(&mut s, T + 1201);
        s.write(|tx| tick(tx, "p", T + 1201 + 600)).unwrap();
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 1);
        assert_eq!(messages(&mut s, "quiet:p:heat")[0].state, MsgState::Open);
        // The lane hands in: a result from the lane after the item.
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "T-1:1".into(),
                task: Some("T-1".into()),
                attempt: Some(1),
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Result,
                urgent: false,
                body: "done".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: T + 1201 + 700,
                meta: None,
            })
            .map(|_| ())
        })
        .unwrap();
        s.write(|tx| tick(tx, "p", T + 1201 + 710)).unwrap();
        let captain = messages(&mut s, "quiet:p:heat");
        assert_eq!(captain.len(), 1);
        assert_eq!(captain[0].state, MsgState::Resolved);
        assert!(
            captain[0]
                .result
                .as_deref()
                .is_some_and(|r| r.contains("handed in")),
            "{:?}",
            captain[0].result
        );
    }
}
