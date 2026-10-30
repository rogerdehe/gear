//! Working hours (T-450, `[schedule].hours`): the fleet works inside the hours
//! and rests outside them, like a team with office hours.
//!
//! One tick of this module, per project, does what the schedule says:
//!
//! - `notice_min` before the end of the hours, every lane and every reviewer
//!   that holds a job is told once: commit and push, hand in, end the turn;
//!   the captain gets one info line;
//! - at the end, every lane is parked with a wait of kind `hours` until the
//!   next start, its round's clock pauses, and its agent is released as
//!   `on_wait = "release"` does ([`crate::quota::release_for_wait`]) whatever
//!   `on_wait` says — its pane, worktree, branch and session stay. A reviewer
//!   still working is released, and its review starts again at the next
//!   start. The captain is never stopped;
//! - at the next start, every lane parked for hours comes back through
//!   [`crate::quota::bring_back`]: the same pane, its session resumed, told to
//!   continue;
//! - with `depart_at` and no `hours`, the end is `depart_at` plus the voyage's
//!   eta: the notice and the release happen then, and nothing comes back.
//!
//! While the fleet rests, the tick delivers nothing and starts no review (the
//! gates in [`crate::tick`]); queued merges still run.

use std::collections::BTreeMap;

use gear_app::config::ProjectConfig;
use gear_herdr::{HerdrAct, Profile};
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{NewMessage, Store};

use crate::EngineCfg;

/// What the hours step did in one tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HoursReport {
    /// One line per notice, park, release or comeback, for the engine log.
    pub events: Vec<String>,
}

/// The schedule of the project as this tick reads it.
struct Schedule {
    config: ProjectConfig,
    /// `[schedule].hours` is set: the fleet rests outside them.
    hours: bool,
    /// The fixed end of a voyage with `depart_at` and no hours.
    depart_end: Option<Ts>,
    /// The departure time, while it is still ahead.
    depart_at: Option<Ts>,
}

/// The voyage's eta: the planned landing of the latest voyage that departed,
/// as the plan wrote it (`2h`).
fn voyage_eta(store: &mut Store, project: &str) -> GearResult<Option<String>> {
    let mut latest: Option<(Ts, Option<String>)> = None;
    for voyage in store.read(|tx| tx.voyages_of_project(project))? {
        let Some(departure) = voyage.departure else { continue };
        if latest.as_ref().map(|(at, _)| departure > *at).unwrap_or(true) {
            latest = Some((departure, voyage.eta));
        }
    }
    Ok(latest.and_then(|(_, eta)| eta))
}

/// The schedule of the project: `None` when it names none, or when the hours
/// or the departure time cannot be read (the config check refuses them).
fn schedule_of(store: &mut Store, project: &str) -> GearResult<Option<Schedule>> {
    let Ok(config) = gear_app::config::latest(store, project) else {
        return Ok(None);
    };
    let Some(section) = config.schedule.clone() else {
        return Ok(None);
    };
    if section.hours.is_some() {
        return Ok(Some(Schedule {
            config,
            hours: true,
            depart_end: None,
            depart_at: None,
        }));
    }
    let Some(depart_at) = section
        .depart_at
        .as_deref()
        .and_then(gear_app::plan::parse_rfc3339)
    else {
        return Ok(None);
    };
    let Some(eta) = voyage_eta(store, project)? else {
        return Ok(None);
    };
    let Some(eta_s) = gear_app::station::eta_seconds(&eta) else {
        return Ok(None);
    };
    Ok(Some(Schedule {
        config,
        hours: false,
        depart_end: Some(depart_at + eta_s),
        depart_at: Some(depart_at),
    }))
}

/// Whether the fleet rests now: outside the working hours, or after the end
/// of a voyage with `depart_at` and no hours. The tick delivers nothing and
/// starts no review while it does.
pub fn resting(store: &mut Store, project: &str, now: Ts) -> GearResult<bool> {
    let Some(schedule) = schedule_of(store, project)? else {
        return Ok(false);
    };
    if schedule.hours {
        return Ok(gear_app::config::off_hours_until(&schedule.config, now).is_some());
    }
    Ok(schedule.depart_end.is_some_and(|end| now >= end))
}

/// The key of the notice sent for one end of the hours.
fn notice_key(project: &str) -> String {
    format!("engine.hours_notice.{project}")
}

/// The key of the release of one rest period.
fn release_key(project: &str) -> String {
    format!("engine.hours_released.{project}")
}

/// The key of the reviewers released for one rest period.
fn reviews_key(project: &str) -> String {
    format!("engine.hours_reviews.{project}")
}

/// One tick of the working hours of `project`.
pub fn tick(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &BTreeMap<String, Profile>,
    project: &str,
    now: Ts,
) -> GearResult<HoursReport> {
    let mut report = HoursReport::default();
    let Some(schedule) = schedule_of(store, project)? else {
        return Ok(report);
    };
    if schedule.hours {
        if let Some(end) = gear_app::config::hours_end(&schedule.config, now) {
            notice(store, project, &schedule.config, true, end, now, &mut report)?;
        } else if let Some(start) = gear_app::config::off_hours_until(&schedule.config, now) {
            rest(
                store,
                herdr,
                cfg,
                profiles,
                project,
                &schedule.config,
                Some(start),
                start,
                now,
                &mut report,
            )?;
        }
        come_back(store, herdr, cfg, profiles, project, &schedule.config, now, &mut report)?;
        return Ok(report);
    }
    let Some(end) = schedule.depart_end else { return Ok(report) };
    // T-448: before the departure the fleet is not started yet.
    if schedule.depart_at.is_some_and(|at| now < at) {
        return Ok(report);
    }
    if now < end {
        notice(store, project, &schedule.config, false, end, now, &mut report)?;
    } else {
        rest(
            store,
            herdr,
            cfg,
            profiles,
            project,
            &schedule.config,
            None,
            end,
            now,
            &mut report,
        )?;
    }
    Ok(report)
}

/// Tell every lane and reviewer that holds a job, once per end, that the
/// hours end; the captain gets one info line.
fn notice(
    store: &mut Store,
    project: &str,
    config: &ProjectConfig,
    hours: bool,
    end: Ts,
    now: Ts,
    report: &mut HoursReport,
) -> GearResult<()> {
    let notice_min = i64::from(config.schedule.as_ref().map(|s| s.notice_min).unwrap_or(15));
    if now < end - notice_min * 60 {
        return Ok(());
    }
    let key = notice_key(project);
    if crate::quota::meta_get(store, &key)?.as_deref() == Some(end.to_string().as_str()) {
        return Ok(());
    }
    let end_clock = gear_board::idle::clock(end);
    let body = match hours
        .then(|| gear_app::config::hours_next_start(config, now))
        .flatten()
    {
        Some(start) => format!(
            "Working hours end at {end_clock}: commit and push your work now, hand in if it is done, then end your turn. Your worktree and session are kept; you continue at {}.",
            gear_board::idle::clock(start)
        ),
        None => format!(
            "Working hours end at {end_clock}: commit and push your work now, hand in if it is done, then end your turn. Your worktree and session are kept."
        ),
    };
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in &seats {
        if seat.role == SeatRole::Captain {
            continue;
        }
        let task = match seat.role {
            SeatRole::Lane => {
                let open = store.read(|tx| tx.open_attempts_of_seat(&seat.id))?;
                open.first().map(|attempt| (attempt.task.clone(), Some(attempt.n)))
            }
            SeatRole::Temp => {
                let pending = store
                    .read(|tx| tx.get_meta(&format!("engine.review_pending.{}", seat.id)))?
                    .filter(|task| !task.is_empty());
                let task = seat.task_id.clone().or(pending);
                task.map(|task| (task, None))
            }
            _ => None,
        };
        let Some((task, attempt)) = task else { continue };
        if seat.role == SeatRole::Temp && seat.state != SeatState::Active {
            continue;
        }
        store.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.to_string(),
                thread: format!("hours:{end}"),
                task: Some(task.clone()),
                attempt,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: seat.id.clone(),
                kind: MsgKind::Instruction,
                urgent: true,
                body: body.clone(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })
        })?;
        report.events.push(format!("{}: told the working hours end", seat.id));
    }
    if let Some(captain) = store.read(|tx| tx.captain_of(project))? {
        let line = match hours
            .then(|| gear_app::config::hours_next_start(config, now))
            .flatten()
        {
            Some(start) => format!(
                "Working hours end at {end_clock}; the fleet rests until {}.",
                gear_board::idle::clock(start)
            ),
            None => format!("The voyage's time is up at {end_clock}; the fleet stops."),
        };
        store.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.to_string(),
                thread: format!("hours:{end}"),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: captain.id,
                kind: MsgKind::System,
                urgent: false,
                body: line.clone(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: now,
            })
        })?;
        report.events.push(format!("captain: {line}"));
    }
    crate::quota::meta_set(store, &key, &end.to_string())?;
    Ok(())
}

/// Park every lane until the next start and release it, and release a reviewer
/// still working — once per rest period.
#[allow(clippy::too_many_arguments)]
fn rest(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    _cfg: &EngineCfg,
    profiles: &BTreeMap<String, Profile>,
    project: &str,
    config: &ProjectConfig,
    next_start: Option<Ts>,
    until: Ts,
    now: Ts,
    report: &mut HoursReport,
) -> GearResult<()> {
    let key = release_key(project);
    let released = crate::quota::meta_get(store, &key)?;
    let marker = next_start.map(|start| start.to_string()).unwrap_or_else(|| until.to_string());
    let release_lanes = released.as_deref() != Some(marker.as_str());
    if release_lanes {
        let seats = store.read(|tx| tx.seats_of_project(project))?;
        for seat in seats.iter().filter(|seat| seat.role == SeatRole::Lane) {
            if !matches!(seat.state, SeatState::Active | SeatState::Standby) {
                // A parked lane is already resting; a lane without an agent has
                // nothing to release.
                continue;
            }
            let task = store
                .read(|tx| tx.open_attempts_of_seat(&seat.id))?
                .first()
                .map(|attempt| attempt.task.clone());
            let wait = crate::quota::Wait {
                kind: "hours".into(),
                until,
                started_at: now,
                provider: String::new(),
                window: String::new(),
                task,
                pause: true,
            };
            let reason = if next_start.is_some() {
                format!("hours: the fleet rests until {until}")
            } else {
                "hours: the voyage's time is up; nothing comes back".to_string()
            };
            let parked = crate::quota::park_seat(
                store,
                project,
                seat,
                &reason,
                Some("working hours"),
                &wait,
                now,
            )?;
            if !parked {
                report.events.push(format!("{}: cannot park for the working hours", seat.id));
                continue;
            }
            report.events.push(format!("{}: parked for the working hours until {until}", seat.id));
            let Some(profile) = profiles.get(&seat.profile) else {
                report.events.push(format!("{}: no profile; its agent stays", seat.id));
                continue;
            };
            match crate::quota::release_for_wait(
                store, herdr, project, config, profile, seat, now, true,
            ) {
                Ok(Some(line)) => report.events.push(line),
                Ok(None) => report.events.push(format!(
                    "{}: kept through the working hours: nothing to release",
                    seat.id
                )),
                Err(error) => report
                    .events
                    .push(format!("{}: release failed: {}", seat.id, error.detail)),
            }
        }
        crate::quota::meta_set(store, &key, &marker)?;
    }

    // A reviewer still working is interrupted and released: its pane and
    // worktree stay, and `start_reviews` starts its review again at the next
    // start. The captain is a role of its own and is never stopped.
    let key = reviews_key(project);
    if crate::quota::meta_get(store, &key)?.as_deref() == Some(marker.as_str()) {
        return Ok(());
    }
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats.iter().filter(|seat| seat.role == SeatRole::Temp) {
        if seat.state != SeatState::Active {
            continue;
        }
        let has_job = seat.task_id.is_some()
            || store
                .read(|tx| tx.get_meta(&format!("engine.review_pending.{}", seat.id)))?
                .is_some_and(|task| !task.is_empty());
        if !has_job {
            continue;
        }
        match crate::release::release_one(
            store,
            herdr,
            project,
            seat,
            seat.task_id.as_deref(),
            now,
            "hours",
        ) {
            Ok(true) => report.events.push(format!("{}: reviewer released for the working hours", seat.id)),
            Ok(false) => report.events.push(format!("{}: reviewer is leaving", seat.id)),
            Err(error) => report.events.push(format!("{}: reviewer release failed: {error}", seat.id)),
        }
    }
    crate::quota::meta_set(store, &key, &marker)?;
    Ok(())
}

/// At the next start, every lane parked for hours comes back through
/// [`crate::quota::bring_back`]: the same pane, its session resumed, told to
/// continue.
#[allow(clippy::too_many_arguments)]
fn come_back(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &BTreeMap<String, Profile>,
    project: &str,
    config: &ProjectConfig,
    now: Ts,
    report: &mut HoursReport,
) -> GearResult<()> {
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats.iter().filter(|seat| seat.role == SeatRole::Lane) {
        let Some(wait) = crate::quota::load_wait(store, &seat.id)? else {
            continue;
        };
        // A final rest (`depart_at` plus the eta) never comes back.
        if wait.kind != "hours" || wait.until > now || wait.until == i64::MAX {
            continue;
        }
        let released = crate::quota::meta_get(
            store,
            &crate::quota::meta_key("wait_released", &seat.id),
        )?
        .as_deref()
            == Some("1");
        if !released {
            // Its agent stayed in the pane: the source wake continues it.
            continue;
        }
        match crate::quota::bring_back(store, herdr, cfg, profiles, project, config, seat, now) {
            Ok(Some(line)) => report.events.push(line),
            Ok(None) => {}
            Err(error) => report
                .events
                .push(format!("{}: comeback failed: {}", seat.id, error.detail)),
        }
    }
    Ok(())
}
