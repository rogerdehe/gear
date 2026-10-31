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
    /// When the current phase began, and when the restart began (T-407): the
    /// durable times its probe lines are measured from.
    pub at: Ts,
    pub started: Ts,
    /// Original CLI and same-CLI session, retained across engine restarts.
    pub old_cli: String,
    pub resume: Option<String>,
    pub resume_required: bool,
    pub profile_name: Option<String>,
    /// T-465: whether the switch happens at once instead of at the seat's next
    /// safe point. The outgoing agent is told to wrap up, and the engine exits
    /// it after `grace_s` whether or not it handed in.
    pub at_once: bool,
    /// How long the outgoing agent has to wrap up, in seconds.
    pub grace_s: i64,
    /// When the wrap-up message went out (0 = not yet).
    pub wrap_at: Ts,
    /// Whether the wrap-up message went out.
    pub wrapped: bool,
    /// The handover document written for the incoming agent.
    pub handover_doc: Option<String>,
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
    if cli.is_empty() {
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
        old_cli: text("old_cli"),
        resume: record
            .get("resume")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        resume_required: record
            .get("resume_required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        profile_name: record
            .get("profile_name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        at_once: record
            .get("at_once")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        grace_s: record.get("grace_s").and_then(|v| v.as_i64()).unwrap_or(0),
        wrap_at: record.get("wrap_at").and_then(|v| v.as_i64()).unwrap_or(0),
        wrapped: record
            .get("wrapped")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        handover_doc: record
            .get("handover_doc")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        waited: record
            .get("waited")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        at: record
            .get("at")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        started: record
            .get("started")
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

/// When an agent switch happens (T-465): at the seat's next safe point, or
/// at once — the outgoing agent is told to wrap up, and the engine exits it
/// after the configured grace period whether or not it handed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum When {
    SafePoint,
    AtOnce,
}

/// Start (or continue) the cross-CLI restart of `seat` on the CLI of
/// `entry`, at the seat's next safe point.
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
    advance_when(
        store,
        herdr,
        cfg,
        profiles,
        project,
        seat,
        entry,
        When::SafePoint,
        now,
    )
}

/// [`advance`] with the switch's timing: an at-once switch tells the outgoing
/// agent to wrap up and commit, waits `[timing].switch_grace_min` (default 10
/// minutes), then exits it whether or not it handed in (T-465).
pub(crate) fn advance_when(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, Profile>,
    project: &str,
    seat: &SeatRow,
    entry: Option<&ModelEntry>,
    when: When,
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
            let old_cli = current_cli(store, seat)?.unwrap_or_default();
            let (mut resume_required, resume) = if old_cli == cli {
                store.read(|tx| {
                    let Some(round) = tx.open_round_of_seat(&seat.id)? else {
                        return Ok((false, None));
                    };
                    let session = crate::session::round_session(tx, &round)?;
                    let session = session.filter(|s| !s.is_empty()).or(tx
                        .get_binding(&seat.id, seat.generation)?
                        .and_then(|binding| binding.session_ref)
                        .filter(|s| !s.is_empty()));
                    Ok((true, session))
                })?
            } else {
                (false, None)
            };
            // T-209: a same-CLI agent that has already exited cannot report
            // its session. If its bound pane is a shell and there is no
            // recoverable round/binding session, start a fresh process in the
            // same worktree instead of parking this restart forever.
            if resume_required && resume.is_none() {
                if let Some(pane) = pane_of(store, seat)? {
                    if matches!(herdr.agent_get(&pane), Ok(None)) {
                        resume_required = false;
                    }
                }
            }
            let profile_name = gear_app::config::latest(store, project)
                .ok()
                .and_then(|config| config.lanes.into_iter().find(|lane| lane.name == seat.name))
                .and_then(|lane| {
                    profiles
                        .get(&lane.profile)
                        .filter(|p| p.kind == cli)
                        .map(|_| lane.profile)
                });
            let plan = Plan {
                old_cli,
                resume,
                resume_required,
                profile_name,
                model: entry.model().to_string(),
                cli: cli.to_string(),
                args: entry.args().to_vec(),
                generation: 0,
                phase: Phase::Stop,
                waited: 0,
                at: now,
                started: now,
                at_once: when == When::AtOnce,
                grace_s: grace_seconds(store, project, when),
                wrap_at: 0,
                wrapped: false,
                handover_doc: None,
            };
            set_pending(store, &seat.id, &plan)?;
            plan
        }
    };

    // A same-CLI restart of an open round cannot safely start fresh merely
    // because its session has not been observed yet. Keep the plan pending.
    if plan.resume_required && plan.resume.is_none() {
        let pane = pane_of(store, seat)?;
        let agent = pane
            .as_ref()
            .and_then(|pane| herdr.agent_get(pane).ok().flatten());
        let session = agent
            .as_ref()
            .filter(|info| info.agent.as_deref() == Some(plan.old_cli.as_str()))
            .and_then(|info| {
                info.agent_session
                    .as_ref()
                    .map(|session| session.value.clone())
            })
            .filter(|session| !session.is_empty());
        if let Some(session) = session {
            plan.resume = Some(session);
            set_pending(store, &seat.id, &plan)?;
        } else if pane
            .as_ref()
            .is_some_and(|pane| matches!(herdr.agent_get(pane), Ok(None)))
        {
            // The persisted plan may have been created while its agent was
            // present but unable to report a session. Once that same pane is
            // a shell, use the fresh-start fallback from plan creation.
            plan.resume_required = false;
            set_pending(store, &seat.id, &plan)?;
        } else {
            return Ok(None);
        }
    }
    loop {
        let before = plan.phase;
        let began = plan.at;
        let moves_on = match plan.phase {
            Phase::Stop => stop(store, herdr, cfg, profiles, seat, &mut plan, now)?,
            Phase::Exit => exit(store, herdr, seat, &mut plan, now)?,
            Phase::Start => start(store, herdr, cfg, profiles, seat, &mut plan, now)?,
            Phase::Finish => {
                let finished = finish(store, herdr, profiles, project, seat, &plan, now)?;
                // T-407: the last phase's own line, with the whole restart's
                // time in it — once it is done, not while it waits.
                if finished.is_some() {
                    let total = now.saturating_sub(plan.started).saturating_mul(1000);
                    crate::ops::probe(
                        "restart",
                        &seat.id,
                        Phase::Finish.as_str(),
                        now.saturating_sub(plan.at).saturating_mul(1000),
                        total,
                    );
                    // The restart's own completion line, as a session action's
                    // `done` (T-407).
                    crate::ops::probe("restart", &seat.id, "done", total, total);
                }
                return Ok(finished);
            }
        };
        // T-407: a phase that ended here is one line of the engine log, from
        // the durable time it began with (the phase itself moved `at` on).
        if plan.phase != before {
            let step_ms = now.saturating_sub(began).saturating_mul(1000);
            let total = now.saturating_sub(plan.started).saturating_mul(1000);
            crate::ops::probe("restart", &seat.id, before.as_str(), step_ms, total);
        }
        if !moves_on {
            return Ok(None);
        }
    }
}

/// The agent must be stopped before it is closed.
#[allow(clippy::too_many_arguments)]
fn stop(
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
    let expected_cli = current_cli(store, seat)?;
    // T-465: an at-once switch tells the outgoing agent once to wrap up and
    // commit, writes the handover document for it, and starts the grace
    // period. The agent's own summary and notes go into that document.
    if plan.at_once && !plan.wrapped {
        crate::names::ensure(herdr, &pane, &crate::names::desired(seat));
        let facts = collect_handover(store, herdr, cfg, seat, plan, now)?;
        if let Some(facts) = &facts {
            crate::handover::write_document(cfg, facts, false)?;
            plan.handover_doc = Some(facts.doc.clone());
        }
        let text = wrap_up_text(plan, facts.as_ref());
        if let Some(name) = crate::names::live_name(herdr, &pane) {
            let _ = herdr.prompt(&name, &text);
        }
        plan.wrapped = true;
        plan.wrap_at = now;
        set_pending(store, &seat.id, plan)?;
        return Ok(false);
    }
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
            let idle = matches!(info.agent_status, AgentStatus::Idle | AgentStatus::Done);
            // T-465: past the grace period the switch exits the agent whether
            // or not it handed in; a model that thinks for hours must not hold
            // the switch.
            let past_grace =
                plan.at_once && plan.wrapped && now >= plan.wrap_at.saturating_add(plan.grace_s);
            if !idle && !past_grace {
                // Still working: wait for it to stop.
                // A busy agent may be legitimately retrying for an arbitrary
                // time. No timeout may turn that into a failed seat.
                if plan.waited != 0 {
                    plan.waited = 0;
                    set_pending(store, &seat.id, plan)?;
                }
                return Ok(false);
            }
        }
        Ok(None) => {
            // The agent is already gone: the handover facts are still the
            // worktree's, and the next CLI may start.
            finish_handover(store, herdr, cfg, seat, plan, now)?;
            plan.phase = Phase::Start;
            plan.waited = 0;
            plan.at = now;
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

    // The handover is written from the worktree as the outgoing agent leaves
    // it (T-465): after the wrap-up grace, before the exit keys.
    finish_handover(store, herdr, cfg, seat, plan, now)?;

    // The seat leaves the states a doorbell may reach **before** its agent
    // goes: a seat whose agent is gone reaches the captain (L-56), and a
    // planned restart is not a loss.
    leave_reachable(store, seat, plan, now)?;

    // T-134: the exit is a prompt (or keys) to the agent in this pane; an agent
    // Herdr shows unnamed refuses a prompt, so name it first.
    crate::names::ensure(herdr, &pane, &crate::names::desired(seat));

    // The exit keys go once: a second `ctrl+c` a moment later would land in
    // the shell instead of the agent.
    let cli = current_cli(store, seat)?.unwrap_or_default();
    let profile = profiles
        .get(&seat.profile)
        .filter(|profile| profile.kind == cli)
        .or_else(|| profile_for_kind(profiles, &cli).map(|(_, profile)| profile));
    let sent = match profile {
        Some(profile) if !profile.exit_command.is_none() => {
            // The CLI takes a command, not a key (grok: `/exit`). Design
            // §10.1: the command goes to the agent's name, never to the pane
            // id — a pane whose agent is gone is a shell.
            match crate::names::live_name(herdr, &pane) {
                Some(name) => {
                    herdr.prompt(&name, profile.exit_command.as_deref().unwrap_or_default())
                }
                // Without a name there is no agent to address: the command is
                // withheld rather than typed into a shell.
                None => Outcome::Refused {
                    code: "agent_not_ready".into(),
                    message: format!("the agent of pane {pane} has no name"),
                },
            }
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
    plan.at = now;
    set_pending(store, &seat.id, plan)?;
    Ok(false)
}

/// The handover facts of this switch (T-465): whose work the incoming agent
/// takes over, the previous conversation, and the worktree as the outgoing
/// agent leaves it.
fn collect_handover(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    seat: &SeatRow,
    plan: &Plan,
    now: Ts,
) -> GearResult<Option<crate::handover::Facts>> {
    let old_model = meta_get(store, &model_meta_key(&seat.id))?.unwrap_or_default();
    let mut facts = crate::handover::collect(
        store,
        cfg,
        herdr,
        &seat.project,
        seat,
        &plan.old_cli,
        &old_model,
        &plan.cli,
        &plan.model,
        now,
    )?;
    // The document written at the wrap-up keeps its path: the stamp is the
    // switch's, not the tick's.
    if let (Some(facts), Some(doc)) = (facts.as_mut(), plan.handover_doc.as_ref()) {
        facts.doc = doc.clone();
    }
    Ok(facts)
}

/// Write the handover document from the worktree as it is now and store the
/// block for the incoming agent's packet (T-465). The generation is the one
/// the new agent will run on.
fn finish_handover(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    seat: &SeatRow,
    plan: &mut Plan,
    now: Ts,
) -> GearResult<()> {
    let Some(facts) = collect_handover(store, herdr, cfg, seat, plan, now)? else {
        return Ok(());
    };
    crate::handover::write_document(cfg, &facts, true)?;
    plan.handover_doc = Some(facts.doc.clone());
    let generation = if plan.generation > 0 {
        plan.generation
    } else {
        seat.generation + 1
    };
    crate::handover::store_block(store, &seat.id, generation, &facts)
}

/// What an at-once switch tells the outgoing agent: wrap up, commit and push,
/// write its part of the handover, then stop (T-465).
fn wrap_up_text(plan: &Plan, facts: Option<&crate::handover::Facts>) -> String {
    let grace = if plan.grace_s > 0 {
        format!("{} minutes", plan.grace_s / 60)
    } else {
        "at once".to_string()
    };
    let document = match facts {
        Some(facts) => format!(
            " Write your summary and any notes into {} (parts 2 and 3 of the handover template, part A of the voyage's handover.md).",
            facts.doc
        ),
        None => String::new(),
    };
    format!(
        "Wrap up and commit now: the engine switches your agent to {} ({}). Commit and push the work on your branch.{document} Then stop: the engine exits this agent in {grace} whether or not you handed in, and the next agent takes over your work.",
        plan.cli, plan.model
    )
}

/// The grace period of an at-once switch, from `[timing].switch_grace_min`
/// (default 10 minutes); zero for a safe-point switch.
fn grace_seconds(store: &mut Store, project: &str, when: When) -> i64 {
    if when != When::AtOnce {
        return 0;
    }
    let minutes = gear_app::config::latest(store, project)
        .ok()
        .map(|config| config.timing.switch_grace_min)
        .unwrap_or(10);
    i64::from(minutes) * 60
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
            plan.at = now;
            set_pending(store, &seat.id, plan)?;
            Ok(true)
        }
        Ok(Some(_)) => {
            // The old agent is still there: the next tick looks again.
            wait(store, seat, plan, now, "the agent is still in the pane")
        }
        Err(error) => wait(
            store,
            seat,
            plan,
            now,
            &format!("the pane could not be read: {}", error.0),
        ),
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
    let selected = plan
        .profile_name
        .as_deref()
        .and_then(|name| {
            profiles
                .get(name)
                .filter(|profile| profile.kind == plan.cli)
                .map(|profile| (name, profile))
        })
        .or_else(|| profile_for_kind(profiles, &plan.cli));
    let Some((profile_name, profile)) = selected else {
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
            plan.at = now;
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
    //    is what the agent inherits. It is sent once per generation. The seat's
    //    own scratch directory comes first (T-420), as on every start.
    let conventions = gear_app::config::conventions_or_default(store, &seat.project)?;
    let worktree = worktree_path(store, seat)?;
    let env = env_command(
        cfg,
        seat,
        plan.generation,
        &plan.model,
        &plan.cli,
        &conventions,
        worktree.as_deref(),
    );
    if !env_ready(store, herdr, &seat.id, &pane, &env, plan.generation)? {
        return wait(store, seat, plan, now, "the environment line was refused");
    }

    // 3. the agent itself: the profile's unattended args, then the model's
    //    own flag and the arguments of the chain entry.
    let mut args = profile.unattended_args.clone();
    args.extend(crate::rules::arguments(
        store,
        &seat.project,
        seat.role,
        &cfg.db_path,
        profile,
    )?);
    if !plan.model.is_empty() {
        if let Some(model_args) = start_args(&plan.cli, &plan.model, &plan.args) {
            args.extend(model_args);
        }
    }
    if let Some(session) = &plan.resume {
        // The model gate must address the resumed session even before finish.
        store.write(|tx| tx.set_binding_session_ref(&seat.id, plan.generation, session))?;
        let Some(resume_args) = profile.resume_args.as_ref() else {
            flag(
                store,
                seat,
                &format!("the profile {profile_name:?} cannot resume its open session"),
                now,
            )?;
            clear_pending(store, &seat.id)?;
            return Ok(false);
        };
        args.extend(
            resume_args
                .iter()
                .map(|arg| arg.replace("{session}", session)),
        );
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
    if plan.cli == "opencode" && !plan.model.is_empty() && plan.resume.is_none() {
        if let Some(path) = worktree_path(store, seat)? {
            let project = store
                .read(|tx| tx.get_project(&seat.project))?
                .expect("project exists");
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
            plan.at = now;
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
            // A CLI that is not installed says so on the pane (T-286): the
            // seat stops here for the install; the chain is not walked.
            let tail = herdr.read_tail(&pane, 16).unwrap_or_default();
            let reason = match crate::start::cli_missing(&tail, &plan.cli) {
                Some(line) => crate::start::cli_missing_text(&plan.cli, &line),
                None => format!("agent_start refused ({code}): {message} — needs check"),
            };
            flag(store, seat, &reason, now)?;
            clear_pending(store, &seat.id)?;
            Ok(false)
        }
        Outcome::Uncertain { reason } => {
            // A missing CLI is certain even when the start is not: waiting
            // would look like a switch that never ends, so the seat is flagged
            // and its plan cleared like a refusal (T-286).
            let tail = herdr.read_tail(&pane, 16).unwrap_or_default();
            if let Some(line) = crate::start::cli_missing(&tail, &plan.cli) {
                flag(
                    store,
                    seat,
                    &crate::start::cli_missing_text(&plan.cli, &line),
                    now,
                )?;
                clear_pending(store, &seat.id)?;
                return Ok(false);
            }
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
    if plan.cli == "opencode" && !plan.model.is_empty() {
        let config = store
            .read(|tx| tx.get_project(project))?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, "project missing"))?;
        let config = gear_app::config::parse(&config.config_toml)?;
        let (_, profile) = plan
            .profile_name
            .as_deref()
            .and_then(|name| {
                profiles
                    .get(name)
                    .filter(|profile| profile.kind == plan.cli)
                    .map(|profile| (name, profile))
            })
            .or_else(|| profile_for_kind(profiles, &plan.cli))
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
    let observed_session = pane_of(store, seat)?
        .and_then(|pane| herdr.agent_get(&pane).ok().flatten())
        .and_then(|info| info.agent_session.map(|s| s.value));
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
        let session = plan.resume.as_ref().or(observed_session.as_ref());
        if let Some(session) = session {
            tx.set_binding_session_ref(&seat.id, plan.generation, session)?;
        }
        if plan.resume.is_some() {
            if let Some(round) = tx.open_round_of_seat(&seat.id)? {
                if round.role != "review" {
                    tx.set_attempt_action(project, &round.task, round.attempt, "resume")?;
                }
                let identity = format!(
                    "{}:{}:{}:{}",
                    round.project, round.task, round.attempt, round.role
                );
                tx.set_meta(&format!("engine.session_step.{}", seat.id), &serde_json::json!({
                    "round":identity,"generation":plan.generation,"action":"resume","step":"done",
                    "step_at":now,"result":"resume","target":plan.resume
                }).to_string())?;
            }
        }
        if !plan.old_cli.is_empty() && plan.old_cli != plan.cli {
            if let Some(round) = tx.open_round_of_seat(&seat.id)? {
                let identity = format!(
                    "{}:{}:{}:{}",
                    round.project, round.task, round.attempt, round.role
                );
                tx.set_meta(
                    &format!("engine.session_cross_cli.{}", seat.id),
                    &serde_json::json!({"generation":plan.generation,"round":identity}).to_string(),
                )?;
                tx.set_meta(&format!("engine.session_step.{}", seat.id), &serde_json::json!({
                    "round":identity,"generation":plan.generation,"action":"fresh","step":"done",
                    "step_at":now,"result":"fresh"
                }).to_string())?;
                if round.role != "review" {
                    tx.set_attempt_action(project, &round.task, round.attempt, "fresh")?;
                }
                if let Some(session) = session {
                    let key = gear_store::RoundKey {
                        project: round.project,
                        task: round.task,
                        attempt: round.attempt,
                        role: round.role,
                    };
                    crate::rounds::add_session(tx, &key, session, &plan.cli)?;
                }
            } else {
                tx.set_meta(
                    &format!("engine.session_fresh.{}", seat.id),
                    &plan.generation.to_string(),
                )?;
            }
        }

        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        // Onboarding may already have made the restarted agent's seat Active.
        if !(row.state == SeatState::Active && row.generation == plan.generation) {
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
        }
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
        // T-217: the attempt the seat is working on moves to the new
        // generation with it, so the lane can still hand the attempt in after
        // the restart.
        gear_app::tasks::carry_attempt_generation(tx, &seat.id, row.generation, generation, now)?;
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

/// The environment line of the new agent: the seat's own scratch directory as
/// `TMPDIR` (T-420), the call context (with the new generation) and the model
/// when the CLI takes one in the environment.
fn env_command(
    cfg: &EngineCfg,
    seat: &SeatRow,
    generation: i64,
    model: &str,
    cli: &str,
    conventions: &gear_app::config::Conventions,
    worktree: Option<&str>,
) -> String {
    let prefix = crate::onboard::tmpdir_prefix(conventions, &seat.project, &seat.name, worktree)
        .unwrap_or_default();
    let mut env = format!(
        "{prefix}export GEAR_CTX={}:{}:{}; export GEAR_DB={}",
        seat.project, seat.name, generation, cfg.db_path
    );
    if !model.is_empty() {
        if let Some(start) = model_start(cli, model) {
            if !start.env.is_empty() {
                env = format!("{env}; {}", start.env);
            }
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
        "at": plan.at,
        "started": plan.started,
        "old_cli": plan.old_cli,
        "resume": plan.resume,
        "resume_required": plan.resume_required,
        "profile_name": plan.profile_name,
        "at_once": plan.at_once,
        "grace_s": plan.grace_s,
        "wrap_at": plan.wrap_at,
        "wrapped": plan.wrapped,
        "handover_doc": plan.handover_doc,
    });
    meta_set(store, &plan_key(seat_id), &record.to_string())
}

fn meta_get(store: &mut Store, key: &str) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(key))
}

fn meta_set(store: &mut Store, key: &str, value: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(key, value))
}

/// The fingerprint of the configuration a lane runs on: its chain head's cli,
/// model and args, and the chain itself. Both onboarding and reconciliation
/// record it, so a seat that onboarding started on the chain's head is not
/// taken for a config change in the same or a later tick.
pub(crate) fn desired_config(
    config: &gear_app::config::ProjectConfig,
    lane_name: &str,
    lane_kind: &str,
) -> serde_json::Value {
    let chain = crate::model::role_chain(config, lane_name);
    let first = chain.first().copied();
    let target_cli = first.and_then(|e| e.cli()).unwrap_or(lane_kind);
    let model = first.map(|e| e.model()).unwrap_or("");
    let args = first.map(|e| e.args()).unwrap_or(&[]);
    serde_json::json!({"cli":target_cli,"model":model,"args":args,
        "chain":chain.iter().map(|e|serde_json::json!([e.model(),e.cli(),e.args()])).collect::<Vec<_>>()})
}

/// Notice a changed lane configuration and start its in-place replacement.
/// The desired configuration is durable so quota fallbacks are not mistaken
/// for a new config change on every tick.
pub(crate) fn reconcile_config(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, Profile>,
    project: &str,
    now: Ts,
) -> GearResult<()> {
    let config = match gear_app::config::latest(store, project) {
        Ok(config) => config,
        Err(_) => return Ok(()),
    };
    let version = store
        .read(|tx| tx.get_project(project))?
        .map(|p| p.config_version)
        .unwrap_or(0);
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        // A seat that is not Active or Standby (Held, Parked, Queued, …) is out
        // of reconciliation: its stored `engine.restart_config` stays as it is,
        // so a change made meanwhile is still stored as a change when the seat
        // comes back, and the comparison below runs then like for any other
        // seat (T-400).
        if seat.role != gear_model::SeatRole::Lane
            || !matches!(seat.state, SeatState::Active | SeatState::Standby)
        {
            continue;
        }
        if pending(store, &seat.id)?.is_some() || crate::switch::pending(store, &seat.id)?.is_some()
        {
            continue;
        }
        let Some(lane) = config.lanes.iter().find(|lane| lane.name == seat.name) else {
            continue;
        };
        let Some(current_cli) = current_cli(store, &seat)? else {
            continue;
        };
        let chain = crate::model::role_chain(&config, &lane.name);
        let first = chain.first().copied();
        let target_cli = first.and_then(|e| e.cli()).unwrap_or(&lane.kind);
        let model = first.map(|e| e.model()).unwrap_or("");
        let args = first.map(|e| e.args()).unwrap_or(&[]);
        let desired = desired_config(&config, &lane.name, &lane.kind);
        let key = format!("engine.restart_config.{}", seat.id);
        let previous =
            meta_get(store, &key)?.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let changed = previous.as_ref().is_some_and(|old| old != &desired);
        let current_model = meta_get(store, &model_meta_key(&seat.id))?;
        // A lane the quota policy moved onto a later entry of its own chain is
        // not a config change: the quota step brings it back at the reset.
        let on_later_entry = chain.iter().skip(1).any(|entry| {
            current_model.as_deref() == Some(entry.model())
                && entry.cli().unwrap_or(&lane.kind) == current_cli
        });
        let quota_fallback = previous.is_none() && on_later_entry;
        // Version one is the initial configuration, not a re-apply. Seed a
        // baseline for existing lanes without confusing a quota fallback with
        // an explicit configuration change.
        let reapply = previous.is_some() || version > 1;
        // An explicit config change of CLI restarts the seat even when its
        // old model/CLI is still a later entry of the new chain; only a seat
        // the quota policy moved down its chain is left alone (T-400 F1).
        let kind_changed = reapply
            && current_cli != target_cli
            && !quota_fallback
            && (changed || previous.is_none());
        let model_changed = reapply
            && ((changed
                && previous.as_ref().is_some_and(|old| {
                    (old["model"] != desired["model"]
                        && (model.is_empty() || current_model.as_deref() != Some(model)))
                        || old["args"] != desired["args"]
                }))
                || (previous.is_none()
                    && !model.is_empty()
                    && current_model.is_some()
                    && current_model.as_deref() != Some(model)
                    && !quota_fallback));
        // T-400: what the seat actually runs is compared with the registered
        // chain head, not only the stored key. A restart planned while the seat
        // was Active is lost when the captain holds the lane and releases it
        // before the plan finishes; the key then says "applied" and the lane
        // would keep the old model for good.
        let runs_other = current_cli != target_cli
            || match current_model.as_deref() {
                Some(running) => running != model,
                None => false,
            };
        let stale = reapply && !on_later_entry && runs_other;
        if !kind_changed && !model_changed && !stale {
            meta_set(store, &key, &desired.to_string())?;
            continue;
        }
        let target = ModelEntry::Entry(gear_app::config::ModelSpec {
            model: model.into(),
            cli: Some(target_cli.into()),
            args: args.to_vec(),
            context_window_k: first.and_then(|e| e.context_window_k()),
        });
        // Persist the request before any terminal action. A crash resumes the
        // restart plan; the desired fingerprint prevents another one afterward.
        // T-465: an explicit config change is the user's request to switch the
        // agent; `[timing].switch_at_once` says whether it waits for the next
        // safe point or happens at once (wrap up, grace, exit).
        let when = if config.timing.switch_at_once {
            When::AtOnce
        } else {
            When::SafePoint
        };
        let _ = advance_when(
            store,
            herdr,
            cfg,
            profiles,
            project,
            &seat,
            Some(&target),
            when,
            now,
        )?;
        meta_set(store, &key, &desired.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod config_change_tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_model::{AttemptState, SeatRole, TaskState};
    use gear_store::{testing, ContractRow, NewRound, RoundKey, TaskRow};
    const T: i64 = 1_800_000_000;
    const PANE: &str = "w1:p2";

    fn config_with_args(kind: &str, model: &str, args: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"
[quota]
lanes = 1
temps = 0
[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"
[[lanes]]
name = "heat"
duty = "work"
owns = ["src/"]
kind = "{kind}"
profile = "{kind}"
[models]
lanes = [{{model="{model}", cli="{kind}", args={args}}}]
"#
        )
    }
    fn config(kind: &str, model: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"
[quota]
lanes = 1
temps = 0
[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"
[[lanes]]
name = "heat"
duty = "work"
owns = ["src/"]
kind = "{kind}"
profile = "{kind}"
[models]
lanes = [{{model="{model}", cli="{kind}"}}]
"#
        )
    }
    fn world(worktree: &str, record_round_session: bool) -> (Store, FakeHerdr, EngineCfg) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T)?;
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config("pi", "old-model");
                tx.update_project(&project)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p1",
                    "codex",
                    T,
                )?;
                tx.set_binding_session_ref("p:heat", 1, "old-session")?;
                tx.set_meta(&model_meta_key("p:heat"), "old-model")?;
                tx.set_meta(&crate::onboard::worktree_path_key("p:heat"), worktree)?;
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
                    goal: "goal".into(),
                    accept: "done".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T,
                    mechanical: None,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
                let key = RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: Some("old-model".into()),
                    start_at: T,
                })?;
                if record_round_session {
                    tx.add_round_session(&key, "old-session")?;
                }
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, "pi", AgentStatus::Idle, "p-heat");
        herdr.set_agent_session(PANE, "old-session");
        let mut cfg = EngineCfg::default();
        cfg.profiles = gear_herdr::builtins();
        (store, herdr, cfg)
    }
    fn change_to(store: &mut Store, kind: &str, model: &str, args: &str) {
        store
            .write(|tx| {
                let mut p = tx.get_project("p")?.unwrap();
                p.config_toml = config_with_args(kind, model, args);
                p.config_version += 1;
                tx.update_project(&p)
            })
            .unwrap();
    }
    fn change(store: &mut Store, kind: &str, model: &str) {
        change_to(store, kind, model, "[]")
    }
    fn advance_pending(store: &mut Store, herdr: &mut FakeHerdr, cfg: &EngineCfg, now: i64) {
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        advance(store, herdr, cfg, &cfg.profiles, "p", &seat, None, now).unwrap();
    }
    #[test]
    fn config_kind_change_waits_for_busy_agent_then_starts_fresh_in_same_worktree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/keep"), "unchanged").unwrap();
        let (mut store, mut herdr, mut cfg) = world(dir.path().to_str().unwrap(), true);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "codex", "new-model");
        store.write(|tx| { let mut p=tx.get_project("p")?.unwrap();
            p.config_toml=p.config_toml.replace("profile = \"codex\"", "profile = \"custom-codex\"");
            p.config_toml.push_str("\n[profiles.custom-codex]\nkind='codex'\nunattended_args=['--custom', '{repo_root}']\n"); tx.update_project(&p)
        }).unwrap();
        let project_config = gear_app::config::latest(&mut store, "p").unwrap();
        cfg.profiles = gear_app::config::profile_table(&cfg.profiles, &project_config).unwrap();
        herdr.set_status(PANE, AgentStatus::Working);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        for n in 2..45 {
            advance_pending(&mut store, &mut herdr, &cfg, T + n);
        }
        assert_eq!(
            store
                .read(|tx| tx.get_seat("p:heat"))
                .unwrap()
                .unwrap()
                .state,
            SeatState::Active
        );
        assert!(pending(&mut store, "p:heat").unwrap().is_some());
        assert!(!herdr
            .calls
            .iter()
            .any(|c| matches!(c, Call::SendKeys { .. } | Call::AgentStart { .. })));
        herdr.set_status(PANE, AgentStatus::Idle);
        advance_pending(&mut store, &mut herdr, &cfg, T + 46);
        assert!(herdr
            .calls
            .iter()
            .any(|c| matches!(c,Call::Prompt {pane,text,..} if pane==PANE && text=="/quit")));
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 47);
        let start = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart {
                    name,
                    kind,
                    pane,
                    args,
                } = c
                {
                    Some((name, kind, pane, args))
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            (start.0.as_str(), start.1.as_str(), start.2.as_str()),
            ("p-heat", "codex", PANE)
        );
        assert!(start
            .3
            .iter()
            .any(|s| s.contains(dir.path().to_str().unwrap())));
        assert!(start.3.iter().any(|s| s == "--custom"));
        assert!(!start.3.iter().any(|s| s.contains("old-session")));
        assert!(!herdr.calls.iter().any(|c| matches!(
            c,
            Call::WorktreeCreate { .. }
                | Call::WorktreeOpen { .. }
                | Call::WorktreeOpenPath { .. }
                | Call::WorktreeRemove { .. }
                | Call::PaneClose { .. }
        )));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("target/keep")).unwrap(),
            "unchanged"
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active);
        assert_eq!(seat.generation, 2);
        let round = store
            .read(|tx| tx.open_round_of_seat("p:heat"))
            .unwrap()
            .unwrap();
        assert_ne!(
            store
                .read(|tx| crate::session::round_session(tx, &round))
                .unwrap()
                .as_deref(),
            Some("old-session")
        );
        assert_eq!(
            store
                .read(|tx| tx.get_attempt("p", "T-1", 1))
                .unwrap()
                .unwrap()
                .session_action
                .as_deref(),
            Some("fresh")
        );
    }
    /// T-420: a restart's agent gets the seat's own scratch directory as
    /// `TMPDIR`, as a lane's first start does.
    #[test]
    fn a_restart_names_the_seats_own_scratch_dir() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "pi", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        let export = herdr
            .calls
            .iter()
            .find_map(|call| match call {
                Call::PaneRun { command, .. } if command.contains("export GEAR_CTX=") => {
                    Some(command.clone())
                }
                _ => None,
            })
            .expect("the environment line");
        let scratch = dir.path().join(".gear").join("tmp");
        assert!(
            export.starts_with(&format!(
                "mkdir -p '{}'; export TMPDIR='{}'; ",
                scratch.display(),
                scratch.display()
            )),
            "{export}"
        );
    }

    #[test]
    fn pi_resume_injects_lane_rules_after_unattended_args() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, mut cfg) = world(dir.path().to_str().unwrap(), false);
        cfg.db_path = dir.path().join("gear.db").to_string_lossy().into();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "pi", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart { kind, args, .. } = c {
                    assert_eq!(kind, "pi");
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        let path = dir.path().join("rules/lane.md");
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--append-system-prompt" && w[1] == path.to_string_lossy()));
        assert!(args.windows(2).any(|w| w == ["--session", "old-session"]));
        assert!(std::fs::read_to_string(path)
            .unwrap()
            .starts_with(include_str!("../../../skills/gear/references/rules/lane.md").trim_end()));
    }

    #[test]
    fn config_change_waits_for_an_existing_model_switch() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), true);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "codex", "new-model");
        store
            .write(|tx| {
                tx.set_meta(
                    &crate::switch::pending_meta_key("p:heat"),
                    "escape|0|0|other-model",
                )
            })
            .unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
        assert!(!herdr.calls.iter().any(|c| matches!(
            c,
            Call::Prompt { .. } | Call::SendKeys { .. } | Call::AgentStart { .. }
        )));
        crate::switch::clear_pending(&mut store, "p:heat").unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 2).unwrap();
        assert!(pending(&mut store, "p:heat").unwrap().is_some());
    }
    /// T-447, the M5.3 departure: a lane onboarding starts on its chain's head
    /// records the fingerprint in the same write that records its model, so
    /// reconciliation sees no config change in that tick or the next. A re-init
    /// that changes the head after the onboarding still restarts the lane.
    #[test]
    fn onboarding_on_the_chain_head_is_not_a_config_change() {
        let mut store = Store::open_in_memory().unwrap();
        store.write(|tx| {
            testing::seed_project(tx,"p","sess",T)?;
            let mut project=tx.get_project("p")?.unwrap();
            project.config_toml=config_with_args("pi","new-model",r#"["--thinking","high"]"#);
            project.config_version=2;
            tx.update_project(&project)?;
            tx.insert_seat(&SeatRow{id:"p:heat".into(),project:"p".into(),role:SeatRole::Lane,name:"heat".into(),profile:"pi".into(),generation:0,state:SeatState::Queued,state_since:T,state_reason:Some("registered".into()),task_id:None})?;
            // What M5.2 left behind: another CLI and model, no args.
            tx.set_meta("engine.restart_config.p:heat",r#"{"cli":"tokenrhythm","model":"old-model","args":[],"chain":[["old-model","tokenrhythm",[]]]}"#)?;
            Ok(())
        }).unwrap();
        let mut herdr = FakeHerdr::new();
        let mut cfg = EngineCfg::default();
        cfg.profiles = gear_herdr::builtins();
        assert_eq!(
            crate::onboard::onboard_queued(&mut store, &mut herdr, &cfg, T + 1).unwrap(),
            ["p:heat"]
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
        assert_eq!(seat.generation, 1);
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&model_meta_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("new-model")
        );
        let recorded = store
            .read(|tx| tx.get_meta("engine.restart_config.p:heat"))
            .unwrap()
            .unwrap();
        assert!(
            recorded.contains("new-model") && recorded.contains("--thinking"),
            "{recorded}"
        );

        // In this tick and the next, reconciliation sees no change: no restart
        // plan, the generation stays, nothing is typed into the new agent.
        herdr.calls.clear();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 2).unwrap();
        assert!(
            pending(&mut store, "p:heat").unwrap().is_none(),
            "no restart plan"
        );
        assert_eq!(
            store
                .read(|tx| tx.get_seat("p:heat"))
                .unwrap()
                .unwrap()
                .generation,
            1
        );
        assert!(
            !herdr.calls.iter().any(
                |c| matches!(c,Call::PaneRun {command,..} if command.contains("export GEAR_CTX"))
            ),
            "nothing typed: {:?}",
            herdr.calls
        );

        // A re-init that changes the head after the onboarding is a real
        // change and still starts a restart.
        change_to(&mut store, "pi", "other-model", r#"["--thinking","low"]"#);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 3).unwrap();
        assert!(
            pending(&mut store, "p:heat").unwrap().is_some(),
            "the re-init starts a restart"
        );
    }
    #[test]
    fn same_cli_restart_waits_until_the_open_session_is_known() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        store
            .write(|tx| tx.set_binding_session_ref("p:heat", 1, ""))
            .unwrap();
        herdr.set_agent_session(PANE, "");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "pi", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        assert!(pending(&mut store, "p:heat").unwrap().is_some());
        assert!(!herdr.calls.iter().any(|c| matches!(
            c,
            Call::Prompt { .. } | Call::AgentStart { .. } | Call::SendKeys { .. }
        )));
        herdr.set_agent_session(PANE, "old-session");
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        assert!(herdr
            .calls
            .iter()
            .any(|c| matches!(c,Call::Prompt {text,..} if text=="/quit")));
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 3);
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart { args, .. } = c {
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            args.windows(2).any(|w| w == ["--session", "old-session"]),
            "{args:?}"
        );
    }
    #[test]
    fn persisted_same_cli_restart_falls_back_when_agent_exits_without_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        store
            .write(|tx| tx.set_binding_session_ref("p:heat", 1, ""))
            .unwrap();
        herdr.set_agent_session(PANE, "");
        change(&mut store, "pi", "new-model");

        // The agent is still present when config reconciliation persists the
        // plan, so GEAR cannot yet know that a fresh session will be needed.
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        let plan = pending(&mut store, "p:heat").unwrap().unwrap();
        assert!(plan.resume_required);
        assert!(plan.resume.is_none());
        assert!(!herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::AgentStart { .. })));

        // Once the agent has exited, the persisted plan must clear its resume
        // requirement and start once with the new model in the same pane.
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        let starts: Vec<_> = herdr
            .calls
            .iter()
            .filter_map(|call| {
                if let Call::AgentStart {
                    kind, pane, args, ..
                } = call
                {
                    Some((kind, pane, args))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(starts.len(), 1, "{starts:?}");
        assert_eq!((starts[0].0.as_str(), starts[0].1.as_str()), ("pi", PANE));
        assert!(
            starts[0]
                .2
                .windows(2)
                .any(|args| args == ["--model", "new-model"]),
            "{:?}",
            starts[0].2
        );
        assert!(
            !starts[0].2.iter().any(|arg| arg == "--session"),
            "{:?}",
            starts[0].2
        );
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
        let ops = store
            .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Deliver))
            .unwrap();
        assert!(
            ops.iter().any(|op| op.payload["text"]
                .as_str()
                .is_some_and(|text| text.contains("Continue T-1"))),
            "{ops:?}"
        );
    }
    #[test]
    fn re_init_to_another_cli_with_an_open_attempt_restarts_on_the_new_model_and_continues() {
        // T-400: a re-init that changes the lane's model and its CLI while its
        // attempt is Started restarts the lane on the new model and hands the
        // attempt back with its Continue line.
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        assert_eq!(
            store
                .read(|tx| tx.get_attempt("p", "T-1", 1))
                .unwrap()
                .unwrap()
                .state,
            AttemptState::Started
        );
        change(&mut store, "codex", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        assert!(herdr
            .calls
            .iter()
            .any(|c| matches!(c,Call::Prompt {text,..} if text=="/quit")));
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 3);
        let (kind, args) = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart { kind, args, .. } = c {
                    Some((kind, args))
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(kind, "codex");
        assert!(
            args.windows(2).any(|w| w == ["-m", "new-model"]),
            "{args:?}"
        );
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&model_meta_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("new-model")
        );
        let ops = store
            .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Deliver))
            .unwrap();
        assert!(
            ops.iter().any(|op| op.payload["text"]
                .as_str()
                .is_some_and(|text| text.contains("Continue T-1"))),
            "{ops:?}"
        );
    }
    #[test]
    fn a_lane_that_came_back_on_its_old_model_is_restarted_again() {
        // T-400, the incident: a restart planned while the seat was Active is
        // lost when the captain holds the lane and releases it before the plan
        // finishes. The stored key already says the new chain was applied, so
        // only what the seat actually runs can still trigger the restart.
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "pi", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        assert!(pending(&mut store, "p:heat").unwrap().is_some());
        assert!(store
            .read(|tx| tx.get_meta("engine.restart_config.p:heat"))
            .unwrap()
            .unwrap()
            .contains("new-model"));
        // Held, the plan gone, released with its agent alive: the lane runs the
        // old model again.
        store
            .write(|tx| {
                tx.set_seat_state(
                    "p:heat",
                    SeatState::Held,
                    Some("hold: the user asked"),
                    T + 2,
                )
            })
            .unwrap();
        clear_pending(&mut store, "p:heat").unwrap();
        store
            .write(|tx| {
                tx.set_seat_state(
                    "p:heat",
                    SeatState::Active,
                    Some("released by the captain: its agent continues"),
                    T + 3,
                )
            })
            .unwrap();
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&model_meta_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("old-model")
        );
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 3).unwrap();
        assert!(
            pending(&mut store, "p:heat").unwrap().is_some(),
            "the seat still runs the old model, so the change is applied again"
        );
        advance_pending(&mut store, &mut herdr, &cfg, T + 4);
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 5);
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart { args, .. } = c {
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            args.windows(2).any(|w| w == ["--model", "new-model"]),
            "{args:?}"
        );
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&model_meta_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("new-model")
        );
    }
    #[test]
    fn review_reinit_cli_change_retaining_old_cli_as_fallback() {
        // F1 of the T-400 review: the old CLI staying a later entry of the new
        // chain must not swallow an explicit CLI change.
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        // Same model, new preferred CLI; the old CLI remains a fallback.
        change(&mut store, "codex", "old-model");
        store.write(|tx| {
            let mut p = tx.get_project("p")?.unwrap();
            p.config_toml = p.config_toml.replace(
                "lanes = [{model=\"old-model\", cli=\"codex\"}]",
                "lanes = [{model=\"old-model\", cli=\"codex\"}, {model=\"old-model\", cli=\"pi\"}]");
            tx.update_project(&p)
        }).unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        assert!(pending(&mut store, "p:heat").unwrap().is_some(),
            "explicit pi-to-codex config change must schedule a restart, even when pi remains a fallback");
    }
    #[test]
    fn a_lane_on_a_later_chain_entry_is_left_to_the_quota_step() {
        // The comparison with what the seat runs must not fight a quota
        // fallback: a lane on a later entry of its own chain waits for the
        // quota step, as before (T-400).
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        store.write(|tx| {let mut p=tx.get_project("p")?.unwrap();
            p.config_toml=p.config_toml.replace("lanes = [{model=\"old-model\", cli=\"pi\"}]","lanes = [{model=\"old-model\", cli=\"pi\"}, {model=\"new-model\", cli=\"pi\"}]");
            tx.update_project(&p)}).unwrap();
        store
            .write(|tx| tx.set_meta(&model_meta_key("p:heat"), "new-model"))
            .unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        store
            .write(|tx| {
                let mut p = tx.get_project("p")?.unwrap();
                p.config_version += 1;
                tx.update_project(&p)
            })
            .unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
        assert!(!herdr.calls.iter().any(|c| matches!(
            c,
            Call::Prompt { .. } | Call::AgentStart { .. } | Call::SendKeys { .. }
        )));
    }
    #[test]
    fn model_change_made_while_the_lane_was_held_is_applied_on_release() {
        // T-400: a hold takes the lane out of reconciliation, so the change
        // waits; the release puts the seat back in and the comparison with the
        // registered chain runs as for any other seat.
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        let seeded = store
            .read(|tx| tx.get_meta("engine.restart_config.p:heat"))
            .unwrap();
        assert!(seeded.is_some());
        store
            .write(|tx| {
                tx.set_seat_state("p:heat", SeatState::Held, Some("hold: the user asked"), T)
            })
            .unwrap();
        change(&mut store, "pi", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        assert!(
            pending(&mut store, "p:heat").unwrap().is_none(),
            "a held lane is not restarted"
        );
        assert_eq!(
            store
                .read(|tx| tx.get_meta("engine.restart_config.p:heat"))
                .unwrap(),
            seeded,
            "the hold keeps the stored configuration untouched"
        );
        assert!(!herdr.calls.iter().any(|c| matches!(
            c,
            Call::Prompt { .. } | Call::AgentStart { .. } | Call::SendKeys { .. }
        )));
        // Released with its agent still alive: the seat is Active again.
        store
            .write(|tx| {
                tx.set_seat_state(
                    "p:heat",
                    SeatState::Active,
                    Some("released by the captain: its agent continues"),
                    T + 2,
                )
            })
            .unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 2).unwrap();
        assert!(
            pending(&mut store, "p:heat").unwrap().is_some(),
            "the change made while held is applied on release"
        );
        advance_pending(&mut store, &mut herdr, &cfg, T + 3);
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 4);
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart { args, .. } = c {
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            args.windows(2).any(|w| w == ["--model", "new-model"]),
            "{args:?}"
        );
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&model_meta_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("new-model")
        );
    }
    /// T-407: every phase of a restart onto another model is one line of the
    /// engine log, from the durable time the phase began with.
    #[test]
    fn a_restart_logs_one_probe_line_per_phase() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        // Its own seat, so no other test's restart lines mix in (T-407).
        store
            .write(|tx| {
                testing::seed_active_seat(tx, "p", "probe", SeatRole::Lane, PANE, "pi", T)?;
                tx.set_binding_session_ref("p:probe", 1, "old-session")
            })
            .unwrap();
        let seat = store.read(|tx| tx.get_seat("p:probe")).unwrap().unwrap();
        let target = ModelEntry::Entry(gear_app::config::ModelSpec {
            model: "new-model".into(),
            cli: Some("pi".into()),
            args: vec![],
            context_window_k: None,
        });
        // The stop: an idle agent gets its exit keys, the phase moves on.
        assert!(advance(
            &mut store,
            &mut herdr,
            &cfg,
            &cfg.profiles,
            "p",
            &seat,
            Some(&target),
            T + 1
        )
        .unwrap()
        .is_none());
        herdr.exit_agent(PANE);
        let mut finished = None;
        for n in 2..10 {
            let seat = store.read(|tx| tx.get_seat("p:probe")).unwrap().unwrap();
            finished = advance(
                &mut store,
                &mut herdr,
                &cfg,
                &cfg.profiles,
                "p",
                &seat,
                None,
                T + n,
            )
            .unwrap();
            if finished.is_some() {
                break;
            }
        }
        assert!(
            finished.is_some(),
            "the restart finished: {:?}",
            herdr.calls
        );
        let log: Vec<String> = crate::ops::probe_log()
            .into_iter()
            .filter(|line| line.starts_with("probe restart p:probe "))
            .collect();
        let steps: Vec<&str> = log
            .iter()
            .map(|line| line.split_whitespace().nth(3).unwrap())
            .collect();
        assert_eq!(
            steps,
            ["stop", "exit", "start", "finish", "done"],
            "{log:?}"
        );
        // The phase that waited a tick shows it (the exit waits for the agent
        // to go); the last line's total is the whole restart's duration, from
        // the tick it began in to the tick it finished in.
        assert_eq!(
            log[1], "probe restart p:probe exit 1000 total=1000",
            "{log:?}"
        );
        assert_eq!(
            log.last().unwrap(),
            "probe restart p:probe done 1000 total=1000",
            "{log:?}"
        );
    }

    #[test]
    fn removing_the_model_chain_restarts_without_a_model_flag() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), true);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        store
            .write(|tx| {
                let mut p = tx.get_project("p")?.unwrap();
                p.config_toml = p
                    .config_toml
                    .replace("lanes = [{model=\"old-model\", cli=\"pi\"}]", "lanes = []");
                p.config_version += 1;
                tx.update_project(&p)
            })
            .unwrap();
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        assert!(pending(&mut store, "p:heat").unwrap().is_some());
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart { args, .. } = c {
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(!args.iter().any(|s| s == "--model"), "{args:?}");
        assert!(
            args.windows(2).any(|w| w == ["--session", "old-session"]),
            "{args:?}"
        );
    }
    #[test]
    fn config_model_change_resumes_open_session_on_the_same_cli() {
        // The binding is authoritative even before rounds bookkeeping has
        // recorded the session in the open round.
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "pi", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart {
                    kind, pane, args, ..
                } = c
                {
                    assert_eq!((kind.as_str(), pane.as_str()), ("pi", PANE));
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            args.windows(2).any(|w| w == ["--session", "old-session"]),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["--model", "new-model"]),
            "{args:?}"
        );
        let binding = store
            .read(|tx| tx.get_binding("p:heat", 2))
            .unwrap()
            .unwrap();
        assert_eq!(binding.pane_id, PANE);
        assert_eq!(binding.session_ref.as_deref(), Some("old-session"));
        assert_eq!(
            store
                .read(|tx| tx.get_attempt("p", "T-1", 1))
                .unwrap()
                .unwrap()
                .session_action
                .as_deref(),
            Some("resume")
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let before = herdr.calls.len();
        assert_eq!(
            crate::session::prepare_lane(&mut store, &mut herdr, &cfg, &seat, "T-1", 1, T + 3)
                .unwrap(),
            crate::session::Prep::Ready
        );
        assert!(
            !herdr.calls[before..].iter().any(|c| matches!(
                c,
                Call::Prompt { .. }
                    | Call::SendKeys { .. }
                    | Call::AgentStart { .. }
                    | Call::PaneType { .. }
            )),
            "the completed resume is not repeated"
        );
    }
    #[test]
    fn a_restart_carries_the_attempt_to_the_new_generation() {
        // T-217: the lane's attempt moves to the generation the restart
        // reserves, so the lane can still hand it in.
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change(&mut store, "pi", "new-model");
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.generation, 2);
        let attempt = store
            .read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap();
        assert_eq!(attempt.generation, 2, "the attempt moves with the seat");
    }
    #[test]
    fn opencode_resume_reuses_session_and_switches_its_model() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), true);
        change(&mut store, "opencode", "provider/new");
        store.write(|tx| {
            let mut p = tx.get_project("p")?.unwrap();
            p.config_toml.push_str("\n[models.switch.\"provider/new\"]\npicker='New Provider'\nbar='New Provider'\n");
            tx.update_project(&p)
        }).unwrap();
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let mut plan = Plan {
            model: "provider/new".into(),
            cli: "opencode".into(),
            args: vec![],
            generation: 0,
            phase: Phase::Start,
            waited: 0,
            old_cli: "opencode".into(),
            resume: Some("old-session".into()),
            resume_required: true,
            profile_name: Some("opencode".into()),
            at: T,
            started: T,
            at_once: false,
            grace_s: 0,
            wrap_at: 0,
            wrapped: false,
            handover_doc: None,
        };
        herdr.exit_agent(PANE);
        assert!(start(
            &mut store,
            &mut herdr,
            &cfg,
            &cfg.profiles,
            &seat,
            &mut plan,
            T + 1
        )
        .unwrap());
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let Call::AgentStart { args, .. } = c {
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            args.iter().filter(|s| s.as_str() == "old-session").count(),
            1,
            "{args:?}"
        );
        assert!(!herdr
            .calls
            .iter()
            .any(|c| matches!(c, Call::OpenCodeApi { args } if args[0] == "session.create")));
        assert_eq!(
            store
                .read(|tx| tx.get_binding("p:heat", plan.generation))
                .unwrap()
                .unwrap()
                .session_ref
                .as_deref(),
            Some("old-session")
        );
        let catalog = serde_json::json!({"data":[{"providerID":"provider","id":"new"}]});
        herdr.api_replies.extend([
            Ok(catalog.clone()),
            Ok(serde_json::json!({"data":{"model":{"providerID":"provider","id":"old"}}})),
            Ok(serde_json::json!({})),
        ]);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert!(finish(
            &mut store,
            &mut herdr,
            &cfg.profiles,
            "p",
            &seat,
            &plan,
            T + 2
        )
        .unwrap()
        .is_none());
        assert!(herdr.calls.iter().any(|c| matches!(c, Call::OpenCodeApi { args } if args[0] == "session.switchModel" && args[2] == "sessionID=old-session")));
        herdr.api_replies.extend([
            Ok(catalog),
            Ok(serde_json::json!({"data":{"model":{"providerID":"provider","id":"new"}}})),
        ]);
        herdr.set_screen(PANE, "Build auto · New Provider");
        assert!(finish(
            &mut store,
            &mut herdr,
            &cfg.profiles,
            "p",
            &seat,
            &plan,
            T + 3
        )
        .unwrap()
        .is_some());
        let before = herdr.calls.len();
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(
            crate::session::prepare_lane(&mut store, &mut herdr, &cfg, &seat, "T-1", 1, T + 4)
                .unwrap(),
            crate::session::Prep::Ready
        );
        assert!(!herdr.calls[before..]
            .iter()
            .any(|c| matches!(c, Call::AgentStart { .. } | Call::Prompt { .. })));
    }

    #[test]
    fn cross_cli_round_counts_each_sessions_own_log_and_missing_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let pi = dir.path().join("pi.jsonl");
        let codex = dir.path().join("codex.jsonl");
        let pi_record = serde_json::json!({"type":"message","timestamp":T+1,"message":{"role":"assistant","usage":{"input":10,"cost":{"total":0.25}}}});
        let codex_record = serde_json::json!({"type":"event_msg","timestamp":T+2,"payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"cached_input_tokens":0,"output_tokens":0}}}});
        std::fs::write(&pi, format!("{pi_record}\n")).unwrap();
        let context = serde_json::json!({"type":"turn_context","timestamp":T+1,"payload":{"model":"new-model"}});
        std::fs::write(&codex, format!("{context}\n{codex_record}\n")).unwrap();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap(), false);
        let key = RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        store
            .write(|tx| {
                tx.set_binding_session_ref("p:heat", 1, pi.to_str().unwrap())?;
                tx.add_round_session(&key, pi.to_str().unwrap())?;
                let mut p = tx.get_project("p")?.unwrap();
                p.config_toml.push_str(
                    "\n[prices.\"new-model\"]\ninput=2\noutput=0\ncache_read=0\ncache_write=0\n",
                );
                tx.update_project(&p)
            })
            .unwrap();
        herdr.exit_agent(PANE);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let mut plan = Plan {
            model: "new-model".into(),
            cli: "codex".into(),
            args: vec![],
            generation: 0,
            phase: Phase::Start,
            waited: 0,
            old_cli: "pi".into(),
            resume: None,
            resume_required: false,
            profile_name: Some("codex".into()),
            at: T,
            started: T,
            at_once: false,
            grace_s: 0,
            wrap_at: 0,
            wrapped: false,
            handover_doc: None,
        };
        assert!(start(
            &mut store,
            &mut herdr,
            &cfg,
            &cfg.profiles,
            &seat,
            &mut plan,
            T + 3
        )
        .unwrap());
        herdr.set_agent_session(PANE, codex.to_str().unwrap());
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert!(finish(
            &mut store,
            &mut herdr,
            &cfg.profiles,
            "p",
            &seat,
            &plan,
            T + 4
        )
        .unwrap()
        .is_some());
        store.write(|tx| tx.close_round(&key, T + 10)).unwrap();
        let round = store
            .read(|tx| tx.rounds_of_project("p"))
            .unwrap()
            .remove(0);
        let tokens = crate::rounds::count_round(&mut store, &cfg, &round, T + 20).unwrap();
        assert_eq!(tokens.input, Some(30));
        assert!((tokens.cost_usd.unwrap() - 0.25004).abs() < 1e-10);
        assert_eq!(tokens.cost_source, "prices");
        std::fs::remove_file(codex).unwrap();
        assert_eq!(
            crate::rounds::count_round(&mut store, &cfg, &round, T + 20)
                .unwrap()
                .input,
            None
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_app::config::{ModelEntry, ModelSpec};
    use gear_herdr::fake::FakeHerdr;
    use gear_model::{SeatRole, TaskState};
    use gear_store::{testing, ContractRow, TaskRow};

    const T: i64 = 1_800_000_000;
    const PANE: &str = "w1:p3";

    fn config() -> String {
        r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "work"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash", {model="gpt-6.1-sol", cli="claude"}]
"#
        .to_string()
    }

    /// A project with the lane's task T-1 started (a Continue can be
    /// delivered) and its seat active on opencode in `PANE`.
    fn world() -> (Store, FakeHerdr, EngineCfg) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T)?;
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config();
                tx.update_project(&project)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "opencode", T)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p1",
                    "claude",
                    T,
                )?;
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
                    goal: "goal".into(),
                    accept: "done".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T,
                    mechanical: None,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", AgentStatus::Idle);
        let mut cfg = EngineCfg::default();
        cfg.db_path = crate::testtmp::temp("gear-engine-restart-tests")
            .join("gear.db")
            .to_string_lossy()
            .into_owned();
        cfg.profiles = gear_herdr::builtins();
        (store, herdr, cfg)
    }

    fn lane_seat(store: &mut Store) -> SeatRow {
        store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap()
    }

    fn step(
        store: &mut Store,
        herdr: &mut FakeHerdr,
        cfg: &EngineCfg,
        target: Option<&ModelEntry>,
        now: Ts,
    ) -> GearResult<Option<String>> {
        let seat = lane_seat(store);
        advance(store, herdr, cfg, &cfg.profiles, "p", &seat, target, now)
    }

    /// Onboarding may set the restarted agent's seat `Active` on the plan's
    /// generation before the finish phase: the finish accepts it, books the
    /// model and the Continue, and clears the plan.
    #[test]
    fn finish_accepts_a_seat_onboarding_made_active_on_the_plans_generation() {
        let (mut store, mut herdr, cfg) = world();
        let target = ModelEntry::Entry(ModelSpec {
            model: "gpt-6.1-sol".into(),
            cli: Some("claude".into()),
            args: vec![],
            context_window_k: None,
        });
        // The plan begins: the old agent is asked to exit.
        assert_eq!(
            step(&mut store, &mut herdr, &cfg, Some(&target), T + 10).unwrap(),
            None
        );
        assert!(pending(&mut store, "p:heat").unwrap().is_some());

        // The old agent is gone; the start is refused once, so the generation
        // is reserved and the seat waits in `Starting` before the finish.
        herdr.exit_agent(PANE);
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));
        assert_eq!(
            step(&mut store, &mut herdr, &cfg, None, T + 11).unwrap(),
            None
        );
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Starting, "{:?}", seat.state_reason);
        let plan = pending(&mut store, "p:heat").unwrap().unwrap();
        assert_eq!(plan.phase, Phase::Start);
        assert_eq!(seat.generation, plan.generation);
        assert_eq!(plan.generation, 2);

        // Onboarding made the new agent's seat Active on the plan's generation
        // before the finish phase.
        store
            .write(|tx| tx.set_seat_state("p:heat", SeatState::Active, Some("onboarded"), T + 12))
            .unwrap();

        let line = step(&mut store, &mut herdr, &cfg, None, T + 13)
            .unwrap()
            .expect("a seat already Active on the plan's generation finishes the restart");
        assert!(line.contains("restarted on claude (gpt-6.1-sol)"), "{line}");
        assert_eq!(
            pending(&mut store, "p:heat").unwrap(),
            None,
            "the plan is cleared"
        );
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&model_meta_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("gpt-6.1-sol"),
            "the model meta is the plan's model"
        );
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Active);
        assert_eq!(seat.generation, plan.generation);
        let ops = store
            .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Deliver))
            .unwrap();
        assert_eq!(ops.len(), 1, "one Continue for the open attempt: {ops:?}");
        assert_eq!(ops[0].task.as_deref(), Some("T-1"));
        assert_eq!(ops[0].attempt, Some(1));
        assert_eq!(ops[0].generation, plan.generation);
        assert_eq!(ops[0].state, OpState::Intended);
    }
}

#[cfg(test)]
mod handover_tests {
    //! T-465: switching a seat's agent carries a handover. An at-once switch
    //! tells the outgoing agent to wrap up, waits the configured grace period
    //! and then exits it whether or not it handed in; the incoming agent's
    //! packet starts with the handover and the worktree's state, a dirty tree
    //! included.
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_model::{SeatRole, TaskState};
    use gear_store::{testing, ContractRow, NewRound, RoundKey, TaskRow};

    const T: i64 = 1_800_000_000;
    const PANE: &str = "w1:p6";
    const GRACE: i64 = 600;

    fn config(kind: &str, model: &str) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"
[quota]
lanes = 1
temps = 0
[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"
[[lanes]]
name = "heat"
duty = "work"
owns = ["src/"]
kind = "{kind}"
profile = "{kind}"
[models]
lanes = [{{model="{model}", cli="{kind}"}}]
[timing]
switch_at_once = true
switch_grace_min = 10
"#
        )
    }

    fn sh(dir: &std::path::Path, command: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{command}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A worktree with one committed file, then a dirty tracked file and an
    /// untracked one: what the handover must name.
    fn dirty_worktree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        sh(
            dir.path(),
            "git init -q -b work && git add -A && \
             git -c user.email=gear@example.invalid -c user.name=gear commit -qm init",
        );
        std::fs::write(dir.path().join("src/a.rs"), "fn a() { let _ = 1; }\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "untracked\n").unwrap();
        dir
    }

    fn world(worktree: &str) -> (Store, FakeHerdr, EngineCfg) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T)?;
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = config("pi", "old-model");
                tx.update_project(&project)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p1",
                    "codex",
                    T,
                )?;
                tx.set_binding_session_ref("p:heat", 1, "old-session")?;
                tx.set_meta(&model_meta_key("p:heat"), "old-model")?;
                tx.set_meta(&crate::onboard::worktree_path_key("p:heat"), worktree)?;
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
                    milestone: Some("M6".into()),
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
                    goal: "goal".into(),
                    accept: "cargo test".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T,
                    mechanical: None,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T)?;
                let key = RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: Some("old-model".into()),
                    start_at: T,
                })?;
                tx.add_round_session(&key, "old-session")?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, "pi", AgentStatus::Idle, "p-heat");
        herdr.set_agent_session(PANE, "old-session");
        let mut cfg = EngineCfg::default();
        cfg.profiles = gear_herdr::builtins();
        (store, herdr, cfg)
    }

    fn change_to_codex(store: &mut Store) {
        store
            .write(|tx| {
                let mut p = tx.get_project("p")?.unwrap();
                p.config_toml = config("codex", "new-model");
                p.config_version += 1;
                tx.update_project(&p)
            })
            .unwrap();
    }

    fn advance_pending(store: &mut Store, herdr: &mut FakeHerdr, cfg: &EngineCfg, now: i64) {
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        advance(store, herdr, cfg, &cfg.profiles, "p", &seat, None, now).unwrap();
    }

    fn quit_prompts(herdr: &FakeHerdr) -> usize {
        herdr
            .prompts_to(PANE)
            .iter()
            .filter(|text| **text == "/quit")
            .count()
    }

    #[test]
    fn an_at_once_switch_waits_the_grace_period_then_exits_even_while_working() {
        let dir = dirty_worktree();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap());
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change_to_codex(&mut store);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();

        // The outgoing agent was told once to wrap up and commit, and the
        // handover document is there for it to write its part into.
        let plan = pending(&mut store, "p:heat").unwrap().unwrap();
        assert!(plan.at_once, "{plan:?}");
        assert_eq!(plan.grace_s, GRACE, "{plan:?}");
        let wrap = herdr
            .prompts_to(PANE)
            .iter()
            .find(|text| text.starts_with("Wrap up and commit now"))
            .cloned()
            .unwrap_or_else(|| panic!("no wrap-up prompt: {:?}", herdr.calls));
        let doc = plan.handover_doc.clone().expect("the handover document");
        assert!(wrap.contains(&doc), "{wrap}");
        assert!(std::path::Path::new(&doc).exists(), "{doc}");
        assert_eq!(quit_prompts(&herdr), 0, "no exit during the wrap-up");

        // A model that thinks for hours must not hold the switch: while the
        // grace runs the agent is left alone, at its end it is exited anyway.
        herdr.set_status(PANE, AgentStatus::Working);
        let wrap_at = plan.wrap_at;
        advance_pending(&mut store, &mut herdr, &cfg, wrap_at + 1);
        assert_eq!(quit_prompts(&herdr), 0, "still inside the grace period");
        advance_pending(&mut store, &mut herdr, &cfg, wrap_at + GRACE - 1);
        assert_eq!(quit_prompts(&herdr), 0, "still inside the grace period");
        advance_pending(&mut store, &mut herdr, &cfg, wrap_at + GRACE);
        assert_eq!(quit_prompts(&herdr), 1, "the exit went out after the grace");
        let plan = pending(&mut store, "p:heat").unwrap().unwrap();
        assert_eq!(plan.phase, Phase::Exit, "{plan:?}");
    }

    #[test]
    fn the_incoming_packet_starts_with_the_handover_of_a_dirty_tree() {
        let dir = dirty_worktree();
        let (mut store, mut herdr, cfg) = world(dir.path().to_str().unwrap());
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T).unwrap();
        change_to_codex(&mut store);
        reconcile_config(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T + 1).unwrap();
        herdr.set_status(PANE, AgentStatus::Working);
        // Idle after the wrap-up: the exit goes now, the new agent starts and
        // the packet is rendered for it.
        herdr.set_status(PANE, AgentStatus::Idle);
        advance_pending(&mut store, &mut herdr, &cfg, T + 2);
        herdr.exit_agent(PANE);
        advance_pending(&mut store, &mut herdr, &cfg, T + 3);
        assert!(pending(&mut store, "p:heat").unwrap().is_none());

        let packet = store
            .read(|tx| gear_app::plan::render_task_packet(tx, "p", "T-1"))
            .unwrap();
        assert!(
            packet.starts_with("## Handover — you take over another agent's work"),
            "{packet}"
        );
        assert!(packet.contains("old-session"), "{packet}");
        assert!(packet.contains("Branch: work"), "{packet}");
        assert!(packet.contains("Uncommitted changes: src/a.rs"), "{packet}");
        assert!(packet.contains("Untracked files: new.txt"), "{packet}");
        assert!(packet.contains("handovers/heat-"), "{packet}");
        assert!(packet.contains("Then the task packet:"), "{packet}");
        // The task packet follows the handover.
        let handover = packet.find("## Handover").unwrap();
        let task = packet.find("# T-1 — goal").unwrap();
        assert!(handover < task, "{packet}");
        // The document holds part 1 and leaves parts 2 and 3 to the agent.
        let doc = pending_doc(&mut store);
        let text = std::fs::read_to_string(&doc).unwrap();
        assert!(text.contains("## 1. Facts"), "{text}");
        assert!(text.contains("## 2. Summary"), "{text}");
        assert!(text.contains("## 3. Free notes"), "{text}");
        assert!(text.contains("Uncommitted changes: src/a.rs"), "{text}");
        assert!(
            sh(dir.path(), "git log -1 --format=%s").contains("gear: handover of p:heat"),
            "the document is committed on the seat's branch"
        );
    }

    fn pending_doc(store: &mut Store) -> String {
        let raw = store
            .read(|tx| tx.get_meta(&crate::handover::meta_key("p:heat")))
            .unwrap()
            .expect("the handover block");
        serde_json::from_str::<serde_json::Value>(&raw).unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_string()
    }
}
