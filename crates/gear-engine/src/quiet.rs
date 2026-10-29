//! Remind quiet lanes and interrupt a stuck working turn.
use crate::observe::{Presence, SeatObs};
use gear_herdr::{AgentStatus, HerdrAct, Outcome};
use gear_model::{AttemptState, GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{AttemptRow, NewMessage, SeatRow, Store, Tx};
use std::hash::{Hash, Hasher};

const LOOP_INTERVAL: i64 = 10 * 60;
const THINK_INTERVAL: i64 = 15 * 60;
fn ask_key(seat: &str) -> String { format!("engine.quiet.{seat}") }
fn loop_key(seat: &str) -> String { format!("engine.quiet.loop.{seat}") }
fn think_key(seat: &str) -> String { format!("engine.quiet.think.{seat}") }
pub(crate) fn compact_key(seat: &str) -> String { format!("engine.quiet.compact.{seat}") }

fn eligible(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Option<AttemptRow>> {
    if seat.role != SeatRole::Lane || !matches!(seat.state, SeatState::Active | SeatState::Standby | SeatState::Retiring)
        || tx.seats_in_backoff(&seat.project)?.iter().any(|s| s == &seat.id)
        || crate::waits::waiting_seat(tx, &seat.id)? {
        return Ok(None);
    }
    Ok(tx.in_progress_attempts_of_seat(&seat.id)?.into_iter().find(|a| a.state == AttemptState::Started))
}

// A loop can also occur while an assigned delivery or a stopped attempt is
// being processed, before a Started check-in has been recorded.
fn loop_attempt(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Option<AttemptRow>> {
    if seat.role != SeatRole::Lane || !matches!(seat.state, SeatState::Active | SeatState::Standby | SeatState::Retiring)
        || tx.seats_in_backoff(&seat.project)?.iter().any(|s| s == &seat.id)
        || crate::waits::waiting_seat(tx, &seat.id)? { return Ok(None) }
    Ok(tx.in_progress_attempts_of_seat(&seat.id)?.into_iter().next())
}

fn last_push(tx: &Tx<'_>, seat: &SeatRow, attempt: &AttemptRow) -> GearResult<(String, Ts)> {
    // The branch is the project's own (T-253): the template it names the task
    // branch with, `{lane}/t{n}` when it says nothing.
    let conventions = crate::dispatch::conventions(tx, &seat.project);
    let branch = crate::dispatch::task_branch(&conventions.task_branch, &seat.project, &seat.name, &attempt.task);
    // The project's own remote (T-262): the push clock reads the ref its
    // `[conventions].remote` names.
    let remote = &conventions.remote;
    let start = tx.attempt_times(&seat.project, &attempt.task, attempt.n)?.started.unwrap_or(attempt.started);
    let task = tx.get_task(&seat.project, &attempt.task)?;
    let project = tx.get_project(&seat.project)?;
    let repo = task.and_then(|t| t.repo).or_else(|| project
        .and_then(|p| gear_app::config::parse(&p.config_toml).ok())
        .map(|c| c.repo_of_lane(&seat.name).path));
    let pushed = repo.and_then(|repo| std::process::Command::new("git")
        .args(["-C", &repo, "log", "-1", "--format=%ct", &format!("refs/remotes/{remote}/{branch}"), "--"])
        .output().ok()).filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<Ts>().ok());
    Ok((branch, pushed.unwrap_or(start).max(start)))
}

/// The time the lane was asked why it has not pushed, while its answer is due.
fn reply_key(seat: &str) -> String { format!("engine.quiet.reply.{seat}") }
/// Where the no-push clock restarts after a question was settled: the end of
/// the answer window, so a lane that explained itself gets another full
/// interval before it is asked again.
fn base_key(seat: &str) -> String { format!("engine.quiet.base.{seat}") }

/// Ask a lane that has not pushed for `minutes` to tell the captain why,
/// within `reply` minutes.
fn notify(tx: &Tx<'_>, seat: &SeatRow, attempt: &AttemptRow, branch: &str, minutes: i64, reply: i64, now: Ts) -> GearResult<()> {
    let body = format!(
        "No push to {branch} for {minutes} min. Within {reply} min, tell the captain why: gear send --kind question --task {task} --attempt {n} \"no push for {minutes} min: <the reason: the task is complex because ..., or the packet is unclear: ..., or ...>\". Then commit and push what builds.",
        task = attempt.task,
        n = attempt.n,
    );
    tx.insert_message(&NewMessage { project: seat.project.clone(), thread: format!("quiet-lane:{}", seat.id), task: Some(attempt.task.clone()), attempt: Some(attempt.n), sender: "engine".into(), sender_gen: 0, recipient: seat.id.clone(), kind: MsgKind::Instruction, urgent: true, body, reply_to: None, needs_ack: false, state: MsgState::Open, created: now, meta: None })?;
    tx.set_meta(&reply_key(&seat.id), &now.to_string())?;
    tx.set_meta(&ask_key(&seat.id), &now.to_string())
}

/// Tell the captain that a lane neither pushed nor explained why in time.
fn alarm(tx: &Tx<'_>, seat: &SeatRow, attempt: &AttemptRow, branch: &str, minutes: i64, reply: i64, now: Ts) -> GearResult<()> {
    let Some(captain) = tx.captain_of(&seat.project)? else { return Ok(()) };
    let body = format!(
        "lane {}: task {}: no push to {branch} for {minutes} min, and no word to the captain within {reply} min of being asked why. This lane is in trouble: look at its pane and act (answer its question, cut the task, or restart it).",
        seat.name, attempt.task
    );
    tx.insert_message(&NewMessage { project: seat.project.clone(), thread: format!("quiet:{}", seat.id), task: Some(attempt.task.clone()), attempt: Some(attempt.n), sender: "engine".into(), sender_gen: 0, recipient: captain.id, kind: MsgKind::System, urgent: true, body, reply_to: None, needs_ack: false, state: MsgState::Open, created: now, meta: None })?;
    Ok(())
}

/// The worktree's fingerprint: its HEAD, its dirty set and its diff. A git
/// that fails says nothing about the lane, so the seat is skipped this tick.
fn think_fp(path: &str) -> Option<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for args in [["rev-parse", "HEAD"], ["status", "--porcelain"], ["diff", "HEAD"]] {
        let output = std::process::Command::new("git").args(["-C", path]).args(args).output().ok()
            .filter(|o| o.status.success())?;
        output.stdout.hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// The lane's session file is still growing, as fn loops reads it.
fn session_growing(tx: &Tx<'_>, seat: &SeatRow, now: Ts) -> GearResult<bool> {
    let Some(path) = tx.get_binding(&seat.id, seat.generation)?.and_then(|b| b.session_ref).filter(|s| !s.is_empty()) else { return Ok(false) };
    Ok(std::fs::metadata(&path).ok().filter(|m| m.is_file()).and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as Ts)
        .is_some_and(|t| now.saturating_sub(t) < LOOP_INTERVAL))
}

fn think_notify(tx: &Tx<'_>, seat: &SeatRow, attempt: &AttemptRow, minutes: i64, now: Ts) -> GearResult<()> {
    let body = format!("Your worktree has not changed for {minutes} min while you work on {}. If a design question holds you, send gear send --kind question now with your default and go on with that default; otherwise write the code and push.", attempt.task);
    tx.insert_message(&NewMessage { project: seat.project.clone(), thread: format!("think-lane:{}", seat.id), task: Some(attempt.task.clone()), attempt: Some(attempt.n), sender: "engine".into(), sender_gen: 0, recipient: seat.id.clone(), kind: MsgKind::Instruction, urgent: true, body, reply_to: None, needs_ack: false, state: MsgState::Open, created: now, meta: None })?;
    if let Some(captain) = tx.captain_of(&seat.project)? {
        tx.insert_message(&NewMessage { project: seat.project.clone(), thread: format!("think:{}", seat.id), task: Some(attempt.task.clone()), attempt: Some(attempt.n), sender: "engine".into(), sender_gen: 0, recipient: captain.id, kind: MsgKind::System, urgent: false, body: format!("lane {}: {}: working for {minutes} min with no change in its worktree", seat.name, attempt.task), reply_to: None, needs_ack: false, state: MsgState::Info, created: now, meta: None })?;
    }
    Ok(())
}

/// A 15-minute think clock (M4.1): a working lane whose session file grows
/// while its worktree does not change is asked to stop thinking. The
/// fingerprint and its clock are persisted per seat across engine restarts.
pub fn thinks(store: &mut Store, project: &str, observed: &[SeatObs], now: Ts) -> GearResult<()> {
    for obs in observed.iter().filter(|o| o.presence == Presence::Present && o.status == Some(AgentStatus::Working)) {
        let candidate = store.read(|tx| {
            let Some(seat) = tx.get_seat(&obs.seat.id)? else { return Ok(None) };
            if seat.project != project || seat.generation != obs.seat.generation { return Ok(None) }
            let Some(attempt) = eligible(tx, &seat)? else { return Ok(None) };
            let Some(path) = tx.get_meta(&crate::onboard::worktree_path_key(&seat.id))?.filter(|p| !p.is_empty()) else { return Ok(None) };
            Ok(Some((seat, attempt, path)))
        })?;
        let Some((seat, attempt, path)) = candidate else { continue };
        let Some(fp) = think_fp(&path) else { continue };
        store.write(|tx| {
            let meta = tx.get_meta(&think_key(&seat.id))?.and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
            let live = meta.as_ref().filter(|m| m["task"].as_str() == Some(attempt.task.as_str())
                && m["n"].as_i64() == Some(attempt.n) && m["fp"].as_u64() == Some(fp));
            let since = live.and_then(|m| m["since"].as_i64()).unwrap_or(now);
            let mut asked = live.and_then(|m| m["asked"].as_i64());
            if now.saturating_sub(since) >= THINK_INTERVAL && asked.is_none_or(|a| now.saturating_sub(a) >= THINK_INTERVAL)
                && session_growing(tx, &seat, now)? {
                think_notify(tx, &seat, &attempt, now.saturating_sub(since) / 60, now)?;
                asked = Some(now);
            }
            tx.set_meta(&think_key(&seat.id), &serde_json::json!({"task":attempt.task,"n":attempt.n,"fp":fp,"since":since,"asked":asked}).to_string())
        })?;
    }
    Ok(())
}

/// The no-push clock, persisted per seat across engine restarts: a lane that
/// has not pushed for `[conventions] push_interval_min` (20) is asked to tell
/// the captain why within `push_reply_min` (10). Its answer goes to the
/// captain as an item to read, and the clock restarts at the end of the
/// window; no answer in time is an urgent item to the captain.
pub fn tick(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<()> {
    let conventions = crate::dispatch::conventions(tx, project);
    let interval = i64::from(conventions.push_interval_min.max(1)) * 60;
    let reply = i64::from(conventions.push_reply_min.max(1)) * 60;
    let captain = tx.captain_of(project)?.map(|seat| seat.id);
    for seat in tx.seats_of_project(project)? {
        let Some(attempt) = eligible(tx, &seat)? else {
            // A hold is not the lane's own time: while a lane seat is not
            // eligible the no-push clock stands still, so a released lane is
            // asked from the end of the hold, not all at once (T-417).
            if seat.role == SeatRole::Lane {
                tx.set_meta(&base_key(&seat.id), &now.to_string())?;
                tx.set_meta(&reply_key(&seat.id), "")?;
            }
            continue
        };
        let (branch, push) = last_push(tx, &seat, &attempt)?;
        let base = tx.get_meta(&base_key(&seat.id))?.and_then(|s| s.parse::<Ts>().ok()).unwrap_or(0);
        let asked = tx.get_meta(&reply_key(&seat.id))?.filter(|s| !s.is_empty()).and_then(|s| s.parse::<Ts>().ok());
        if let Some(asked) = asked {
            let settle = |tx: &Tx<'_>| -> GearResult<()> {
                tx.set_meta(&reply_key(&seat.id), "")?;
                tx.set_meta(&base_key(&seat.id), &(asked + reply).to_string())
            };
            if push > asked {
                settle(tx)?;
                continue;
            }
            let answers: Vec<_> = tx.messages_from_since(project, &seat.id, asked)?
                .into_iter()
                .filter(|m| captain.as_deref() == Some(m.recipient.as_str()))
                .collect();
            if !answers.is_empty() {
                // The answer is the captain's to read, whatever kind it was sent as.
                for answer in answers.iter().filter(|m| m.state == MsgState::Info) {
                    tx.update_message_state(project, &answer.id, MsgState::Open, None, None, None, now)?;
                }
                settle(tx)?;
                continue;
            }
            if now - asked >= reply {
                alarm(tx, &seat, &attempt, &branch, now.saturating_sub(push) / 60, reply / 60, now)?;
                settle(tx)?;
            }
            continue;
        }
        if now.saturating_sub(push.max(base)) > interval {
            notify(tx, &seat, &attempt, &branch, now.saturating_sub(push) / 60, reply / 60, now)?;
        }
    }
    Ok(())
}

/// Reserve the interruption before using Herdr: a crash or uncertain response
/// cannot send repeated escapes. Unreadable session files are not evidence.
pub fn loops(store: &mut Store, herdr: &mut dyn HerdrAct, project: &str, observed: &[SeatObs], now: Ts) -> GearResult<()> {
    for obs in observed.iter().filter(|o| o.presence == Presence::Present && o.status == Some(AgentStatus::Working)) {
        let candidate = store.read(|tx| {
            let Some(seat) = tx.get_seat(&obs.seat.id)? else { return Ok(None) };
            if seat.project != project || seat.generation != obs.seat.generation { return Ok(None) }
            let Some(attempt) = loop_attempt(tx, &seat)? else { return Ok(None) };
            let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else { return Ok(None) };
            let Some(path) = binding.session_ref.as_deref().filter(|s| !s.is_empty()) else { return Ok(None) };
            let modified = std::fs::metadata(path).ok().filter(|m| m.is_file()).and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as Ts);
            if modified.is_none_or(|t| now.saturating_sub(t) < LOOP_INTERVAL) { return Ok(None) }
            let last = tx.get_meta(&loop_key(&seat.id))?.and_then(|s| s.parse::<Ts>().ok());
            if last.is_some_and(|t| now.saturating_sub(t) < LOOP_INTERVAL) { return Ok(None) }
            let (branch, pushed) = last_push(tx, &seat, &attempt)?;
            Ok(Some((seat, attempt, binding, branch, now.saturating_sub(pushed) / 60)))
        })?;
        let Some((seat, attempt, binding, branch, minutes)) = candidate else { continue };
        // Confirm the current occupant immediately before the escape.
        let current = herdr.agent_get(&binding.pane_id).ok().flatten();
        if !current.is_some_and(|a| a.agent.as_deref() == Some(binding.kind.as_str()) && a.agent_status == AgentStatus::Working
            && a.agent_session.as_ref().is_none_or(|s| Some(s.value.as_str()) == binding.session_ref.as_deref())) { continue }
        let reserved = store.write(|tx| {
            let last = tx.get_meta(&loop_key(&seat.id))?.and_then(|s| s.parse::<Ts>().ok());
            if last.is_some_and(|t| now.saturating_sub(t) < LOOP_INTERVAL) { return Ok(None) }
            let Some(current) = tx.get_seat(&seat.id)? else { return Ok(None) };
            let same_attempt = loop_attempt(tx, &current)?.is_some_and(|a| a.task == attempt.task && a.n == attempt.n);
            let same_binding = tx.get_binding(&seat.id, current.generation)?.is_some_and(|b|
                b.generation == binding.generation && b.pane_id == binding.pane_id && b.session_ref == binding.session_ref);
            if !same_attempt || !same_binding { return Ok(None) }
            let already_compacting = tx.get_meta(&compact_key(&seat.id))?.is_some_and(|v| !v.is_empty());
            tx.set_meta(&loop_key(&seat.id), &now.to_string())?;
            if !already_compacting { tx.set_meta(&compact_key(&seat.id), "pending")?; }
            Ok(Some(already_compacting))
        })?;
        let Some(already_compacting) = reserved else { continue };
        if matches!(herdr.send_keys(&binding.pane_id, &["esc".into()]), Outcome::Refused { .. }) {
            if !already_compacting { store.write(|tx| tx.set_meta(&compact_key(&seat.id), ""))?; }
            continue;
        }
        store.write(|tx| {
            let reply = i64::from(crate::dispatch::conventions(tx, project).push_reply_min.max(1));
            notify(tx, &seat, &attempt, &branch, minutes, reply, now)
        })?;
    }
    thinks(store, project, observed, now)
}

/// A delivery can have been intended before the loop was detected. Drive its
/// compaction too; execution holds the op until the marker is cleared.
pub fn prepare_pending_deliveries(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &crate::EngineCfg, project: &str, now: Ts) -> GearResult<()> {
    let pending = store.read(|tx| {
        let mut work = Vec::new();
        for op in tx.ops_in_state(project, gear_model::OpState::Intended)? {
            if op.kind != gear_model::OpKind::Deliver || !tx.get_meta(&compact_key(&op.seat))?.is_some_and(|s| !s.is_empty()) { continue; }
            let (Some(seat), Some(task), Some(attempt)) = (tx.get_seat(&op.seat)?, op.task, op.attempt) else { continue };
            let active = tx.get_attempt(project, &task, attempt)?.is_some_and(|a| !a.state.is_terminal())
                && tx.get_task(project, &task)?.is_some_and(|t| !t.state.is_terminal());
            if active && seat.generation == op.generation && seat.role == SeatRole::Lane
                && !crate::waits::waiting_seat(tx, &seat.id)? { work.push((seat, task, attempt)); }
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

    fn world(repo: &str) -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T)?;
            tx.insert_task(&TaskRow { project:"p".into(), id:"T-1".into(), rank:1, state:TaskState::Ready, priority:"must".into(), lane:Some("heat".into()), milestone:None, required:false, contract_version:1, deps:vec![], followup_of:None, wait_on:None, wait_cond:None, rework_note:None, estimate_min:None, issue:None, updated:T, repo:Some(repo.into()), base:None, gate:None, rework_estimate_min:None })?;
            tx.insert_contract(&ContractRow {project:"p".into(),task:"T-1".into(),version:1,goal:"g".into(),accept:"a".into(),evidence_files:vec![],evidence_due_min:None,body:String::new(),created:T, mechanical: None,})?;
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T)?;
            Ok(())
        }).unwrap();
        s
    }

    fn git(repo: &std::path::Path, args: &[&str], date: Option<&str>) {
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(repo).args(args);
        if let Some(date) = date { command.env("GIT_AUTHOR_DATE", date).env("GIT_COMMITTER_DATE", date); }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    fn messages(s: &mut Store, thread: &str) -> Vec<gear_store::MessageRow> {
        s.read(|tx| tx.thread_messages("p", thread)).unwrap()
    }

    fn repo_with_commit() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None);
        git(dir.path(), &["config", "user.email", "test@example.invalid"], None);
        git(dir.path(), &["config", "user.name", "test"], None);
        std::fs::write(dir.path().join("file"), "x").unwrap();
        git(dir.path(), &["add", "file"], None);
        git(dir.path(), &["commit", "-qm", "one"], Some("@1800001200 +0000"));
        dir
    }

    /// The lane's own word to the captain, as `gear send` writes it.
    fn lane_says(s: &mut Store, kind: MsgKind, state: MsgState, at: Ts) {
        s.write(|tx| tx.insert_message(&NewMessage { project: "p".into(), thread: "T-1:q".into(), task: Some("T-1".into()), attempt: Some(1), sender: "p:heat".into(), sender_gen: 1, recipient: "p:captain".into(), kind, urgent: false, body: "no push: the task is complex".into(), reply_to: None, needs_ack: false, state, created: at, meta: None }).map(|_| ())).unwrap();
    }

    /// 20 min without a push: the lane is asked once to tell the captain why
    /// within 10 min; its answer becomes the captain's item; the clock then
    /// restarts at the end of the window, so the next question comes at 50.
    #[test]
    fn a_quiet_lane_is_asked_why_and_its_answer_restarts_the_clock() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1199)).unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty(), "not before 20 min");
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        let lane = messages(&mut s, "quiet-lane:p:heat");
        assert_eq!(lane.len(), 1);
        assert!(lane[0].urgent);
        assert!(lane[0].body.starts_with("No push to heat/t1 for 20 min. Within 10 min, tell the captain why: gear send --kind question --task T-1 --attempt 1"), "{}", lane[0].body);
        assert!(messages(&mut s, "quiet:p:heat").is_empty(), "the captain hears the lane's answer, not a notice");
        s.write(|tx| tick(tx, "p", T + 1260)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1, "asked once");
        // The lane answers as progress (an info kind): it becomes an item.
        lane_says(&mut s, MsgKind::Progress, MsgState::Info, T + 1500);
        s.write(|tx| tick(tx, "p", T + 1510)).unwrap();
        let items = s.read(|tx| tx.actionable_for("p", "p:captain", T + 1510)).unwrap();
        assert!(items.iter().any(|m| m.body == "no push: the task is complex"), "{items:?}");
        // Asked at 20, window to 30: the next question is after 50.
        s.write(|tx| tick(tx, "p", T + 1201 + 600 + 1200)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        s.write(|tx| tick(tx, "p", T + 1201 + 600 + 1201)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 2, "asked again after 50 min");
        assert!(messages(&mut s, "quiet:p:heat").is_empty(), "no alarm: the lane answered");
    }

    /// No answer within the window: the captain gets an urgent item that the
    /// lane is in trouble; a push, instead, settles the question.
    #[test]
    fn a_quiet_lane_that_does_not_answer_is_reported_and_a_push_settles_it() {
        let dir = repo_with_commit();
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        s.write(|tx| tick(tx, "p", T + 1201 + 599)).unwrap();
        assert!(messages(&mut s, "quiet:p:heat").is_empty(), "not before the window ends");
        s.write(|tx| tick(tx, "p", T + 1201 + 600)).unwrap();
        let captain = messages(&mut s, "quiet:p:heat");
        assert_eq!(captain.len(), 1);
        assert!(captain[0].urgent && captain[0].state == MsgState::Open);
        assert!(captain[0].body.contains("no word to the captain within 10 min"), "{}", captain[0].body);
        s.write(|tx| tick(tx, "p", T + 1201 + 660)).unwrap();
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 1, "told once");

        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T + 1201)).unwrap();
        git(dir.path(), &["update-ref", "refs/remotes/origin/heat/t1", "HEAD"], None);
        // The commit is dated T+1200 (before the question): a later push resets.
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "two"], Some("@1800001400 +0000"));
        git(dir.path(), &["update-ref", "refs/remotes/origin/heat/t1", "HEAD"], None);
        s.write(|tx| tick(tx, "p", T + 1201 + 700)).unwrap();
        assert!(messages(&mut s, "quiet:p:heat").is_empty(), "a push settles the question");
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
        }).unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty(), "a held lane is not asked");
        s.write(|tx| tx.set_seat_state("p:heat", SeatState::Active, None, T + 3 * 3600)).unwrap();
        s.write(|tx| tick(tx, "p", T + 3 * 3600 + 1199)).unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty(), "the clock runs from the end of the hold");
        s.write(|tx| tick(tx, "p", T + 3 * 3600 + 1201)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1, "asked once after the interval");
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
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1, "asked after 15 min");
        s.write(|tx| tick(tx, "p", T + 901 + 300)).unwrap();
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 1, "reported after 5 more");
    }

    #[test]
    fn parked_held_backoff_and_unstarted_lanes_are_quiet() {
        for state in [SeatState::Parked, SeatState::Held] {
            let mut s = world("/missing");
            s.write(|tx| { tx.set_seat_state("p:heat", state, None, T)?; tick(tx, "p", T+1300) }).unwrap();
            assert!(messages(&mut s, "quiet-lane:p:heat").is_empty());
        }
        let mut s = world("/missing");
        s.write(|tx| { tx.set_seats_in_backoff("p", &["p:heat".into()])?; tick(tx, "p", T+1300) }).unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty());
        s.write(|tx| {
            tx.set_seats_in_backoff("p", &[])?;
            let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap(); attempt.state = AttemptState::Submitted; tx.update_attempt(&attempt)?;
            tick(tx, "p", T+1300)
        }).unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty());
        let mut s = world("/missing");
        s.write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap(); task.wait_on = Some("T-2".into()); tx.update_task(&task)?;
            tick(tx, "p", T+1300)
        }).unwrap();
        assert!(messages(&mut s, "quiet-lane:p:heat").is_empty(), "declared task waits are not stalled work");
    }

    fn observation(s: &mut Store) -> SeatObs {
        SeatObs { seat:s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap(), presence:Presence::Present,
            status:Some(AgentStatus::Working), flags:Default::default(), idle:false, screen:None, session:None }
    }
    fn set_mtime(path: &std::path::Path, ts: i64) {
        std::fs::File::open(path).unwrap().set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(ts as u64))).unwrap();
    }

    #[test]
    fn resumed_attempt_is_monitored_after_seat_generation_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "session").unwrap();
        set_mtime(&path, T+100);
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
            }).unwrap();
            s
        };
        let mut s = restarted();
        s.write(|tx| tick(tx, "p", T+1260)).unwrap();
        s.write(|tx| tick(tx, "p", T+1261)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        // The captain hears the lane's answer, or the alarm when none comes.
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 0);

        let mut s = restarted();
        let mut h = FakeHerdr::new();
        h.add_agent_named(PANE, "pi", AgentStatus::Working, "p-heat");
        h.set_agent_session(PANE, path);
        let obs = observation(&mut s);
        loops(&mut s, &mut h, "p", &[obs.clone()], T+760).unwrap();
        loops(&mut s, &mut h, "p", &[obs], T+761).unwrap();
        assert_eq!(h.calls.iter().filter(|c| matches!(c, Call::SendKeys {pane, keys} if pane == PANE && keys == &["esc"])).count(), 1);
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 0, "the captain hears the answer or the alarm");
    }

    #[test]
    fn stuck_working_session_interrupts_once_and_requests_compaction() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "session").unwrap(); set_mtime(&path, T+100);
        let mut s = world(dir.path().to_str().unwrap());
        let path = path.to_str().unwrap();
        s.write(|tx| tx.set_binding_session_ref("p:heat", 1, path)).unwrap();
        let mut h = FakeHerdr::new(); h.add_agent_named(PANE, "pi", AgentStatus::Working, "p-heat"); h.set_agent_session(PANE, path);
        let obs = observation(&mut s);
        loops(&mut s, &mut h, "p", &[obs.clone()], T+100+660).unwrap();
        assert_eq!(h.calls.iter().filter(|c| matches!(c, Call::SendKeys {pane, keys} if pane == PANE && keys == &["esc"])).count(), 1);
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 0, "the captain hears the answer or the alarm");
        assert_eq!(s.read(|tx| tx.get_meta(&compact_key("p:heat"))).unwrap().as_deref(), Some("pending"));
        loops(&mut s, &mut h, "p", &[obs], T+100+661).unwrap();
        assert_eq!(h.calls.iter().filter(|c| matches!(c, Call::SendKeys {keys, ..} if keys == &["esc"])).count(), 1);
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
    }

    #[test]
    fn fresh_or_unreadable_session_does_not_interrupt() {        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "session").unwrap(); set_mtime(&path, T+700);
        let mut s = world(dir.path().to_str().unwrap()); let path = path.to_str().unwrap();
        s.write(|tx| tx.set_binding_session_ref("p:heat", 1, path)).unwrap();
        let mut h = FakeHerdr::new(); h.add_agent_named(PANE, "pi", AgentStatus::Working, "p-heat"); h.set_agent_session(PANE, path);
        let obs = observation(&mut s);
        loops(&mut s, &mut h, "p", &[obs], T+760).unwrap();
        assert!(!h.calls.iter().any(|c| matches!(c, Call::SendKeys {..})));
        assert!(messages(&mut s, "quiet:p:heat").is_empty());
    }

    struct Think { s: Store, session: std::path::PathBuf, _dir: tempfile::TempDir }

    fn thinking(mtime: i64) -> Think {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None);
        git(dir.path(), &["config", "user.email", "test@example.invalid"], None);
        git(dir.path(), &["config", "user.name", "test"], None);
        std::fs::write(dir.path().join("file"), "x").unwrap();
        git(dir.path(), &["add", "file"], None);
        git(dir.path(), &["commit", "-qm", "one"], Some("@1800001200 +0000"));
        let session = dir.path().join("session.jsonl");
        std::fs::write(&session, "session").unwrap();
        set_mtime(&session, mtime);
        let worktree = dir.path().to_str().unwrap().to_string();
        let path = session.to_str().unwrap().to_string();
        let mut s = world(&worktree);
        s.write(|tx| {
            tx.set_meta(&crate::onboard::worktree_path_key("p:heat"), &worktree)?;
            tx.set_binding_session_ref("p:heat", 1, &path)
        }).unwrap();
        Think { s, session, _dir: dir }
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
        assert_eq!(lane.len(), 1); assert!(lane[0].urgent); assert_eq!(lane[0].kind, MsgKind::Instruction);
        assert_eq!(lane[0].body, "Your worktree has not changed for 16 min while you work on T-1. If a design question holds you, send gear send --kind question now with your default and go on with that default; otherwise write the code and push.");
        assert_eq!(captain.len(), 1); assert_eq!(captain[0].state, MsgState::Info);
        assert_eq!(captain[0].body, "lane heat: T-1: working for 16 min with no change in its worktree");

        set_mtime(&t.session, T + 1500);
        think(&mut t, T + 1500);
        assert_eq!(messages(&mut t.s, "think-lane:p:heat").len(), 1, "a second ask only 9 min later is too soon");
        set_mtime(&t.session, T + 1860);
        think(&mut t, T + 1860);
        assert_eq!(messages(&mut t.s, "think-lane:p:heat").len(), 2, "15 min after the first ask");
        assert_eq!(messages(&mut t.s, "think:p:heat").len(), 2);
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
}
