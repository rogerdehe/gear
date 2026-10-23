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


pub mod dispatch;
pub mod doorbell;
pub mod observe;
pub mod onboard;
pub mod ops;

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
}

impl Default for EngineCfg {
    fn default() -> Self {
        EngineCfg { tick_s: 4, cooldown_s: 90, doorbell_check_budget: 3, profiles: Default::default(), db_path: String::new() }
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
    /// `project:task/attempt` of the review verdicts that settled a task this
    /// tick (T-13).
    pub settled: Vec<String>,
}

/// Run one engine tick over every project in the store.
pub fn tick(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &EngineCfg, now: Ts) -> GearResult<TickReport> {
    let mut report = TickReport::default();

    // 0. onboard: every queued lane gets its worktree, pane and agent. A seat
    //    that fails a step is parked in NeedsCheck, never retried blindly.
    report.onboarded = onboard::onboard_queued(store, herdr, cfg, now)?;

    // 0b. settle: a review verdict that arrived settles its task without the
    //     captain (T-13) — before the dispatch below, so a pass frees the
    //     task's dependants and a fail puts the rework at the head of the
    //     lane's queue in this same tick. It needs no Herdr, so it runs even
    //     when the observation fails.
    report.settled = settle_reviews(store, now)?;

    let projects = store.read(|tx| tx.project_ids())?;

    // 1. observe: one agents() call for the whole session. If it fails, every
    //    project is marked and nothing at all is decided this tick.
    let Ok(agents) = observe::snapshot(herdr) else {
        report.observe_failed = projects;
        return Ok(report);
    };

    for project in projects {
        // Seat observations (reads screen tails; no write transaction is held).
        let observed = store.read(|tx| observe::reconcile(tx, herdr, &project, &agents, &cfg.profiles))?;

        // 2. confirm: settle Submitted/Unknown doorbells from view evidence.
        let (confirmed, flagged) =
            ops::confirm_pending(store, &project, cfg.doorbell_check_budget, now)?;
        report.confirmed.extend(confirmed);
        report.flagged.extend(flagged);

        // 3. decide (pure): who should be rung now.
        let inputs = store.read(|tx| doorbell_inputs(tx, &project, &observed, now))?;
        let actions = doorbell::decide(&inputs, cfg.cooldown_s, now);

        // 4. intend: fix the doorbell actions as Intended ops in one transaction.
        let intended = store.write(|tx| ops::intend_doorbells(tx, &actions, now))?;
        report.intended.extend(intended.into_iter().map(|op| op.id));

        // 5. dispatch: per lane seat a view and a decision, read-only with this
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

        // 6. execute: re-check each op, call Herdr, record the outcome.
        report.executed.extend(ops::execute_intended(store, herdr, &project, now)?);
    }
    Ok(report)
}

/// Settle every review verdict whose attempt is still handed in (T-13), in
/// every project. One `settle_review` call per verdict; a verdict that already
/// settled its attempt is not listed (and would be a no-op anyway).
fn settle_reviews(store: &mut Store, now: Ts) -> GearResult<Vec<String>> {
    let mut settled = Vec::new();
    for project in store.read(|tx| tx.project_ids())? {
        for review in store.read(|tx| tx.reviews_to_settle(&project))? {
            store.write(|tx| {
                gear_app::tasks::settle_review(tx, &project, &review.task, review.attempt, now)
            })?;
            settled.push(format!("{project}:{}/{}", review.task, review.attempt));
        }
    }
    Ok(settled)
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
        // A seat without an active binding has no pane to ring.
        let Some(binding) = tx
            .get_binding(&seat.id, seat.generation)?
            .filter(|b| b.status == BindingStatus::Active)
        else {
            continue;
        };

        // What this generation has already been shown: an open item it has
        // seen does not ring again (lesson L-42) — it waits for the seat's own
        // next `gear`.
        let shown: BTreeSet<String> = tx
            .views_since(&seat.id, seat.generation, 0)?
            .into_iter()
            .flat_map(|view| view.shown_msgs)
            .collect();
        let actionable: Vec<_> = tx
            .actionable_for(project, &seat.id, now)?
            .into_iter()
            .filter(|m| !shown.contains(&m.id))
            .collect();
        let urgent = actionable.iter().filter(|m| m.urgent).count();
        let actionable_ids: Vec<String> = actionable.into_iter().map(|m| m.id).collect();

        // Reply/Instruction messages this generation has not shown yet.
        let unshown: Vec<String> = tx
            .messages_to_since(project, &seat.id, 0)?
            .into_iter()
            .filter(|m| !shown.contains(&m.id))
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
