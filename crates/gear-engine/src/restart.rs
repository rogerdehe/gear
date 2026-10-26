//! Changing a lane's **CLI** (T-35): the quota fallback needs a model that
//! runs on another CLI.
//!
//! The engine waits for the agent to stop, closes it, starts the next CLI in
//! the same pane and worktree with the arguments of its chain entry, records
//! the model and delivers "Continue T-<n>". No session crosses CLIs: the new
//! agent starts from the git state of the pane, and the lane is told to carry
//! on with its task.
//!
//! The restart is a **state machine across ticks** (like the in-place switch
//! of T-33): the plan (the target model, the CLI, its arguments and the
//! phase) is durable in the seat's meta before the first key, so an engine
//! restart in the middle continues the plan instead of starting a second
//! agent.
//!
//! The seat leaves `Active` **before** its agent goes: a seat whose agent is
//! gone reaches the captain (L-56), and a planned restart is not a loss. The
//! seat comes back as `Active` when the new agent is up.

use gear_app::config::ModelEntry;
use gear_herdr::{AgentStatus, HerdrAct, Outcome, Profile};
use gear_model::state::require_transition;
use gear_model::{ErrorCode, GearError, GearResult, OpKind, OpState, SeatState, Ts};
use gear_store::{BindingRow, BindingStatus, SeatRow, Store};

use crate::model::{model_meta_key, model_start, start_args};
use crate::EngineCfg;

/// The phases of a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The agent must be stopped before it is closed.
    Stop,
    /// The exit keys were sent (or may have been): the agent must be gone.
    Exit,
    /// The agent is gone: the new CLI is started in the same pane.
    Start,
    /// The new agent is up: the model, the seat and the Continue are booked.
    Finish,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Stop => "stop",
            Phase::Exit => "exit",
            Phase::Start => "start",
            Phase::Finish => "finish",
        }
    }

    fn parse(text: &str) -> Option<Phase> {
        match text {
            "stop" => Some(Phase::Stop),
            "exit" => Some(Phase::Exit),
            "start" => Some(Phase::Start),
            "finish" => Some(Phase::Finish),
            _ => None,
        }
    }
}

/// The plan of a restart, durable in the seat's meta before the first key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The model the seat will run on.
    pub model: String,
    /// The CLI that runs it (the Herdr agent kind).
    pub cli: String,
    /// The arguments of the chain entry, after the model flag.
    pub args: Vec<String>,
    /// The generation the new agent gets; 0 while it is not reserved.
    pub generation: i64,
    pub phase: Phase,
    /// How many ticks the current phase has waited.
    pub waited: i64,
}

/// How many ticks a phase may wait before the seat needs a human. A pane
/// that never becomes a shell, or an agent that never goes, must not hang the
/// lane forever.
const MAX_WAIT: i64 = 30;

/// The plan of a seat's restart, if one is unfinished.
pub fn pending(store: &mut Store, seat_id: &str) -> GearResult<Option<Plan>> {
    let Some(raw) = meta_get(store, &plan_key(seat_id))? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let Ok(record) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    let text = |key: &str| {
        record
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let phase = record
        .get("phase")
        .and_then(|value| value.as_str())
        .and_then(Phase::parse);
    let Some(phase) = phase else { return Ok(None) };
    let model = text("model");
    let cli = text("cli");
    if model.is_empty() || cli.is_empty() {
        return Ok(None);
    }
    let args = record
        .get("args")
        .and_then(|value| value.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(Plan {
        model,
        cli,
        args,
        generation: record
            .get("generation")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        phase,
        waited: record
            .get("waited")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
    }))
}

/// Forget a plan (after the restart finished, or when the seat was flagged).
pub fn clear_pending(store: &mut Store, seat_id: &str) -> GearResult<()> {
    meta_set(store, &plan_key(seat_id), "")
}

/// The meta key of a seat's restart plan, for callers that clear it inside
/// their own transaction.
pub fn pending_meta_key(seat_id: &str) -> String {
    plan_key(seat_id)
}

/// The CLI the seat runs on now: the kind of its live binding. `None` when
/// the seat has no binding.
pub(crate) fn current_cli(store: &mut Store, seat: &SeatRow) -> GearResult<Option<String>> {
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    Ok(binding.map(|binding| binding.kind))
}

/// The pane of the seat's live binding.
pub(crate) fn pane_of(store: &mut Store, seat: &SeatRow) -> GearResult<Option<String>> {
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    Ok(binding.map(|binding| binding.pane_id))
}

/// The profile that drives `kind`: the one named after it, else the first
/// with that kind. `None` when no profile drives that CLI.
pub(crate) fn profile_for_kind<'a>(
    profiles: &'a std::collections::BTreeMap<String, Profile>,
    kind: &'a str,
) -> Option<(&'a str, &'a Profile)> {
    profiles
        .get(kind)
        .filter(|profile| profile.kind == kind)
        .map(|profile| (kind, profile))
        .or_else(|| {
            profiles
                .iter()
                .find(|(_, profile)| profile.kind == kind)
                .map(|(name, profile)| (name.as_str(), profile))
        })
}

/// Start (or continue) the cross-CLI restart of `seat` on the CLI of
/// `entry`. One step per tick; the plan carries the target, so a restart
/// without an entry continues where it stopped.
///
/// `entry` is the chain entry that needs another CLI. `None` when a plan is
/// already running.
pub(crate) fn advance(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, Profile>,
    project: &str,
    seat: &SeatRow,
    entry: Option<&ModelEntry>,
    now: Ts,
) -> GearResult<Option<String>> {
    let mut plan = match pending(store, &seat.id)? {
        Some(plan) => plan,
        None => {
            let entry = entry.ok_or_else(|| {
                GearError::new(
                    ErrorCode::BadFormat,
                    format!("seat {} has no restart target", seat.id),
                )
            })?;
            let cli = entry.cli().ok_or_else(|| {
                GearError::new(
                    ErrorCode::BadFormat,
                    format!("the model {} names no CLI", entry.model()),
                )
            })?;
            let plan = Plan {
                model: entry.model().to_string(),
                cli: cli.to_string(),
                args: entry.args().to_vec(),
                generation: 0,
                phase: Phase::Stop,
                waited: 0,
            };
            set_pending(store, &seat.id, &plan)?;
            plan
        }
    };

    loop {
        match plan.phase {
            Phase::Stop => {
                if !stop(store, herdr, profiles, seat, &mut plan, now)? {
                    return Ok(None);
                }
            }
            Phase::Exit => {
                if !exit(store, herdr, seat, &mut plan, now)? {
                    return Ok(None);
                }
            }
            Phase::Start => {
                if !start(store, herdr, cfg, profiles, seat, &mut plan, now)? {
                    return Ok(None);
                }
            }
            Phase::Finish => {
                return finish(store, herdr, profiles, project, seat, &plan, now);
            }
        }
    }
}

/// The agent must be stopped before it is closed.
fn stop(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    profiles: &std::collections::BTreeMap<String, Profile>,
    seat: &SeatRow,
    plan: &mut Plan,
    now: Ts,
) -> GearResult<bool> {
    let Some(pane) = pane_of(store, seat)? else {
        flag(store, seat, "the seat has no pane to restart in", now)?;
        clear_pending(store, &seat.id)?;
        return Ok(false);
    };
    let expected_cli = current_cli(store, seat)?;
    // The agent's status is read again right now: a stale `idle` is not
    // trusted (the rule of the in-place switch).
    match herdr.agent_get(&pane) {
        Ok(Some(info)) => {
            if info.agent.as_deref() != expected_cli.as_deref() {
                flag(
                    store,
                    seat,
                    "another agent holds the pane — needs check",
                    now,
                )?;
                clear_pending(store, &seat.id)?;
                return Ok(false);
            }
            if !matches!(info.agent_status, AgentStatus::Idle | AgentStatus::Done) {
                // Still working: wait for it to stop.
                return wait(store, seat, plan, now, "the agent is still working");
            }
        }
        Ok(None) => {
            // The agent is already gone: the next CLI may start.
            plan.phase = Phase::Start;
            plan.waited = 0;
            set_pending(store, &seat.id, plan)?;
            return Ok(true);
        }
        Err(error) => {
            return wait(
                store,
                seat,
                plan,
                now,
                &format!("the pane could not be read: {}", error.0),
            );
        }
    }

    // The seat leaves the states a doorbell may reach **before** its agent
    // goes: a seat whose agent is gone reaches the captain (L-56), and a
    // planned restart is not a loss.
    leave_reachable(store, seat, plan, now)?;

    // The exit keys go once: a second `ctrl+c` a moment later would land in
    // the shell instead of the agent.
    let cli = current_cli(store, seat)?.unwrap_or_default();
    let profile = profiles
        .get(&seat.profile)
        .filter(|profile| profile.kind == cli)
        .or_else(|| profile_for_kind(profiles, &cli).map(|(_, profile)| profile));
    let sent = match profile {
        Some(profile) if !profile.exit_command.is_none() => {
            // The CLI takes a command, not a key (grok: `/exit`).
            herdr.prompt(&pane, profile.exit_command.as_deref().unwrap_or_default())
        }
        Some(profile) if !profile.exit_keys.is_empty() => {
            herdr.send_keys(&pane, &profile.exit_keys)
        }
        _ => {
            flag(
                store,
                seat,
                &format!("the CLI {cli:?} has no way to close its agent — needs check"),
                now,
            )?;
            clear_pending(store, &seat.id)?;
            return Ok(false);
        }
    };
    match sent {
        Outcome::Ok(()) => {}
        // The effect may have happened: the agent may be gone already. It is
        // never sent twice.
        Outcome::Uncertain { .. } => {}
        Outcome::Refused { code, message } => {
            // Nothing was executed: the next tick tries again.
            return wait(
                store,
                seat,
                plan,
                now,
                &format!("the exit keys were refused ({code}): {message}"),
            );
        }
    }
    plan.phase = Phase::Exit;
    plan.waited = 0;
    set_pending(store, &seat.id, plan)?;
    Ok(false)
}

/// The exit keys were sent: the agent must be gone before the next CLI
/// starts in the same pane.
fn exit(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
    plan: &mut Plan,
    now: Ts,
) -> GearResult<bool> {
    let Some(pane) = pane_of(store, seat)? else {
        flag(store, seat, "the seat has no pane to restart in", now)?;
        clear_pending(store, &seat.id)?;
        return Ok(false);
    };
    match herdr.agent_get(&pane) {
        Ok(None) => {
            // The pane is a shell again: the next CLI may start.
            plan.phase = Phase::Start;
            plan.waited = 0;
            set_pending(store, &seat.id, plan)?;
            Ok(true)
        }
        Ok(Some(_)) => {
            // The old agent is still there: the next tick looks again.
            wait(store, seat, plan, now, "the agent is still in the pane")
        }
        Err(error) => {
            wait(
                store,
                seat,
                plan,
                now,
                &format!("the pane could not be read: {}", error.0),
            )
        }
    }
}

/// The agent is gone: the next CLI is started in the same pane.
fn start(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, Profile>,
    seat: &SeatRow,
    plan: &mut Plan,
    now: Ts,
) -> GearResult<bool> {
    let Some(pane) = pane_of(store, seat)? else {
        flag(store, seat, "the seat has no pane to restart in", now)?;
        clear_pending(store, &seat.id)?;
        return Ok(false);
    };
    let Some((profile_name, profile)) = profile_for_kind(profiles, &plan.cli) else {
        flag(
            store,
            seat,
            &format!("no profile drives the CLI {:?} — needs check", plan.cli),
            now,
        )?;
        clear_pending(store, &seat.id)?;
        return Ok(false);
    };
    if !profile.verified {
        flag(
            store,
            seat,
            &format!("the profile {profile_name:?} is not verified — needs check"),
            now,
        )?;
        clear_pending(store, &seat.id)?;
        return Ok(false);
    }

    // The start may have happened before the engine died: the pane already
    // hosts the target CLI's agent.
    match herdr.agent_get(&pane) {
        Ok(Some(info)) if info.agent.as_deref() == Some(plan.cli.as_str()) => {
            plan.phase = Phase::Finish;
            plan.waited = 0;
            set_pending(store, &seat.id, plan)?;
            return Ok(true);
        }
        Ok(Some(_)) => {
            flag(
                store,
                seat,
                "another agent holds the pane — needs check",
                now,
            )?;
            clear_pending(store, &seat.id)?;
            return Ok(false);
        }
        Ok(None) => {}
        Err(error) => {
            return wait(
                store,
                seat,
                plan,
                now,
                &format!("the pane could not be read: {}", error.0),
            );
        }
    }

    // 1. the generation is reserved and the seat leaves `Active`, in one
    //    transaction: the old binding is void, the new one is pending in the
    //    same pane with the new CLI.
    if plan.generation == 0 {
        let generation = reserve(store, seat, &pane, plan, profile_name, now)?;
        plan.generation = generation;
        set_pending(store, &seat.id, plan)?;
    }

    // 2. the environment of the new agent: the pane is a shell, so the export
    //    is what the agent inherits. It is sent once per generation.
    let env = env_command(cfg, seat, plan.generation, &plan.model, &plan.cli);
    if !env_ready(store, herdr, &seat.id, &pane, &env, plan.generation)? {
        return wait(store, seat, plan, now, "the environment line was refused");
    }

    // 3. the agent itself: the profile's unattended args, then the model's
    //    own flag and the arguments of the chain entry.
    let mut args = profile.unattended_args.clone();
    if let Some(model_args) = start_args(&plan.cli, &plan.model, &plan.args) {
        args.extend(model_args);
    }
    let name = format!("{}-{}", seat.project, seat.name);
    // A grok worktree is trusted before its agent starts: grok asks on the
    // first start, while Herdr reports the agent idle, and the prompt would
    // be lost (T-32).
    if let Some(path) = worktree_path(store, seat)? {
        if let Err(reason) = crate::trust::trust_worktree(&plan.cli, &path, now) {
            flag(store, seat, &format!("trust: {reason} — needs check"), now)?;
            clear_pending(store, &seat.id)?;
            return Ok(false);
        }
    }
    if plan.cli == "opencode" {
        if let Some(path) = worktree_path(store, seat)? {
            let project = store.read(|tx| tx.get_project(&seat.project))?.expect("project exists");
            let config = gear_app::config::parse(&project.config_toml)?;
            match crate::opencode::start_args(
                store,
                herdr,
                &seat.id,
                plan.generation,
                &path,
                &config,
                &plan.model,
                cfg.model_list_wait_s,
                cfg.model_list_poll_ms,
            ) {
                Ok(session_args) => args.extend(session_args),
                Err(reason) => {
                    flag(store, seat, &reason, now)?;
                    clear_pending(store, &seat.id)?;
                    return Ok(false);
                }
            }
        }
    }
    // T-76: the restart's args name the folder the agent runs in — its
    // worktree; a seat from before the worktree was recorded falls back to the
    // root of its lane's repo, which Codex trusts just the same.
    let folder = match worktree_path(store, seat)? {
        Some(path) => Some(path),
        None => store
            .read(|tx| tx.get_project(&seat.project))?
            .and_then(|row| gear_app::config::parse(&row.config_toml).ok())
            .map(|config| config.repo_of_lane(&seat.name).path),
    };
    let args = match folder {
        Some(folder) => gear_herdr::render_args(&args, &folder),
        None => args,
    };
    match herdr.agent_start(&name, &plan.cli, &pane, &args) {
        Outcome::Ok(_) => {
            plan.phase = Phase::Finish;
            plan.waited = 0;
            set_pending(store, &seat.id, plan)?;
            Ok(true)
        }
        Outcome::Refused { code, .. } if code == "agent_pane_busy" => {
            // The shell is still busy: the next tick tries again.
            wait(store, seat, plan, now, "the pane is still busy")
        }
        Outcome::Refused { code, .. } if code == "agent_name_taken" => {
            // The old agent's name is not free yet: the next tick tries again.
            wait(store, seat, plan, now, "the agent name is still taken")
        }
        Outcome::Refused { code, message } => {
            flag(
                store,
                seat,
                &format!("agent_start refused ({code}): {message} — needs check"),
                now,
            )?;
            clear_pending(store, &seat.id)?;
            Ok(false)
        }
        Outcome::Uncertain { reason } => {
            // The agent may be up: the next tick looks at the pane and finds
            // it (or sends the start again only if the pane is still empty).
            wait(
                store,
                seat,
                plan,
                now,
                &format!("agent_start uncertain: {reason}"),
            )
        }
    }
}

/// The new agent is up: the model, the seat and the Continue are booked, in
/// one transaction. An engine restart sees either none of it or all of it.
fn finish(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    profiles: &std::collections::BTreeMap<String, Profile>,
    project: &str,
    seat: &SeatRow,
    plan: &Plan,
    now: Ts,
) -> GearResult<Option<String>> {
    if plan.cli == "opencode" {
        let config = store
            .read(|tx| tx.get_project(project))?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, "project missing"))?;
        let config = gear_app::config::parse(&config.config_toml)?;
        let (_, profile) = profile_for_kind(profiles, &plan.cli)
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, "restart profile missing"))?;
        match crate::switch::ensure_model(store, herdr, seat, profile, &config, &plan.model, now)? {
            crate::switch::ModelGate::Ready => {}
            crate::switch::ModelGate::Waiting => return Ok(None),
            crate::switch::ModelGate::Failed(reason) => {
                // T-60: the agent is up but not on the chain's model — the
                // lane stops here, so the captain is rung (once).
                crate::quota::flag_and_alert(store, project, seat, &reason, now)?;
                clear_pending(store, &seat.id)?;
                crate::switch::clear_pending(store, &seat.id)?;
                return Ok(None);
            }
        }
    }
    // What the seat carries on with: a lane its Continue line, a reviewer its
    // review packet again (T-115).
    let (text, open) = crate::quota::resume_delivery(store, project, seat)?;
    store.write(|tx| {
        // The new binding must be there: the seat was reserved before the
        // agent started.
        let binding = tx.get_binding(&seat.id, plan.generation)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!(
                    "no binding of seat {} generation {}",
                    seat.id, plan.generation
                ),
            )
        })?;
        if binding.kind != plan.cli {
            return Err(GearError::new(
                ErrorCode::BadTransition,
                format!(
                    "the binding of seat {} runs {:?}, not {:?}",
                    seat.id, binding.kind, plan.cli
                ),
            ));
        }
        tx.set_binding_status(&seat.id, plan.generation, BindingStatus::Active)?;
        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        require_transition(
            SeatState::can_transition(row.role, row.state, SeatState::Active),
            "seat",
            row.state.as_str(),
            SeatState::Active.as_str(),
        )?;
        tx.set_seat_state(
            &seat.id,
            SeatState::Active,
            Some(&format!("restarted on {} ({})", plan.cli, plan.model)),
            now,
        )?;
        tx.set_meta(&model_meta_key(&seat.id), &plan.model)?;
        tx.set_meta(&crate::quota::quota_meta_key(&seat.id), "0")?;
        tx.set_meta(&pending_meta_key(&seat.id), "")?;
        if let Some((task, attempt)) = open.clone() {
            tx.insert_op(&gear_store::NewOp {
                project: project.to_string(),
                seat: seat.id.clone(),
                generation: plan.generation,
                kind: OpKind::Deliver,
                task: Some(task),
                attempt: Some(attempt),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": text }),
                created: now,
            })?;
        }
        Ok(())
    })?;
    let line = format!(
        "{}: restarted on {} ({}) — the model is {}",
        seat.id, plan.cli, plan.model, plan.model
    );
    crate::quota::tell_captain(store, project, seat, &line, now)?;
    Ok(Some(line))
}

/// The seat leaves the states a doorbell may reach **before** its agent
/// goes: a seat whose agent is gone reaches the captain (L-56), and a planned
/// restart is not a loss.
fn leave_reachable(store: &mut Store, seat: &SeatRow, plan: &Plan, now: Ts) -> GearResult<()> {
    let reason = format!("restarting on {} ({})", plan.cli, plan.model);
    store.write(|tx| {
        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        if !matches!(
            row.state,
            SeatState::Active | SeatState::Standby | SeatState::Retiring | SeatState::Parked
        ) {
            return Ok(());
        }
        require_transition(
            SeatState::can_transition(row.role, row.state, SeatState::Missing),
            "seat",
            row.state.as_str(),
            SeatState::Missing.as_str(),
        )?;
        tx.set_seat_state(&seat.id, SeatState::Missing, Some(&reason), now)
    })
}

/// The generation of the new agent is reserved, the old binding is void and
/// the new one is pending in the same pane with the new CLI. The generation
/// it used is returned: the reservation is idempotent, so an engine restart
/// between the reservation and the plan finds it done.
fn reserve(
    store: &mut Store,
    seat: &SeatRow,
    pane: &str,
    plan: &Plan,
    profile_name: &str,
    now: Ts,
) -> GearResult<i64> {
    let reason = format!("restarting on {} ({})", plan.cli, plan.model);
    store.write(|tx| {
        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        // Already reserved: the seat is `Starting` with a pending binding for
        // the target CLI.
        if row.state == SeatState::Starting {
            if let Some(binding) = tx.get_binding(&seat.id, row.generation)? {
                if binding.kind == plan.cli && binding.status == BindingStatus::Pending {
                    return Ok(row.generation);
                }
            }
        }
        // The seat is `Missing` (its agent is gone) and becomes `Starting`
        // (a new agent is coming).
        let mut state = row.state;
        if matches!(
            state,
            SeatState::Active | SeatState::Standby | SeatState::Retiring | SeatState::Parked
        ) {
            require_transition(
                SeatState::can_transition(row.role, state, SeatState::Missing),
                "seat",
                state.as_str(),
                SeatState::Missing.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Missing, Some(&reason), now)?;
            state = SeatState::Missing;
        }
        if state == SeatState::Missing {
            require_transition(
                SeatState::can_transition(row.role, SeatState::Missing, SeatState::Starting),
                "seat",
                SeatState::Missing.as_str(),
                SeatState::Starting.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Starting, Some(&reason), now)?;
        }
        // The old binding is void: a late write from the old generation is
        // rejected (design/03).
        if let Some(binding) = tx.get_binding(&seat.id, row.generation)? {
            if binding.status != BindingStatus::Void {
                tx.set_binding_status(&seat.id, row.generation, BindingStatus::Void)?;
            }
        }
        // Deliveries to the departed generation cannot acquire new view
        // evidence. Settle them before the replacement's Continue owns the slot.
        for op in tx.slot_ops(&seat.id)? {
            if op.generation > row.generation {
                continue;
            }
            let state = match op.state {
                OpState::Intended => OpState::Void,
                OpState::Executing | OpState::Unknown => OpState::Failed,
                OpState::Submitted => OpState::Confirmed,
                _ => continue,
            };
            tx.set_op_state(
                &seat.project,
                &op.id,
                state,
                Some("superseded by cross-CLI restart"),
                now,
            )?;
        }
        let project = tx.get_project(&seat.project)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no project {}", seat.project))
        })?;
        let generation = row.generation + 1;
        tx.set_seat_generation(&seat.id, generation)?;
        tx.set_seat_profile(&seat.id, profile_name)?;
        tx.insert_binding(&BindingRow {
            seat: seat.id.clone(),
            generation,
            session: project.herdr_session.clone(),
            pane_id: pane.to_string(),
            kind: plan.cli.clone(),
            session_ref: None,
            status: BindingStatus::Pending,
            handover_id: None,
            created: now,
        })?;
        Ok(generation)
    })
}

/// The environment line of the new agent: the call context (with the new
/// generation) and the model when the CLI takes one in the environment.
fn env_command(cfg: &EngineCfg, seat: &SeatRow, generation: i64, model: &str, cli: &str) -> String {
    let mut env = format!(
        "export GEAR_CTX={}:{}:{}; export GEAR_DB={}",
        seat.project, seat.name, generation, cfg.db_path
    );
    if let Some(start) = model_start(cli, model) {
        if !start.env.is_empty() {
            env = format!("{env}; {}", start.env);
        }
    }
    env
}

/// The key of the seat's environment line for this restart: it is sent once
/// per generation, so a retry does not run it again.
fn env_key(seat: &str, generation: i64) -> String {
    format!("engine.restart_env.{seat}.{generation}")
}

/// Whether the environment of the new agent is set. It is set once: the
/// export is submitted before the agent starts, so the shell has applied it
/// long before the agent comes up.
fn env_ready(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &str,
    pane: &str,
    command: &str,
    generation: i64,
) -> GearResult<bool> {
    let key = env_key(seat, generation);
    if store
        .read(|tx| tx.get_meta(&key))?
        .is_some_and(|done| !done.is_empty())
    {
        return Ok(true);
    }
    if let Outcome::Ok(()) = herdr.pane_run(pane, command) {
        store.write(|tx| tx.set_meta(&key, "1"))?;
        Ok(true)
    } else {
        // The pane refused the line: the next tick tries again.
        Ok(false)
    }
}

/// One step that waits: the plan is kept and the next tick continues it.
/// A phase that waits too long flags the seat for a human.
fn wait(
    store: &mut Store,
    seat: &SeatRow,
    plan: &mut Plan,
    now: Ts,
    reason: &str,
) -> GearResult<bool> {
    plan.waited += 1;
    if plan.waited > MAX_WAIT {
        flag(
            store,
            seat,
            &format!(
                "the restart on {} ({}) did not finish: {reason}",
                plan.cli, plan.model
            ),
            now,
        )?;
        clear_pending(store, &seat.id)?;
        return Ok(false);
    }
    set_pending(store, &seat.id, plan)?;
    Ok(false)
}

/// Flag the seat for a human (`NeedsCheck`): the engine must not guess.
fn flag(store: &mut Store, seat: &SeatRow, reason: &str, now: Ts) -> GearResult<()> {
    crate::quota::flag_and_alert(store, &seat.project, seat, reason, now)
}

/// The path of the seat's worktree, when the engine knows it.
fn worktree_path(store: &mut Store, seat: &SeatRow) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(&crate::onboard::worktree_path_key(&seat.id)))
}

/// The key of a seat's restart plan.
fn plan_key(seat_id: &str) -> String {
    format!("engine.restart.{seat_id}")
}

fn set_pending(store: &mut Store, seat_id: &str, plan: &Plan) -> GearResult<()> {
    let record = serde_json::json!({
        "phase": plan.phase.as_str(),
        "model": plan.model,
        "cli": plan.cli,
        "args": plan.args,
        "generation": plan.generation,
        "waited": plan.waited,
    });
    meta_set(store, &plan_key(seat_id), &record.to_string())
}

fn meta_get(store: &mut Store, key: &str) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(key))
}

fn meta_set(store: &mut Store, key: &str, value: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(key, value))
}
