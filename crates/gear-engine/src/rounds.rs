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
                    // T-458: a round whose attempt still goes on does not end
                    // here. A lane that reported blocked keeps its parked
                    // attempt (still Started) and takes its next task; the
                    // parked round keeps its clock, since the wait is its
                    // time.
                    if attempt_in_progress(tx, round)? {
                        continue;
                    }
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
                let Some(session) = binding.session_ref.as_deref().filter(|s| !s.is_empty()) else {
                    continue;
                };
                let Some(round) = tx.open_round_of_seat(&seat.id)? else {
                    continue;
                };
                if round.sessions.iter().any(|owned| owned == session) {
                    continue;
                }
                add_session(tx, &key_of(&round), session, &binding.kind)?;
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
            if now < end.saturating_add(60)
                || round
                    .counted_at
                    .is_some_and(|at| round.input.is_some() || now < at.saturating_add(60))
            {
                continue;
            }
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

/// Record the owner of each session independently of the round's initial CLI.
fn session_cli_key(key: &RoundKey, session: &str) -> String {
    format!(
        "round-session-cli:{}",
        serde_json::json!([key.project, key.task, key.attempt, key.role, session])
    )
}

pub(crate) fn add_session(tx: &Tx<'_>, key: &RoundKey, session: &str, cli: &str) -> GearResult<()> {
    tx.add_round_session(key, session)?;
    tx.set_meta(&session_cli_key(key, session), cli)
}

/// Is the round's attempt still going on (T-458)? A round with an attempt
/// that occupies the seat — a parked wait included — is not ended by the next
/// round on its seat.
fn attempt_in_progress(tx: &Tx<'_>, round: &gear_store::RoundRow) -> GearResult<bool> {
    Ok(tx
        .get_attempt(&round.project, &round.task, round.attempt)?
        .is_some_and(|attempt| attempt.state.occupies_seat()))
}

/// How a round is named in the report.
fn label(round: &gear_store::RoundRow) -> String {
    format!("{}/{}/{}", round.task, round.attempt, round.role)
}

/// The open round of a seat, if it has one (the round it is working on).
pub fn open_round(tx: &Tx<'_>, seat: &str) -> GearResult<Option<gear_store::RoundRow>> {
    tx.open_round_of_seat(seat)
}

/// Count a round without writing it. Reports can use this for an open round;
/// its exclusive upper boundary is `now`. Unreadable logs remain unknown.
pub fn count_round(
    store: &mut Store,
    cfg: &EngineCfg,
    round: &gear_store::RoundRow,
    now: Ts,
) -> GearResult<RoundTokens> {
    let unknown = || RoundTokens {
        input: None,
        output: None,
        cache_read: None,
        cache_write: None,
        reasoning: None,
        cost_usd: None,
        cost_source: "none".into(),
    };
    let (seat, project) =
        store.read(|tx| Ok((tx.get_seat(&round.seat)?, tx.get_project(&round.project)?)))?;
    let (Some(seat), Some(project)) = (seat, project) else {
        return Ok(unknown());
    };
    let config = project.config_toml.parse::<toml::Value>().ok();
    let resolve = |name: &str| {
        let raw = config
            .as_ref()
            .and_then(|c| c.get("profiles"))
            .and_then(|p| p.get(name));
        let mut profile = cfg
            .profiles
            .get(name)
            .cloned()
            .or_else(|| gear_herdr::builtin(name))
            .or_else(|| raw.map(|_| gear_herdr::Profile::for_new()))?;
        if let Some(raw) = raw {
            if let Ok(patch) = raw.clone().try_into::<gear_herdr::ProfilePatch>() {
                profile = profile.patched(&patch, name);
            }
        }
        Some(profile)
    };
    if round.sessions.is_empty() {
        return Ok(unknown());
    }
    let owners = store.read(|tx| {
        round
            .sessions
            .iter()
            .map(|session| {
                Ok((
                    session.clone(),
                    tx.get_meta(&session_cli_key(&key_of(round), session))?
                        .filter(|cli| !cli.is_empty())
                        .unwrap_or_else(|| round.cli.clone()),
                ))
            })
            .collect::<GearResult<Vec<_>>>()
    })?;
    let end = round.end_at.unwrap_or(now).min(now);
    let mut prices = std::collections::BTreeMap::new();
    if let Some(table) = config
        .as_ref()
        .and_then(|c| c.get("prices"))
        .and_then(toml::Value::as_table)
    {
        for (model, price) in table {
            let rate = |key: &str| {
                price
                    .get(key)
                    .and_then(|n| n.as_float().or_else(|| n.as_integer().map(|i| i as f64)))
            };
            if let (Some(input), Some(output), Some(cache_read), Some(cache_write)) = (
                rate("input"),
                rate("output"),
                rate("cache_read"),
                rate("cache_write"),
            ) {
                prices.insert(
                    model.clone(),
                    gear_usage::Price {
                        input,
                        output,
                        cache_read,
                        cache_write,
                    },
                );
            }
        }
    }
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (session, cli) in owners {
        groups.entry(cli).or_default().push(session);
    }
    let mut total = RoundTokens {
        input: Some(0),
        output: Some(0),
        cache_read: Some(0),
        cache_write: Some(0),
        reasoning: Some(0),
        cost_usd: None,
        cost_source: "none".into(),
    };
    let (mut cost, mut any_cost, mut cost_known, mut priced) = (0.0, false, true, false);
    for (cli, sessions) in groups {
        // Keep custom profile behavior for the current CLI; use the original
        // CLI's profile for older sessions after a cross-CLI restart.
        let profile = resolve(&seat.profile)
            .filter(|p| p.kind == cli)
            .or_else(|| resolve(&cli).filter(|p| p.kind == cli));
        let Some(log) = profile
            .as_ref()
            .and_then(|p| p.usage_log.as_deref())
            .and_then(gear_usage::UsageLog::from_name)
        else {
            return Ok(unknown());
        };
        let model = if cli == round.cli {
            round.model.as_deref()
        } else {
            None
        };
        let Some(result) =
            gear_usage::round_usage(log, &sessions, round.start_at, end, model, &prices)
        else {
            return Ok(unknown());
        };
        let u = result.usage;
        *total.input.as_mut().unwrap() += u.input;
        *total.output.as_mut().unwrap() += u.output;
        *total.cache_read.as_mut().unwrap() += u.cache_read;
        *total.cache_write.as_mut().unwrap() += u.cache_write;
        *total.reasoning.as_mut().unwrap() += u.reasoning;
        if let Some(value) = u.cost_usd {
            cost += value;
            any_cost = true;
            priced |= result.cost_source == "prices";
        } else if u.records > 0 {
            cost_known = false;
        }
    }
    if any_cost && cost_known {
        total.cost_usd = Some(cost);
        total.cost_source = if priced { "prices" } else { "agent" }.into();
    }
    Ok(total)
}
