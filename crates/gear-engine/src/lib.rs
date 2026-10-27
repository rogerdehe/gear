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
pub mod delivery;
pub mod dispatch;
pub mod doorbell;
pub mod feed;
pub mod guardrails;
pub mod merge;
pub mod model;
pub mod names;
pub mod observe;
pub mod onboard;
mod opencode;
pub mod ops;
pub mod overrun;
pub mod parallel;
pub mod quota;
pub mod release;
pub mod restart;
pub mod rounds;
pub mod session;
pub mod review;
pub mod start;
pub mod switch;
pub mod trust;
pub mod stall;
pub mod tailed;
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
    /// The wait between two attempts of a merge that failed for a reason that
    /// can clear: the engine sleeps, doubling from 2 s up to 30 s (T-61). A
    /// field so a test injects a hook that never touches the clock (T-127).
    pub merge_retry_wait: merge::RetryWait,
}

impl Default for EngineCfg {
    fn default() -> Self {
        EngineCfg { tick_s: 4, cooldown_s: 90, doorbell_check_budget: 3, delivery_grace_s: 180, profiles: Default::default(), db_path: String::new(), auto_settle: false, merges: Default::default(), pane_busy_ticks: start::MAX_TICKS, model_list_wait_s: 60, model_list_poll_ms: 500, remote: std::sync::Arc::new(merge::GitRemote), review_git: std::sync::Arc::new(review::RealReviewGit), merge_retry_wait: merge::RetryWait::backoff() }
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
    /// Attempts confirmed by a **late** check-in (design §10.3): the delivery
    /// op had already failed, and the view that showed the attempt came after.
    pub late_confirmed: Vec<String>,
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
    /// Station message ids opened at ETA/task checkpoints (`project:M-n`).
    pub stations: Vec<String>,
    /// Lanes alerted for being Working without their own development attempt.
    pub busy_lanes: Vec<String>,
    /// Seats rung for being blocked by a dialog for too long (lesson 7b).
    pub blocked: Vec<String>,
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
    /// The rounds that ended or learned a session this tick (design §4.1).
    pub rounds: crate::rounds::RoundReport,
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
    // The GitHub side of a project in GitHub mode (T-91): every call is an op,
    // so an outage queues them and never stops the tick.
    for project in &projects {
        gear_forge::sync(store, project, now)?;
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

    // 0c. names (T-134): Herdr clears an agent's name when it re-detects the
    //     occupant, and a prompt to an unnamed agent is refused. Read every
    //     agent's name once, then give each seat its name back before anything
    //     is delivered this tick. A failed read is not an observation failure:
    //     the deliveries below still make sure of the name they prompt.
    let names = match names::read(herdr) {
        Ok(names) => names,
        Err(error) => {
            report.name_failed.push(format!("agent.list: {}", error.0));
            std::collections::BTreeMap::new()
        }
    };

    for project in projects {
        // 0d. every seat with an active binding carries its name (`<project>-
        //     <seat>`, 17 §2/§3). A name another pane holds is reported to the
        //     captain once, never stolen.
        if !names.is_empty() {
            let restored = names::restore(store, herdr, &project, &names, now)?;
            report.renamed.extend(restored.renamed);
            report.name_taken.extend(restored.taken);
            report.name_failed.extend(restored.failed);
        }

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
        // A hand restart of an agent is a new session in the same generation
        // (T-97): the binding follows what the pane reports, so a delivery that
        // was never confirmed can be sent again without a store edit.
        observe::record_sessions(store, &observed)?;
        report.busy_lanes.extend(store.write(|tx| busy::check(tx, &project, &observed, now))?);
        // A seat blocked by a dialog for minutes reaches the captain (lesson
        // 7b): the agent waits on a dialog nobody sees.
        report
            .blocked
            .extend(store.write(|tx| blocked::check(tx, &project, &observed, now))?);
        // A lane refused by its own guardrails (pi: @aliou/pi-guardrails) goes
        // on with the refusal in its transcript; the captain is told (T-113).
        report
            .guardrails
            .extend(store.write(|tx| guardrails::check(tx, &project, &observed, now))?);

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
            ops::confirm_pending_with(store, &project, cfg.doorbell_check_budget, cfg.delivery_grace_s, now)?;
        report.confirmed.extend(confirmed);
        report.flagged.extend(flagged);

        // 2b. a late check-in (design §10.3): a view that shows the attempt
        //     after its delivery op failed confirms the attempt anyway and
        //     closes the stall item.
        report.late_confirmed.extend(ops::confirm_late_views(store, &project, now)?);

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

        // 6a. a reviewer seat that runs no reviewer frees its slot (B02-04):
        //     a dead temp seat is released before the reviews start, so the
        //     count of temps in use never passes the quota and the next
        //     review can take the seat.
        report.dead_temps.extend(release::release_dead_temps(store, herdr, &project, now)?);

        // 6c. every lane in the tail while required work remains: one item in
        //     the captain's inbox, so the warnings a captain turned off do not
        //     go forgotten (B02-08).
        report.tail.extend(
            tailed::watch(store, &project, now)?
                .map(|change| format!("all-tail:{}", change.as_str())),
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

        // 8a. retire: a Retiring lane that is idle at a safe point says
        //     goodbye, closes its pane and becomes Retired (T-65).
        report.released.extend(release::retire_finished_lanes(
            store,
            herdr,
            &project,
            &observed,
            &profiles,
            now,
        )?);

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
    // 13. the rounds bookkeeping of every project (design §4.1): the session of
    //     a seat joins its open round, and a round ends at the next round on
    //     its seat or at the seat's release. The overrun question (§5.2) and
    //     the declared waits (§5.3) tick with it; both are empty until their
    //     own tasks land.
    report.rounds = rounds::tick(store, herdr, cfg, now)?;
    overrun::tick(store, herdr, cfg, now)?;
    waits::tick(store, herdr, cfg, now)?;
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
        };
        let mut waiting: Vec<doorbell::DoorbellItem> = Vec::new();
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
            if include && !doorbelled.contains(&message.id) {
                waiting.push(item(&message));
            }
        }

        // Reply/Instruction messages this generation has not shown yet.
        for message in tx.messages_to_since(project, &seat.id, since)? {
            if shown.contains(&message.id)
                || doorbelled.contains(&message.id)
                || !gear_app::seats::message_belongs(seat, &message, since)
                || waiting.iter().any(|item| item.id == message.id)
            {
                continue;
            }
            waiting.push(item(&message));
        }

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
            // Design §10.4: a lane with no open attempt gets the text that
            // tells it to stop working on earlier things.
            no_task: no_task_of(tx, seat)?,
            waiting,
        });
    }
    Ok(inputs)
}
