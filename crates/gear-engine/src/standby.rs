//! The fleet stands by while a finished voyage waits for the user's landing
//! word.
//!
//! When every task of the voyage is done and merged, the engine opens the
//! landing order ("Voyage M is ready to land") and only the user's acceptance
//! is left — which may take hours. Lanes and reviewers with no work would sit
//! in their panes all that time. While the landing order is open and the
//! voyage has no unfinished task:
//!
//! - every lane without an unfinished task is parked with a wait of kind
//!   `standby` that has no deadline, and its agent is released as
//!   `on_wait = "release"` does ([`crate::quota::release_for_wait`]): its
//!   pane, worktree and branch stay;
//! - every reviewer without a job is released; a later review starts one
//!   again as usual;
//! - the captain is told once, and stays: aftercare is the captain's.
//!
//! A lane comes back by itself in the same pane as soon
//! as it has an unfinished task again (the captain planned a fix), or when the
//! stand-by ends without a landing (the order was answered `keep course`).
//! The landing itself retires the lanes as before.

use std::collections::BTreeMap;

use gear_herdr::{HerdrAct, Profile};
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{NewMessage, Store};

use crate::EngineCfg;

/// The kind of the wait a lane standing by is parked with.
pub(crate) const STANDBY: &str = "standby";

/// The voyage standing by and its open landing order, or `None`.
fn standing_by(store: &mut Store, project: &str) -> GearResult<Option<(String, String)>> {
    store.read(|tx| {
        let milestone = gear_app::plan::current_voyage(tx, project)?;
        if milestone.is_empty() {
            return Ok(None);
        }
        let Some(order) = tx
            .get_meta(&format!("station:{project}:{milestone}:land"))?
            .filter(|id| !id.is_empty())
        else {
            return Ok(None);
        };
        let open = tx
            .thread_messages(project, &format!("station:{milestone}"))?
            .into_iter()
            .any(|message| {
                message.id == order && matches!(message.state, MsgState::Open | MsgState::Waiting)
            });
        if !open {
            return Ok(None);
        }
        let unfinished = tx.tasks_of_project(project)?.into_iter().any(|task| {
            task.milestone.as_deref() == Some(milestone.as_str()) && !task.state.is_terminal()
        });
        if unfinished {
            return Ok(None);
        }
        Ok(Some((milestone, order)))
    })
}

/// Whether the lane has an unfinished task of any voyage.
fn lane_has_work(store: &mut Store, project: &str, lane: &str) -> GearResult<bool> {
    store.read(|tx| {
        Ok(tx
            .tasks_of_project(project)?
            .into_iter()
            .any(|task| task.lane.as_deref() == Some(lane) && !task.state.is_terminal()))
    })
}

/// The key of the captain's line for one landing order.
fn told_key(project: &str) -> String {
    format!("engine.standby_told.{project}")
}

/// One tick of the stand-by of `project`: one line per park, release or
/// comeback, for the engine log.
pub fn tick(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &BTreeMap<String, Profile>,
    project: &str,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut events = Vec::new();
    let Ok(config) = gear_app::config::latest(store, project) else {
        return Ok(events);
    };
    let standing = standing_by(store, project)?;
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats.iter().filter(|seat| seat.role == SeatRole::Lane) {
        let has_work = lane_has_work(store, project, &seat.name)?;
        let wait = crate::quota::load_wait(store, &seat.id)?;
        let parked_for_standby =
            seat.state == SeatState::Parked && wait.as_ref().is_some_and(|w| w.kind == STANDBY);
        if parked_for_standby {
            if standing.is_some() && !has_work {
                continue;
            }
            let line =
                crate::quota::end_standby(store, herdr, cfg, profiles, project, &config, seat, now);
            match line {
                Ok(Some(line)) => events.push(line),
                Ok(None) => {}
                Err(error) => events.push(format!(
                    "{}: comeback from standby failed: {}",
                    seat.id, error.detail
                )),
            }
            continue;
        }
        if standing.is_none() || has_work {
            continue;
        }
        if !matches!(seat.state, SeatState::Active | SeatState::Standby) {
            continue;
        }
        let wait = crate::quota::Wait {
            kind: STANDBY.into(),
            until: i64::MAX,
            started_at: now,
            provider: String::new(),
            window: String::new(),
            task: None,
            pause: false,
        };
        let parked = crate::quota::park_seat(
            store,
            project,
            seat,
            "standby: the voyage waits for its landing",
            None,
            &wait,
            now,
        )?;
        if !parked {
            continue;
        }
        events.push(format!("{}: parked for the landing", seat.id));
        let Some(profile) = profiles.get(&seat.profile) else {
            continue;
        };
        match crate::quota::release_for_wait(
            store, herdr, project, &config, profile, seat, now, true,
        ) {
            Ok(Some(line)) => events.push(line),
            Ok(None) => {}
            Err(error) => events.push(format!("{}: release failed: {}", seat.id, error.detail)),
        }
    }
    let Some((milestone, order)) = standing else {
        return Ok(events);
    };
    // Reviewers without a job go; one starts again for the next review.
    for seat in seats.iter().filter(|seat| seat.role == SeatRole::Temp) {
        if !matches!(seat.state, SeatState::Active | SeatState::Standby) {
            continue;
        }
        let has_job = seat.task_id.is_some()
            || store
                .read(|tx| tx.get_meta(&format!("engine.review_pending.{}", seat.id)))?
                .is_some_and(|task| !task.is_empty());
        if has_job {
            continue;
        }
        match crate::release::release_one(store, herdr, project, seat, None, now, STANDBY) {
            Ok(_) => events.push(format!("{}: reviewer released for the landing", seat.id)),
            Err(error) => events.push(format!("{}: reviewer release failed: {error}", seat.id)),
        }
    }
    // A captain working alone has no fleet to stand by.
    if !seats.iter().any(|seat| seat.role == SeatRole::Lane) {
        return Ok(events);
    }
    let key = told_key(project);
    if crate::quota::meta_get(store, &key)?.as_deref() == Some(order.as_str()) {
        return Ok(events);
    }
    if let Some(captain) = store.read(|tx| tx.captain_of(project))? {
        let body = format!(
            "Voyage {milestone} stands by for its landing ({order}): every task is done and merged, so the lanes and the idle reviewers are released; the lanes keep their panes, worktrees and branches. Aftercare is yours while the user accepts: a small fix you make yourself, a bigger one is a task as usual, and its lane comes back by itself."
        );
        store.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.to_string(),
                thread: format!("standby:{milestone}"),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: captain.id,
                kind: MsgKind::System,
                urgent: false,
                body,
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: now,
            })?;
            Ok(())
        })?;
        events.push(format!("captain: {milestone} stands by"));
    }
    crate::quota::meta_set(store, &key, &order)?;
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_herdr::AgentStatus;
    use gear_model::TaskState;
    use gear_store::{testing, ProjectRow, TaskRow, VoyageRow};

    const T0: i64 = 1_800_000_000;
    const PANE: &str = "w1:p1";
    const REVIEW_PANE: &str = "w2:p1";

    const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 2

[captain]
pane = "w9:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "pi"
profile = "pi"

[temps]
kind = "pi"
profile = "pi"

[models]
lanes = ["m"]
review = ["r"]
"#;

    fn task(id: &str, state: TaskState) -> TaskRow {
        TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: id.into(),
            rank: 1,
            state,
            priority: "must".into(),
            lane: Some("heat".into()),
            milestone: Some("M1".into()),
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
        }
    }

    /// Voyage M1 with its one task done and its landing order M-1 open; an
    /// active lane and a reviewer with no job.
    fn world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: CONFIG.into(),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 2,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: T0,
                })?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T0)?;
                tx.set_binding_session_ref("p:heat", 1, "ses_lane")?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w9:p1",
                    "claude",
                    T0,
                )?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "review-1",
                    SeatRole::Temp,
                    REVIEW_PANE,
                    "pi",
                    T0,
                )?;
                tx.upsert_voyage(&VoyageRow {
                    project: "p".into(),
                    milestone: "M1".into(),
                    title: "the voyage".into(),
                    done: Some("done".into()),
                    bar: Some("bar".into()),
                    eta: Some("2h".into()),
                    departure: Some(T0 - 3600),
                    departure_tasks: Some(1),
                    departure_estimate_min: Some(60),
                    updated: T0,
                })?;
                tx.insert_task(&task("T-1", TaskState::Done))?;
                let order = tx.insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "station:M1".into(),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: MsgKind::System,
                    urgent: true,
                    body: "Voyage M1 is ready to land".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: T0,
                })?;
                tx.set_meta("station:p:M1:land", &order.id)?;
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
        cfg.profiles = gear_herdr::builtins();
        cfg
    }

    fn state(store: &mut Store, id: &str) -> SeatState {
        store.read(|tx| tx.get_seat(id)).unwrap().unwrap().state
    }

    fn exits(herdr: &FakeHerdr) -> usize {
        herdr
            .calls
            .iter()
            .filter(|call| matches!(call, Call::Prompt { text, .. } if text == "/quit"))
            .count()
    }

    fn captain_lines(store: &mut Store) -> Vec<String> {
        store
            .read(|tx| tx.messages_from_since("p", "engine", 0))
            .unwrap()
            .into_iter()
            .filter(|m| m.recipient == "p:captain" && m.thread.starts_with("standby:"))
            .map(|m| m.body)
            .collect()
    }

    fn lane_starts(herdr: &FakeHerdr) -> Vec<Vec<String>> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::AgentStart { pane, args, .. } if pane == PANE => Some(args.clone()),
                _ => None,
            })
            .collect()
    }

    fn fleet() -> FakeHerdr {
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "pi", AgentStatus::Idle);
        herdr.add_agent(REVIEW_PANE, "pi", AgentStatus::Idle);
        herdr
    }

    /// The landing order is open and nothing is left: the lane is parked and
    /// released with its session kept, the idle reviewer goes, and the
    /// captain is told once.
    #[test]
    fn a_finished_voyage_waiting_for_its_landing_releases_the_fleet() {
        let mut store = world();
        let mut herdr = fleet();
        let cfg = cfg();

        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 60).unwrap();
        assert_eq!(state(&mut store, "p:heat"), SeatState::Parked);
        let wait = crate::quota::load_wait(&mut store, "p:heat")
            .unwrap()
            .unwrap();
        assert_eq!(wait.kind, STANDBY);
        assert_eq!(
            exits(&herdr),
            1,
            "the lane's agent exits: {:?}",
            herdr.calls
        );
        let binding = store
            .read(|tx| tx.get_binding("p:heat", 1))
            .unwrap()
            .unwrap();
        assert_eq!(binding.session_ref.as_deref(), Some("ses_lane"));
        assert!(matches!(
            state(&mut store, "p:review-1"),
            SeatState::Releasing | SeatState::Released
        ));
        assert_eq!(state(&mut store, "p:captain"), SeatState::Active);
        let told = captain_lines(&mut store);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].contains("Aftercare is yours"), "{}", told[0]);

        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 120).unwrap();
        assert_eq!(exits(&herdr), 1, "nothing is released twice");
        assert_eq!(captain_lines(&mut store).len(), 1, "told once");
    }

    /// A fix planned for the lane during the stand-by brings it back in its
    /// own pane.
    #[test]
    fn a_new_task_for_the_lane_brings_it_back_in_its_pane() {
        let mut store = world();
        let mut herdr = fleet();
        let cfg = cfg();
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 60).unwrap();
        herdr.add_shell(PANE);
        assert!(lane_starts(&herdr).is_empty());

        store
            .write(|tx| tx.insert_task(&task("T-2", TaskState::Ready)))
            .unwrap();
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 120).unwrap();
        assert_eq!(lane_starts(&herdr).len(), 1, "{:?}", herdr.calls);
        assert!(crate::quota::load_wait(&mut store, "p:heat")
            .unwrap()
            .is_none());
    }

    /// The landing order answered without a landing ends the stand-by: the
    /// lane comes back even with no task of its own.
    #[test]
    fn a_landing_order_answered_without_a_landing_brings_the_lane_back() {
        let mut store = world();
        let mut herdr = fleet();
        let cfg = cfg();
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 60).unwrap();
        herdr.add_shell(PANE);
        store
            .write(|tx| {
                let order = tx.get_meta("station:p:M1:land")?.unwrap();
                tx.update_message_state(
                    "p",
                    &order,
                    MsgState::Resolved,
                    Some("keep course"),
                    None,
                    None,
                    T0 + 90,
                )
            })
            .unwrap();
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 120).unwrap();
        assert_eq!(lane_starts(&herdr).len(), 1, "{:?}", herdr.calls);
    }

    /// A voyage with work left never stands by.
    #[test]
    fn a_voyage_with_work_left_does_not_stand_by() {
        let mut store = world();
        store
            .write(|tx| tx.insert_task(&task("T-2", TaskState::Ready)))
            .unwrap();
        let mut herdr = fleet();
        let cfg = cfg();
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", T0 + 60).unwrap();
        assert_eq!(state(&mut store, "p:heat"), SeatState::Active);
        assert_eq!(exits(&herdr), 0);
        assert!(captain_lines(&mut store).is_empty());
    }
}
