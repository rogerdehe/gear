//! Quota policy (PRD §9, design/01 `[models].on_quota`): what the engine does
//! when a lane's agent stops on a provider limit.
//!
//! When a lane's screen shows a provider limit (`Profile::quota_markers`) for
//! the profile's `confirm_rounds` ticks in a row, the engine applies the lane
//! policy of the project config:
//!
//! - `fallback`: restart the lane on the next model of its chain. The chain is
//!   `[models].lanes`, or the lane's own `models` when it has one.
//! - `wait`, or a chain with no next model: park the seat until the provider
//!   resets (`[models].quota_reset_h` hours) and resume on the first model
//!   then.
//!
//! A restart is two ticks, on purpose: the exit keys go first, and a second
//! `ctrl+c` a moment later would land in the shell instead of the agent. The
//! next tick, with the pane a shell again, exports the new model in the
//! environment line (the same line the onboarding uses), starts the agent with
//! the profile's resume args and the binding's session reference, and delivers
//! "Continue T-<n>: run `gear` first" — the conversation continues on the new
//! model.
//!
//! The engine keeps the little state this needs in the store's `meta`
//! (consecutive quota ticks, the lane's current model, the reset deadline, the
//! restart in progress): the `seats` table has `park_until`, but gear-store
//! exposes no setter for it yet, and a store API is the follow-up.

use gear_app::config::{ProjectConfig, QuotaPolicy};
use gear_herdr::{HerdrAct, Profile, ScreenFlag};
use gear_model::{
    ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, SeatRole, SeatState, Ts,
};
use gear_store::{NewMessage, NewOp, SeatRow, Store};

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
    _cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
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

    for obs in observed {
        let seat = &obs.seat;
        if seat.role != SeatRole::Lane {
            continue;
        }
        let Some(profile) = profiles.get(&seat.profile) else {
            continue;
        };
        // A switch already on its way finishes before anything else (F6): the
        // plan is durable, so it resumes whatever the screen says now — the
        // quota line may have cleared, the engine may have restarted mid-switch.
        if crate::switch::pending(store, &seat.id)?.is_some() {
            if let Some(line) =
                resume_switch(store, herdr, project, &config, seat, profile, Some(obs), now)?
            {
                report.events.push(line);
            }
            continue;
        }
        // A seat parked on quota wakes up when the provider resets: it is
        // switched back in place, never closed and restarted (F4).
        if seat.state == SeatState::Parked {
            if parked_until(store, &seat.id)?.is_some_and(|until| until <= now) {
                let chain = lane_chain(&config, &seat.name);
                match chain.first().map(|model| (*model).to_string()) {
                    Some(model) => {
                        if let Some(line) = start_switch(
                            store, herdr, project, &config, seat, profile, Some(obs), &model, now,
                        )? {
                            report.events.push(line);
                        }
                    }
                    None => {
                        flag_needs_check(
                            store,
                            seat,
                            "quota: the lane's chain is empty; no model to switch back to",
                            now,
                        )?;
                    }
                }
            }
            continue;
        }
        // Only a working lane gets the policy.
        if !matches!(
            seat.state,
            SeatState::Active | SeatState::Standby | SeatState::Retiring
        ) {
            continue;
        }
        // 1. count the consecutive quota ticks. A marker left on the screen
        //    from before (after a model switch the old limit line stays in the
        //    tail) is not a new fact, and a working agent is never parked on
        //    screen text (M-266): only a stopped agent counts.
        let quota = obs.flags.contains(&ScreenFlag::Quota) && obs.idle;
        let ticks = count_quota_tick(store, &seat.id, quota)?;
        if !quota || ticks < i64::from(profile.confirm_rounds) {
            continue;
        }
        // 2. the policy.
        let chain = lane_chain(&config, &seat.name);
        let current = current_model(store, &seat.id, &chain);
        let policy = config.models.on_quota.lanes;
        let next = match policy {
            QuotaPolicy::Wait => None,
            QuotaPolicy::Fallback => next_model(&chain, current.as_deref()),
        };
        match next {
            Some(next) => {
                // T-33: switch the model in place — no exit, no restart, the
                // conversation stays. Without labels the seat is flagged, never
                // closed (F4): a restart cannot carry a model through the
                // shared OpenCode service.
                if let Some(line) =
                    start_switch(store, herdr, project, &config, seat, profile, Some(obs), &next, now)?
                {
                    report.events.push(line);
                }
            }
            // The chain is exhausted, or the policy is to wait: park until the
            // provider resets.
            None => {
                let hours = i64::from(config.models.quota_reset_h.max(1));
                let until = now + hours * 3600;
                park(store, seat, current.as_deref(), until, now)?;
                let line = format!(
                    "{}: parked until {until} (provider limit, {})",
                    seat.id,
                    match policy {
                        QuotaPolicy::Wait => "policy wait",
                        QuotaPolicy::Fallback => "chain exhausted",
                    }
                );
                tell_captain(store, project, seat, &line, now)?;
                report.events.push(line);
            }
        }
    }
    Ok(report)
}

/// The lane chain: its own `models` when it has one, else `[models].lanes`
/// (17 §0). Empty entries are dropped.
fn lane_chain<'a>(config: &'a ProjectConfig, lane_name: &str) -> Vec<&'a str> {
    let own = config
        .lanes
        .iter()
        .find(|lane| lane.name == lane_name)
        .map(|lane| &lane.models);
    let chain = match own {
        Some(chain) if !chain.is_empty() => chain,
        _ => &config.models.lanes,
    };
    chain
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .collect()
}

/// The model the lane runs on now: the one the last switch recorded, else the
/// first of its chain.
fn current_model(store: &mut Store, seat: &str, chain: &[&str]) -> Option<String> {
    match meta_get(store, &meta_key("model", seat)).ok().flatten() {
        Some(model) if !model.is_empty() => Some(model),
        _ => chain.first().map(|model| (*model).to_string()),
    }
}

/// The model after `current` in the chain; `None` when the chain is exhausted
/// (or `current` is not in it: then the first model is the next one).
fn next_model(chain: &[&str], current: Option<&str>) -> Option<String> {
    let Some(current) = current else {
        return chain.first().map(|model| (*model).to_string());
    };
    match chain.iter().position(|model| *model == current) {
        Some(index) => chain.get(index + 1).map(|model| (*model).to_string()),
        None => chain.first().map(|model| (*model).to_string()),
    }
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
        flag_needs_check(store, seat, &line, now)?;
        return Ok(Some(line));
    };
    let outcome = crate::switch::switch_in_place(store, herdr, &seat.id, profile, obs, labels, now)?;
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
    let model = config
        .models
        .switch
        .iter()
        .find(|(_, labels)| labels.bar == plan.to)
        .map(|(model, _)| model.clone());
    let Some(model) = model else {
        crate::switch::clear_pending(store, &seat.id)?;
        let line = format!(
            "{}: the switch target {:?} has no [models.switch] labels any more — needs check",
            seat.id, plan.to
        );
        flag_needs_check(store, seat, &line, now)?;
        return Ok(Some(line));
    };
    let labels = config.models.switch.get(&model).cloned().expect("just found");
    let outcome = crate::switch::switch_in_place(store, herdr, &seat.id, profile, obs, &labels, now)?;
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
            flag_needs_check(store, seat, &line, now)?;
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
    let text = continue_text(store, &seat.id)?;
    let open = open_attempt(store, &seat.id)?;
    store.write(|tx| {
        tx.set_meta(&meta_key("model", &seat.id), model)?;
        tx.set_meta(&meta_key("quota", &seat.id), "0")?;
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
        let row = tx.get_seat(&seat.id)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id))
        })?;
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

/// `Continue T-<n>: run `gear` first` for the lane's open attempt.
fn continue_text(store: &mut Store, seat: &str) -> GearResult<String> {
    match open_attempt(store, seat)? {
        Some((task, _)) => Ok(format!("Continue {task}: run `gear` first")),
        None => Ok("Continue: run `gear` first".to_string()),
    }
}

/// The seat's open attempt: (task, attempt).
fn open_attempt(store: &mut Store, seat: &str) -> GearResult<Option<(String, i64)>> {
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

/// Flag the seat for a human (`NeedsCheck`): the engine must not guess.
fn flag_needs_check(
    store: &mut Store,
    seat: &SeatRow,
    reason: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let seat = tx.get_seat(&seat.id)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id))
        })?;
        if seat.state == SeatState::NeedsCheck {
            return tx.set_seat_state(&seat.id, SeatState::NeedsCheck, Some(reason), now);
        }
        gear_model::state::require_transition(
            SeatState::can_transition(seat.role, seat.state, SeatState::NeedsCheck),
            "seat",
            seat.state.as_str(),
            SeatState::NeedsCheck.as_str(),
        )?;
        tx.set_seat_state(&seat.id, SeatState::NeedsCheck, Some(reason), now)
    })
}

/// The reset deadline of a parked lane, if it was parked on quota.
fn parked_until(store: &mut Store, seat: &str) -> GearResult<Option<Ts>> {
    Ok(meta_get(store, &meta_key("resume", seat))?
        .and_then(|value| value.parse::<Ts>().ok()))
}

/// The seats of `project` whose screen shows a provider limit right now: the
/// consecutive-quota counter is above zero. Read outside any transaction (the
/// tick counts first, then confirms).
pub fn seats_on_quota(store: &mut Store, project: &str) -> GearResult<Vec<String>> {
    let seats: Vec<String> = store
        .read(|tx| Ok(tx.seats_of_project(project)?.into_iter().map(|seat| seat.id).collect()))?;
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
fn count_quota_tick(store: &mut Store, seat: &str, quota: bool) -> GearResult<i64> {
    let key = meta_key("quota", seat);
    let previous = meta_get(store, &key)?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let ticks = if quota { previous + 1 } else { 0 };
    meta_set(store, &key, &ticks.to_string())?;
    Ok(ticks)
}

/// A key of the engine's own state in the store's meta.
fn meta_key(what: &str, seat: &str) -> String {
    format!("engine.{what}.{seat}")
}

/// Read one key of the engine's own state.
fn meta_get(store: &mut Store, key: &str) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(key))
}

/// Write one key of the engine's own state.
fn meta_set(store: &mut Store, key: &str, value: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(key, value))
}

/// Tell the captain about a switch or a park: one information line.
fn tell_captain(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    line: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        let captain = tx.captain_of(project)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("project {project} has no captain"))
        })?;
        let seq = next_thread_seq(tx, project, &captain.id, now, &seat.name)?;
        tx.insert_message(&NewMessage {
            project: project.to_string(),
            thread: format!("{}:{seq}", seat.name),
            task: None,
            attempt: None,
            sender: seat.id.clone(),
            sender_gen: seat.generation,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::Presence;
    use gear_herdr::fake::{Call, FakeHerdr};
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
quota_reset_h = 5
"#
        )
    }

    /// A world with the lane's task T-1 started (so a continue can be
    /// delivered), its seat active and bound to `PANE`.
    fn world(config_toml: &str) -> Store {
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
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "opencode", T0)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w9:p1", "claude", T0)?;
                tx.set_binding_session_ref("p:heat", 1, "ses_lane")?;
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
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
                Ok(())
            })
            .unwrap();
        store
    }

    fn cfg() -> EngineCfg {
        let mut cfg = EngineCfg { db_path: "/state/gear.db".into(), ..Default::default() };
        for kind in ["opencode", "claude"] {
            cfg.profiles.insert(kind.into(), gear_herdr::builtin(kind).unwrap());
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
        }]
    }

    /// One quota tick of the policy, with the lane's observation this tick.
    fn tick_quota(store: &mut Store, herdr: &mut FakeHerdr, quota: bool, now: Ts) -> QuotaReport {
        let seat = lane_seat(store);
        let obs = observed(&seat, quota);
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
        assert!(report.events[0].contains("chain exhausted"), "{:?}", report.events);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Parked);
        let until = T0 + 20 + 5 * 3600;
        assert!(seat.state_reason.unwrap_or_default().contains(&until.to_string()));
        assert!(exits(&herdr).is_empty(), "nothing is restarted");

        // Before the reset: still parked, nothing happens.
        let report = tick_quota(&mut store, &mut herdr, false, until - 1);
        assert!(report.events.is_empty());
        assert!(exits(&herdr).is_empty());

        // At the reset: no `[models.switch]` labels in this config, so the
        // lane is flagged for a human — never closed and restarted (T-33, F4).
        let report = tick_quota(&mut store, &mut herdr, false, until);
        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        assert!(report.events[0].contains("models.switch"), "{:?}", report.events);
        assert!(exits(&herdr).is_empty(), "no exit keys: {:?}", herdr.calls);
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck);
    }

    #[test]
    fn the_wait_policy_parks_even_with_a_longer_chain() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "wait"));
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);

        assert!(report.events[0].contains("policy wait"), "{:?}", report.events);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Parked);
        assert!(exits(&herdr).is_empty(), "waited, not restarted");
    }

    #[test]
    fn a_quota_turn_does_not_flag_the_seat() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
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
            let (_, flagged) = crate::ops::confirm_pending(&mut store, "p", 1, T0 + 20 + tick).unwrap();
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
            ("DeepSeek V4.1 Flash".into(), "Build auto · DeepSeek V4.1 Flash · high".into()),
        ];

        // Tick 1: one quota screen is not enough (`confirm_rounds` is 2).
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        assert!(texts(&herdr).is_empty(), "nothing typed on the first tick");
        // Tick 2: the limit is confirmed — the switch starts (esc this tick).
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert!(texts(&herdr).is_empty(), "only the escape so far");
        assert!(deliveries(&mut store).is_empty(), "no continue before the bar confirms");

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
        assert!(starts(&herdr).is_empty(), "no restart: {:?}", starts(&herdr));
        assert!(texts(&herdr).iter().any(|t| t == "/model"), "{:?}", texts(&herdr));
        assert!(texts(&herdr).iter().any(|t| t == "DeepSeek V4.1 Flash"), "{:?}", texts(&herdr));
        // Exactly one Continue, on the new model.
        let delivered = deliveries(&mut store);
        assert_eq!(delivered.len(), 1, "{delivered:?}");
        assert_eq!(meta_get(&mut store, &meta_key("model", "p:heat")).unwrap().as_deref(), Some("deepseek/deepseek-flash"));
        // More quota ticks add no second Continue.
        tick_quota(&mut store, &mut herdr, true, T0 + 60);
        assert_eq!(deliveries(&mut store).len(), 1, "one continue only");
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
            ("DeepSeek V4.1 Flash".into(), "Build auto · DeepSeek V4.1 Flash · high".into()),
        ];

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(crate::switch::pending(&mut store, "p:heat").unwrap().is_some());

        // The marker is gone from the screen from now on: the switch goes on.
        for tick in 0..4 {
            tick_quota(&mut store, &mut herdr, false, T0 + 30 + tick);
        }

        let sent = texts(&herdr);
        assert_eq!(sent.iter().filter(|t| *t == "/model").count(), 1, "typed once: {sent:?}");
        assert_eq!(deliveries(&mut store).len(), 1, "one Continue");
        assert_eq!(
            meta_get(&mut store, &meta_key("model", "p:heat")).unwrap().as_deref(),
            Some("deepseek/deepseek-flash")
        );
        assert!(crate::switch::pending(&mut store, "p:heat").unwrap().is_none(), "the plan is cleared");
        assert!(starts(&herdr).is_empty(), "no restart: {:?}", starts(&herdr));
    }

    /// T-33 (F4): no labels for the next model — the seat is flagged, never
    /// closed: zero exit keys, zero starts.
    #[test]
    fn a_missing_switch_label_flags_the_seat_without_closing_it() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);

        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::NeedsCheck, "{:?}", report.events);
        assert!(seat.state_reason.unwrap_or_default().contains("models.switch"), "{report:?}");
        assert!(exits(&herdr).is_empty(), "no exit keys: {:?}", herdr.calls);
        assert!(starts(&herdr).is_empty(), "no agent start: {:?}", herdr.calls);
        assert!(texts(&herdr).is_empty(), "nothing typed");
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
            after.iter().all(|call| !matches!(call, Call::AgentStart { .. })),
            "no restart: {after:?}"
        );
        assert!(
            after.iter().all(|call| !matches!(call, Call::SendKeys { keys, .. } if keys.iter().any(|k| k == "ctrl+c"))),
            "no exit keys: {after:?}"
        );
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Active, "the lane is back: {events:?}");
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
            apply_quota_policy(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", &busy, T0 + 10 + tick).unwrap();
        }
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Active, "a working lane is not parked");
        assert!(exits(&herdr).is_empty(), "and not taken off its model");
    }

    fn texts(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneRun { command, .. } => Some(command.clone()),
                _ => None,
            })
            .collect()
    }

}
