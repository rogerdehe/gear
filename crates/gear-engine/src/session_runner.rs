//! Persisted, bounded execution of a round's session action.
use crate::{
    session::{Action, Prep, Role},
    EngineCfg,
};
use gear_herdr::{AgentStatus, HerdrAct, Outcome, Profile};
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{NewMessage, SeatRow, Store};
use serde_json::{json, Value};

fn key(seat: &SeatRow) -> String {
    format!("engine.session_step.{}", seat.id)
}
fn ready(status: AgentStatus) -> bool {
    matches!(status, AgentStatus::Idle | AgentStatus::Done)
}
fn save(store: &mut Store, seat: &SeatRow, state: &Value) -> GearResult<()> {
    store.write(|tx| tx.set_meta(&key(seat), &state.to_string()))
}
fn str_field<'a>(state: &'a Value, field: &str) -> &'a str {
    state.get(field).and_then(Value::as_str).unwrap_or("")
}
fn time(state: &Value, field: &str) -> Ts {
    state.get(field).and_then(Value::as_i64).unwrap_or(0)
}

/// The operation's elapsed time in milliseconds, from the durable state: the
/// first step (`started`, written when the state was created — `t0` is
/// re-anchored when the action's own wait begins) (T-407).
fn elapsed(state: &Value, now: Ts) -> i64 {
    let started = match time(state, "started") {
        0 => match time(state, "t0") {
            0 => time(state, "step_at"),
            t0 => t0,
        },
        started => started,
    };
    now.saturating_sub(started).saturating_mul(1000)
}

/// One probe line for the step that ends here, and the whole operation so far
/// (T-407): a step is measured from the `step_at` it entered with to now.
fn probe_step(seat: &SeatRow, state: &Value, step: &str, now: Ts) {
    let step_ms = now
        .saturating_sub(time(state, "step_at"))
        .saturating_mul(1000);
    crate::ops::probe(
        str_field(state, "action"),
        &seat.id,
        step,
        step_ms,
        elapsed(state, now),
    );
}

fn step(seat: &SeatRow, state: &mut Value, name: &str, now: Ts) {
    // T-407: the step that leaves here is one line of the engine log. The
    // times are the durable ones, so a restart in the middle still logs them.
    let previous = str_field(state, "step").to_string();
    if !previous.is_empty() && previous != name {
        probe_step(seat, state, &previous, now);
    }
    state["step"] = json!(name);
    state["step_at"] = json!(now);
}

/// Press Enter on the text already in the box; `true` when Herdr took it.
/// The step that types a session command submits it here, in the same call
/// (T-434): a tick that runs long or an agent that looks busy after the text
/// can no longer leave `/new` or `/quit` typed and unsent.
fn enter(herdr: &mut dyn HerdrAct, pane: &str) -> bool {
    herdr.type_settle();
    !matches!(
        herdr.send_keys(pane, &["enter".into()]),
        Outcome::Refused { .. }
    )
}

/// How many lines of the pane the typed command is looked for in (T-453).
const COMPOSER_READ_LINES: u32 = 12;

/// Whether `text` still sits in the pane's input box (T-453): the Enter did not
/// take. A failed read says nothing is held (the next call types).
fn composer_holds(herdr: &mut dyn HerdrAct, pane: &str, text: &str) -> bool {
    herdr
        .read_tail(pane, COMPOSER_READ_LINES)
        .map(|tail| crate::composer::holds(&tail, text))
        .unwrap_or(false)
}

/// Type `command` into the pane and submit it (T-434), then confirm the Enter
/// took: the command no longer shows in the composer (T-453). A refused type
/// changes nothing. `false` = the text is still in the box; the step waits at
/// `enter` and tries again on a later tick.
fn type_and_submit(herdr: &mut dyn HerdrAct, pane: &str, command: &str) -> Option<bool> {
    if !composer_holds(herdr, pane, command)
        && matches!(herdr.pane_type(pane, command), Outcome::Refused { .. })
    {
        return None;
    }
    Some(enter(herdr, pane) && !composer_holds(herdr, pane, command))
}

/// The step a submitted session command waits in, with the `t0` and the
/// compact `offset` that belong to this submission (T-434).
fn submitted(seat: &SeatRow, state: &mut Value, action_name: &str, now: Ts) {
    step(
        seat,
        state,
        if action_name == "resume" {
            "wait_exit"
        } else {
            "wait_action"
        },
        now,
    );
    state["t0"] = json!(now);
    if action_name == "compact" {
        state["offset"] = json!(state
            .get("old_session_ref")
            .and_then(Value::as_str)
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|m| m.len())
            .unwrap_or(0));
    }
}

fn record(
    store: &mut Store,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
    role: Role,
    action: &str,
    state: &mut Value,
    now: Ts,
) -> GearResult<Prep> {
    step(seat, state, "done", now);
    // T-407: the operation's own line, with the whole duration in it.
    let total = elapsed(state, now);
    crate::ops::probe(str_field(state, "action"), &seat.id, "done", total, total);
    state["result"] = json!(action);
    store.write(|tx| {
        if matches!(role, Role::Review | Role::Direction) {
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
        let Some(captain) = tx.captain_of(&seat.project)? else {
            return Ok(());
        };
        tx.insert_message(&NewMessage {
            meta: None,
            project: seat.project.clone(),
            thread: format!("session:{}", seat.id),
            task: None,
            attempt: None,
            sender: "engine".into(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: false,
            body: format!(
                "/new did not change the session id of {}; check the profile",
                seat.id
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        Ok(())
    })
}

fn recovery_failed(
    store: &mut Store,
    seat: &SeatRow,
    state: &mut Value,
    phase: &str,
    now: Ts,
) -> GearResult<Prep> {
    let reason = format!("session resume recovery {phase} timed out; the agent needs checking");
    // Temp seats use existing lifecycle transitions to reach NeedsCheck without
    // typing into or closing a possibly working agent.
    store.write(|tx| {
        let row = tx.get_seat(&seat.id)?.ok_or_else(|| {
            gear_model::GearError::new(gear_model::ErrorCode::NoTarget, "session seat disappeared")
        })?;
        let mut current = row.state;
        if row.role == SeatRole::Temp {
            for next in match current {
                SeatState::Queued | SeatState::Released => {
                    vec![SeatState::Starting, SeatState::Unknown]
                }
                SeatState::Starting => vec![SeatState::Unknown],
                SeatState::Standby => vec![SeatState::Releasing, SeatState::Unknown],
                SeatState::Releasing => vec![SeatState::Unknown],
                _ => vec![],
            } {
                gear_model::state::require_transition(
                    SeatState::can_transition(row.role, current, next),
                    "seat",
                    current.as_str(),
                    next.as_str(),
                )?;
                tx.set_seat_state(&seat.id, next, Some(&reason), now)?;
                current = next;
            }
        }
        if current != SeatState::NeedsCheck {
            gear_model::state::require_transition(
                SeatState::can_transition(row.role, current, SeatState::NeedsCheck),
                "seat",
                current.as_str(),
                SeatState::NeedsCheck.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::NeedsCheck, Some(&reason), now)?;
            if let Some(captain) = tx.captain_of(&seat.project)? {
                tx.insert_message(&NewMessage {
                    meta: None,
                    project: seat.project.clone(),
                    thread: format!("session:{}", seat.id),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: captain.id,
                    kind: MsgKind::System,
                    urgent: true,
                    body: format!("{}: {reason}", seat.id),
                    reply_to: None,
                    needs_ack: true,
                    state: MsgState::Open,
                    created: now,
                })?;
            }
        }
        step(seat, state, "failed", now);
        state["failure"] = json!(reason);
        tx.set_meta(&key(seat), &state.to_string())
    })?;
    Ok(Prep::Wait)
}

fn profile(
    store: &mut Store,
    cfg: &EngineCfg,
    seat: &SeatRow,
    kind: &str,
) -> GearResult<Option<Profile>> {
    let project = store.read(|tx| tx.get_project(&seat.project))?;
    let mut profiles = cfg.profiles.clone();
    if let Some(config) = project.and_then(|p| gear_app::config::parse(&p.config_toml).ok()) {
        profiles = gear_app::config::profile_table(&profiles, &config)?;
    }
    Ok(profiles
        .get(&seat.profile)
        .or_else(|| profiles.values().find(|p| p.kind == kind))
        .cloned())
}

/// Drive one action by at most one terminal submission or state transition.
pub(crate) fn run(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
    role: Role,
    action: &Action,
    compact_after: bool,
    now: Ts,
) -> GearResult<Prep> {
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    let Some(binding) = binding else {
        return Ok(Prep::Wait);
    };
    let Some(profile) = profile(store, cfg, seat, &binding.kind)? else {
        return Ok(Prep::Wait);
    };
    let identity = format!("{}:{task}:{attempt}:{}", seat.project, role.as_str());
    let stored = store
        .read(|tx| tx.get_meta(&key(seat)))?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|s| {
            str_field(s, "round") == identity && s["generation"].as_i64() == Some(seat.generation)
        });
    let occupant = match herdr.agent_get(&binding.pane_id) {
        Ok(value) => value,
        Err(_) => {
            if let Some(mut state) = stored {
                let phase = str_field(&state, "step").to_string();
                if (phase == "wait_exit" && now.saturating_sub(time(&state, "step_at")) >= 30)
                    || (phase == "wait_resume" && now.saturating_sub(time(&state, "step_at")) >= 60)
                {
                    step(seat, &mut state, "fallback_exit", now);
                    save(store, seat, &state)?;
                    return Ok(Prep::Wait);
                }
                let limit = match phase.as_str() {
                    "fallback_exit" | "fallback_enter" | "fallback_wait_exit" => Some(30),
                    "start_fallback" | "wait_fallback" => Some(60),
                    _ => None,
                };
                if limit.is_some_and(|limit| now.saturating_sub(time(&state, "step_at")) >= limit) {
                    return recovery_failed(store, seat, &mut state, &phase, now);
                }
            }
            return Ok(Prep::Wait);
        }
    };
    let existing = stored.filter(|state| {
        if str_field(state, "step") != "done" || str_field(state, "result") == "resume-fallback" {
            return true;
        }
        match action {
            Action::Resume(target) if !target.is_empty() => {
                occupant.is_some() && binding.session_ref.as_deref() == Some(target.as_str())
            }
            _ => true,
        }
    });
    let mut state = if let Some(state) = existing {
        state
    } else {
        let target = match action {
            Action::Resume(s) => Some(s.as_str()),
            _ => None,
        };
        let old = binding.session_ref.as_deref().filter(|s| !s.is_empty());
        let offset = old
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        let state = json!({"round":identity, "generation":seat.generation, "action":action.as_str(), "t0":now, "started":now, "step_at":now, "old_session_ref":old, "target":target, "step":"initial", "offset":offset, "saw_working":false, "compact_after":compact_after, "initial_done":occupant.as_ref().is_some_and(|a| a.agent_status == AgentStatus::Done)});
        save(store, seat, &state)?;
        state
    };
    let action_name = str_field(&state, "action").to_string();
    let phase = str_field(&state, "step").to_string();
    let is_ready = occupant.as_ref().is_some_and(|a| ready(a.agent_status));
    // T-290: `keep` and `fresh` do no pane work, so an agent that queues input
    // typed while it works (pi, claude) is ready mid-turn — the packet waits in
    // its input queue and is read after the current step. Every other action
    // types into the pane and still waits for idle, as does every other kind.
    let mid_turn_ready = occupant.as_ref().is_some_and(|a| {
        a.agent_status == AgentStatus::Working && gear_herdr::queues_input(&binding.kind)
    });
    if phase == "done" {
        let done = is_ready
            || (occupant.is_none() && str_field(&state, "result") == "fresh")
            || (mid_turn_ready && matches!(action_name.as_str(), "keep" | "fresh"));
        return Ok(if done { Prep::Ready } else { Prep::Wait });
    }
    if phase == "failed" {
        return Ok(Prep::Wait);
    }
    if occupant
        .as_ref()
        .is_some_and(|a| a.agent_status == AgentStatus::Working)
        && action_name == "compact"
        && !state["saw_working"].as_bool().unwrap_or(false)
    {
        state["saw_working"] = json!(true);
        save(store, seat, &state)?;
    }
    if action_name == "keep" || action_name == "fresh" {
        return if occupant.is_none() || is_ready || mid_turn_ready {
            record(
                store,
                seat,
                task,
                attempt,
                role,
                &action_name,
                &mut state,
                now,
            )
        } else {
            Ok(Prep::Wait)
        };
    }
    if phase == "initial" {
        if action_name == "resume" && occupant.is_none() {
            step(seat, &mut state, "start_resume", now);
            save(store, seat, &state)?;
            return Ok(Prep::Wait);
        }
        if !is_ready {
            return Ok(Prep::Wait);
        }
        let command = match action_name.as_str() {
            "new" => profile.new_session.as_str(),
            "compact" => profile.compact.as_str(),
            "resume" => profile.exit_command.as_deref().unwrap_or(""),
            _ => return Ok(Prep::Wait),
        };
        if action_name == "resume" && command.is_empty() {
            if matches!(
                herdr.send_keys(&binding.pane_id, &profile.exit_keys),
                Outcome::Refused { .. }
            ) {
                return Ok(Prep::Wait);
            }
            step(seat, &mut state, "wait_exit", now);
        } else if command.is_empty() {
            return record(
                store,
                seat,
                task,
                attempt,
                role,
                if action_name == "compact" {
                    "compact-fallback"
                } else {
                    "new-unconfirmed"
                },
                &mut state,
                now,
            );
        } else {
            // T-434: type and Enter in this same step; T-453: the next step
            // goes only when the composer emptied — a command left in the box
            // would join the packet typed after it.
            let Some(took) = type_and_submit(herdr, &binding.pane_id, command) else {
                return Ok(Prep::Wait);
            };
            if took {
                submitted(seat, &mut state, &action_name, now)
            } else {
                step(seat, &mut state, "enter", now);
            }
        }
        save(store, seat, &state)?;
        return Ok(Prep::Wait);
    }
    if phase == "enter" {
        if action_name == "resume" && occupant.is_none() {
            step(seat, &mut state, "start_resume", now);
            save(store, seat, &state)?;
            return Ok(Prep::Wait);
        }
        // T-434: the text is already in the box, so a later Enter does not
        // wait for the agent to be idle or done again. T-453: the step moves
        // on only when the composer emptied.
        let command = match action_name.as_str() {
            "new" => profile.new_session.as_str(),
            "compact" => profile.compact.as_str(),
            "resume" => profile.exit_command.as_deref().unwrap_or(""),
            _ => "",
        };
        if enter(herdr, &binding.pane_id) && !composer_holds(herdr, &binding.pane_id, command) {
            submitted(seat, &mut state, &action_name, now);
            save(store, seat, &state)?;
        }
        return Ok(Prep::Wait);
    }
    if phase == "wait_action" {
        // T-457: Codex 0.165 opens "Where should the new conversation run?"
        // after `/new`. Herdr reports the agent done/idle while the dialog
        // waits, and the text of the next delivery would land in it. The
        // profile's answer keys take it (Enter keeps the seat in its current
        // worktree); the step waits a tick for the conversation to start.
        if action_name == "new"
            && matches!(
                gear_herdr::answer_dialog(herdr, &binding.pane_id, &profile),
                Some(Outcome::Ok(()))
            )
        {
            save(store, seat, &state)?;
            return Ok(Prep::Wait);
        }
        let elapsed = now.saturating_sub(time(&state, "t0"));
        if action_name == "compact"
            && occupant
                .as_ref()
                .is_some_and(|a| a.agent_status != AgentStatus::Done)
            && !state["seen_not_done"].as_bool().unwrap_or(false)
        {
            state["seen_not_done"] = json!(true);
            save(store, seat, &state)?;
        }
        if action_name == "new" {
            let old = state.get("old_session_ref").and_then(Value::as_str);
            let changed = binding
                .session_ref
                .as_deref()
                .filter(|s| !s.is_empty())
                .is_some_and(|s| Some(s) != old);
            if is_ready && elapsed >= 5 && (changed || binding.kind == "codex") {
                return record(
                    store,
                    seat,
                    task,
                    attempt,
                    role,
                    if changed { "new" } else { "new-unconfirmed" },
                    &mut state,
                    now,
                );
            }
            if is_ready && elapsed >= 30 {
                if binding.kind != "codex" {
                    notify_new(store, seat, now)?;
                }
                return record(
                    store,
                    seat,
                    task,
                    attempt,
                    role,
                    "new-unconfirmed",
                    &mut state,
                    now,
                );
            }
        } else {
            let session = state.get("old_session_ref").and_then(Value::as_str);
            let log_new = session.is_some_and(|s| {
                new_compaction(s, state["offset"].as_u64().unwrap_or(0))
                    || profile
                        .usage_log
                        .as_deref()
                        .and_then(gear_usage::UsageLog::from_name)
                        .and_then(|log| {
                            gear_usage::usage(
                                log,
                                s,
                                time(&state, "t0").saturating_add(1),
                                now.saturating_add(1),
                            )
                        })
                        .is_some_and(|u| u.records > 0)
            });
            let done = occupant
                .as_ref()
                .is_some_and(|a| a.agent_status == AgentStatus::Done)
                && (!state["initial_done"].as_bool().unwrap_or(false)
                    || state["seen_not_done"].as_bool().unwrap_or(false));
            let small_pi = binding.kind == "pi"
                && elapsed >= 15
                && !state["saw_working"].as_bool().unwrap_or(false);
            if is_ready && (done || log_new || small_pi) {
                return record(
                    store,
                    seat,
                    task,
                    attempt,
                    role,
                    if small_pi && !done && !log_new {
                        "keep"
                    } else {
                        "compact"
                    },
                    &mut state,
                    now,
                );
            }
            if is_ready && elapsed >= 300 {
                return record(
                    store,
                    seat,
                    task,
                    attempt,
                    role,
                    "compact-fallback",
                    &mut state,
                    now,
                );
            }
        }
        return Ok(Prep::Wait);
    }
    if phase == "wait_exit" {
        if occupant.is_none() {
            step(seat, &mut state, "start_resume", now);
            save(store, seat, &state)?;
        } else if now.saturating_sub(time(&state, "step_at")) >= 30 {
            step(seat, &mut state, "fallback_exit", now);
            save(store, seat, &state)?;
        }
        return Ok(Prep::Wait);
    }
    if phase == "fallback_exit" {
        if occupant.is_none() {
            step(seat, &mut state, "start_fallback", now);
            save(store, seat, &state)?;
        } else if now.saturating_sub(time(&state, "step_at")) >= 30 {
            return recovery_failed(store, seat, &mut state, &phase, now);
        } else if is_ready {
            if let Some(command) = profile.exit_command.as_deref().filter(|s| !s.is_empty()) {
                if !matches!(
                    herdr.pane_type(&binding.pane_id, command),
                    Outcome::Refused { .. }
                ) {
                    // T-434: the exit command leaves with its Enter too;
                    // "fallback_enter" stays for a refused Enter. T-453: the
                    // step moves on only when the composer emptied.
                    if enter(herdr, &binding.pane_id)
                        && !composer_holds(herdr, &binding.pane_id, command)
                    {
                        step(seat, &mut state, "fallback_wait_exit", now)
                    } else {
                        step(seat, &mut state, "fallback_enter", now);
                    }
                    save(store, seat, &state)?;
                }
            } else if !matches!(
                herdr.send_keys(&binding.pane_id, &profile.exit_keys),
                Outcome::Refused { .. }
            ) {
                step(seat, &mut state, "fallback_wait_exit", now);
                save(store, seat, &state)?;
            }
        }
        return Ok(Prep::Wait);
    }
    if phase == "fallback_enter" {
        if occupant.is_none() {
            step(seat, &mut state, "start_fallback", now);
            save(store, seat, &state)?;
            return Ok(Prep::Wait);
        }
        if now.saturating_sub(time(&state, "step_at")) >= 30 {
            return recovery_failed(store, seat, &mut state, &phase, now);
        }
        if is_ready
            && enter(herdr, &binding.pane_id)
            && !composer_holds(
                herdr,
                &binding.pane_id,
                profile.exit_command.as_deref().unwrap_or(""),
            )
        {
            step(seat, &mut state, "fallback_wait_exit", now);
            save(store, seat, &state)?;
        }
        return Ok(Prep::Wait);
    }
    if phase == "fallback_wait_exit" {
        if occupant.is_none() {
            step(seat, &mut state, "start_fallback", now);
            save(store, seat, &state)?;
        } else if now.saturating_sub(time(&state, "step_at")) >= 30 {
            return recovery_failed(store, seat, &mut state, &phase, now);
        }
        return Ok(Prep::Wait);
    }
    if phase == "start_resume" || phase == "start_fallback" {
        if phase == "start_fallback" && now.saturating_sub(time(&state, "step_at")) >= 60 {
            return recovery_failed(store, seat, &mut state, &phase, now);
        }
        if occupant.is_some() {
            return Ok(Prep::Wait);
        }
        let mut args = profile.unattended_args.clone();
        args.extend(crate::rules::arguments(
            store,
            &seat.project,
            seat.role,
            &seat.name,
            &cfg.db_path,
            &profile,
            phase == "start_resume",
        )?);
        let target = str_field(&state, "target");
        if phase == "start_resume" {
            let Some(resume) = profile.resume_args.as_ref().filter(|_| !target.is_empty()) else {
                step(seat, &mut state, "start_fallback", now);
                save(store, seat, &state)?;
                return Ok(Prep::Wait);
            };
            args.extend(resume.iter().map(|s| s.replace("{session}", target)));
        }
        let (model, project) = store.read(|tx| {
            Ok((
                tx.get_meta(&crate::model::model_meta_key(&seat.id))?,
                tx.get_project(&seat.project)?,
            ))
        })?;
        if let Some(config) = project.and_then(|p| gear_app::config::parse(&p.config_toml).ok()) {
            let entries = match seat.role {
                SeatRole::Lane => config
                    .lanes
                    .iter()
                    .find(|lane| lane.name == seat.name)
                    .map(|lane| gear_app::config::lane_chain(&config, lane))
                    .unwrap_or(&config.models.lanes),
                SeatRole::Temp => config.models.review.as_slice(),
                SeatRole::Captain => config.models.captain.as_slice(),
            };
            let entry = match model.as_deref() {
                Some(model) => entries.iter().find(|e| e.model() == model),
                None => entries.first(),
            };
            if let Some(start) = entry.and_then(|e| crate::model::entry_args(&binding.kind, e)) {
                args.extend(start);
            } else if let Some(start) = model
                .as_deref()
                .and_then(|model| crate::model::model_start(&binding.kind, model))
            {
                args.extend(start.args);
            }
        } else if let Some(model) = model {
            if let Some(start) = crate::model::model_start(&binding.kind, &model) {
                args.extend(start.args);
            }
        }
        let folder_key = if matches!(role, Role::Review | Role::Direction) {
            format!("engine.review_path.{}", seat.id)
        } else {
            crate::onboard::worktree_path_key(&seat.id)
        };
        if let Some(folder) = store.read(|tx| tx.get_meta(&folder_key))? {
            args = gear_herdr::render_args(&args, &folder);
        }
        let name = crate::names::desired(seat);
        match herdr.agent_start(&name, &binding.kind, &binding.pane_id, &args) {
            Outcome::Ok(_) | Outcome::Uncertain { .. } => step(
                seat,
                &mut state,
                if phase == "start_resume" {
                    "wait_resume"
                } else {
                    "wait_fallback"
                },
                now,
            ),
            Outcome::Refused { .. } if phase == "start_resume" => {
                step(seat, &mut state, "start_fallback", now)
            }
            Outcome::Refused { .. } => return Ok(Prep::Wait),
        }
        save(store, seat, &state)?;
        return Ok(Prep::Wait);
    }
    if phase == "wait_resume" || phase == "wait_fallback" {
        // T-457: a resume from another directory asks which working directory
        // to use. The start args keep it from showing
        // (`tui.resume_cwd="current"` — the seat's own directory); if it does
        // show, the profile answers it with the preselected current directory.
        if phase == "wait_resume"
            && matches!(
                gear_herdr::answer_dialog(herdr, &binding.pane_id, &profile),
                Some(Outcome::Ok(()))
            )
        {
            save(store, seat, &state)?;
            return Ok(Prep::Wait);
        }
        if is_ready {
            if phase == "wait_resume" && state["compact_after"].as_bool().unwrap_or(false) {
                state["action"] = json!("compact");
                state["old_session_ref"] = state["target"].clone();
                state["offset"] = json!(str_field(&state, "target")
                    .is_empty()
                    .then_some(0)
                    .unwrap_or_else(|| std::fs::metadata(str_field(&state, "target"))
                        .map(|m| m.len())
                        .unwrap_or(0)));
                state["saw_working"] = json!(false);
                state["initial_done"] = json!(occupant
                    .as_ref()
                    .is_some_and(|a| a.agent_status == AgentStatus::Done));
                step(seat, &mut state, "initial", now);
                save(store, seat, &state)?;
                return Ok(Prep::Wait);
            }
            return record(
                store,
                seat,
                task,
                attempt,
                role,
                if phase == "wait_resume" {
                    "resume"
                } else {
                    "resume-fallback"
                },
                &mut state,
                now,
            );
        }
        if phase == "wait_resume" && now.saturating_sub(time(&state, "step_at")) >= 60 {
            step(
                seat,
                &mut state,
                if occupant.is_none() {
                    "start_fallback"
                } else {
                    "fallback_exit"
                },
                now,
            );
            save(store, seat, &state)?;
        }
        if phase == "wait_fallback" && now.saturating_sub(time(&state, "step_at")) >= 60 {
            return recovery_failed(store, seat, &mut state, &phase, now);
        }
    }
    Ok(Prep::Wait)
}

fn new_compaction(path: &str, offset: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return false;
    }
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        return false;
    }
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|value| value.get("type").and_then(Value::as_str) == Some("compaction"))
}
