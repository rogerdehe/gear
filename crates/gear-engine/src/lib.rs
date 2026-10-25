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


pub mod captain_quota;
mod attention;
pub mod blocked;
pub mod busy;
pub mod dispatch;
pub mod doorbell;
pub mod feed;
pub mod merge;
pub mod model;
pub mod observe;
pub mod onboard;
mod opencode;
pub mod ops;
pub mod parallel;
pub mod quota;
pub mod release;
pub mod restart;
pub mod review;
pub mod switch;
pub mod trust;
pub mod stall;

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
    /// How long onboarding waits for a fresh worktree's shell before it gives
    /// up (T-89), and how long between two start attempts. A shell that still
    /// runs the environment line refuses the start with `agent_pane_busy`;
    /// that is "not yet", not a failure, until this wait runs out.
    pub pane_busy_limit_s: i64,
    pub pane_busy_wait_ms: u64,
}

impl Default for EngineCfg {
    fn default() -> Self {
        EngineCfg { tick_s: 4, cooldown_s: 90, doorbell_check_budget: 3, profiles: Default::default(), db_path: String::new(), auto_settle: false, merges: Default::default(), pane_busy_limit_s: 20, pane_busy_wait_ms: 250 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Projects whose observation failed this tick (nothing decided for them).
    pub observe_failed: Vec<String>,
    /// Op ids that went Intended this tick.
    pub intended: Vec<String>,
    /// (op id, resulting state) for ops executed this tick.
    pub executed: Vec<(String, String)>,
    /// Op ids confirmed by evidence this tick.
    pub confirmed: Vec<String>,
    /// Seats flagged for attention (e.g. doorbell evidence budget exhausted).
    pub flagged: Vec<String>,
    /// Seats onboarded this tick (worktree, pane and agent all up).
    pub onboarded: Vec<String>,
    /// Quota switches and parks this tick, one line each (design/01 `on_quota`).
    pub quota: Vec<String>,
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
    /// Tasks merged into the integration branch this tick (`task @ branch`).
    pub merged: Vec<String>,
    /// Lane items for the captain opened, updated or closed this tick
    /// (`lane:action`), from the keep-lanes-fed watch.
    pub feed: Vec<String>,
    /// Station message ids opened at ETA/task checkpoints (`project:M-n`).
    pub stations: Vec<String>,
    /// Lanes alerted for being Working without their own development attempt.
    pub busy_lanes: Vec<String>,
    /// Seats rung for being blocked by a dialog for too long (lesson 7b).
    pub blocked: Vec<String>,
    /// Seats that wait for their provider (T-75): a provider back-off is not
    /// a stall, and the home page shows it.
    pub backoff: Vec<String>,
    /// Parallel-change warning message ids (`project:M-n`).
    pub parallel: Vec<String>,
    /// Git evidence that could not be read during a parallel-change scan.
    pub parallel_failed: Vec<String>,
}

/// Run one engine tick over every project in the store.
pub fn tick(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &EngineCfg, now: Ts) -> GearResult<TickReport> {
    let mut report = TickReport::default();

    // 0. onboard: every queued lane gets its worktree, pane and agent. A seat
    //    that fails a step is parked in NeedsCheck, never retried blindly.
    report.onboarded = onboard::onboard_queued(store, herdr, cfg, now)?;

    // 0b. settle: a review verdict that arrived settles its task without the
    //     captain (T-13) — before the dispatch below, so a pass frees the
    //     task's dependants. Failing verdicts wait for the captain's judgement.
    //     Settlement needs no Herdr, so it runs even
    //     when the observation fails.
    if cfg.auto_settle {
        let (settled, settle_failed) = settle_reviews(store, now)?;
        report.settled = settled;
        report.settle_failed = settle_failed;
    }

    let projects = store.read(|tx| tx.project_ids())?;
    for project in &projects {
        let checked = parallel::check(store, project, now)?;
        report.parallel.extend(checked.opened.into_iter().map(|id| format!("{project}:{id}")));
        report.parallel_failed.extend(checked.problems.into_iter().map(|error| format!("{project}: {error}")));
    }
    // Station facts need no Herdr observation. Enqueue them before deciding
    // doorbells, including when a previous outage delayed a checkpoint.
    for project in &projects {
        let ids = store.write(|tx| gear_app::station::checkpoints(tx, project, now))?;
        report.stations.extend(ids.into_iter().map(|id| format!("{project}:{id}")));
    }

    // 1. observe: one agents() call for the whole session. If it fails, every
    //    project is marked and nothing at all is decided this tick.
    let Ok(agents) = observe::snapshot(herdr) else {
        // No evidence of continuous work across a failed observation.
        for project in &projects {
            store.write(|tx| busy::observation_lost(tx, project))?;
        }
        report.observe_failed = projects;
        return Ok(report);
    };

    for project in projects {
        // Seat observations (reads screen tails; no write transaction is held).
        // The profile table of this project: the built-ins with the project's
        // own `[profiles.<name>]` on top (T-31). A config that does not parse
        // leaves the built-ins in place (the lanes are parked by the onboard
        // step instead).
        let profiles = match gear_app::config::latest(store, &project) {
            Ok(config) => gear_app::config::profile_table(&cfg.profiles, &config)?,
            Err(_) => cfg.profiles.clone(),
        };
        let observed = store.read(|tx| observe::reconcile(tx, herdr, &project, &agents, &profiles))?;
        report.busy_lanes.extend(store.write(|tx| busy::check(tx, &project, &observed, now))?);
        // A seat blocked by a dialog for minutes reaches the captain (lesson
        // 7b): the agent waits on a dialog nobody sees.
        report
            .blocked
            .extend(store.write(|tx| blocked::check(tx, &project, &observed, now))?);

        // T-75: a seat whose provider is in a back-off waits for the provider.
        // It is not stuck, so no stall item is raised for it, and the captain's
        // home page shows it as a back-off.
        let waiting = stall::waiting_for_provider(&profiles, &observed);
        store.write(|tx| tx.set_seats_in_backoff(&project, &waiting))?;
        report.backoff.extend(waiting);

        captain_quota::update(store, herdr, &profiles, &project, &observed, now)?;

        // 1a. a lane or reviewer whose agent is gone from its pane (it exited,
        //     the pane was closed) reaches the captain at once (L-56): the
        //     seat still says Active, so nothing else would ever notice.
        for obs in &observed {
            if matches!(obs.presence, observe::Presence::Missing)
                && obs.seat.state == gear_model::SeatState::Active
                && matches!(obs.seat.role, gear_model::SeatRole::Lane | gear_model::SeatRole::Temp)
                && !report.flagged.contains(&obs.seat.id)
            {
                report.flagged.push(obs.seat.id.clone());
            }
        }

        // 1b. quota: a lane on a provider limit switches model or parks
        //     (design/01 `[models].on_quota`). It runs before the confirm, so a
        //     lane that is on quota does not have its delivery budget spent.
        let quota = quota::apply_quota_policy(store, herdr, cfg, &profiles, &project, &observed, now)?;
        report.quota.extend(quota.events);

        // 2. confirm: settle Submitted/Unknown doorbells from view evidence.
        let (confirmed, flagged) =
            ops::confirm_pending(store, &project, cfg.doorbell_check_budget, now)?;
        report.confirmed.extend(confirmed);
        report.flagged.extend(flagged);

        attention::promote(store, &project, now)?;

        // 3. dispatch: per lane seat a view and a decision, read-only with this
        //    tick's observations …
        let dispatch_cfg = dispatch::DispatchCfg {
            cooldown_s: cfg.cooldown_s,
            ..Default::default()
        };
        let planned =
            store.read(|tx| dispatch::plan_dispatch(tx, &project, &observed, &dispatch_cfg, now))?;
        for (seat, action) in &planned {
            // An escalated seat needs a human; nothing is sent to it.
            if matches!(action, dispatch::Action::Escalate { .. })
                && !report.flagged.contains(&seat.id)
            {
                report.flagged.push(seat.id.clone());
            }
        }
        // … then the attempts and their deliveries in one write transaction.
        let deliveries = store.write(|tx| dispatch::intend_dispatch(tx, &project, &planned, now))?;
        report.intended.extend(deliveries.into_iter().map(|op| op.id));

        // 6. feed: a lane running dry gets one item in the captain's inbox,
        //    before it stops (never-stop). Idempotent per lane and tick.
        report.feed.extend(
            feed::keep_lanes_fed(store, &project, now)?
                .into_iter()
                .map(|change| format!("{}:{}", change.lane, change.action)),
        );

        // 7. review: a lane handed a result in → a temp reviewer starts. The
        //    lane is not blocked; the reviewer is a seat of its own.
        let reviews = review::start_reviews(store, herdr, cfg, &profiles, &project, now)?;
        report.reviewers.extend(reviews.started);

        // 7b. a reviewer that stopped before its verdict is told to go on
        //     (L-53); past its nudge budget it is flagged for the captain.
        let (_nudged, stuck) =
            review::nudge_stopped_reviewers(store, &project, &observed, cfg.cooldown_s, now)?;
        for seat in stuck {
            if !report.flagged.contains(&seat) {
                report.flagged.push(seat);
            }
        }

        // 8. release: a reviewer whose task left review frees its slot (T-17).
        let releases = release::release_finished_reviews(store, herdr, &project, now)?;
        report.released.extend(releases.released);

        // 9. decide (pure): who should be rung now. This comes after every
        //    delivery of the tick: a seat that is being handed work is not
        //    rung in the same tick (T-71) — on T-61 both went into the
        //    reviewer's pane in one second and the packet was swallowed. The
        //    delivery holds the seat's slot, so the doorbell waits.
        let inputs = store.read(|tx| doorbell_inputs(tx, &project, &observed, now))?;
        let actions = doorbell::decide(&inputs, cfg.cooldown_s, now);

        // 10. intend: fix the doorbell actions as Intended ops in one transaction.
        let intended = store.write(|tx| ops::intend_doorbells(tx, &actions, now))?;
        report.intended.extend(intended.into_iter().map(|op| op.id));

        // 11. execute: re-check each op, call Herdr, record the outcome.
        report.executed.extend(ops::execute_intended(store, herdr, &project, now)?);

        // 12. merge: collect a finished background merge or launch the next
        //     one. Git and the gate run beside the tick (T-36).
        for merged in merge::merge_passed(store, herdr, cfg, &project, now)? {
            report.merged.push(format!("{} @ {}", merged.task, merged.branch));
        }
    }
    // 10. every flagged seat reaches its captain (L-52): a flag that only goes
    //     to the log leaves a lane standing still with nobody knowing.
    for seat in report.flagged.clone() {
        if let Err(e) = store.write(|tx| stall::alert(tx, &seat, now)) {
            // One seat's alert failing never stops the tick.
            report.settle_failed.push(format!("stall alert {seat}: {}", e.detail));
        }
    }
    Ok(report)
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
        for review in store.read(|tx| tx.reviews_to_settle(&project))?.into_iter().filter(|review| review.passed) {
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

/// What the doorbell decision needs to know about one seat (design/07).
fn doorbell_inputs(
    tx: &gear_store::Tx<'_>,
    project: &str,
    observed: &[observe::SeatObs],
    now: Ts,
) -> GearResult<Vec<doorbell::DoorbellInput>> {
    use gear_model::{OpKind, OpState};
    use gear_store::BindingStatus;
    use std::collections::BTreeSet;

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
        let mut actionable = Vec::new();
        for message in tx.actionable_for(project, &seat.id, now)? {
            let include = if seat.role == gear_model::SeatRole::Captain {
                // The captain's open items are the engine's business: they
                // ring until they are answered, whatever generation took the
                // seat over (F2 of the T-71 review).
                attention::should_show(tx, &message, shown.contains(&message.id), period, now)?
            } else {
                gear_app::seats::message_belongs(seat, &message, since)
                    && !shown.contains(&message.id)
            };
            if include { actionable.push(message); }
        }
        let urgent = actionable.iter().filter(|m| m.urgent).count();
        let actionable_ids: Vec<String> = actionable.into_iter().map(|m| m.id).collect();

        // Reply/Instruction messages this generation has not shown yet.
        let unshown: Vec<String> = tx
            .messages_to_since(project, &seat.id, since)?
            .into_iter()
            .filter(|m| !shown.contains(&m.id) && gear_app::seats::message_belongs(seat, m, since))
            .map(|m| m.id)
            .collect();

        // A delivery/doorbell that still occupies the seat's slot, and when the
        // previous doorbell was settled (for the cooldown).
        let slot_busy = !tx.slot_ops(&seat.id)?.is_empty();
        let last_settled_at = tx
            .last_op(&seat.id, OpKind::Doorbell)?
            .filter(|op| matches!(op.state, OpState::Confirmed | OpState::Failed | OpState::Void))
            .map(|op| op.updated);

        inputs.push(doorbell::DoorbellInput {
            project: project.to_string(),
            seat: seat.id.clone(),
            generation: seat.generation,
            pane_id: binding.pane_id,
            // Only a seat that may receive work is rung, even if its agent is
            // idle (a parked/held seat must not be woken by a doorbell).
            idle: seat_obs.idle && observe::reachable(seat.state),
            slot_busy,
            last_settled_at,
            actionable: actionable_ids,
            unshown,
            urgent,
        });
    }
    Ok(inputs)
}
