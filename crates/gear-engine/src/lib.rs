//! GEAR engine (design/08). The only code that performs Herdr side effects.
//!
//! One [`tick`]:
//! 0. onboard: every queued lane gets its worktree, pane and agent
//!    ([`onboard::onboard_queued`]);
//! 1. observe: one `agents()` call → per-seat [`observe::SeatObs`]
//!    (observation failure ⇒ no decisions for that project this tick);
//! 2. confirm: settle Submitted/Unknown ops using ViewRecords as evidence;
//! 3. decide (pure): doorbells (slice 1), dispatch/continue (slice 2), …;
//! 4. intend: write ops (Intended) in one transaction, fixing generation etc.;
//! 5. execute: re-check each op against current state (stale ⇒ Void), call
//!    Herdr outside any transaction, then record Submitted/Failed/Unknown.
//!
//! The binary loops `tick` every `cfg.tick_s` seconds. Tests call `tick`
//! directly with an injected `now` and a `FakeHerdr`.

mod attention;
pub mod audit;
pub mod blocked;
pub mod busy;
pub mod captain_quota;
pub mod composer;
pub mod delivery;
pub mod dialog;
pub mod dispatch;
pub mod doorbell;
pub mod feed;
pub mod friction;
pub mod guardrails;
pub mod handover;
pub mod health;
pub mod hours;
pub mod lane_nudge;
pub mod merge;
pub mod model;
pub mod names;
pub mod observe;
pub mod onboard;
mod opencode;
pub mod ops;
pub mod overrun;
pub mod parallel;
pub mod quiet;
pub mod quota;
pub mod rebind;
pub mod release;
pub mod restart;
pub mod review;
pub mod rounds;
mod rules;
pub mod session;
mod session_runner;
pub mod silent;
pub mod stall;
pub mod start;
pub mod switch;
pub mod tailed;
pub mod trust;
pub mod waits;

use gear_herdr::HerdrAct;
use gear_model::{GearResult, Ts};
use gear_store::Store;

#[derive(Debug, Clone)]
pub struct EngineCfg {
    pub tick_s: i64,
    /// Minimum seconds between two doorbells/deliveries to the same seat after
    /// the previous one was settled (design/09). Must be > tick_s.
    pub cooldown_s: i64,
    /// Ticks a Submitted/Unknown doorbell may wait for evidence before the seat
    /// is flagged "needs check" (no resend either way).
    pub doorbell_check_budget: i64,
    /// How long a delivery waits for the lane's check-in, in wall-clock
    /// seconds (design §10.3: `timing.delivery_grace_s`, default 180). The
    /// window is not a number of checks: a lane that reads its packet after a
    /// minute still confirms it.
    pub delivery_grace_s: i64,
    /// Observation profiles by name (seat.profile). Unknown profile name ⇒ the
    /// seat is not observed as idle (fail closed) and is flagged.
    pub profiles: std::collections::BTreeMap<String, gear_herdr::Profile>,
    /// The store path agents get as `GEAR_DB` when onboarded (slice 3).
    pub db_path: String,
    /// Settle review verdicts without the captain (T-13). OFF by default
    /// (L-57): until the captain's gate (T-23: blocking vs logged findings,
    /// a round cap) and the engine merge (T-12) exist, an automatic pass marks
    /// a task done that is not merged, and an automatic fail sends back every
    /// finding round after round. With it off verdicts remain stored without automatic settlement.
    pub auto_settle: bool,
    /// Background merge jobs shared across ticks and configuration clones.
    pub merges: merge::MergeWorkers,
    /// How many ticks a start refused with `agent_pane_busy` may wait for its
    /// shell before the seat parks (T-95). One tick is one attempt: a slow
    /// shell is retried once per tick on the same binding and never holds the
    /// tick, so a busy pane no longer delays the lanes queued behind it.
    pub pane_busy_ticks: i64,
    /// How long an OpenCode lane waits for its directory's model list to load
    /// before it parks (T-67), and how long between two looks.
    pub model_list_wait_s: u64,
    pub model_list_poll_ms: u64,
    /// The one runner the merge uses to touch the remote (T-107): the push of
    /// the integration branch and the branch deletes. Production is
    /// `merge::GitRemote`; the engine's tests use `merge::FetchRemote`, which
    /// moves the refs from the remote's own side, so no test runs `git push`.
    pub remote: std::sync::Arc<dyn merge::Remote>,
    /// Executor for review worktree Git operations.
    pub review_git: std::sync::Arc<dyn review::ReviewGit>,
    /// The machine and process reader of the health watch (T-472). Production
    /// is `health::SystemRead`; a test injects its own readings.
    pub health: std::sync::Arc<dyn health::Read>,
    /// The wait between two attempts of a merge that failed for a reason that
    /// can clear: the engine sleeps, doubling from 2 s up to 30 s (T-61). A
    /// field so a test injects a hook that never touches the clock (T-127).
    pub merge_retry_wait: merge::RetryWait,
}

impl Default for EngineCfg {
    fn default() -> Self {
        EngineCfg {
            tick_s: 4,
            cooldown_s: 90,
            doorbell_check_budget: 3,
            delivery_grace_s: 180,
            profiles: Default::default(),
            db_path: String::new(),
            auto_settle: false,
            merges: Default::default(),
            pane_busy_ticks: start::MAX_TICKS,
            model_list_wait_s: 60,
            model_list_poll_ms: 500,
            remote: std::sync::Arc::new(merge::GitRemote),
            review_git: std::sync::Arc::new(review::RealReviewGit),
            health: std::sync::Arc::new(health::Silent),
            merge_retry_wait: merge::RetryWait::backoff(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Projects whose observation failed this tick (nothing decided for them).
    pub observe_failed: Vec<String>,
    /// Projects one of whose per-project steps failed this tick, one line each
    /// (`<project>: <code>: <detail>`; T-243). The tick goes on with the other
    /// projects: one project's bad row or config never stops the rest.
    pub project_failed: Vec<String>,
    /// Op ids that went Intended this tick.
    pub intended: Vec<String>,
    /// (op id, resulting state) for ops executed this tick.
    pub executed: Vec<(String, String)>,
    /// Op ids confirmed by evidence this tick.
    pub confirmed: Vec<String>,
    /// Attempts confirmed by a **late** check-in (design §10.3): the delivery
    /// op had already failed, and the view that showed the attempt came after.
    pub late_confirmed: Vec<String>,
    /// Deliver op ids whose text sat unsent in their lane's input box and got
    /// its Enter this tick (T-371).
    pub resubmitted: Vec<String>,
    /// Seats flagged for attention (e.g. doorbell evidence budget exhausted).
    pub flagged: Vec<String>,
    /// Seats onboarded this tick (worktree, pane and agent all up).
    pub onboarded: Vec<String>,
    /// Quota switches and parks this tick, one line each (design/01 `on_quota`).
    pub quota: Vec<String>,
    /// Working-hours notices, parks, releases and comebacks this tick, one
    /// line each (T-450).
    pub hours: Vec<String>,
    /// The lanes that were told to go on after they stopped (T-206).
    pub lane_nudged: Vec<String>,
    /// The lanes whose plan park ended this tick (T-190), one line each.
    pub park_ended: Vec<String>,
    /// `project:task/attempt` of the review verdicts that settled a task this
    /// tick (T-13).
    pub settled: Vec<String>,
    /// `project:task/attempt: CODE: detail` of the review verdicts whose
    /// settlement failed this tick. Reported, never fatal (review R-67).
    pub settle_failed: Vec<String>,
    /// Reviewer seats started this tick (a lane handed a result in).
    pub reviewers: Vec<String>,
    /// Reviewer seats released this tick (their task left review).
    pub released: Vec<String>,
    /// Reviewer seats released this tick because they run no reviewer
    /// (B02-04): a dead seat that held a temp slot.
    pub dead_temps: Vec<String>,
    /// Tasks merged into the integration branch this tick (`task @ branch`).
    pub merged: Vec<String>,
    /// Lane items for the captain opened, updated or closed this tick
    /// (`lane:action`), from the keep-lanes-fed watch.
    pub feed: Vec<String>,
    /// The all-tail item opened or closed this tick (`all-tail:opened|closed`):
    /// every lane in the tail while required work remains (B02-08).
    pub tail: Vec<String>,
    /// The friction item opened, updated or closed this tick
    /// (`friction:opened|updated|closed`): the captain hears about the open
    /// friction reports when it has nothing else to do (design §6.4).
    pub friction: Vec<String>,
    /// Station message ids opened at ETA/task checkpoints (`project:M-n`).
    pub stations: Vec<String>,
    /// Lanes alerted for being Working without their own development attempt.
    pub busy_lanes: Vec<String>,
    /// Seats rung for being blocked by a dialog for too long (lesson 7b).
    pub blocked: Vec<String>,
    /// Seats whose dialog the engine closed this tick (T-468): a lane's or a
    /// reviewer's at once, a captain's after ten unanswered minutes.
    pub dialogs: Vec<String>,
    /// The health watch's actions (T-472): the seats it stopped, and the
    /// captain items it opened or resolved.
    pub health: Vec<String>,
    /// Seats whose own guardrails refused one of their actions (T-113).
    pub guardrails: Vec<String>,
    /// Seats whose agent was renamed back to its seat name this tick (T-134).
    pub renamed: Vec<String>,
    /// Seats whose name another pane holds (reported, never stolen; T-134).
    pub name_taken: Vec<String>,
    /// `seat: reason` of a name the engine could neither read nor give back
    /// (T-134).
    pub name_failed: Vec<String>,
    /// Seats that wait for their provider (T-75): a provider back-off is not
    /// a stall, and the home page shows it.
    pub backoff: Vec<String>,
    /// Parallel-change warning message ids (`project:M-n`).
    pub parallel: Vec<String>,
    /// Git evidence that could not be read during a parallel-change scan.
    pub parallel_failed: Vec<String>,
    /// `project:task/attempt` of the voyage audits handed in for review this
    /// tick (T-505).
    pub audits: Vec<String>,
    /// `project:task: CODE: detail` of the audits that could not be handed in
    /// this tick; reported, never fatal.
    pub audit_failed: Vec<String>,
    /// The rounds that ended or learned a session this tick (design §4.1).
    pub rounds: crate::rounds::RoundReport,
    /// The steps of this tick that took over 200 ms, as `<step>: <ms> ms`
    /// (T-386): a tick that costs is named, so a step that grows is seen
    /// before it makes the engine late.
    pub slow: Vec<String>,
}

/// One line of [`TickReport::project_failed`]: `<project>: <code>: <detail>`
/// (T-243).
fn project_error(project: &str, error: &gear_model::GearError) -> String {
    format!("{project}: {}: {}", error.code, error.detail)
}

/// The time past which a tick step is named in [`TickReport::slow`] (T-386).
const SLOW_STEP_MS: u128 = 200;

/// Name a tick step that took longer than the budget (T-386):
/// `<step>: <ms> ms`, printed with the rest of the report.
fn note_slow(report: &mut TickReport, step: &str, started: std::time::Instant) {
    let ms = started.elapsed().as_millis();
    if ms > SLOW_STEP_MS {
        report.slow.push(format!("{step}: {ms} ms"));
    }
}

/// Run one engine tick over every project in the store.
pub fn tick(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    now: Ts,
) -> GearResult<TickReport> {
    let mut report = TickReport::default();

    // Settle recorded lane rebinds before onboarding or dispatch sees config.
    // T-243: a project whose step fails is reported and the other projects go
    // on; one bad project never stops the tick.
    // T-386: every step of the tick is timed, and one over 200 ms is named.
    let step = std::time::Instant::now();
    for project in store.read(|tx| tx.project_ids())? {
        if let Err(error) = gear_app::plan::reconcile_rebinds(store, &project) {
            report.project_failed.push(project_error(&project, &error));
        }
    }
    note_slow(&mut report, "reconcile rebinds", step);

    let step = std::time::Instant::now();
    rebind::tick(store, herdr, cfg, now)?;
    note_slow(&mut report, "rebind", step);

    // 0. onboard: every queued lane gets its worktree, pane and agent. A seat
    //    that fails a step is parked in NeedsCheck, never retried blindly.
    let step = std::time::Instant::now();
    report.onboarded = onboard::onboard_queued(store, herdr, cfg, now)?;
    note_slow(&mut report, "onboard", step);

    // 0b. settle: a review verdict that arrived settles its task without the
    //     captain (T-13) — before the dispatch below, so a pass frees the
    //     task's dependants. Failing verdicts wait for the captain's judgement.
    //     Settlement needs no Herdr, so it runs even
    //     when the observation fails.
    if cfg.auto_settle {
        let step = std::time::Instant::now();
        let (settled, settle_failed) = settle_reviews(store, now)?;
        report.settled = settled;
        report.settle_failed = settle_failed;
        note_slow(&mut report, "settle reviews", step);
    }

    // Reconcile waits before dispatch and stall decisions in this tick.
    let step = std::time::Instant::now();
    waits::tick(store, herdr, cfg, now)?;
    note_slow(&mut report, "waits", step);
    let projects = store.read(|tx| tx.project_ids())?;
    let step = std::time::Instant::now();
    for project in &projects {
        match parallel::check(store, project, now) {
            Ok(checked) => {
                report.parallel.extend(
                    checked
                        .opened
                        .into_iter()
                        .map(|id| format!("{project}:{id}")),
                );
                report.parallel_failed.extend(
                    checked
                        .problems
                        .into_iter()
                        .map(|error| format!("{project}: {error}")),
                );
            }
            Err(error) => report.project_failed.push(project_error(project, &error)),
        }
    }
    note_slow(&mut report, "parallel check", step);
    // The GitHub side of a project in GitHub mode (T-91): every call is an op,
    // so an outage queues them and never stops the tick.
    let step = std::time::Instant::now();
    for project in &projects {
        if let Err(error) = gear_forge::sync(store, project, now) {
            report.project_failed.push(project_error(project, &error));
        }
    }
    note_slow(&mut report, "forge sync", step);
    // Station facts need no Herdr observation. Enqueue them before deciding
    // doorbells, including when a previous outage delayed a checkpoint.
    let step = std::time::Instant::now();
    for project in &projects {
        match store.write(|tx| gear_app::station::checkpoints(tx, project, now)) {
            Ok(ids) => report
                .stations
                .extend(ids.into_iter().map(|id| format!("{project}:{id}"))),
            Err(error) => report.project_failed.push(project_error(project, &error)),
        }
    }
    note_slow(&mut report, "stations", step);

    // 1. observe: one agents() call for the whole session. If it fails, every
    //    project is marked and nothing at all is decided this tick.
    let step = std::time::Instant::now();
    let agents = observe::snapshot(herdr);
    // T-458: a failed observation is timed too. It used to return before the
    // step was named, so a slow Herdr read that failed was missing from the
    // tick's slow-step report (T-386 review F1).
    note_slow(&mut report, "observe", step);
    let Ok(agents) = agents else {
        // No evidence of continuous work across a failed observation.
        for project in &projects {
            store.write(|tx| busy::observation_lost(tx, project))?;
        }
        report.observe_failed = projects;
        return Ok(report);
    };

    // 0c. names (T-134): Herdr clears an agent's name when it re-detects the
    //     occupant, and a prompt to an unnamed agent is refused. Read every
    //     agent's name once, then give each seat its name back before anything
    //     is delivered this tick. A failed read is not an observation failure:
    //     the deliveries below still make sure of the name they prompt.
    let step = std::time::Instant::now();
    let names = match names::read(herdr) {
        Ok(names) => names,
        Err(error) => {
            report.name_failed.push(format!("agent.list: {}", error.0));
            std::collections::BTreeMap::new()
        }
    };
    note_slow(&mut report, "names", step);

    let mut reviewer_nudge_flags = Vec::new();
    // T-487: the doorbell inputs of every project, gathered per project and
    // joined by pane after the project loop: one ring for a pane that holds
    // several captain seats.
    let mut ring_inputs: Vec<doorbell::DoorbellInput> = Vec::new();
    // T-386: one read of a repository's branch tips per tick, shared by the
    // push clock and the review step.
    let mut tips_by_repo = review::RepoTips::new();
    let mut missing_seats = std::collections::BTreeSet::new();
    let mut present_seats = Vec::new();
    let mut observed_seats = Vec::new();
    // T-243: the steps of one project run in `one_project`, once per project.
    // A project whose step fails is reported in `report.project_failed` and the
    // other projects run all the same: several projects share one engine, so
    // one project's bad row or config never stops the other projects' tick.
    let mut one_project =
        |store: &mut Store, report: &mut TickReport, id: &str| -> GearResult<()> {
            let project = id.to_string();
            // 0d. every seat with an active binding carries its name (`<project>-
            //     <seat>`, 17 §2/§3). A name another pane holds is reported to the
            //     captain once, never stolen.
            if !names.is_empty() {
                let step = std::time::Instant::now();
                let restored = names::restore(store, herdr, &project, &names, now)?;
                report.renamed.extend(restored.renamed);
                report.name_taken.extend(restored.taken);
                report.name_failed.extend(restored.failed);
                note_slow(report, "names restore", step);
            }

            // Seat observations (reads screen tails; no write transaction is held).
            // The profile table of this project: the built-ins with the project's
            // own `[profiles.<name>]` on top (T-31). A config that does not parse
            // leaves the built-ins in place (the lanes are parked by the onboard
            // step instead); a profile block the engine cannot read is this
            // project's failure this tick (T-243), reported in
            // `report.project_failed`, never the tick's.
            let step = std::time::Instant::now();
            let profiles = match gear_app::config::latest(store, &project) {
                Ok(config) => gear_app::config::profile_table(&cfg.profiles, &config)?,
                Err(_) => cfg.profiles.clone(),
            };
            let observed =
                store.read(|tx| observe::reconcile(tx, herdr, &project, &agents, &profiles))?;
            for obs in &observed {
                observed_seats.push((
                    project.clone(),
                    obs.seat.id.clone(),
                    obs.presence == observe::Presence::Present,
                ));
                if obs.presence == observe::Presence::Missing {
                    missing_seats.insert(obs.seat.id.clone());
                } else if obs.presence == observe::Presence::Present {
                    present_seats.push((project.clone(), obs.seat.id.clone()));
                }
            }
            // A hand restart of an agent is a new session in the same generation
            // (T-97): the binding follows what the pane reports, so a delivery that
            // was never confirmed can be sent again without a store edit.
            observe::record_sessions(store, &observed)?;
            note_slow(report, "observe seats", step);
            // A park whose time has come ends (T-190): the lane the captain
            // parked until a time that has passed works again.
            let step = std::time::Instant::now();
            let ended = store
                .write(|tx| gear_app::plan::end_parked_lanes(tx, &project, Some(&agents), now))?;
            report.park_ended.extend(ended);
            report
                .busy_lanes
                .extend(store.write(|tx| busy::check(tx, &project, &observed, now))?);
            note_slow(report, "park end and busy", step);
            // T-288: a lane still busy with no task five minutes on is interrupted,
            // so its next packet can go. A lane that is helping is never touched.
            let step = std::time::Instant::now();
            for pane in store.write(|tx| busy::due_interrupts(tx, &project, &observed, now))? {
                let _ = herdr.send_keys(&pane, &["esc".into()]);
            }
            note_slow(report, "busy interrupt", step);
            // A dialog never holds a seat (M6 S2, T-468): a lane's or a
            // reviewer's blocking screen is closed in the same tick and the
            // seat is told to ask the captain instead; a captain's dialog
            // nobody answers is cancelled after ten minutes.
            let step = std::time::Instant::now();
            report
                .dialogs
                .extend(dialog::check(store, herdr, &observed, &profiles, now)?);
            note_slow(report, "dialogs", step);
            // The fleet and machine health watch (T-472): once a minute, the
            // seats' own CPU and memory and the machine's load, memory and
            // disk. A seat over the safe line is stopped and the captain told;
            // a machine short for another reason gets one urgent item naming
            // the top processes. Between samples this is one meta read.
            let step = std::time::Instant::now();
            let health = health::check(store, herdr, cfg, &project, &observed, &profiles, now)?;
            report
                .health
                .extend(health.stopped.iter().map(|seat| format!("{seat}:stopped")));
            report.health.extend(health.items);
            report.health.extend(health.notes);
            note_slow(report, "health", step);
            // A seat blocked by a dialog for minutes reaches the captain (lesson
            // 7b): the agent waits on a dialog nobody sees.
            let step = std::time::Instant::now();
            report
                .blocked
                .extend(store.write(|tx| blocked::check(tx, &project, &observed, now))?);
            note_slow(report, "blocked", step);
            // A lane refused by its own guardrails (pi: @aliou/pi-guardrails) goes
            // on with the refusal in its transcript; the captain is told (T-113).
            let step = std::time::Instant::now();
            report
                .guardrails
                .extend(store.write(|tx| guardrails::check(tx, &project, &observed, now))?);
            note_slow(report, "guardrails", step);

            // T-75: a seat whose provider is in a back-off waits for the provider.
            // It is not stuck, so no stall item is raised for it, and the captain's
            // home page shows it as a back-off.
            let step = std::time::Instant::now();
            let waiting = stall::waiting_for_provider(&profiles, &observed);
            store.write(|tx| tx.set_seats_in_backoff(&project, &waiting))?;
            report.backoff.extend(waiting);
            note_slow(report, "backoff", step);

            let step = std::time::Instant::now();
            captain_quota::update(store, herdr, &profiles, &project, &observed, now)?;
            note_slow(report, "captain quota", step);

            // T-209: when an active attempt's agent exited but its bound pane is
            // still a shell, restart it once in place before treating the absence
            // as a stall. Pending starts continue through the same durable restart
            // machine on later ticks.
            let step = std::time::Instant::now();
            let restarted =
                restart_missing_jobs(store, herdr, cfg, &profiles, &project, &observed, now)?;
            missing_seats.retain(|seat| !restarted.contains(seat));
            report.flagged.retain(|seat| !restarted.contains(seat));
            note_slow(report, "restart missing jobs", step);

            // A seat that holds work and has produced nothing for ten minutes
            // reaches the captain (T-227): one alert per round. Its seats stay
            // out of report.flagged: that path sends a second, generic stall
            // alert (review round 1, F1).
            let step = std::time::Instant::now();
            let _ = silent::tick(store, &project, &observed, &profiles, now)?;
            note_slow(report, "silent", step);

            // 1a. a lane or reviewer whose agent is gone from its pane (it exited,
            //     the pane was closed) reaches the captain at once (L-56): the
            //     seat still says Active, so nothing else would ever notice.
            for obs in &observed {
                if matches!(obs.presence, observe::Presence::Missing)
                    && obs.seat.state == gear_model::SeatState::Active
                    && matches!(
                        obs.seat.role,
                        gear_model::SeatRole::Lane | gear_model::SeatRole::Temp
                    )
                    && !report.flagged.contains(&obs.seat.id)
                {
                    report.flagged.push(obs.seat.id.clone());
                }
            }

            // Configuration changes take effect through the same durable in-place
            // restart machinery before quota and dispatch act on the lane.
            let step = std::time::Instant::now();
            restart::reconcile_config(store, herdr, cfg, &profiles, &project, now)?;
            note_slow(report, "restart config", step);

            // 1a. the working hours (T-450): the notice before the end, the park
            //     and release at it, and the comeback at the next start. It runs
            //     before quota, so a lane it parks is not decided on again.
            let step = std::time::Instant::now();
            let hours = hours::tick(store, herdr, cfg, &profiles, &project, now)?;
            report.hours.extend(hours.events);
            note_slow(report, "hours", step);

            // 1b. quota: a lane on a provider limit switches model or parks
            //     (design/01 `[models].on_quota`). It runs before the confirm, so a
            //     lane that is on quota does not have its delivery budget spent.
            let step = std::time::Instant::now();
            let quota =
                quota::apply_quota_policy(store, herdr, cfg, &profiles, &project, &observed, now)?;
            report.quota.extend(quota.events);
            note_slow(report, "quota", step);

            // The loop guard acts on the observed working session before dispatch;
            // its message also resets the ordinary push reminder clock.
            // T-386: its branch read is one `for-each-ref` per repository, timed
            // with the push clock it serves.
            let step = std::time::Instant::now();
            quiet::loops(
                store,
                herdr,
                cfg,
                &mut tips_by_repo,
                &project,
                &observed,
                now,
            )?;
            store.write(|tx| quiet::tick(tx, cfg, &mut tips_by_repo, &project, now))?;
            note_slow(report, "quiet", step);

            // 2. confirm: settle Submitted/Unknown doorbells from view evidence.
            // T-453: first record every seat's agent status and the idle-to-busy
            // turns it takes: a turn after a delivery was submitted is the
            // agent showing it took the packet, and the idle episode starts the
            // window of a text that waited in a working agent's queue.
            let step = std::time::Instant::now();
            let _turns = store.write(|tx| ops::note_agent_turns(tx, &observed, now))?;
            note_slow(report, "agent turns", step);
            // 2a. a delivery whose text sits unsent in its lane's input box gets
            //     its Enter back (T-371): the lane is idle, the packet was typed,
            //     and only the key that submits it was lost. It runs before the
            //     confirm (T-453): the Enter the window finds still needed goes
            //     first, then the one resend, then the captain's item.
            let step = std::time::Instant::now();
            report
                .resubmitted
                .extend(ops::resubmit_unsent(store, herdr, &project, now)?);
            note_slow(report, "resubmit", step);
            let step = std::time::Instant::now();
            let delivery_grace = gear_app::config::latest(store, &project)
                .map(|config| i64::from(config.timing.delivery_grace_s))
                .unwrap_or(cfg.delivery_grace_s);
            // T-213: the seats whose agent Herdr reports working this tick. A
            // doorbell that reached one of them waits behind the agent's turn:
            // that is not a stall, so its check budget is not spent.
            let working: Vec<String> = observed
                .iter()
                .filter(|obs| {
                    obs.presence == observe::Presence::Present
                        && obs.status == Some(gear_herdr::AgentStatus::Working)
                })
                .map(|obs| obs.seat.id.clone())
                .collect();
            let (confirmed, flagged) = ops::confirm_pending_with(
                store,
                &project,
                cfg.doorbell_check_budget,
                delivery_grace,
                &working,
                now,
            )?;
            report.confirmed.extend(confirmed);
            report.flagged.extend(flagged);
            note_slow(report, "confirm", step);

            // 2b. a late check-in (design §10.3): a view that shows the attempt
            //     after its delivery op failed confirms the attempt anyway and
            //     closes the stall item.
            let step = std::time::Instant::now();
            report
                .late_confirmed
                .extend(ops::confirm_late_views(store, &project, now)?);
            note_slow(report, "late confirm", step);

            let step = std::time::Instant::now();
            attention::promote(store, &project, now)?;
            note_slow(report, "attention", step);

            // 3. dispatch: per lane seat a view and a decision, read-only with this
            //    tick's observations …
            let step = std::time::Instant::now();
            let dispatch_cfg = dispatch::DispatchCfg {
                cooldown_s: cfg.cooldown_s,
                ..Default::default()
            };
            let planned = store
                .read(|tx| dispatch::plan_dispatch(tx, &project, &observed, &dispatch_cfg, now))?;
            for (seat, action) in &planned {
                // An escalated seat needs a human; nothing is sent to it.
                if matches!(action, dispatch::Action::Escalate { .. })
                    && !restarted.contains(&seat.id)
                    && !report.flagged.contains(&seat.id)
                {
                    report.flagged.push(seat.id.clone());
                }
            }
            // Session setup may take several ticks; no packet goes before Ready.
            dispatch::recover_open_rounds(store, herdr, cfg, &project, &observed, &planned, now)?;
            // A voyage that has not departed yet plans no delivery this tick
            // (T-448): its lanes are not started yet, and nothing is sent to
            // them. Reviews, merges and the captain's items are untouched.
            let departing = gear_app::config::latest(store, &project)
                .map(|config| gear_app::config::departs_at(&config, now).is_some())
                .unwrap_or(false);
            // T-450: while the fleet rests, no delivery goes out.
            let resting = hours::resting(store, &project, now)?;
            let prepared = if departing || resting {
                Vec::new()
            } else {
                dispatch::prepare_dispatch(store, herdr, cfg, &project, &planned, now)?
            };
            let deliveries =
                store.write(|tx| dispatch::intend_dispatch(tx, &project, &prepared, now))?;
            report
                .intended
                .extend(deliveries.into_iter().map(|op| op.id));
            note_slow(report, "dispatch", step);

            // 6. feed: a lane running dry gets one item in the captain's inbox,
            //    before it stops (never-stop). Idempotent per lane and tick.
            let step = std::time::Instant::now();
            report.feed.extend(
                feed::keep_lanes_fed(store, &project, now)?
                    .into_iter()
                    .map(|change| format!("{}:{}", change.lane, change.action)),
            );
            note_slow(report, "feed", step);

            // 6a. a reviewer seat that runs no reviewer frees its slot (B02-04):
            //     a dead temp seat is released before the reviews start, so the
            //     count of temps in use never passes the quota and the next
            //     review can take the seat.
            let step = std::time::Instant::now();
            report
                .dead_temps
                .extend(release::release_dead_temps(store, herdr, &project, now)?);
            note_slow(report, "dead temps", step);

            // 6c. every lane in the tail while required work remains: one item in
            //     the captain's inbox, so the warnings a captain turned off do not
            //     go forgotten (B02-08).
            let step = std::time::Instant::now();
            report.tail.extend(
                tailed::watch(store, &project, now)?
                    .map(|change| format!("all-tail:{}", change.as_str())),
            );
            note_slow(report, "tail", step);

            // 6d. the friction ring (design §6.4): an idle captain hears about
            //     the open friction reports — one item on the thread `friction`,
            //     at most once per 30 min, updated by a new report and closed
            //     when every report it lists is closed.
            let step = std::time::Instant::now();
            report.friction.extend(
                friction::ring(store, &project, now)?
                    .map(|change| format!("friction:{}", change.as_str())),
            );
            note_slow(report, "friction", step);

            // 6e. a voyage audit hands itself in (T-505): a task the captain
            //     marked `audit: true` is never assigned to its lane, so when
            //     it is Ready and every dependency is merged it becomes a
            //     hand-in at the integration branch's tip, and step 7 gives it
            //     a reviewer.
            let step = std::time::Instant::now();
            let audits = match gear_app::config::latest(store, &project) {
                Ok(config) => {
                    audit::start_audits(store, cfg, &mut tips_by_repo, &config, &project, now)?
                }
                // A config that does not parse is reported by the steps that
                // need it; an audit waits for the next tick with a good one.
                Err(_) => Default::default(),
            };
            report.audits.extend(audits.handed_in);
            report.audit_failed.extend(audits.failed);
            note_slow(report, "audit", step);

            // 7. review: a lane handed a result in → a temp reviewer starts. The
            //    lane is not blocked; the reviewer is a seat of its own.
            // T-386: the step reads every repository's branch tips once, so its
            // cost no longer grows with the started attempts.
            let step = std::time::Instant::now();
            // T-450: while the fleet rests, no review starts. Its reviewers were
            // released at the end, and the review starts again at the next start.
            let reviews = if hours::resting(store, &project, now)? {
                review::ReviewReport::default()
            } else {
                review::start_reviews_with_tips(
                    store,
                    herdr,
                    cfg,
                    &profiles,
                    &mut tips_by_repo,
                    &project,
                    now,
                )?
            };
            report.reviewers.extend(reviews.started);
            // A mechanical hand-in settles itself (T-273): reported the way a
            // review's own settlement is, so one failed settlement is visible and
            // still never stops the tick.
            report.settled.extend(reviews.settled);
            report.settle_failed.extend(reviews.settle_failed);
            note_slow(report, "review", step);

            // 7b. a reviewer that stopped before its verdict is told to go on
            //     (L-53); past its nudge budget it is flagged for the captain.
            let step = std::time::Instant::now();
            let (_nudged, stuck) =
                review::nudge_stopped_reviewers(store, &project, &observed, cfg.cooldown_s, now)?;
            for seat in stuck {
                reviewer_nudge_flags.push((project.clone(), seat));
            }
            note_slow(report, "reviewer nudge", step);

            // 7c. a lane that stopped on a server error goes on by itself
            //     (T-206): its agent is idle with an attempt in flight.
            let step = std::time::Instant::now();
            let (nudged_lanes, stuck_lanes) =
                lane_nudge::nudge_stopped_lanes(store, &project, &observed, now)?;
            report.lane_nudged.extend(nudged_lanes);
            for seat in stuck_lanes {
                report.flagged.push(seat);
            }
            note_slow(report, "lane nudge", step);

            // 8. release: a reviewer whose task left review frees its slot (T-17).
            let step = std::time::Instant::now();
            let releases = release::release_finished_reviews(store, herdr, &project, now)?;
            report.released.extend(releases.released);
            note_slow(report, "release", step);

            // 8a. retire: a Retiring lane that is idle at a safe point says
            //     goodbye, closes its pane and becomes Retired (T-65).
            let step = std::time::Instant::now();
            report.released.extend(release::retire_finished_lanes(
                store, herdr, &project, &observed, &profiles, now,
            )?);
            note_slow(report, "retire", step);

            // 9. gather (pure): what each seat of this project would be told
            //    now. The rings themselves are built after every project has
            //    run, so a pane that holds several captain seats gets one ring
            //    (T-487). This comes after every delivery of the tick: a seat
            //    being handed work is not rung in the same tick (T-71) — on
            //    T-61 both went into the reviewer's pane in one second and the
            //    packet was swallowed. The delivery holds the seat's slot, so
            //    the doorbell waits.
            let step = std::time::Instant::now();
            let inputs = store.read(|tx| doorbell_inputs(tx, &project, &observed, now))?;
            ring_inputs.extend(inputs);
            note_slow(report, "doorbell gather", step);

            // 12. merge: collect a finished background merge or launch the next
            //     one. Git and the gate run beside the tick (T-36).
            let step = std::time::Instant::now();
            for merged in merge::merge_passed(store, herdr, cfg, &project, now)? {
                report
                    .merged
                    .push(format!("{} @ {}", merged.task, merged.branch));
            }
            note_slow(report, "merge", step);
            Ok(())
        };
    for project in &projects {
        let step = std::time::Instant::now();
        if let Err(error) = one_project(store, &mut report, project) {
            report.project_failed.push(project_error(&project, &error));
        }
        note_slow(&mut report, &format!("project {project}"), step);
    }
    // 10. rings (T-487): the seats of every project bound to one pane are
    //     joined into one doorbell, each line naming its project, urgent
    //     first. A project outside its working hours adds nothing; the other
    //     projects' rings go on. A single project's ring is the text it always
    //     was. The idle, at-once and cooldown rules are the ones of `decide`,
    //     applied per seat before the join.
    let step = std::time::Instant::now();
    let mut resting: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for project in &projects {
        if hours::resting(store, project, now)? {
            resting.insert(project.clone());
        }
    }
    let rings = doorbell::rings(&ring_inputs, &resting, cfg.cooldown_s, now);
    let intended = store.write(|tx| ops::intend_rings(tx, &rings, now))?;
    report.intended.extend(intended.into_iter().map(|op| op.id));
    note_slow(&mut report, "ring intend", step);

    // 11. execute: re-check each op, call Herdr, record the outcome. The
    //     ring's own op types the joined text once for its pane; the other
    //     seats' ops are already submitted and are never typed again.
    let step = std::time::Instant::now();
    for project in &projects {
        quiet::prepare_pending_deliveries(store, herdr, cfg, project, now)?;
        let executed = ops::execute_intended(store, herdr, project, now)?;
        store.write(|tx| {
            for (id, state) in &executed {
                if state != "submitted" {
                    continue;
                }
                let Some(op) = tx.get_op(project, id)? else {
                    continue;
                };
                if op.kind != gear_model::OpKind::Deliver {
                    continue;
                }
                let Some(seat) = tx.get_seat(&op.seat)? else {
                    continue;
                };
                if seat.role != gear_model::SeatRole::Lane {
                    continue;
                }
                if let (Some(task), Some(n)) = (op.task.as_deref(), op.attempt) {
                    if tx
                        .get_attempt(project, task, n)?
                        .is_some_and(|a| a.sent_at.is_none())
                    {
                        tx.set_attempt_sent(project, task, n, now)?;
                    }
                }
            }
            Ok(())
        })?;
        report.executed.extend(executed);
    }
    note_slow(&mut report, "execute", step);

    // 13. the rounds bookkeeping of every project (design §4.1): the session of
    //     a seat joins its open round, and a round ends at the next round on
    //     its seat or at the seat's release. The overrun question (§5.2) and
    //     the declared waits (§5.3) tick with it; both are empty until their
    //     own tasks land.
    let step = std::time::Instant::now();
    report.rounds = rounds::tick(store, herdr, cfg, now)?;
    note_slow(&mut report, "rounds", step);
    let step = std::time::Instant::now();
    waits::tick(store, herdr, cfg, now)?;
    note_slow(&mut report, "waits after", step);
    let step = std::time::Instant::now();
    overrun::tick(store, herdr, cfg, now)?;
    note_slow(&mut report, "overrun", step);
    let step = std::time::Instant::now();
    for (project, seat) in reviewer_nudge_flags {
        if store.read(|tx| review::nudge_flag_current(tx, &project, &seat, now))?
            && !report.flagged.contains(&seat)
        {
            report.flagged.push(seat);
        }
    }
    note_slow(&mut report, "reviewer nudge flags", step);
    // 10. every flagged seat reaches its captain (L-52): a flag that only goes
    //     to the log leaves a lane standing still with nobody knowing.
    // A failed delivery remains a live fact until its retry is cleared or
    // confirmed. The initial failure is emitted only once by the op, so
    // revisit it on later ticks to allow a resolved item to ring again.
    for (project, seat, present) in &observed_seats {
        let other_cause = report.flagged.contains(seat);
        let outstanding = store.write(|tx| {
            let Some(failed) = delivery::failed(tx, seat)? else {
                return Ok(false);
            };
            let task = tx.get_task(project, &failed.task)?;
            let attempt = tx.get_attempt(project, &failed.task, failed.attempt)?;
            let ended = task.as_ref().is_some_and(|t| t.state.is_terminal())
                || attempt.as_ref().is_some_and(|a| a.state.is_terminal());
            if ended {
                delivery::clear(tx, seat)?;
                // A missing agent or independent escalation is still a live
                // fact; keep its item and let the normal flag update it.
                if !other_cause {
                    stall::resolve(tx, project, seat, now, None)?;
                }
                return Ok(false);
            }
            // T-218 (F3): while the one automatic retry of this delivery is
            // still ahead, the first failure's flag stays suppressed: the
            // sweep must not restore it. An independent cause (a missing
            // agent, an escalation) keeps its own flag.
            if crate::lane_nudge::retry_pending(tx, seat, &failed.task, failed.attempt)? {
                // T-218 (F1): a retry that failed while it was sent (the
                // agent of the pane was gone) never reaches the
                // confirmation window: it spends its marker here, and the
                // captain hears about it now.
                if crate::lane_nudge::retry_failed(tx, seat, &failed.task, failed.attempt)? {
                    if attempt
                        .as_ref()
                        .is_some_and(|a| a.state == gear_model::AttemptState::Started)
                    {
                        delivery::clear(tx, seat)?;
                        tx.set_meta(
                            &crate::lane_nudge::retry_marker(seat, &failed.task, failed.attempt),
                            "",
                        )?;
                        return Ok(false);
                    }
                    tx.set_meta(
                        &crate::lane_nudge::retry_marker(seat, &failed.task, failed.attempt),
                        "",
                    )?;
                    return Ok(true);
                }
                return Ok(false);
            }
            Ok(*present
                && tx
                    .get_meta(&format!("stall-active:{seat}"))?
                    .is_some_and(|v| !v.is_empty()))
        })?;
        if outstanding && !report.flagged.contains(seat) {
            report.flagged.push(seat.clone());
        }
    }
    let mut actionable = Vec::new();
    for seat in &report.flagged {
        if !store.read(|tx| waits::waiting_seat(tx, seat))? {
            actionable.push(seat.clone());
        }
    }
    report.flagged = actionable;
    for (project, seat) in present_seats {
        if !report.flagged.contains(&seat) {
            store.write(|tx| {
                if tx.get_meta(&format!("stall-missing:{seat}"))?.as_deref() == Some("1") {
                    stall::resolve(tx, &project, &seat, now, None)?;
                    tx.set_meta(&format!("stall-missing:{seat}"), "")?;
                }
                Ok(())
            })?;
        }
    }
    for seat in report.flagged.clone() {
        if let Err(e) = store.write(|tx| {
            let missing = missing_seats.contains(&seat);
            tx.set_meta(&format!("stall-missing:{seat}"), if missing { "1" } else { "" })?;
            let row = tx.get_seat(&seat)?;
            // T-324: a lane helping on an unanswered captain instruction is
            // busy on purpose. While its agent is present no stall item is
            // written; a missing agent ("its agent is gone from its pane") is
            // still told.
            if !missing {
                if let Some(row) = row.as_ref() {
                    if gear_board::idle::helping(tx, &row.project, row, now)?.is_some() {
                        return Ok(false);
                    }
                }
            }
            // T-287: a seat the engine already restarted once on this config
            // that is missing again ends the same way (M4.2 #17). Its item says
            // so, and names the pane and the config as the place to look, so
            // the captain does not spend a second restart on it.
            let again = match (missing, row) {
                (true, Some(row)) => restart_spent(tx, &row)?,
                _ => false,
            };
            let fact = if again {
                "its agent exited again after the engine restarted it once on the current config; the cause is in its pane or its config (a refused model id, a missing CLI, a login): fix that first, since another restart on the same config ends the same way"
            } else if missing {
                "its agent is gone from its pane"
            } else {
                "a delivery was not confirmed or continuing it made no progress"
            };
            stall::alert_fact(tx, &seat, now, fact)
        }) {
            // One seat's alert failing never stops the tick.
            report.settle_failed.push(format!("stall alert {seat}: {}", e.detail));
        }
    }
    Ok(report)
}

/// Did the engine's one restart already come back to the same end for this
/// seat's **current job** (T-287)?
///
/// The restart of a job writes `engine.restart.<seat>.<task>.<attempt>`
/// (`fn restart_missing_jobs`): `"started"` means the restart was spent on that
/// job, so a seat missing again after it cannot be helped by another restart on
/// the same config. The current job is the attempts in progress for a lane, and
/// the seat's own task with its last attempt for a temp seat.
fn restart_spent(tx: &gear_store::Tx<'_>, seat: &gear_store::SeatRow) -> GearResult<bool> {
    use gear_model::{AttemptState, SeatRole};
    let jobs: Vec<(String, i64)> = match seat.role {
        SeatRole::Lane => tx
            .in_progress_attempts_of_seat(&seat.id)?
            .into_iter()
            .filter(|a| matches!(a.state, AttemptState::Started | AttemptState::Delivered))
            .map(|a| (a.task, a.n))
            .collect(),
        // A released reviewer has no `task_id` any more: the restart finds its
        // job through the pending review, so this does the same.
        SeatRole::Temp => match tx
            .get_meta(&format!("engine.review_pending.{}", seat.id))?
            .filter(|s| !s.is_empty())
            .or_else(|| seat.task_id.clone())
        {
            Some(task) => tx
                .attempts_of_task(&seat.project, &task)?
                .last()
                .map(|a| (a.task.clone(), a.n))
                .into_iter()
                .collect(),
            None => Vec::new(),
        },
        _ => Vec::new(),
    };
    for (task, attempt) in jobs {
        let marker = format!("engine.restart.{}.{}.{}", seat.id, task, attempt);
        if tx.get_meta(&marker)?.as_deref() == Some("started") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn restart_missing_jobs(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    profiles: &std::collections::BTreeMap<String, gear_herdr::Profile>,
    project: &str,
    observed: &[observe::SeatObs],
    now: Ts,
) -> GearResult<std::collections::BTreeSet<String>> {
    use gear_model::{AttemptState, SeatRole, SeatState};
    let shell_panes = match herdr.panes() {
        Ok(panes) => panes,
        Err(_) => return Ok(Default::default()),
    };
    let mut handled = std::collections::BTreeSet::new();
    let seats = store.read(|tx| tx.seats_of_project(project))?;
    for seat in seats {
        if !matches!(seat.role, SeatRole::Lane | SeatRole::Temp) {
            continue;
        }
        let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
        let pending_restart = restart::pending(store, &seat.id)?;
        let pane = pending_restart
            .as_ref()
            .and_then(|_| restart::pane_of(store, &seat).ok().flatten())
            .or_else(|| {
                binding
                    .as_ref()
                    .filter(|b| {
                        b.status == gear_store::BindingStatus::Active
                            || b.status == gear_store::BindingStatus::Pending
                    })
                    .map(|b| b.pane_id.clone())
            });
        let Some(pane) = pane.filter(|p| shell_panes.contains(p)) else {
            continue;
        };
        if herdr.agent_get(&pane).ok().flatten().is_some() {
            continue;
        }

        let jobs = if seat.role == SeatRole::Lane {
            store
                .read(|tx| tx.open_attempts_of_seat(&seat.id))?
                .into_iter()
                .filter(|a| matches!(a.state, AttemptState::Started | AttemptState::Delivered))
                .map(|a| (a.task, a.n))
                .collect::<Vec<_>>()
        } else {
            let task = store.read(|tx| {
                Ok(tx
                    .get_meta(&format!("engine.review_pending.{}", seat.id))?
                    .filter(|s| !s.is_empty())
                    .or(seat.task_id.clone()))
            })?;
            match task {
                Some(task) => store
                    .read(|tx| tx.attempts_of_task(project, &task))?
                    .into_iter()
                    .filter(|a| {
                        a.state == AttemptState::Submitted
                            && store
                                .read(|tx| {
                                    tx.get_review_verdict(project, &task, a.n)
                                        .map(|v| v.is_none())
                                })
                                .unwrap_or(false)
                    })
                    .map(|a| (a.task, a.n))
                    .collect(),
                None => Vec::new(),
            }
        };
        for (task, attempt) in jobs {
            let marker = format!("engine.restart.{}.{}.{}", seat.id, task, attempt);
            let state = store.read(|tx| tx.get_meta(&marker))?;
            if matches!(state.as_deref(), Some("started" | "failed")) {
                break;
            }
            let is_missing = observed.iter().any(|o| {
                o.seat.id == seat.id
                    && o.presence == observe::Presence::Missing
                    && o.seat.state == SeatState::Active
            });
            let plan = restart::pending(store, &seat.id)?;
            if !is_missing && !(state.as_deref() == Some("starting") && plan.is_some()) {
                continue;
            }
            if state.is_none() {
                store.write(|tx| tx.set_meta(&marker, "starting"))?;
            }
            let entry = if plan.is_some() {
                None
            } else {
                if binding.is_none() {
                    continue;
                }
                let project_row = store.read(|tx| tx.get_project(project))?;
                let Some(config) =
                    project_row.and_then(|p| gear_app::config::parse(&p.config_toml).ok())
                else {
                    continue;
                };
                let saved = store
                    .read(|tx| tx.get_meta(&crate::model::model_meta_key(&seat.id)))?
                    .filter(|s| !s.is_empty());
                // T-226: the restart follows the **current** config, not the
                // snapshot the round began with: the entry of the current
                // chain whose model is the seat's saved model, else the first
                // entry. The CLI is the entry's own, else the role's current
                // kind — never the kind of the old binding.
                let (model, cli, args) = if seat.role == SeatRole::Temp {
                    let chain = crate::model::review_chain(&config);
                    let kind = config
                        .temps
                        .as_ref()
                        .map(|temps| temps.kind.clone())
                        .unwrap_or_default();
                    let chosen = saved
                        .as_deref()
                        .and_then(|model| chain.iter().find(|entry| entry.model() == model))
                        .or_else(|| chain.first())
                        .copied();
                    match chosen {
                        Some(entry) => (
                            entry.model().to_string(),
                            entry.cli().map(str::to_owned).unwrap_or(kind),
                            entry.args().to_vec(),
                        ),
                        None => (saved.unwrap_or_default(), kind, Vec::new()),
                    }
                } else {
                    let lane = config.lanes.iter().find(|l| l.name == seat.name);
                    let chain = lane
                        .map(|l| crate::model::role_chain(&config, &l.name))
                        .unwrap_or_default();
                    let kind = lane.map(|l| l.kind.clone()).unwrap_or_default();
                    let chosen = saved
                        .as_deref()
                        .and_then(|model| chain.iter().find(|entry| entry.model() == model))
                        .or_else(|| chain.first())
                        .copied();
                    match chosen {
                        Some(entry) => (
                            entry.model().to_string(),
                            entry.cli().map(str::to_owned).unwrap_or(kind),
                            entry.args().to_vec(),
                        ),
                        None => (saved.unwrap_or_default(), kind, Vec::new()),
                    }
                };
                if seat.role == SeatRole::Temp {
                    let chosen =
                        serde_json::json!({"cli": cli, "model": model, "args": args}).to_string();
                    store.write(|tx| {
                        tx.set_meta(&format!("engine.review_model_entry.{}", seat.id), &chosen)
                    })?;
                }
                Some(gear_app::config::ModelEntry::Entry(
                    gear_app::config::ModelSpec {
                        model,
                        cli: Some(cli),
                        args,
                        context_window_k: None,
                    },
                ))
            };
            match restart::advance(
                store,
                herdr,
                cfg,
                profiles,
                project,
                &seat,
                entry.as_ref(),
                now,
            ) {
                Ok(Some(_)) => {
                    store.write(|tx| tx.set_meta(&marker, "started"))?;
                    handled.insert(seat.id.clone());
                }
                Ok(None) if restart::pending(store, &seat.id)?.is_some() => {
                    handled.insert(seat.id.clone());
                }
                Ok(None) => {
                    store.write(|tx| tx.set_meta(&marker, "failed"))?;
                }
                Err(_) => {
                    store.write(|tx| tx.set_meta(&marker, "failed"))?;
                }
            }
            break;
        }
    }
    Ok(handled)
}

/// Settle every review verdict whose attempt is still handed in (T-13), in
/// every project. One `settle_review` call per verdict; a verdict that already
/// settled its attempt, or whose task is finished, is not listed.
///
/// A settlement that fails is reported with its verdict (`settle_failed`) and
/// does **not** abort the tick: one obsolete or malformed review must never
/// stop the dispatch of unrelated work (review R-67, P1).
fn settle_reviews(store: &mut Store, now: Ts) -> GearResult<(Vec<String>, Vec<String>)> {
    let mut settled = Vec::new();
    let mut failed = Vec::new();
    for project in store.read(|tx| tx.project_ids())? {
        // Only passes settle themselves: a pass merges without the captain's
        // hands (M1). A fail goes to the captain, who judges its findings
        // against the voyage's goal and bar and writes the rework (L-57).
        for review in store
            .read(|tx| tx.reviews_to_settle(&project))?
            .into_iter()
            .filter(|review| review.passed)
        {
            let label = format!("{project}:{}/{}", review.task, review.attempt);
            match store.write(|tx| {
                gear_app::tasks::settle_review(tx, &project, &review.task, review.attempt, now)
            }) {
                Ok(()) => settled.push(label),
                // Logged in the tick report; the rest of the tick goes on.
                Err(e) => failed.push(format!("{label}: {}: {}", e.code, e.detail)),
            }
        }
    }
    Ok((settled, failed))
}

/// Does this seat have no open attempt? (design §10.4: a lane with no open
/// attempt gets the doorbell text that tells it to stop.)
///
/// Only a **lane** counts: the captain and the reviewers have no attempt of
/// their own by design, and their doorbell keeps its usual text.
pub(crate) fn no_task_of(tx: &gear_store::Tx<'_>, seat: &gear_store::SeatRow) -> GearResult<bool> {
    if seat.role != gear_model::SeatRole::Lane {
        return Ok(false);
    }
    Ok(tx.open_attempts_of_seat(&seat.id)?.is_empty())
}

/// What the doorbell decision needs to know about one seat (design/07).
/// Session preparation owns the pane until its current round is ready.
fn session_preparation_pending(
    tx: &gear_store::Tx<'_>,
    obs: &observe::SeatObs,
) -> GearResult<bool> {
    let seat = &obs.seat;
    let Some(round) = tx.open_round_of_seat(&seat.id)? else {
        return Ok(false);
    };
    let identity = format!(
        "{}:{}:{}:{}",
        round.project, round.task, round.attempt, round.role
    );
    let state = tx
        .get_meta(&format!("engine.session_step.{}", seat.id))?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    if let Some(state) = state
        .filter(|s| s["round"] == identity && s["generation"].as_i64() == Some(seat.generation))
    {
        match state["step"].as_str() {
            Some("failed") => return Ok(false),
            Some("done") => {}
            _ => return Ok(true),
        }
    }
    // A round can exist before the runner has written its first step (or when
    // no profile is available); an unsent assignment still reserves the pane.
    let Some(attempt) = tx.get_attempt(&round.project, &round.task, round.attempt)? else {
        return Ok(false);
    };
    if attempt.state == gear_model::AttemptState::Delivered && attempt.sent_at.is_none() {
        return Ok(true);
    }
    if !matches!(
        attempt.state,
        gear_model::AttemptState::Delivered
            | gear_model::AttemptState::Started
            | gear_model::AttemptState::Stopped
    ) {
        return Ok(false);
    }
    let expected = crate::session::round_session(tx, &round)?;
    Ok(expected.is_some_and(|session| {
        obs.presence == observe::Presence::Missing
            || (obs.presence == observe::Presence::Present
                && obs
                    .session
                    .as_deref()
                    .is_some_and(|current| current != session))
    }))
}

fn doorbell_inputs(
    tx: &gear_store::Tx<'_>,
    project: &str,
    observed: &[observe::SeatObs],
    now: Ts,
) -> GearResult<Vec<doorbell::DoorbellInput>> {
    use gear_model::{OpKind, OpState};
    use gear_store::BindingStatus;
    use std::collections::BTreeSet;

    // The sender whose instruction reaches a lane that queues input while it
    // works (T-130).
    let captain = tx.captain_of(project)?.map(|seat| seat.id);
    let mut inputs = Vec::new();
    for seat_obs in observed {
        let seat = &seat_obs.seat;
        if seat.role == gear_model::SeatRole::Captain && tx.captain_quota_paused(project)? {
            continue;
        }
        // A seat without an active binding has no pane to ring.
        let Some(binding) = tx
            .get_binding(&seat.id, seat.generation)?
            .filter(|b| b.status == BindingStatus::Active)
        else {
            continue;
        };

        // One doorbell per message, never repeated (T-130): an id an earlier
        // doorbell of this seat named is not named again — not after the
        // cooldown, and not when that doorbell was never confirmed. A doorbell
        // that never reached the pane covers nothing: a voided one, and a
        // definitely failed one (Herdr refused it, review F2), so a refused
        // message is rung again on a later tick.
        //
        // The captain is the exception: its unanswered items are reminded on
        // purpose, once per configured period (T-40), and that reminder is the
        // one thing that brings an ignored item back.
        let doorbelled: BTreeSet<String> = if seat.role == gear_model::SeatRole::Captain {
            BTreeSet::new()
        } else {
            crate::ops::covered_messages(tx, &seat.id)?
        };

        // What this generation has already been shown: an open item it has
        // seen normally waits for the seat's next `gear` (L-42). Stale
        // captain items are the exception: one reminder per configured period.
        let shown: BTreeSet<String> = tx
            .views_since(&seat.id, seat.generation, 0)?
            .into_iter()
            .flat_map(|view| view.shown_msgs)
            .collect();
        // Only this generation's messages may ring the seat (T-71): a warm
        // temp seat keeps its id across jobs and a re-registered lane keeps
        // its name, so an earlier job's or voyage's items would otherwise be
        // counted as unshown for ever. A temp seat's messages must also be
        // about the task it reviews now (F1 of the T-71 review).
        let since = gear_app::seats::generation_since(tx, &seat.id, seat.generation)?;
        let period = attention::period(tx, project)?;
        // A captain instruction reaches a lane that queues input even while
        // it works; everything else waits for idle unless it is urgent
        // (T-130). `binding.kind` is the CLI that runs in the seat's pane; a
        // reviewer (Temp) is not a lane and is not interrupted mid-review.
        let item = |message: &gear_store::MessageRow| doorbell::DoorbellItem {
            id: message.id.clone(),
            urgent: message.urgent,
            at_once: doorbell::reaches_while_working(
                message,
                seat,
                &binding.kind,
                captain.as_deref(),
            ),
            // T-454 v2: the notice that closes a direction check under its
            // reviewer cuts that turn and goes even while the seat holds a
            // delivery slot (the review it was just handed).
            cut: doorbell::ends_direction_check(message),
        };
        let mut waiting: Vec<doorbell::DoorbellItem> = Vec::new();
        let declared_wait = waits::waiting_seat(tx, &seat.id)?;
        let mut overrun_questions = BTreeSet::new();
        for message in tx.actionable_for(project, &seat.id, now)? {
            if declared_wait && message.thread.starts_with("overrun-question:") {
                continue;
            }
            let include = if seat.role == gear_model::SeatRole::Captain {
                // The captain's open items are the engine's business: they
                // ring until they are answered, whatever generation took the
                // seat over (F2 of the T-71 review).
                attention::should_show(tx, &message, shown.contains(&message.id), period, now)?
            } else {
                gear_app::seats::message_belongs(seat, &message, since)
                    && !shown.contains(&message.id)
            };
            if include && !doorbelled.contains(&message.id) {
                if message.urgent && message.thread.starts_with("overrun-question:") {
                    overrun_questions.insert(message.id.clone());
                }
                waiting.push(item(&message));
            }
        }

        // Reply/Instruction messages this generation has not shown yet.
        for message in tx.messages_to_since(project, &seat.id, since)? {
            if declared_wait && message.thread.starts_with("overrun-question:") {
                continue;
            }
            if shown.contains(&message.id)
                || doorbelled.contains(&message.id)
                || !gear_app::seats::message_belongs(seat, &message, since)
                || waiting.iter().any(|item| item.id == message.id)
            {
                continue;
            }
            if message.urgent && message.thread.starts_with("overrun-question:") {
                overrun_questions.insert(message.id.clone());
            }
            waiting.push(item(&message));
        }

        // The captain's doorbell names only the items its `gear` shows
        // (T-254): the home page shows a batch (at most `max_threads` threads,
        // `per_sender` per sender), so naming every open item rang the captain
        // for items its own `gear` never listed. Lanes and reviewers keep
        // their own rule, unchanged.
        if seat.role == gear_model::SeatRole::Captain {
            let batch = gear_app::mail::select_batch(
                tx,
                project,
                &seat.id,
                now,
                gear_app::mail::BatchLimits::default(),
            )?;
            let batch_ids: BTreeSet<String> = batch
                .into_iter()
                .flat_map(|view| view.open.into_iter().map(|message| message.id))
                .collect();
            waiting.retain(|item| batch_ids.contains(&item.id));
        }

        // A delivery/doorbell that still occupies the seat's slot, and when the
        // previous doorbell was settled (for the cooldown).
        let slot_ops = tx.slot_ops(&seat.id)?;
        // A review delivery stays submitted until the verdict. Its reviewer
        // must still receive the urgent overrun question during the review.
        let review_question = seat.role == gear_model::SeatRole::Temp
            && !overrun_questions.is_empty()
            && !slot_ops.is_empty()
            && slot_ops.iter().all(|op| {
                op.kind == OpKind::Deliver
                    && op.state == OpState::Submitted
                    && op.task == seat.task_id
            });
        if review_question {
            // T-454 v2: a message that cuts the turn is never dropped by the
            // review-question narrowing — it is the one notice that must
            // reach the reviewer now.
            waiting.retain(|item| overrun_questions.contains(&item.id) || item.cut);
        }
        let slot_busy =
            (!slot_ops.is_empty() && !review_question && !waiting.iter().any(|item| item.cut))
                || session_preparation_pending(tx, seat_obs)?;
        let last_settled_at = tx
            .last_op(&seat.id, OpKind::Doorbell)?
            .filter(|op| {
                matches!(
                    op.state,
                    OpState::Confirmed | OpState::Failed | OpState::Void
                )
            })
            .map(|op| op.updated);

        inputs.push(doorbell::DoorbellInput {
            project: project.to_string(),
            seat: seat.id.clone(),
            generation: seat.generation,
            pane_id: binding.pane_id,
            // Only a seat that may receive work is rung, even if its agent is
            // idle (a parked/held seat must not be woken by a doorbell).
            idle: seat_obs.idle && observe::reachable(seat.state),
            status: if observe::reachable(seat.state) {
                seat_obs.status
            } else {
                None
            },
            slot_busy,
            last_settled_at,
            // Design §10.4: a lane with no open attempt gets the text that
            // tells it to stop working on earlier things.
            no_task: no_task_of(tx, seat)?,
            waiting,
            goal: if seat.role == gear_model::SeatRole::Captain {
                doorbell::goal_line(tx, project)?
            } else {
                None
            },
        });
    }
    Ok(inputs)
}

/// Scratch directories of the unit tests, removed when the test is done (T-412).
#[cfg(test)]
pub(crate) mod testtmp;

#[cfg(test)]
mod t209_tick_tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_model::{AttemptState, MsgState, SeatRole, TaskState};
    use gear_store::{testing, AttemptRow, ContractRow, NewRound, RoundKey, TaskRow};

    const NOW: Ts = 1_900_000_000;
    const CONFIG: &str = r#"
[project]
id="p"
herdr_session="sess"
repo="/repo"
[quota]
lanes=1
temps=1
[captain]
pane="w1:p9"
kind="claude"
profile="claude"
[[lanes]]
name="heat"
duty="everything"
owns=["src/"]
kind="codex"
profile="codex"
[temps]
kind="codex"
profile="codex"
[models]
lanes=[{model="gpt-6.1-sol",cli="codex",args=["-c","model_reasoning_effort=high"]}]
review=[{model="gpt-6.1-sol",cli="codex",args=["-c","model_reasoning_effort=high"]}]
"#;

    fn fixture(role: SeatRole) -> (Store, FakeHerdr, EngineCfg, String) {
        let mut store = Store::open_in_memory().unwrap();
        let pane = if role == SeatRole::Lane {
            "w1:p1"
        } else {
            "w1:p2"
        };
        let seat_name = if role == SeatRole::Lane {
            "heat"
        } else {
            "review-1"
        };
        let task_state = if role == SeatRole::Lane {
            TaskState::Running
        } else {
            TaskState::Verifying
        };
        let attempt_state = if role == SeatRole::Lane {
            AttemptState::Started
        } else {
            AttemptState::Submitted
        };
        let seat=store.write(|tx| {
            let mut p=testing::seed_project(tx,"p","sess",NOW)?; p.config_toml=CONFIG.into(); tx.update_project(&p)?;
            let seat=testing::seed_active_seat(tx,"p",seat_name,role,pane,"codex",NOW)?;
            testing::seed_active_seat(tx,"p","captain",SeatRole::Captain,"w1:p9","claude",NOW)?;
            if role==SeatRole::Temp { tx.set_seat_task(&seat.id,"T-1")?; }
            tx.insert_task(&TaskRow {project:"p".into(),id:"T-1".into(),rank:1,state:task_state,priority:"must".into(),lane:Some("heat".into()),milestone:None,required:false,contract_version:1,deps:vec![],followup_of:None,wait_on:None,wait_cond:None,rework_note:None,estimate_min:Some(30),issue:None,updated:NOW,repo:None,base:None,gate:None,rework_estimate_min:None})?;
            tx.insert_contract(&ContractRow {project:"p".into(),task:"T-1".into(),version:1,goal:"goal".into(),accept:"accept".into(),evidence_files:vec![],evidence_due_min:None,body:String::new(),created:NOW, mechanical: None,})?;
            tx.insert_attempt(&AttemptRow {project:"p".into(),task:"T-1".into(),n:1,seat:"p:heat".into(),generation:1,contract_version:1,adopted_version:1,pending_update:None,state:attempt_state,candidate:(role==SeatRole::Temp).then(||"abc1234".into()),report:None,safe_point:None,started:NOW,updated:NOW,session_ref:None,session_cli:Some("codex".into()),session_action:None,sent_at:Some(NOW)})?;
            let round_role=if role==SeatRole::Lane {"dev"} else {"review"};
            tx.insert_round(&NewRound {key:RoundKey {project:"p".into(),task:"T-1".into(),attempt:1,role:round_role.into()},seat:seat.id.clone(),generation:1,cli:"codex".into(),model:Some("gpt-6.1-sol".into()),start_at:NOW})?;
            tx.set_meta(&crate::model::model_meta_key(&seat.id),"gpt-6.1-sol")?;
            if role==SeatRole::Temp {
                tx.set_meta("engine.review_model_entry.p:review-1",&serde_json::json!({"cli":"codex","model":"gpt-6.1-sol","args":["-c","model_reasoning_effort=high"]}).to_string())?;
                tx.set_meta("engine.review_pending.p:review-1","T-1")?;
                tx.set_meta("engine.review_path.p:review-1","/work/review")?;
                tx.set_meta("engine.review_repo.p:review-1","/repo")?;
            } else { tx.set_meta(&crate::onboard::worktree_path_key(&seat.id),"/work/heat")?; }
            Ok(seat)
        }).unwrap();
        let _ = seat;
        let mut herdr = FakeHerdr::new();
        herdr.add_shell(pane);
        herdr.add_shell("w1:p9");
        let mut cfg = EngineCfg::default();
        cfg.db_path = crate::testtmp::temp("t209")
            .join("t209.db")
            .to_string_lossy()
            .into_owned();
        for kind in ["codex", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        (store, herdr, cfg, pane.into())
    }

    #[test]
    fn exited_lane_and_reviewer_restart_once_replay_their_packets_and_stall_after_second_exit() {
        for role in [SeatRole::Lane, SeatRole::Temp] {
            let (mut store, mut herdr, cfg, pane) = fixture(role);
            tick(&mut store, &mut herdr, &cfg, NOW + 1).unwrap();
            let starts: Vec<_> = herdr
                .calls
                .iter()
                .filter_map(|call| {
                    if let Call::AgentStart {
                        kind, args, pane, ..
                    } = call
                    {
                        Some((kind.clone(), args.clone(), pane.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(starts.len(), 1, "{role:?}: {starts:?}");
            assert_eq!(starts[0].0, "codex");
            assert_eq!(starts[0].2, pane);
            assert!(
                starts[0].1.iter().any(|arg| arg.contains("gpt-6.1-sol")),
                "{:?}",
                starts[0].1
            );
            assert!(
                starts[0]
                    .1
                    .iter()
                    .any(|arg| arg.contains("model_reasoning_effort=high")),
                "{:?}",
                starts[0].1
            );
            let packet = herdr
                .calls
                .iter()
                .find_map(|call| {
                    if let Call::Prompt { text, .. } = call {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            if role == SeatRole::Lane {
                assert!(packet.contains("Continue T-1"), "{packet}");
            } else {
                assert!(packet.contains("Review T-1 attempt 1"), "{packet}");
            }
            herdr.exit_agent(&pane);
            tick(&mut store, &mut herdr, &cfg, NOW + 2).unwrap();
            assert_eq!(
                herdr
                    .calls
                    .iter()
                    .filter(|call| matches!(call, Call::AgentStart { .. }))
                    .count(),
                1
            );
            let stall = store
                .read(|tx| tx.thread_messages("p", "stall:p:review-1"))
                .unwrap();
            let lane_stall = store
                .read(|tx| tx.thread_messages("p", "stall:p:heat"))
                .unwrap();
            assert_eq!(
                if role == SeatRole::Lane {
                    lane_stall.len()
                } else {
                    stall.len()
                },
                1
            );
            assert_eq!(
                if role == SeatRole::Lane {
                    lane_stall[0].state
                } else {
                    stall[0].state
                },
                MsgState::Open
            );
            // T-287: the second exit after the engine's one restart says so.
            let body = &if role == SeatRole::Lane {
                &lane_stall[0].body
            } else {
                &stall[0].body
            };
            assert!(
                body.contains("exited again after the engine restarted it once"),
                "{role:?}: {body}"
            );
        }
    }

    /// T-287: a missing seat the engine never restarted keeps the plain fact:
    /// nothing says another restart would end the same way.
    #[test]
    fn a_missing_seat_never_restarted_keeps_the_plain_fact() {
        for role in [SeatRole::Lane, SeatRole::Temp] {
            let (mut store, mut herdr, cfg, pane) = fixture(role);
            // The pane itself is gone: there is nowhere to restart the agent.
            let _ = herdr.pane_close(&pane);
            tick(&mut store, &mut herdr, &cfg, NOW + 1).unwrap();
            assert_eq!(
                herdr
                    .calls
                    .iter()
                    .filter(|call| matches!(call, Call::AgentStart { .. }))
                    .count(),
                0
            );
            let items = if role == SeatRole::Lane {
                store
                    .read(|tx| tx.thread_messages("p", "stall:p:heat"))
                    .unwrap()
            } else {
                store
                    .read(|tx| tx.thread_messages("p", "stall:p:review-1"))
                    .unwrap()
            };
            assert_eq!(items.len(), 1, "{role:?}: {items:?}");
            assert!(
                items[0].body.contains("its agent is gone from its pane"),
                "{role:?}: {}",
                items[0].body
            );
            assert!(
                !items[0]
                    .body
                    .contains("exited again after the engine restarted it once"),
                "{role:?}: {}",
                items[0].body
            );
        }
    }
}

/// T-386: a tick step that takes over 200 ms is named in the report.
#[cfg(test)]
mod t386_tick_tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_model::{AttemptState, SeatRole, TaskState};
    use gear_store::{testing, AttemptRow, ContractRow, TaskRow};

    const NOW: Ts = 1_900_000_200;
    const CONFIG: &str = r#"
[project]
id="p"
herdr_session="sess"
repo="/repo"
[quota]
lanes=1
temps=1
[captain]
pane="w1:p9"
kind="claude"
profile="claude"
[[lanes]]
name="heat"
duty="everything"
owns=["src/"]
kind="codex"
profile="codex"
[temps]
kind="codex"
profile="codex"
[models]
lanes=[{model="gpt-6.1-sol",cli="codex",args=["-c","model_reasoning_effort=high"]}]
review=[{model="gpt-6.1-sol",cli="codex",args=["-c","model_reasoning_effort=high"]}]
"#;

    /// A git that takes 250 ms per call: a step that reads a repository is
    /// past the 200 ms budget.
    #[derive(Debug)]
    struct SlowReviewGit;
    impl review::ReviewGit for SlowReviewGit {
        fn run(&self, _path: &str, _args: &[&str]) -> Result<String, String> {
            std::thread::sleep(std::time::Duration::from_millis(250));
            Ok(String::new())
        }
    }

    #[test]
    fn a_step_over_the_budget_is_named_in_the_report() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                let mut project = testing::seed_project(tx, "p", "sess", NOW)?;
                project.config_toml = CONFIG.into();
                tx.update_project(&project)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p1", "codex", NOW)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p9",
                    "claude",
                    NOW,
                )?;
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
                    issue: None,
                    updated: NOW,
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    version: 1,
                    goal: "goal".into(),
                    accept: "accept".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: NOW,
                    mechanical: None,
                })?;
                tx.insert_attempt(&AttemptRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    n: 1,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Started,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: NOW,
                    updated: NOW,
                    session_ref: None,
                    session_cli: Some("codex".into()),
                    session_action: None,
                    sent_at: Some(NOW),
                })?;
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.state = TaskState::Running;
                tx.update_task(&task)
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_shell("w1:p1");
        herdr.add_shell("w1:p9");
        let mut cfg = EngineCfg::default();
        cfg.db_path = crate::testtmp::temp("t386")
            .join("t386.db")
            .to_string_lossy()
            .into_owned();
        cfg.review_git = std::sync::Arc::new(SlowReviewGit);
        for kind in ["codex", "claude"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        let report = tick(&mut store, &mut herdr, &cfg, NOW + 1).unwrap();
        // The step that read the branch tips (the push clock, then the review
        // step, both through the one slow git) passed the budget.
        let line = report
            .slow
            .iter()
            .find(|line| line.starts_with("quiet: ") || line.starts_with("review: "))
            .unwrap_or_else(|| panic!("a step over the budget is named: {report:?}"));
        let (step, ms) = line.rsplit_once(": ").expect(line);
        assert!(!step.is_empty(), "{line}");
        assert!(
            ms.trim_end_matches(" ms")
                .parse::<u128>()
                .is_ok_and(|ms| ms > 200),
            "{line}"
        );
        // Every line names a step and its time in milliseconds.
        for line in &report.slow {
            assert!(line.ends_with(" ms"), "{line}");
            assert!(
                line.rsplit_once(": ")
                    .is_some_and(|(step, _)| !step.is_empty()),
                "{line}"
            );
        }
    }

    /// T-458: a **failed** observation that took the time is named too (T-386
    /// review F1). The tick returns early on a failed Herdr read, and the step
    /// used to return before it was timed.
    #[test]
    fn a_slow_failed_observation_is_named_in_the_report() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", NOW)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.agents_fail = true;
        herdr.agents_delay = Some(std::time::Duration::from_millis(250));
        let cfg = EngineCfg::default();
        let report = tick(&mut store, &mut herdr, &cfg, NOW + 1).unwrap();
        assert_eq!(report.observe_failed, vec!["p".to_string()]);
        let line = report
            .slow
            .iter()
            .find(|line| line.starts_with("observe: "))
            .unwrap_or_else(|| panic!("a failed observation over the budget is named: {report:?}"));
        let ms = line
            .rsplit_once(": ")
            .and_then(|(_, ms)| ms.trim_end_matches(" ms").parse::<u128>().ok())
            .unwrap_or_else(|| panic!("{line}"));
        assert!(ms > 200, "{line}");
    }
}

/// T-226: a restarted agent takes the **current** config, not the snapshot
/// its round began with. The model, the CLI and the arguments of a restart
/// come from the chain of the config as it is now.
#[cfg(test)]
mod t226_restart_tests {
    use super::*;
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_herdr::AgentStatus;
    use gear_model::{AttemptState, SeatRole, TaskState};
    use gear_store::{testing, AttemptRow, ContractRow, NewRound, RoundKey, TaskRow};

    const NOW: Ts = 1_900_000_100;

    /// The config of the project: the lane's kind and model and the review
    /// chain. The reviewer chain names its CLI, the lane chain does not (the
    /// lane takes its CLI from its own kind).
    fn config(
        lane_kind: &str,
        lane_model: &str,
        review_model: &str,
        review_effort: &str,
    ) -> String {
        format!(
            r#"
[project]
id="p"
herdr_session="sess"
repo="/repo"
[quota]
lanes=1
temps=1
[captain]
pane="w1:p9"
kind="claude"
profile="claude"
[[lanes]]
name="heat"
duty="everything"
owns=["src/"]
kind="{lane_kind}"
profile="{lane_kind}"
[temps]
kind="claude"
profile="claude"
[models]
lanes=[{{model="{lane_model}"}}]
review=[{{model="{review_model}",cli="codex",args=["-c","model_reasoning_effort={review_effort}"]}}]
"#
        )
    }

    /// The world of the T-209 test: one seat with a binding, an open attempt
    /// and the model of its round in the meta.
    fn fixture(
        role: SeatRole,
        binding_kind: &str,
        config_toml: &str,
    ) -> (Store, FakeHerdr, EngineCfg, String) {
        let mut store = Store::open_in_memory().unwrap();
        let pane = if role == SeatRole::Lane {
            "w1:p1"
        } else {
            "w1:p2"
        };
        let seat_name = if role == SeatRole::Lane {
            "heat"
        } else {
            "review-1"
        };
        let task_state = if role == SeatRole::Lane {
            TaskState::Running
        } else {
            TaskState::Verifying
        };
        let attempt_state = if role == SeatRole::Lane {
            AttemptState::Started
        } else {
            AttemptState::Submitted
        };
        let seat = store
            .write(|tx| {
                let mut p = testing::seed_project(tx, "p", "sess", NOW)?;
                p.config_toml = config_toml.into();
                tx.update_project(&p)?;
                let seat = testing::seed_active_seat(tx, "p", seat_name, role, pane, binding_kind, NOW)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p9", "claude", NOW)?;
                if role == SeatRole::Temp {
                    tx.set_seat_task(&seat.id, "T-1")?;
                }
                tx.insert_task(&TaskRow {project:"p".into(),id:"T-1".into(),rank:1,state:task_state,priority:"must".into(),lane:Some("heat".into()),milestone:None,required:false,contract_version:1,deps:vec![],followup_of:None,wait_on:None,wait_cond:None,rework_note:None,estimate_min:Some(30),issue:None,updated:NOW,repo:None,base:None,gate:None,rework_estimate_min:None})?;
                tx.insert_contract(&ContractRow {project:"p".into(),task:"T-1".into(),version:1,goal:"goal".into(),accept:"accept".into(),evidence_files:vec![],evidence_due_min:None,body:String::new(),created:NOW, mechanical: None,})?;
                tx.insert_attempt(&AttemptRow {project:"p".into(),task:"T-1".into(),n:1,seat:"p:heat".into(),generation:1,contract_version:1,adopted_version:1,pending_update:None,state:attempt_state,candidate:(role==SeatRole::Temp).then(||"abc1234".into()),report:None,safe_point:None,started:NOW,updated:NOW,session_ref:None,session_cli:Some(binding_kind.into()),session_action:None,sent_at:Some(NOW)})?;
                let round_role = if role == SeatRole::Lane { "dev" } else { "review" };
                tx.insert_round(&NewRound {key:RoundKey {project:"p".into(),task:"T-1".into(),attempt:1,role:round_role.into()},seat:seat.id.clone(),generation:1,cli:binding_kind.into(),model:Some("gpt-6.1-sol".into()),start_at:NOW})?;
                tx.set_meta(&crate::model::model_meta_key(&seat.id),"gpt-6.1-sol")?;
                if role == SeatRole::Temp {
                    tx.set_meta("engine.review_model_entry.p:review-1",&serde_json::json!({"cli":"codex","model":"gpt-6.1-sol","args":["-c","model_reasoning_effort=high"]}).to_string())?;
                    tx.set_meta("engine.review_pending.p:review-1","T-1")?;
                    tx.set_meta("engine.review_path.p:review-1","/work/review")?;
                    tx.set_meta("engine.review_repo.p:review-1","/repo")?;
                } else {
                    tx.set_meta(&crate::onboard::worktree_path_key(&seat.id),"/work/heat")?;
                }
                Ok(seat)
            })
            .unwrap();
        let _ = seat;
        let mut herdr = FakeHerdr::new();
        herdr.add_shell(pane);
        herdr.add_shell("w1:p9");
        let mut cfg = EngineCfg {
            db_path: crate::testtmp::temp("t226")
                .join("t226.db")
                .to_string_lossy()
                .into_owned(),
            ..EngineCfg::default()
        };
        for kind in ["codex", "claude", "opencode", "pi"] {
            cfg.profiles
                .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
        }
        (store, herdr, cfg, pane.into())
    }

    /// The starts of the engine, in order: (kind, args).
    fn starts(herdr: &FakeHerdr) -> Vec<(String, Vec<String>)> {
        herdr
            .calls
            .iter()
            .filter_map(|call| {
                if let Call::AgentStart { kind, args, .. } = call {
                    Some((kind.clone(), args.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// The agent of the round is in its pane; it exits; the engine restarts
    /// it in place (T-209).
    fn exit_and_restart(
        store: &mut Store,
        herdr: &mut FakeHerdr,
        cfg: &EngineCfg,
        pane: &str,
        kind: &str,
    ) {
        herdr.add_agent(pane, kind, AgentStatus::Idle);
        tick(store, herdr, cfg, NOW + 1).unwrap();
        assert!(starts(herdr).is_empty(), "a live agent is not restarted");
        herdr.exit_agent(pane);
        tick(store, herdr, cfg, NOW + 2).unwrap();
    }

    /// (a) A reviewer whose round began on model X, with the config applied
    /// again and the review chain [Y]: the restart starts Y with Y's
    /// arguments, and the reviewer's meta entry now holds Y.
    #[test]
    fn a_restarted_reviewer_takes_the_current_review_chain() {
        let (mut store, mut herdr, cfg, pane) = fixture(
            SeatRole::Temp,
            "codex",
            &config("codex", "gpt-6.1-sol", "gpt-6.1-sol", "high"),
        );
        // The config is applied again with a new review chain.
        store
            .write(|tx| {
                let mut p = tx.get_project("p")?.unwrap();
                p.config_toml = config("codex", "gpt-6.1-sol", "gpt-6.2-codex", "low");
                tx.update_project(&p)
            })
            .unwrap();
        exit_and_restart(&mut store, &mut herdr, &cfg, &pane, "codex");
        let started = starts(&herdr);
        assert_eq!(started.len(), 1, "{started:?}");
        assert_eq!(started[0].0, "codex", "{started:?}");
        assert!(
            started[0].1.iter().any(|arg| arg == "gpt-6.2-codex"),
            "{:?}",
            started[0].1
        );
        assert!(
            started[0]
                .1
                .iter()
                .any(|arg| arg == "model_reasoning_effort=low"),
            "{:?}",
            started[0].1
        );
        assert!(
            !started[0]
                .1
                .iter()
                .any(|arg| arg == "model_reasoning_effort=high"),
            "{:?}",
            started[0].1
        );
        // The meta of the reviewer now holds the entry it was restarted with.
        let recorded = store
            .read(|tx| tx.get_meta("engine.review_model_entry.p:review-1"))
            .unwrap()
            .unwrap();
        let recorded: serde_json::Value = serde_json::from_str(&recorded).unwrap();
        assert_eq!(recorded["model"], "gpt-6.2-codex");
        assert_eq!(recorded["cli"], "codex");
        assert_eq!(recorded["args"][1], "model_reasoning_effort=low");
    }

    /// (b) A lane bound with the CLI opencode, whose config now says the kind
    /// pi with the model M: the restart starts the pi CLI with M — the kind
    /// of the old binding is never used.
    #[test]
    fn a_restarted_lane_takes_the_current_kind_and_model() {
        let (mut store, mut herdr, cfg, pane) = fixture(
            SeatRole::Lane,
            "opencode",
            &config("opencode", "gpt-6.1-sol", "gpt-6.1-sol", "high"),
        );
        store
            .write(|tx| {
                let mut p = tx.get_project("p")?.unwrap();
                p.config_toml = config("pi", "pi-model", "gpt-6.1-sol", "high");
                tx.update_project(&p)
            })
            .unwrap();
        exit_and_restart(&mut store, &mut herdr, &cfg, &pane, "opencode");
        let started = starts(&herdr);
        assert_eq!(started.len(), 1, "{started:?}");
        assert_eq!(started[0].0, "pi", "{started:?}");
        assert!(
            started[0].1.iter().any(|arg| arg == "--model"),
            "{:?}",
            started[0].1
        );
        assert!(
            started[0].1.iter().any(|arg| arg == "pi-model"),
            "{:?}",
            started[0].1
        );
        // The model of the seat follows the restart.
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&crate::model::model_meta_key("p:heat")))
                .unwrap()
                .as_deref(),
            Some("pi-model")
        );
    }

    /// (c) An unchanged config: the restart is exactly as before — the same
    /// model, the same CLI and the same arguments.
    #[test]
    fn an_unchanged_config_restarts_as_before() {
        for role in [SeatRole::Lane, SeatRole::Temp] {
            let (mut store, mut herdr, cfg, pane) = fixture(
                role,
                "codex",
                &config("codex", "gpt-6.1-sol", "gpt-6.1-sol", "high"),
            );
            exit_and_restart(&mut store, &mut herdr, &cfg, &pane, "codex");
            let started = starts(&herdr);
            assert_eq!(started.len(), 1, "{role:?}: {started:?}");
            assert_eq!(started[0].0, "codex", "{role:?}: {started:?}");
            assert!(
                started[0].1.iter().any(|arg| arg == "gpt-6.1-sol"),
                "{:?}",
                started[0].1
            );
            if role == SeatRole::Temp {
                // The reviewer's entry carries the arguments of its chain.
                assert!(
                    started[0]
                        .1
                        .iter()
                        .any(|arg| arg == "model_reasoning_effort=high"),
                    "{:?}",
                    started[0].1
                );
            }
        }
    }
}
