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
//!   released;
//! - T-515: a direction check's round ends when the check ends (its job is no
//!   longer running), and an open round of a seat whose generation moved on
//!   and which holds no job for that round's task ends when the generation
//!   changed — a reviewer that handed its direction result in and went Standby
//!   no longer counts hours it did not work.

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
                    close_with_clock(tx, round, next.start_at)?;
                    report.closed.push(label(round));
                }
            }
            // 1a. T-515: a direction check's round ends when the check ends. A
            //     reviewer that sends its direction result and goes Standby
            //     keeps no open round, so the overrun watch stops counting it
            //     (M-5705: T-407's round stayed open 91 minutes after its
            //     result). The round ends at the time of the result, else now.
            //     The rounds are read again (F2 of the review of attempt 1):
            //     step 1 may have closed one already, and closing it a second
            //     time with `now` would push its end past the next round's
            //     start.
            for round in &tx.rounds_of_project(&project)? {
                if round.end_at.is_some() || round.role != "direction" {
                    continue;
                }
                let job = tx
                    .get_meta(&crate::review::direction_key(
                        &round.project,
                        &round.task,
                        round.attempt,
                    ))?
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
                // A job that still runs (or is starting) holds its round open.
                if job.as_ref().is_some_and(|job| {
                    matches!(job["state"].as_str(), Some("running" | "starting"))
                }) {
                    continue;
                }
                let at = direction_result_at(tx, round)?.unwrap_or(now);
                close_with_clock(tx, round, at)?;
                report.closed.push(label(round));
            }
            // 1b. T-458: a parked attempt whose wait has ended goes on in a
            //     new window of its round — the resumed work is its own time,
            //     and the seat has one live round. Only when the seat works on
            //     no other attempt: a lane still busy with the next task
            //     reopens the parked round when that task is handed in.
            let mut live: Vec<&gear_store::RoundRow> = Vec::new();
            for round in &rounds {
                if attempt_in_progress(tx, round)? {
                    live.push(round);
                }
            }
            for round in &tx.rounds_of_project(&project)? {
                if round.end_at.is_none() || !attempt_in_progress(tx, round)? {
                    continue;
                }
                // T-515 (F1 of the review of attempt 1): a direction round is
                // never a parked attempt to resume. A check whose result is in
                // ends in step 1a and stays closed; reopening it would ring the
                // captain about the check it already answered.
                // T-515 (F1 of the review of attempt 1): a direction round is
                // never a parked attempt to resume. A check whose result is in
                // ends in step 1a and stays closed; reopening it would ring the
                // captain about the check it already answered.
                if round.role == "direction" {
                    continue;
                }
                let waiting = tx
                    .get_task(&round.project, &round.task)?
                    .is_some_and(|task| task.wait_on.is_some());
                if waiting {
                    continue;
                }
                let same = |other: &gear_store::RoundRow| {
                    other.task == round.task
                        && other.attempt == round.attempt
                        && other.role == round.role
                };
                let busy_elsewhere = live.iter().any(|other| {
                    other.seat == round.seat && other.end_at.is_none() && !same(other)
                });
                if busy_elsewhere {
                    continue;
                }
                for other in &rounds {
                    if other.seat == round.seat && other.end_at.is_none() && !same(other) {
                        close_with_clock(tx, other, now)?;
                        report.closed.push(label(other));
                    }
                }
                tx.reopen_round(&key_of(round), now)?;
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
                    close_with_clock(tx, &round, seat.state_since)?;
                    report.closed.push(label(&round));
                }
            }
            // 3b. T-515: a round whose seat has moved on to another generation
            //     and holds no job for that round's task ends when the
            //     generation changed: the work of the old generation is over.
            //     A round whose attempt is still in progress belongs to the
            //     seat whatever its generation (a restart inside a task), and
            //     keeps running.
            let moved = tx.rounds_of_project(&project)?;
            for round in &moved {
                if round.end_at.is_some() || attempt_in_progress(tx, round)? {
                    continue;
                }
                let Some(seat) = tx.get_seat(&round.seat)? else {
                    continue;
                };
                if seat.generation <= round.generation {
                    continue;
                }
                if holds_job_for(tx, &seat, &round.task)? {
                    continue;
                }
                let at = tx
                    .get_binding(&seat.id, seat.generation)?
                    .map(|binding| binding.created)
                    .unwrap_or(now)
                    .max(round.start_at);
                close_with_clock(tx, round, at)?;
                report.closed.push(label(round));
            }
            Ok(())
        })?;
        // T-525: the project's config shapes the profiles and prices a round's
        // usage is read with; it is parsed once for the tick, not once per
        // round. On a store full of rounds the reader cannot find, that parse
        // ran nine times a second.
        let config = store
            .read(|tx| tx.get_project(&project))?
            .map(|project| project.config_toml)
            .and_then(|raw| raw.parse::<toml::Value>().ok());
        let rounds = store.read(|tx| tx.rounds_of_project(&project))?;
        for round in rounds {
            let Some(end) = round.end_at else { continue };
            // T-525: a round whose usage was found is counted once and kept.
            if round.counted_at.is_some_and(|_| round.input.is_some()) {
                continue;
            }
            // T-525: while the usage stays unknown the round is counted on a
            // back-off — 1, 5 and 30 minutes after its end — and then never
            // again. It stays unknown (never 0): a session the reader cannot
            // find must not read as a round of no tokens.
            let Some(due) = unknown_count_due(end, round.counted_at) else {
                continue;
            };
            if now < due {
                continue;
            }
            let tokens = count_round_with(store, cfg, config.as_ref(), &round, now)?;
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

/// End a round at `at` with its clock reaching that moment (T-458): the
/// overrun pass accounts a round each tick while it is open, and a round
/// closed before that pass would lose the time since the last tick — a lane
/// that reports blocked and takes its next task parks its attempt, and the
/// parked round's clock must stop at the next round's start, not at the last
/// tick.
pub(crate) fn close_with_clock(
    tx: &Tx<'_>,
    round: &gear_store::RoundRow,
    at: Ts,
) -> GearResult<()> {
    crate::overrun::process(tx, round, at)?;
    tx.close_round(&key_of(round), at)
}

/// The time of the direction result a round's check handed in (T-515): the
/// result the round's seat sent for its attempt, else the last direction result
/// of that attempt. `None` when the check ended with no result (a closed job).
fn direction_result_at(tx: &Tx<'_>, round: &gear_store::RoundRow) -> GearResult<Option<Ts>> {
    let results = tx.direction_results_of_attempt(&round.project, &round.task, round.attempt)?;
    Ok(results
        .iter()
        .filter(|result| result.sender == round.seat)
        .map(|result| result.created)
        .max()
        .or_else(|| results.iter().map(|result| result.created).max()))
}

/// Whether the seat holds a job for `task` (T-515): the task it works, a review
/// it was given, or a direction check of that task that still runs. A seat that
/// holds no job for a round's task has finished that round's work.
fn holds_job_for(tx: &Tx<'_>, seat: &gear_store::SeatRow, task: &str) -> GearResult<bool> {
    if seat.task_id.as_deref() == Some(task) {
        return Ok(true);
    }
    let pending = tx
        .get_meta(&crate::review::pending_review_key(&seat.id))?
        .filter(|value| !value.is_empty());
    if pending.as_deref() == Some(task) {
        return Ok(true);
    }
    // A direction job of that task that still runs is the seat's job too.
    for round in tx.rounds_of_project(&seat.project)? {
        if round.task != task || round.end_at.is_some() {
            continue;
        }
        let job = tx
            .get_meta(&crate::review::direction_key(
                &round.project,
                &round.task,
                round.attempt,
            ))?
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
        if job.as_ref().is_some_and(|job| {
            job["seat"] == seat.id && matches!(job["state"].as_str(), Some("running" | "starting"))
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// How a round is named in the report.
fn label(round: &gear_store::RoundRow) -> String {
    format!("{}/{}/{}", round.task, round.attempt, round.role)
}

/// The open round of a seat, if it has one (the round it is working on).
pub fn open_round(tx: &Tx<'_>, seat: &str) -> GearResult<Option<gear_store::RoundRow>> {
    tx.open_round_of_seat(seat)
}

/// How long after its end an ended round is counted again while its usage is
/// unknown (T-525): 1, 5 and 30 minutes. After the last the round is left
/// alone — it keeps its unknown usage instead of costing a walk of the agent's
/// session files on every tick for the rest of the project's life.
const UNKNOWN_COUNT_AFTER_END_S: [i64; 3] = [60, 300, 1800];

/// When an ended round whose usage is still unknown is due its next count, or
/// `None` when the back-off is spent (T-525). The count happens at the first
/// of the three marks that its last count did not already pass.
fn unknown_count_due(end: Ts, counted_at: Option<Ts>) -> Option<Ts> {
    let marks = UNKNOWN_COUNT_AFTER_END_S.map(|delay| end.saturating_add(delay));
    match counted_at {
        None => marks.first().copied(),
        Some(at) => marks.into_iter().find(|due| *due > at),
    }
}

/// Count a round without writing it. Reports can use this for an open round;
/// its exclusive upper boundary is `now`. Unreadable logs remain unknown.
pub fn count_round(
    store: &mut Store,
    cfg: &EngineCfg,
    round: &gear_store::RoundRow,
    now: Ts,
) -> GearResult<RoundTokens> {
    // The project's config carries the profiles and prices of its rounds; a
    // caller counting several rounds of one project parses it once (T-525).
    let config = store
        .read(|tx| tx.get_project(&round.project))?
        .map(|project| project.config_toml)
        .and_then(|raw| raw.parse::<toml::Value>().ok());
    count_round_with(store, cfg, config.as_ref(), round, now)
}

/// `count_round` with the project's config already parsed.
fn count_round_with(
    store: &mut Store,
    cfg: &EngineCfg,
    config: Option<&toml::Value>,
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
    let seat = store.read(|tx| tx.get_seat(&round.seat))?;
    let Some(seat) = seat else {
        return Ok(unknown());
    };
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
    // The project's `[prices]` override the built-in table (T-484); the
    // built-in entries are read by gear-usage itself.
    let mut prices = std::collections::BTreeMap::new();
    if let Some(table) = config
        .and_then(|c| c.get("prices"))
        .and_then(toml::Value::as_table)
    {
        for (model, price) in table {
            if let Ok(price) = price.clone().try_into::<gear_app::config::Price>() {
                if let Some(price) = price.to_usage() {
                    prices.insert(model.clone(), price);
                }
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
