//! Persisted, bounded execution of a round's session action.
use gear_herdr::{AgentStatus, HerdrAct, Outcome, Profile};
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, Ts};
use gear_store::{NewMessage, SeatRow, Store};
use serde_json::{json, Value};
use crate::{EngineCfg, session::{Action, Prep, Role}};

fn key(seat: &SeatRow) -> String { format!("engine.session_step.{}", seat.id) }
fn ready(status: AgentStatus) -> bool { matches!(status, AgentStatus::Idle | AgentStatus::Done) }
fn save(store: &mut Store, seat: &SeatRow, state: &Value) -> GearResult<()> {
    store.write(|tx| tx.set_meta(&key(seat), &state.to_string()))
}
fn str_field<'a>(state: &'a Value, field: &str) -> &'a str { state.get(field).and_then(Value::as_str).unwrap_or("") }
fn time(state: &Value, field: &str) -> Ts { state.get(field).and_then(Value::as_i64).unwrap_or(0) }
fn step(state: &mut Value, name: &str, now: Ts) { state["step"] = json!(name); state["step_at"] = json!(now); }

fn record(store: &mut Store, seat: &SeatRow, task: &str, attempt: i64, role: Role, action: &str, state: &mut Value, now: Ts) -> GearResult<Prep> {
    step(state, "done", now);
    state["result"] = json!(action);
    store.write(|tx| {
        if role == Role::Review {
            let value = json!({"action": action, "session_ref": state.get("target").cloned().unwrap_or(Value::Null)});
            tx.set_meta(&format!("engine.review_action.{}.{task}.{attempt}", seat.project), &value.to_string())?;
        } else {
            tx.set_attempt_action(&seat.project, task, attempt, action)?;
        }
        tx.set_meta(&key(seat), &state.to_string())
    })?;
    Ok(Prep::Ready)
}

fn notify_new(store: &mut Store, seat: &SeatRow, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let Some(captain) = tx.captain_of(&seat.project)? else { return Ok(()) };
        tx.insert_message(&NewMessage { meta: None, project: seat.project.clone(), thread: format!("session:{}", seat.id), task: None, attempt: None,
            sender: "engine".into(), sender_gen: 0, recipient: captain.id, kind: MsgKind::System, urgent: false,
            body: format!("/new did not change the session id of {}; check the profile", seat.id), reply_to: None, needs_ack: false, state: MsgState::Open, created: now })?;
        Ok(())
    })
}

fn profile<'a>(cfg: &'a EngineCfg, seat: &SeatRow, kind: &str) -> Option<&'a Profile> {
    cfg.profiles.get(&seat.profile).or_else(|| cfg.profiles.values().find(|p| p.kind == kind))
}

/// Drive one action by at most one terminal submission or state transition.
pub(crate) fn run(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &EngineCfg, seat: &SeatRow, task: &str, attempt: i64, role: Role, action: &Action, now: Ts) -> GearResult<Prep> {
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    let Some(binding) = binding else { return Ok(Prep::Wait) };
    let Some(profile) = profile(cfg, seat, &binding.kind) else { return Ok(Prep::Wait) };
    let occupant = match herdr.agent_get(&binding.pane_id) { Ok(value) => value, Err(_) => return Ok(Prep::Wait) };
    let identity = format!("{}:{task}:{attempt}:{}", seat.project, role.as_str());
    let existing = store.read(|tx| tx.get_meta(&key(seat)))?.and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(|s| str_field(s, "round") == identity);
    let mut state = if let Some(state) = existing { state } else {
        let target = match action { Action::Resume(s) => Some(s.as_str()), _ => None };
        let old = binding.session_ref.as_deref().filter(|s| !s.is_empty());
        let offset = old.and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0);
        let state = json!({"round":identity, "action":action.as_str(), "t0":now, "step_at":now, "old_session_ref":old, "target":target, "step":"initial", "offset":offset, "saw_working":false});
        save(store, seat, &state)?;
        state
    };
    let action_name = str_field(&state, "action").to_string();
    let phase = str_field(&state, "step").to_string();
    if phase == "done" { return Ok(Prep::Ready) }
    let is_ready = occupant.as_ref().is_some_and(|a| ready(a.agent_status));
    if occupant.as_ref().is_some_and(|a| a.agent_status == AgentStatus::Working) && action_name == "compact" && !state["saw_working"].as_bool().unwrap_or(false) {
        state["saw_working"] = json!(true); save(store, seat, &state)?;
    }
    if action_name == "keep" || action_name == "fresh" {
        return if occupant.is_none() || is_ready { record(store, seat, task, attempt, role, &action_name, &mut state, now) } else { Ok(Prep::Wait) };
    }
    if phase == "initial" {
        if action_name == "resume" && occupant.is_none() { step(&mut state, "start_resume", now); save(store, seat, &state)?; return Ok(Prep::Wait) }
        if !is_ready { return Ok(Prep::Wait) }
        let command = match action_name.as_str() { "new" => profile.new_session.as_str(), "compact" => profile.compact.as_str(), "resume" => profile.exit_command.as_deref().unwrap_or(""), _ => return Ok(Prep::Wait) };
        if action_name == "resume" && command.is_empty() {
            if matches!(herdr.send_keys(&binding.pane_id, &profile.exit_keys), Outcome::Refused { .. }) { return Ok(Prep::Wait) }
            step(&mut state, "wait_exit", now);
        } else if command.is_empty() {
            return record(store, seat, task, attempt, role, if action_name == "compact" { "compact-fallback" } else { "new-unconfirmed" }, &mut state, now);
        } else {
            if matches!(herdr.pane_type(&binding.pane_id, command), Outcome::Refused { .. }) { return Ok(Prep::Wait) }
            step(&mut state, "enter", now);
        }
        save(store, seat, &state)?; return Ok(Prep::Wait);
    }
    if phase == "enter" {
        if !is_ready { return Ok(Prep::Wait) }
        herdr.type_settle();
        if matches!(herdr.send_keys(&binding.pane_id, &["enter".into()]), Outcome::Refused { .. }) { return Ok(Prep::Wait) }
        step(&mut state, if action_name == "resume" { "wait_exit" } else { "wait_action" }, now);
        state["t0"] = json!(now);
        save(store, seat, &state)?; return Ok(Prep::Wait);
    }
    if phase == "wait_action" {
        let elapsed = now.saturating_sub(time(&state, "t0"));
        if action_name == "new" {
            let old = state.get("old_session_ref").and_then(Value::as_str);
            let changed = binding.session_ref.as_deref().filter(|s| !s.is_empty()).is_some_and(|s| Some(s) != old);
            if is_ready && elapsed >= 5 && (changed || binding.kind == "codex") {
                return record(store, seat, task, attempt, role, if changed { "new" } else { "new-unconfirmed" }, &mut state, now);
            }
            if is_ready && elapsed >= 30 {
                if binding.kind != "codex" { notify_new(store, seat, now)?; }
                return record(store, seat, task, attempt, role, "new-unconfirmed", &mut state, now);
            }
        } else {
            let session = state.get("old_session_ref").and_then(Value::as_str);
            let log_new = session.is_some_and(|s| new_compaction(s, state["offset"].as_u64().unwrap_or(0)) || profile.usage_log.as_deref().and_then(gear_usage::UsageLog::from_name).and_then(|log| gear_usage::usage(log, s, time(&state,"t0"), now.saturating_add(1))).is_some_and(|u| u.records > 0));
            let done = occupant.as_ref().is_some_and(|a| a.agent_status == AgentStatus::Done);
            let small_pi = binding.kind == "pi" && elapsed >= 15 && !state["saw_working"].as_bool().unwrap_or(false);
            if is_ready && (done || log_new || small_pi) { return record(store, seat, task, attempt, role, "compact", &mut state, now) }
            if is_ready && elapsed >= 300 { return record(store, seat, task, attempt, role, "compact-fallback", &mut state, now) }
        }
        return Ok(Prep::Wait);
    }
    if phase == "wait_exit" {
        if occupant.is_none() { step(&mut state, "start_resume", now); save(store, seat, &state)?; }
        else if now.saturating_sub(time(&state, "step_at")) >= 30 { step(&mut state, "fallback_exit", now); save(store, seat, &state)?; }
        return Ok(Prep::Wait);
    }
    if phase == "fallback_exit" {
        if occupant.is_none() { step(&mut state, "start_fallback", now); save(store, seat, &state)?; }
        return Ok(Prep::Wait);
    }
    if phase == "start_resume" || phase == "start_fallback" {
        if occupant.is_some() { return Ok(Prep::Wait) }
        let mut args = profile.unattended_args.clone();
        let target = str_field(&state, "target");
        if phase == "start_resume" {
            let Some(resume) = profile.resume_args.as_ref().filter(|_| !target.is_empty()) else { step(&mut state, "start_fallback", now); save(store, seat, &state)?; return Ok(Prep::Wait) };
            args.extend(resume.iter().map(|s| s.replace("{session}", target)));
        }
        if let Some(model) = store.read(|tx| tx.get_meta(&crate::model::model_meta_key(&seat.id)))? {
            if let Some(start) = crate::model::model_start(&binding.kind, &model) { args.extend(start.args); }
        }
        let folder_key = if role == Role::Review { format!("engine.review_path.{}", seat.id) } else { crate::onboard::worktree_path_key(&seat.id) };
        if let Some(folder) = store.read(|tx| tx.get_meta(&folder_key))? { args = gear_herdr::render_args(&args, &folder); }
        let name = if seat.role == SeatRole::Temp { crate::names::review_agent_name(&seat.project, task) } else { crate::names::desired(seat) };
        match herdr.agent_start(&name, &binding.kind, &binding.pane_id, &args) {
            Outcome::Ok(_) | Outcome::Uncertain { .. } => step(&mut state, if phase == "start_resume" { "wait_resume" } else { "wait_fallback" }, now),
            Outcome::Refused { .. } if phase == "start_resume" => step(&mut state, "start_fallback", now),
            Outcome::Refused { .. } => return Ok(Prep::Wait),
        }
        save(store, seat, &state)?; return Ok(Prep::Wait);
    }
    if phase == "wait_resume" || phase == "wait_fallback" {
        if is_ready { return record(store, seat, task, attempt, role, if phase == "wait_resume" { "resume" } else { "resume-fallback" }, &mut state, now) }
        if phase == "wait_resume" && now.saturating_sub(time(&state, "step_at")) >= 60 {
            step(&mut state, if occupant.is_none() { "start_fallback" } else { "fallback_exit" }, now); save(store, seat, &state)?;
        }
    }
    Ok(Prep::Wait)
}

fn new_compaction(path: &str, offset: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    if file.seek(SeekFrom::Start(offset)).is_err() { return false }
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() { return false }
    text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).any(|value| value.get("type").and_then(Value::as_str) == Some("compaction"))
}
