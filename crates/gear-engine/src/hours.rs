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
        // Outside the hours the fleet rests: park and release it. Inside, the
        // notice is due `notice_min` before the end.
        if let Some(start) = gear_app::config::off_hours_until(&schedule.config, now) {
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
        } else if let Some(end) = gear_app::config::hours_end(&schedule.config, now) {
            notice(store, project, &schedule.config, true, end, now, &mut report)?;
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
            // A final rest (`depart_at` plus the eta) never comes back: its
            // wait is past every clock, so no comeback can fire.
            let until = if next_start.is_some() { until } else { i64::MAX };
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

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;
    use gear_model::{AttemptState, OpKind, SeatState, TaskState};
    use gear_store::{testing, AttemptRow, ContractRow, ProjectRow, Store, TaskRow, VoyageRow};

    const T0: i64 = 1_800_000_000;
    const PANE: &str = "w1:p1";
    const REVIEW_PANE: &str = "w2:p1";

    /// A local time on 2027-03-10, as the machine's clock sees it.
    fn local(hour: i32, minute: i32) -> Ts {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = 127;
        tm.tm_mon = 2;
        tm.tm_mday = 10;
        tm.tm_hour = hour;
        tm.tm_min = minute;
        tm.tm_isdst = -1;
        (unsafe { libc::mktime(&mut tm) }) as Ts
    }

    fn config_toml(schedule: &str) -> String {
        format!(
            r#"
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
on_quota = {{ lanes = "wait", review = "wait" }}
{schedule}
"#
        )
    }

    fn hours_schedule() -> String {
        "[schedule]\nhours = \"10:00-18:00\"\nnotice_min = 15\n".to_string()
    }

    /// A project with one active pi lane that works on T-1 and, when
    /// `with_reviewer` is set, a reviewer working on T-2.
    fn world(config: &str, with_reviewer: bool) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: config.into(),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 2,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: T0,
                })?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "pi", T0)?;
                tx.set_binding_session_ref("p:heat", 1, "ses_lane")?;
                let round = gear_store::RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&gear_store::NewRound {
                    key: round.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: Some("m".into()),
                    start_at: T0,
                })?;
                tx.add_round_session(&round, "ses_lane")?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w9:p1", "claude", T0)?;
                if with_reviewer {
                    testing::seed_temp_seat(tx, "p", "review-1", "T-2", REVIEW_PANE, "pi", T0)?;
                }
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
        cfg.profiles = gear_herdr::builtins();
        cfg
    }

    fn seat(store: &mut Store, id: &str) -> gear_store::SeatRow {
        store.read(|tx| tx.get_seat(id)).unwrap().unwrap()
    }

    fn messages(store: &mut Store, to: &str) -> Vec<gear_store::MessageRow> {
        store.read(|tx| tx.messages_to_since("p", to, 0)).unwrap()
    }

    /// Every line the engine sent to the captain, info included.
    fn captain_lines(store: &mut Store) -> Vec<gear_store::MessageRow> {
        store
            .read(|tx| tx.messages_from_since("p", "engine", 0))
            .unwrap()
            .into_iter()
            .filter(|message| message.recipient == "p:captain")
            .collect()
    }

    fn deliveries(store: &mut Store) -> Vec<String> {
        store
            .read(|tx| tx.ops_in_state("p", gear_model::OpState::Intended))
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

    fn agent_starts(herdr: &FakeHerdr) -> Vec<(String, String, Vec<String>)> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                gear_herdr::fake::Call::AgentStart { kind, pane, args, .. } => {
                    Some((kind.clone(), pane.clone(), args.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn exits(herdr: &FakeHerdr) -> usize {
        herdr
            .calls
            .iter()
            .filter(|call| matches!(call, gear_herdr::fake::Call::Prompt { text, .. } if text == "/quit"))
            .count()
    }

    /// T-450 (1): at end minus `notice_min` every lane and reviewer with a job
    /// is told once; the captain gets one info line.
    #[test]
    fn the_notice_goes_to_every_seat_with_a_job_once() {
        let mut store = world(&config_toml(&hours_schedule()), true);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "pi", AgentStatus::Idle);
        herdr.add_agent(REVIEW_PANE, "pi", AgentStatus::Idle);
        let cfg = cfg();
        let end = local(18, 0);
        let notice = end - 15 * 60;

        let report = tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", notice - 1).unwrap();
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert!(messages(&mut store, "p:heat").is_empty());

        let report = tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", notice).unwrap();
        let lane = messages(&mut store, "p:heat");
        assert_eq!(lane.len(), 1, "{lane:?}");
        assert!(lane[0].body.starts_with("Working hours end at 18:00:"), "{:?}", lane[0].body);
        assert!(lane[0].body.contains("you continue at 10:00"), "{:?}", lane[0].body);
        let reviewer = messages(&mut store, "p:review-1");
        assert_eq!(reviewer.len(), 1, "{reviewer:?}");
        assert_eq!(reviewer[0].body, lane[0].body);
        let captain = captain_lines(&mut store);
        assert_eq!(captain.len(), 1, "{captain:?}");
        assert!(captain[0].body.contains("the fleet rests until 10:00"), "{:?}", captain[0].body);
        assert_eq!(captain[0].state, gear_model::MsgState::Info);
        assert!(report.events.iter().any(|line| line.contains("captain:")), "{:?}", report.events);

        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", notice + 1).unwrap();
        assert_eq!(messages(&mut store, "p:heat").len(), 1, "one notice");
        assert_eq!(messages(&mut store, "p:review-1").len(), 1, "one notice");
        assert_eq!(captain_lines(&mut store).len(), 1, "one line");
    }

    /// T-450 (2): at the end the lane is parked with a `hours` wait until the
    /// next start and released; its pane, worktree and session stay. The
    /// reviewer is released; nothing is delivered and the fleet rests.
    #[test]
    fn the_end_parks_and_releases_the_lane_and_rests() {
        let mut store = world(&config_toml(&hours_schedule()), true);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "pi", AgentStatus::Idle);
        herdr.add_agent(REVIEW_PANE, "pi", AgentStatus::Idle);
        let cfg = cfg();
        let end = local(18, 0);
        let next = gear_app::config::off_hours_until(&gear_app::config::latest(&mut store, "p").unwrap(), end).unwrap();
        assert_eq!(gear_board::idle::clock(next), "10:00");

        let report = tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", end).unwrap();
        let lane = seat(&mut store, "p:heat");
        assert_eq!(lane.state, SeatState::Parked);
        assert!(lane.state_reason.as_deref().unwrap_or_default().contains("hours"), "{lane:?}");
        let wait = crate::quota::load_wait(&mut store, "p:heat").unwrap().expect("the hours wait");
        assert_eq!(wait.kind, "hours");
        assert_eq!(wait.until, next);
        assert!(wait.pause, "the round's clock takes the pause");
        assert_eq!(
            crate::quota::meta_get(&mut store, &crate::quota::meta_key("wait_released", "p:heat"))
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert_eq!(exits(&herdr), 1, "{:?}", herdr.calls);
        // The pane and its session are kept: only the agent went.
        let binding = store.read(|tx| tx.get_binding("p:heat", 1)).unwrap().unwrap();
        assert_eq!(binding.status, gear_store::BindingStatus::Active);
        assert_eq!(binding.session_ref.as_deref(), Some("ses_lane"));
        assert!(deliveries(&mut store).is_empty(), "{:?}", deliveries(&mut store));
        assert!(resting(&mut store, "p", end).unwrap());
        // The reviewer is on its way out, not reviewing any more.
        assert!(matches!(seat(&mut store, "p:review-1").state, SeatState::Releasing | SeatState::Released));
        assert!(report.events.iter().any(|line| line.contains("p:review-1: reviewer")), "{:?}", report.events);

        // A second tick at rest releases nothing again.
        let report = tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", end + 60).unwrap();
        assert_eq!(exits(&herdr), 1, "{:?}", herdr.calls);
        assert!(!report.events.iter().any(|line| line.contains("parked")), "{:?}", report.events);
    }

    /// T-450 (3): at the next start the lane comes back in the same pane with
    /// its session and a Continue.
    #[test]
    fn the_next_start_brings_the_lane_back_with_its_session_and_a_continue() {
        let mut store = world(&config_toml(&hours_schedule()), false);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "pi", AgentStatus::Idle);
        let cfg = cfg();
        let end = local(18, 0);
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", end).unwrap();
        assert_eq!(seat(&mut store, "p:heat").state, SeatState::Parked);
        // The agent exited: the pane is a shell again.
        herdr.add_shell(PANE);

        let start = local(10, 0) + 86_400;
        let report = tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", start).unwrap();
        let starts = agent_starts(&herdr);
        assert_eq!(starts.len(), 1, "{:?}", herdr.calls);
        let (kind, pane, args) = &starts[0];
        assert_eq!((kind.as_str(), pane.as_str()), ("pi", PANE));
        assert!(args.windows(2).any(|pair| pair == ["--session", "ses_lane"]), "{args:?}");
        let lane = seat(&mut store, "p:heat");
        assert_eq!(lane.state, SeatState::Active);
        assert_eq!(lane.generation, 2);
        assert!(deliveries(&mut store).iter().any(|text| text.starts_with("Continue T-1")), "{:?}", deliveries(&mut store));
        assert!(crate::quota::load_wait(&mut store, "p:heat").unwrap().is_none());
        assert!(report.events.iter().any(|line| line.contains("restarted on pi")), "{:?}", report.events);
    }

    /// T-450 (4): with `depart_at` and no hours, the end is `depart_at` plus
    /// the voyage's eta; the notice and the release happen then, and nothing
    /// comes back.
    #[test]
    fn depart_at_plus_the_eta_ends_the_voyage_and_nothing_comes_back() {
        let depart = gear_app::plan::parse_rfc3339("2027-03-10T12:00:00Z").unwrap();
        let config = config_toml(&format!(
            "[schedule]\ndepart_at = \"2027-03-10T12:00:00Z\"\nnotice_min = 15\n"
        ));
        let mut store = world(&config, false);
        store
            .write(|tx| {
                tx.upsert_voyage(&VoyageRow {
                    project: "p".into(),
                    milestone: "M1".into(),
                    title: "the voyage".into(),
                    done: Some("done".into()),
                    bar: Some("bar".into()),
                    eta: Some("2h".into()),
                    departure: Some(depart - 3600),
                    departure_tasks: Some(1),
                    departure_estimate_min: Some(60),
                    updated: T0,
                })
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "pi", AgentStatus::Idle);
        let cfg = cfg();
        let end = depart + 7200;

        // Before the departure nothing happens (T-448 owns that window).
        let report = tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", depart - 60).unwrap();
        assert!(report.events.is_empty(), "{:?}", report.events);
        assert!(messages(&mut store, "p:heat").is_empty());

        // The notice, 15 min before the end.
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", end - 15 * 60).unwrap();
        let lane = messages(&mut store, "p:heat");
        assert_eq!(lane.len(), 1, "{lane:?}");
        assert!(lane[0].body.starts_with("Working hours end at"), "{:?}", lane[0].body);
        assert_eq!(captain_lines(&mut store).len(), 1);

        // At the end: parked and released, and the wait never ends.
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", end).unwrap();
        let lane = seat(&mut store, "p:heat");
        assert_eq!(lane.state, SeatState::Parked);
        let wait = crate::quota::load_wait(&mut store, "p:heat").unwrap().expect("the final wait");
        assert_eq!(wait.kind, "hours");
        assert_eq!(wait.until, i64::MAX);
        assert_eq!(exits(&herdr), 1, "{:?}", herdr.calls);
        assert!(resting(&mut store, "p", end).unwrap());

        // Nothing comes back later.
        herdr.add_shell(PANE);
        tick(&mut store, &mut herdr, &cfg, &cfg.profiles, "p", end + 86_400).unwrap();
        assert!(agent_starts(&herdr).is_empty(), "{:?}", herdr.calls);
        assert_eq!(seat(&mut store, "p:heat").state, SeatState::Parked);
    }

    /// T-450 (2): while the fleet rests the tick starts no review.
    #[test]
    fn no_review_starts_while_the_fleet_rests() {
        let mut store = world(&config_toml(&hours_schedule()), false);
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-2".into(),
                    rank: 2,
                    state: TaskState::Verifying,
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
                    task: "T-2".into(),
                    version: 1,
                    goal: "the goal".into(),
                    accept: "the accept".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
                tx.insert_attempt(&AttemptRow {
                    project: "p".into(),
                    task: "T-2".into(),
                    n: 1,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Submitted,
                    candidate: Some("deadbeef".into()),
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                    session_ref: Some("ses_lane".into()),
                    session_cli: Some("pi".into()),
                    session_action: None,
                    sent_at: Some(T0),
                })?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "pi", AgentStatus::Idle);
        let cfg = cfg();
        let end = local(18, 0);

        crate::tick(&mut store, &mut herdr, &cfg, end).unwrap();

        let temps: Vec<_> = store
            .read(|tx| tx.seats_of_project("p"))
            .unwrap()
            .into_iter()
            .filter(|seat| seat.role == SeatRole::Temp)
            .collect();
        assert!(temps.is_empty(), "no reviewer started at rest: {temps:?}");
        assert_eq!(seat(&mut store, "p:heat").state, SeatState::Parked);
        assert!(resting(&mut store, "p", end).unwrap());
    }
}
