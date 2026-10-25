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
//! How the model changes depends on the **CLI** the next entry names (T-35):
//!
//! - the same CLI: the model changes **in place** (T-33) — the picker, the
//!   variant dialog and the status bar; the conversation stays.
//! - another CLI: the agent is closed and that CLI starts in the same pane
//!   and worktree ([`crate::restart`]) — no session crosses CLIs; the new
//!   agent starts from the git state and the lane is told to continue.
//!
//! The engine keeps the little state this needs in the store's `meta`
//! (consecutive quota ticks, the lane's current model, the reset deadline, the
//! restart in progress): the `seats` table has `park_until`, but gear-store
//! exposes no setter for it yet, and a store API is the follow-up.

use gear_app::config::{ModelEntry, ProjectConfig, QuotaPolicy};
use gear_herdr::{HerdrAct, Profile, ScreenFlag};
use gear_model::{
    ErrorCode, GearError, GearResult, MsgKind, MsgState, OpKind, SeatRole, SeatState, Ts,
};
use gear_store::{NewMessage, NewOp, SeatRow, Store};

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
        // A restart on another CLI is on its way (T-35): the plan is durable,
        // so it continues whatever the seat's state says now — the seat is
        // `Missing` or `Starting` while its agent is replaced. It comes
        // before the profile: the plan drives the restart, not the seat's
        // current profile.
        if crate::restart::pending(store, &seat.id)?.is_some() {
            if let Some(line) =
                crate::restart::advance(store, herdr, cfg, profiles, project, seat, None, now)?
            {
                report.events.push(line);
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
            if let Some(line) =
                resume_switch(store, herdr, project, &config, seat, profile, Some(obs), now)?
            {
                report.events.push(line);
            }
            continue;
        }
        // A seat parked on quota wakes up when the provider resets: it goes
        // back to the first model of its chain — in place when that model runs
        // on the seat's own CLI, on another CLI when it does not (T-35).
        if seat.state == SeatState::Parked {
            if parked_until(store, &seat.id)?.is_some_and(|until| until <= now) {
                let chain = crate::model::role_chain(&config, &seat.name);
                match chain.first().copied() {
                    Some(entry) => {
                        if let Some(line) = change_model(
                            store, herdr, cfg, profiles, project, &config, seat, profile, Some(obs),
                            entry, now,
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
        let chain = crate::model::role_chain(&config, &seat.name);
        let current = current_model(store, &seat.id, &chain);
        let policy = config.models.on_quota.lanes;
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
                    store, herdr, cfg, profiles, project, &config, seat, profile, Some(obs), next,
                    now,
                )? {
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
/// (T-33): no exit, no restart, the conversation stays. The model needs
/// **another CLI** (T-35): the agent is closed, the other CLI starts in the
/// same pane and worktree, and the lane continues its task.
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
    let default_cli = config.lanes.iter().find(|lane| lane.name == seat.name)
        .map(|lane| lane.kind.as_str());
    let target_cli = entry.cli().or(default_cli).map(str::to_string).or(current_cli.clone());
    let same_cli = match (&target_cli, &current_cli) {
        (Some(target), Some(current)) => target == current,
        _ => true,
    };
    if same_cli {
        return start_switch(store, herdr, project, config, seat, profile, obs, entry.model(), now);
    }
    let target = ModelEntry::Entry(gear_app::config::ModelSpec {
        model: entry.model().to_string(),
        cli: target_cli,
        args: entry.args().to_vec(),
    });
    crate::restart::advance(store, herdr, cfg, profiles, project, seat, Some(&target), now)
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
    let outcome = crate::switch::switch_model(store, herdr, &seat.id, profile, obs, labels, model, now)?;
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
        flag_and_alert(store, project, seat, &line, now)?;
        return Ok(Some(line));
    };
    let labels = config.models.switch.get(&model).cloned().expect("just found");
    let outcome = crate::switch::switch_model(store, herdr, &seat.id, profile, obs, &labels, &model, now)?;
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
    let text = continue_text(store, &seat.id)?;
    let open = open_attempt(store, &seat.id)?;
    store.write(|tx| {
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
pub(crate) fn continue_text(store: &mut Store, seat: &str) -> GearResult<String> {
    match open_attempt(store, seat)? {
        Some((task, _)) => Ok(format!("Continue {task}: run `gear` first")),
        None => Ok("Continue: run `gear` first".to_string()),
    }
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
        let row = tx.get_seat(&seat.id)?.ok_or_else(|| {
            GearError::new(ErrorCode::NoTarget, format!("no seat {}", seat.id))
        })?;
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

/// The key of a seat's quota counter (how many ticks it showed a provider
/// limit in a row).
pub(crate) fn quota_meta_key(seat: &str) -> String {
    meta_key("quota", seat)
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
pub(crate) fn tell_captain(
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
        // The seat's current generation: a restart on another CLI (T-35) gave
        // the seat a new one.
        let generation = tx
            .get_seat(&seat.id)?
            .map(|row| row.generation)
            .unwrap_or(seat.generation);
        let seq = next_thread_seq(tx, project, &captain.id, now, &seat.name)?;
        tx.insert_message(&NewMessage {
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
        GearError::new(ErrorCode::NoTarget, format!("project {project} has no captain"))
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
        project: project.to_string(),
        thread: format!("{}:{seq}", seat.name),
        task: None,
        attempt: None,
        sender: seat.id.clone(),
        sender_gen: generation,
        recipient: captain.id,
        kind: MsgKind::System,
        urgent: true,
        body: format!("{body} — the lane is stopped: switch it to another model by hand, or let it wait"),
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
                    estimate_min: None,
                     issue: None,updated: T0,
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
        for kind in ["opencode", "claude", "codex"] {
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
            screen: None,
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

    /// The agent starts the engine asked for: (kind, pane, args).
    fn agent_starts(herdr: &FakeHerdr) -> Vec<(String, String, Vec<String>)> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { kind, pane, args, .. } => {
                    Some((kind.clone(), pane.clone(), args.clone()))
                }
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
        // L-74: the captain gets an urgent action item that rings, not an info line.
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 30)).unwrap();
        assert!(
            open.iter().any(|m| m.urgent && m.body.contains("parked until")),
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
        assert!(deliveries(&mut store).is_empty(), "no continue before the agent is up");

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
            meta_get(&mut store, &model_meta_key("p:heat")).unwrap().as_deref(),
            Some("gpt-6.1-sol")
        );
        // The seat is back: it runs the new CLI, in a new generation, in the
        // same pane.
        let seat = lane_seat(&mut store);
        assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
        assert_eq!(seat.generation, 2);
        assert_eq!(seat.profile, "codex");
        let binding = store.read(|tx| tx.get_binding("p:heat", 2)).unwrap().unwrap();
        assert_eq!(binding.kind, "codex");
        assert_eq!(binding.pane_id, PANE);
        let old = store.read(|tx| tx.get_binding("p:heat", 1)).unwrap().unwrap();
        assert_eq!(old.status, gear_store::BindingStatus::Void);

        // More quota ticks add no second Continue.
        tick_quota(&mut store, &mut herdr, false, T0 + 40);
        assert_eq!(deliveries(&mut store).len(), 1, "one continue only");
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "one info item: {told:?}");
        assert!(told[0].state == MsgState::Info && !told[0].urgent && told[0].body.contains("restarted on"));
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
        assert!(crate::restart::pending(&mut store, "p:heat").unwrap().is_some());
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
        store.write(|tx| {
            let op = tx.insert_op(&NewOp {
                project: "p".into(), seat: "p:heat".into(), generation: 1,
                kind: OpKind::Doorbell, task: None, attempt: None,
                ref_msg: None, plan_version: None,
                payload: serde_json::json!({ "text": "gear", "covers": ["M-404"] }),
                created: T0,
            })?;
            tx.set_op_state("p", &op.id, OpState::Executing, None, T0)?;
            tx.set_op_state("p", &op.id, OpState::Submitted, None, T0)
        }).unwrap();
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        herdr.exit_agent(PANE);
        herdr.refuse_next_start = Some("agent_pane_busy".into());
        tick_quota(&mut store, &mut herdr, false, T0 + 30);
        assert_eq!(lane_seat(&mut store).state, SeatState::Starting);
        assert!(deliveries(&mut store).is_empty());
        // A later tick resumes the durable plan with the same generation.
        tick_quota(&mut store, &mut herdr, false, T0 + 40);
        assert_eq!(lane_seat(&mut store).generation, 2);
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
        assert_eq!(deliveries(&mut store).len(), 1);
        assert_eq!(exits(&herdr).len(), 1);
        assert_eq!(store.read(|tx| tx.get_op("p", "O-1")).unwrap().unwrap().state, OpState::Confirmed);
        let (_, flagged) = crate::ops::confirm_pending(&mut store, "p", 1, T0 + 50).unwrap();
        assert!(flagged.is_empty(), "the departed generation cannot flag the replacement");
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
        assert!(deliveries(&mut store).is_empty(), "no Continue before model confirmation");
        // Onboarding must not take over this pending restart's model switch.
        assert!(crate::onboard::onboard_queued(&mut store, &mut herdr, &cfg(), reset + 11).unwrap().is_empty());
        herdr.set_screen(PANE, "Build auto · MiniMax-M3.1-Flash MiniMax · high");
        tick_quota(&mut store, &mut herdr, false, reset + 20);
        assert_eq!(lane_seat(&mut store).state, SeatState::Active);
        assert_eq!(lane_seat(&mut store).generation, 3);
        assert_eq!(deliveries(&mut store).len(), 1, "one Continue for the new generation");
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
            ("DeepSeek V4.1 Flash".into(), "Build auto · DeepSeek V4.1 Flash · high".into()),
        ];

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        for tick in 0..5 {
            tick_quota(&mut store, &mut herdr, true, T0 + 20 + tick);
        }

        assert!(agent_starts(&herdr).is_empty(), "no restart: {:?}", agent_starts(&herdr));
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

    /// T-60: a failed model switch stops the lane, so the captain is rung — an
    /// urgent item that names the seat and the reason. Once per failure: a
    /// later tick says nothing new.
    #[test]
    fn a_failed_switch_rings_the_captain_once() {
        let mut store = world(&config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback"));
        let mut herdr = herdr_with_quota();

        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);

        // The seat is flagged: no [models.switch] labels for the next model.
        assert_eq!(lane_seat(&mut store).state, SeatState::NeedsCheck);
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 30)).unwrap();
        assert_eq!(open.len(), 1, "one urgent item: {open:?}");
        assert!(open[0].urgent, "{open:?}");
        assert!(open[0].body.contains("p:heat"), "it names the seat: {open:?}");
        assert!(open[0].body.contains("models.switch"), "it names the reason: {open:?}");

        // A later tick does not ring again.
        tick_quota(&mut store, &mut herdr, true, T0 + 40);
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 50)).unwrap();
        assert_eq!(open.len(), 1, "the failure was told once: {open:?}");
    }

    #[test]
    fn removing_labels_during_a_switch_rings_once_and_clears_the_plan() {
        let mut text = config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback");
        let without_labels = text.clone();
        text.push_str("\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n");
        let mut store = world(&text);
        let mut herdr = herdr_with_quota();
        tick_quota(&mut store, &mut herdr, true, T0 + 10);
        tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(crate::switch::pending(&mut store, "p:heat").unwrap().is_some());
        store.write(|tx| {
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = without_labels;
            tx.update_project(&project)
        }).unwrap();
        for tick in 0..4 {
            tick_quota(&mut store, &mut herdr, false, T0 + 30 + tick);
        }
        assert_eq!(lane_seat(&mut store).state, SeatState::NeedsCheck);
        assert!(crate::switch::pending(&mut store, "p:heat").unwrap().is_none());
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].urgent && told[0].body.contains("p:heat") && told[0].body.contains("no [models.switch] labels any more"));
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
        assert!(crate::restart::pending(&mut store, "p:heat").unwrap().is_none());
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].urgent && told[0].body.contains("p:heat") && told[0].body.contains("did not finish"));
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
        store.write(|tx| tx.set_seat_state(&seat.id, SeatState::Active, None, T0 + 4)).unwrap();
        flag_and_alert(&mut store, "p", &seat, "another switch failed", T0 + 5).unwrap();
        assert_eq!(captain_messages(&mut store).len(), 2);
    }

    /// T-60: a switch that never confirms (the timeout in switch.rs) stops the
    /// lane too, and rings the captain once.
    #[test]
    fn a_switch_that_never_confirms_rings_the_captain_once() {
        let mut text = config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback");
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
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 40)).unwrap();
        assert_eq!(open.len(), 1, "one urgent item: {open:?}");
        assert!(
            open[0].urgent && open[0].body.contains("did not confirm twice"),
            "{open:?}"
        );

        // More ticks do not ring again.
        for tick in 0..5 {
            tick_quota(&mut store, &mut herdr, true, T0 + 60 + tick);
        }
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 80)).unwrap();
        assert_eq!(open.len(), 1, "the failure was told once: {open:?}");
    }

    /// T-60: a successful switch tells the captain in an **info** line — it
    /// does not ring.
    #[test]
    fn a_successful_switch_does_not_ring_the_captain() {
        let mut text = config(r#""minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-flash""#, "fallback");
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
        let report = tick_quota(&mut store, &mut herdr, true, T0 + 20);
        assert!(report.events.is_empty(), "{:?}", report.events);
        // Ticks 3..6: /model, the filter, then the bar check.
        for tick in 0..4 {
            tick_quota(&mut store, &mut herdr, true, T0 + 30 + tick);
        }

        // The switch worked: the lane runs on the new model.
        assert_eq!(deliveries(&mut store).len(), 1, "one Continue");
        // No urgent item: a successful switch is an information line.
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 40)).unwrap();
        assert!(open.is_empty(), "a successful switch does not ring: {open:?}");
        let told = captain_messages(&mut store);
        assert_eq!(told.len(), 1, "one info item: {told:?}");
        assert!(
            told.iter().all(|m| m.state == MsgState::Info && !m.urgent && m.body.contains("switched to")),
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
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", reset + 20)).unwrap();
        // The park before it is an urgent item of its own; the failed model
        // switch is the one that names the missing labels.
        assert!(
            open.iter().any(|m| m.urgent && m.body.contains("models.switch")),
            "{open:?}"
        );
        assert_eq!(open.iter().filter(|m| m.body.contains("models.switch")).count(), 1);
        let failure = open.iter().find(|m| m.body.contains("models.switch")).unwrap();
        assert_eq!(failure.sender_gen, seat.generation);
        assert!(failure.body.contains("p:heat"));
        let told = open.len();

        // More ticks do not ring again about the same failure.
        for tick in 0..5 {
            tick_quota(&mut store, &mut herdr, false, reset + 30 + tick);
        }
        let open = store.read(|tx| tx.actionable_for("p", "p:captain", reset + 60)).unwrap();
        assert_eq!(open.len(), told, "the failure was told once: {open:?}");
    }

    /// Every message the engine sent the captain about the lane, in thread
    /// order (the threads are named after the lane).
    fn captain_messages(store: &mut Store) -> Vec<gear_store::MessageRow> {
        let mut all = Vec::new();
        for seq in 1..=20 {
            all.extend(store.read(|tx| tx.thread_messages("p", &format!("heat:{seq}"))).unwrap());
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
                Call::PaneType { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

}
