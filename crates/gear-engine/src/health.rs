//! The fleet and machine health watch (M6 S3, T-472).
//!
//! GEAR must watch the machine it runs the fleet on, and the watch must not
//! load the machine it protects. Once per `[health].sample_min` minutes (one
//! by default) the engine samples, as facts and not screen text:
//!
//! - the CPU and resident memory of each lane's and reviewer's own processes,
//!   its pane's shell and foreground processes and every child of them;
//! - the machine's load, its free memory and the free space of the volumes
//!   GEAR needs (the state directory and the project's repos);
//! - the top processes outside the fleet, named but never touched.
//!
//! A machine that stays short (`[health].load_x_cores` × the cores for
//! `machine_over_min` minutes, free memory under `mem_free_pct` %, or a
//! watched volume under `disk_free_gb` GB) has one of two causes:
//!
//! - internal: a seat's own processes are over the safe line (`seat_cpu_pct`
//!   % of one core or `seat_mem_mb` MB for `seat_over_min` minutes). The
//!   engine interrupts that seat and stops its agent (its profile's exit),
//!   parks the seat in NeedsCheck for the captain, and tells the captain
//!   what it measured;
//! - external: no seat is over the safe line. The captain gets one urgent
//!   item naming the top processes and the machine's facts; nothing outside
//!   the fleet is touched.
//!
//! The reader is a trait, so the engine's tests inject readings; production
//! is [`SystemRead`], which runs one `ps` and a few system reads per sample.

use crate::observe::{Presence, SeatObs};
use gear_herdr::{HerdrAct, Profile};
use gear_model::{GearResult, MsgKind, MsgState, SeatRole, SeatState, Ts};
use gear_store::{BindingStatus, NewMessage, Store};
use std::collections::{BTreeMap, BTreeSet};

/// The thread a seat's health item lives in.
fn seat_thread(seat: &str) -> String {
    format!("health:{seat}")
}

/// The thread the machine's own item lives in.
fn machine_thread(project: &str) -> String {
    format!("health:machine:{project}")
}

fn sample_key(project: &str) -> String {
    format!("engine.health.sample.{project}")
}
fn short_key(project: &str) -> String {
    format!("engine.health.short.{project}")
}
fn seat_since_key(seat: &str) -> String {
    format!("engine.health.seat.{seat}.since")
}
fn seat_stop_key(seat: &str) -> String {
    format!("engine.health.seat.{seat}.stop")
}
fn machine_alert_key(project: &str) -> String {
    format!("engine.health.machine.{project}.alert")
}
fn cpu_key(seat: &str) -> String {
    format!("engine.health.cpu.{seat}")
}

/// A seat's own processes: the sum of its pane's tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SeatProcs {
    /// Cumulative CPU seconds (user + system) of the tree.
    pub cpu_s: f64,
    /// Resident memory of the tree, in KB.
    pub rss_kb: u64,
    /// How many processes the tree holds.
    pub procs: usize,
}

/// The machine's facts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Machine {
    pub cores: u32,
    /// The 5-minute load average.
    pub load5: f64,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    /// Free KB of each watched volume: (path, free KB).
    pub disk: Vec<(String, u64)>,
}

/// One process outside the fleet, for the captain's item.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Proc {
    pub pid: u32,
    /// Recent CPU use, in percent of one core.
    pub cpu_pct: f64,
    pub rss_kb: u64,
    pub command: String,
}

/// One sample of the machine and the fleet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reading {
    pub machine: Machine,
    /// Each watched seat's own processes, by seat id.
    pub seats: BTreeMap<String, SeatProcs>,
    /// The top processes outside the fleet, worst first.
    pub top: Vec<Proc>,
}

/// Where a seat's processes start: Herdr's pane process info.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneProcs {
    pub shell: Option<u32>,
    pub foreground: Vec<u32>,
    pub foreground_group: Option<u32>,
}

/// The machine and process reader of the health watch. Production is
/// [`SystemRead`]; a test injects its own readings.
pub trait Read: Send + Sync + std::fmt::Debug {
    /// One sample of the machine, the seats and the top processes outside the
    /// fleet. `volumes` are the paths whose free space the watch needs.
    ///
    /// `&self`: the engine shares one reader behind an `Arc`; a reader that
    /// keeps state between samples (the production one keeps the previous
    /// process table) guards it itself.
    fn sample(
        &self,
        panes: &BTreeMap<String, PaneProcs>,
        volumes: &[String],
    ) -> GearResult<Reading>;
}

/// What one watch step did, for the tick report.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Watch {
    /// Seats stopped for being over the safe line.
    pub stopped: Vec<String>,
    /// Captain items opened or resolved (`<seat>:opened|resolved`,
    /// `machine:opened|resolved`).
    pub items: Vec<String>,
    /// What the watch could not do: a sample that failed, a stop whose exit
    /// was not delivered. Never silent: the tick report and the engine log
    /// carry these.
    pub notes: Vec<String>,
}

/// The health limits of a project, from its `[health]` (the defaults when the
/// config names none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub sample_min: i64,
    pub load_x_cores: f64,
    pub machine_over_min: i64,
    pub mem_free_pct: u64,
    pub disk_free_gb: u64,
    pub seat_cpu_pct: f64,
    pub seat_mem_mb: u64,
    pub seat_over_min: i64,
}

impl Default for Limits {
    fn default() -> Self {
        let health = gear_app::config::Health::default();
        Limits::of(&health)
    }
}

impl Limits {
    fn of(health: &gear_app::config::Health) -> Limits {
        Limits {
            sample_min: i64::from(health.sample_min.max(1)),
            load_x_cores: health.load_x_cores,
            machine_over_min: i64::from(health.machine_over_min.max(1)),
            mem_free_pct: u64::from(health.mem_free_pct.clamp(1, 99)),
            disk_free_gb: health.disk_free_gb,
            seat_cpu_pct: f64::from(health.seat_cpu_pct),
            seat_mem_mb: health.seat_mem_mb,
            seat_over_min: i64::from(health.seat_over_min.max(1)),
        }
    }
}

/// The limits of the project's `[health]`, or the defaults when its config
/// does not parse.
fn limits_of(store: &mut Store, project: &str) -> Limits {
    match store.read(|tx| tx.get_project(project)) {
        Ok(Some(row)) => match gear_app::config::parse(&row.config_toml) {
            Ok(config) => Limits::of(&config.health),
            Err(_) => Limits::default(),
        },
        _ => Limits::default(),
    }
}

/// The volumes whose free space the watch reads: the state directory (the
/// store the engine writes) and the project's repos and worktree root.
fn volumes_of(store: &mut Store, cfg: &crate::EngineCfg, project: &str) -> Vec<String> {
    let mut volumes: Vec<String> = Vec::new();
    if let Some(dir) = std::path::Path::new(&cfg.db_path).parent() {
        volumes.push(dir.to_string_lossy().into_owned());
    }
    if let Ok(Some(row)) = store.read(|tx| tx.get_project(project)) {
        if let Ok(config) = gear_app::config::parse(&row.config_toml) {
            for repo in config.repos() {
                volumes.push(repo.path.clone());
            }
        }
    }
    volumes.sort();
    volumes.dedup();
    volumes
}

/// Whether the machine is short from this sample, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Short {
    /// The line the captain's item names.
    why: String,
}

fn short_reason(machine: &Machine, limits: &Limits) -> Option<Short> {
    let cores = machine.cores.max(1);
    let load_line = limits.load_x_cores * f64::from(cores);
    if machine.load5 > load_line {
        return Some(Short {
            why: format!(
                "the load average is {:.2} on {} cores (the safe line is {:.1})",
                machine.load5, cores, load_line
            ),
        });
    }
    if machine.mem_total_kb > 0 {
        let free_pct = machine.mem_free_kb.saturating_mul(100) / machine.mem_total_kb;
        if free_pct < limits.mem_free_pct {
            return Some(Short {
                why: format!(
                    "free memory is {} MB of {} MB ({free_pct} %, the safe line is {} %)",
                    machine.mem_free_kb / 1024,
                    machine.mem_total_kb / 1024,
                    limits.mem_free_pct
                ),
            });
        }
    }
    let disk_line = limits.disk_free_gb.saturating_mul(1024 * 1024);
    for (path, free_kb) in &machine.disk {
        if *free_kb < disk_line {
            return Some(Short {
                why: format!(
                    "the volume {path} has {} GB free (the safe line is {} GB)",
                    free_kb / (1024 * 1024),
                    limits.disk_free_gb
                ),
            });
        }
    }
    None
}

/// A seat over the safe line, with what it used.
#[derive(Debug, Clone, PartialEq)]
struct Over {
    cpu_pct: f64,
    rss_mb: u64,
}

fn over(procs: &SeatProcs, cpu_pct: Option<f64>, limits: &Limits) -> Option<Over> {
    let rss_mb = procs.rss_kb / 1024;
    let cpu_over = cpu_pct.is_some_and(|pct| pct >= limits.seat_cpu_pct);
    let mem_over = rss_mb >= limits.seat_mem_mb;
    (cpu_over || mem_over).then_some(Over {
        cpu_pct: cpu_pct.unwrap_or(0.0),
        rss_mb,
    })
}

/// The seats the watch reads: lanes and reviewers whose agent is present.
fn watched(observed: &[SeatObs]) -> Vec<&SeatObs> {
    observed
        .iter()
        .filter(|obs| {
            matches!(obs.seat.role, SeatRole::Lane | SeatRole::Temp)
                && obs.presence == Presence::Present
                && matches!(
                    obs.seat.state,
                    SeatState::Active | SeatState::Standby | SeatState::Retiring
                )
        })
        .collect()
}

/// One step of the health watch for `project`.
///
/// It samples once per `[health].sample_min` minutes; between samples it reads
/// the store's sample time and parses the project's config (a few KB of TOML),
/// and nothing else. A sample that fails is noted (the tick report and the
/// engine log) and retried at the next interval. The actions are the engine's
/// own seats only: a seat
/// over the safe line is interrupted and stopped, and the captain is told;
/// a machine that is short for another reason gets one urgent captain item
/// naming the top processes.
pub fn check(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &crate::EngineCfg,
    project: &str,
    observed: &[SeatObs],
    profiles: &BTreeMap<String, Profile>,
    now: Ts,
) -> GearResult<Watch> {
    let mut watch = Watch::default();
    let limits = limits_of(store, project);
    // Once per sample_min minutes: between samples this is the sample time
    // and the project config, not a machine read.
    let last = store
        .read(|tx| tx.get_meta(&sample_key(project)))?
        .and_then(|value| value.parse::<Ts>().ok());
    if last.is_some_and(|last| now - last < limits.sample_min * 60) {
        return Ok(watch);
    }
    store.write(|tx| tx.set_meta(&sample_key(project), &now.to_string()))?;

    // The panes of the seats this project watches: their shell and foreground
    // processes are where the seat's own processes start.
    let seats = watched(observed);
    let mut panes: BTreeMap<String, PaneProcs> = BTreeMap::new();
    for obs in &seats {
        let pane = store.read(|tx| {
            Ok(tx
                .get_binding(&obs.seat.id, obs.seat.generation)?
                .filter(|binding| binding.status == BindingStatus::Active)
                .map(|binding| binding.pane_id))
        })?;
        let Some(pane) = pane else {
            continue;
        };
        let Ok(info) = herdr.pane_process_info(&pane) else {
            continue;
        };
        panes.insert(
            obs.seat.id.clone(),
            PaneProcs {
                shell: info.shell_pid,
                foreground: info
                    .foreground_processes
                    .iter()
                    .map(|process| process.pid)
                    .collect(),
                foreground_group: info.foreground_process_group_id,
            },
        );
    }
    let volumes = volumes_of(store, cfg, project);
    let reading = match cfg.health.sample(&panes, &volumes) {
        Ok(reading) => reading,
        Err(error) => {
            // The watch is blind this minute, not quiet: the failure reaches
            // the tick report and the engine log, and the next sample retries.
            watch.notes.push(format!(
                "sample failed ({}: {}): the next sample retries",
                error.code, error.detail
            ));
            return Ok(watch);
        }
    };

    // Each seat's CPU use over the interval: the sample carries cumulative
    // seconds, so the previous sample's total is the baseline.
    let interval = last.map(|last| (now - last).max(1)).unwrap_or(0);
    let mut over_seats: Vec<(String, Over)> = Vec::new();
    for obs in &seats {
        let Some(procs) = reading.seats.get(&obs.seat.id) else {
            continue;
        };
        let previous = store
            .read(|tx| tx.get_meta(&cpu_key(&obs.seat.id)))?
            .and_then(|value| value.parse::<f64>().ok());
        store.write(|tx| tx.set_meta(&cpu_key(&obs.seat.id), &procs.cpu_s.to_string()))?;
        let cpu_pct = previous.filter(|_| interval > 0).map(|previous| {
            let delta = (procs.cpu_s - previous).max(0.0);
            delta / interval as f64 * 100.0
        });
        if let Some(over) = over(procs, cpu_pct, &limits) {
            let key = seat_since_key(&obs.seat.id);
            let since = store
                .read(|tx| tx.get_meta(&key))?
                .and_then(|value| value.parse::<Ts>().ok());
            let since = match since {
                Some(since) if since <= now => since,
                _ => {
                    store.write(|tx| tx.set_meta(&key, &now.to_string()))?;
                    now
                }
            };
            if now - since >= limits.seat_over_min * 60 {
                over_seats.push((obs.seat.id.clone(), over));
            }
        } else {
            store.write(|tx| tx.set_meta(&seat_since_key(&obs.seat.id), ""))?;
            resolve_seat_item(store, project, &obs.seat.id, now, &mut watch)?;
        }
    }

    let Some(short) = short_reason(&reading.machine, &limits) else {
        // The machine is fine: the spell is over, a later one starts fresh.
        store.write(|tx| tx.set_meta(&short_key(project), ""))?;
        resolve_machine_item(store, project, now, &mut watch)?;
        return Ok(watch);
    };
    let short_since = store
        .read(|tx| tx.get_meta(&short_key(project)))?
        .and_then(|value| value.parse::<Ts>().ok());
    let short_since = match short_since {
        Some(since) if since <= now => since,
        _ => {
            store.write(|tx| tx.set_meta(&short_key(project), &now.to_string()))?;
            now
        }
    };
    if now - short_since < limits.machine_over_min * 60 {
        return Ok(watch); // the machine is short, but not long enough yet
    }

    if over_seats.is_empty() {
        // External cause: our seats are not the reason. Name the top
        // processes; never touch them.
        open_machine_item(
            store,
            project,
            &short.why,
            &reading.machine,
            &reading.top,
            now,
            &mut watch,
        )?;
        return Ok(watch);
    }

    // Internal cause: stop each seat over the safe line once per spell, and
    // tell the captain. The machine item is resolved: the cause is known.
    resolve_machine_item(store, project, now, &mut watch)?;
    for (seat_id, over) in over_seats {
        let Some(obs) = seats.iter().find(|obs| obs.seat.id == seat_id) else {
            continue;
        };
        let stop_key = seat_stop_key(&seat_id);
        let stopped = store
            .read(|tx| tx.get_meta(&stop_key))?
            .is_some_and(|value| !value.is_empty());
        if stopped {
            continue; // this spell already stopped and told
        }
        let pane = store.read(|tx| {
            Ok(tx
                .get_binding(&seat_id, obs.seat.generation)?
                .filter(|binding| binding.status == BindingStatus::Active)
                .map(|binding| binding.pane_id))
        })?;
        let Some(pane) = pane else {
            continue;
        };
        let delivered = stop_seat(herdr, profiles, &obs.seat, &pane);
        let reason = if delivered {
            format!(
                "the health watch stopped seat {}: its own processes used {:.0}% of one core and {} MB of memory for {} minutes while the machine was short ({})",
                obs.seat.name, over.cpu_pct, over.rss_mb, limits.seat_over_min, short.why
            )
        } else {
            // The seat is parked either way; the words say what really
            // happened, and the tick report carries the note.
            watch.notes.push(format!(
                "{}: the exit was not delivered, the seat is parked and the captain told",
                seat_id
            ));
            format!(
                "the health watch found seat {} over the safe line and asked it to stop, but its exit was not delivered: its own processes used {:.0}% of one core and {} MB of memory for {} minutes while the machine was short ({})",
                obs.seat.name, over.cpu_pct, over.rss_mb, limits.seat_over_min, short.why
            )
        };
        store.write(|tx| {
            tx.set_seat_state(&seat_id, SeatState::NeedsCheck, Some(&reason), now)?;
            tx.set_meta(&stop_key, "1")
        })?;
        let Some(captain) = store.read(|tx| tx.captain_of(project))? else {
            continue;
        };
        let body = format!(
            "{reason}. Look at its pane, fix the cause, then start it again (`- retry {}` in the plan for a lane; a reviewer is released and started for its next job). Machine: load {:.2} on {} cores, {} MB free of {} MB.",
            obs.seat.name,
            reading.machine.load5,
            reading.machine.cores,
            reading.machine.mem_free_kb / 1024,
            reading.machine.mem_total_kb / 1024
        );
        store.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.into(),
                thread: seat_thread(&seat_id),
                task: obs.seat.task_id.clone(),
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: captain.id,
                kind: MsgKind::System,
                urgent: true,
                body,
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })?;
            Ok(())
        })?;
        watch.stopped.push(seat_id.clone());
        watch.items.push(format!("{seat_id}:opened"));
    }
    // T-472 F2: the spell is answered, so the machine-short timer starts over:
    // the next samples do not blame an outside cause while the machine
    // recovers (a genuinely external cause re-arms and is reported after
    // `machine_over_min` again).
    store.write(|tx| tx.set_meta(&short_key(project), ""))?;
    Ok(watch)
}

/// Interrupt the seat's agent and stop it: its turn is cut (esc), then the
/// profile's exit (a command for the CLIs whose exit is a slash command, keys
/// for the others). The pane stays open; the seat waits for the captain.
///
/// Returns whether the exit was actually delivered: a pane that is gone, an
/// agent Herdr left unnamed, or a refused call means the agent may still run,
/// and the caller says so instead of claiming a stop that did not happen.
fn stop_seat(
    herdr: &mut dyn HerdrAct,
    profiles: &BTreeMap<String, Profile>,
    seat: &gear_store::SeatRow,
    pane: &str,
) -> bool {
    crate::names::ensure(herdr, pane, &crate::names::desired(seat));
    // Interrupt the turn that is burning the machine; an idle agent ignores it.
    let _ = herdr.send_keys(pane, &["esc".into()]);
    let Some(profile) = profiles.get(&seat.profile) else {
        return false;
    };
    if let Some(command) = profile.exit_command.as_deref() {
        let Some(name) = crate::names::live_name(herdr, pane) else {
            return false;
        };
        return matches!(herdr.prompt(&name, command), gear_herdr::Outcome::Ok(()));
    }
    if profile.exit_keys.is_empty() {
        return false;
    }
    matches!(
        herdr.send_keys(pane, &profile.exit_keys),
        gear_herdr::Outcome::Ok(())
    )
}

/// Open (or keep) the captain's item for a machine that is short without one
/// of our seats being the cause.
fn open_machine_item(
    store: &mut Store,
    project: &str,
    why: &str,
    machine: &Machine,
    top: &[Proc],
    now: Ts,
    watch: &mut Watch,
) -> GearResult<()> {
    let Some(captain) = store.read(|tx| tx.captain_of(project))? else {
        return Ok(());
    };
    let top_lines: Vec<String> = top
        .iter()
        .take(5)
        .map(|proc| {
            format!(
                "  {} {:.0}% of one core, {} MB, {}",
                proc.pid,
                proc.cpu_pct,
                proc.rss_kb / 1024,
                proc.command
            )
        })
        .collect();
    let body = format!(
        "the machine is short and no seat of this fleet is the reason: {why}. Load {:.2} on {} cores, {} MB free of {} MB. The top processes (never touched; the captain finds the cause):\n{}",
        machine.load5,
        machine.cores,
        machine.mem_free_kb / 1024,
        machine.mem_total_kb / 1024,
        top_lines.join("\n")
    );
    let key = machine_alert_key(project);
    let existing = store
        .read(|tx| tx.get_meta(&key))?
        .filter(|value| !value.is_empty());
    if let Some(id) = existing {
        if let Some(item) = store.read(|tx| tx.get_message(project, &id))? {
            if matches!(item.state, MsgState::Open | MsgState::Waiting) && item.body != body {
                store.write(|tx| tx.update_message_body(project, &id, &body))?;
                watch.items.push("machine:updated".into());
            }
        }
        return Ok(());
    }
    let message = store.write(|tx| {
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.into(),
            thread: machine_thread(project),
            task: None,
            attempt: None,
            sender: "engine".into(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: true,
            body,
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })
    })?;
    store.write(|tx| tx.set_meta(&key, &message.id))?;
    watch.items.push("machine:opened".into());
    Ok(())
}

/// The machine is fine again: resolve the external item.
fn resolve_machine_item(
    store: &mut Store,
    project: &str,
    now: Ts,
    watch: &mut Watch,
) -> GearResult<()> {
    let key = machine_alert_key(project);
    let id = store
        .read(|tx| tx.get_meta(&key))?
        .filter(|value| !value.is_empty());
    let Some(id) = id else {
        return Ok(());
    };
    if let Some(item) = store.read(|tx| tx.get_message(project, &id))? {
        if matches!(item.state, MsgState::Open | MsgState::Waiting) {
            store.write(|tx| {
                tx.update_message_state(
                    project,
                    &id,
                    MsgState::Resolved,
                    Some("the machine is back within the safe line"),
                    None,
                    None,
                    now,
                )
            })?;
            watch.items.push("machine:resolved".into());
        }
    }
    store.write(|tx| tx.set_meta(&key, ""))?;
    Ok(())
}

/// A watched seat is back under the safe line: resolve its item and clear the
/// stop mark, so a later spell stops it again.
fn resolve_seat_item(
    store: &mut Store,
    project: &str,
    seat: &str,
    now: Ts,
    watch: &mut Watch,
) -> GearResult<()> {
    let stop_key = seat_stop_key(seat);
    let stopped = store
        .read(|tx| tx.get_meta(&stop_key))?
        .is_some_and(|value| !value.is_empty());
    if !stopped {
        return Ok(());
    }
    if let Some(item) = store.read(|tx| {
        Ok(tx
            .thread_messages(project, &seat_thread(seat))?
            .into_iter()
            .rev()
            .find(|item| matches!(item.state, MsgState::Open | MsgState::Waiting)))
    })? {
        store.write(|tx| {
            tx.update_message_state(
                project,
                &item.id,
                MsgState::Resolved,
                Some("the seat is back under the safe line"),
                None,
                None,
                now,
            )
        })?;
        watch.items.push(format!("{seat}:resolved"));
    }
    store.write(|tx| tx.set_meta(&stop_key, ""))?;
    Ok(())
}

// ---- the production reader -------------------------------------------------

/// The real reader: one `ps` per sample, the machine's load, memory and the
/// watched volumes. It keeps the previous process table to name the top
/// processes by their CPU use since the last sample.
#[derive(Debug, Default)]
pub struct SystemRead {
    previous: std::sync::Mutex<(BTreeMap<u32, (f64, Ts)>, Ts)>,
}

impl SystemRead {
    pub fn new() -> SystemRead {
        SystemRead::default()
    }
}

impl Read for SystemRead {
    fn sample(
        &self,
        panes: &BTreeMap<String, PaneProcs>,
        volumes: &[String],
    ) -> GearResult<Reading> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as Ts;
        let table = system_ps().ok_or_else(|| {
            gear_model::GearError::new(gear_model::ErrorCode::BadFormat, "ps could not be read")
        })?;
        let machine = system_machine(volumes);
        let mut reading = Reading {
            machine,
            ..Default::default()
        };
        // Each seat's tree: its pane's shell and foreground processes, and
        // every child of them.
        for (seat, pane) in panes {
            let tree = seat_tree(&table, pane);
            let mut procs = SeatProcs::default();
            for index in &tree {
                procs.cpu_s += table[*index].cpu_s;
                procs.rss_kb += table[*index].rss_kb;
                procs.procs += 1;
            }
            reading.seats.insert(seat.clone(), procs);
        }
        let fleet: BTreeSet<u32> = panes
            .values()
            .flat_map(|pane| seat_tree(&table, pane))
            .map(|index| table[index].pid)
            .collect();
        // The top processes outside the fleet, by the CPU they used since the
        // last sample (or the ps reading on the first one).
        let (previous, previous_at) = {
            let guard = self.previous.lock().unwrap_or_else(|e| e.into_inner());
            (guard.0.clone(), guard.1)
        };
        let interval = (now - previous_at).max(1) as f64;
        let mut top: Vec<Proc> = table
            .iter()
            .filter(|proc| !fleet.contains(&proc.pid))
            .map(|proc| {
                let cpu_pct = match previous.get(&proc.pid) {
                    Some((cpu, _)) => (proc.cpu_s - cpu).max(0.0) / interval * 100.0,
                    None => proc.cpu_pct,
                };
                Proc {
                    pid: proc.pid,
                    cpu_pct,
                    rss_kb: proc.rss_kb,
                    command: proc.command.clone(),
                }
            })
            .collect();
        top.sort_by(|a, b| {
            b.cpu_pct
                .partial_cmp(&a.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.rss_kb.cmp(&a.rss_kb))
        });
        top.truncate(20);
        reading.top = top;
        {
            let mut guard = self.previous.lock().unwrap_or_else(|e| e.into_inner());
            guard.0 = table
                .iter()
                .map(|proc| (proc.pid, (proc.cpu_s, now)))
                .collect();
            guard.1 = now;
        }
        Ok(reading)
    }
}

/// A reader that reads nothing: the engine's own tests use it unless they
/// inject their own readings, so a test never reads the machine it runs on.
#[derive(Debug, Default)]
pub struct Silent;

impl Read for Silent {
    fn sample(
        &self,
        _panes: &BTreeMap<String, PaneProcs>,
        _volumes: &[String],
    ) -> GearResult<Reading> {
        Ok(Reading::default())
    }
}

/// One process of the machine's table, as `ps` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct PsProc {
    pid: u32,
    ppid: u32,
    pgid: u32,
    /// Cumulative CPU seconds.
    cpu_s: f64,
    /// The `ps` %cpu reading (the first sample's fallback for the top list).
    cpu_pct: f64,
    rss_kb: u64,
    command: String,
}

/// The processes under a seat's pane: its shell and foreground processes and
/// every child of them, plus the foreground process group (a build that left
/// the shell's tree by its own session).
fn seat_tree(table: &[PsProc], pane: &PaneProcs) -> Vec<usize> {
    let mut children: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, proc) in table.iter().enumerate() {
        children.entry(proc.ppid).or_default().push(index);
    }
    let mut wanted: BTreeSet<u32> = BTreeSet::new();
    let mut queue: Vec<u32> = Vec::new();
    if let Some(shell) = pane.shell {
        queue.push(shell);
    }
    queue.extend(pane.foreground.iter().copied());
    while let Some(pid) = queue.pop() {
        if !wanted.insert(pid) {
            continue;
        }
        if let Some(kids) = children.get(&pid) {
            for index in kids {
                queue.push(table[*index].pid);
            }
        }
    }
    let mut tree: Vec<usize> = Vec::new();
    for (index, proc) in table.iter().enumerate() {
        let in_group = pane
            .foreground_group
            .is_some_and(|group| proc.pgid == group && group != proc.pid);
        if wanted.contains(&proc.pid) || in_group {
            tree.push(index);
        }
    }
    tree
}

fn system_ps() -> Option<Vec<PsProc>> {
    let text = run("ps", &["-axo", "pid=,ppid=,pgid=,pcpu=,rss=,time=,comm="])?;
    Some(parse_ps(&text))
}

/// Parse `ps -axo pid=,ppid=,pgid=,pcpu=,rss=,time=,comm=`. One line per
/// process; a line that does not parse is skipped.
pub fn parse_ps(text: &str) -> Vec<PsProc> {
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let (Ok(pid), Ok(ppid), Ok(pgid), Ok(cpu_pct), Ok(rss_kb)) = (
            fields[0].parse::<u32>(),
            fields[1].parse::<u32>(),
            fields[2].parse::<u32>(),
            fields[3].parse::<f64>(),
            fields[4].parse::<u64>(),
        ) else {
            continue;
        };
        let Some(cpu_s) = parse_cpu_time(fields[5]) else {
            continue;
        };
        // `comm` may hold spaces: it is the rest of the line.
        let command = fields[6..].join(" ");
        out.push(PsProc {
            pid,
            ppid,
            pgid,
            cpu_s,
            cpu_pct,
            rss_kb,
            command,
        });
    }
    out
}

/// A `ps` CPU time as seconds: macOS `MM:SS.ss` (minutes unbounded), Linux
/// `[[DD-]HH:]MM:SS`.
pub fn parse_cpu_time(text: &str) -> Option<f64> {
    let (days, rest) = match text.split_once('-') {
        Some((days, rest)) => (days.parse::<f64>().ok()?, rest),
        None => (0.0, text),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let seconds = match parts.as_slice() {
        [minutes, seconds] => minutes.parse::<f64>().ok()? * 60.0 + seconds.parse::<f64>().ok()?,
        [hours, minutes, seconds] => {
            hours.parse::<f64>().ok()? * 3600.0
                + minutes.parse::<f64>().ok()? * 60.0
                + seconds.parse::<f64>().ok()?
        }
        _ => return None,
    };
    Some(days * 86400.0 + seconds)
}

fn system_machine(volumes: &[String]) -> Machine {
    let cores = system_cores();
    let load5 = system_load5();
    let (mem_total_kb, mem_free_kb) = system_memory();
    let disk = volumes
        .iter()
        .filter_map(|volume| disk_free_kb(volume).map(|free| (volume.clone(), free)))
        .collect();
    Machine {
        cores,
        load5,
        mem_total_kb,
        mem_free_kb,
        disk,
    }
}

fn run(command: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn system_cores() -> u32 {
    if let Some(text) = run("sysctl", &["-n", "hw.ncpu"]) {
        if let Ok(cores) = text.trim().parse::<u32>() {
            return cores;
        }
    }
    if let Some(text) = run("nproc", &[]) {
        if let Ok(cores) = text.trim().parse::<u32>() {
            return cores;
        }
    }
    if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
        let cores = text
            .lines()
            .filter(|line| line.starts_with("processor"))
            .count();
        if cores > 0 {
            return cores as u32;
        }
    }
    1
}

fn system_load5() -> f64 {
    if let Some(text) = run("sysctl", &["-n", "vm.loadavg"]) {
        let numbers: Vec<f64> = text
            .trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace())
            .split_whitespace()
            .filter_map(|value| value.parse::<f64>().ok())
            .collect();
        if numbers.len() >= 2 {
            return numbers[1];
        }
    }
    if let Ok(text) = std::fs::read_to_string("/proc/loadavg") {
        let numbers: Vec<f64> = text
            .split_whitespace()
            .take(3)
            .filter_map(|value| value.parse::<f64>().ok())
            .collect();
        if numbers.len() >= 2 {
            return numbers[1];
        }
    }
    0.0
}

fn system_memory() -> (u64, u64) {
    // macOS: the installed memory, and the pages that are free or reclaimable
    // (free + inactive + speculative); Linux: MemTotal and MemAvailable.
    if let Some(text) = run("sysctl", &["-n", "hw.memsize"]) {
        if let Ok(bytes) = text.trim().parse::<u64>() {
            let total_kb = bytes / 1024;
            let free_kb = run("vm_stat", &[])
                .map(|text| parse_vm_stat(&text))
                .unwrap_or(0);
            return (total_kb, free_kb);
        }
    }
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        let field = |name: &str| -> u64 {
            text.lines()
                .find(|line| line.starts_with(name))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0)
        };
        return (field("MemTotal:"), field("MemAvailable:"));
    }
    (0, 0)
}

/// The reclaimable memory from `vm_stat`, in KB: free, inactive and
/// speculative pages at the page size the header names.
pub fn parse_vm_stat(text: &str) -> u64 {
    let mut page_size: u64 = 0;
    let mut pages: u64 = 0;
    for line in text.lines() {
        if let Some(start) = line.find("page size of ") {
            let rest = &line[start + "page size of ".len()..];
            page_size = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
        }
        for name in ["Pages free:", "Pages inactive:", "Pages speculative:"] {
            if let Some(rest) = line.strip_prefix(name) {
                let value = rest.trim().trim_end_matches('.');
                pages += value.parse::<u64>().unwrap_or(0);
            }
        }
    }
    pages.saturating_mul(page_size) / 1024
}

fn disk_free_kb(volume: &str) -> Option<u64> {
    let text = run("df", &["-k", volume])?;
    parse_df(&text)
}

/// The free KB from `df -k`'s last line (the `Available` column).
pub fn parse_df(text: &str) -> Option<u64> {
    let line = text.lines().rev().find(|line| !line.trim().is_empty())?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    // Filesystem 1024-blocks Used Available Capacity ...
    if fields.len() < 4 {
        return None;
    }
    fields[3].parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_lines_are_parsed_with_their_cpu_time() {
        let table = parse_ps(
            "  501     1   501   2.5  10240 1:05.70 /usr/bin/thing --flag\n\
             bad line\n\
             700   501   501  10.0  4096 0:03.42 /bin/sh -c build",
        );
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].pid, 501);
        assert_eq!(table[0].ppid, 1);
        assert_eq!(table[0].pgid, 501);
        assert_eq!(table[0].cpu_s, 65.7);
        assert_eq!(table[0].rss_kb, 10240);
        assert_eq!(table[0].command, "/usr/bin/thing --flag");
        assert_eq!(table[1].cpu_s, 3.42);
    }

    #[test]
    fn cpu_times_of_both_platforms_are_seconds() {
        assert_eq!(parse_cpu_time("1:05.70"), Some(65.7));
        assert_eq!(parse_cpu_time("727:51.33"), Some(727.0 * 60.0 + 51.33));
        assert_eq!(parse_cpu_time("02:03:04"), Some(7384.0));
        assert_eq!(parse_cpu_time("1-02:03:04"), Some(86400.0 + 7384.0));
        assert_eq!(parse_cpu_time("nonsense"), None);
    }

    #[test]
    fn the_vm_stat_free_memory_counts_reclaimable_pages() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
            Pages free: 100.\n\
            Pages active: 500.\n\
            Pages inactive: 200.\n\
            Pages speculative: 50.\n";
        // (100 + 200 + 50) pages * 16 KB.
        assert_eq!(parse_vm_stat(text), 350 * 16);
    }

    #[test]
    fn df_gives_the_available_column() {
        let text = "Filesystem   1024-blocks      Used  Available Capacity  Mounted on\n\
            /dev/disk7s1  4000581936 462866140 3537233272    12%  /Volumes/Data\n";
        assert_eq!(parse_df(text), Some(3537233272));
    }

    #[test]
    fn a_seat_tree_is_the_shell_and_its_children() {
        let table = vec![
            PsProc {
                pid: 100,
                ppid: 1,
                pgid: 100,
                cpu_s: 1.0,
                cpu_pct: 1.0,
                rss_kb: 1000,
                command: "shell".into(),
            },
            PsProc {
                pid: 101,
                ppid: 100,
                pgid: 100,
                cpu_s: 20.0,
                cpu_pct: 20.0,
                rss_kb: 2000,
                command: "child".into(),
            },
            PsProc {
                pid: 102,
                ppid: 101,
                pgid: 100,
                cpu_s: 30.0,
                cpu_pct: 30.0,
                rss_kb: 3000,
                command: "grandchild".into(),
            },
            PsProc {
                pid: 200,
                ppid: 1,
                pgid: 200,
                cpu_s: 5.0,
                cpu_pct: 5.0,
                rss_kb: 500,
                command: "other".into(),
            },
        ];
        let pane = PaneProcs {
            shell: Some(100),
            foreground: vec![],
            foreground_group: Some(100),
        };
        let tree = seat_tree(&table, &pane);
        assert_eq!(tree, vec![0, 1, 2]);
    }

    #[test]
    fn the_short_line_is_the_load_memory_or_disk_line() {
        let limits = Limits::default();
        let fine = Machine {
            cores: 10,
            load5: 10.0,
            mem_total_kb: 16 * 1024 * 1024,
            mem_free_kb: 8 * 1024 * 1024,
            disk: vec![("/state".into(), 50 * 1024 * 1024)],
        };
        assert!(short_reason(&fine, &limits).is_none());
        let load = Machine {
            load5: 21.0,
            ..fine.clone()
        };
        assert!(short_reason(&load, &limits).unwrap().why.contains("load"));
        let memory = Machine {
            mem_free_kb: 1024 * 1024,
            ..fine.clone()
        };
        assert!(short_reason(&memory, &limits)
            .unwrap()
            .why
            .contains("free memory"));
        let disk = Machine {
            disk: vec![("/state".into(), 1024 * 1024)],
            ..fine
        };
        assert!(short_reason(&disk, &limits).unwrap().why.contains("volume"));
    }
}
