//! Rounds: the ownership of a seat's agent records (design §4.1).
//!
//! A round owns every agent record on its seat from its `start_at` until its
//! `end_at`: the start of the next round on the same seat, or the seat's
//! release — whichever comes first. There is no "between rounds" bucket: what
//! a seat does after a hand-in belongs to the round it handed in.
//!
//! The tick of this module keeps that bookkeeping true:
//!
//! - the session the seat's binding carries now joins the open round, so a
//!   `/new`, a restart or a resume inside the round is counted in it (once —
//!   the round keeps the list of the sessions it owns);
//! - a round ends when the next round on its seat starts, or when the seat is
//!   released.

use gear_herdr::HerdrAct;
use gear_model::{GearResult, SeatState, Ts};
use gear_store::{RoundKey, RoundTokens, Store, Tx};

use crate::EngineCfg;

/// What one tick of the rounds bookkeeping did (for the tick report).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoundReport {
    /// The rounds that ended this tick (`task/attempt`).
    pub closed: Vec<String>,
    /// The rounds that learned a new session this tick (`task/attempt`).
    pub sessions: Vec<String>,
}

/// The rounds bookkeeping of every project (design §4.1).
///
pub fn tick(
    store: &mut Store,
    _herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    now: Ts,
) -> GearResult<RoundReport> {
    let mut report = RoundReport::default();
    let projects = store.read(|tx| tx.project_ids())?;
    for project in projects {
        store.write(|tx| {
            let rounds = tx.rounds_of_project(&project)?;
            // 1. §4.1: a round ends when the next round on the same seat
            //    starts. The seat's open round is the newest one, so every
            //    other open round of that seat ends here.
            for round in &rounds {
                if round.end_at.is_some() {
                    continue;
                }
                let next = rounds
                    .iter()
                    .filter(|other| other.seat == round.seat && other.start_at > round.start_at)
                    .min_by_key(|other| other.start_at);
                if let Some(next) = next {
                    tx.close_round(&key_of(round), next.start_at)?;
                    report.closed.push(label(round));
                }
            }
            // 2. The session the seat carries now belongs to its open round —
            //    before the release below closes it, so the round keeps the
            //    session of its last work.
            for seat in tx.seats_of_project(&project)? {
                let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else {
                    continue;
                };
                let Some(session) = binding.session_ref.as_deref().filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let Some(round) = tx.open_round_of_seat(&seat.id)? else {
                    continue;
                };
                if round.sessions.iter().any(|owned| owned == session) {
                    continue;
                }
                tx.add_round_session(&key_of(&round), session)?;
                report.sessions.push(label(&round));
            }
            // 3. §4.1: or when the seat is released — the agent is gone and
            //    its records belong to the round it handed in. The round keeps
            //    the release time, not the time of this tick.
            for round in tx.rounds_of_project(&project)? {
                if round.end_at.is_some() {
                    continue;
                }
                let Some(seat) = tx.get_seat(&round.seat)? else {
                    continue;
                };
                if matches!(seat.state, SeatState::Released | SeatState::Retired) {
                    tx.close_round(&key_of(&round), seat.state_since)?;
                    report.closed.push(label(&round));
                }
            }
            Ok(())
        })?;
        let rounds = store.read(|tx| tx.rounds_of_project(&project))?;
        for round in rounds {
            let Some(end) = round.end_at else { continue };
            if now < end.saturating_add(60) || round.counted_at.is_some_and(|at| round.input.is_some() || now < at.saturating_add(60)) { continue }
            let tokens = count_round(store, cfg, &round, now)?;
            store.write(|tx| {
                tx.set_round_tokens(&key_of(&round), &tokens, now)?;
                // The overrun pass owns active-time and pause accounting.
                tx.set_round_active(&key_of(&round), round.active_s)
            })?;
        }
    }
    Ok(report)
}

/// The key of a round (the store's identity of one round).
fn key_of(round: &gear_store::RoundRow) -> RoundKey {
    RoundKey {
        project: round.project.clone(),
        task: round.task.clone(),
        attempt: round.attempt,
        role: round.role.clone(),
    }
}

/// How a round is named in the report.
fn label(round: &gear_store::RoundRow) -> String {
    format!("{}/{}/{}", round.task, round.attempt, round.role)
}

/// The open round of a seat, if it has one (the round it is working on).
pub fn open_round(
    tx: &Tx<'_>,
    seat: &str,
) -> GearResult<Option<gear_store::RoundRow>> {
    tx.open_round_of_seat(seat)
}

/// Count a round without writing it. Reports can use this for an open round;
/// its exclusive upper boundary is `now`. Unreadable logs remain unknown.
pub fn count_round(store: &mut Store, cfg: &EngineCfg, round: &gear_store::RoundRow, now: Ts) -> GearResult<RoundTokens> {
    let unknown = || RoundTokens { input:None, output:None, cache_read:None, cache_write:None, reasoning:None, cost_usd:None, cost_source:"none".into() };
    let (seat, project) = store.read(|tx| Ok((tx.get_seat(&round.seat)?, tx.get_project(&round.project)?)))?;
    let (Some(seat), Some(project)) = (seat, project) else { return Ok(unknown()) };
    let config = project.config_toml.parse::<toml::Value>().ok();
    let resolve = |name: &str| {
        let raw = config.as_ref().and_then(|c| c.get("profiles")).and_then(|p| p.get(name));
        let mut profile = cfg.profiles.get(name).cloned().or_else(|| gear_herdr::builtin(name))
            .or_else(|| raw.map(|_| gear_herdr::Profile::for_new()))?;
        if let Some(raw) = raw {
            if let Ok(patch) = raw.clone().try_into::<gear_herdr::ProfilePatch>() { profile = profile.patched(&patch, name); }
        }
        Some(profile)
    };
    // A seat may have switched CLI after this round ended. Its new profile
    // must not decide how the old CLI's session logs are read.
    let profile = resolve(&seat.profile).filter(|p| p.kind == round.cli)
        .or_else(|| resolve(&round.cli).filter(|p| p.kind == round.cli));
    let Some(log) = profile.as_ref().and_then(|p|p.usage_log.as_deref()).and_then(gear_usage::UsageLog::from_name) else { return Ok(unknown()) };
    if round.sessions.is_empty() { return Ok(unknown()) }
    let end = round.end_at.unwrap_or(now).min(now);
    let mut prices = std::collections::BTreeMap::new();
    if let Some(table) = config.as_ref().and_then(|c|c.get("prices")).and_then(toml::Value::as_table) {
        for (model, price) in table {
            let rate = |key: &str| price.get(key).and_then(|n| n.as_float().or_else(|| n.as_integer().map(|i|i as f64)));
            if let (Some(input),Some(output),Some(cache_read),Some(cache_write)) = (rate("input"),rate("output"),rate("cache_read"),rate("cache_write")) {
                prices.insert(model.clone(), gear_usage::Price {input, output, cache_read, cache_write});
            }
        }
    }
    let Some(result) = gear_usage::round_usage(log, &round.sessions, round.start_at, end, round.model.as_deref(), &prices) else { return Ok(unknown()) };
    let u = result.usage;
    Ok(RoundTokens { input:Some(u.input), output:Some(u.output), cache_read:Some(u.cache_read), cache_write:Some(u.cache_write), reasoning:Some(u.reasoning), cost_usd:u.cost_usd, cost_source:result.cost_source.into() })
}
