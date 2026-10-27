//! Remind quiet lanes and interrupt a stuck working turn.
use crate::observe::{Presence, SeatObs};
use gear_herdr::{AgentStatus, HerdrAct, Outcome};
use gear_model::{AttemptState, GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{AttemptRow, NewMessage, SeatRow, Store, Tx};

const PUSH_INTERVAL: i64 = 20 * 60;
const LOOP_INTERVAL: i64 = 10 * 60;
fn ask_key(seat: &str) -> String { format!("engine.quiet.{seat}") }
fn loop_key(seat: &str) -> String { format!("engine.quiet.loop.{seat}") }
pub(crate) fn compact_key(seat: &str) -> String { format!("engine.quiet.compact.{seat}") }

fn eligible(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Option<AttemptRow>> {
    if seat.role != SeatRole::Lane || !matches!(seat.state, SeatState::Active | SeatState::Standby | SeatState::Retiring)
        || tx.seats_in_backoff(&seat.project)?.iter().any(|s| s == &seat.id) {
        return Ok(None);
    }
    Ok(tx.in_progress_attempts_of_seat(&seat.id)?.into_iter().find(|a| a.state == AttemptState::Started && a.generation == seat.generation))
}

// A loop can also occur while an assigned delivery or a stopped attempt is
// being processed, before a Started check-in has been recorded.
fn loop_attempt(tx: &Tx<'_>, seat: &SeatRow) -> GearResult<Option<AttemptRow>> {
    if seat.role != SeatRole::Lane || !matches!(seat.state, SeatState::Active | SeatState::Standby | SeatState::Retiring)
        || tx.seats_in_backoff(&seat.project)?.iter().any(|s| s == &seat.id) { return Ok(None) }
    Ok(tx.in_progress_attempts_of_seat(&seat.id)?.into_iter().find(|a| a.generation == seat.generation))
}

fn last_push(tx: &Tx<'_>, seat: &SeatRow, attempt: &AttemptRow) -> GearResult<(String, Ts)> {
    let branch = crate::dispatch::task_branch(&seat.name, &attempt.task);
    let start = tx.attempt_times(&seat.project, &attempt.task, attempt.n)?.started.unwrap_or(attempt.started);
    let task = tx.get_task(&seat.project, &attempt.task)?;
    let project = tx.get_project(&seat.project)?;
    let repo = task.and_then(|t| t.repo).or_else(|| project
        .and_then(|p| gear_app::config::parse(&p.config_toml).ok())
        .map(|c| c.repo_of_lane(&seat.name).path));
    let pushed = repo.and_then(|repo| std::process::Command::new("git")
        .args(["-C", &repo, "log", "-1", "--format=%ct", &format!("refs/remotes/origin/{branch}"), "--"])
        .output().ok()).filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<Ts>().ok());
    Ok((branch, pushed.unwrap_or(start).max(start)))
}

fn notify(tx: &Tx<'_>, seat: &SeatRow, attempt: &AttemptRow, branch: &str, minutes: i64, now: Ts) -> GearResult<()> {
    let body = format!("No push to {branch} for {minutes} min. If anything in your task is unclear, send gear send --kind question now with your default; otherwise commit and push what builds.");
    tx.insert_message(&NewMessage { project: seat.project.clone(), thread: format!("quiet-lane:{}", seat.id), task: Some(attempt.task.clone()), attempt: Some(attempt.n), sender: "engine".into(), sender_gen: 0, recipient: seat.id.clone(), kind: MsgKind::Instruction, urgent: true, body, reply_to: None, needs_ack: false, state: MsgState::Open, created: now, meta: None })?;
    if let Some(captain) = tx.captain_of(&seat.project)? {
        tx.insert_message(&NewMessage { project: seat.project.clone(), thread: format!("quiet:{}", seat.id), task: Some(attempt.task.clone()), attempt: Some(attempt.n), sender: "engine".into(), sender_gen: 0, recipient: captain.id, kind: MsgKind::System, urgent: false, body: format!("lane {}: task {} has not pushed to {branch} for {minutes} min", seat.name, attempt.task), reply_to: None, needs_ack: false, state: MsgState::Info, created: now, meta: None })?;
    }
    tx.set_meta(&ask_key(&seat.id), &now.to_string())
}

/// A 20-minute push clock, persisted per seat across engine restarts.
pub fn tick(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<()> {
    for seat in tx.seats_of_project(project)? {
        let Some(attempt) = eligible(tx, &seat)? else { continue };
        let (branch, push) = last_push(tx, &seat, &attempt)?;
        let last = tx.get_meta(&ask_key(&seat.id))?.and_then(|s| s.parse::<Ts>().ok());
        if now.saturating_sub(push) > PUSH_INTERVAL && last.is_none_or(|t| now.saturating_sub(t) >= PUSH_INTERVAL) {
            notify(tx, &seat, &attempt, &branch, now.saturating_sub(push) / 60, now)?;
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
            if seat.generation != obs.seat.generation { return Ok(None) }
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
            if last.is_some_and(|t| now.saturating_sub(t) < LOOP_INTERVAL) { return Ok(false) }
            tx.set_meta(&loop_key(&seat.id), &now.to_string())?;
            tx.set_meta(&compact_key(&seat.id), "pending")?;
            Ok(true)
        })?;
        if !reserved { continue }
        if matches!(herdr.send_keys(&binding.pane_id, &["esc".into()]), Outcome::Refused { .. }) {
            store.write(|tx| tx.set_meta(&compact_key(&seat.id), ""))?;
            continue;
        }
        store.write(|tx| {
            tx.set_meta(&compact_key(&seat.id), "pending")?;
            notify(tx, &seat, &attempt, &branch, minutes, now)
        })?;
    }
    let _ = project;
    Ok(())
}

/// A delivery can have been intended before the loop was detected. Drive its
/// compaction too; execution holds the op until the marker is cleared.
pub fn prepare_pending_deliveries(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &crate::EngineCfg, project: &str, now: Ts) -> GearResult<()> {
    let pending = store.read(|tx| {
        let mut work = Vec::new();
        for op in tx.ops_in_state(project, gear_model::OpState::Intended)? {
            if op.kind != gear_model::OpKind::Deliver || !tx.get_meta(&compact_key(&op.seat))?.is_some_and(|s| !s.is_empty()) { continue; }
            let (Some(seat), Some(task), Some(attempt)) = (tx.get_seat(&op.seat)?, op.task, op.attempt) else { continue };
            if seat.generation == op.generation && seat.role == SeatRole::Lane { work.push((seat, task, attempt)); }
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
            tx.insert_contract(&ContractRow {project:"p".into(),task:"T-1".into(),version:1,goal:"g".into(),accept:"a".into(),evidence_files:vec![],evidence_due_min:None,body:String::new(),created:T})?;
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

    #[test]
    fn push_clock_reminds_once_and_a_newer_push_resets_it() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"], None);
        git(dir.path(), &["config", "user.email", "test@example.invalid"], None);
        git(dir.path(), &["config", "user.name", "test"], None);
        std::fs::write(dir.path().join("file"), "x").unwrap();
        git(dir.path(), &["add", "file"], None);
        git(dir.path(), &["commit", "-qm", "one"], Some("@1800001200 +0000"));
        let mut s = world(dir.path().to_str().unwrap());
        s.write(|tx| tick(tx, "p", T+1260)).unwrap();
        let lane = messages(&mut s, "quiet-lane:p:heat");
        let captain = messages(&mut s, "quiet:p:heat");
        assert_eq!(lane.len(), 1); assert!(lane[0].urgent);
        assert_eq!(lane[0].body, "No push to heat/t1 for 21 min. If anything in your task is unclear, send gear send --kind question now with your default; otherwise commit and push what builds.");
        assert_eq!(captain.len(), 1); assert_eq!(captain[0].state, MsgState::Info);
        s.write(|tx| tick(tx, "p", T+1261)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
        git(dir.path(), &["update-ref", "refs/remotes/origin/heat/t1", "HEAD"], None);
        s.write(|tx| tick(tx, "p", T+2400)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1, "a push resets the clock");
        s.write(|tx| tick(tx, "p", T+2401)).unwrap();
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 2);
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
    }

    fn observation(s: &mut Store) -> SeatObs {
        SeatObs { seat:s.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap(), presence:Presence::Present,
            status:Some(AgentStatus::Working), flags:Default::default(), idle:false, screen:None, session:None }
    }
    fn set_mtime(path: &std::path::Path, ts: i64) {
        std::fs::File::open(path).unwrap().set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(ts as u64))).unwrap();
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
        assert_eq!(messages(&mut s, "quiet:p:heat").len(), 1);
        assert_eq!(s.read(|tx| tx.get_meta(&compact_key("p:heat"))).unwrap().as_deref(), Some("pending"));
        loops(&mut s, &mut h, "p", &[obs], T+100+661).unwrap();
        assert_eq!(h.calls.iter().filter(|c| matches!(c, Call::SendKeys {keys, ..} if keys == &["esc"])).count(), 1);
        assert_eq!(messages(&mut s, "quiet-lane:p:heat").len(), 1);
    }

    #[test]
    fn fresh_or_unreadable_session_does_not_interrupt() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "session").unwrap(); set_mtime(&path, T+700);
        let mut s = world(dir.path().to_str().unwrap()); let path = path.to_str().unwrap();
        s.write(|tx| tx.set_binding_session_ref("p:heat", 1, path)).unwrap();
        let mut h = FakeHerdr::new(); h.add_agent_named(PANE, "pi", AgentStatus::Working, "p-heat"); h.set_agent_session(PANE, path);
        let obs = observation(&mut s);
        loops(&mut s, &mut h, "p", &[obs], T+760).unwrap();
        assert!(!h.calls.iter().any(|c| matches!(c, Call::SendKeys {..})));
        assert!(messages(&mut s, "quiet:p:heat").is_empty());
    }
}
