//! Quota policy (PRD §9, design/01 `[models].on_quota`): what the engine does
//! when a lane's agent stops on a provider limit.
//!
//! When a lane's screen shows a provider limit (`Profile::quota_markers`) for
//! the profile's `confirm_rounds` ticks in a row, the engine applies the lane
//! policy of the project config:
//!
//! - `fallback`: move the lane to the next entry of its chain. The chain is
//!   `[models].lanes`, or the lane's own `models` when it has one.
//! - `wait`, or a chain with no next entry: park the seat until the provider
//!   resets (`[models].quota_reset_h` hours) and resume on the first model
//!   then.
//!
//! How the model changes depends on the **CLI** the next entry names (T-35)
//! and on whether GEAR drives that CLI's model picker (T-114):
//!
//! - the same CLI **with** a picker GEAR drives (OpenCode): the model changes
//!   **in place** (T-33) — the picker, the variant dialog and the status bar;
//!   the conversation stays.
//! - another CLI, or a CLI without such a picker (pi): the agent is closed
//!   and that CLI starts in the same pane and worktree ([`crate::restart`]) —
//!   no session crosses a restart; the new agent starts from the git state and
//!   the lane is told to continue.
//!
//! The engine keeps the little state this needs in the store's `meta`
//! (consecutive quota ticks, the lane's current model, the reset deadline, the
//! restart in progress): the `seats` table has `park_until`, but gear-store
//! exposes no setter for it yet, and a store API is the follow-up.
//!
//! # The quota of the providers (design §9.1-§9.2)
//!
//! The screen is not the source of quota facts: a marker on the screen or a
//! provider error is only a **signal**. The engine reads the provider's API
//! ([`gear_usage::read_quota`]) with the key the config names
//! (`[[quota.sources]]`, §9.1) and the reading decides:
//!
//! - a window is spent (0 % left): the seat is parked until that window's
//!   reset + 1 min, the round's clock pauses, a pause row of kind `quota` is
//!   opened (§5.4) and, at the reset, the seat continues in the same session
//!   (`Continue T-n: your quota is back.`, §3.3 S4);
//! - quota is left: not a quota stop but a rate limit or an error — the seat
//!   waits 5 min and continues; the second time in a row it becomes a captain
//!   item (`<seat>: provider error, not quota`);
//! - the source cannot be read: today's park, `quota_reset_h` hours.
//!
//! Every source is read every 5 min and its last reading is kept in the meta
//! `quota.reading.<provider>` (§9.1); a reading older than 15 min is stale.

use gear_app::config::{ModelEntry, ProjectConfig, QuotaPolicy, WaitPolicy};
use gear_herdr::{HerdrAct, Outcome, Profile, ScreenFlag};
use gear_model::{
    ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, OpState, SeatRole, SeatState, Ts,
};
use gear_store::{BindingStatus, NewMessage, NewOp, RoundKey, SeatRow, Store, Tx};
use gear_usage::{QuotaReader, Reading};

use crate::model::model_meta_key;
use crate::observe::SeatObs;
use crate::EngineCfg;

/// The restart phases of a lane that is switching models (F3 of the T-15
/// review): after the exit keys the engine waits for the pane to show a
/// **shell**; only then is the start attempted.

/// What the quota step did in one tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaReport {
    /// One line per switch or park, for the engine log.
    pub events: Vec<String>,
}

/// Apply the lane quota policy to the observed seats of `project`.
pub fn apply_quota_policy(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
) -> GearResult<QuotaReport> {
    apply_quota_policy_with(
        store, herdr, cfg, profiles, project, observed, now, &LiveQuota,
    )
}

/// The quota policy with the readers of §9.1 as a parameter: the live ones
/// call the provider's API, a test a fake — so every branch of §9.2 is shown
/// without a provider.
pub fn apply_quota_policy_with(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
    api: &dyn QuotaApi,
) -> GearResult<QuotaReport> {
    let mut report = QuotaReport::default();
    let Some(row) = store.read(|tx| tx.get_project(project))? else {
        return Ok(report);
    };
    let config = match gear_app::config::parse(&row.config_toml) {
        Ok(config) => config,
        // A config that does not parse: no policy can be applied.
        Err(_) => return Ok(report),
    };
    // §9.1: the sources of the project and their last readings. Every source
    // is read every 5 min; a reading older than 15 min is stale.
    let sources = sources_of(&config);
    // The codex reader reads the rollout of the reviewer's session (§9.1).
    let codex_session = codex_session(store, project)?;
    report.events.extend(refresh_readings(
        store,
        api,
        &sources,
        now,
        codex_session.as_deref(),
    )?);

    for obs in observed {
        let seat = &obs.seat;
        // A lane's chain is its own; a reviewer's is `[models].review` (T-115):
        // a Codex reviewer whose limit is spent moves to the next review model
        // (`on_quota.review`), which is what makes done clause (2) work.
        let reviewer = seat.role == SeatRole::Temp;
        if !reviewer && seat.role != SeatRole::Lane {
            continue;
        }
        // A restart on another CLI is on its way (T-35): the plan is durable,
        // so it continues whatever the seat's state says now — the seat is
        // `Missing` or `Starting` while its agent is replaced. It comes
        // before the profile: the plan drives the restart, not the seat's
        // current profile.
        if crate::restart::pending(store, &seat.id)?.is_some() {
            // One seat's failed restart is reported; it never stops the project's tick.
            match crate::restart::advance(store, herdr, cfg, profiles, project, seat, None, now) {
                Ok(Some(line)) => report.events.push(line),
                Ok(None) => {}
                Err(e) => report
                    .events
                    .push(format!("{}: restart step failed: {e}", seat.id)),
            }
            continue;
        }
        let Some(profile) = profiles.get(&seat.profile) else {
            continue;
        };
        // A switch already on its way finishes before anything else (F6): the
        // plan is durable, so it resumes whatever the screen says now — the
        // quota line may have cleared, the engine may have restarted mid-switch.
        if crate::switch::pending(store, &seat.id)?.is_some() {
            if let Some(line) = resume_switch(
                store,
                herdr,
                project,
                &config,
                seat,
                profile,
                Some(obs),
                now,
            )? {
                report.events.push(line);
            }
            continue;
        }
        // A seat parked on quota wakes up when the provider resets: it goes
        // back to the first model of its chain — in place when that model runs
        // on the seat's own CLI, on another CLI when it does not (T-35). A
        // reviewer is never parked (its role has no Parked state, T-115), so
        // this is the lane's path.
        if seat.state == SeatState::Parked {
            // T-326: with `on_wait = "release"` a lane parked on a long wait
            // does not hold its agent: the agent exits until the reset, and its
            // pane, worktree and session are kept.
            if let Some(line) =
                release_for_wait(store, herdr, project, &config, profile, seat, now, false)?
            {
                report.events.push(line);
                continue;
            }
            // T-327: a released lane has no agent to take a Continue or a
            // model switch — its pane is a shell. At the reset the same CLI
            // comes back in the same pane with the round's session, and the
            // Continue waits for it ([`bring_back`]). A seat that was not
            // released wakes exactly as before.
            if meta_get(store, &meta_key("wait_released", &seat.id))?.as_deref() == Some("1") {
                let over = match load_wait(store, &seat.id)? {
                    Some(wait) => wait.until <= now,
                    None => parked_until(store, &seat.id)?.is_some_and(|until| until <= now),
                };
                if over {
                    if let Some(line) =
                        bring_back(store, herdr, cfg, profiles, project, &config, seat, now)?
                    {
                        report.events.push(line);
                    }
                    continue;
                }
            }
            // §9.2: a wait the source decided (a spent window, a provider
            // error, an unreadable source) ends at its own time: the seat
            // continues in the same session.
            if let Some(line) = wake_from_source(store, project, seat, now)? {
                report.events.push(line);
                continue;
            }
            if parked_until(store, &seat.id)?.is_some_and(|until| until <= now) {
                let chain = crate::model::role_chain(&config, &seat.name);
                match chain.first().copied() {
                    Some(entry) => {
                        if let Some(line) = change_model(
                            store,
                            herdr,
                            cfg,
                            profiles,
                            project,
                            &config,
                            seat,
                            profile,
                            Some(obs),
                            entry,
                            now,
                        )? {
                            report.events.push(line);
                        }
                    }
                    None => {
                        flag_and_alert(
                            store,
                            project,
                            seat,
                            "quota: the lane's chain is empty; no model to switch back to",
                            now,
                        )?;
                    }
                }
            }
            continue;
        }
        // Only a live seat gets the policy: a lane may be Standby or Retiring
        // (it still finishes its attempt), a reviewer is Active or it is not
        // reviewing (T-115).
        let live = if reviewer {
            seat.state == SeatState::Active
        } else {
            matches!(
                seat.state,
                SeatState::Active | SeatState::Standby | SeatState::Retiring
            )
        };
        if !live {
            continue;
        }
        // 1. count the consecutive quota ticks. A marker left on the screen
        //    from before (after a model switch the old limit line stays in the
        //    tail) is not a new fact, and a working agent is never parked on
        //    screen text (M-266): only a stopped agent counts.
        let quota = obs.flags.contains(&ScreenFlag::Quota) && obs.idle;
        let (ticks, quota_start) = count_quota_tick(store, &seat.id, quota, now)?;
        // A provider error is a signal too (§9.2): the source tells a spent
        // quota from an error, and the error is not a quota stop.
        let error = obs.flags.contains(&ScreenFlag::Error) && obs.idle;
        let (error_ticks, error_start) = count_error_tick(store, &seat.id, error, now)?;
        let confirmed = (quota && ticks >= i64::from(profile.confirm_rounds))
            || (error && error_ticks >= i64::from(profile.confirm_rounds));
        // §9.2: the source of the seat's provider decides. Without a source
        // the policy of the config decides (below); a reviewer has no source
        // of its own here — it keeps the policy of its chain (T-115).
        if !reviewer && (quota || error) {
            let chain = crate::model::role_chain(&config, &seat.name);
            if let Some(source) = source_of(&sources, &chain, store, seat)? {
                // A new signal reads the source at once (F1): the reading of
                // the signal's episode is the one that decides. A read made
                // after the signal began is shared by the confirmation ticks
                // and the other seats of the same provider.
                let since = quota_start.max(error_start);
                let read = read_source(store, api, &source, now, since, codex_session.as_deref());
                if confirmed {
                    match read {
                        Ok(reading) => {
                            match verdict_of(&reading, now, config.models.quota_reset_h) {
                                Verdict::Spent { window, until } => {
                                    let line = park_for_quota(
                                        store,
                                        project,
                                        seat,
                                        &source.provider,
                                        &window,
                                        until,
                                        now,
                                    )?;
                                    report.events.push(line.clone());
                                    // A parked lane is work stopped for hours:
                                    // an urgent item that rings the captain
                                    // (L-74).
                                    alert_park(store, project, seat, &line, now)?;
                                }
                                Verdict::Left => {
                                    let episodes = count_error_episode(store, &seat.id)?;
                                    let line = wait_for_provider(
                                        store,
                                        project,
                                        seat,
                                        &source.provider,
                                        episodes >= 2,
                                        now,
                                    )?;
                                    report.events.push(line);
                                }
                            }
                            continue;
                        }
                        Err(_) => {
                            // The source cannot be read: today's park,
                            // `quota_reset_h` hours (§9.2).
                            let line = park_unreadable(
                                store,
                                project,
                                seat,
                                &source.provider,
                                config.models.quota_reset_h,
                                now,
                            )?;
                            report.events.push(line.clone());
                            alert_park(store, project, seat, &line, now)?;
                            continue;
                        }
                    }
                }
            }
        }
        if !quota || ticks < i64::from(profile.confirm_rounds) {
            continue;
        }
        // A reviewer whose verdict is in (or whose attempt has no candidate)
        // has no job to move for: the release step frees its seat (T-115).
        if reviewer && crate::review::job_for_seat(store, project, seat)?.is_none() {
            continue;
        }
        // 2. the policy. A reviewer's chain and policy are their own (T-115).
        let chain: Vec<&ModelEntry> = if reviewer {
            crate::model::review_chain(&config)
        } else {
            crate::model::role_chain(&config, &seat.name)
        };
        let current = if reviewer {
            crate::review::seat_current_model(store, project, &config, seat)?
        } else {
            current_model(store, &seat.id, &chain)
        };
        let policy = if reviewer {
            config.models.on_quota.review
        } else {
            config.models.on_quota.lanes
        };
        let next = match policy {
            QuotaPolicy::Wait => None,
            QuotaPolicy::Fallback => next_entry(&chain, current.as_deref()),
        };
        match next {
            Some(next) => {
                // T-33: the model changes **in place** when the next model runs
                // on the seat's own CLI — no exit, no restart, the
                // conversation stays. Without labels the seat is flagged,
                // never closed (F4): a restart cannot carry a model through
                // the shared OpenCode service.
                // T-35: the next model needs another CLI — the agent is
                // closed and the other CLI starts in the same pane.
                if let Some(line) = change_model(
                    store,
                    herdr,
                    cfg,
                    profiles,
                    project,
                    &config,
                    seat,
                    profile,
                    Some(obs),
                    next,
                    now,
                )? {
                    report.events.push(line);
                }
            }
            // The chain is exhausted, or the policy is to wait: park until the
            // provider resets.
            None if reviewer => {
                // A reviewer cannot be parked (its role has no Parked state,
                // T-115): it is flagged and the captain hears about it once,
                // with the task whose review waits.
                let line = format!(
                    "{}: quota: the review chain has no next model for {} ({}) — the review of {} waits for a human",
                    seat.id,
                    current.as_deref().unwrap_or("no model"),
                    match policy {
                        QuotaPolicy::Wait => "policy wait",
                        QuotaPolicy::Fallback => "chain exhausted",
                    },
                    seat.task_id.clone().unwrap_or_default(),
                );
                flag_and_alert(store, project, seat, &line, now)?;
                report.events.push(line);
            }
            None => {
                // T-309: the limit line on the screen often states when the
                // quota comes back. Without a quota source the park ends at
                // that reset plus a minute when the screen states one, else
                // after `quota_reset_h` hours as before.
                let hours = i64::from(config.models.quota_reset_h.max(1));
                let from_screen = obs
                    .screen
                    .as_deref()
                    .and_then(|screen| gear_usage::reset_from_text(screen, now));
                let (until, reset) = match from_screen {
                    Some(reset) => (reset + 60, "reset read from the screen"),
                    None => (now + hours * 3600, "no reset on the screen: quota_reset_h"),
                };
                park(store, seat, current.as_deref(), until, now)?;
                // T-295 v2: a `wait`-policy park writes a wait, so the reset
                // takes `wake_from_source` and continues the lane in its own
                // session. A `fallback` chain-exhausted park keeps its reset
                // restart back to the original model (T-33/T-35), and
                // `change_model` stays for a seat that has no wait.
                if policy == QuotaPolicy::Wait {
                    let task = open_attempt(store, &seat.id)?.map(|(task, _)| task);
                    let provider = current
                        .as_deref()
                        .map(|model| model.split('/').next().unwrap_or(model).to_string())
                        .unwrap_or_default();
                    let wait = Wait {
                        kind: "policy".to_string(),
                        until,
                        started_at: now,
                        provider,
                        window: String::new(),
                        task,
                        pause: true,
                    };
                    store.write(|tx| write_wait(tx, &seat.id, &wait))?;
                }
                let line = format!(
                    "{}: parked until {until} (provider limit, {}, {reset})",
                    seat.id,
                    match policy {
                        QuotaPolicy::Wait => "policy wait",
                        QuotaPolicy::Fallback => "chain exhausted",
                    }
                );
                // A parked lane is work stopped for hours: an urgent item that
                // rings the captain, never an info line (L-74).
                alert_captain(store, project, seat, &line, now)?;
                report.events.push(line);
            }
        }
    }
    Ok(report)
}

/// The model the lane runs on now: the one the last switch recorded, else the
/// first of its chain.
fn current_model(store: &mut Store, seat: &str, chain: &[&ModelEntry]) -> Option<String> {
    match meta_get(store, &model_meta_key(seat)).ok().flatten() {
        Some(model) if !model.is_empty() => Some(model),
        _ => chain.first().map(|entry| entry.model().to_string()),
    }
}

/// The entry after `current` in the chain; `None` when the chain is exhausted
/// (or `current` is not in it: then the first entry is the next one).
fn next_entry<'a>(chain: &[&'a ModelEntry], current: Option<&str>) -> Option<&'a ModelEntry> {
    let Some(current) = current else {
        return chain.first().copied();
    };
    match chain.iter().position(|entry| entry.model() == current) {
        Some(index) => chain.get(index + 1).copied(),
        None => chain.first().copied(),
    }
}

// ------------------------------------------------------- in place or another CLI

/// Change the lane's model to `entry`'s.
///
/// The model changes **in place** when the entry runs on the seat's own CLI
/// **and** GEAR drives that CLI's model picker (T-33): no exit, no restart, the
/// conversation stays. Today that is OpenCode only.
///
/// Every other case is a **restart** (T-35, T-114): the agent is closed, the
/// CLI of the entry starts in the same pane and worktree, and the lane
/// continues its task. That is the case for another CLI, and for a CLI
/// without a picker GEAR drives (pi): the model is given at the start, so the
/// next model needs a new session.
fn change_model(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, Profile>,
    project: &str,
    config: &ProjectConfig,
    seat: &SeatRow,
    profile: &Profile,
    obs: Option<&SeatObs>,
    entry: &ModelEntry,
    now: Ts,
) -> GearResult<Option<String>> {
    let current_cli = crate::restart::current_cli(store, seat)?;
    // The entry names the CLI that runs the model; without one the seat's own
    // CLI runs it.
    let default_cli = config
        .lanes
        .iter()
        .find(|lane| lane.name == seat.name)
        .map(|lane| lane.kind.as_str());
    let target_cli = entry
        .cli()
        .or(default_cli)
        .map(str::to_string)
        .or(current_cli.clone());
    let same_cli = match (&target_cli, &current_cli) {
        (Some(target), Some(current)) => target == current,
        _ => true,
    };
    // T-114: the in-place switch needs a model picker GEAR drives. A CLI
    // without one (pi) is started again on the next model instead — the same
    // pane and worktree, the lane's task delivered again in the new session.
    // A CLI that is not known at all keeps the old path, which flags the seat
    // with a reason.
    let in_place = match target_cli.as_deref() {
        None => true,
        Some(cli) => same_cli && crate::switch::switches_in_place(cli),
    };
    if in_place {
        return start_switch(
            store,
            herdr,
            project,
            config,
            seat,
            profile,
            obs,
            entry.model(),
            now,
        );
    }
    let target = ModelEntry::Entry(gear_app::config::ModelSpec {
        model: entry.model().to_string(),
        cli: target_cli,
        args: entry.args().to_vec(),
        context_window_k: entry.context_window_k(),
    });
    crate::restart::advance(
        store,
        herdr,
        cfg,
        profiles,
        project,
        seat,
        Some(&target),
        now,
    )
}

// ------------------------------------------------------------- the in-place switch

/// Start (or continue) an in-place switch to `model`, one UI step per tick.
///
/// No labels for the model: the seat is flagged (`NeedsCheck`) and nothing is
/// typed — a restart cannot carry the model through OpenCode's shared service
/// (review F4).
fn start_switch(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    config: &ProjectConfig,
    seat: &SeatRow,
    profile: &Profile,
    obs: Option<&SeatObs>,
    model: &str,
    now: Ts,
) -> GearResult<Option<String>> {
    let Some(labels) = config.models.switch.get(model) else {
        let line = format!(
            "{}: no [models.switch.\"{model}\"] labels (picker, bar) — needs check, not restarted",
            seat.id
        );
        flag_and_alert(store, project, seat, &line, now)?;
        return Ok(Some(line));
    };
    let outcome =
        crate::switch::switch_model(store, herdr, &seat.id, profile, obs, labels, model, now)?;
    settle_switch(store, project, seat, model, outcome, now)
}

/// Continue the switch a seat has on its way, whatever the screen says now
/// (review F6): the plan carries the target, so a cleared quota line, an open
/// dialog or an engine restart does not lose it.
fn resume_switch(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    config: &ProjectConfig,
    seat: &SeatRow,
    profile: &Profile,
    obs: Option<&SeatObs>,
    now: Ts,
) -> GearResult<Option<String>> {
    let Some(plan) = crate::switch::pending(store, &seat.id)? else {
        return Ok(None);
    };
    // The plan keeps the bar text; the config gives back the model id and its
    // picker filter.
    let target = crate::switch::plan_target(store, herdr, config, &seat.id, &plan.to)?;
    let Some((model, labels)) = target else {
        crate::switch::clear_pending(store, &seat.id)?;
        let line = format!(
            "{}: the switch target {:?} has no [models.switch] labels any more — needs check",
            seat.id, plan.to
        );
        flag_and_alert(store, project, seat, &line, now)?;
        return Ok(Some(line));
    };
    let outcome =
        crate::switch::switch_model(store, herdr, &seat.id, profile, obs, &labels, &model, now)?;
    settle_switch(store, project, seat, &model, outcome, now)
}

/// What a switch step means for the lane: `Done`/`Already` finish it, `Waiting`
/// is silence (the next tick continues), `Failed` flags the seat.
fn settle_switch(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    model: &str,
    outcome: crate::switch::SwitchOutcome,
    now: Ts,
) -> GearResult<Option<String>> {
    match outcome {
        crate::switch::SwitchOutcome::Done | crate::switch::SwitchOutcome::Already => {
            Ok(Some(finish_switch(store, project, seat, model, now)?))
        }
        crate::switch::SwitchOutcome::Waiting => Ok(None),
        crate::switch::SwitchOutcome::Failed => {
            let line = format!(
                "{}: the model switch to {model} did not confirm twice — needs check",
                seat.id
            );
            flag_and_alert(store, project, seat, &line, now)?;
            Ok(Some(line))
        }
    }
}

/// A confirmed in-place switch (T-33): the new model, the reset quota counter,
/// the cleared plan, the resumed seat state and the one "Continue" op land in a
/// single transaction (review F7) — an engine restart sees either none of it
/// (the plan resumes) or all of it (the op exists). The captain's line follows.
fn finish_switch(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    model: &str,
    now: Ts,
) -> GearResult<String> {
    let (text, open) = resume_delivery(store, project, seat)?;
    store.write(|tx| {
        // T-97: a delivery the lane never confirmed before the switch — the
        // prompt the provider limit killed — is retired, not failed through
        // its check budget: the policy is handling this seat, so the captain is
        // not rung as for a stall, and the Continue below is its delivery
        // again. The slot is free for it.
        retire_unconfirmed_deliveries(tx, project, seat, now)?;
        tx.set_meta(&model_meta_key(&seat.id), model)?;
        tx.set_meta(&quota_meta_key(&seat.id), "0")?;
        tx.set_meta(&meta_key("resume", &seat.id), "")?;
        tx.set_meta(&crate::switch::pending_meta_key(&seat.id), "")?;
        if let Some((task, attempt)) = open.clone() {
            tx.insert_op(&NewOp {
                project: project.to_string(),
                seat: seat.id.clone(),
                generation: seat.generation,
                kind: OpKind::Deliver,
                task: Some(task),
                attempt: Some(attempt),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": text }),
                created: now,
            })?;
        }
        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        if row.state == SeatState::Parked {
            gear_model::state::require_transition(
                SeatState::can_transition(row.role, row.state, SeatState::Active),
                "seat",
                row.state.as_str(),
                SeatState::Active.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Active, Some("quota switch"), now)?;
        }
        Ok(())
    })?;
    let line = format!("{}: switched to {model} in place (provider limit)", seat.id);
    tell_captain(store, project, seat, &line, now)?;
    Ok(line)
}

/// Retire the seat's deliveries that Herdr accepted and the lane never
/// confirmed (T-97). An in-place switch types into the agent after them, so the
/// prompt cannot be trusted; the switch's own Continue op sends the attempt
/// again. No flag: this is the quota policy doing its job, not a stall.
fn retire_unconfirmed_deliveries(
    tx: &Tx<'_>,
    project: &str,
    seat: &SeatRow,
    now: Ts,
) -> GearResult<()> {
    for state in [OpState::Submitted, OpState::Unknown] {
        for op in tx.ops_in_state(project, state)? {
            if op.kind != OpKind::Deliver || op.seat != seat.id || op.generation != seat.generation
            {
                continue;
            }
            tx.set_op_state(
                project,
                &op.id,
                OpState::Failed,
                Some("the model switched before the delivery was confirmed: the switch sends it again on the new model"),
                now,
            )?;
        }
    }
    Ok(())
}

/// `Continue T-<n>: run `gear` first` for the lane's open attempt.
pub(crate) fn continue_text(store: &mut Store, seat: &str) -> GearResult<String> {
    match open_attempt(store, seat)? {
        Some((task, _)) => Ok(format!("Continue {task}: run `gear` first")),
        None => Ok("Continue: run `gear` first".to_string()),
    }
}

/// What a seat is told to carry on with after its model changed (T-115): a
/// reviewer gets its review packet again, for the same task and attempt — its
/// conversation did not cross the CLI, so the job has to be handed to it a
/// second time; a lane gets its Continue line.
pub(crate) fn resume_delivery(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
) -> GearResult<(String, Option<(String, i64)>)> {
    if seat.role == SeatRole::Temp {
        return match crate::review::job_for_seat(store, project, seat)? {
            Some(job) => Ok((job.packet, Some((job.task, job.attempt)))),
            // No job left (its verdict is in): a short line, no stale packet.
            None => Ok((
                format!(
                    "No review is waiting for you: run `gear` first{}.",
                    match seat.task_id.as_deref() {
                        Some(task) => format!(" ({task}'s verdict is in)"),
                        None => String::new(),
                    }
                ),
                None,
            )),
        };
    }
    Ok((
        continue_text(store, &seat.id)?,
        open_attempt(store, &seat.id)?,
    ))
}

/// The seat's open attempt: (task, attempt).
pub(crate) fn open_attempt(store: &mut Store, seat: &str) -> GearResult<Option<(String, i64)>> {
    store.read(|tx| {
        Ok(tx
            .open_attempts_of_seat(seat)?
            .into_iter()
            .next()
            .map(|attempt| (attempt.task, attempt.n)))
    })
}

/// Park the seat: no assignment until the provider resets.
fn park(
    store: &mut Store,
    seat: &SeatRow,
    model: Option<&str>,
    until: Ts,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        gear_model::state::require_transition(
            SeatState::can_transition(seat.role, seat.state, SeatState::Parked),
            "seat",
            seat.state.as_str(),
            SeatState::Parked.as_str(),
        )?;
        tx.set_seat_state(
            &seat.id,
            SeatState::Parked,
            Some(&format!(
                "quota: {}; resume at {until}",
                model.unwrap_or("no model")
            )),
            now,
        )
    })?;
    meta_set(store, &meta_key("resume", &seat.id), &until.to_string())
}

/// Park a failed switch and notify the captain in one transaction. The state
/// transition suppresses duplicate alerts on later ticks.
pub(crate) fn flag_and_alert(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        if row.state == SeatState::NeedsCheck {
            return Ok(());
        }
        gear_model::state::require_transition(
            SeatState::can_transition(row.role, row.state, SeatState::NeedsCheck),
            "seat",
            row.state.as_str(),
            SeatState::NeedsCheck.as_str(),
        )?;
        tx.set_seat_state(&seat.id, SeatState::NeedsCheck, Some(reason), now)?;
        insert_alert(tx, project, &row, reason, now)
    })
}

/// The reset deadline of a parked lane, if it was parked on quota.
fn parked_until(store: &mut Store, seat: &str) -> GearResult<Option<Ts>> {
    Ok(meta_get(store, &meta_key("resume", seat))?.and_then(|value| value.parse::<Ts>().ok()))
}

/// The shortest wait that justifies releasing a lane's agent (T-326): 15 min.
const RELEASE_MIN_WAIT_S: i64 = 900;

/// Release a lane's agent for a long wait (`[models].on_wait = "release"`,
/// T-326): the agent exits until the reset, so it uses nothing, while its
/// pane, worktree and session are kept — the comeback starts the same CLI in
/// the same pane and resumes the session in it ([`crate::restart`]).
///
/// The seat stays `Parked` and its binding stays `Active`: nothing but the
/// agent changes. `Some(line)` means the exit went out (or may have gone out);
/// `None` means this seat is not released this tick.
///
/// Only a **long** wait is released, and only a wait that would be resumed:
/// - the project asks for it (`on_wait = release`), and only a lane — a
///   captain or a reviewer keeps its agent;
/// - a quota or unreadable park (a wait that pauses the round, §5.4) or a
///   policy park's deadline, ending at least [`RELEASE_MIN_WAIT_S`] from now.
///   The 5-min provider-error wait is never released;
/// - the release happens once: the meta `wait_released` marks it;
/// - the CLI can resume the conversation afterwards (`resume_args` and a
///   session Herdr reports), else the agent waits out the quota in its pane;
/// - the seat's binding is `Active` and its pane holds an agent — a pane that
///   is already a shell has nothing to release.
/// The exit goes the way the stop phase of a restart sends it (T-134): the
/// agent's name first, the command as a prompt or the profile's keys.
#[allow(clippy::too_many_arguments)]
pub(crate) fn release_for_wait(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    config: &ProjectConfig,
    profile: &Profile,
    seat: &SeatRow,
    now: Ts,
    force: bool,
) -> GearResult<Option<String>> {
    // T-450: the working hours release every lane whatever `on_wait` says.
    if !force && config.models.on_wait != WaitPolicy::Release {
        return Ok(None);
    }
    if seat.role != SeatRole::Lane {
        return Ok(None);
    }
    let key = meta_key("wait_released", &seat.id);
    if meta_get(store, &key)?.is_some_and(|value| !value.trim().is_empty()) {
        return Ok(None);
    }
    // The wait the seat is in: a quota or unreadable park carries a wait that
    // pauses the round; a policy park only has its deadline.
    let until = match load_wait(store, &seat.id)? {
        Some(wait) if wait.pause => wait.until,
        _ => match parked_until(store, &seat.id)? {
            Some(until) => until,
            None => return Ok(None),
        },
    };
    if until < now + RELEASE_MIN_WAIT_S {
        return Ok(None);
    }
    // A CLI whose session cannot be resumed keeps its agent: after the exit
    // the seat could not come back to the same conversation.
    if profile.resume_args.is_none() || !profile.session_ref {
        return Ok(Some(format!(
            "{}: kept through the wait: its CLI cannot resume a session",
            seat.id
        )));
    }
    let Some(binding) = store.read(|tx| tx.get_binding(&seat.id, seat.generation))? else {
        return Ok(None);
    };
    if binding.status != BindingStatus::Active {
        return Ok(None);
    }
    let pane = binding.pane_id.clone();
    if !matches!(herdr.agent_get(&pane), Ok(Some(_))) {
        return Ok(None);
    }
    // The exit goes the way the stop phase of a restart sends it (T-134): the
    // agent's name first, the command as a prompt or the profile's keys.
    crate::names::ensure(herdr, &pane, &crate::names::desired(seat));
    let sent = match &profile.exit_command {
        Some(command) => match crate::names::live_name(herdr, &pane) {
            Some(name) => herdr.prompt(&name, command),
            // Without a name there is no agent to address: the command is
            // withheld rather than typed into a shell.
            None => Outcome::Refused {
                code: "agent_not_ready".into(),
                message: format!("the agent of pane {pane} has no name"),
            },
        },
        None => herdr.send_keys(&pane, &profile.exit_keys),
    };
    match sent {
        // The exit went out (or may have gone out): it is never sent twice.
        Outcome::Ok(()) | Outcome::Uncertain { .. } => {}
        // Nothing was executed: the next tick tries again.
        Outcome::Refused { .. } => return Ok(None),
    }
    meta_set(store, &key, "1")?;
    let line = format!(
        "{}: released for the wait until {until} ({}): its agent exited; its pane, worktree and session are kept",
        seat.id,
        if force { "working hours end" } else { "on_wait = release" }
    );
    tell_captain(store, project, seat, &line, now)?;
    Ok(Some(line))
}

/// The seats of `project` whose screen shows a provider limit right now: the
/// consecutive-quota counter is above zero. Read outside any transaction (the
/// tick counts first, then confirms).
pub fn seats_on_quota(store: &mut Store, project: &str) -> GearResult<Vec<String>> {
    let seats: Vec<String> = store.read(|tx| {
        Ok(tx
            .seats_of_project(project)?
            .into_iter()
            .map(|seat| seat.id)
            .collect())
    })?;
    let mut on_quota = Vec::new();
    for seat in seats {
        let ticks = meta_get(store, &meta_key("quota", &seat))?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        if ticks > 0 {
            on_quota.push(seat);
        }
    }
    Ok(on_quota)
}

/// Count this tick's quota observation: `previous + 1` while the screen shows
/// a quota flag, back to 0 when it does not.
///
/// The time the episode began is kept too: a **new** signal reads the source
/// at once (F1), and the reading of that episode is the one that decides.
fn count_quota_tick(store: &mut Store, seat: &str, quota: bool, now: Ts) -> GearResult<(i64, Ts)> {
    let key = meta_key("quota", seat);
    let previous = meta_get(store, &key)?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let ticks = if quota { previous + 1 } else { 0 };
    meta_set(store, &key, &ticks.to_string())?;
    Ok((
        ticks,
        signal_start(store, seat, "quota_signal", quota, previous == 0, now)?,
    ))
}

/// The time the signal episode of a seat began: `now` when the signal is new,
/// the time it began when it continues, none when there is no signal.
fn signal_start(
    store: &mut Store,
    seat: &str,
    episode: &str,
    signal: bool,
    fresh: bool,
    now: Ts,
) -> GearResult<Ts> {
    let key = meta_key(episode, seat);
    if !signal {
        meta_set(store, &key, "0")?;
        return Ok(0);
    }
    if fresh {
        // A new signal: the episode begins now, so the source is read at once.
        meta_set(store, &key, &now.to_string())?;
        return Ok(now);
    }
    if let Some(start) = meta_get(store, &key)?
        .and_then(|value| value.trim().parse::<Ts>().ok())
        .filter(|start| *start > 0)
    {
        return Ok(start);
    }
    // A restart or upgrade can preserve the counter without its new episode
    // key. Seed it now rather than treating zero as an ancient fresh reading.
    meta_set(store, &key, &now.to_string())?;
    Ok(now)
}

/// A key of the engine's own state in the store's meta.
pub(crate) fn meta_key(what: &str, seat: &str) -> String {
    format!("engine.{what}.{seat}")
}

/// The key of a seat's quota counter (how many ticks it showed a provider
/// limit in a row).
pub(crate) fn quota_meta_key(seat: &str) -> String {
    meta_key("quota", seat)
}

/// Read one key of the engine's own state.
pub(crate) fn meta_get(store: &mut Store, key: &str) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(key))
}

/// Write one key of the engine's own state.
pub(crate) fn meta_set(store: &mut Store, key: &str, value: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(key, value))
}

/// Tell the captain about a switch or a park: one information line.
pub(crate) fn tell_captain(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    line: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} has no captain"),
            )
        })?;
        // The seat's current generation: a restart on another CLI (T-35) gave
        // the seat a new one.
        let generation = tx
            .get_seat(&seat.id)?
            .map(|row| row.generation)
            .unwrap_or(seat.generation);
        let seq = next_thread_seq(tx, project, &captain.id, now, &seat.name)?;
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread: format!("{}:{seq}", seat.name),
            task: None,
            attempt: None,
            sender: seat.id.clone(),
            sender_gen: generation,
            recipient: captain.id,
            kind: MsgKind::Progress,
            urgent: false,
            body: line.to_string(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Info,
            created: now,
        })?;
        Ok(())
    })
}

/// An urgent action item for the captain: it rings the doorbell.
///
/// A failed model switch stops the lane (T-60): the seat is parked in
/// `NeedsCheck` and waits for a human, so the captain has to hear about it —
/// an info line would leave the lane silent.
pub(crate) fn alert_captain(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    line: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| insert_alert(tx, project, seat, line, now))
}

fn insert_alert(
    tx: &gear_store::Tx<'_>,
    project: &str,
    seat: &SeatRow,
    line: &str,
    now: Ts,
) -> GearResult<()> {
    let captain = tx.captain_of(project)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("project {project} has no captain"),
        )
    })?;
    // The seat's current generation: a restart on another CLI (T-35) gave
    // the seat a new one.
    let generation = tx
        .get_seat(&seat.id)?
        .map(|row| row.generation)
        .unwrap_or(seat.generation);
    let seq = next_thread_seq(tx, project, &captain.id, now, &seat.name)?;
    // The item names the seat and the reason (T-60): the captain has to
    // know which lane stopped and why.
    let prefix = format!("{}: ", seat.id);
    let body = if line.starts_with(&prefix) {
        line.to_string()
    } else {
        format!("{prefix}{line}")
    };
    tx.insert_message(&NewMessage {
        meta: None,
        project: project.to_string(),
        thread: format!("{}:{seq}", seat.name),
        task: None,
        attempt: None,
        sender: seat.id.clone(),
        sender_gen: generation,
        recipient: captain.id,
        kind: MsgKind::System,
        urgent: true,
        body: format!(
            "{body} — the lane is stopped: switch it to another model by hand, or let it wait"
        ),
        reply_to: None,
        needs_ack: false,
        state: MsgState::Open,
        created: now,
    })?;
    Ok(())
}

/// The next free thread number for `prefix` (the same rule as `gear send`).
fn next_thread_seq(
    tx: &gear_store::Tx<'_>,
    project: &str,
    recipient: &str,
    now: Ts,
    prefix: &str,
) -> GearResult<u32> {
    let mut seq = tx
        .actionable_for(project, recipient, now)?
        .iter()
        .filter_map(|message| {
            message
                .thread
                .strip_prefix(prefix)?
                .strip_prefix(':')?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    loop {
        seq += 1;
        if seq > 10_000 {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("too many threads named {prefix}:* in project {project}"),
            ));
        }
        if tx
            .thread_messages(project, &format!("{prefix}:{seq}"))?
            .is_empty()
        {
            return Ok(seq);
        }
    }
}

// ------------------------------------------- the sources and the reading (§9.1)

/// How often a source is read again (§9.1): every 5 min.
const READ_EVERY_S: i64 = 300;
/// A reading older than this is stale (§9.1): 15 min.
const STALE_AFTER_S: i64 = 900;
/// How long a seat waits after a provider error (§9.2): 5 min.
const WAIT_AFTER_ERROR_S: i64 = 300;

/// A source of §9.1 with the reader the engine uses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    /// The provider part of the model ids this source is about.
    provider: String,
    /// The reader that knows the provider's API.
    reader: QuotaReader,
    /// The file the key is in.
    key_file: String,
    /// The dotted path of the key inside that file.
    key_path: String,
}

/// The sources of the project config (§9.1): `[[quota.sources]]`.
///
/// A source whose reader the engine does not know is skipped — the config
/// names it as a finding, so the captain sees it.
fn sources_of(config: &ProjectConfig) -> Vec<Source> {
    config
        .quota
        .sources
        .iter()
        .filter_map(|source| {
            Some(Source {
                provider: source.provider.trim().to_string(),
                reader: QuotaReader::from_name(&source.reader)?,
                key_file: source.key_file.clone(),
                key_path: source.key_path.clone(),
            })
        })
        .collect()
}

/// The key of a source: the file it is in, then the dotted path in it (§9.1).
fn key_of(source: &Source) -> Result<String, String> {
    let path = expand_home(&source.key_file);
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut cursor = &value;
    for part in source.key_path.split('.').filter(|part| !part.is_empty()) {
        cursor = cursor
            .get(part)
            .ok_or_else(|| format!("{}: {} is not in the file", path.display(), source.key_path))?;
    }
    cursor
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{}: the key is not text", path.display()))
}

/// A path that starts with `~` is in the home folder of the user.
fn expand_home(path: &str) -> std::path::PathBuf {
    let path = path.trim();
    let Some(home) = std::env::var_os("HOME") else {
        return std::path::PathBuf::from(path);
    };
    match path.strip_prefix('~') {
        Some(rest) => {
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            let mut home = std::path::PathBuf::from(home);
            if !rest.is_empty() {
                home.push(rest);
            }
            home
        }
        None => std::path::PathBuf::from(path),
    }
}

/// The quota readers as the engine uses them (§9.1).
///
/// The live one calls `gear_usage::read_quota`; a test uses a fake one, so
/// every branch of §9.2 is shown without a provider.
pub trait QuotaApi {
    /// The quota of a provider now, or the reason it cannot be read.
    fn read(
        &self,
        reader: QuotaReader,
        key: &str,
        codex_session: Option<&str>,
        now: Ts,
    ) -> Result<Reading, String>;
}

/// The live readers (§9.1): the API of the provider.
pub struct LiveQuota;

impl QuotaApi for LiveQuota {
    fn read(
        &self,
        reader: QuotaReader,
        key: &str,
        codex_session: Option<&str>,
        now: Ts,
    ) -> Result<Reading, String> {
        gear_usage::read_quota(reader, key, codex_session, now)
    }
}

/// A reading of a provider as the store keeps it (§9.1): the meta
/// `quota.reading.<provider>`, JSON with the time it was read.
#[derive(Debug, Clone, PartialEq)]
struct StoredReading {
    provider: String,
    read_at: Ts,
    windows: Vec<StoredWindow>,
    /// The reading is older than 15 min (§9.1).
    stale: bool,
}

/// One window of a stored reading (§9.1).
#[derive(Debug, Clone, PartialEq)]
struct StoredWindow {
    name: String,
    used_percent: f64,
    resets_at: Option<Ts>,
}

/// The meta key of a provider's last reading (§9.1).
fn reading_meta_key(provider: &str) -> String {
    format!("quota.reading.{provider}")
}

/// The meta key of a provider's last **attempt** (§9.1): the last read, be
/// it a success or a failure. The next periodic read is due five minutes
/// after it (F2).
fn attempt_meta_key(provider: &str) -> String {
    format!("quota.attempt.{provider}")
}

/// When the source was last read, be it a success or a failure (F2).
fn load_attempt(store: &mut Store, provider: &str) -> GearResult<Option<Ts>> {
    Ok(meta_get(store, &attempt_meta_key(provider))?
        .and_then(|value| value.trim().parse::<Ts>().ok()))
}

/// The session of the project's codex reviewer: the codex reader reads the
/// rollout of that session (§9.1, F4).
fn codex_session(store: &mut Store, project: &str) -> GearResult<Option<String>> {
    store.read(|tx| {
        let mut seats = tx.seats_of_project(project)?;
        seats.retain(|seat| {
            seat.role == SeatRole::Temp
                && matches!(seat.state, SeatState::Active | SeatState::Standby)
        });
        seats.sort_by_key(|seat| (seat.state != SeatState::Active, seat.id.clone()));
        for seat in seats {
            let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                continue;
            };
            if binding.status != gear_store::BindingStatus::Active || binding.kind != "codex" {
                continue;
            }
            if let Some(session) = binding.session_ref.filter(|s| !s.trim().is_empty()) {
                return Ok(Some(session));
            }
        }
        Ok(None)
    })
}

/// The reading of a provider as it is stored (None when there is none).
fn load_reading(store: &mut Store, provider: &str) -> GearResult<Option<StoredReading>> {
    let Some(raw) = meta_get(store, &reading_meta_key(provider))? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    let windows = value
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|window| {
                    Some(StoredWindow {
                        name: window.get("name")?.as_str()?.to_string(),
                        used_percent: window.get("used_percent")?.as_f64()?,
                        resets_at: window.get("resets_at").and_then(serde_json::Value::as_i64),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(StoredReading {
        provider: value
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(provider)
            .to_string(),
        read_at: value
            .get("read_at")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        windows,
        stale: value
            .get("stale")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }))
}

/// Write a reading of a provider (§9.1): the shape the board reads.
fn store_reading(store: &mut Store, reading: &StoredReading) -> GearResult<()> {
    let windows: Vec<serde_json::Value> = reading
        .windows
        .iter()
        .map(|window| {
            serde_json::json!({
                "name": window.name,
                "used_percent": window.used_percent,
                "resets_at": window.resets_at,
            })
        })
        .collect();
    let value = serde_json::json!({
        "provider": reading.provider,
        "read_at": reading.read_at,
        "windows": windows,
        "stale": reading.stale,
    });
    meta_set(
        store,
        &reading_meta_key(&reading.provider),
        &value.to_string(),
    )
}

/// Read the quota of a source (§9.1): the key out of its file, then the
/// reader of the provider.
///
/// A reading taken at or after `since` is fresh enough and is kept: the
/// signal that asks for it is answered by it (F1). `Err` is the reason the
/// source cannot be read.
///
/// The attempt is timed whether it succeeds or not (F2): a provider that
/// does not answer is not asked again before the next window.
fn read_source(
    store: &mut Store,
    api: &dyn QuotaApi,
    source: &Source,
    now: Ts,
    since: Ts,
    codex_session: Option<&str>,
) -> Result<Reading, String> {
    if let Some(stored) =
        load_reading(store, &source.provider).map_err(|error| error.to_string())?
    {
        if !stored.windows.is_empty() && stored.read_at >= since {
            return Ok(Reading {
                provider: stored.provider,
                windows: stored
                    .windows
                    .into_iter()
                    .map(|window| gear_usage::Window {
                        name: window.name,
                        used_percent: window.used_percent,
                        resets_at: window.resets_at,
                    })
                    .collect(),
                read_at: stored.read_at,
            });
        }
    }
    // The attempt counts, success or failure (F2).
    store
        .write(|tx| tx.set_meta(&attempt_meta_key(&source.provider), &now.to_string()))
        .map_err(|error| error.to_string())?;
    // The codex reader needs no key: it reads the rollout of the reviewer's
    // session (§9.1, F4). The other readers need the key of the provider.
    let (key, session) = match source.reader {
        QuotaReader::Codex => (String::new(), codex_session.map(str::to_string)),
        _ => (key_of(source)?, None),
    };
    let reading = api.read(source.reader, &key, session.as_deref(), now)?;
    store_reading(
        store,
        &StoredReading {
            provider: source.provider.clone(),
            read_at: reading.read_at,
            windows: reading
                .windows
                .iter()
                .map(|window| StoredWindow {
                    name: window.name.clone(),
                    used_percent: window.used_percent,
                    resets_at: window.resets_at,
                })
                .collect(),
            stale: false,
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(reading)
}

/// Read every source that is due (every 5 min after its last attempt, §9.1)
/// and mark a reading older than 15 min as stale (§9.1).
fn refresh_readings(
    store: &mut Store,
    api: &dyn QuotaApi,
    sources: &[Source],
    now: Ts,
    codex_session: Option<&str>,
) -> GearResult<Vec<String>> {
    let mut events = Vec::new();
    for source in sources {
        // The next read is due five minutes after the last attempt. A
        // failure is an attempt too (F2): an outage does not make the engine
        // call the provider on every tick.
        let attempt = load_attempt(store, &source.provider)?;
        let due = attempt.map_or(true, |at| now - at >= READ_EVERY_S);
        if due {
            let since = attempt.map(|at| at + READ_EVERY_S).unwrap_or(now);
            match read_source(store, api, source, now, since, codex_session) {
                Ok(_) => {}
                Err(reason) => events.push(format!(
                    "the quota of {} could not be read: {reason}",
                    source.provider
                )),
            }
        }
        // A reading that is still old after the refresh is stale (§9.1).
        if let Some(reading) = load_reading(store, &source.provider)? {
            if now - reading.read_at > STALE_AFTER_S && !reading.stale {
                let mut stale = reading.clone();
                stale.stale = true;
                store_reading(store, &stale)?;
                events.push(format!(
                    "the quota reading of {} is stale (older than 15 min)",
                    source.provider
                ));
            }
        }
    }
    Ok(events)
}

/// What the source said about the provider (§9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// A window is spent: the seat waits for its reset.
    Spent { window: String, until: Ts },
    /// Quota is left: a rate limit or an error, not a quota stop.
    Left,
}

/// What a reading means (§9.2): a window with nothing left in it is a quota
/// stop, anything else is not.
fn verdict_of(reading: &Reading, now: Ts, reset_h: u32) -> Verdict {
    let spent: Vec<&gear_usage::Window> = reading
        .windows
        .iter()
        .filter(|window| window.used_percent >= 100.0 - 0.001)
        .collect();
    if spent.is_empty() {
        return Verdict::Left;
    }
    // The latest deadline of the spent windows: the seat waits for all of
    // them (the captain's quota bar does the same, design/04-herdr.md).
    let until = spent
        .iter()
        .filter_map(|window| window.resets_at)
        .max()
        // A window without a reset time: today's window (`quota_reset_h`).
        .unwrap_or(now + i64::from(reset_h.max(1)) * 3600)
        // A minute after the reset, so the provider is ready (§9.2).
        + 60;
    let window = spent
        .iter()
        .map(|window| window.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    Verdict::Spent { window, until }
}

/// The provider part of a model id (`minimax-cn/MiniMax-M3.1-Flash` →
/// `minimax-cn`): the part before the first slash (§9.1). Providerless
/// model ids on the Codex CLI use the codex source.
fn provider_of(model: &str, cli: &str) -> Option<String> {
    let model = model.trim();
    let Some((provider, _)) = model.split_once('/') else {
        return (cli == "codex" && !model.is_empty()).then(|| "codex".to_string());
    };
    let provider = provider.trim();
    if provider.is_empty() {
        None
    } else {
        Some(provider.to_string())
    }
}

/// The source of the seat's provider (§9.1): None when the project has no
/// source for it.
fn source_of(
    sources: &[Source],
    chain: &[&ModelEntry],
    store: &mut Store,
    seat: &SeatRow,
) -> GearResult<Option<Source>> {
    if sources.is_empty() {
        return Ok(None);
    }
    let Some(model) = current_model(store, &seat.id, chain) else {
        return Ok(None);
    };
    let cli = store
        .read(|tx| tx.get_binding(&seat.id, seat.generation))?
        .map(|binding| binding.kind)
        .unwrap_or_default();
    let Some(provider) = provider_of(&model, &cli) else {
        return Ok(None);
    };
    Ok(sources
        .iter()
        .find(|source| source.provider == provider)
        .cloned())
}

// ------------------------------------------------------- the waits of §9.2

/// What a seat waits for (§9.2).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Wait {
    /// `quota` (a spent window), `error` (quota is left), `unreadable` or
    /// `hours` (T-450: the working hours are over).
    pub kind: String,
    /// When the wait ends.
    pub until: Ts,
    /// When it began: the round's clock pauses for this long (§5.1).
    pub started_at: Ts,
    /// The provider the seat waits for.
    pub provider: String,
    /// The window that is spent (a quota park).
    pub window: String,
    /// The task the seat continues.
    pub task: Option<String>,
    /// The wait opened a pause row (§5.4): a quota park does, a provider
    /// error does not — that is not quota wait time.
    pub pause: bool,
}

/// The key of a seat's wait in the engine's meta.
fn wait_meta_key(seat: &str) -> String {
    meta_key("quota_wait", seat)
}

/// The wait of a seat, if it has one.
pub(crate) fn load_wait(store: &mut Store, seat: &str) -> GearResult<Option<Wait>> {
    let Some(raw) = meta_get(store, &wait_meta_key(seat))? else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    Ok(Some(Wait {
        kind: value
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        until: value.get("until").and_then(|v| v.as_i64()).unwrap_or(0),
        started_at: value
            .get("started_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        provider: value
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        window: value
            .get("window")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        task: value
            .get("task")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        pause: value
            .get("pause")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }))
}

/// Write the wait of a seat (in the caller's transaction: a park and its
/// deadline are one fact, F3).
fn write_wait(tx: &Tx<'_>, seat: &str, wait: &Wait) -> GearResult<()> {
    let value = serde_json::json!({
        "kind": wait.kind,
        "until": wait.until,
        "started_at": wait.started_at,
        "provider": wait.provider,
        "window": wait.window,
        "task": wait.task,
        "pause": wait.pause,
    });
    tx.set_meta(&wait_meta_key(seat), &value.to_string())
}

/// Count this tick's provider error: `previous + 1` while the screen shows
/// an error, back to 0 when it does not.
/// Count this tick's provider error: `previous + 1` while the screen shows
/// an error, back to 0 when it does not. The start of the episode is kept,
/// as for the quota marker (F1).
fn count_error_tick(store: &mut Store, seat: &str, error: bool, now: Ts) -> GearResult<(i64, Ts)> {
    let key = meta_key("quota_error_ticks", seat);
    let previous = meta_get(store, &key)?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let ticks = if error { previous + 1 } else { 0 };
    meta_set(store, &key, &ticks.to_string())?;
    Ok((
        ticks,
        signal_start(store, seat, "quota_error_signal", error, previous == 0, now)?,
    ))
}

/// Count the provider errors of a seat in a row (§9.2): the second one is a
/// captain item.
fn count_error_episode(store: &mut Store, seat: &str) -> GearResult<i64> {
    let key = meta_key("quota_errors", seat);
    let previous = meta_get(store, &key)?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let episodes = previous + 1;
    meta_set(store, &key, &episodes.to_string())?;
    Ok(episodes)
}

/// The provider errors of a seat are over (a quota stop or a good reading).
fn reset_error_episodes(store: &mut Store, seat: &str) -> GearResult<()> {
    meta_set(store, &meta_key("quota_errors", seat), "0")
}

/// Park a seat, open its pause row and write its deadline — **one**
/// transaction (F3): a restart between two writes left a parked lane without
/// a deadline, and it never woke up.
///
/// False when the seat's state does not allow the park (a retiring lane, for
/// instance).
pub(crate) fn park_seat(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    reason: &str,
    pause_reason: Option<&str>,
    wait: &Wait,
    now: Ts,
) -> GearResult<bool> {
    let parked = store.write(|tx| {
        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        if !SeatState::can_transition(row.role, row.state, SeatState::Parked) {
            return Ok(false);
        }
        tx.set_seat_state(&seat.id, SeatState::Parked, Some(reason), now)?;
        if let Some(pause_reason) = pause_reason {
            tx.open_pause(project, &seat.id, "quota", pause_reason, now)?;
        }
        // The prompt the provider killed is retired: the policy handles this
        // seat, and the Continue at the reset is its delivery again.
        retire_unconfirmed_deliveries(tx, project, seat, now)?;
        // The deadline of the wait, with the park (F3).
        write_wait(tx, &seat.id, wait)?;
        Ok(true)
    })?;
    Ok(parked)
}

/// A quota stop: the seat waits for the provider's reset (§9.2).
///
/// The seat is parked, a pause row of kind `quota` is opened (§5.4) and the
/// round's clock pauses with it (§5.1). At the reset the seat continues in
/// the same session.
fn park_for_quota(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    provider: &str,
    window: &str,
    until: Ts,
    now: Ts,
) -> GearResult<String> {
    let task = open_attempt(store, &seat.id)?.map(|(task, _)| task);
    let reason = format!("quota: {provider} {window}; resume at {until}");
    let wait = Wait {
        kind: "quota".to_string(),
        until,
        started_at: now,
        provider: provider.to_string(),
        window: window.to_string(),
        task,
        pause: true,
    };
    let parked = park_seat(
        store,
        project,
        seat,
        &reason,
        Some(&format!("{provider} {window}")),
        &wait,
        now,
    )?;
    if !parked {
        return Ok(format!(
            "{}: the quota of {provider} is spent, but the seat cannot park",
            seat.id
        ));
    }
    reset_error_episodes(store, &seat.id)?;
    Ok(format!(
        "{}: parked until {until} (quota: {provider} {window})",
        seat.id
    ))
}

/// Quota is left: not a quota stop but a rate limit or an error (§9.2).
///
/// The seat waits 5 min and continues; the second time in a row it becomes a
/// captain item.
fn wait_for_provider(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    provider: &str,
    second: bool,
    now: Ts,
) -> GearResult<String> {
    let until = now + WAIT_AFTER_ERROR_S;
    let task = open_attempt(store, &seat.id)?.map(|(task, _)| task);
    let reason = format!("provider error, not quota; try again at {until}");
    let wait = Wait {
        kind: "error".to_string(),
        until,
        started_at: now,
        provider: provider.to_string(),
        window: String::new(),
        task,
        pause: false,
    };
    let parked = park_seat(store, project, seat, &reason, None, &wait, now)?;
    if !parked {
        return Ok(format!(
            "{}: the provider of {provider} errors, but the seat cannot park",
            seat.id
        ));
    }
    if second {
        // The second time in a row: the captain hears about it (§9.2).
        alert_provider_error(store, project, seat, now)?;
    }
    Ok(format!(
        "{}: waits until {until} (provider error, not quota)",
        seat.id
    ))
}

/// The source cannot be read: today's park, `quota_reset_h` hours (§9.2).
fn park_unreadable(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    provider: &str,
    reset_h: u32,
    now: Ts,
) -> GearResult<String> {
    let until = now + i64::from(reset_h.max(1)) * 3600;
    let task = open_attempt(store, &seat.id)?.map(|(task, _)| task);
    let reason = format!("quota: the source of {provider} cannot be read; resume at {until}");
    let wait = Wait {
        kind: "unreadable".to_string(),
        until,
        started_at: now,
        provider: provider.to_string(),
        window: String::new(),
        task,
        pause: true,
    };
    let parked = park_seat(
        store,
        project,
        seat,
        &reason,
        Some(&format!("{provider} unreadable")),
        &wait,
        now,
    )?;
    if !parked {
        return Ok(format!(
            "{}: the source of {provider} cannot be read, but the seat cannot park",
            seat.id
        ));
    }
    reset_error_episodes(store, &seat.id)?;
    Ok(format!(
        "{}: parked until {until} (the source of {provider} cannot be read)",
        seat.id
    ))
}

/// End the pause row of a source wait and add the round's pause for it, in the
/// caller's transaction (§5.4, §5.1). The in-place wake ([`wake_from_source`])
/// and the comeback of a released lane ([`bring_back`]) book a wait this way.
fn end_wait_pause(
    tx: &Tx<'_>,
    project: &str,
    seat: &SeatRow,
    wait: &Wait,
    now: Ts,
) -> GearResult<()> {
    if !wait.pause {
        return Ok(());
    }
    let open_pause = tx
        .pauses_of_project(project)?
        .into_iter()
        .any(|pause| pause.seat == seat.id && pause.kind == "quota" && pause.end_at.is_none());
    if open_pause {
        tx.close_pause(project, &seat.id, "quota", now)?;
    }
    if let Some(round) = tx.open_round_of_seat(&seat.id)? {
        let key = RoundKey {
            project: project.to_string(),
            task: round.task.clone(),
            attempt: round.attempt,
            role: round.role.clone(),
        };
        let secs = (now - wait.started_at).max(0);
        tx.add_round_pause(&key, secs)?;
    }
    Ok(())
}

/// A wait that ended: the seat continues its task in the same session (§9.2,
/// §3.3 S4).
///
/// The whole wake is **one** transaction (F3): the pause row ends, the round's
/// clock takes its pause, the seat works again, the Continue is delivered and
/// the wait is cleared — all of it or none of it. A restart between two of
/// these writes left a lane awake without its Continue, or paused twice.
fn wake_from_source(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    now: Ts,
) -> GearResult<Option<String>> {
    let Some(wait) = load_wait(store, &seat.id)? else {
        return Ok(None);
    };
    if wait.until > now {
        return Ok(None);
    }
    // The Continue: the same session, the same task (§9.2). T-450: a lane that
    // waited out the working hours is told its hours are back, not its quota.
    let open = open_attempt(store, &seat.id)?;
    let (back, reason) = if wait.kind == "hours" {
        (
            "the working hours are back",
            "hours: the working hours start again",
        )
    } else {
        ("your quota is back", "quota: the provider is back")
    };
    let text = match wait.task.as_deref() {
        Some(task) => format!("Continue {task}: {back}."),
        None => format!("Continue: {back}."),
    };
    store.write(|tx| {
        // The pause row ends (§5.4) and the round's clock pauses for the
        // whole wait (§5.1): the round clocks take their pauses from these
        // rows.
        end_wait_pause(tx, project, seat, &wait, now)?;
        // The seat works again.
        let row = tx
            .get_seat(&seat.id)?
            .ok_or_else(|| GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id)))?;
        if row.state == SeatState::Parked {
            gear_model::state::require_transition(
                SeatState::can_transition(row.role, row.state, SeatState::Active),
                "seat",
                row.state.as_str(),
                SeatState::Active.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Active, Some(reason), now)?;
        }
        // The Continue: the delivery the provider killed is retired, and the
        // Continue is the seat's delivery again.
        retire_unconfirmed_deliveries(tx, project, seat, now)?;
        tx.insert_op(&NewOp {
            project: project.to_string(),
            seat: seat.id.clone(),
            generation: seat.generation,
            kind: OpKind::Deliver,
            task: open.as_ref().map(|(task, _)| task.clone()),
            attempt: open.as_ref().map(|(_, attempt)| *attempt),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({ "text": text }),
            created: now,
        })?;
        // The wait is over: the counters of the next signal start again.
        tx.set_meta(&wait_meta_key(&seat.id), "")?;
        tx.set_meta(&quota_meta_key(&seat.id), "0")?;
        tx.set_meta(&meta_key("quota_error_ticks", &seat.id), "0")?;
        Ok(())
    })?;
    Ok(Some(format!(
        "{}: continues after the wait ({})",
        seat.id, wait.kind
    )))
}

/// T-327: a lane released for a long wait comes back at the reset in the same
/// pane and worktree, with its session resumed, and is told to continue.
///
/// The release left the pane a **shell**, so neither wake path works: a
/// Continue typed into the pane would land in the shell
/// ([`wake_from_source`]), and there is no live agent to change the model of
/// ([`change_model`]). [`crate::restart`] starts the same CLI again in the
/// same pane with the round's session and delivers the Continue once the
/// agent is up.
///
/// The wait's books are cleared in one write first: the pause row ends, the
/// round's clock takes its pause, and every counter of the wait starts again.
/// A restart that is still pending goes on through the `crate::restart::pending`
/// check at the top of the loop on later ticks.
pub(crate) fn bring_back(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    project: &str,
    config: &ProjectConfig,
    seat: &SeatRow,
    now: Ts,
) -> GearResult<Option<String>> {
    let wait = load_wait(store, &seat.id)?;
    store.write(|tx| {
        if let Some(wait) = &wait {
            end_wait_pause(tx, project, seat, wait, now)?;
        }
        // The wait is over: the counters of the next signal start again.
        tx.set_meta(&wait_meta_key(&seat.id), "")?;
        tx.set_meta(&quota_meta_key(&seat.id), "0")?;
        tx.set_meta(&meta_key("quota_error_ticks", &seat.id), "0")?;
        tx.set_meta(&meta_key("resume", &seat.id), "")?;
        tx.set_meta(&meta_key("wait_released", &seat.id), "")?;
        Ok(())
    })?;
    // The target entry is the first of the lane's chain, as the restart of a
    // missing seat builds it (T-226): the entry's own CLI, else the lane's
    // kind.
    let chain = crate::model::role_chain(config, &seat.name);
    let Some(entry) = chain.first().copied() else {
        flag_and_alert(
            store,
            project,
            seat,
            "quota: the lane's chain is empty; no model to bring it back on",
            now,
        )?;
        return Ok(None);
    };
    let kind = config
        .lanes
        .iter()
        .find(|lane| lane.name == seat.name)
        .map(|lane| lane.kind.clone())
        .unwrap_or_default();
    let target = gear_app::config::ModelEntry::Entry(gear_app::config::ModelSpec {
        model: entry.model().to_string(),
        cli: Some(entry.cli().map(str::to_owned).unwrap_or(kind)),
        args: entry.args().to_vec(),
        context_window_k: None,
    });
    crate::restart::advance(
        store,
        herdr,
        cfg,
        profiles,
        project,
        seat,
        Some(&target),
        now,
    )
}

/// The captain item of a parked lane (§9.2): the lane waits for the provider.
fn alert_park(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    line: &str,
    now: Ts,
) -> GearResult<()> {
    alert_body(
        store,
        project,
        seat,
        &format!("{line} — the lane waits for the provider"),
        now,
    )
}

/// The captain item of §9.2: the provider errors again — the lane is not out
/// of quota.
fn alert_provider_error(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    now: Ts,
) -> GearResult<()> {
    alert_body(
        store,
        project,
        seat,
        &format!("{}: provider error, not quota", seat.id),
        now,
    )
}

/// One urgent item for the captain, with the body given.
fn alert_body(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    body: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!("project {project} has no captain"),
            )
        })?;
        let generation = tx
            .get_seat(&seat.id)?
            .map(|row| row.generation)
            .unwrap_or(seat.generation);
        let seq = next_thread_seq(tx, project, &captain.id, now, &seat.name)?;
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread: format!("{}:{seq}", seat.name),
            task: seat.task_id.clone(),
            attempt: None,
            sender: seat.id.clone(),
            sender_gen: generation,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: true,
            body: body.to_string(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::Presence;
    use gear_herdr::fake::{Call, FakeHerdr, Inject};
    use gear_model::{OpState, TaskState};
    use gear_store::{testing, ContractRow, ProjectRow, TaskRow};
    use std::collections::BTreeSet;

    const T0: i64 = 1_800_000_000;
    const PANE: &str = "w1:p1";
    /// The message an opencode screen shows when the provider limit is hit
    /// (`Profile::quota_markers`).
    const QUOTA_SCREEN: &str = "  Error: Token Plan ... (2056)\n  Session failed";

    /// A project with one opencode lane on a two-model chain.
    fn config(chain: &str, policy: &str) -> String {
        config_full(chain, policy, "keep", 5)
    }

    /// The same project with `[models].on_wait` and `quota_reset_h` given
    /// (T-326).
    fn config_full(chain: &str, policy: &str, on_wait: &str, reset_h: u32) -> String {
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
pane = "w9:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[models]
lanes = [{chain}]
on_quota = {{ lanes = "{policy}" }}
on_wait = "{on_wait}"
quota_reset_h = {reset_h}
"#
        )
    }

    /// The same project with its lane on pi (T-327): pi's profile reports the
    /// session and resumes it (`--session <session>`), which is the CLI a
    /// released lane comes back on.
    fn config_pi(chain: &str, policy: &str, on_wait: &str) -> String {
        config_full(chain, policy, on_wait, 5)
            .replace("kind = \"opencode\"", "kind = \"pi\"")
            .replace("profile = \"opencode\"", "profile = \"pi\"")
    }

    /// A world with the lane's task T-1 started (so a continue can be
    /// delivered), its seat active and bound to `PANE`.
    fn world(config_toml: &str) -> Store {
        world_kind(config_toml, "opencode")
    }

    /// The same world with the lane's seat and binding on `kind` (T-327: pi,
    /// whose profile resumes the session a released lane comes back to).
    fn world_kind(config_toml: &str, kind: &str) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: config_toml.into(),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 0,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: T0,
                })?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, kind, T0)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w9:p1",
                    "claude",
                    T0,
                )?;
                tx.set_binding_session_ref("p:heat", 1, "ses_lane")?;
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
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    version: 1,
                    goal: "the goal".into(),
                    accept: "the accept".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
                Ok(())
            })
            .unwrap();
        store
    }

    fn cfg() -> EngineCfg {
        let mut cfg = EngineCfg {
            db_path: crate::testtmp::temp("gear-engine-tests")
                .join("gear.db")
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        for kind in ["opencode", "claude", "codex"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        cfg
    }

    /// The lane's row, for building this tick's observation.
    fn lane_seat(store: &mut Store) -> SeatRow {
        store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap()
    }

    /// The lane's observation this tick, with the screen flags given.
    fn observed(seat: &SeatRow, quota: bool) -> Vec<SeatObs> {
        let mut flags = BTreeSet::new();
        if quota {
            flags.insert(ScreenFlag::Quota);
        }
        vec![SeatObs {
            seat: seat.clone(),
            presence: Presence::Present,
            status: Some(gear_herdr::AgentStatus::Idle),
            flags,
            idle: true,
            screen: None,
            session: None,
        }]
    }

    /// One quota tick of the policy, with the lane's observation this tick.
    fn tick_quota(store: &mut Store, herdr: &mut FakeHerdr, quota: bool, now: Ts) -> QuotaReport {
        let cfg = cfg();
        tick_quota_with(store, herdr, &cfg, quota, now)
    }

    /// One quota tick of the policy with the profiles given (T-326).
    fn tick_quota_with(
        store: &mut Store,
        herdr: &mut FakeHerdr,
        cfg: &EngineCfg,
        quota: bool,
        now: Ts,
    ) -> QuotaReport {
        let seat = lane_seat(store);
        let obs = observed(&seat, quota);
        apply_quota_policy(store, herdr, cfg, &cfg.profiles, "p", &obs, now).unwrap()
    }

    /// The engine's profiles with the lane's CLI able to resume a session
    /// (T-326): the builtin opencode profile has the resume args but Herdr
    /// does not report its session, so the test profile reports it. With
    /// `resumable` false the resume args are gone instead: the CLI cannot
    /// come back to the conversation.
    fn cfg_resumable(resumable: bool) -> EngineCfg {
        let mut cfg = cfg();
        let mut profile = cfg.profiles.get("opencode").unwrap().clone();
        profile.session_ref = true;
        if !resumable {
            profile.resume_args = None;
        }
        cfg.profiles.insert("opencode".into(), profile);
        cfg
    }

    /// The engine's profiles for a pi lane (T-327), with the rules file the
    /// restart writes under the engine's own directory.
    fn cfg_pi(dir: &std::path::Path) -> EngineCfg {
        EngineCfg {
            db_path: dir.join("gear.db").to_string_lossy().into_owned(),
            profiles: gear_herdr::builtins(),
            ..Default::default()
        }
    }

    /// One quota tick of the policy, with the screen the agent shows this
    /// tick (T-309).
    fn tick_quota_screen(
        store: &mut Store,
        herdr: &mut FakeHerdr,
        quota: bool,
        screen: &str,
        now: Ts,
    ) -> QuotaReport {
        let seat = lane_seat(store);
        let mut obs = observed(&seat, quota);
        obs[0].screen = Some(screen.to_string());
        apply_quota_policy(store, herdr, &cfg(), &cfg().profiles, "p", &obs, now).unwrap()
    }

    /// A world with an opencode agent in PANE that shows the quota screen.
    fn herdr_with_quota() -> FakeHerdr {
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        herdr
    }

    fn exits(herdr: &FakeHerdr) -> Vec<Vec<String>> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::SendKeys { keys, .. } => Some(keys.clone()),
                _ => None,
            })
            .collect()
    }

    fn starts(herdr: &FakeHerdr) -> Vec<Vec<String>> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { args, .. } => Some(args.clone()),
                _ => None,
            })
            .collect()
    }

    /// The agent starts the engine asked for: (kind, pane, args).
    fn agent_starts(herdr: &FakeHerdr) -> Vec<(String, String, Vec<String>)> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart {
                    kind, pane, args, ..
                } => Some((kind.clone(), pane.clone(), args.clone())),
                _ => None,
            })
            .collect()
    }

    fn deliveries(store: &mut Store) -> Vec<String> {
        store
            .read(|tx| tx.ops_in_state("p", OpState::Intended))
            .unwrap()
            .into_iter()
            .filter(|op| op.kind == OpKind::Deliver)
            .filter_map(|op| {
                op.payload
                    .get("text")
                    .and_then(|text| text.as_str())
                    .map(str::to_string)
            })
            .collect()
    }

    #[test]
    fn an_exhausted_chain_parks_until_the_reset() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback"));
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);

        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        assert!(
            report.events[0].contains("chain exhausted"),
            "{:?}",
            report.events
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Parked);
        let until = T0 + 20 + 5 * 3600;
        assert!(seat
            .state_reason
            .unwrap_or_default()
            .contains(&until.to_string()));
        assert!(exits(&herdr).is_empty(), "nothing is restarted");
        // L-74: the captain gets an urgent action item that rings, not an info line.
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", T0 + 30))
            .unwrap();
        assert!(
            open.iter()
                .any(|m| m.urgent && m.body.contains("parked until")),
            "an urgent captain item: {open:?}"
        );

        // Before the reset: still parked, nothing happens.
        let report = tick_quota(&mut store, &mut herdr, false, until - 1);
        assert!(report.events.is_empty());
        assert!(exits(&herdr).is_empty());

        // At the reset: no `[models.switch]` labels in this config, so the
        // lane is flagged for a human — never closed and restarted (T-33, F4).
        let report = tick_quota(&mut store, &mut herdr, false, until);
        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        assert!(
            report.events[0].contains("models.switch"),
            "{:?}",
            report.events
        );
        assert!(exits(&herdr).is_empty(), "no exit keys: {:?}", herdr.calls);
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck);
    }

    #[test]
    fn the_wait_policy_parks_even_with_a_longer_chain() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "wait",
        ));
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);

        assert!(
            report.events[0].contains("policy wait"),
            "{:?}",
            report.events
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Parked);
        assert!(exits(&herdr).is_empty(), "waited, not restarted");
    }

    /// T-309: a lane parked without a quota source wakes at the reset its
    /// screen states, plus a minute — not after `quota_reset_h` hours.
    #[test]
    fn the_policy_parks_until_the_reset_the_screen_states() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "wait",
        ));
        let mut herdr = herdr_with_quota();
        let screen = "  Error: Token Plan ... (2056)\n  Usage limit reached. Try again in 2h 13m.";

        tick_quota_screen(&mut store, &mut herdr, true, screen, T0 + 10);
        let report = tick_quota_screen(&mut store, &mut herdr, true, screen, T0 + 20);

        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        let until = T0 + 20 + 7_980 + 60;
        assert!(
            report.events[0].contains("policy wait"),
            "{:?}",
            report.events
        );
        assert!(
            report.events[0].contains("reset read from the screen"),
            "{:?}",
            report.events
        );
        assert!(
            report.events[0].contains(&until.to_string()),
            "{:?}",
            report.events
        );
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked);
        assert!(seat
            .state_reason
            .unwrap_or_default()
            .contains(&until.to_string()));
        assert!(exits(&herdr).is_empty(), "waited, not restarted");
    }

    /// T-309: a screen without a reset keeps the configured hours.
    #[test]
    fn the_policy_parks_for_the_configured_hours_without_a_reset() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "wait",
        ));
        let mut herdr = herdr_with_quota();

        tick_quota_screen(&mut store, &mut herdr, true, QUOTA_SCREEN, T0 + 10);
        let report = tick_quota_screen(&mut store, &mut herdr, true, QUOTA_SCREEN, T0 + 20);

        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        let until = T0 + 20 + 5 * 3600;
        assert!(
            report.events[0].contains("no reset on the screen"),
            "{:?}",
            report.events
        );
        assert!(
            report.events[0].contains(&until.to_string()),
            "{:?}",
            report.events
        );
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked);
        assert!(seat
            .state_reason
            .unwrap_or_default()
            .contains(&until.to_string()));
        assert!(exits(&herdr).is_empty(), "waited, not restarted");
    }

    #[test]
    fn a_quota_turn_does_not_flag_the_seat() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();
        // A delivery the agent cannot answer while it is on quota.
        store
            .write(|tx| {
                let op = tx
                    .insert_op(&NewOp {
                        project: "p".into(),
                        seat: "p:heat".into(),
                        generation: 1,
                        kind: OpKind::Doorbell,
                        task: None,
                        attempt: None,
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({ "text": "x", "covers": ["M-404"] }),
                        created: T0,
                    })
                    .unwrap();
                tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
                tx.set_op_state("p", &op.id, OpState::Submitted, None, T0)?;
                Ok(())
            })
            .unwrap();

        // One quota screen: the counter is set, the seat is on quota…
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        // … so the delivery's budget is not spent and the seat is not flagged.
        for tick in 0..5 {
            let (_, flagged) =
                crate::ops::confirm_pending(&mut store, "p", 1, T0 + 20 + tick).unwrap();
            assert!(flagged.is_empty(), "a quota turn is not 'no progress'");
        }
        let op = store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap();
        assert_eq!(op.check_budget, 0);
    }

    /// T-33: the quota fallback switches the model in place — no exit keys, no
    /// agent start — and delivers one Continue once the status bar confirms.
    /// The switch takes several ticks: one UI submission per tick.
    #[test]
    fn the_quota_fallback_switches_the_model_in_place() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        );
        text.push_str(
            "\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n",
        );
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        // The screens one switch produces: the toast cleared, the picker, the
        // bar.
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            (
                "/model".into(),
                "  Search\n  Recent\n  DeepSeek V4.1 Flash  OpenCode Go".into(),
            ),
            (
                "DeepSeek V4.1 Flash".into(),
                "Build auto · DeepSeek V4.1 Flash · high".into(),
            ),
        ];

        // Tick 1: one quota screen is not enough (`confirm_rounds` is 2).
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        assert!(texts(&herdr).is_empty(), "nothing typed on the first tick");
        // Tick 2: the limit is confirmed — the switch starts (esc this tick).
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert!(texts(&herdr).is_empty(), "only the escape so far");
        assert!(
            deliveries(&mut store).is_empty(),
            "no continue before the bar confirms"
        );

        // Ticks 3..6: /model, the filter, then the bar check.
        for tick in 0..4 {
            tick_quota(&mut store, &mut herdr, true, T0 + 30 + tick);
        }

        // In place: no exit keys (the profile's ctrl+c), no agent start.
        let exit_keys = gear_herdr::builtin("opencode").unwrap().exit_keys;
        let sent: Vec<Vec<String>> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::SendKeys { keys, .. } => Some(keys.clone()),
                _ => None,
            })
            .collect();
        assert!(
            sent.iter().flatten().all(|key| !exit_keys.contains(key)),
            "no exit keys: {sent:?}"
        );
        assert!(
            starts(&herdr).is_empty(),
            "no restart: {:?}",
            starts(&herdr)
        );
        assert!(
            texts(&herdr).iter().any(|t| t == "/model"),
            "{:?}",
            texts(&herdr)
        );
        assert!(
            texts(&herdr).iter().any(|t| t == "DeepSeek V4.1 Flash"),
            "{:?}",
            texts(&herdr)
        );
        // Exactly one Continue, on the new model.
        let delivered = deliveries(&mut store);
        assert_eq!(delivered.len(), 1, "{delivered:?}");
        assert_eq!(
            meta_get(&mut store, &meta_key("model", "p:heat"))
                .unwrap()
                .as_deref(),
            Some("deepseek/deepseek-flash")
        );
        // More quota ticks add no second Continue.
        tick_quota(&mut store, &mut herdr, true, T0 + 60);
        assert_eq!(deliveries(&mut store).len(), 1, "one continue only");
    }

    /// T-97: the delivery a lane never confirmed is retired by the switch (the
    /// model change types into the agent after it), the switch's Continue is
    /// its delivery again, and no stall item is raised — the quota policy is
    /// handling the seat, not a human.
    #[test]
    fn a_switch_retires_an_unconfirmed_delivery_without_a_stall_item() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        );
        text.push_str(
            "\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n",
        );
        let mut store = world(&text);
        // The prompt Herdr accepted and the lane never confirmed: the quota
        // killed the turn.
        let op = store
            .write(|tx| {
                let op = tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Deliver,
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "text": "New task T-1" }),
                    created: T0 + 5,
                })?;
                tx.set_op_state("p", &op.id, OpState::Executing, None, T0 + 5)?;
                tx.set_op_state("p", &op.id, OpState::Submitted, None, T0 + 6)?;
                Ok(op.id)
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            (
                "/model".into(),
                "  Search\n  Recent\n  DeepSeek V4.1 Flash  OpenCode Go".into(),
            ),
            (
                "DeepSeek V4.1 Flash".into(),
                "Build auto · DeepSeek V4.1 Flash · high".into(),
            ),
        ];

        // Two quota ticks start the switch; the rest confirm it.
        for tick in 0..6 {
            tick_quota(&mut store, &mut herdr, true, T0 + 10 + tick);
        }

        let retired = store.read(|tx| tx.get_op("p", &op)).unwrap().unwrap();
        assert_eq!(retired.state, OpState::Failed, "{retired:?}");
        let reason = retired.result.clone().unwrap_or_default();
        assert!(
            reason.contains("the model switched before the delivery was confirmed"),
            "{reason}"
        );
        // The switch's Continue is the delivery again, for the same attempt.
        let delivered = deliveries(&mut store);
        assert_eq!(delivered.len(), 1, "{delivered:?}");
        assert!(delivered[0].contains("Continue T-1"), "{delivered:?}");
        // No stall item: the policy handled the seat.
        let items = store
            .read(|tx| tx.thread_messages("p", "stall:p:heat"))
            .unwrap();
        assert!(items.is_empty(), "no stall item: {items:?}");
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
    }

    /// T-35: the next model of the chain runs on **another CLI** — the agent
    /// is closed, that CLI starts in the same pane and worktree with the
    /// arguments of its chain entry, and the lane gets one Continue.
    #[test]
    fn a_quota_stop_restarts_the_lane_on_another_cli() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=medium"] }"#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();

        // Tick 1: one quota screen is not enough (`confirm_rounds` is 2).
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        // Tick 2: the limit is confirmed — the restart begins: the opencode
        // agent is closed, and nothing else happens yet.
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert_eq!(
            exits(&herdr),
            vec![vec!["ctrl+c".to_string()]],
            "the agent is closed once: {:?}",
            herdr.calls
        );
        assert!(agent_starts(&herdr).is_empty(), "nothing started yet");
        assert!(
            deliveries(&mut store).is_empty(),
            "no continue before the agent is up"
        );

        // The agent leaves: the pane is a shell again.
        herdr.exit_agent(PANE);

        // A tick while the agent is still there waits — the exit keys are
        // never sent twice (a second ctrl+c would land in the shell).
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 25);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert_eq!(
            exits(&herdr),
            vec![vec!["ctrl+c".to_string()]],
            "the exit keys went once: {:?}",
            herdr.calls
        );
        herdr.exit_agent(PANE);

        // Tick 3: codex starts in the same pane, and the lane continues.
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 30);
        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        let starts = agent_starts(&herdr);
        assert_eq!(starts.len(), 1, "{starts:?}");
        assert_eq!(starts[0].0, "codex", "the CLI of the chain entry");
        assert_eq!(starts[0].1, PANE, "the same pane");
        assert_eq!(
            starts[0].2,
            vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-c".to_string(),
                "check_for_update_on_startup=false".to_string(),
                // T-76: the folder the agent runs in (the lane's repo here:
                // the seat's worktree is not recorded in this fixture).
                "-c".to_string(),
                "projects={\"/repo\"={trust_level=\"trusted\"}}".to_string(),
                // T-457: a resume runs in the seat's own directory.
                "-c".to_string(),
                "tui.resume_cwd=\"current\"".to_string(),
                "-c".to_string(),
                gear_herdr::profile::render_rules(
                    &[gear_herdr::profile::CODEX_RULES_ARG.to_string()],
                    &std::path::Path::new(&cfg().db_path)
                        .parent()
                        .unwrap()
                        .join("rules/lanes/heat.md")
                        .to_string_lossy(),
                    ""
                )[0]
                .clone(),
                "-m".to_string(),
                "gpt-6.1-sol".to_string(),
                "-c".to_string(),
                "model_reasoning_effort=medium".to_string(),
            ],
            "the arguments of the chain entry"
        );
        // One Continue, and the model is recorded.
        let delivered = deliveries(&mut store);
        assert_eq!(delivered.len(), 1, "{delivered:?}");
        assert!(delivered[0].starts_with("Continue T-1"), "{delivered:?}");
        assert_eq!(
            meta_get(&mut store, &model_meta_key("p:heat"))
                .unwrap()
                .as_deref(),
            Some("gpt-6.1-sol")
        );
        // The seat is back: it runs the new CLI, in a new generation, in the
        // same pane.
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
        assert_eq!(seat.generation, 2);
        assert_eq!(seat.profile, "codex");
        let binding = store
            .read(|tx| tx.get_binding("p:heat", 2))
            .unwrap()
            .unwrap();
        assert_eq!(binding.kind, "codex");
        assert_eq!(binding.pane_id, PANE);
        let old = store
            .read(|tx| tx.get_binding("p:heat", 1))
            .unwrap()
            .unwrap();
        assert_eq!(old.status, gear_store::BindingStatus::Void);

        // More quota ticks add no second Continue.
        tick_quota(&mut store, &mut herdr, false, T0 + 40);
        assert_eq!(deliveries(&mut store).len(), 1, "one continue only");
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "one info item: {told:?}");
        assert!(
            told[0].state == MsgState::Info
                && !told[0].urgent
                && told[0].body.contains("restarted on")
        );
    }

    /// An uncertain start whose screen shows a missing CLI is certain enough
    /// (T-286): the seat is flagged and the plan cleared instead of waiting
    /// forever for a switch that cannot happen.
    #[test]
    fn an_uncertain_cross_cli_start_with_a_missing_cli_flags_instead_of_waiting() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex" }"#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        herdr.exit_agent(PANE);
        // The shell is still busy, so the start waits (the environment line
        // has run and is not sent again).
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));
        tick_quota(&mut store, &mut herdr, false, T0 + 30);
        assert_eq!(lane_seat(&mut store).state, SeatState::Starting);

        herdr.set_screen(PANE, "zsh: command not found: codex");
        herdr.inject_next = Some(Inject::Uncertain);
        tick_quota(&mut store, &mut herdr, false, T0 + 40);

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck, "{:?}", seat.state_reason);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(
            reason.contains("the codex CLI is not installed or not on PATH"),
            "{reason}"
        );
        assert_eq!(crate::restart::pending(&mut store, "p:heat").unwrap(), None);
        assert_eq!(agent_starts(&herdr).len(), 2, "{:?}", agent_starts(&herdr));

        tick_quota(&mut store, &mut herdr, false, T0 + 50);
        assert_eq!(
            agent_starts(&herdr).len(),
            2,
            "no third agent_start: {:?}",
            herdr.calls
        );
    }

    /// A restart whose next CLI is not installed says so and stops (T-286):
    /// the seat is flagged for the install, the plan is cleared, and no later
    /// tick sends the start again — a missing CLI is not a switch that never
    /// ends.
    #[test]
    fn a_cross_cli_restart_to_a_missing_cli_flags_the_seat_once() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=medium"] }"#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        // The limit is confirmed: the restart begins (the agent is closed).
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(agent_starts(&herdr).is_empty(), "nothing started yet");
        herdr.exit_agent(PANE);
        // The shell has no codex on PATH when the start comes.
        herdr.set_screen(PANE, "zsh: command not found: codex");
        herdr.refuse_starts = Some(("agent_start_timeout".into(), 1));

        tick_quota(&mut store, &mut herdr, false, T0 + 30);

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck, "{:?}", seat.state_reason);
        let reason = seat.state_reason.unwrap_or_default();
        assert!(
            reason.contains("the codex CLI is not installed or not on PATH"),
            "{reason}"
        );
        assert_eq!(
            crate::restart::pending(&mut store, "p:heat").unwrap(),
            None,
            "the plan is cleared"
        );
        assert_eq!(
            agent_starts(&herdr).len(),
            1,
            "one attempt: {:?}",
            agent_starts(&herdr)
        );

        // The next tick does not send the start again.
        tick_quota(&mut store, &mut herdr, false, T0 + 40);
        assert_eq!(
            agent_starts(&herdr).len(),
            1,
            "no second agent_start: {:?}",
            herdr.calls
        );
        assert_eq!(lane_seat(&mut store).state, SeatState::NeedsCheck);
    }

    #[test]
    fn a_cross_cli_restart_waits_for_a_fresh_idle_status() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex" }"#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        // The observation says idle, but the agent has begun working since.
        herdr.set_status(PANE, gear_herdr::AgentStatus::Working);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(exits(&herdr).is_empty());
        assert!(agent_starts(&herdr).is_empty());
        assert!(crate::restart::pending(&mut store, "p:heat")
            .unwrap()
            .is_some());
        herdr.set_status(PANE, gear_herdr::AgentStatus::Idle);
        tick_quota(&mut store, &mut herdr, false, T0 + 30);
        assert_eq!(exits(&herdr).len(), 1);
    }

    #[test]
    fn a_refused_cross_cli_start_retries_the_reserved_generation() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex" }"#,
            "fallback",
        ));
        store
            .write(|tx| {
                let op = tx.insert_op(&NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: OpKind::Doorbell,
                    task: None,
                    attempt: None,
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({ "text": "gear", "covers": ["M-404"] }),
                    created: T0,
                })?;
                tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
                tx.set_op_state("p", &op.id, OpState::Submitted, None, T0)
            })
            .unwrap();
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        herdr.exit_agent(PANE);
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));
        tick_quota(&mut store, &mut herdr, false, T0 + 30);
        assert_eq!(lane_seat(&mut store).state, SeatState::Starting);
        assert!(deliveries(&mut store).is_empty());
        // A later tick resumes the durable plan with the same generation.
        tick_quota(&mut store, &mut herdr, false, T0 + 40);
        assert_eq!(lane_seat(&mut store).generation, 2);
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
        assert_eq!(deliveries(&mut store).len(), 1);
        assert_eq!(exits(&herdr).len(), 1);
        assert_eq!(
            store
                .read(|tx| tx.get_op("p", "O-1"))
                .unwrap()
                .unwrap()
                .state,
            OpState::Confirmed
        );
        let (_, flagged) = crate::ops::confirm_pending(&mut store, "p", 1, T0 + 50).unwrap();
        assert!(
            flagged.is_empty(),
            "the departed generation cannot flag the replacement"
        );
    }

    #[test]
    fn reset_returns_to_the_original_cli_and_confirms_its_model() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex" }"#,
            "fallback",
        );
        text.push_str("\n[models.switch.\"minimax-cn/MiniMax-M3.1-Flash\"]\npicker = \"MiniMax-M3.1-Flash\"\nbar = \"MiniMax-M3.1-Flash\"\n");
        let mut store = world(&text);
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        herdr.exit_agent(PANE);
        tick_quota(&mut store, &mut herdr, false, T0 + 30);
        // Exhaust the fallback and wait for the configured reset.
        tick_quota(&mut store, &mut herdr, true, T0 + 40);
        tick_quota(&mut store, &mut herdr, true, T0 + 50);
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
        let reset = T0 + 50 + 5 * 3600;
        tick_quota(&mut store, &mut herdr, false, reset);
        herdr.exit_agent(PANE);
        herdr.start_screen = Some("Build auto · wrong-model · high".into());
        tick_quota(&mut store, &mut herdr, false, reset + 10);
        assert_eq!(agent_starts(&herdr).last().unwrap().0, "opencode");
        assert!(
            deliveries(&mut store).is_empty(),
            "no Continue before model confirmation"
        );
        // Onboarding must not take over this pending restart's model switch.
        assert!(
            crate::onboard::onboard_queued(&mut store, &mut herdr, &cfg(), reset + 11)
                .unwrap()
                .is_empty()
        );
        herdr.set_screen(PANE, "Build auto · MiniMax-M3.1-Flash MiniMax · high");
        tick_quota(&mut store, &mut herdr, false, reset + 20);
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
        assert_eq!(lane_seat(&mut store).generation, 3);
        assert_eq!(
            deliveries(&mut store).len(),
            1,
            "one Continue for the new generation"
        );
        assert_eq!(agent_starts(&herdr).len(), 2);
    }

    /// T-35: the same CLI keeps the in-place switch (T-33): a plain chain
    /// entry names no CLI, so the seat's own CLI runs the next model and the
    /// conversation stays.
    #[test]
    fn a_plain_chain_entry_switches_in_place() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        );
        text.push_str(
            "\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n",
        );
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            (
                "/model".into(),
                "  Search\n  Recent\n  DeepSeek V4.1 Flash  OpenCode Go".into(),
            ),
            (
                "DeepSeek V4.1 Flash".into(),
                "Build auto · DeepSeek V4.1 Flash · high".into(),
            ),
        ];

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        for tick in 0..5 {
            tick_quota(&mut store, &mut herdr, true, T0 + 20 + tick);
        }

        assert!(
            agent_starts(&herdr).is_empty(),
            "no restart: {:?}",
            agent_starts(&herdr)
        );
        assert!(
            exits(&herdr).iter().flatten().all(|key| key != "ctrl+c"),
            "the agent was not closed: {:?}",
            exits(&herdr)
        );
        assert_eq!(deliveries(&mut store).len(), 1, "one Continue");
    }

    /// T-33 (F6): the quota line clears while the switch is under way (a toast
    /// closes, a dialog opens, the engine restarts): the plan is durable, so
    /// the next tick continues it — no retyping, one Continue.
    #[test]
    fn a_switch_survives_a_cleared_quota_marker() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        );
        text.push_str(
            "\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n",
        );
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            (
                "/model".into(),
                "  Search\n  Recent\n  DeepSeek V4.1 Flash  OpenCode Go".into(),
            ),
            (
                "DeepSeek V4.1 Flash".into(),
                "Build auto · DeepSeek V4.1 Flash · high".into(),
            ),
        ];

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(crate::switch::pending(&mut store, "p:heat")
            .unwrap()
            .is_some());

        // The marker is gone from the screen from now on: the switch goes on.
        for tick in 0..4 {
            tick_quota(&mut store, &mut herdr, false, T0 + 30 + tick);
        }

        let sent = texts(&herdr);
        assert_eq!(
            sent.iter().filter(|t| *t == "/model").count(),
            1,
            "typed once: {sent:?}"
        );
        assert_eq!(deliveries(&mut store).len(), 1, "one Continue");
        assert_eq!(
            meta_get(&mut store, &meta_key("model", "p:heat"))
                .unwrap()
                .as_deref(),
            Some("deepseek/deepseek-flash")
        );
        assert!(
            crate::switch::pending(&mut store, "p:heat")
                .unwrap()
                .is_none(),
            "the plan is cleared"
        );
        assert!(
            starts(&herdr).is_empty(),
            "no restart: {:?}",
            starts(&herdr)
        );
    }

    /// T-33 (F4): no labels for the next model — the seat is flagged, never
    /// closed: zero exit keys, zero starts.
    #[test]
    fn a_missing_switch_label_flags_the_seat_without_closing_it() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck, "{:?}", report.events);
        assert!(
            seat.state_reason
                .unwrap_or_default()
                .contains("models.switch"),
            "{report:?}"
        );
        assert!(exits(&herdr).is_empty(), "no exit keys: {:?}", herdr.calls);
        assert!(
            starts(&herdr).is_empty(),
            "no agent start: {:?}",
            herdr.calls
        );
        assert!(texts(&herdr).is_empty(), "nothing typed");
    }

    /// T-60: a failed model switch stops the lane, so the captain is rung — an
    /// urgent item that names the seat and the reason. Once per failure: a
    /// later tick says nothing new.
    #[test]
    fn a_failed_switch_rings_the_captain_once() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);

        // The seat is flagged: no [models.switch] labels for the next model.
        assert_eq!(lane_seat(&mut store).state, SeatState::NeedsCheck);
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", T0 + 30))
            .unwrap();
        assert_eq!(open.len(), 1, "one urgent item: {open:?}");
        assert!(open[0].urgent, "{open:?}");
        assert!(
            open[0].body.contains("p:heat"),
            "it names the seat: {open:?}"
        );
        assert!(
            open[0].body.contains("models.switch"),
            "it names the reason: {open:?}"
        );

        // A later tick does not ring again.
        tick_quota(&mut store, &mut herdr, true, T0 + 40);
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", T0 + 50))
            .unwrap();
        assert_eq!(open.len(), 1, "the failure was told once: {open:?}");
    }

    #[test]
    fn removing_labels_during_a_switch_rings_once_and_clears_the_plan() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        );
        let without_labels = text.clone();
        text.push_str("\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n");
        let mut store = world(&text);
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(crate::switch::pending(&mut store, "p:heat")
            .unwrap()
            .is_some());
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = without_labels;
                tx.update_project(&project)
            })
            .unwrap();
        for tick in 0..4 {
            tick_quota(&mut store, &mut herdr, false, T0 + 30 + tick);
        }
        assert_eq!(lane_seat(&mut store).state, SeatState::NeedsCheck);
        assert!(crate::switch::pending(&mut store, "p:heat")
            .unwrap()
            .is_none());
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].urgent
                && told[0].body.contains("p:heat")
                && told[0].body.contains("no [models.switch] labels any more")
        );
    }

    #[test]
    fn a_cross_cli_restart_timeout_rings_once() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex" }"#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();
        // The old agent never exits, so the replacement cannot start.
        for tick in 0..40 {
            tick_quota(&mut store, &mut herdr, true, T0 + 10 + tick);
        }
        assert_eq!(lane_seat(&mut store).state, SeatState::NeedsCheck);
        assert!(crate::restart::pending(&mut store, "p:heat")
            .unwrap()
            .is_none());
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].urgent
                && told[0].body.contains("p:heat")
                && told[0].body.contains("did not finish")
        );
    }

    #[test]
    fn flagging_the_same_failure_with_a_stale_row_does_not_duplicate_the_alert() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback"));
        let seat = lane_seat(&mut store);
        for tick in 0..3 {
            flag_and_alert(&mut store, "p", &seat, "model switch failed", T0 + tick).unwrap();
        }
        assert_eq!(captain_messages(&mut store).len(), 1);
        // After recovery, another failure must create its own item.
        store
            .write(|tx| tx.set_seat_state(&seat.id, SeatState::Active, None, T0 + 4))
            .unwrap();
        flag_and_alert(&mut store, "p", &seat, "another switch failed", T0 + 5).unwrap();
        assert_eq!(captain_messages(&mut store).len(), 2);
    }

    /// T-60: a switch that never confirms (the timeout in switch.rs) stops the
    /// lane too, and rings the captain once.
    #[test]
    fn a_switch_that_never_confirms_rings_the_captain_once() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        );
        text.push_str(
            "\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n",
        );
        let mut store = world(&text);
        // The screen keeps showing the provider limit: the new model never
        // appears on the status bar, so the switch cannot confirm.
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        // The switch is bounded: the picker never renders and the status bar
        // never confirms, so the plan runs out of ticks.
        for tick in 0..24 {
            tick_quota(&mut store, &mut herdr, true, T0 + 20 + tick);
        }

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck, "{:?}", seat.state_reason);
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", T0 + 40))
            .unwrap();
        assert_eq!(open.len(), 1, "one urgent item: {open:?}");
        assert!(
            open[0].urgent && open[0].body.contains("did not confirm twice"),
            "{open:?}"
        );

        // More ticks do not ring again.
        for tick in 0..5 {
            tick_quota(&mut store, &mut herdr, true, T0 + 60 + tick);
        }
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", T0 + 80))
            .unwrap();
        assert_eq!(open.len(), 1, "the failure was told once: {open:?}");
    }

    /// T-60: a successful switch tells the captain in an **info** line — it
    /// does not ring.
    #[test]
    fn a_successful_switch_does_not_ring_the_captain() {
        let mut text = config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        );
        text.push_str(
            "\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n",
        );
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            (
                "/model".into(),
                "  Search\n  Recent\n  DeepSeek V4.1 Flash  OpenCode Go".into(),
            ),
            (
                "DeepSeek V4.1 Flash".into(),
                "Build auto · DeepSeek V4.1 Flash · high".into(),
            ),
        ];

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(report.events.is_empty(), "{:?}", report.events);
        // Ticks 3..6: /model, the filter, then the bar check.
        for tick in 0..4 {
            tick_quota(&mut store, &mut herdr, true, T0 + 30 + tick);
        }

        // The switch worked: the lane runs on the new model.
        assert_eq!(deliveries(&mut store).len(), 1, "one Continue");
        // No urgent item: a successful switch is an information line.
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", T0 + 40))
            .unwrap();
        assert!(
            open.is_empty(),
            "a successful switch does not ring: {open:?}"
        );
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "one info item: {told:?}");
        assert!(
            told.iter()
                .all(|m| m.state == MsgState::Info && !m.urgent && m.body.contains("switched to")),
            "the captain is told in an info line: {told:?}"
        );
    }

    /// T-60: the cross-CLI restart back to OpenCode, whose model has no
    /// `[models.switch]` labels, stops the lane — the captain is rung.
    #[test]
    fn a_restart_onto_a_model_without_labels_rings_the_captain_once() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex" }"#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        herdr.exit_agent(PANE);
        tick_quota(&mut store, &mut herdr, false, T0 + 30);
        // The chain is exhausted: the lane parks until the reset.
        tick_quota(&mut store, &mut herdr, true, T0 + 40);
        tick_quota(&mut store, &mut herdr, true, T0 + 50);
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);

        // At the reset the lane goes back to the first model of its chain —
        // on OpenCode, whose labels this config does not have.
        let reset = T0 + 50 + 5 * 3600;
        tick_quota(&mut store, &mut herdr, false, reset);
        herdr.exit_agent(PANE);
        herdr.start_screen = Some("Build auto · minimax-cn/MiniMax-M3.1-Flash · high".into());
        tick_quota(&mut store, &mut herdr, false, reset + 10);

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck, "{:?}", seat.state_reason);
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", reset + 20))
            .unwrap();
        // The park before it is an urgent item of its own; the failed model
        // switch is the one that names the missing labels.
        assert!(
            open.iter()
                .any(|m| m.urgent && m.body.contains("models.switch")),
            "{open:?}"
        );
        assert_eq!(
            open.iter()
                .filter(|m| m.body.contains("models.switch"))
                .count(),
            1
        );
        let failure = open
            .iter()
            .find(|m| m.body.contains("models.switch"))
            .unwrap();
        assert_eq!(failure.sender_gen, seat.generation);
        assert!(failure.body.contains("p:heat"));
        let told = open.len();

        // More ticks do not ring again about the same failure.
        for tick in 0..5 {
            tick_quota(&mut store, &mut herdr, false, reset + 30 + tick);
        }
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", reset + 60))
            .unwrap();
        assert_eq!(open.len(), told, "the failure was told once: {open:?}");
    }

    /// M3 acceptance: a switch planned on labels derived from OpenCode's
    /// catalogue (T-94) is continued on them — the config has no
    /// `[models.switch]` entry, and the lane must not be flagged for it.
    #[test]
    fn a_switch_planned_on_derived_labels_is_continued_on_them() {
        let text = config(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback");
        let cfg = gear_app::config::parse(&text).unwrap();
        let mut store = world(&text);
        let mut herdr = FakeHerdr::new();
        herdr.api_replies.push_back(Ok(serde_json::json!({"data": [{
            "providerID": "minimax-cn", "id": "MiniMax-M3.1-Flash", "name": "MiniMax-M3.1-Flash"
        }]})));
        herdr.api_replies.push_back(Ok(serde_json::json!({"data": [
            {"id": "minimax-cn", "name": "MiniMax (minimax.cn)"}
        ]})));
        let target = crate::switch::plan_target(
            &mut store,
            &mut herdr,
            &cfg,
            "p:heat",
            "MiniMax-M3.1-Flash MiniMax (minimax.cn)",
        )
        .unwrap();
        let (model, labels) = target.expect("the catalogue gives the plan's target");
        assert_eq!(model, "minimax-cn/MiniMax-M3.1-Flash");
        assert_eq!(labels.bar, "MiniMax-M3.1-Flash MiniMax (minimax.cn)");

        // A target no chain model renders as is still unknown.
        herdr
            .api_replies
            .push_back(Ok(serde_json::json!({"data": []})));
        assert!(crate::switch::plan_target(
            &mut store,
            &mut herdr,
            &cfg,
            "p:heat",
            "Other Model X"
        )
        .unwrap()
        .is_none());
    }

    /// Every message the engine sent the captain about the lane, in thread
    /// order (the threads are named after the lane).
    fn captain_messages(store: &mut Store) -> Vec<gear_store::MessageRow> {
        let mut all = Vec::new();
        for seq in 1..=20 {
            all.extend(
                store
                    .read(|tx| tx.thread_messages("p", &format!("heat:{seq}")))
                    .unwrap(),
            );
        }
        all
    }

    /// T-33 (F4): a parked lane waking at the provider reset is switched back
    /// in place, never restarted.
    #[test]
    fn a_parked_lane_wakes_up_in_place() {
        let mut text = config(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback");
        text.push_str(
            "\n[models.switch.\"minimax-cn/MiniMax-M3.1-Flash\"]\npicker = \"MiniMax-M3.1-Flash\"\nbar = \"MiniMax-M3.1-Flash\"\n",
        );
        let mut store = world(&text);
        let mut herdr = herdr_with_quota();

        // The chain is exhausted: the lane parks until the reset.
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked);
        let calls_before = herdr.calls.len();

        // The provider has reset: the wake-up switches in place.
        herdr.set_screen(PANE, "Build auto · MiniMax-M3.1-Flash MiniMax · high");
        let mut events = Vec::new();
        for tick in 0..6 {
            // The screen no longer shows the limit: the wake-up tick.
            events.extend(tick_quota(&mut store, &mut herdr, false, T0 + 3600 * 6 + tick).events);
        }
        let after: Vec<&Call> = herdr.calls[calls_before..].iter().collect();
        assert!(
            after
                .iter()
                .all(|call| !matches!(call, Call::AgentStart { .. })),
            "no restart: {after:?}"
        );
        assert!(
            after.iter().all(|call| !matches!(call, Call::SendKeys { keys, .. } if keys.iter().any(|k| k == "ctrl+c"))),
            "no exit keys: {after:?}"
        );
        let seat = lane_seat(&mut store);
        assert_eq!(
            seat.state,
            SeatState::Active,
            "the lane is back: {events:?}"
        );
    }

    /// M-266: a stale limit line on a **working** agent is not a quota turn: it
    /// is never parked (or switched) on screen text alone.
    #[test]
    fn a_working_lane_with_a_stale_limit_line_is_not_parked() {
        let mut store = world(&config(
            r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#,
            "fallback",
        ));
        let mut herdr = herdr_with_quota();
        let seat = lane_seat(&mut store);
        let mut busy = observed(&seat, true);
        busy[0].idle = false;
        busy[0].status = Some(gear_herdr::AgentStatus::Working);

        for tick in 0..5 {
            apply_quota_policy(
                &mut store,
                &mut herdr,
                &cfg(),
                &cfg().profiles,
                "p",
                &busy,
                T0 + 10 + tick,
            )
            .unwrap();
        }
        let seat = lane_seat(&mut store);
        assert_eq!(
            seat.state,
            SeatState::Active,
            "a working lane is not parked"
        );
        assert!(exits(&herdr).is_empty(), "and not taken off its model");
    }

    fn texts(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneType { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    // ------------------------------------------- the quota of the providers (§9.2)

    /// A fake reader (§9.2): it answers what the test says.
    struct FakeQuota {
        answer: std::cell::RefCell<Result<gear_usage::Reading, String>>,
        calls: std::cell::RefCell<Vec<String>>,
        sessions: std::cell::RefCell<Vec<Option<String>>>,
    }

    impl FakeQuota {
        /// The reader answers with a reading.
        fn reading(reading: gear_usage::Reading) -> Self {
            FakeQuota {
                answer: std::cell::RefCell::new(Ok(reading)),
                calls: std::cell::RefCell::new(Vec::new()),
                sessions: std::cell::RefCell::new(Vec::new()),
            }
        }

        /// The reader cannot read the source.
        fn failing(reason: &str) -> Self {
            FakeQuota {
                answer: std::cell::RefCell::new(Err(reason.to_string())),
                calls: std::cell::RefCell::new(Vec::new()),
                sessions: std::cell::RefCell::new(Vec::new()),
            }
        }

        /// The provider answers differently from now on (a quota that ran out
        /// between two readings).
        fn set_answer(&self, reading: gear_usage::Reading) {
            *self.answer.borrow_mut() = Ok(reading);
        }

        /// How many times the provider was asked.
        fn calls(&self) -> usize {
            self.calls.borrow().len()
        }

        /// The sessions the reader was given.
        fn sessions(&self) -> Vec<Option<String>> {
            self.sessions.borrow().clone()
        }
    }

    impl QuotaApi for FakeQuota {
        fn read(
            &self,
            reader: QuotaReader,
            key: &str,
            codex_session: Option<&str>,
            now: Ts,
        ) -> Result<gear_usage::Reading, String> {
            self.calls
                .borrow_mut()
                .push(format!("{}:{key}", reader.name()));
            self.sessions
                .borrow_mut()
                .push(codex_session.map(str::to_string));
            let mut answer = self.answer.borrow_mut();
            match &mut *answer {
                Ok(reading) => {
                    let mut reading = reading.clone();
                    reading.read_at = now;
                    Ok(reading)
                }
                Err(reason) => Err(reason.clone()),
            }
        }
    }

    /// The key file of the source (§9.1): a JSON file with the key of the
    /// provider in it.
    fn key_file(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("auth.json");
        std::fs::write(&path, r#"{"minimax-cn":{"type":"api","key":"secret"}}"#).unwrap();
        path
    }

    /// A project whose lane runs on a provider with a source (§9.1) and whose
    /// round is open (the clock of §5.1).
    fn source_world(key_file: &std::path::Path) -> Store {
        source_world_on_wait(key_file, "keep")
    }

    /// The same project with `[models].on_wait` given (T-326).
    fn source_world_on_wait(key_file: &std::path::Path, on_wait: &str) -> Store {
        let mut text = config_full(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback", on_wait, 5);
        text.push_str(&format!(
            "\n[[quota.sources]]\nprovider = \"minimax-cn\"\nreader = \"minimax\"\nkey_file = \"{}\"\nkey_path = \"minimax-cn.key\"\n",
            key_file.display()
        ));
        let mut store = world(&text);
        store
            .write(|tx| {
                tx.insert_round(&gear_store::NewRound {
                    key: gear_store::RoundKey {
                        project: "p".into(),
                        task: "T-1".into(),
                        attempt: 1,
                        role: "dev".into(),
                    },
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "opencode".into(),
                    model: Some("minimax-cn/MiniMax-M3.1-Flash".into()),
                    start_at: T0,
                })?;
                Ok(())
            })
            .unwrap();
        store
    }

    /// The project of [`source_world_on_wait`] with its lane on pi (T-327).
    fn source_world_pi(key_file: &std::path::Path, on_wait: &str) -> Store {
        let mut text = config_pi(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback", on_wait);
        text.push_str(&format!(
            "\n[[quota.sources]]\nprovider = \"minimax-cn\"\nreader = \"minimax\"\nkey_file = \"{}\"\nkey_path = \"minimax-cn.key\"\n",
            key_file.display()
        ));
        world_kind(&text, "pi")
    }

    /// The state a release (T-326) leaves: the round open with the session
    /// "s-1", the seat parked, the release marked and the wait's books
    /// written. `wait` is the source wait; `None` is a policy park, whose only
    /// wait is the `resume` deadline.
    fn released(store: &mut Store, wait: Option<Wait>, until: Ts) {
        store
            .write(|tx| {
                let key = gear_store::RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: Some("minimax-cn/MiniMax-M3.1-Flash".into()),
                    start_at: T0,
                })?;
                tx.add_round_session(&key, "s-1")?;
                tx.set_seat_state(
                    "p:heat",
                    SeatState::Parked,
                    Some("quota: the window is spent"),
                    T0 + 20,
                )?;
                if let Some(wait) = &wait {
                    write_wait(tx, "p:heat", wait)?;
                    tx.open_pause(
                        "p",
                        "p:heat",
                        "quota",
                        "minimax-cn MiniMax-M3.1-Flash 5h",
                        T0 + 20,
                    )?;
                }
                tx.set_meta(&meta_key("resume", "p:heat"), &until.to_string())?;
                tx.set_meta(&meta_key("quota", "p:heat"), "1")?;
                tx.set_meta(&meta_key("quota_error_ticks", "p:heat"), "0")?;
                tx.set_meta(&meta_key("wait_released", "p:heat"), "1")?;
                Ok(())
            })
            .unwrap();
    }

    /// The observation of a lane with the flags given.
    fn observed_flags(seat: &SeatRow, quota: bool, error: bool) -> Vec<SeatObs> {
        let mut flags = BTreeSet::new();
        if quota {
            flags.insert(ScreenFlag::Quota);
        }
        if error {
            flags.insert(ScreenFlag::Error);
        }
        vec![SeatObs {
            seat: seat.clone(),
            presence: Presence::Present,
            status: Some(gear_herdr::AgentStatus::Idle),
            flags,
            idle: true,
            screen: None,
            session: None,
        }]
    }

    /// One tick of the quota step with the fake reader (§9.2).
    fn tick_source(
        store: &mut Store,
        herdr: &mut FakeHerdr,
        api: &FakeQuota,
        quota: bool,
        error: bool,
        now: Ts,
    ) -> QuotaReport {
        let cfg = cfg();
        tick_source_with(store, herdr, api, &cfg, quota, error, now)
    }

    /// One tick of the quota step with the fake reader and the profiles given
    /// (T-326).
    fn tick_source_with(
        store: &mut Store,
        herdr: &mut FakeHerdr,
        api: &FakeQuota,
        cfg: &EngineCfg,
        quota: bool,
        error: bool,
        now: Ts,
    ) -> QuotaReport {
        let seat = lane_seat(store);
        let obs = observed_flags(&seat, quota, error);
        apply_quota_policy_with(store, herdr, cfg, &cfg.profiles, "p", &obs, now, api).unwrap()
    }

    /// A reading of a provider: one window with so much used.
    fn reading_with(used_percent: f64, resets_at: Option<Ts>) -> gear_usage::Reading {
        gear_usage::Reading {
            provider: "minimax".into(),
            windows: vec![gear_usage::Window {
                name: "MiniMax-M3.1-Flash 5h".into(),
                used_percent,
                resets_at,
            }],
            read_at: T0,
        }
    }

    /// §9.2: a spent window parks the lane until the reset plus a minute, the
    /// round's clock pauses, and at the reset the lane continues in the same
    /// session.
    #[test]
    fn a_spent_window_parks_the_lane_until_the_reset() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 1800)));

        // The first tick reads the source at once (§9.2) and stores the
        // reading (§9.1); the signal is not confirmed yet.
        tick_source(&mut store, &mut herdr, &api, true, false, T0 + 10);
        let raw = meta_get(&mut store, "quota.reading.minimax-cn")
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["provider"], "minimax-cn");
        assert_eq!(value["read_at"], T0 + 10);
        assert_eq!(value["windows"][0]["name"], "MiniMax-M3.1-Flash 5h");
        assert_eq!(value["windows"][0]["used_percent"], 100.0);
        assert_eq!(value["windows"][0]["resets_at"], T0 + 1800);
        assert_eq!(
            lane_seat(&mut store).state,
            SeatState::Active,
            "one tick is no fact"
        );

        // The second tick confirms the signal: the lane parks until the reset
        // of the window plus a minute.
        tick_source(&mut store, &mut herdr, &api, true, false, T0 + 20);
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked);
        let until = T0 + 1800 + 60;
        assert!(
            seat.state_reason
                .as_deref()
                .unwrap_or_default()
                .contains(&until.to_string()),
            "{seat:?}"
        );
        // The pause row of §5.4 is open.
        let pauses = store.read(|tx| tx.pauses_of_project("p")).unwrap();
        assert_eq!(pauses.len(), 1, "{pauses:?}");
        assert_eq!(pauses[0].kind, "quota");
        assert_eq!(
            pauses[0].reason.as_deref(),
            Some("minimax-cn MiniMax-M3.1-Flash 5h")
        );
        assert_eq!(pauses[0].end_at, None);
        // The park and its deadline are one fact (F3): a parked lane always
        // has the time it wakes up.
        let wait = load_wait(&mut store, "p:heat")
            .unwrap()
            .expect("the deadline of the wait");
        assert_eq!(wait.until, until);

        // At the reset the lane continues in the same session (§9.2, §3.3 S4).
        tick_source(&mut store, &mut herdr, &api, false, false, until);
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
        let delivered = deliveries(&mut store);
        assert_eq!(delivered.len(), 1, "{delivered:?}");
        assert_eq!(delivered[0], "Continue T-1: your quota is back.");
        // The wake and its Continue are one fact too (F3): an awake lane has
        // its Continue and no wait.
        assert!(
            load_wait(&mut store, "p:heat").unwrap().is_none(),
            "the wait is over"
        );
        // The pause row ends and the round's clock pauses for the wait (§5.1).
        let pauses = store.read(|tx| tx.pauses_of_project("p")).unwrap();
        assert_eq!(pauses[0].end_at, Some(until));
        let round = store
            .read(|tx| tx.open_round_of_seat("p:heat"))
            .unwrap()
            .unwrap();
        assert_eq!(
            round.paused_s,
            until - (T0 + 20),
            "the clock paused for the park"
        );
    }

    /// T-326: `on_wait = "release"` — a lane parked on a long wait has its
    /// agent exit once; its pane, worktree and session are kept, the seat stays
    /// parked and the captain hears about it.
    #[test]
    fn on_wait_release_exits_the_agent_of_a_long_wait() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world_on_wait(&key, "release");
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 1800)));
        let cfg = cfg_resumable(true);

        // The park of the spent window, as in
        // `a_spent_window_parks_the_lane_until_the_reset`.
        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 10);
        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 20);
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
        let wait = load_wait(&mut store, "p:heat").unwrap().unwrap();
        assert!(wait.pause, "a quota park pauses the round: {wait:?}");
        let until = T0 + 1800 + 60;
        assert_eq!(wait.until, until);
        assert!(
            exits(&herdr).is_empty(),
            "the park is not a release: {:?}",
            herdr.calls
        );

        // The next tick: the agent exits through the profile's exit keys, once.
        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, T0 + 30);
        assert_eq!(
            exits(&herdr),
            vec![vec!["ctrl+c".to_string()]],
            "{:?}",
            herdr.calls
        );
        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        assert!(
            report.events[0].contains(&format!("released for the wait until {until}")),
            "{:?}",
            report.events
        );
        // The release is marked, and the captain gets its line.
        assert_eq!(
            meta_get(&mut store, &meta_key("wait_released", "p:heat"))
                .unwrap()
                .as_deref(),
            Some("1")
        );
        let said = captain_messages(&mut store);
        assert!(
            said.iter().any(|message| message
                .body
                .contains(&format!("released for the wait until {until}"))
                && message
                    .body
                    .contains("its pane, worktree and session are kept")),
            "{said:?}"
        );
        // The seat stays parked and its binding active: only the agent went.
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
        let binding = store
            .read(|tx| tx.get_binding("p:heat", 1))
            .unwrap()
            .unwrap();
        assert_eq!(binding.status, BindingStatus::Active);

        // A third tick sends nothing more.
        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, T0 + 40);
        assert_eq!(exits(&herdr).len(), 1, "one release: {:?}", herdr.calls);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
    }

    /// T-326: with `on_wait = "keep"` (the default) nothing is sent: the park
    /// is exactly as before.
    #[test]
    fn on_wait_keep_leaves_the_agent_in_its_pane() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 1800)));
        let cfg = cfg_resumable(true);

        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 10);
        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 20);
        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, T0 + 30);

        assert!(
            exits(&herdr).is_empty(),
            "the agent stays: {:?}",
            herdr.calls
        );
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert!(
            meta_get(&mut store, &meta_key("wait_released", "p:heat"))
                .unwrap()
                .is_none(),
            "nothing is marked"
        );
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
    }

    /// T-326: a wait that ends in 10 min is not worth an exit: the agent waits
    /// it out.
    #[test]
    fn a_short_wait_is_not_released() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world_on_wait(&key, "release");
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 600)));
        let cfg = cfg_resumable(true);

        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 10);
        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 20);
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
        // `until` is T0 + 660, 630 s from this tick: under the 900 s bar.
        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, T0 + 30);

        assert!(exits(&herdr).is_empty(), "not released: {:?}", herdr.calls);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert!(meta_get(&mut store, &meta_key("wait_released", "p:heat"))
            .unwrap()
            .is_none());
    }

    /// T-326: a policy park (no source) is released too: its deadline is the
    /// wait.
    #[test]
    fn a_policy_park_is_released() {
        let mut store = world(&config_full(
            r#""minimax-cn/MiniMax-M3.1-Flash""#,
            "fallback",
            "release",
            2,
        ));
        let mut herdr = herdr_with_quota();
        let cfg = cfg_resumable(true);

        tick_quota_with(&mut store, &mut herdr, &cfg, true, T0 + 10);
        tick_quota_with(&mut store, &mut herdr, &cfg, true, T0 + 20);
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked);
        let until = T0 + 20 + 2 * 3600;
        assert_eq!(parked_until(&mut store, "p:heat").unwrap(), Some(until));
        assert!(
            load_wait(&mut store, "p:heat").unwrap().is_none(),
            "a fallback park has only its deadline"
        );

        let report = tick_quota_with(&mut store, &mut herdr, &cfg, false, T0 + 30);
        assert_eq!(
            exits(&herdr),
            vec![vec!["ctrl+c".to_string()]],
            "{:?}",
            herdr.calls
        );
        assert!(
            report
                .events
                .iter()
                .any(|line| line.contains(&format!("until {until}"))),
            "{:?}",
            report.events
        );
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
    }

    /// T-326: a CLI whose session cannot be resumed keeps its agent — after the
    /// exit the seat could not come back to the same conversation.
    #[test]
    fn a_profile_without_resume_args_keeps_its_agent() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world_on_wait(&key, "release");
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 1800)));
        let cfg = cfg_resumable(false);

        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 10);
        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 20);
        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, T0 + 30);

        assert!(exits(&herdr).is_empty(), "kept: {:?}", herdr.calls);
        assert!(
            report.events.iter().any(
                |line| line == "p:heat: kept through the wait: its CLI cannot resume a session"
            ),
            "{:?}",
            report.events
        );
        assert!(meta_get(&mut store, &meta_key("wait_released", "p:heat"))
            .unwrap()
            .is_none());
        assert_eq!(lane_seat(&mut store).state, SeatState::Parked);
    }

    /// T-327: a lane released for a quota wait (T-326) has no agent at its
    /// reset. The same CLI comes back in the same pane with the round's
    /// session, and the Continue waits for it — today's wake would type into
    /// an empty pane.
    #[test]
    fn a_released_lane_comes_back_at_the_reset_and_is_told_to_continue() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world_pi(&key, "release");
        let until = T0 + 1800 + 60;
        let wait = Wait {
            kind: "quota".into(),
            until,
            started_at: T0 + 20,
            provider: "minimax-cn".into(),
            window: "MiniMax-M3.1-Flash 5h".into(),
            task: Some("T-1".into()),
            pause: true,
        };
        released(&mut store, Some(wait), until);
        let mut herdr = FakeHerdr::new();
        herdr.add_shell(PANE);
        let cfg = cfg_pi(dir.path());
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 1800)));

        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, until);

        // The same pane gets the same CLI again, on the round's session.
        let starts = agent_starts(&herdr);
        assert_eq!(starts.len(), 1, "{:?}", herdr.calls);
        let (kind, pane, args) = &starts[0];
        assert_eq!((kind.as_str(), pane.as_str()), ("pi", PANE));
        assert!(
            args.windows(2).any(|pair| pair == ["--session", "s-1"]),
            "{args:?}"
        );
        // The seat is active again on a new generation, and the Continue is
        // its one delivery.
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Active);
        assert_eq!(seat.generation, 2);
        assert_eq!(
            deliveries(&mut store),
            vec!["Continue T-1: run `gear` first".to_string()]
        );
        let binding = store
            .read(|tx| tx.get_binding("p:heat", 2))
            .unwrap()
            .unwrap();
        assert_eq!(binding.status, BindingStatus::Active);
        assert_eq!(binding.session_ref.as_deref(), Some("s-1"));
        // The old wake's line is never typed: the pane was a shell.
        assert!(
            !herdr.calls.iter().any(|call| matches!(call, Call::Prompt { text, .. } if text.contains("your quota is back"))),
            "{:?}",
            herdr.calls
        );
        // The wait is gone: its metas are empty and the pause row is closed.
        assert!(
            load_wait(&mut store, "p:heat").unwrap().is_none(),
            "the wait is over"
        );
        for what in ["wait_released", "resume"] {
            assert!(
                meta_get(&mut store, &meta_key(what, "p:heat"))
                    .unwrap()
                    .unwrap_or_default()
                    .is_empty(),
                "{what} is cleared"
            );
        }
        for what in ["quota", "quota_error_ticks"] {
            assert_eq!(
                meta_get(&mut store, &meta_key(what, "p:heat"))
                    .unwrap()
                    .as_deref(),
                Some("0"),
                "{what} starts again"
            );
        }
        let pauses = store.read(|tx| tx.pauses_of_project("p")).unwrap();
        assert_eq!(pauses.len(), 1, "{pauses:?}");
        assert_eq!(pauses[0].end_at, Some(until));
        let round = store
            .read(|tx| tx.open_round_of_seat("p:heat"))
            .unwrap()
            .unwrap();
        assert_eq!(
            round.paused_s,
            until - (T0 + 20),
            "the clock paused for the wait"
        );
        assert!(
            report
                .events
                .iter()
                .any(|line| line.contains("restarted on pi")),
            "{:?}",
            report.events
        );
    }

    /// T-327: a lane released for a **policy** park comes back the same way —
    /// its only wait is the `resume` deadline, so there is no pause row to
    /// close and no round pause to add.
    #[test]
    fn a_released_policy_park_comes_back_at_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = world_kind(
            &config_pi(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback", "release"),
            "pi",
        );
        let until = T0 + 2 * 3600;
        released(&mut store, None, until);
        let mut herdr = FakeHerdr::new();
        herdr.add_shell(PANE);
        let cfg = cfg_pi(dir.path());
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 1800)));

        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, until);

        let starts = agent_starts(&herdr);
        assert_eq!(starts.len(), 1, "{:?}", herdr.calls);
        let (kind, pane, args) = &starts[0];
        assert_eq!((kind.as_str(), pane.as_str()), ("pi", PANE));
        assert!(
            args.windows(2).any(|pair| pair == ["--session", "s-1"]),
            "{args:?}"
        );
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Active);
        assert_eq!(seat.generation, 2);
        assert_eq!(
            deliveries(&mut store),
            vec!["Continue T-1: run `gear` first".to_string()]
        );
        assert_eq!(parked_until(&mut store, "p:heat").unwrap(), None);
        assert!(load_wait(&mut store, "p:heat").unwrap().is_none());
        assert!(meta_get(&mut store, &meta_key("wait_released", "p:heat"))
            .unwrap()
            .unwrap_or_default()
            .is_empty());
        // A policy park has no pause row and no round pause.
        assert!(store
            .read(|tx| tx.pauses_of_project("p"))
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .read(|tx| tx.open_round_of_seat("p:heat"))
                .unwrap()
                .unwrap()
                .paused_s,
            0
        );
        assert!(
            report
                .events
                .iter()
                .any(|line| line.contains("restarted on pi")),
            "{:?}",
            report.events
        );
    }

    /// T-327: a lane kept through its wait (not released) wakes exactly as it
    /// did: today's Continue in the same live session, and nothing started.
    #[test]
    fn a_kept_lane_at_its_reset_gets_todays_continue() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world_on_wait(&key, "release");
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(100.0, Some(T0 + 600)));
        let cfg = cfg_resumable(true);

        // Ten minutes are under the release bar (T-326): the agent stays.
        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 10);
        tick_source_with(&mut store, &mut herdr, &api, &cfg, true, false, T0 + 20);
        let report = tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, T0 + 30);
        assert!(report.events.is_empty(), "{:?}", report.events);
        let until = T0 + 660;

        // At the reset the lane continues in the same session.
        tick_source_with(&mut store, &mut herdr, &api, &cfg, false, false, until);
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
        assert_eq!(
            deliveries(&mut store),
            vec!["Continue T-1: your quota is back.".to_string()]
        );
        assert!(agent_starts(&herdr).is_empty(), "{:?}", herdr.calls);
        assert!(
            meta_get(&mut store, &meta_key("wait_released", "p:heat"))
                .unwrap()
                .is_none(),
            "nothing was released"
        );
    }

    /// §9.2: quota is left — a rate limit or an error, not a quota stop. The
    /// lane waits 5 min and continues; the second time in a row it becomes a
    /// captain item.
    #[test]
    fn quota_left_waits_five_minutes_then_continues() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(30.0, Some(T0 + 1800)));

        // The signal is a provider error this time (§9.2).
        tick_source(&mut store, &mut herdr, &api, false, true, T0 + 10);
        assert_eq!(
            lane_seat(&mut store).state,
            SeatState::Active,
            "one tick is no fact"
        );
        tick_source(&mut store, &mut herdr, &api, false, true, T0 + 20);

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked);
        assert!(
            seat.state_reason
                .as_deref()
                .unwrap_or_default()
                .contains("provider error, not quota"),
            "{seat:?}"
        );
        // It is not quota wait time: no pause row (§5.4).
        assert!(store
            .read(|tx| tx.pauses_of_project("p"))
            .unwrap()
            .is_empty());

        // Five minutes later the lane continues.
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 20 + 300);
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
        let delivered = deliveries(&mut store);
        assert_eq!(delivered.len(), 1, "{delivered:?}");
        assert_eq!(delivered[0], "Continue T-1: your quota is back.");

        // The second time in a row: the captain hears about it (§9.2).
        tick_source(&mut store, &mut herdr, &api, false, true, T0 + 400);
        tick_source(&mut store, &mut herdr, &api, false, true, T0 + 410);
        let open = store
            .read(|tx| tx.actionable_for("p", "p:captain", T0 + 420))
            .unwrap();
        assert!(
            open.iter()
                .any(|message| message.body.contains("p:heat: provider error, not quota")),
            "{open:?}"
        );
    }

    /// §9.2: the source cannot be read — today's park, `quota_reset_h` hours.
    #[test]
    fn an_unreadable_source_parks_for_the_reset_window() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::failing("the provider does not answer");

        tick_source(&mut store, &mut herdr, &api, true, false, T0 + 10);
        tick_source(&mut store, &mut herdr, &api, true, false, T0 + 20);

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked);
        let until = T0 + 20 + 5 * 3600;
        assert!(
            seat.state_reason
                .as_deref()
                .unwrap_or_default()
                .contains(&until.to_string()),
            "{seat:?}"
        );
        // The pause row is open: the lane waits (§5.4).
        let pauses = store.read(|tx| tx.pauses_of_project("p")).unwrap();
        assert_eq!(pauses.len(), 1, "{pauses:?}");
        assert_eq!(pauses[0].kind, "quota");
    }

    /// §9.1: a reading older than 15 min is stale, and the meta says so.
    #[test]
    fn a_stale_reading_is_marked() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        // A reading of 20 minutes ago.
        store
            .write(|tx| {
                tx.set_meta(
                    "quota.reading.minimax-cn",
                    &serde_json::json!({
                        "provider": "minimax-cn",
                        "read_at": T0 - 1200,
                        "windows": [{"name": "5h", "used_percent": 10.0, "resets_at": null}],
                    })
                    .to_string(),
                )
            })
            .unwrap();
        let api = FakeQuota::failing("the provider does not answer");

        tick_source(&mut store, &mut herdr, &api, false, false, T0);

        let raw = meta_get(&mut store, "quota.reading.minimax-cn")
            .unwrap()
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["stale"], true, "{value}");
        assert_eq!(value["read_at"], T0 - 1200, "the old reading is kept");
    }

    /// F1: a new signal reads the source at once. The periodic reading said
    /// quota is left; the provider ran out before the signal came: the
    /// decision must be the fresh reading.
    #[test]
    fn a_new_signal_reads_the_source_again() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::reading(reading_with(30.0, Some(T0 + 1800)));

        // The periodic read (no signal yet).
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 10);
        assert_eq!(api.calls(), 1, "the periodic read");

        // The provider is spent now.
        api.set_answer(reading_with(100.0, Some(T0 + 1800)));

        // The signal comes: the source is read again, at once.
        tick_source(&mut store, &mut herdr, &api, true, false, T0 + 20);
        assert_eq!(api.calls(), 2, "the signal reads the source at once");
        // The confirmation tick shares that reading: no second call.
        tick_source(&mut store, &mut herdr, &api, true, false, T0 + 30);
        assert_eq!(api.calls(), 2, "the confirmation shares the reading");
        // The lane parks until the reset of the spent window plus a minute.
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Parked, "{seat:?}");
        let until = T0 + 1800 + 60;
        assert!(
            seat.state_reason
                .as_deref()
                .unwrap_or_default()
                .contains(&until.to_string()),
            "{seat:?}"
        );
    }

    #[test]
    fn active_episode_with_missing_or_zero_start_is_seeded_and_retained() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        meta_set(&mut store, &meta_key("quota", "p:heat"), "1").unwrap();
        meta_set(&mut store, &meta_key("quota_signal", "p:heat"), "0").unwrap();
        assert_eq!(
            count_quota_tick(&mut store, "p:heat", true, T0 + 4)
                .unwrap()
                .1,
            T0 + 4
        );
        assert_eq!(
            count_quota_tick(&mut store, "p:heat", true, T0 + 8)
                .unwrap()
                .1,
            T0 + 4
        );
        meta_set(&mut store, &meta_key("quota_error_ticks", "p:heat"), "1").unwrap();
        assert_eq!(
            count_error_tick(&mut store, "p:heat", true, T0 + 12)
                .unwrap()
                .1,
            T0 + 12
        );
        assert_eq!(
            count_error_tick(&mut store, "p:heat", true, T0 + 16)
                .unwrap()
                .1,
            T0 + 12
        );
    }

    #[test]
    fn failed_signal_reads_never_reuse_a_pre_episode_reading() {
        for (quota, error) in [(true, false), (false, true)] {
            for cached_percent in [30.0, 100.0] {
                let dir = tempfile::tempdir().unwrap();
                let key = key_file(dir.path());
                let mut store = source_world(&key);
                let mut herdr = FakeHerdr::new();
                herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
                let api = FakeQuota::reading(reading_with(cached_percent, Some(T0 + 1800)));
                tick_source(&mut store, &mut herdr, &api, false, false, T0);
                assert_eq!(api.calls(), 1);
                *api.answer.borrow_mut() = Err("signal read failed".into());
                tick_source(&mut store, &mut herdr, &api, quota, error, T0 + 4);
                let episode = if quota {
                    "engine.quota_signal.p:heat"
                } else {
                    "engine.quota_error_signal.p:heat"
                };
                assert_eq!(
                    meta_get(&mut store, episode).unwrap().as_deref(),
                    Some((T0 + 4).to_string().as_str())
                );
                tick_source(&mut store, &mut herdr, &api, quota, error, T0 + 8);
                assert_eq!(
                    api.calls(),
                    3,
                    "the old reading must not satisfy either signal read"
                );
                let seat = lane_seat(&mut store);
                assert_eq!(seat.state, SeatState::Parked);
                let reason = seat.state_reason.unwrap();
                assert!(reason.contains("cannot be read"), "{reason}");
                assert!(
                    reason.contains(&(T0 + 8 + 5 * 3600).to_string()),
                    "{reason}"
                );
                assert_eq!(
                    meta_get(&mut store, episode).unwrap().as_deref(),
                    Some((T0 + 4).to_string().as_str())
                );
            }
        }
    }

    /// F2: a source that cannot be read is read again in the next window,
    /// not on every tick: an outage does not stall the engine.
    #[test]
    fn an_unreadable_source_is_read_every_five_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let mut store = source_world(&key);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        let api = FakeQuota::failing("the provider does not answer");

        // No signal, no lane on a limit: the engine reads the source on its
        // own schedule.
        tick_source(&mut store, &mut herdr, &api, false, false, T0);
        assert_eq!(api.calls(), 1, "the first read");
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 4);
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 8);
        assert_eq!(api.calls(), 1, "no read before the window is over");
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 300);
        assert_eq!(api.calls(), 2, "the next window reads again");
    }

    #[test]
    fn provider_mapping_keeps_other_models_unchanged() {
        assert_eq!(
            provider_of("minimax-cn/MiniMax-M3.1-Flash", "pi").as_deref(),
            Some("minimax-cn")
        );
        assert_eq!(provider_of("gpt-6.1-terra", "pi"), None);
        assert_eq!(
            provider_of("minimax-cn/model", "codex").as_deref(),
            Some("minimax-cn")
        );
    }

    #[test]
    fn providerless_codex_seat_matches_its_rollout_source() {
        let mut store = codex_world();
        let seat = store.read(|tx| tx.get_seat("p:codex-1")).unwrap().unwrap();
        let config = config(r#"{ model = "gpt-6.1-terra", cli = "codex" }"#, "fallback");
        let config: ProjectConfig = toml::from_str(&config).unwrap();
        let source = Source {
            provider: "codex".into(),
            reader: QuotaReader::Codex,
            key_file: String::new(),
            key_path: String::new(),
        };
        let matched = source_of(&[source], &[&config.models.lanes[0]], &mut store, &seat)
            .unwrap()
            .expect("codex source matches");
        let api = FakeQuota::reading(reading_with(10.0, None));
        read_source(&mut store, &api, &matched, T0 + 10, T0, Some("ses_codex")).unwrap();
        assert_eq!(api.sessions(), vec![Some("ses_codex".into())]);
    }

    /// F4: a codex source needs no key file: the reader is given the session
    /// of the codex reviewer, and its reading is kept.
    #[test]
    fn a_codex_source_reads_the_session_of_the_reviewer() {
        let mut store = codex_world();
        let mut herdr = FakeHerdr::new();
        let api = FakeQuota::reading(reading_with(10.0, None));

        // The periodic read of the codex source.
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 10);
        assert_eq!(api.calls(), 1, "the source was read");
        assert_eq!(
            api.sessions(),
            vec![Some("ses_codex".to_string())],
            "the reader got the session of the reviewer"
        );
        let raw = meta_get(&mut store, "quota.reading.codex")
            .unwrap()
            .expect("the reading is kept");
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["provider"], "codex");
        assert_eq!(value["read_at"], T0 + 10);
    }

    #[test]
    fn codex_source_uses_live_reviewer_instead_of_released_or_void_bindings() {
        let mut store = codex_world();
        store
            .write(|tx| {
                tx.set_seat_state("p:codex-1", SeatState::Released, None, T0 + 1)?;
                tx.set_binding_status("p:codex-1", 1, gear_store::BindingStatus::Void)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "codex-0",
                    SeatRole::Temp,
                    "w1:p8",
                    "codex",
                    T0,
                )?;
                tx.set_seat_state("p:codex-0", SeatState::Standby, None, T0 + 1)?;
                tx.set_binding_session_ref("p:codex-0", 1, "standby-session")?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "codex-2",
                    SeatRole::Temp,
                    "w1:p10",
                    "codex",
                    T0,
                )?;
                tx.set_seat_profile("p:codex-2", "custom-codex-profile")?;
                tx.set_binding_session_ref("p:codex-2", 1, "current-reviewer-session")
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        let api = FakeQuota::reading(reading_with(10.0, None));
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 10);
        assert_eq!(
            api.sessions(),
            vec![Some("current-reviewer-session".into())]
        );
        store
            .write(|tx| tx.set_binding_status("p:codex-2", 1, gear_store::BindingStatus::Void))
            .unwrap();
        assert_eq!(
            codex_session(&mut store, "p").unwrap().as_deref(),
            Some("standby-session")
        );
        store
            .write(|tx| tx.set_binding_status("p:codex-0", 1, gear_store::BindingStatus::Void))
            .unwrap();
        assert_eq!(codex_session(&mut store, "p").unwrap(), None);
        tick_source(&mut store, &mut herdr, &api, false, false, T0 + 400);
        assert_eq!(api.sessions().last(), Some(&None));
    }

    /// A project with two lanes: `heat`, whose restart is pending, and `glow`,
    /// whose own chain is one model — a quota step of its own is a park.
    fn two_lane_config() -> String {
        r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 0

[captain]
pane = "w9:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[[lanes]]
name = "glow"
duty = "everything"
owns = ["lib/"]
kind = "opencode"
profile = "opencode"
models = ["minimax-cn/MiniMax-M3.1-Flash"]

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash", { model = "gpt-6.1-sol", cli = "codex" }]
on_quota = { lanes = "fallback" }
on_wait = "keep"
quota_reset_h = 5
"#
        .to_string()
    }

    /// One seat's failed restart step is reported as an event and never stops
    /// the rest of the project's quota tick: the other lane is still handled in
    /// the same call (bcdc8d0a).
    #[test]
    fn one_seats_failed_restart_step_does_not_stop_the_other_seats_quota_step() {
        const GLOW: &str = "w1:p2";
        let mut store = world(&two_lane_config());
        store
            .write(|tx| {
                testing::seed_active_seat(tx, "p", "glow", SeatRole::Lane, GLOW, "opencode", T0)
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        herdr.add_agent(GLOW, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(GLOW, QUOTA_SCREEN);
        let cfg = cfg();
        let heat = lane_seat(&mut store);
        let glow = store.read(|tx| tx.get_seat("p:glow")).unwrap().unwrap();
        let both = |quota: bool| {
            let mut seats = observed(&heat, quota);
            seats.extend(observed(&glow, quota));
            seats
        };

        // The first quota tick: both lanes count it, neither acts yet.
        apply_quota_policy(
            &mut store,
            &mut herdr,
            &cfg,
            &cfg.profiles,
            "p",
            &both(true),
            T0 + 10,
        )
        .unwrap();
        // The limit is confirmed for heat: its restart onto codex begins.
        apply_quota_policy(
            &mut store,
            &mut herdr,
            &cfg,
            &cfg.profiles,
            "p",
            &observed(&heat, true),
            T0 + 20,
        )
        .unwrap();
        assert!(crate::restart::pending(&mut store, "p:heat")
            .unwrap()
            .is_some());
        // The old agent is gone; the start is refused once, so the generation
        // is reserved and the restart waits before its finish phase.
        herdr.exit_agent(PANE);
        herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));
        apply_quota_policy(
            &mut store,
            &mut herdr,
            &cfg,
            &cfg.profiles,
            "p",
            &observed(&heat, false),
            T0 + 30,
        )
        .unwrap();
        let plan = crate::restart::pending(&mut store, "p:heat")
            .unwrap()
            .unwrap();
        assert_eq!(plan.phase, crate::restart::Phase::Start);
        assert_eq!(lane_seat(&mut store).state, SeatState::Starting);

        // heat's seat is retired mid-restart: `Active` can no longer be reached.
        store
            .write(|tx| {
                tx.set_seat_state(
                    "p:heat",
                    SeatState::Retired,
                    Some("retired mid-restart"),
                    T0 + 35,
                )
            })
            .unwrap();

        // The finish of that restart fails; glow's confirmed limit parks it in
        // the same call.
        let report = apply_quota_policy(
            &mut store,
            &mut herdr,
            &cfg,
            &cfg.profiles,
            "p",
            &both(true),
            T0 + 40,
        )
        .unwrap();

        assert_eq!(report.events.len(), 2, "{:?}", report.events);
        assert!(
            report.events[0].starts_with("p:heat: restart step failed:")
                && report.events[0].contains("retired -> active"),
            "{:?}",
            report.events
        );
        // The failed step is reported, not lost: the plan waits for the next tick.
        assert!(crate::restart::pending(&mut store, "p:heat")
            .unwrap()
            .is_some());
        // The other lane is still handled in the same call.
        assert!(
            report.events[1].starts_with("p:glow: parked until")
                && report.events[1].contains("chain exhausted"),
            "{:?}",
            report.events
        );
        assert_eq!(
            store
                .read(|tx| tx.get_seat("p:glow"))
                .unwrap()
                .unwrap()
                .state,
            SeatState::Parked
        );
    }

    /// A project with a codex reviewer and a codex source without a key file
    /// (§9.1, F4).
    fn codex_world() -> Store {
        let mut text = config(r#""minimax-cn/MiniMax-M3.1-Flash""#, "fallback");
        text.push_str("\n[[quota.sources]]\nprovider = \"codex\"\nreader = \"codex\"\n");
        let mut store = world(&text);
        store
            .write(|tx| {
                testing::seed_active_seat(
                    tx,
                    "p",
                    "codex-1",
                    SeatRole::Temp,
                    "w1:p9",
                    "codex",
                    T0,
                )?;
                tx.set_binding_session_ref("p:codex-1", 1, "ses_codex")
            })
            .unwrap();
        store
    }
}
