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
use gear_herdr::{HerdrAct, Outcome, Profile, ScreenFlag};
use gear_model::{
    ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, SeatRole, SeatState, Ts,
};
use gear_store::{NewMessage, NewOp, SeatRow, Store};

use crate::observe::SeatObs;
use crate::EngineCfg;

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
        let Some(profile) = cfg.profiles.get(&seat.profile) else {
            continue;
        };
        // A restart that is already on its way finishes before anything else.
        if restart_model(store, &seat.id)?.is_some() {
            if let Some(line) =
                finish_restart(store, herdr, cfg, project, &config, seat, profile, now)?
            {
                report.events.push(line);
            }
            continue;
        }
        // A seat parked on quota wakes up when the provider resets.
        if seat.state == SeatState::Parked {
            if parked_until(store, &seat.id)?.is_some_and(|until| until <= now) {
                let chain = lane_chain(&config, &seat.name);
                let model = chain.first().map(|model| (*model).to_string());
                if let Some(line) =
                    begin_restart(store, herdr, seat, profile, model.as_deref(), now)?
                {
                    report.events.push(line);
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
        // 1. count the consecutive quota ticks.
        let quota = obs.flags.contains(&ScreenFlag::Quota);
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
                if let Some(line) =
                    begin_restart(store, herdr, seat, profile, Some(&next), now)?
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

// ---------------------------------------------------------------- the restart

/// Take the lane off its current model: send the profile's exit keys and
/// remember which model to start on. The next tick starts the agent, when the
/// pane is a shell again.
fn begin_restart(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
    profile: &Profile,
    model: Option<&str>,
    now: Ts,
) -> GearResult<Option<String>> {
    let _ = now;
    let Some(binding) = active_binding(store, seat)? else {
        return Ok(None);
    };
    // One press per entry, never the same key twice by accident: a second
    // `ctrl+c` would land in the shell.
    for key in &profile.exit_keys {
        match herdr.send_keys(&binding.pane_id, &[key.clone(), ]) {
            Outcome::Ok(()) => {}
            _ => return Ok(None),
        }
    }
    meta_set(store, &meta_key("restart", &seat.id), model.unwrap_or("-"))?;
    // The counter is not reset here: the old agent's screen still shows the
    // limit, and while it does the seat must not be flagged (F2 of the T-15
    // review). It goes back to zero when the screen does.
    let from = current_model(store, &seat.id, &[]).unwrap_or_else(|| "-".to_string());
    Ok(Some(format!(
        "{}: quota limit, restarting on {} (was {from})",
        seat.id,
        model.unwrap_or("the first model")
    )))
}

/// The model a restart in progress starts on, if there is one. The meta has no
/// delete, so the empty value means "no restart"; `-` means "start on the
/// CLI's own default" (no model in the environment line).
fn restart_model(store: &mut Store, seat: &str) -> GearResult<Option<String>> {
    Ok(match meta_get(store, &meta_key("restart", seat))? {
        Some(model) if model == "-" => Some(String::new()),
        Some(model) if !model.is_empty() => Some(model),
        _ => None,
    })
}

/// Start the agent again on the model the restart fixed: the environment line
/// carries it, the args carry the profile's unattended and resume args with the
/// binding's session reference, and the lane gets "Continue T-<n>".
fn finish_restart(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    project: &str,
    config: &ProjectConfig,
    seat: &SeatRow,
    profile: &Profile,
    now: Ts,
) -> GearResult<Option<String>> {
    let Some(binding) = active_binding(store, seat)? else {
        // No pane to start in: forget the restart rather than spin.
        meta_set(store, &meta_key("restart", &seat.id), "")?;
        return Ok(None);
    };
    let raw = restart_model(store, &seat.id)?.unwrap_or_default();
    let model = if raw.is_empty() { None } else { Some(raw) };

    // 1. the pane: a shell accepts the environment line; an agent of this
    //    seat's kind means the start was accepted before the engine died, so
    //    it is adopted rather than started twice; anything else is parked (F3).
    let holder = match herdr.agent_get(&binding.pane_id) {
        Ok(Some(info)) if info.agent.as_deref() == Some(binding.kind.as_str()) => Some(info),
        Ok(Some(_)) => {
            park_for_human(store, seat, "another agent holds the lane's pane", now)?;
            return Ok(None);
        }
        Ok(None) => None,
        Err(error) => {
            park_for_human(store, seat, &format!("agent_get: {}", error.0), now)?;
            return Ok(None);
        }
    };

    let _started = match holder {
        Some(info) => info,
        None => {
            // 2. the session id: without it the restart would begin a new
            //    conversation, which is not a restart. Take the binding's, or
            //    the one the CLI printed on exit (the SOP's exit line), and
            //    park when there is none (F1).
            let session = match binding.session_ref.clone() {
                Some(session) if !session.is_empty() => session,
                _ => match session_from_pane(herdr, &binding.pane_id) {
                    Some(session) => {
                        store.write(|tx| {
                            tx.set_binding_session_ref(&seat.id, seat.generation, &session)
                        })?;
                        session
                    }
                    None => {
                        park_for_human(
                            store,
                            seat,
                            "quota: no session id to resume; not restarting blind",
                            now,
                        )?;
                        return Ok(None);
                    }
                },
            };

            // 3. the environment line: the call context and the model.
            let mut env_command = format!(
                "export GEAR_CTX={}:{}:{}; export GEAR_DB={}",
                project, seat.name, seat.generation, cfg.db_path
            );
            let mut args = profile.unattended_args.clone();
            if let Some(model) = model.as_deref() {
                if let Some(start) = crate::onboard::model_start(&binding.kind, model) {
                    if !start.env.is_empty() {
                        env_command = format!("{env_command}; {}", start.env);
                    }
                    args.extend(start.args);
                }
            }
            // 4. the same conversation: the profile's resume args and the id.
            if let Some(resume) = profile.resume_args.as_ref() {
                args.extend(resume.iter().map(|arg| arg.replace("{session}", &session)));
            }
            if let Outcome::Ok(()) = herdr.pane_run(&binding.pane_id, &env_command) {
                // the environment is set
            } else {
                return Ok(None);
            }
            match herdr.agent_start(&agent_name(config, seat), &binding.kind, &binding.pane_id, &args) {
                Outcome::Ok(info) => info,
                // The pane may still be exiting: the next tick tries again.
                _ => return Ok(None),
            }
        }
    };

    // 4. the seat is working again, on the new model.
    store.write(|tx| {
        let seat = tx.get_seat(&seat.id)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id))
        })?;
        if seat.state == SeatState::Parked {
            gear_model::state::require_transition(
                SeatState::can_transition(seat.role, seat.state, SeatState::Active),
                "seat",
                seat.state.as_str(),
                SeatState::Active.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Active, Some("quota switch"), now)?;
        }
        Ok(())
    })?;
    meta_set(store, &meta_key("restart", &seat.id), "")?;
    meta_set(store, &meta_key("quota", &seat.id), "0")?;
    meta_set(store, &meta_key("resume", &seat.id), "")?;
    if let Some(model) = model.as_deref() {
        meta_set(store, &meta_key("model", &seat.id), model)?;
    }

    // 5. the lane continues its open task on the new model.
    let text = continue_text(store, &seat.id)?;
    if let Some((task, attempt)) = open_attempt(store, &seat.id)? {
        store.write(|tx| {
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
            Ok(())
        })?;
    }
    let line = format!(
        "{}: restarted on {} (provider limit)",
        seat.id,
        model.as_deref().unwrap_or("its first model")
    );
    tell_captain(store, project, seat, &line, now)?;
    Ok(Some(line))
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

/// The seat's active binding: the pane the agent runs in.
fn active_binding(store: &mut Store, seat: &SeatRow) -> GearResult<Option<gear_store::BindingRow>> {
    let (id, generation) = (seat.id.clone(), seat.generation);
    store.read(|tx| {
        Ok(tx
            .get_binding(&id, generation)?
            .filter(|binding| binding.status == gear_store::BindingStatus::Active))
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

/// The session id an OpenCode agent prints on exit (`Continue  opencode -s
/// <id>`, see `skills/gear/references/agents/opencode.md`): the last `-s` token
/// of the pane's tail.
fn session_from_pane(herdr: &mut dyn HerdrAct, pane: &str) -> Option<String> {
    let tail = herdr.read_tail(pane, 40).ok()?;
    let mut found = None;
    for line in tail.lines() {
        let mut parts = line.split_whitespace();
        while let Some(word) = parts.next() {
            if word == "-s" {
                if let Some(id) = parts.next() {
                    found = Some(id.trim_matches(|c: char| c == '`' || c == ',' || c == ';').to_string());
                }
            }
        }
    }
    found.filter(|id| !id.is_empty())
}

/// A restart that cannot continue safely: the seat waits for a human.
fn park_for_human(
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

/// The Herdr agent name of a lane: `<project>-<lane>` (17 §2).
fn agent_name(config: &ProjectConfig, seat: &SeatRow) -> String {
    format!("{}-{}", config.project.id, seat.name)
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

    /// The lane's observation when its agent has exited.
    fn gone(seat: &SeatRow) -> Vec<SeatObs> {
        vec![SeatObs {
            seat: seat.clone(),
            presence: Presence::Missing,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
        }]
    }

    /// One quota tick of the policy, with the lane's observation this tick.
    fn tick_quota(store: &mut Store, herdr: &mut FakeHerdr, quota: bool, now: Ts) -> QuotaReport {
        let seat = lane_seat(store);
        let obs = observed(&seat, quota);
        apply_quota_policy(store, herdr, &cfg(), "p", &obs, now).unwrap()
    }

    /// One tick with the agent gone (the restart continues).
    fn tick_gone(store: &mut Store, herdr: &mut FakeHerdr, now: Ts) -> QuotaReport {
        let seat = lane_seat(store);
        let obs = gone(&seat);
        apply_quota_policy(store, herdr, &cfg(), "p", &obs, now).unwrap()
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

    fn env_lines(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneRun { command, .. } => Some(command.clone()),
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
    fn a_quota_limit_restarts_the_lane_on_the_next_model() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
        let mut herdr = herdr_with_quota();

        // Tick 1: one quota screen is not enough (`confirm_rounds` is 2).
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 10);
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert!(exits(&herdr).is_empty());

        // Tick 2: the limit is confirmed — the agent is taken off the model.
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        assert!(report.events[0].contains("restarting on deepseek/deepseek-flash"), "{:?}", report.events);
        assert_eq!(exits(&herdr), vec![vec!["ctrl+c".to_string()]]);

        // The agent exits; the next tick starts it on the next model.
        herdr.exit_agent(PANE);
        let report = tick_gone(&mut store, &mut herdr, T0 + 30);
        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        assert_eq!(
            env_lines(&herdr),
            vec![
                "export GEAR_CTX=p:heat:1; export GEAR_DB=/state/gear.db; \
                 export OPENCODE_CONFIG_CONTENT='{\"model\":\"deepseek/deepseek-flash\"}'"
                    .to_string()
            ]
        );
        // … with the resume args and the session reference: the conversation
        // continues on the new model.
        assert_eq!(starts(&herdr), vec![vec![
            "--auto".to_string(),
            "--standalone".to_string(),
            "-s".to_string(),
            "ses_lane".to_string(),
        ]]);
        // … and one continue delivery.
        assert_eq!(deliveries(&mut store), vec!["Continue T-1: run `gear` first".to_string()]);
        // The switch reached the captain as one information line (Info: it
        // does not ring the doorbell, it is read on the home page).
        let thread = store.read(|tx| tx.thread_messages("p", "heat:1")).unwrap();
        assert_eq!(thread.len(), 1, "{thread:?}");
        assert!(thread[0].body.contains("restarted on deepseek/deepseek-flash"), "{:?}", thread[0].body);
        assert_eq!(thread[0].recipient, "p:captain");
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

        // At the reset: the lane comes back on the first model.
        let report = tick_quota(&mut store, &mut herdr, false, until);
        assert_eq!(report.events.len(), 1, "{:?}", report.events);
        assert!(report.events[0].contains("restarting on minimax-cn/MiniMax-M3.1-Flash"), "{:?}", report.events);
        assert_eq!(exits(&herdr), vec![vec!["ctrl+c".to_string()]]);
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

    /// F1: OpenCode reports no session reference, so the restart took a fresh
    /// conversation. The id OpenCode prints on exit is used instead, and the
    /// binding keeps it for the next time.
    #[test]
    fn an_opencode_restart_resumes_the_same_conversation() {
        let seat_row = |store: &mut Store| -> SeatRow {
            store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap()
        };
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
        // OpenCode reports no session reference: the binding has none.
        store
            .write(|tx| tx.set_binding_session_ref("p:heat", 1, ""))
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);

        // Two quota ticks: the exit keys.
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        // The agent exits and prints its session id (the SOP's exit line).
        herdr.exit_agent(PANE);
        herdr.set_screen(PANE, "  Session  T-1 work\n  Continue  opencode -s ses_from_pane\n");
        let seat = seat_row(&mut store);

        // The pane is a shell: the restart types the environment there and
        // resumes that conversation.
        let seat_obs = gone(&seat);
        apply_quota_policy(&mut store, &mut herdr, &cfg(), "p", &seat_obs, T0 + 30).unwrap();

        let starts: Vec<Vec<String>> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { args, .. } => Some(args.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 1, "{starts:?}");
        assert!(starts[0].contains(&"-s".to_string()), "{:?}", starts[0]);
        assert!(starts[0].contains(&"ses_from_pane".to_string()), "{:?}", starts[0]);
        // … and the binding keeps it for the next restart.
        let binding = store.read(|tx| tx.get_binding("p:heat", 1)).unwrap().unwrap();
        assert_eq!(binding.session_ref.as_deref(), Some("ses_from_pane"));
    }

    /// F1, the other side: no id anywhere → the lane is not restarted blind,
    /// it waits for a human.
    #[test]
    fn without_a_session_id_the_lane_is_not_restarted() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
        // OpenCode reports no session reference: the binding has none.
        store
            .write(|tx| tx.set_binding_session_ref("p:heat", 1, ""))
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        herdr.exit_agent(PANE);
        // A shell with no exit line in its tail.
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();

        apply_quota_policy(&mut store, &mut herdr, &cfg(), "p", &gone(&seat), T0 + 30).unwrap();

        let starts = herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::AgentStart { .. }))
            .count();
        assert_eq!(starts, 0, "no blind start");
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck, "a human decides");
        assert!(seat.state_reason.unwrap_or_default().contains("session id"));
    }

    /// F2: the tick that takes the lane off its model must not flag it: the
    /// quota counter is still the reason its delivery gets no budget.
    #[test]
    fn the_restart_tick_does_not_flag_the_lane() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
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

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);

        let (_, flagged) = crate::ops::confirm_pending(&mut store, "p", 1, T0 + 30).unwrap();
        assert!(flagged.is_empty(), "the restart tick is not 'no progress'");
    }

    /// F3: the environment line is never typed into an agent. The pane is read
    /// first; a live agent of the same kind is adopted, anything else parks.
    #[test]
    fn the_environment_line_is_not_typed_into_an_agent() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, QUOTA_SCREEN);
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        // The exit has not happened yet: the old agent is still in the pane.
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let runs_before = herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::PaneRun { .. }))
            .count();

        apply_quota_policy(&mut store, &mut herdr, &cfg(), "p", &observed(&seat, true), T0 + 30).unwrap();

        let runs_after = herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::PaneRun { .. }))
            .count();
        assert_eq!(runs_before, runs_after, "nothing was typed into the agent");
        let starts = herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::AgentStart { .. }))
            .count();
        assert_eq!(starts, 0, "the live agent is adopted, not started again");
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::Active, "the lane is back on its task");
    }
}
