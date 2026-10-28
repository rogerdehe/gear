//! A seat that produces nothing reaches the captain (T-227).
//!
//! Seen on another project: a reviewer whose model id the CLI refused
//! answered its first turn with an error and sat idle for good. The home page
//! said "temps 1 running" and the only trace was an op `failed` in the engine
//! log. A seat that holds work and has produced nothing for ten minutes is
//! **silent**, and the captain hears about it once per round, on the thread
//! `silent:<seat>`.
//!
//! The evidence is the agent's own records: a round owns the sessions of its
//! seat, and a session with usage records inside the round's window means the
//! seat produced something. When the records cannot be read at all — the round
//! has no session yet, or the seat's profile reads no usage log — the seat's
//! own words decide: no message from the seat since the round began.

use std::collections::BTreeMap;

use gear_herdr::Profile;
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{NewMessage, RoundRow, SeatRow, Store};

use crate::observe::SeatObs;

/// A round must be this old before its seat counts as silent (T-227).
const SILENT_AFTER_S: i64 = 600;

/// The mark of the alert of one round: one alert per round. A new round of
/// the same seat (a rework, a second review) has its own mark and may alert
/// again.
fn mark(seat: &str, task: &str, attempt: i64, role: &str) -> String {
    format!("engine.silent.{seat}.{task}.{attempt}.{role}")
}

/// The seats of a project that produced nothing, and now say so to the
/// captain. The seat ids are returned, so the tick report can flag them.
pub fn tick(
    store: &mut Store,
    project: &str,
    observed: &[SeatObs],
    profiles: &BTreeMap<String, Profile>,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut flagged = Vec::new();
    for seat in store.read(|tx| tx.seats_of_project(project))? {
        if !matches!(seat.role, SeatRole::Lane | SeatRole::Temp) {
            continue;
        }
        let Some(round) = store.read(|tx| tx.open_round_of_seat(&seat.id))? else {
            continue;
        };
        // A temp seat answers only for the work it holds now (T-289): the
        // round of a task it no longer holds, or of a seat that is no longer
        // Active, is not this seat's silence. A lane is checked as before.
        if seat.role == SeatRole::Temp
            && (seat.state != SeatState::Active
                || seat.task_id.as_deref() != Some(round.task.as_str()))
        {
            continue;
        }
        // The seat holds no live work: its attempt is over (cancelled,
        // settled). A seat that waits for the next task is not silent.
        let attempt = store.read(|tx| tx.get_attempt(project, &round.task, round.attempt))?;
        if attempt.is_none_or(|attempt| attempt.state.is_terminal()) {
            continue;
        }
        if now.saturating_sub(round.start_at) < SILENT_AFTER_S {
            continue;
        }
        // The seat's agent is not sitting idle (it works, or it is gone, or it
        // waits on a dialog): it is not silent, and the paths that own those
        // cases have their own alarm.
        let Some(obs) = observed.iter().find(|obs| obs.seat.id == seat.id) else {
            continue;
        };
        if !obs.idle {
            continue;
        }
        let key = mark(&seat.id, &round.task, round.attempt, &round.role);
        if store.read(|tx| tx.get_meta(&key))?.is_some_and(|value| !value.is_empty()) {
            continue; // this round was already reported
        }
        if !silent(store, project, &seat, &round, profiles, now)? {
            continue;
        }
        let minutes = now.saturating_sub(round.start_at) / 60;
        // The alert never asks for a hand exit (T-289): the engine restarts a
        // seat whose agent exits, and a hand exit can leave the old session
        // running beside the new one.
        let body = format!(
            "{} has produced nothing for {minutes} min on {} ({} round, model {}, generation {}): its pane may show an error such as a refused model id. Look at the pane and fix the config; do not exit the agent by hand — the engine restarts a seat whose agent exits, and a hand exit can leave its old session running beside the new one.",
            seat.name,
            round.task,
            round.role,
            round.model.as_deref().unwrap_or("unknown"),
            seat.generation,
        );
        let Some(captain) = store.read(|tx| tx.captain_of(project))? else {
            continue;
        };
        store.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.to_string(),
                thread: format!("silent:{}", seat.id),
                task: Some(round.task.clone()),
                attempt: Some(round.attempt),
                sender: "engine".into(),
                sender_gen: 0,
                recipient: captain.id.clone(),
                kind: MsgKind::System,
                urgent: true,
                body: body.clone(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })?;
            tx.set_meta(&key, &now.to_string())
        })?;
        flagged.push(seat.id.clone());
    }
    Ok(flagged)
}

/// Has this seat produced nothing since its round began?
///
/// The records of its agent decide it: none of the round's sessions has a
/// usage record inside the round's window means the seat produced nothing.
/// When the records cannot be read at all — the round has no session yet, or
/// the seat's profile reads no usage log — the seat's own words decide: no
/// message from the seat since the round began.
fn silent(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    round: &RoundRow,
    profiles: &BTreeMap<String, Profile>,
    now: Ts,
) -> GearResult<bool> {
    let log = usage_log(store, seat, profiles)?;
    if let Some(log) = log.filter(|_| !round.sessions.is_empty()) {
        let produced = round.sessions.iter().any(|session| {
            gear_usage::usage(log, session, round.start_at, now).is_some_and(|usage| usage.records > 0)
        });
        return Ok(!produced);
    }
    Ok(!spoke_since(store, project, seat, round)?)
}

/// The usage log of the seat's profile, as the session runner reads it: the
/// profile of the seat, else the profile of its kind.
fn usage_log(
    store: &mut Store,
    seat: &SeatRow,
    profiles: &BTreeMap<String, Profile>,
) -> GearResult<Option<gear_usage::UsageLog>> {
    let profile = match profiles.get(&seat.profile) {
        Some(profile) => Some(profile.clone()),
        None => {
            let kind = store
                .read(|tx| tx.get_binding(&seat.id, seat.generation))?
                .map(|binding| binding.kind);
            kind.and_then(|kind| profiles.values().find(|profile| profile.kind == kind).cloned())
        }
    };
    Ok(profile
        .and_then(|profile| profile.usage_log.clone())
        .and_then(|name| gear_usage::UsageLog::from_name(&name)))
}

/// Has the seat said anything since the round began? Any message of the seat
/// counts: a progress answer, a question, a blocked report.
fn spoke_since(
    store: &mut Store,
    project: &str,
    seat: &SeatRow,
    round: &RoundRow,
) -> GearResult<bool> {
    let messages = store.read(|tx| tx.messages_from_since(project, &seat.id, round.start_at))?;
    Ok(!messages.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::Presence;
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;
    use gear_model::SeatState;
    use gear_store::{testing, NewRound, RoundKey};
    use std::collections::BTreeSet;

    const NOW: Ts = 1_900_000_000;
    const SEAT: &str = "p:review-1";
    const CAPTAIN: &str = "p:captain";

    /// The world the tests share: a project with a captain, a reviewer seat
    /// with a round, and the pi profile (its usage log reads a file, so a
    /// test can write the records it wants).
    struct World {
        store: Store,
        dir: tempfile::TempDir,
        profiles: BTreeMap<String, Profile>,
    }

    fn the_world() -> World {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", NOW)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p9", "pi", NOW)?;
                let seat = testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, "w1:p2", "pi", NOW)?;
                tx.set_seat_task(&seat.id, "T-1")?;
                tx.insert_task(&gear_store::TaskRow {
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: gear_model::TaskState::Verifying,
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
                    estimate_min: Some(30),
                    issue: None,
                    updated: NOW,
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                })?;
                // The attempt under review: it is live, so its reviewer holds
                // work.
                for n in 1..=3 {
                    tx.insert_attempt(&gear_store::AttemptRow {
                        project: "p".into(),
                        task: "T-1".into(),
                        n,
                        seat: "p:heat".into(),
                        generation: 1,
                        contract_version: 1,
                        adopted_version: 1,
                        pending_update: None,
                        state: gear_model::AttemptState::Submitted,
                        candidate: Some("abc1234".into()),
                        report: None,
                        safe_point: None,
                        started: NOW - 30 * 60,
                        updated: NOW,
                        session_ref: None,
                        session_cli: Some("pi".into()),
                        session_action: None,
                        sent_at: Some(NOW - 30 * 60),
                    })?;
                }
                Ok(())
            })
            .unwrap();
        let mut profiles = BTreeMap::new();
        profiles.insert("pi".to_string(), gear_herdr::builtin("pi").unwrap());
        World {
            store,
            dir: tempfile::tempdir().unwrap(),
            profiles,
        }
    }

    fn round_key(attempt: i64) -> RoundKey {
        RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt,
            role: "review".into(),
        }
    }

    /// A review round of the reviewer, started `start_at`, with the session
    /// `session` (none: a round whose agent never got a session).
    fn insert_round(world: &mut World, attempt: i64, start_at: Ts, session: Option<&str>) {
        world
            .store
            .write(|tx| {
                tx.insert_round(&NewRound {
                    key: round_key(attempt),
                    seat: SEAT.into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: Some("minimax-m3".into()),
                    start_at,
                })?;
                if let Some(session) = session {
                    tx.add_round_session(&round_key(attempt), session)?;
                }
                Ok(())
            })
            .unwrap();
    }

    /// A usage log of the pi kind: one assistant record with usage, or none.
    fn usage_log(world: &World, records: usize) -> String {
        let path = world.dir.path().join(format!("session-{records}.jsonl"));
        let mut text = String::new();
        for index in 0..records {
            text.push_str(&format!(
                "{{\"type\":\"message\",\"timestamp\":{},\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":10,\"output\":5}}}}}}\n",
                NOW - 300 + index as i64
            ));
        }
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().to_string()
    }

    /// The reviewer seat as the engine sees it: present and idle.
    fn idle() -> SeatObs {
        SeatObs {
            seat: SeatRow {
                id: SEAT.into(),
                project: "p".into(),
                role: SeatRole::Temp,
                name: "review-1".into(),
                profile: "pi".into(),
                generation: 1,
                state: SeatState::Active,
                state_since: NOW,
                state_reason: None,
                task_id: Some("T-1".into()),
            },
            presence: Presence::Present,
            status: Some(AgentStatus::Idle),
            flags: BTreeSet::new(),
            idle: true,
            screen: None,
            session: None,
        }
    }

    /// The reviewer seat with an agent at work.
    fn working() -> SeatObs {
        SeatObs {
            status: Some(AgentStatus::Working),
            idle: false,
            ..idle()
        }
    }

    fn alerts(world: &mut World) -> Vec<gear_store::MessageRow> {
        world
            .store
            .read(|tx| tx.thread_messages("p", &format!("silent:{SEAT}")))
            .unwrap()
    }

    #[test]
    fn a_silent_reviewer_alerts_the_captain_once() {
        let mut world = the_world();
        let session = usage_log(&world, 0);
        insert_round(&mut world, 1, NOW - 11 * 60, Some(&session));

        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert_eq!(flagged, vec![SEAT.to_string()]);

        let messages = alerts(&mut world);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].recipient, CAPTAIN);
        assert!(messages[0].urgent);
        assert_eq!(messages[0].task.as_deref(), Some("T-1"));
        assert_eq!(messages[0].attempt, Some(1));
        assert!(messages[0].body.contains("review-1 has produced nothing for 11 min"));
        assert!(messages[0].body.contains("(review round, model minimax-m3, generation 1)"));
        assert!(messages[0].body.contains("do not exit the agent by hand"));
        assert!(!messages[0].body.contains("stop the agent"));

        // The second tick says nothing new: the round was already reported.
        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert!(flagged.is_empty());
        assert_eq!(alerts(&mut world).len(), 1);
    }

    #[test]
    fn a_temp_seat_whose_work_moved_on_is_not_silent() {
        // The seat holds T-2 now; its open round on T-1 is a round of a task it
        // no longer holds, and its silence is not this seat's (T-289).
        let mut world = the_world();
        let session = usage_log(&world, 0);
        insert_round(&mut world, 1, NOW - 11 * 60, Some(&session));
        world
            .store
            .write(|tx| tx.set_seat_task(SEAT, "T-2"))
            .unwrap();

        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert!(flagged.is_empty(), "{flagged:?}");
        assert!(alerts(&mut world).is_empty());
    }

    #[test]
    fn a_standby_temp_seat_is_not_silent() {
        let mut world = the_world();
        let session = usage_log(&world, 0);
        insert_round(&mut world, 1, NOW - 11 * 60, Some(&session));
        world
            .store
            .write(|tx| {
                tx.set_seat_state(SEAT, SeatState::Standby, None, NOW)?;
                Ok(())
            })
            .unwrap();

        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert!(flagged.is_empty(), "{flagged:?}");
        assert!(alerts(&mut world).is_empty());
    }

    #[test]
    fn a_temp_seat_restarted_in_place_still_alerts() {
        // The round keeps the generation it started with; the seat restarted
        // (T-209) on its own task is exactly what the alert is for (T-289).
        let mut world = the_world();
        let session = usage_log(&world, 0);
        insert_round(&mut world, 1, NOW - 11 * 60, Some(&session));
        world
            .store
            .write(|tx| tx.set_seat_generation(SEAT, 2))
            .unwrap();

        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert_eq!(flagged, vec![SEAT.to_string()]);
        let messages = alerts(&mut world);
        assert_eq!(messages.len(), 1);
        // The generation named is the seat's live one (contract v2), not the
        // round's older one.
        assert!(messages[0].body.contains("generation 2"), "{}", messages[0].body);
    }

    #[test]
    fn a_reviewer_with_usage_records_is_not_silent() {
        let mut world = the_world();
        let session = usage_log(&world, 1);
        insert_round(&mut world, 1, NOW - 11 * 60, Some(&session));

        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert!(flagged.is_empty(), "{flagged:?}");
        assert!(alerts(&mut world).is_empty());
    }

    #[test]
    fn a_round_younger_than_ten_minutes_is_not_silent() {
        let mut world = the_world();
        let session = usage_log(&world, 0);
        insert_round(&mut world, 1, NOW - 9 * 60, Some(&session));

        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert!(flagged.is_empty(), "{flagged:?}");
        assert!(alerts(&mut world).is_empty());
    }

    #[test]
    fn a_working_agent_is_not_silent() {
        let mut world = the_world();
        let session = usage_log(&world, 0);
        insert_round(&mut world, 1, NOW - 11 * 60, Some(&session));

        let flagged = tick(&mut world.store, "p", &[working()], &world.profiles, NOW).unwrap();
        assert!(flagged.is_empty(), "{flagged:?}");
        assert!(alerts(&mut world).is_empty());
    }

    #[test]
    fn a_new_round_of_the_same_seat_alerts_again() {
        let mut world = the_world();
        let session = usage_log(&world, 0);
        insert_round(&mut world, 1, NOW - 11 * 60, Some(&session));
        assert_eq!(
            tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap(),
            vec![SEAT.to_string()]
        );

        // The next round of the same seat is a new round: it may alert again.
        world
            .store
            .write(|tx| tx.close_round(&round_key(1), NOW - 60))
            .unwrap();
        insert_round(&mut world, 2, NOW - 11 * 60, Some(&session));
        assert_eq!(
            tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap(),
            vec![SEAT.to_string()]
        );
        assert_eq!(alerts(&mut world).len(), 2);
    }

    #[test]
    fn a_round_without_a_session_follows_the_words_of_the_seat() {
        // No session: the seat's own messages decide.
        let mut world = the_world();
        insert_round(&mut world, 1, NOW - 11 * 60, None);
        world
            .store
            .write(|tx| {
                tx.insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "T-1:1".into(),
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    sender: SEAT.into(),
                    sender_gen: 1,
                    recipient: CAPTAIN.into(),
                    kind: MsgKind::Progress,
                    urgent: false,
                    body: "I am on it".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Info,
                    created: NOW - 300,
                })?;
                Ok(())
            })
            .unwrap();
        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert!(flagged.is_empty(), "{flagged:?}");
        assert!(alerts(&mut world).is_empty());

        // The same round without a word of the seat: silent.
        let mut world = the_world();
        insert_round(&mut world, 1, NOW - 11 * 60, None);
        let flagged = tick(&mut world.store, "p", &[idle()], &world.profiles, NOW).unwrap();
        assert_eq!(flagged, vec![SEAT.to_string()]);
        assert_eq!(alerts(&mut world).len(), 1);
    }

    /// The whole engine tick (review round 1, F1): a silent seat raises one
    /// captain alert, on its own thread. It must not also land in
    /// `report.flagged`: that path sends a second, generic stall alert.
    #[test]
    fn a_full_tick_raises_one_captain_alert_for_a_silent_seat() {
        let mut world = the_world();
        insert_round(&mut world, 1, NOW - 11 * 60, None);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named("w1:p9", "pi", AgentStatus::Working, "p-captain");
        herdr.add_agent_named("w1:p2", "pi", AgentStatus::Idle, "p-review-1");
        let mut cfg = crate::EngineCfg::default();
        cfg.profiles = world.profiles.clone();

        crate::tick(&mut world.store, &mut herdr, &cfg, NOW).unwrap();
        let messages = alerts(&mut world);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].recipient, CAPTAIN);
        assert!(messages[0].urgent);
        assert_eq!(messages[0].thread, format!("silent:{SEAT}"));
        let stall = world
            .store
            .read(|tx| tx.thread_messages("p", &format!("stall:{SEAT}")))
            .unwrap();
        assert!(stall.is_empty(), "no second, generic stall alert: {stall:?}");

        // The second tick says nothing new.
        crate::tick(&mut world.store, &mut herdr, &cfg, NOW).unwrap();
        assert_eq!(alerts(&mut world).len(), 1);
    }
}
