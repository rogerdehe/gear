//! `gear plan apply` (design/05 "Apply flow"), first cut: the `# Queue`
//! section. One transaction via `run_request`; an invalid draft writes
//! nothing. `# Verdicts` (T-50) and `# Staff` park/hold/retire (T-55) are applied here; `temp` staff lines and `# Review` come in later tasks and are
//! rejected here when non-empty (`BadFormat`, naming the section).

use crate::draft::{Draft, Priority, QueueItem, StaffItem};
use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::state::require_transition;
use gear_model::{
    AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, SeatRole, SeatState,
    TaskState, Ts,
};
use gear_store::{
    BindingStatus, ContractRow, NewMessage, PlanRow, RequestOutcome, Store, TaskRow, Tx,
};
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApplyResult {
    /// The new plan version (base + 1).
    pub version: i64,
    /// Ids assigned to `## NEW` blocks, in draft order.
    pub created: Vec<String>,
    /// Existing tasks whose contract got a new version: (id, new version).
    pub contract_changed: Vec<(String, i64)>,
    /// Non-terminal tasks the draft did not mention (kept, ranked after the listed ones).
    pub not_mentioned: Vec<String>,
    /// Verdicts settled by this apply, `<task> attempt <n> pass|fail`, in draft order (T-50).
    #[serde(default)]
    pub settled: Vec<String>,
    /// Staff targets applied (T-55): `park heat`, `hold wire`, `retire prompts`, in draft order.
    #[serde(default)]
    pub staffed: Vec<String>,
    /// Tasks cancelled by this apply (T-112): a finished task whose accepted
    /// work was never merged.
    #[serde(default)]
    pub cancelled: Vec<String>,
    /// Items closed with a cancellation (T-112): the merge and push items of a
    /// cancelled done task, resolved "cancelled by the captain".
    #[serde(default)]
    pub resolved: Vec<String>,
    /// The `# Milestone` blocks this apply wrote back into the plan file from
    /// the store (T-333), in the order their milestones are first named.
    #[serde(default)]
    pub milestone_blocks: Vec<String>,
    /// What the captain should know about this apply (design §7.2: a decision
    /// line that is not structured is logged as a note).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Apply `draft` (parsed from `text`, which is stored verbatim as the plan row).
///
/// 1. identity: captain seat (`require_seat` with role Captain, states Active);
/// 2. `draft.project` = the caller's project, else `BadFormat`;
/// 3. `draft.base` ≠ current `plan_version` → `StaleVersion`;
/// 4. non-empty Review → `BadFormat` (not in this cut). `# Staff` (T-55): `park`,
///    `hold`, `retire` change the named **Lane** seat of this project directly in
///    the transaction (design/05 step 5) via `SeatState::can_transition`
///    (Parked / Held / Retiring); `state_reason` records `until=<value>`,
///    `until-task=<T-n>`, `hold: <reason>` or `retire`; a target already in that
///    state is skipped (Staff is a target, not a command); an unknown seat, a
///    non-lane seat or a forbidden transition → `BadFormat`; `temp` lines →
///    `BadFormat` "not in this cut"; applied lines are listed in `staffed`.
///    `release` (T-133, T-140): a named **lane** in Held goes back to Active or
///    Queued (the session's agents say whether its agent is still there); a
///    named **reviewer** seat in Standby goes to Releasing (the engine closes
///    its pane); a reviewer that is reviewing is refused with the task it is
///    reviewing.
///    `# Verdicts` (T-50):
///    after the queue writes, each line → `tasks::settle` (by_seat = the caller's
///    seat) in draft order, in the same transaction — any error aborts the whole
///    apply; settled lines are listed in `settled`;
/// 5. validation, nothing written on failure (`BadFormat`, detail names the task/line):
///    - an existing id must exist; a `NEW` block needs `lane`, `goal`, `accept`
///      and `estimate` (the planned time in minutes, T-38);
///    - every dep names an existing task or a task of this draft; no cycle;
///    - no task that is still to be worked on (not `Done`, `Failed` or
///      `Cancelled`) keeps a deps entry naming a task that is or becomes
///      `Cancelled` in this apply (T-428): a cancelled dep never becomes Done
///      and merged, so such a task would never be delivered — one line per
///      pair, `<id> depends on cancelled <dep>: drop or replace that
///      dependency`, and nothing is applied;
///    - `state=` only on existing tasks: `cancelled` from any non-terminal state,
///      and from Done when the task's accepted candidate is **not** on its
///      integration branch (T-112: its merge was given up; a done task whose
///      work is merged stays uncancellable); `ready` only from Draft (a Done
///      task reopens by a `# Verdicts` fail, T-90, not by a queue line);
///    - a `NEW` block, or an existing task's block that changes its milestone,
///      whose `milestone=` names a milestone that has neither a
///      `# Milestone <id>` block in this plan file nor a stored voyage with a
///      non-empty done sentence (T-442) → `<task>: milestone <id> is not
///      described: add a `# Milestone <id>` block with its done, bar and eta,
///      or use a milestone the plan describes`. A task already in such a
///      milestone that the plan does not move is history and stays, and
///      cancelling a task keeps its milestone (a cancelled block that moves
///      one is checked like any other);
/// 6. writes: `NEW` → `T-<n>` via `next_id`, state Ready, contract v1; an existing
///    task whose goal/accept/evidence differs from its current contract → new
///    contract version; rank = position in the draft (1, 2, …), unmentioned
///    non-terminal tasks keep their relative order after them; lane, milestone,
///    required, priority, deps and the planned time (`estimate:`, T-38) as
///    written; `insert_plan(version = base + 1)`;
///    an existing task's block: absent `goal`/`accept` keep the current text
///    (they cannot be empty), absent `evidence-files`/`evidence-due` mean none —
///    an exported draft always writes every field (R-33 decision);
/// 7. replay: same `request_id` + same text → the stored result.
pub fn apply(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    agents: Option<&[gear_herdr::AgentInfo]>,
    draft: &Draft,
    text: &str,
    request_id: &str,
    now: Ts,
) -> GearResult<ApplyResult> {
    // T-112: a Done task may be cancelled only when its accepted work is not on
    // the integration branch. That is a git question, so it is answered here,
    // before the write transaction (the store is never held while a subprocess
    // runs) and only for the done tasks this draft cancels.
    let cancellable = cancellable_done_tasks(store, draft)?;
    // T-92 F1: a remerge of a candidate that is already on the integration
    // branch has nothing to queue. Also a git question, asked here for the
    // same reason: the store is never held while a subprocess runs.
    let already_merged = remerges_already_merged(store, draft)?;
    // The verdicts that name the merge that landed their work are checked
    // before anything is written (git runs outside the store transaction).
    require_integrated_by(store, draft)?;

    store.write(|tx| {
        // 1. identity: only the captain applies a plan.
        let (ctx, seat) = require_seat(
            tx,
            env,
            fp,
            Some(&[SeatRole::Captain]),
            Some(&[SeatState::Active]),
        )?;

        // 2. the draft must be for the caller's project.
        if draft.project != ctx.project {
            return Err(bad(
                1,
                format!(
                    "the draft is for project {:?}, the caller is in {:?}",
                    draft.project, ctx.project
                ),
            ));
        }

        let args_key = format!(
            "{}|{}:{}|plan-apply|base={}|text={:?}",
            ctx.project, ctx.seat, ctx.generation, draft.base, text
        );
        let outcome = tx.run_request(request_id, &args_key, now, |tx| {
            // Steps 3 to 6 run inside the closure, so a replay returns the
            // stored result instead of meeting a moved plan version.
            // T-461: the departure and the move this draft would make, read
            // before the queue writes so the guard can tell a new milestone
            // from one that already carried tasks.
            let guard = planned_guard(tx, &ctx.project, draft)?;
            let result = apply_queue(
                tx,
                &ctx.project,
                &seat.id,
                ctx.generation,
                draft,
                text,
                &cancellable,
                &already_merged,
                agents,
                now,
            )?;
            // The milestone blocks of the draft become voyages (T-22), after the
            // queue so the task count at departure is the plan's.
            record_tail_decisions(tx, &ctx.project, draft)?;
            // T-410: which voyages had departed before this apply, read before
            // fix_departures moves a departure.
            let departed = departed_voyages(tx, &ctx.project)?;
            fix_departures(tx, &ctx.project, now)?;
            record_voyages(tx, &ctx.project, draft, &departed, now)?;
            record_task_stations(tx, &ctx.project, draft, &result.created)?;
            // T-461: after every validation of the file has spoken, a new
            // milestone's departure and a move between milestones need the
            // user's word: --confirmed and its `confirmed:` line.
            require_confirmed_changes(draft, &guard.0, &guard.1)?;
            // T-461: the captain's goal check of the voyage's done clauses,
            // written before a landing of this same apply reads it.
            let checked = voyage_of(tx, &ctx.project, draft)?;
            crate::goal::record_goal_check(tx, &ctx.project, &checked, draft)?;
            crate::station::apply_decisions(tx, &ctx.project, draft, now)?;
            serde_json::to_value(&result).map_err(|e| {
                GearError::new(
                    ErrorCode::BadFormat,
                    format!("cannot serialize the apply result: {e}"),
                )
            })
        })?;

        let value = match outcome {
            RequestOutcome::Executed(value) | RequestOutcome::Replayed(value) => value,
        };
        serde_json::from_value(value).map_err(|e| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("the stored apply result is broken: {e}"),
            )
        })
    })
}

/// Steps 3 to 6 of the apply flow, inside the request closure (so a replay
/// returns the stored result without meeting a moved plan version).
fn apply_queue(
    tx: &Tx<'_>,
    project: &str,
    by_seat: &str,
    by_gen: i64,
    draft: &Draft,
    text: &str,
    cancellable: &BTreeSet<String>,
    already_merged: &BTreeSet<String>,
    agents: Option<&[gear_herdr::AgentInfo]>,
    now: Ts,
) -> GearResult<ApplyResult> {
    // 3. the draft must be based on the current plan version.
    let current = tx.plan_version(project)?;
    if draft.base != current {
        return Err(GearError::new(
            ErrorCode::StaleVersion,
            format!(
                "the draft is based on plan version {} but the project is at {current}",
                draft.base
            ),
        ));
    }

    // 4. sections that belong to later cuts (after the base check, as the doc
    // comment orders the steps).
    reject_sections(draft)?;

    // The captain's decision lines (design §7): every line of `# Decisions` is
    // read, structured or a note (§7.2), and recorded (§7.3).
    let decisions: Vec<crate::decision::Decision> = draft
        .decisions
        .iter()
        .map(|line| crate::decision::parse(line))
        .collect();
    let voyage = voyage_of(tx, project, draft)?;
    let applied_version = current + 1;
    let mut warnings: Vec<String> = Vec::new();
    for (index, line) in draft.decisions.iter().enumerate() {
        // T-461: a goal-check line is a check of the voyage's done clauses,
        // written to the board's meta below; it is no decision of the log.
        if crate::goal::is_done_line(line) {
            continue;
        }
        let decision = &decisions[index];
        if decision.note {
            warnings.push(format!(
                "warning: decision line {} is not structured; logged as a note",
                index + 1
            ));
        }
        record_decision(tx, project, &voyage, applied_version, line, decision, now)?;
    }

    // Apply rebinds before taking the existing-task and config snapshots:
    // an idle lane's NEW tasks belong to its destination repo. A deferred
    // rebind keeps the old config until the lane's attempts finish.
    let mut staffed: Vec<String> = Vec::new();
    for item in &draft.staff {
        if let StaffItem::Rebind {
            lane,
            repo,
            owns,
            line,
        } = item
        {
            // The old values come from the stored config, before the request:
            // an idle lane's rebind is applied in this call (T-305).
            let (old_repo, old_owns) = lane_binding(tx, project, lane, *line)?;
            let waiting = request_rebind(tx, project, lane, repo, owns, *line)?;
            let preview = rebind_preview(&old_repo, repo, &old_owns, owns);
            let message = if waiting.is_empty() {
                let mut text = format!("rebind {lane}: {preview}: applied");
                // The undo is the rebind line back to the old binding; a lane
                // in the project root has none (a repo is named, not "root").
                if !old_repo.is_empty() {
                    text.push_str(&format!(
                        " (to undo: - rebind {lane} repo={old_repo} owns=[{}])",
                        old_owns.join(", ")
                    ));
                }
                text
            } else {
                format!(
                    "rebind {lane}: {preview}: waits for {}; nothing changes until then (to withdraw: - rebind {lane} cancel)",
                    waiting.join(", ")
                )
            };
            warnings.push(message.clone());
            staffed.push(message);
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "staff",
                &staff_text(item),
                None,
                &staff_text(item),
                now,
            )?;
            continue;
        }
        if let StaffItem::RebindCancel { lane, line } = item {
            let message = cancel_rebind(tx, project, lane, *line)?;
            warnings.push(message.clone());
            staffed.push(message);
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "staff",
                &staff_text(item),
                None,
                &staff_text(item),
                now,
            )?;
            continue;
        }
    }

    let existing: BTreeMap<String, TaskRow> = tx
        .tasks_of_project(project)?
        .into_iter()
        .map(|task| (task.id.clone(), task))
        .collect();

    // Ids for the `## NEW` blocks, in draft order (rolled back with a failure).
    let mut new_ids: BTreeMap<usize, String> = BTreeMap::new();
    for (index, item) in draft.queue.iter().enumerate() {
        if item.id.is_none() {
            new_ids.insert(index, tx.next_id(project, 'T')?);
        }
    }
    let known: BTreeSet<String> = existing
        .keys()
        .cloned()
        .chain(new_ids.values().cloned())
        .collect();

    // What the cancellations of this apply did (T-112).
    let mut cancelled: Vec<String> = Vec::new();
    let mut resolved: Vec<String> = Vec::new();

    // 5. validation, nothing written on failure. The project's cap on a
    // task's planned time (T-279) is read once for the whole apply:
    // `[conventions].max_task_min` from the stored config, the default when
    // none parses. A rework estimate (`estimate=` on a fail verdict) is not
    // capped: it is the price of finishing a task that is already open.
    let max_task_min = tx
        .get_project(project)?
        .and_then(|row| crate::config::parse(&row.config_toml).ok())
        .map(|config| config.conventions.max_task_min)
        .unwrap_or(crate::config::MAX_TASK_MIN);
    let mut graph: BTreeMap<String, Vec<String>> = existing
        .iter()
        .map(|(id, task)| (id.clone(), task.deps.clone()))
        .collect();
    for (index, item) in draft.queue.iter().enumerate() {
        match &item.id {
            Some(id) => {
                let task = existing
                    .get(id)
                    .ok_or_else(|| bad(item.line, format!("task {id} does not exist")))?;
                match item.state.as_deref() {
                    None => {}
                    Some("cancelled") => {
                        // Cancelling a cancelled task is a no-op, not an error
                        // (T-93: a plan keeps saying so after the cancel, and
                        // re-applying it failed three times in M3). A finished
                        // task is not cancelled after the fact — except a Done
                        // task whose accepted work never reached the
                        // integration branch (T-112): its merge was given up,
                        // and cancelling is how the captain drops it.
                        let done_and_unmerged =
                            task.state == TaskState::Done && cancellable.contains(id);
                        if !done_and_unmerged
                            && matches!(task.state, TaskState::Done | TaskState::Failed)
                        {
                            return Err(bad(
                                item.line,
                                format!(
                                    "task {id} is {}, it cannot be cancelled",
                                    task.state.as_str()
                                ),
                            ));
                        }
                    }
                    Some("ready") => {
                        if task.state != TaskState::Draft {
                            return Err(bad(
                                item.line,
                                format!(
                                    "task {id} is {}, only a draft task becomes ready here",
                                    task.state.as_str()
                                ),
                            ));
                        }
                    }
                    Some(other) => {
                        return Err(bad(item.line, format!("unknown state {other:?}")));
                    }
                }
            }
            None => {
                if item.lane.is_none() {
                    return Err(bad(item.line, "a NEW block needs `lane=`"));
                }
                if item.goal.is_none() {
                    return Err(bad(item.line, "a NEW block needs `goal:`"));
                }
                if item.accept.is_none() {
                    return Err(bad(item.line, "a NEW block needs `accept:`"));
                }
                // The time of a new task is planned, not guessed later (T-38):
                // a NEW block without an estimate is refused. An existing task
                // may keep the estimate it has.
                if item.estimate.is_none() {
                    return Err(bad(
                        item.line,
                        "a NEW block needs `estimate:` (the planned time in minutes)",
                    ));
                }
                if item.state.is_some() {
                    return Err(bad(item.line, "`state=` is only for existing tasks"));
                }
            }
        }
        // A task sails on a voyage the plan describes (T-442): a milestone no
        // `# Milestone` block in this file and no departure with a done
        // sentence in the store describes is a typo or a made-up id, and a
        // voyage row for it carried no words at all. A task already in such a
        // milestone that the plan does not move is history and stays as it is;
        // a block that moves a task into one, or a `NEW` block, is refused.
        // Cancelling keeps a task's milestone, so a cancelled block is checked
        // like any other that moves one.
        if let Some(milestone) = item.milestone.as_deref() {
            let moves = match &item.id {
                Some(id) => existing
                    .get(id)
                    .is_none_or(|task| task.milestone.as_deref() != Some(milestone)),
                None => true,
            };
            if moves && !milestone_described(tx, project, draft, milestone)? {
                let task = item.id.clone().unwrap_or_else(|| "NEW".to_string());
                return Err(bad(
                    item.line,
                    format!(
                        "{task}: milestone {milestone} is not described: add a `# Milestone {milestone}` block with its done, bar and eta, or use a milestone the plan describes"
                    ),
                ));
            }
        }
        // T-461: a cancel line changes the state only. Every other field it
        // carries must be the task's current one; a cancel that also moves
        // the task, its lane or its words is refused, so the cut and the
        // change stay apart in the log. Checked after the milestone is known
        // to be described, so a move into an undescribed milestone still
        // names that milestone.
        if item.state.as_deref() == Some("cancelled") {
            if let Some(task) = item.id.as_ref().and_then(|id| existing.get(id)) {
                cancel_changes_only_state(tx, project, item, task)?;
            }
        }
        // A task that would take longer than the project's cap is cut into
        // tasks each with its own test cycle (T-279): a lane works
        // unattended and is reviewed only at its hand-in, so a long task is a
        // long stretch nobody checks.
        if let Some(estimate) = item.estimate {
            if estimate > max_task_min {
                let task = item.id.clone().unwrap_or_else(|| "NEW".to_string());
                return Err(bad(
                    item.line,
                    format!(
                        "{task}: estimate {estimate} min is over the project's cap of {max_task_min} min: cut it into tasks of at most {max_task_min} min, each with its own test cycle ([conventions] max_task_min)"
                    ),
                ));
            }
        }
        for dep in &item.deps {
            if !known.contains(dep) {
                return Err(bad(
                    item.line,
                    format!("dep {dep} does not exist and is not created by this draft"),
                ));
            }
        }
        // The follow-up names a task that exists (design §3.3): the field is
        // live, and a task that does not exist cannot be the one whose
        // session the lane continues.
        if let Some(followup) = &item.followup_of {
            if !existing.contains_key(followup) {
                return Err(bad(
                    item.line,
                    format!("followup-of {followup} does not exist"),
                ));
            }
        }
        let id = match &item.id {
            Some(id) => id.clone(),
            None => new_ids
                .get(&index)
                .cloned()
                .ok_or_else(|| bad(item.line, "cannot assign an id to this NEW block"))?,
        };
        graph.insert(id, item.deps.clone());
    }
    if let Some(cycle) = find_cycle(&graph) {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!("the dependencies form a cycle through {cycle}"),
        ));
    }
    // A plan never leaves a task waiting for a cancelled one (T-428): a
    // cancelled dep never becomes Done and merged, so the engine never
    // delivers the task and nothing says why (T-358 and T-368 kept a
    // cancelled T-332 in their deps; the other project's T-140 and T-178 wait
    // on its cancelled T-186). Every task of the project counts, with the
    // cancellations and dep changes of this draft applied; one line per pair.
    let mut states: BTreeMap<String, TaskState> = existing
        .iter()
        .map(|(id, task)| (id.clone(), task.state))
        .collect();
    for (index, item) in draft.queue.iter().enumerate() {
        let id = match &item.id {
            Some(id) => Some(id.clone()),
            None => new_ids.get(&index).cloned(),
        };
        let Some(id) = id else { continue };
        let state = match item.state.as_deref() {
            Some("cancelled") => TaskState::Cancelled,
            Some("ready") => TaskState::Ready,
            _ => states.get(&id).copied().unwrap_or(TaskState::Ready),
        };
        states.insert(id, state);
    }
    let waiting: Vec<String> = graph
        .iter()
        .filter(|(id, _)| {
            !matches!(
                states.get(id.as_str()),
                Some(TaskState::Cancelled | TaskState::Done | TaskState::Failed)
            )
        })
        .flat_map(|(id, deps)| {
            deps.iter()
                .filter(|dep| {
                    states
                        .get(dep.as_str())
                        .is_some_and(|state| *state == TaskState::Cancelled)
                })
                .map(move |dep| {
                    format!("{id} depends on cancelled {dep}: drop or replace that dependency")
                })
        })
        .collect();
    if !waiting.is_empty() {
        return Err(GearError::new(ErrorCode::BadFormat, waiting.join("\n")));
    }

    // The `# Milestone` blocks the draft owes the file (T-333): a task sails on
    // a voyage the file describes, so a milestone the plan names and does not
    // describe gets its block back from the store — or the apply is refused.
    let milestone_blocks = crate::goal::missing_blocks(tx, project, draft)?;
    for block in &milestone_blocks {
        let header = block.lines().next().unwrap_or_default();
        let id = header
            .trim_start_matches("# Milestone ")
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(':');
        warnings.push(format!(
            "the # Milestone {id} block was written back into the plan file from the store"
        ));
    }

    // 6. writes.
    let mut created: Vec<String> = Vec::new();
    let mut contract_changed: Vec<(String, i64)> = Vec::new();
    let mut mentioned: BTreeSet<String> = BTreeSet::new();
    // The lane's repo, the base and the gate (design §1.1): a task keeps the
    // ones it was created with. The config is read once for the whole apply.
    let config = project_config(tx, project)?;
    for (position, item) in draft.queue.iter().enumerate() {
        let rank = position as i64 + 1;
        match &item.id {
            None => {
                let id = new_ids
                    .get(&position)
                    .cloned()
                    .ok_or_else(|| bad(item.line, "cannot assign an id to this NEW block"))?;
                // The repo, the base and the gate of the task (design §1.1):
                // copied from the lane's config, so a lane that is rebound to
                // another repo later leaves its old tasks where they were.
                let (repo, base, gate) = match &config {
                    Some(config) => {
                        let repo = config.repo_of_lane(item.lane.as_deref().unwrap_or_default());
                        (Some(repo.path), Some(repo.integration), Some(repo.gate))
                    }
                    None => (None, None, None),
                };
                let task = TaskRow {
                    repo,
                    base,
                    gate,
                    rework_estimate_min: None,
                    project: project.to_string(),
                    id: id.clone(),
                    rank,
                    state: TaskState::Ready,
                    priority: priority_str(item.priority).to_string(),
                    lane: item.lane.clone(),
                    milestone: item.milestone.clone(),
                    required: item.required,
                    contract_version: 1,
                    deps: item.deps.clone(),
                    followup_of: item.followup_of.clone(),
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    // The planned time of the task (T-38); a NEW block must
                    // carry it (checked above).
                    estimate_min: item.estimate.map(i64::from),
                    // The GitHub issue the task came from (T-79); the
                    // parser keeps it inside the store's range.
                    issue: item.issue.and_then(|number| i64::try_from(number).ok()),
                    updated: now,
                };
                tx.insert_task(&task)?;
                // The `fresh:` mark (T-481): the task is unrelated to what the
                // lane did before, so its first round starts a new
                // conversation.
                if let Some(why) = &item.fresh {
                    tx.set_task_fresh(project, &id, why)?;
                }
                let contract = ContractRow {
                    project: project.to_string(),
                    task: id.clone(),
                    version: 1,
                    goal: item.goal.clone().unwrap_or_default(),
                    accept: item.accept.clone().unwrap_or_default(),
                    evidence_files: item.evidence_files.clone(),
                    evidence_due_min: item.evidence_due.map(i64::from),
                    body: contract_body(item),
                    created: now,
                    mechanical: item.mechanical.clone(),
                };
                tx.insert_contract(&contract)?;
                created.push(id);
            }
            Some(id) => {
                mentioned.insert(id.clone());
                let mut task = existing
                    .get(id)
                    .cloned()
                    .ok_or_else(|| bad(item.line, format!("task {id} does not exist")))?;
                task.rank = rank;
                task.priority = priority_str(item.priority).to_string();
                let old_lane = task.lane.clone();
                task.lane = item.lane.clone();
                // A task moved to another voyage is recorded in the voyage it
                // leaves, so that voyage's board can name it (T-351).
                if let (Some(old), Some(new)) = (&task.milestone, &item.milestone) {
                    if old != new {
                        record_gear_decision(
                            tx,
                            project,
                            old,
                            applied_version,
                            "move",
                            &format!("{id} moved to {new}"),
                            None,
                            &format!("move|{id}|{old}|{new}"),
                            now,
                        )?;
                    }
                }
                task.milestone = item.milestone.clone();
                task.required = item.required;
                task.deps = item.deps.clone();
                // The follow-up (design §3.3): the captain sets it on an
                // existing task too — the field is live. A block without the
                // field keeps the follow-up the task has.
                if let Some(followup) = &item.followup_of {
                    task.followup_of = Some(followup.clone());
                }
                // The `fresh:` mark (T-481): a draft that names one sets it; a
                // block without `fresh:` keeps the mark the task has.
                if let Some(why) = &item.fresh {
                    tx.set_task_fresh(project, id, why)?;
                }
                // The planned time (T-38): the draft may change it; a draft
                // that says nothing keeps the estimate the task has.
                if let Some(estimate) = item.estimate {
                    task.estimate_min = Some(i64::from(estimate));
                }
                // The GitHub issue (T-79): a draft that names one sets it; a
                // block without `issue:` keeps the issue the task came from.
                if let Some(issue) = item.issue.and_then(|number| i64::try_from(number).ok()) {
                    task.issue = Some(issue);
                }
                match item.state.as_deref() {
                    Some("cancelled") => {
                        // T-93: cancelling a task finishes it. Its open attempt
                        // is abandoned — the seat is free for the next task —
                        // and the lane is told in one line. A cancelled task
                        // that kept a started attempt left the lane looking
                        // busy with a full queue behind it (M3: T-80).
                        let was = task.state;
                        if task.state != TaskState::Cancelled {
                            cancel_task(tx, project, &task, by_seat, by_gen, now)?;
                        }
                        task.state = TaskState::Cancelled;
                        if was != TaskState::Cancelled {
                            cancelled.push(id.clone());
                            // The decision log (design §7.4): a cancelled task
                            // is a cut, and the why is the one the captain
                            // wrote about it.
                            let cut_voyage = task.milestone.as_deref().unwrap_or(&voyage);
                            record_gear_decision(
                                tx,
                                project,
                                cut_voyage,
                                applied_version,
                                "cut",
                                &format!("{id} cancelled"),
                                why_naming(&decisions, id)
                                    .or_else(|| Some("(no why written)".to_string())),
                                id,
                                now,
                            )?;
                        }
                        // T-112: a done task's open merge items close with it —
                        // the failed merge and the push refusal are history,
                        // and the captain's own decision is the answer. The
                        // failed op itself stays as history; no op is created.
                        resolved.extend(resolve_merge_items(
                            tx,
                            project,
                            id,
                            "cancelled by the captain",
                            now,
                        )?);
                    }
                    Some("ready") => task.state = TaskState::Ready,
                    _ => {}
                }
                // A task moved to another lane takes its work with it (T-311):
                // the old lane's attempts close, so its seat is free and it
                // stops working on a task that is not its own any more. The
                // rule is keyed on the attempts, not on the state — a rework
                // attempt is opened while the task is Ready. A task with no
                // attempt in progress only changes its lane.
                if let (Some(new_lane), Some(old_lane)) =
                    (item.lane.as_deref(), old_lane.as_deref())
                {
                    let moving =
                        new_lane != old_lane && !matches!(item.state.as_deref(), Some("cancelled"));
                    if moving {
                        let attempts = tx.attempts_of_task(project, id)?;
                        // A handed-in attempt is under review: its verdict
                        // belongs to the lane that did the work, so the move
                        // waits for it.
                        if let Some(handed_in) = attempts
                            .iter()
                            .find(|attempt| attempt.state == AttemptState::Submitted)
                        {
                            return Err(bad(
                                item.line,
                                format!(
                                    "{id} is handed in (attempt {}) and under review: change its lane after the verdict",
                                    handed_in.n
                                ),
                            ));
                        }
                        // The seats to clear the wait pause of, read before
                        // the attempts are closed.
                        let seats: Vec<String> = attempts
                            .iter()
                            .filter(|attempt| attempt.state.occupies_seat())
                            .map(|attempt| attempt.seat.clone())
                            .collect();
                        let closed = abandon_open_attempts(tx, project, id, now)?;
                        if closed {
                            clear_wait(tx, project, &mut task, &seats, now)?;
                            task.state = TaskState::Ready;
                            let old_seat = format!("{project}:{old_lane}");
                            if tx.get_seat(&old_seat)?.is_some() {
                                tx.insert_message(&NewMessage {
                                    meta: None,
                                    project: project.to_string(),
                                    thread: format!("{id}:moved"),
                                    task: Some(id.clone()),
                                    attempt: None,
                                    sender: by_seat.to_string(),
                                    sender_gen: by_gen,
                                    recipient: old_seat,
                                    kind: MsgKind::Instruction,
                                    urgent: false,
                                    body: format!(
                                        "{id} moved to lane {new_lane}; stop work on it, nothing to hand in"
                                    ),
                                    reply_to: None,
                                    needs_ack: false,
                                    state: MsgState::Open,
                                    created: now,
                                })?;
                            }
                        }
                        let moved = format!("{id} moved {old_lane} → {new_lane}");
                        record_gear_decision(
                            tx,
                            project,
                            &voyage,
                            applied_version,
                            "staff",
                            &moved,
                            None,
                            &moved,
                            now,
                        )?;
                    }
                }
                task.updated = now;

                // A change to any contract field (goal, accept, evidence-files
                // or evidence-due) opens a new version — including clearing a
                // value: the queue item is the contract as the captain wants it.
                let current_contract = tx.get_contract(project, id, task.contract_version)?;
                if let Some(contract) = current_contract {
                    let goal_changed = item
                        .goal
                        .as_ref()
                        .map(|goal| *goal != contract.goal)
                        .unwrap_or(false);
                    let accept_changed = item
                        .accept
                        .as_ref()
                        .map(|accept| *accept != contract.accept)
                        .unwrap_or(false);
                    let files_changed = item.evidence_files != contract.evidence_files;
                    let due_changed = item.evidence_due.map(i64::from) != contract.evidence_due_min;
                    let mechanical_changed = item.mechanical != contract.mechanical;
                    if goal_changed
                        || accept_changed
                        || files_changed
                        || due_changed
                        || mechanical_changed
                    {
                        let version = task.contract_version + 1;
                        let next = ContractRow {
                            project: project.to_string(),
                            task: id.clone(),
                            version,
                            goal: item.goal.clone().unwrap_or(contract.goal),
                            accept: item.accept.clone().unwrap_or(contract.accept),
                            evidence_files: item.evidence_files.clone(),
                            evidence_due_min: item.evidence_due.map(i64::from),
                            body: contract_body(item),
                            created: now,
                            mechanical: item.mechanical.clone(),
                        };
                        tx.insert_contract(&next)?;
                        task.contract_version = version;
                        if task.wait_on.is_some() {
                            let seats: Vec<String> = tx
                                .attempts_of_task(project, id)?
                                .into_iter()
                                .filter(|a| {
                                    matches!(a.state, AttemptState::Started | AttemptState::Stopped)
                                })
                                .map(|a| a.seat)
                                .collect();
                            clear_wait(tx, project, &mut task, &seats, now)?;
                        }
                        contract_changed.push((id.clone(), version));

                        // An attempt that is working on the task hears about the
                        // update at its next idle point (T-8): the marker holds
                        // the newest version, so two bumps before the lane looks
                        // still give one delivery. A handed-in attempt gets
                        // none — its review and verdict follow the version rule.
                        if let Some(mut attempt) = tx
                            .attempts_of_task(project, id)?
                            .into_iter()
                            .rev()
                            .find(|attempt| attempt.state.occupies_seat())
                        {
                            attempt.pending_update = Some(version);
                            attempt.updated = now;
                            tx.update_attempt(&attempt)?;
                        }
                    }
                }
                tx.update_task(&task)?;
            }
        }
    }

    // Unmentioned non-terminal tasks keep their relative order after the listed
    // ones; the terminal ones keep their rank.
    let mut rest: Vec<TaskRow> = existing
        .values()
        .filter(|task| !mentioned.contains(&task.id) && !task.state.is_terminal())
        .cloned()
        .collect();
    rest.sort_by(|a, b| (a.rank, &a.id).cmp(&(b.rank, &b.id)));
    let mut not_mentioned: Vec<String> = Vec::new();
    let mut rank = draft.queue.len() as i64;
    for mut task in rest {
        rank += 1;
        task.rank = rank;
        task.updated = now;
        tx.update_task(&task)?;
        not_mentioned.push(task.id);
    }

    // Staff (T-55): park/hold/retire/release move a lane seat of this project
    // in this transaction; a target already reached is skipped. `remerge` (T-92)
    // is not a seat: it queues a new merge op for a task whose merge failed.
    for item in &draft.staff {
        if matches!(
            item,
            StaffItem::Rebind { .. } | StaffItem::RebindCancel { .. }
        ) {
            continue;
        }
        if let StaffItem::Remerge { task, .. } = item {
            // T-92 F1: a candidate that is already on the integration branch
            // needs no new merge op; the remerge only settles its failed-merge
            // item and says so.
            if let Some(note) = remerge(
                tx,
                project,
                task,
                staff_line(item),
                now,
                already_merged.contains(task),
            )? {
                warnings.push(note);
            }
            // The decision log (design §7.4): a staff line is a decision.
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "staff",
                &staff_text(item),
                None,
                &staff_text(item),
                now,
            )?;
            continue;
        }
        if let StaffItem::Retry { lane, .. } = item {
            retry_lane(tx, project, lane, staff_line(item), now)?;
            staffed.push(format!("retry {lane}"));
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "staff",
                &staff_text(item),
                None,
                &staff_text(item),
                now,
            )?;
            continue;
        }
        if let StaffItem::Release { name, .. } = item {
            release_seat(tx, project, name, staff_line(item), agents, now)?;
            staffed.push(format!("release {name}"));
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "staff",
                &staff_text(item),
                None,
                &staff_text(item),
                now,
            )?;
            continue;
        }
        if let StaffItem::Raise {
            role,
            to,
            until,
            reason,
            line,
        } = item
        {
            // T-467: the user-confirmed raise of a count for a backlog. It
            // lives in the store, not in the config, and the engine returns
            // the count by itself when the end condition holds.
            let row = tx.get_project(project)?.ok_or_else(|| {
                GearError::new(ErrorCode::NoTarget, format!("no project {project}"))
            })?;
            let config = crate::config::parse(&row.config_toml).map_err(|e| {
                bad(
                    *line,
                    format!("the project config cannot be read: {}", e.detail),
                )
            })?;
            let from = config.quota.temps;
            if *to <= from {
                return Err(bad(
                    *line,
                    format!("`raise {role}` must be above the configured count {from} (got {to})"),
                ));
            }
            let key = format!("engine.review_raise.{project}");
            let value = serde_json::json!({
                "from": from,
                "to": to,
                "until": until,
                "reason": reason,
                "at": now,
                "plan_version": applied_version,
            });
            tx.set_meta(&key, &value.to_string())?;
            let message = format!("raise {role} {from}->{to} until={until}: {reason}");
            staffed.push(message);
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "staff",
                &staff_text(item),
                Some(reason.clone()),
                &staff_text(item),
                now,
            )?;
            continue;
        }
        let (lane, verb, target, reason) = match item {
            StaffItem::Park {
                lane,
                until,
                until_task,
                ..
            } => {
                let reason = match (until, until_task) {
                    (Some(until), _) => format!("until={until}"),
                    (_, Some(task)) => format!("until-task={task}"),
                    _ => {
                        return Err(bad(
                            staff_line(item),
                            "`park` needs `until=` or `until-task=`",
                        ))
                    }
                };
                (lane, "park", SeatState::Parked, reason)
            }
            StaffItem::Hold { lane, reason, .. } => {
                (lane, "hold", SeatState::Held, format!("hold: {reason}"))
            }
            StaffItem::Retire { lane, .. } => {
                (lane, "retire", SeatState::Retiring, "retire".to_string())
            }
            StaffItem::Temp { .. } => {
                return Err(bad(
                    staff_line(item),
                    "`temp` staff lines are not in this cut",
                ))
            }
            StaffItem::Remerge { .. } => unreachable!("remerge is applied above"),
            StaffItem::Retry { .. } => unreachable!("retry is applied above"),
            StaffItem::Release { .. } => unreachable!("release is applied above"),
            StaffItem::Rebind { .. } => unreachable!("rebind is applied above"),
            StaffItem::RebindCancel { .. } => unreachable!("rebind is applied above"),
            StaffItem::Raise { .. } => unreachable!("raise is applied above"),
        };
        let seat_id = format!("{project}:{lane}");
        let seat = tx.get_seat(&seat_id)?.ok_or_else(|| {
            bad(
                staff_line(item),
                format!("no seat {seat_id} in this project"),
            )
        })?;
        if seat.role != SeatRole::Lane {
            return Err(bad(
                staff_line(item),
                format!(
                    "seat {seat_id} is a {} seat, staff lines name lanes",
                    seat.role.as_str()
                ),
            ));
        }
        if seat.state == target {
            // Staff is a target, not a command: already there, nothing to do.
            // The captain still made a decision (design §7.4), so it is
            // logged — once. The result says so too (T-303): a line that
            // does nothing must not be silent.
            warnings.push(format!(
                "- {verb} {lane}: the lane is already {}, nothing to do",
                seat.state.as_str()
            ));
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "staff",
                &staff_text(item),
                None,
                &staff_text(item),
                now,
            )?;
            continue;
        }
        require_transition(
            SeatState::can_transition(SeatRole::Lane, seat.state, target),
            "seat",
            seat.state.as_str(),
            target.as_str(),
        )
        .map_err(|_| {
            bad(
                staff_line(item),
                format!(
                    "seat {seat_id} cannot go from {} to {}",
                    seat.state.as_str(),
                    target.as_str()
                ),
            )
        })?;
        tx.set_seat_state(&seat_id, target, Some(&reason), now)?;
        staffed.push(format!("{verb} {lane}"));
        // The decision log (design §7.4): a staff line is a decision.
        record_gear_decision(
            tx,
            project,
            &voyage,
            applied_version,
            "staff",
            &staff_text(item),
            None,
            &staff_text(item),
            now,
        )?;
    }

    let version = current + 1;
    // Settle the verdicts, after the queue writes and before the plan row:
    // an error aborts the whole apply, so nothing of it is kept.
    let mut settled: Vec<String> = Vec::new();
    for line in &draft.verdicts {
        // A pass over a failed review says why (design §7.4): the captain
        // overrules the reviewer, and the reason is recorded.
        let failed_review = tx
            .get_review_verdict(project, &line.task, line.attempt)?
            .is_some_and(|review| !review.passed);
        if line.pass && failed_review && line.why.is_none() {
            return Err(bad(
                line.line,
                "a pass over a failed review needs why=\"…\"",
            ));
        }
        let verdict = crate::tasks::Verdict {
            project: project.to_string(),
            task: line.task.clone(),
            attempt: line.attempt,
            contract_version: line.contract,
            candidate: line.candidate.clone(),
            pass: line.pass,
            evidence: line.evidence.clone(),
            rework: line.rework.clone(),
            // The task whose merge landed this work (design §10.7, row
            // B03-01); the settle decides what it means.
            integrated_by: line.integrated_by.clone(),
            by_seat: by_seat.to_string(),
        };
        crate::tasks::settle(tx, &verdict, now)?;
        // The review item of that attempt is settled with the verdict (T-187):
        // the captain does not have to resolve it by hand.
        crate::tasks::resolve_review_items(
            tx,
            project,
            &line.task,
            line.attempt,
            &format!("settled in plan {applied_version}"),
            now,
        )?;
        // The decision log (design §7.4): a pass over a failed review is an
        // override, a fail is a rework.
        if line.pass {
            if failed_review {
                record_gear_decision(
                    tx,
                    project,
                    &voyage,
                    applied_version,
                    "override",
                    &format!(
                        "{} attempt {} passed over a failed review",
                        line.task, line.attempt
                    ),
                    line.why.clone(),
                    &format!("{}|{}", line.task, line.attempt),
                    now,
                )?;
            }
        } else {
            record_gear_decision(
                tx,
                project,
                &voyage,
                applied_version,
                "rework",
                line.rework.as_deref().unwrap_or_default(),
                None,
                &format!("{}|{}", line.task, line.attempt),
                now,
            )?;
        }
        // The minutes the rework round gets (design §5.1): written after the
        // settle, which writes the task row itself.
        if let Some(minutes) = line.estimate {
            tx.set_task_rework_estimate(project, &line.task, Some(minutes))?;
        }
        settled.push(format!(
            "{} attempt {} {}",
            line.task,
            line.attempt,
            if line.pass { "pass" } else { "fail" }
        ));
    }

    // A park whose time has come ends (T-190) — after the staff lines and
    // the verdicts, so an explicit release in this apply runs against the
    // state the captain saw, and the parks that are left end here.
    end_parked_lanes(tx, project, agents, now)?;

    // A retired lane comes back when a task names it again (T-247): a config
    // re-apply alone never revives a lane, a Ready task on that lane does —
    // and only while the lane is still in the project's config and the lane
    // quota has a free slot (T-304).
    let lanes: Vec<String> = match project_config(tx, project)? {
        Some(config) => config.lanes.into_iter().map(|lane| lane.name).collect(),
        None => Vec::new(),
    };
    if !lanes.is_empty() {
        let tasks = tx.tasks_of_project(project)?;
        let quota = tx
            .get_project(project)?
            .map(|row| row.quota_lanes)
            .unwrap_or_default();
        let seats = tx.seats_of_project(project)?;
        // The running lanes, counted once and advanced after every revival in
        // this apply: a second revival must see the slot the first one took
        // (T-304, review F1). `set_seat_state` writes the store, not `seats`.
        let mut running = seats
            .iter()
            .filter(|seat| seat.role == SeatRole::Lane && seat.state != SeatState::Retired)
            .count();
        for seat in &seats {
            if seat.role != SeatRole::Lane || seat.state != SeatState::Retired {
                continue;
            }
            if !lanes.contains(&seat.name) {
                continue;
            }
            if !tasks.iter().any(|task| {
                task.lane.as_deref() == Some(seat.name.as_str()) && task.state == TaskState::Ready
            }) {
                continue;
            }
            // The quota counts the lane seats that are not Retired: a lane
            // that is already retired keeps its seat and takes no slot, so a
            // full quota leaves it retired (T-304).
            if running as i64 >= quota {
                let warning = format!(
                    "lane {} stays retired: {running} lanes run and the quota allows {quota}; retire another lane or raise [quota] lanes",
                    seat.name
                );
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
                continue;
            }
            tx.set_seat_state(
                &seat.id,
                SeatState::Queued,
                Some("a task names it again"),
                now,
            )?;
            running += 1;
            staffed.push(format!("start {}", seat.name));
        }
    }

    tx.insert_plan(&PlanRow {
        project: project.to_string(),
        version,
        draft: text.to_string(),
        applied: now,
        by_seat: Some(by_seat.to_string()),
        by_gen: Some(by_gen),
    })?;

    Ok(ApplyResult {
        version,
        warnings,
        created,
        contract_changed,
        not_mentioned,
        settled,
        staffed,
        cancelled,
        resolved,
        milestone_blocks,
    })
}

fn bad(line: usize, detail: impl std::fmt::Display) -> GearError {
    GearError::new(ErrorCode::BadFormat, format!("line {line}: {detail}"))
}

/// Whether the plan describes this milestone (T-442): a `# Milestone <id>`
/// block in this file, or a voyage the store keeps a done sentence for (the
/// words of a block an earlier apply recorded, T-333).
fn milestone_described(
    tx: &Tx<'_>,
    project: &str,
    draft: &Draft,
    milestone: &str,
) -> GearResult<bool> {
    if draft
        .milestones
        .iter()
        .any(|(header, _)| crate::draft::milestone_parts(header).0 == milestone)
    {
        return Ok(true);
    }
    Ok(tx
        .get_voyage(project, milestone)?
        .and_then(|voyage| voyage.done)
        .is_some_and(|done| !done.trim().is_empty()))
}

/// The done tasks this draft cancels whose accepted work is **not** on their
/// integration branch (T-112): the only finished tasks that may be cancelled.
///
/// Git runs before the write transaction. A fetch comes first and any doubt —
/// a fetch that fails, a candidate git cannot place — is an error: a
/// cancellation is never accepted unchecked. A done task with no accepted
/// candidate has no landed work, so it may be cancelled too.
/// The verdicts that name the task whose merge landed the work
/// (design §10.7, row B03-01): the named task must be merged, and the
/// candidate must be an ancestor of the integration branch — otherwise the
/// work is not there and the verdict is refused.
///
/// The store is asked first, so the refusal names the task that is not
/// merged. Git runs before the write transaction, like the cancellation
/// check: a fetch comes first, and any doubt — a fetch that fails, a
/// candidate git cannot place — is an error: a verdict is never accepted
/// unchecked.
fn require_integrated_by(store: &mut Store, draft: &Draft) -> GearResult<()> {
    let wanted: Vec<(&String, &String, &String)> = draft
        .verdicts
        .iter()
        .filter(|line| line.pass)
        .filter_map(|line| {
            line.integrated_by
                .as_ref()
                .map(|by| (&line.task, &line.candidate, by))
        })
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let project = draft.project.clone();
    for (task, candidate, by) in wanted {
        // The named task must be merged: its merge is the one that landed
        // this work.
        let merged = store.read(|tx| crate::tasks::require_merged_at(tx, &project, by))?;
        let _ = merged;
        let config = crate::config::latest(store, &project)?;
        let lane = store
            .read(|tx| tx.get_task(&project, task))?
            .and_then(|row| row.lane);
        let repo = match lane.as_deref() {
            Some(lane) => config.repo_of_lane(lane),
            None => config.root_repo(),
        };
        // The candidate must be an ancestor of the integration branch: the
        // work is there because the candidate is (design §10.7, row B03-01,
        // contract v3 of T-165). A commit with the same tree but no ancestry
        // is not the work that landed.
        let reference = crate::base::fetch_integration(
            &repo.path,
            &config.conventions.remote,
            &repo.integration,
        )?;
        if !crate::base::contains(std::path::Path::new(&repo.path), candidate, &reference)? {
            return Err(bad(
                0,
                format!(
                    "{task}: the candidate {candidate} is not in {}: a pass may name the task whose merge landed the work only when that work is in the integration branch",
                    repo.integration
                ),
            ));
        }
    }
    Ok(())
}

fn cancellable_done_tasks(store: &mut Store, draft: &Draft) -> GearResult<BTreeSet<String>> {
    let wanted: Vec<&String> = draft
        .queue
        .iter()
        .filter(|item| item.state.as_deref() == Some("cancelled"))
        .filter_map(|item| item.id.as_ref())
        .collect();
    if wanted.is_empty() {
        return Ok(BTreeSet::new());
    }
    let project = draft.project.clone();
    // Only a done task's merge is in question (the config is not read for a
    // running one, and a test project may have none).
    let rows = store.read(|tx| {
        let mut rows: Vec<(String, Option<String>, Option<String>)> = Vec::new();
        for id in &wanted {
            let Some(task) = tx.get_task(&project, id)? else {
                continue; // the validation names a missing task
            };
            if task.state != TaskState::Done {
                continue;
            }
            let candidate = tx
                .attempts_of_task(&project, id)?
                .into_iter()
                .rev()
                .find(|attempt| attempt.state == AttemptState::Accepted)
                .and_then(|attempt| attempt.candidate);
            rows.push((id.to_string(), task.lane.clone(), candidate));
        }
        Ok(rows)
    })?;
    if !rows.iter().any(|(_, _, candidate)| candidate.is_some()) {
        // No candidate to look for: nothing to answer with git (and a project
        // without a readable config is not consulted for nothing).
        return Ok(BTreeSet::new());
    }
    let config = crate::config::latest(store, &project)?;
    let mut cancellable = BTreeSet::new();
    for (id, lane, candidate) in rows {
        let Some(candidate) = candidate else {
            // A done task with no accepted work has no candidate to look for;
            // it is not the case this opens (the plan refuses it as before).
            continue;
        };
        if !on_integration(&config, lane.as_deref(), &candidate)? {
            cancellable.insert(id);
        }
    }
    Ok(cancellable)
}

/// The remerge lines of this draft whose accepted candidate is already on the
/// integration branch (T-92 F1): a candidate that landed by another way — a
/// hand merge, another task that carried it — has nothing left to merge, and
/// its open `merge-failed:<task>` item would otherwise stay open forever.
///
/// A git failure does not put a task in the set: the remerge then runs as
/// before and the engine's merge confirms the already-merged candidate itself.
fn remerges_already_merged(store: &mut Store, draft: &Draft) -> GearResult<BTreeSet<String>> {
    let wanted: Vec<&String> = draft
        .staff
        .iter()
        .filter_map(|item| match item {
            StaffItem::Remerge { task, .. } => Some(task),
            _ => None,
        })
        .collect();
    if wanted.is_empty() {
        return Ok(BTreeSet::new());
    }
    let project = draft.project.clone();
    let (rows, text) = store.read(|tx| {
        let mut rows: Vec<(String, Option<String>, Option<String>)> = Vec::new();
        for id in &wanted {
            let Some(task) = tx.get_task(&project, id)? else {
                continue; // the remerge names a missing task
            };
            let candidate = tx
                .attempts_of_task(&project, id)?
                .into_iter()
                .rev()
                .find(|attempt| attempt.state == AttemptState::Accepted)
                .and_then(|attempt| attempt.candidate);
            rows.push((id.to_string(), task.lane.clone(), candidate));
        }
        let text = tx
            .get_project(&project)?
            .map(|row| row.config_toml)
            .unwrap_or_default();
        Ok((rows, text))
    })?;
    // A store written before a config was stored has nothing to ask git with,
    // and text that does not parse is not a gate here.
    if text.trim().is_empty() {
        return Ok(BTreeSet::new());
    }
    let Ok(config) = crate::config::parse(&text) else {
        return Ok(BTreeSet::new());
    };
    let mut already = BTreeSet::new();
    for (id, lane, candidate) in rows {
        let Some(candidate) = candidate else {
            continue; // the remerge names the missing attempt
        };
        if matches!(
            on_integration(&config, lane.as_deref(), &candidate),
            Ok(true)
        ) {
            already.insert(id);
        }
    }
    Ok(already)
}

/// Whether `candidate` is on the repo's integration branch at the project's
/// remote (T-112, T-257: `[conventions].remote`). The branch is fetched first:
/// a local tracking ref may be stale, and the answer must be about the remote
/// as it is now. Anything git cannot answer is an error, never a pass.
fn on_integration(
    config: &crate::config::ProjectConfig,
    lane: Option<&str>,
    candidate: &str,
) -> GearResult<bool> {
    let repo = match lane {
        Some(lane) => config.repo_of_lane(lane),
        None => config.root_repo(),
    };
    let remote = &config.conventions.remote;
    let reference = format!("refs/remotes/{remote}/{}", repo.integration);
    let fetched = Command::new("git")
        .args([
            "fetch",
            remote,
            &format!("{}:{}", repo.integration, reference),
        ])
        .current_dir(&repo.path)
        .output()
        .map_err(|e| {
            bad(
                0,
                format!(
                    "cannot fetch {} in {} to tell whether {candidate} is merged: {e}",
                    repo.integration, repo.path
                ),
            )
        })?;
    if !fetched.status.success() {
        return Err(bad(
            0,
            format!(
                "cannot fetch {} in {} to tell whether {candidate} is merged: {}",
                repo.integration,
                repo.path,
                String::from_utf8_lossy(&fetched.stderr).trim()
            ),
        ));
    }
    let placed = Command::new("git")
        .args(["merge-base", "--is-ancestor", candidate, &reference])
        .current_dir(&repo.path)
        .output()
        .map_err(|e| {
            bad(
                0,
                format!(
                    "cannot place {candidate} against {reference} in {}: {e}",
                    repo.path
                ),
            )
        })?;
    match placed.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(bad(
            0,
            format!(
                "cannot place {candidate} against {reference} in {}: {}",
                repo.path,
                String::from_utf8_lossy(&placed.stderr).trim()
            ),
        )),
    }
}

/// Close the open merge items of a task (T-112: a cancelled task; T-92 F1: a
/// remerge of an already-merged candidate): its failed merge and its push
/// refusal are history. `result` is the line the captain reads — "cancelled by
/// the captain" or "the candidate <sha> is already on the integration branch".
/// Returns the message ids that were closed.
fn resolve_merge_items(
    tx: &Tx<'_>,
    project: &str,
    task: &str,
    result: &str,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut resolved = Vec::new();
    for thread in [format!("merge-failed:{task}"), format!("push:{task}")] {
        for message in tx.thread_messages(project, &thread)? {
            if !matches!(
                message.state,
                gear_model::MsgState::Open | gear_model::MsgState::Waiting
            ) {
                continue;
            }
            gear_model::state::require_transition(
                gear_model::MsgState::can_transition(message.state, gear_model::MsgState::Resolved),
                "message",
                message.state.as_str(),
                gear_model::MsgState::Resolved.as_str(),
            )?;
            tx.update_message_state(
                project,
                &message.id,
                gear_model::MsgState::Resolved,
                Some(result),
                None,
                None,
                now,
            )?;
            resolved.push(message.id);
        }
    }
    Ok(resolved)
}

/// Staff and reviews belong to later cuts; a draft that carries them
/// is rejected whole.
fn reject_sections(draft: &Draft) -> GearResult<()> {
    // Staff targets park/hold/retire are applied (T-55); temp workers are not
    // in this cut yet.
    if let Some(item) = draft
        .staff
        .iter()
        .find(|item| matches!(item, StaffItem::Temp { .. }))
    {
        return Err(bad(
            staff_line(item),
            "`temp` staff lines are not in this cut",
        ));
    }
    if let Some(item) = draft.reviews.first() {
        let line = item.line;
        return Err(bad(
            line,
            "the `# Review` section is not supported in this cut",
        ));
    }
    Ok(())
}

/// The draft line of a staff item.
pub fn staff_line(item: &StaffItem) -> usize {
    match item {
        StaffItem::Temp { line, .. }
        | StaffItem::Park { line, .. }
        | StaffItem::Hold { line, .. }
        | StaffItem::Retire { line, .. }
        | StaffItem::Retry { line, .. }
        | StaffItem::Release { line, .. }
        | StaffItem::Remerge { line, .. }
        | StaffItem::Rebind { line, .. }
        | StaffItem::RebindCancel { line, .. }
        | StaffItem::Raise { line, .. } => *line,
    }
}

/// A durable rebind request; the engine calls `reconcile_rebinds` each tick.
#[derive(serde::Serialize, serde::Deserialize)]
struct RebindRequest {
    repo: String,
    owns: Vec<String>,
}

pub fn rebind_pending_key(seat: &str) -> String {
    format!("lane-rebind:{seat}")
}

/// The lane's binding before a rebind (T-305): the repo's name ("" for the
/// project root) and the paths it owns.
fn lane_binding(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    line: usize,
) -> GearResult<(String, Vec<String>)> {
    let row = tx
        .get_project(project)?
        .ok_or_else(|| bad(line, "project not found"))?;
    let config = crate::config::parse(&row.config_toml)?;
    let owns = config
        .lanes
        .iter()
        .find(|l| l.name == lane)
        .map(|l| l.owns.clone())
        .unwrap_or_default();
    Ok((config.repo_of_lane(lane).name, owns))
}

/// The preview of a rebind, which is the apply line itself (T-305): what
/// changes, old and new.
fn rebind_preview(
    old_repo: &str,
    new_repo: &str,
    old_owns: &[String],
    new_owns: &[String],
) -> String {
    let old = if old_repo.is_empty() {
        "root"
    } else {
        old_repo
    };
    format!(
        "repo {old} → {new_repo}, owns [{}] → [{}]",
        old_owns.join(", "),
        new_owns.join(", ")
    )
}

/// `- rebind <lane> cancel` (T-305): the way back from a recorded request.
fn cancel_rebind(tx: &Tx<'_>, project: &str, lane: &str, line: usize) -> GearResult<String> {
    let seat_id = format!("{project}:{lane}");
    let seat = tx
        .get_seat(&seat_id)?
        .ok_or_else(|| bad(line, format!("no seat {seat_id}")))?;
    if seat.role != SeatRole::Lane || seat.state == SeatState::Retired {
        return Err(bad(line, "rebind needs a non-retired lane"));
    }
    if tx
        .get_meta(&format!("lane-rebind-applied:{seat_id}"))?
        .is_some_and(|v| !v.is_empty())
    {
        return Err(bad(
            line,
            format!("lane {lane} is still moving to its new worktree"),
        ));
    }
    let key = rebind_pending_key(&seat_id);
    if tx.get_meta(&key)?.is_some_and(|v| !v.is_empty()) {
        tx.set_meta(&key, "")?;
        Ok(format!("rebind {lane}: the pending rebind is withdrawn"))
    } else {
        Ok(format!("rebind {lane}: nothing pending to withdraw"))
    }
}

fn rebind_config(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    request: &RebindRequest,
    line: usize,
) -> GearResult<String> {
    let row = tx
        .get_project(project)?
        .ok_or_else(|| bad(line, "project not found"))?;
    let current = crate::config::parse(&row.config_toml)?;
    if !current.repos.iter().any(|r| r.name == request.repo) {
        return Err(bad(line, format!("unknown repo {:?}", request.repo)));
    }
    let mut value: toml::Value = row
        .config_toml
        .parse()
        .map_err(|e| bad(line, format!("invalid config: {e}")))?;
    let lanes = value
        .get_mut("lanes")
        .and_then(toml::Value::as_array_mut)
        .ok_or_else(|| bad(line, "project has no lanes"))?;
    let entry = lanes
        .iter_mut()
        .find(|l| l.get("name").and_then(toml::Value::as_str) == Some(lane))
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| bad(line, format!("lane {lane:?} is not configured")))?;
    entry.insert("repo".into(), toml::Value::String(request.repo.clone()));
    entry.insert(
        "owns".into(),
        toml::Value::Array(
            request
                .owns
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    let text =
        toml::to_string(&value).map_err(|e| bad(line, format!("cannot serialize config: {e}")))?;
    let new = crate::config::parse(&text)?;
    let old_issues = crate::config::validate(&current);
    if let Some(issue) = crate::config::validate(&new)
        .into_iter()
        .find(|issue| !old_issues.contains(issue))
    {
        return Err(bad(line, format!("{}: {}", issue.field, issue.problem)));
    }
    Ok(text)
}

fn request_rebind(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    repo: &str,
    owns: &[String],
    line: usize,
) -> GearResult<Vec<String>> {
    let seat_id = format!("{project}:{lane}");
    let seat = tx
        .get_seat(&seat_id)?
        .ok_or_else(|| bad(line, format!("no seat {seat_id}")))?;
    if seat.role != SeatRole::Lane || seat.state == SeatState::Retired {
        return Err(bad(line, "rebind needs a non-retired lane"));
    }
    if tx
        .get_meta(&format!("lane-rebind-applied:{seat_id}"))?
        .is_some_and(|v| !v.is_empty())
    {
        return Err(bad(
            line,
            format!("lane {lane} is still moving to its new worktree"),
        ));
    }
    let request = RebindRequest {
        repo: repo.into(),
        owns: owns.to_vec(),
    };
    rebind_config(tx, project, lane, &request, line)?; // validate before recording anything
                                                       // Include other lanes' pending targets as reservations. A conflicting
                                                       // pair must be refused in this transaction, not discovered repeatedly by
                                                       // the engine after both attempts finish.
    let row = tx
        .get_project(project)?
        .ok_or_else(|| bad(line, "project not found"))?;
    let mut projected = crate::config::parse(&row.config_toml)?;
    let old_issues = crate::config::validate(&projected);
    for other in tx
        .seats_of_project(project)?
        .into_iter()
        .filter(|s| s.role == SeatRole::Lane && s.id != seat_id)
    {
        let Some(raw) = tx
            .get_meta(&rebind_pending_key(&other.id))?
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        let pending: RebindRequest = serde_json::from_str(&raw)
            .map_err(|e| bad(line, format!("invalid pending rebind: {e}")))?;
        let target = projected
            .lanes
            .iter_mut()
            .find(|l| l.name == other.name)
            .ok_or_else(|| {
                bad(
                    line,
                    format!("pending lane {:?} is not configured", other.name),
                )
            })?;
        target.repo = Some(pending.repo);
        target.owns = pending.owns;
    }
    let target = projected
        .lanes
        .iter_mut()
        .find(|l| l.name == lane)
        .ok_or_else(|| bad(line, format!("lane {lane:?} is not configured")))?;
    target.repo = Some(request.repo.clone());
    target.owns = request.owns.clone();
    if let Some(issue) = crate::config::validate(&projected)
        .into_iter()
        .find(|issue| !old_issues.contains(issue))
    {
        return Err(bad(line, format!("{}: {}", issue.field, issue.problem)));
    }
    let key = rebind_pending_key(&seat_id);
    let data = serde_json::to_string(&request).map_err(|e| bad(line, e))?;
    if tx
        .get_meta(&key)?
        .is_some_and(|old| !old.is_empty() && old != data)
    {
        return Err(bad(
            line,
            format!("lane {lane} already has a pending rebind"),
        ));
    }
    tx.set_meta(&key, &data)?;
    let mut waiting: Vec<_> = tx
        .open_attempts_of_seat(&seat_id)?
        .into_iter()
        .map(|a| a.task)
        .collect();
    waiting.sort();
    waiting.dedup();
    if waiting.is_empty() {
        apply_rebind(tx, project, lane, &seat_id, &request)?;
    }
    Ok(waiting)
}

fn apply_rebind(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    seat: &str,
    request: &RebindRequest,
) -> GearResult<()> {
    let mut row = tx
        .get_project(project)?
        .ok_or_else(|| bad(0, "project not found"))?;
    let old_config = crate::config::parse(&row.config_toml)?;
    let text = rebind_config(tx, project, lane, request, 0)?;
    // Legacy tasks created before repo snapshots keep the old target as well.
    let old = old_config.repo_of_lane(lane);
    for mut task in tx
        .tasks_of_project(project)?
        .into_iter()
        .filter(|t| t.lane.as_deref() == Some(lane) && t.repo.is_none())
    {
        task.repo = Some(old.path.clone());
        if task.base.is_none() {
            task.base = Some(old.integration.clone());
        }
        if task.gate.is_none() {
            task.gate = Some(old.gate.clone());
        }
        tx.update_task(&task)?;
    }
    row.config_toml = text;
    row.config_version += 1;
    tx.update_project(&row)?;
    // The engine-side rebind can use this durable marker to move the worktree.
    if old.name != request.repo {
        let mut marker = serde_json::to_value(request).map_err(|e| bad(0, e))?;
        marker["old_repo"] = serde_json::Value::String(old.path.clone());
        tx.set_meta(&format!("lane-rebind-applied:{seat}"), &marker.to_string())?;
    }
    tx.set_meta(&rebind_pending_key(seat), "")?;
    Ok(())
}

/// Finish recorded requests after their last non-terminal attempt settles.
pub fn reconcile_rebinds(store: &mut Store, project: &str) -> GearResult<Vec<String>> {
    store.write(|tx| {
        let mut changed = Vec::new();
        for seat in tx
            .seats_of_project(project)?
            .into_iter()
            .filter(|s| s.role == SeatRole::Lane)
        {
            let Some(raw) = tx
                .get_meta(&rebind_pending_key(&seat.id))?
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            if !tx.open_attempts_of_seat(&seat.id)?.is_empty() {
                continue;
            }
            let request: RebindRequest = serde_json::from_str(&raw)
                .map_err(|e| bad(0, format!("invalid pending rebind: {e}")))?;
            apply_rebind(tx, project, &seat.name, &seat.id, &request)?;
            changed.push(seat.name);
        }
        Ok(changed)
    })
}

/// `- retry <lane>` (T-82): a lane the engine parked in NeedsCheck, or lost
/// (Unknown), goes back to Queued with the reason "retried by the captain", so
/// the engine onboards it again — the captain's way back without an edit in
/// the store (trial G02, G04). Any other state is a transition the seat cannot
/// make: `BadTransition`.
fn retry_lane(tx: &Tx<'_>, project: &str, lane: &str, line: usize, now: Ts) -> GearResult<()> {
    let seat_id = format!("{project}:{lane}");
    let seat = tx
        .get_seat(&seat_id)?
        .ok_or_else(|| bad(line, format!("no seat {seat_id} in this project")))?;
    if seat.role != SeatRole::Lane {
        return Err(bad(
            line,
            format!(
                "seat {seat_id} is a {} seat, staff lines name lanes",
                seat.role.as_str()
            ),
        ));
    }
    if seat.state == SeatState::Queued {
        return Ok(()); // already there: a target, not a command
    }
    require_transition(
        SeatState::can_transition(SeatRole::Lane, seat.state, SeatState::Queued),
        "seat",
        seat.state.as_str(),
        SeatState::Queued.as_str(),
    )
    .map_err(|_| {
        GearError::new(
            ErrorCode::BadTransition,
            format!(
                "seat {seat_id} cannot go from {} to {}: only a lane in NeedsCheck or Unknown is retried",
                seat.state.as_str(),
                SeatState::Queued.as_str()
            ),
        )
    })?;
    let rebind_key = format!("engine.rebind.{seat_id}");
    if let Some(mut phase) = tx
        .get_meta(&rebind_key)?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    {
        if phase["phase"] == "exiting" {
            phase["phase"] = serde_json::Value::String("retry".into());
            tx.set_meta(&rebind_key, &phase.to_string())?;
        }
    }
    tx.set_seat_state(
        &seat_id,
        SeatState::Queued,
        Some("retried by the captain"),
        now,
    )?;
    Ok(())
}

/// `- release <lane>` (T-133) and `- release <reviewer>` (T-140): the captain
/// takes a seat out of the way.
///
/// A **lane**: a held lane goes back to the state the engine continues or
/// onboards from. A held lane whose agent is still in its pane continues where
/// it was — `Active`; one whose agent is gone goes back to `Queued`, so the
/// engine onboards it again (`- retry` refuses Held, and editing the store was
/// the captain's only way out, M3.1 machine recovery).
///
/// A **reviewer** (design §3.6): a reviewer seat in Standby — its agent alive,
/// its pane open, no job — goes to `Releasing`, and the engine closes the pane
/// as for any release. A reviewer with a job is refused while its agent is
/// there: the captain does not pull a live reviewer out of the review it is
/// doing. When its agent is gone, the seat is let go (see
/// [`release_reviewer`]).
///
/// Whether a lane's agent is still there is the session's fact, not the
/// store's (review F1): the store keeps the pane GEAR started an agent in, and
/// an agent that exits leaves that binding Active. So the target is `Active`
/// only when the caller brought the session's agents in and one of the
/// binding's kind sits in the binding's pane; a binding with no such agent,
/// and no binding at all, go back to `Queued`. An unread observation is not
/// "gone": the release is refused instead of guessed.
fn release_seat(
    tx: &Tx<'_>,
    project: &str,
    name: &str,
    line: usize,
    agents: Option<&[gear_herdr::AgentInfo]>,
    now: Ts,
) -> GearResult<()> {
    let seat_id = format!("{project}:{name}");
    let seat = tx
        .get_seat(&seat_id)?
        .ok_or_else(|| bad(line, format!("no seat {seat_id} in this project")))?;
    match seat.role {
        SeatRole::Lane => release_lane(tx, &seat, name, line, agents, now),
        SeatRole::Temp => release_reviewer(tx, &seat, name, agents, now),
        role => Err(bad(
            line,
            format!("seat {seat_id} is a {role} seat, staff lines name lanes and reviewers"),
        )),
    }
}

/// `- release <lane>` (T-133): a held lane is released: any other state is
/// a transition this line cannot mean, and it is refused with the reason.
///
/// A **parked** lane is released the same way (T-190): the captain's `- park
/// <lane> until=…` has no other way out, and a park whose time has come ends
/// by itself (see [`end_parked_lanes`]).
fn release_lane(
    tx: &Tx<'_>,
    seat: &gear_store::SeatRow,
    name: &str,
    line: usize,
    agents: Option<&[gear_herdr::AgentInfo]>,
    now: Ts,
) -> GearResult<()> {
    let seat_id = seat.id.clone();
    if !matches!(seat.state, SeatState::Held | SeatState::Parked) {
        return Err(GearError::new(
            ErrorCode::BadTransition,
            format!(
                "seat {seat_id} is {}, not held: only a lane in Held is released (`- hold {name} reason=…` puts it there); a Parked lane is released the same way",
                seat.state.as_str()
            ),
        ));
    }
    // The seat's binding is GEAR's record of the pane it started an agent in;
    // the session's agents say whether one is in that pane now.
    let binding = tx
        .get_binding(&seat_id, seat.generation)?
        .filter(|binding| binding.status == BindingStatus::Active);
    let running = match &binding {
        // No live binding: the agent was ended (a restart, a release) or never
        // bound. Either way there is none to continue with.
        None => false,
        Some(binding) => {
            let Some(agents) = agents else {
                return Err(bad(
                    line,
                    format!(
                        "cannot tell whether seat {seat_id}'s agent is still running: the session's agents were not read, so the release is refused and the lane stays held"
                    ),
                ));
            };
            agents.iter().any(|agent| {
                agent.pane_id == binding.pane_id
                    && agent.agent.as_deref() == Some(binding.kind.as_str())
            })
        }
    };
    let (target, reason) = if running {
        (
            SeatState::Active,
            "released by the captain: its agent continues",
        )
    } else {
        (
            SeatState::Queued,
            "released by the captain: its agent is gone",
        )
    };
    // A parked lane whose agent is gone is let go first (Parked → Held →
    // Queued): the seat states allow no direct Parked → Queued, the same way
    // a park that ended lets the lane go (T-190).
    let from = if seat.state == SeatState::Parked && target == SeatState::Queued {
        require_transition(
            SeatState::can_transition(SeatRole::Lane, SeatState::Parked, SeatState::Held),
            "seat",
            SeatState::Parked.as_str(),
            SeatState::Held.as_str(),
        )?;
        tx.set_seat_state(&seat_id, SeatState::Held, Some(reason), now)?;
        SeatState::Held
    } else {
        seat.state
    };
    require_transition(
        SeatState::can_transition(SeatRole::Lane, from, target),
        "seat",
        from.as_str(),
        target.as_str(),
    )?;
    tx.set_seat_state(&seat_id, target, Some(reason), now)?;
    Ok(())
}

/// End the parks whose time has come (T-190).
///
/// A lane the plan parked (`- park <lane> until=…` or `until-task=T-n`) goes
/// back to work when the park ends: **Active** when its agent is still in the
/// pane, **Queued** when the agent is gone (the engine onboards it again, as
/// for a released lane).
///
/// A lane parked on **quota** is left to the quota path: the engine's
/// `resume:<seat>` meta says so, and the quota park ends when the provider's
/// window starts again.
///
/// The plan's apply, `gear plan check` and the engine's tick call it: a park
/// that has passed must not keep a lane standing still.
pub fn end_parked_lanes(
    tx: &Tx<'_>,
    project: &str,
    agents: Option<&[gear_herdr::AgentInfo]>,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut ended = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane || seat.state != SeatState::Parked {
            continue;
        }
        // A quota park is the quota path's business: it writes the resume
        // deadline under its own key (`engine.resume.<seat>`), and a park
        // without that deadline there is no quota park.
        if tx
            .get_meta(&format!("engine.resume.{}", seat.id))?
            .is_some_and(|deadline| !deadline.is_empty())
        {
            continue;
        }
        let Some(reason) = seat.state_reason.as_deref() else {
            continue;
        };
        let over = if let Some(until) = reason.strip_prefix("until=") {
            // The park ends at the time the captain wrote.
            match parse_rfc3339(until) {
                Some(at) => at <= now,
                None => false,
            }
        } else if let Some(task) = reason.strip_prefix("until-task=") {
            // The park ends when the task it waits for is done.
            tx.get_task(project, task)?
                .is_some_and(|task| task.state == TaskState::Done)
        } else {
            false
        };
        if !over {
            continue;
        }
        // The seat's binding is GEAR's record of the pane it started an agent
        // in; the session's agents say whether one is in that pane now.
        let binding = tx
            .get_binding(&seat.id, seat.generation)?
            .filter(|binding| binding.status == BindingStatus::Active);
        let running = match &binding {
            // No live binding: the agent was ended or never bound.
            None => false,
            Some(binding) => {
                let Some(agents) = agents else {
                    // Without the session's agents nobody may say whether the
                    // agent is there: the park stays.
                    continue;
                };
                agents.iter().any(|agent| {
                    agent.pane_id == binding.pane_id
                        && agent.agent.as_deref() == Some(binding.kind.as_str())
                })
            }
        };
        if running {
            require_transition(
                SeatState::can_transition(SeatRole::Lane, seat.state, SeatState::Active),
                "seat",
                seat.state.as_str(),
                SeatState::Active.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Active, Some("park ended"), now)?;
        } else {
            // The agent is gone: the lane is let go and the engine onboards it
            // again. Parked → Held → Queued are the transitions the seat states
            // allow.
            require_transition(
                SeatState::can_transition(SeatRole::Lane, seat.state, SeatState::Held),
                "seat",
                seat.state.as_str(),
                SeatState::Held.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Held, Some("park ended"), now)?;
            require_transition(
                SeatState::can_transition(SeatRole::Lane, SeatState::Held, SeatState::Queued),
                "seat",
                SeatState::Held.as_str(),
                SeatState::Queued.as_str(),
            )?;
            tx.set_seat_state(&seat.id, SeatState::Queued, Some("park ended"), now)?;
        }
        ended.push(format!("{}: the park ended", seat.name));
    }
    Ok(ended)
}

/// `- release <reviewer>` (T-140, design §3.6): a reviewer seat in `Standby` —
/// its agent alive, its pane open, no job — is released: the seat goes to
/// `Releasing` and the engine closes the pane as for any release.
///
/// A reviewer **in a job** is refused while its agent is there: the captain
/// does not pull a live reviewer out of the review it is doing. The refusal
/// names the task it is reviewing.
///
/// A reviewer in a job whose **agent is gone** is let go (T-303): the review
/// cannot finish, and the captain's line is the way out. The seat's task is
/// cleared and the seat goes to `Releasing`; the task stays `Verifying` with
/// its attempt handed in, and the engine's normal `start_reviews` pass starts
/// the review again. Whether the agent is gone is the session's fact, as for a
/// lane (review F1): without the session's agents the release is refused, not
/// guessed.
///
/// A reviewer in any other state (queued, starting, already released) is
/// refused with the reason: only a reviewer in Standby is idle enough to let
/// go of.
fn release_reviewer(
    tx: &Tx<'_>,
    seat: &gear_store::SeatRow,
    name: &str,
    agents: Option<&[gear_herdr::AgentInfo]>,
    now: Ts,
) -> GearResult<()> {
    // A reviewer with a job binding is reviewing it (§3.6: the binding ends
    // when the seat goes to Standby).
    if let Some(task) = seat.task_id.as_deref() {
        // The seat's binding is GEAR's record of the pane it started an agent
        // in; the session's agents say whether one is in that pane now.
        let binding = tx
            .get_binding(&seat.id, seat.generation)?
            .filter(|binding| binding.status == BindingStatus::Active);
        match (binding, agents) {
            (Some(binding), Some(agents)) => {
                let there = agents.iter().any(|agent| {
                    agent.pane_id == binding.pane_id
                        && agent.agent.as_deref() == Some(binding.kind.as_str())
                });
                if there {
                    return Err(GearError::new(
                        ErrorCode::BadTransition,
                        format!("{name} is reviewing {task}"),
                    ));
                }
                // The agent is gone mid-review: the seat is let go, the task
                // stays Verifying and the engine starts the review again.
                tx.clear_seat_task(&seat.id)?;
                tx.set_seat_state(
                    &seat.id,
                    SeatState::Releasing,
                    Some("released by the captain: its agent is gone mid-review"),
                    now,
                )?;
                return Ok(());
            }
            (Some(_), None) => {
                return Err(GearError::new(
                    ErrorCode::BadTransition,
                    format!(
                        "{name} is reviewing {task} (the session's agents were not read, so GEAR cannot tell whether its agent is gone)"
                    ),
                ));
            }
            (None, _) => {
                return Err(GearError::new(
                    ErrorCode::BadTransition,
                    format!("{name} is reviewing {task}"),
                ));
            }
        }
    }
    if seat.state != SeatState::Standby {
        return Err(GearError::new(
            ErrorCode::BadTransition,
            format!(
                "seat {} is {}, not standby: only a reviewer in Standby is released",
                seat.id,
                seat.state.as_str()
            ),
        ));
    }
    // Standby → Releasing: the engine closes the pane and the seat goes to
    // Released. The model's transition table (gear-model, deck's file) knows no
    // reviewer in Standby yet (§3.6 is new), so the transition is stated here.
    tx.set_seat_state(
        &seat.id,
        SeatState::Releasing,
        Some("released by the captain: it was idle"),
        now,
    )?;
    Ok(())
}

/// The project's config, read from the store: `None` when the project has no
/// config yet (nothing to copy, and a reader that finds empty columns falls
/// back to the lane's current config).
fn project_config(tx: &Tx<'_>, project: &str) -> GearResult<Option<crate::config::ProjectConfig>> {
    let Some(row) = tx.get_project(project)? else {
        return Ok(None);
    };
    if row.config_toml.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(crate::config::parse(&row.config_toml)?))
}

/// `- remerge T-<n>` (T-92): the captain retries a merge that failed on
/// something that can clear (a flaky test, a broken gate run). A new merge op
/// is queued for the task's accepted attempt and candidate.
///
/// Only a task whose **last merge op failed** can be retried: a merge that is
/// still in flight is not queued twice, and a task that is merged has nothing
/// to do. Anything else is refused with the reason, like every plan line.
///
/// `already` is the answer of `remerges_already_merged` (T-92 F1): when the
/// accepted candidate is on the integration branch already, no op is queued —
/// the task's open `merge-failed:<task>` and `push:<task>` items are resolved
/// and the line the captain reads is returned, so the apply warns.
fn remerge(
    tx: &gear_store::Tx<'_>,
    project: &str,
    task_id: &str,
    line: usize,
    now: Ts,
    already: bool,
) -> GearResult<Option<String>> {
    let task = tx
        .get_task(project, task_id)?
        .ok_or_else(|| bad(line, format!("no task {task_id} in this project")))?;
    let latest = tx
        .attempts_of_task(project, task_id)?
        .into_iter()
        .last()
        .ok_or_else(|| bad(line, format!("{task_id} has no attempt to merge")))?;
    if latest.state != gear_model::AttemptState::Accepted {
        return Err(bad(
            line,
            format!(
                "{task_id} attempt {} is {}, not accepted: only an accepted attempt is merged",
                latest.n,
                latest.state.as_str()
            ),
        ));
    }
    let candidate = latest.candidate.clone().ok_or_else(|| {
        bad(
            line,
            format!("{task_id} attempt {} has no candidate", latest.n),
        )
    })?;
    // The task's merge ops, oldest first (the store has no query by task and
    // kind; the states are few).
    let mut merges: Vec<gear_store::OpRow> = Vec::new();
    for state in [
        gear_model::OpState::Intended,
        gear_model::OpState::Executing,
        gear_model::OpState::Submitted,
        gear_model::OpState::Unknown,
        gear_model::OpState::Confirmed,
        gear_model::OpState::Failed,
        gear_model::OpState::Void,
    ] {
        for op in tx.ops_in_state(project, state)? {
            if op.kind == gear_model::OpKind::Merge && op.task.as_deref() == Some(task_id) {
                merges.push(op);
            }
        }
    }
    // The order of ops: the time they were created, then their number — not
    // their id as a string, where O-999 sorts after O-1000 (T-268).
    let op_number = |id: &str| -> u64 {
        id.strip_prefix("O-")
            .and_then(|rest| rest.parse::<u64>().ok())
            .unwrap_or(0)
    };
    merges.sort_by(|a, b| (a.created, op_number(&a.id)).cmp(&(b.created, op_number(&b.id))));
    let Some(last) = merges.into_iter().last() else {
        return Err(bad(
            line,
            format!(
                "{task_id} has no merge to retry: the engine queues one when its review passes"
            ),
        ));
    };
    match last.state {
        gear_model::OpState::Failed => {}
        gear_model::OpState::Confirmed => {
            return Err(bad(
                line,
                format!("{task_id} is already merged ({})", last.id),
            ))
        }
        state => {
            return Err(bad(
                line,
                format!(
                    "a merge of {task_id} is already {} ({}), not failed: nothing to retry",
                    state.as_str(),
                    last.id
                ),
            ))
        }
    }
    // The same payload the engine writes when a review passes: the lane's
    // branch (a hint for closing it, named by the project's template — T-252),
    // the accepted candidate and the review branch.
    if already {
        // T-92 F1: the work landed by another way. The merge is not queued
        // again; the failed-merge item is answered with the candidate's sha.
        resolve_merge_items(
            tx,
            project,
            task_id,
            &format!("the candidate {candidate} is already on the integration branch"),
            now,
        )?;
        return Ok(Some(format!(
            "remerge {task_id}: {candidate} is already on the integration branch: no merge queued, its failed-merge item resolved"
        )));
    }
    let lane = task.lane.clone().unwrap_or_default();
    let conventions = project_config(tx, project)?
        .map(|config| config.conventions)
        .unwrap_or_default();
    let branch = if lane.is_empty() {
        String::new()
    } else {
        crate::base::task_branch(&conventions, project, &lane, task_id)
    };
    // T-405: a mechanically settled attempt has no reviewer, so it has no
    // review branch. Its merge op says so, and the merge does not require
    // one.
    let mechanical = settled_mechanically(tx, project, task_id, latest.n)?;
    let mut payload = serde_json::json!({
        "branch": branch,
        "candidate": candidate,
        "review": gear_model::branch::expand(
            &conventions.review_branch,
            project,
            &lane,
            task_id
        ),
        "remerge": true,
    });
    if mechanical {
        payload["mechanical"] = serde_json::json!(true);
    }
    tx.insert_op(&gear_store::NewOp {
        project: project.to_string(),
        seat: if lane.is_empty() {
            format!("{project}:captain")
        } else {
            format!("{project}:{lane}")
        },
        generation: 1,
        kind: gear_model::OpKind::Merge,
        task: Some(task_id.to_string()),
        attempt: Some(latest.n),
        ref_msg: None,
        plan_version: None,
        payload,
        created: now,
    })?;
    Ok(None)
}

/// T-405: whether the task's accepted attempt was settled by the engine as
/// mechanical (T-273) — the pass verdict whose note starts `mechanical:`.
/// Such a task never had a reviewer, so its merge has no review branch to
/// merge and must not require one.
fn settled_mechanically(
    tx: &gear_store::Tx<'_>,
    project: &str,
    task: &str,
    attempt: i64,
) -> GearResult<bool> {
    Ok(tx
        .verdicts_of_task(project, task)?
        .into_iter()
        .any(|verdict| {
            verdict.passed
                && verdict.attempt == attempt
                && verdict.by_seat == "engine"
                && verdict
                    .evidence
                    .as_deref()
                    .is_some_and(|note| note.starts_with("mechanical:"))
        }))
}

fn priority_str(priority: Priority) -> &'static str {
    match priority {
        Priority::Now => "now",
        Priority::Must => "must",
        Priority::Should => "should",
        Priority::Could => "could",
        Priority::Wont => "wont",
    }
}

/// The stored contract body: the fields the draft wrote, one per line.
fn contract_body(item: &QueueItem) -> String {
    let mut body = String::new();
    if let Some(goal) = &item.goal {
        body.push_str(&format!("goal: {goal}\n"));
    }
    if let Some(accept) = &item.accept {
        body.push_str(&format!("accept: {accept}\n"));
    }
    if !item.evidence_files.is_empty() {
        body.push_str(&format!(
            "evidence-files: {}\n",
            item.evidence_files.join(", ")
        ));
    }
    if let Some(minutes) = item.evidence_due {
        body.push_str(&format!("evidence-due: {minutes}\n"));
    }
    if let Some(reason) = &item.mechanical {
        body.push_str(&format!("mechanical: {reason}\n"));
    }
    body
}

/// `gear plan show T-n`: one task's block as it would stand in the plan file
/// (design §8.3), so the captain edits a block without reading the whole
/// file.
///
/// The block is the one the draft parser reads back: the header
/// (`## T-<n> [priority] lane=… milestone=…`), then the fields the task
/// carries — `goal`, `accept`, `evidence-files`, `evidence-due`, `mechanical`,
/// `estimate`, `deps`, `followup-of` and `issue`. A block that parses back is
/// the same task, so the captain can copy it into the plan file as it is.
///
/// Read-only: nothing is written.
pub fn show(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<String> {
    let row = tx.get_task(project, task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {task} in project {project}"),
        )
    })?;
    let contract = tx.get_contract(project, task, row.contract_version)?;
    let mut text = String::new();
    // The header: the id, the priority, the lane, the milestone and the
    // `required` mark, exactly as the plan file writes them.
    text.push_str(&format!("## {} [{}]", row.id, row.priority));
    if let Some(lane) = row.lane.as_deref() {
        text.push_str(&format!(" lane={lane}"));
    }
    if let Some(milestone) = row.milestone.as_deref() {
        text.push_str(&format!(" milestone={milestone}"));
        if let Some(station) = tx.task_station(project, task)? {
            text.push_str(&format!(" station={station}"));
        }
    }
    if row.required {
        text.push_str(" required");
    }
    text.push('\n');
    // The fields the task carries: the contract's goal and acceptance, its
    // evidence, its estimate, its dependencies and what it follows up.
    if let Some(contract) = contract.as_ref() {
        text.push_str(&format!("goal: {}\n", contract.goal));
        text.push_str(&format!("accept: {}\n", contract.accept));
        if !contract.evidence_files.is_empty() {
            text.push_str(&format!(
                "evidence-files: {}\n",
                contract.evidence_files.join(", ")
            ));
        }
        if let Some(minutes) = contract.evidence_due_min {
            text.push_str(&format!("evidence-due: {minutes}\n"));
        }
        if let Some(reason) = &contract.mechanical {
            text.push_str(&format!("mechanical: {reason}\n"));
        }
    }
    if let Some(minutes) = row.estimate_min {
        text.push_str(&format!("estimate: {minutes}\n"));
    }
    text.push_str(&format!(
        "deps: {}\n",
        if row.deps.is_empty() {
            "-".to_string()
        } else {
            row.deps.join(", ")
        }
    ));
    if let Some(followup) = row.followup_of.as_deref() {
        text.push_str(&format!("followup-of: {followup}\n"));
    }
    // The `fresh:` mark (T-481): the captain's reason that the task is
    // unrelated to what the lane did before.
    if let Some(why) = tx.task_fresh(project, task)? {
        text.push_str(&format!("fresh: {why}\n"));
    }
    if let Some(issue) = row.issue {
        text.push_str(&format!("issue: #{issue}\n"));
    }
    Ok(text.trim_end().to_string())
}

/// The first task on a dependency cycle, if there is one.
fn find_cycle(graph: &BTreeMap<String, Vec<String>>) -> Option<String> {
    let mut colour: BTreeMap<String, u8> = BTreeMap::new();
    for start in graph.keys() {
        if colour.get(start).copied().unwrap_or(0) == 0 {
            if let Some(cycle) = walk(start, graph, &mut colour) {
                return Some(cycle);
            }
        }
    }
    None
}

fn walk(
    node: &str,
    graph: &BTreeMap<String, Vec<String>>,
    colour: &mut BTreeMap<String, u8>,
) -> Option<String> {
    colour.insert(node.to_string(), 1);
    if let Some(deps) = graph.get(node) {
        for dep in deps {
            match colour.get(dep.as_str()).copied().unwrap_or(0) {
                0 => {
                    if let Some(cycle) = walk(dep, graph, colour) {
                        return Some(cycle);
                    }
                }
                1 => return Some(dep.clone()),
                _ => {}
            }
        }
    }
    colour.insert(node.to_string(), 2);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft::parse;
    use gear_model::GearCtx;
    use gear_store::testing;

    const NOW: i64 = 1_800_000_000;
    const CAP_PANE: &str = "w1:p1";

    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                NOW,
            )?;
            Ok(())
        })
        .unwrap();
        s
    }

    fn cap() -> (Env, Fingerprint) {
        (
            Env {
                ctx: Some(GearCtx {
                    project: "p".into(),
                    seat: "captain".into(),
                    generation: 1,
                }),
                pane_id: Some(CAP_PANE.into()),
                admin: false,
            },
            Fingerprint {
                pane_id: CAP_PANE.into(),
                kind: Some("codex".into()),
                session_ref: None,
            },
        )
    }

    fn draft_text(base: i64, queue: &str) -> String {
        format!("<!-- gear-plan project=p base={base} -->\n# Queue\n{queue}")
    }

    fn run(s: &mut Store, text: &str, rid: &str) -> GearResult<ApplyResult> {
        let draft = parse(text).unwrap();
        let (env, fp) = cap();
        apply(s, &env, Some(&fp), None, &draft, text, rid, NOW)
    }

    #[test]
    fn an_evidence_due_change_opens_a_contract_version() {
        let mut s = world();
        run(
            &mut s,
            &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nevidence-due: 60\ndeps: -\n"),
            "r1",
        )
        .unwrap();

        let r = run(
            &mut s,
            &draft_text(
                1,
                "## T-1 [must] lane=heat\ngoal: A\naccept: a\nevidence-due: 90\ndeps: -\n",
            ),
            "r2",
        )
        .unwrap();

        assert_eq!(r.contract_changed, vec![("T-1".to_string(), 2)]);
        let contract = s
            .read(|tx| tx.get_contract("p", "T-1", 2))
            .unwrap()
            .unwrap();
        assert_eq!(contract.evidence_due_min, Some(90));
        assert_eq!(
            s.read(|tx| tx.get_task("p", "T-1"))
                .unwrap()
                .unwrap()
                .contract_version,
            2
        );
    }

    #[test]
    fn clearing_the_evidence_files_opens_a_contract_version() {
        let mut s = world();
        run(
            &mut s,
            &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nevidence-files: notes/x.md\ndeps: -\n"),
            "r1",
        )
        .unwrap();

        let r = run(
            &mut s,
            &draft_text(
                1,
                "## T-1 [must] lane=heat\ngoal: A\naccept: a\nevidence-files:\ndeps: -\n",
            ),
            "r2",
        )
        .unwrap();

        assert_eq!(r.contract_changed, vec![("T-1".to_string(), 2)]);
        let contract = s
            .read(|tx| tx.get_contract("p", "T-1", 2))
            .unwrap()
            .unwrap();
        assert!(contract.evidence_files.is_empty(), "the files were cleared");
    }

    #[test]
    fn a_mechanical_task_stores_its_reason_and_the_block_shows_it() {
        // T-272: the queue field is a contract field: `mechanical: <reason>`
        // says the gate alone proves the change, so no review is needed.
        let mut s = world();
        run(
            &mut s,
            &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nmechanical: delete three dead fns\ndeps: -\n"),
            "r1",
        )
        .unwrap();

        let contract = s
            .read(|tx| tx.get_contract("p", "T-1", 1))
            .unwrap()
            .unwrap();
        assert_eq!(
            contract.mechanical.as_deref(),
            Some("delete three dead fns")
        );

        // `gear plan show` prints it as a field the captain can edit back in.
        let shown = s.read(|tx| show(tx, "p", "T-1")).unwrap();
        assert!(
            shown.contains("mechanical: delete three dead fns"),
            "{shown}"
        );
    }

    #[test]
    fn clearing_the_mechanical_reason_opens_a_contract_version() {
        // Re-applying the block without the field clears it, the way
        // `evidence-due` does: the queue item is the contract as the captain
        // wants it.
        let mut s = world();
        run(
            &mut s,
            &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nmechanical: delete three dead fns\ndeps: -\n"),
            "r1",
        )
        .unwrap();

        let r = run(
            &mut s,
            &draft_text(1, "## T-1 [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n"),
            "r2",
        )
        .unwrap();

        assert_eq!(r.contract_changed, vec![("T-1".to_string(), 2)]);
        let contract = s
            .read(|tx| tx.get_contract("p", "T-1", 2))
            .unwrap()
            .unwrap();
        assert_eq!(contract.mechanical, None);
        assert_eq!(
            s.read(|tx| tx.get_task("p", "T-1"))
                .unwrap()
                .unwrap()
                .contract_version,
            2
        );
    }

    #[test]
    fn a_mechanical_field_without_a_reason_is_refused() {
        let text = draft_text(
            0,
            "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nmechanical:\ndeps: -\n",
        );
        let err = parse(&text).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat);
        assert!(
            err.detail
                .contains("`mechanical` needs the reason the gate alone proves this change"),
            "{}",
            err.detail
        );
    }

    #[test]
    fn a_stale_base_is_reported_before_an_unsupported_section() {
        let mut s = world();
        run(
            &mut s,
            &draft_text(
                0,
                "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
            ),
            "r1",
        )
        .unwrap();

        let text = "<!-- gear-plan project=p base=0 -->\n# Staff\n- temp R-4 task=T-1 profile=opencode-review\n";
        let draft = parse(text).unwrap();
        let (env, fp) = cap();
        let err = apply(&mut s, &env, Some(&fp), None, &draft, text, "r2", NOW).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::StaleVersion,
            "step 3 is checked before step 4"
        );
    }
}

/// One line for a voyage (T-22): the milestone, its planned landing, the
/// sentence it is judged by, the quality bar and where it departed from.
pub fn voyage_line(voyage: &gear_store::VoyageRow) -> String {
    let mut line = match (&voyage.done, &voyage.bar, &voyage.eta) {
        (Some(done), Some(bar), Some(eta)) => format!(
            "Voyage {} {} · eta {} · done: {} · bar: {}",
            voyage.milestone, voyage.title, eta, done, bar
        ),
        // Tasks predate the first block: the departure is known, the words are not.
        _ => format!(
            "Voyage {} {} · not described yet (no `# Voyage {}` block)",
            voyage.milestone, voyage.title, voyage.milestone
        ),
    };
    match (voyage.departure, voyage.departure_tasks) {
        (Some(departure), Some(tasks)) => line.push_str(&format!(
            " · departed {} with {tasks} tasks",
            utc_date(departure)
        )),
        _ => line.push_str(" · not departed yet"),
    }
    line
}

/// `YYYY-MM-DD` for unix seconds (civil-from-days, no extra crate) — enough to
/// date a departure without pulling a time library in.
pub fn utc_date(ts: i64) -> String {
    let (days, _) = (ts.div_euclid(86_400), ts.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Whether refill/idle warnings are suppressed by the captain's tail decision.
///
/// The tail can end by itself (B6): `- tail: heat until=T-12` stops silencing
/// the lane when T-12 is done, `- tail: heat until=2026-10-26T18:00:00Z` when
/// that time has come. The callers that have no clock of their own get the
/// wall clock — a deadline is a wall-clock stamp, so that is the clock it is
/// compared with.
pub fn lane_in_tail(tx: &Tx<'_>, project: &str, lane: &str) -> GearResult<bool> {
    lane_in_tail_now(tx, project, lane, wall_clock(), true)
}

/// The same, with the caller's clock, writing nothing: the read-only callers
/// (the plan's own checks) see the tail as it is, without ending it.
pub fn lane_in_tail_at(tx: &Tx<'_>, project: &str, lane: &str, now: Ts) -> GearResult<bool> {
    lane_in_tail_now(tx, project, lane, now, false)
}

/// Whether the captain's tail silences this lane now, and — when the caller
/// writes — the end of a tail that has ended.
///
/// The end is written once: the lanes the tail silences go back to "not
/// tailed" and the declaration is marked spent. A task that is reopened for a
/// rework (a failed merge, a fail verdict) therefore never revives a tail
/// that has already ended (design §10.7, row B6).
fn lane_in_tail_now(
    tx: &Tx<'_>,
    project: &str,
    lane: &str,
    now: Ts,
    write: bool,
) -> GearResult<bool> {
    if tx
        .get_meta(&format!("lane-tail:{project}:{lane}"))?
        .as_deref()
        != Some("1")
    {
        return Ok(false);
    }
    if !tail_is_over(tx, project, now)? {
        return Ok(true);
    }
    if write {
        persist_tail_end(tx, project)?;
    }
    Ok(false)
}

/// Write the end of a tail: the lanes it silences go back to "not tailed" and
/// the declaration is marked spent — a later apply of the same line (it is
/// still in the plan file) does not revive it.
fn persist_tail_end(tx: &Tx<'_>, project: &str) -> GearResult<()> {
    let mut lanes: Vec<String> = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane {
            continue;
        }
        if tx
            .get_meta(&format!("lane-tail:{project}:{}", seat.name))?
            .as_deref()
            == Some("1")
        {
            lanes.push(seat.name);
        }
    }
    if lanes.is_empty() {
        return Ok(());
    }
    lanes.sort();
    let until = tx
        .get_meta(&format!("lane-tail-until:{project}"))?
        .unwrap_or_default();
    let declaration = tail_declaration(&lanes, &until);
    for lane in &lanes {
        tx.set_meta(&format!("lane-tail:{project}:{lane}"), "0")?;
    }
    tx.set_meta(&format!("lane-tail-spent:{project}"), &declaration)?;
    Ok(())
}

/// The fingerprint of a tail declaration: the lanes it silences and its end.
/// The same declaration replayed (the line is still in the plan file) has the
/// same fingerprint.
fn tail_declaration(lanes: &[String], until: &str) -> String {
    format!("{}|{}", lanes.join(","), until)
}

/// When the captain's tail ends by itself (B6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailUntil {
    /// The tail ends when the task is over (done, failed or cancelled).
    Task(String),
    /// The tail ends at this moment (unix seconds).
    At(Ts),
}

impl TailUntil {
    /// The form the store keeps it in.
    fn as_stored(&self) -> String {
        match self {
            TailUntil::Task(task) => format!("task:{task}"),
            TailUntil::At(at) => format!("at:{at}"),
        }
    }

    /// The form the store keeps it in, read back.
    fn from_stored(text: &str) -> Option<TailUntil> {
        let (kind, value) = text.split_once(':')?;
        match kind {
            "task" => Some(TailUntil::Task(value.to_string())),
            "at" => value.parse().ok().map(TailUntil::At),
            _ => None,
        }
    }

    /// The text the captain reads (in the plan and in the apply's output).
    pub fn as_text(&self) -> String {
        match self {
            TailUntil::Task(task) => format!("until={task}"),
            TailUntil::At(at) => format!("until={}", rfc3339(*at)),
        }
    }
}

/// The tail's end, when it has one.
pub fn tail_until(tx: &Tx<'_>, project: &str) -> GearResult<Option<TailUntil>> {
    Ok(tx
        .get_meta(&format!("lane-tail-until:{project}"))?
        .and_then(|text| TailUntil::from_stored(&text)))
}

/// Whether the tail is over: its end has come (B6).
fn tail_is_over(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<bool> {
    let Some(until) = tail_until(tx, project)? else {
        return Ok(false);
    };
    match until {
        // The task the tail waits for is over: done, failed or cancelled. A
        // task that failed can be reworked by a new attempt, but the wait is
        // over — the captain decides what happens next.
        TailUntil::Task(task) => Ok(match tx.get_task(project, &task)? {
            Some(row) => matches!(
                row.state,
                TaskState::Done | TaskState::Failed | TaskState::Cancelled
            ),
            // A task that does not exist will never be done.
            None => true,
        }),
        TailUntil::At(at) => Ok(now >= at),
    }
}

/// The lanes the captain's tail silences right now (B5: the apply says which
/// ones, so a lane that gets no warning is not a mystery). The end of a tail
/// that has ended is written here, so the apply's path ends it too.
pub fn tailed_lanes(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<String>> {
    let mut tailed = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane {
            continue;
        }
        if lane_in_tail_now(tx, project, &seat.name, now, true)? {
            tailed.push(seat.name);
        }
    }
    Ok(tailed)
}

/// The wall clock, for the callers that have none. A tail's deadline is a
/// wall-clock stamp, so the wall clock is the clock it is compared with.
fn wall_clock() -> Ts {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// A lane that is about to stand idle after an apply (T-58, gear/voyages/M2.1/lessons.md 7a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleLane {
    /// The lane's name.
    pub lane: String,
    /// The lane's current task, when it has one: the task it is working on
    /// (Running), waiting on (Waiting) or in review (Verifying).
    pub current: Option<String>,
    /// The lane's queued tasks that wait for a dependency that is not done.
    pub blocked: Vec<String>,
}

impl IdleLane {
    /// The warning the captain reads after an apply (T-58):
    /// `warning: lane heat has nothing to start after T-3 (1 queued behind deps: T-5)`.
    pub fn warning(&self) -> String {
        let after = self.current.clone().unwrap_or_else(|| "now".to_string());
        let queued = if self.blocked.is_empty() {
            "0 queued behind deps".to_string()
        } else {
            format!(
                "{} queued behind deps: {}",
                self.blocked.len(),
                self.blocked.join(", ")
            )
        };
        format!(
            "warning: lane {} has nothing to start after {after} ({queued})",
            self.lane
        )
    }
}

/// The lanes that are **about to** stand idle after an apply (T-58, gear/voyages/M2.1/lessons.md
/// 7a): a lane that is up (Active or Standby — the states the engine's feed
/// watches), **not** in the voyage's tail, with no queued task it could start
/// now — every dependency of its queued tasks is still open.
///
/// The lane is told before it stands idle, not after: a lane working on its
/// last task with nothing queued behind it is warned too (the user: warn
/// before the lane runs dry — idle is only the last net).
///
/// A task in Verifying does not occupy the lane: it handed the task in and the
/// review runs in the background (never-stop), so the lane may take the next
/// one — and stands idle when there is none.
///
/// Read-only: the apply is not changed by this, the captain is only told.
pub fn idle_lanes(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<Vec<IdleLane>> {
    let tasks = tx.tasks_of_project(project)?;
    let done: BTreeSet<&str> = tasks
        .iter()
        .filter(|task| task.state == TaskState::Done)
        .map(|task| task.id.as_str())
        .collect();
    let mut idle = Vec::new();
    for seat in tx.seats_of_project(project)? {
        if seat.role != SeatRole::Lane
            || !matches!(seat.state, SeatState::Active | SeatState::Standby)
        {
            continue;
        }
        // The captain's tail: these lanes may stand idle (design/05). The
        // read does not end a tail (the apply's path does).
        if lane_in_tail_at(tx, project, &seat.name, now)? {
            continue;
        }
        // A lane that helps on an unanswered instruction of the captain is
        // busy, not idle (T-323).
        if gear_board::idle::helping(tx, project, &seat, now)?.is_some() {
            continue;
        }
        let lane_tasks: Vec<&TaskRow> = tasks
            .iter()
            .filter(|task| task.lane.as_deref() == Some(seat.name.as_str()))
            .collect();
        // The lane's current task: the one it is working on, waiting on, or
        // in review (the board's wording).
        let current = lane_tasks
            .iter()
            .find(|task| task.state == TaskState::Running)
            .or_else(|| {
                lane_tasks
                    .iter()
                    .find(|task| task.state == TaskState::Waiting)
            })
            .or_else(|| {
                lane_tasks
                    .iter()
                    .find(|task| task.state == TaskState::Verifying)
            })
            .map(|task| task.id.clone());
        // The lane's queued tasks: the one it can start now, and the ones
        // that wait for a dependency that is not done.
        let mut startable = false;
        let mut blocked = Vec::new();
        for task in lane_tasks
            .iter()
            .filter(|task| task.state == TaskState::Ready)
            .filter(|task| task.priority != "wont")
        {
            if task.deps.iter().all(|dep| done.contains(dep.as_str())) {
                startable = true;
            } else {
                blocked.push(task.id.clone());
            }
        }
        if startable {
            continue;
        }
        idle.push(IdleLane {
            lane: seat.name.clone(),
            current,
            blocked,
        });
    }
    idle.sort_by(|a, b| a.lane.cmp(&b.lane));
    Ok(idle)
}

/// `# Decisions` / `- tail: heat wire` replaces the named tail lanes;
/// `- tail: -` clears the declaration. Omitted declarations keep the state.
///
/// The tail can end by itself (B6): `- tail: heat until=T-12` (the tail ends
/// when T-12 is over) or `- tail: heat until=2026-10-26T18:00:00Z` (at that
/// time). The end belongs to the declaration: a tail without one lasts until
/// the captain clears it.
///
/// Every tail line is validated, but only the final effective declaration
/// writes: the `# Decisions` history is retained, and an earlier line must
/// neither clear the spent marker nor revive a tail that has ended (T-191).
fn record_tail_decisions(tx: &Tx<'_>, project: &str, draft: &Draft) -> GearResult<()> {
    // Every tail line is validated (a broken one refuses the whole apply),
    // but only the final effective declaration writes: the `# Decisions`
    // history is retained, and an earlier line must never clear the spent
    // marker or revive a tail that has ended (T-191).
    let mut decls: Vec<TailDecl> = Vec::new();
    for decision in &draft.decisions {
        let Some(rest) = decision.strip_prefix("tail:") else {
            continue;
        };
        let mut lanes: BTreeSet<String> = BTreeSet::new();
        let mut until: Option<TailUntil> = None;
        for token in rest.split(|c: char| c.is_whitespace() || c == ',') {
            if token.is_empty() {
                continue;
            }
            if let Some(value) = token.strip_prefix("until=") {
                if until.is_some() {
                    return Err(bad(1, "tail: takes one `until=`"));
                }
                until = Some(parse_tail_until(value)?);
                continue;
            }
            lanes.insert(token.to_string());
        }
        if lanes.is_empty() {
            return Err(bad(1, "tail: needs lane names, or - to clear"));
        }
        let seats = tx.seats_of_project(project)?;
        for name in &lanes {
            if *name != "-"
                && !seats
                    .iter()
                    .any(|s| s.role == SeatRole::Lane && s.name == *name)
            {
                return Err(bad(1, format!("tail: unknown lane {name}")));
            }
        }
        if lanes.contains("-") && lanes.len() != 1 {
            return Err(bad(1, "tail: - cannot be combined with lane names"));
        }
        // The end of the tail: a task that must be over, or a moment.
        if let Some(TailUntil::Task(task)) = &until {
            if tx.get_task(project, task)?.is_none() {
                return Err(bad(1, format!("tail: unknown task {task}")));
            }
        }
        if lanes.contains("-") && until.is_some() {
            return Err(bad(1, "tail: - clears the tail, it takes no `until=`"));
        }
        let until_stored = until.as_ref().map(TailUntil::as_stored).unwrap_or_default();
        decls.push(TailDecl {
            lanes: lanes.into_iter().collect(),
            until: until_stored,
        });
    }
    // A tail line replaces the set of lanes it silences: the last line is the
    // effective declaration, the earlier ones are history.
    let Some(effective) = decls.last() else {
        // No tail line at all: the state is preserved.
        return Ok(());
    };
    // The same declaration, replayed (its line is still in the plan file),
    // must not revive a tail that has already ended: only a genuinely new
    // declaration resets it. The marker is read before this replay writes
    // anything — an earlier line in the retained history never clears it.
    let spent_before = tx
        .get_meta(&format!("lane-tail-spent:{project}"))?
        .unwrap_or_default();
    let declaration = tail_declaration(&effective.lanes, &effective.until);
    let spent = spent_before == declaration;
    let seats = tx.seats_of_project(project)?;
    for seat in seats.iter().filter(|s| s.role == SeatRole::Lane) {
        tx.set_meta(
            &format!("lane-tail:{project}:{}", seat.name),
            if spent {
                "0"
            } else if effective.lanes.contains(&seat.name) {
                "1"
            } else {
                "0"
            },
        )?;
    }
    if !spent {
        tx.set_meta(&format!("lane-tail-spent:{project}"), "")?;
    }
    tx.set_meta(&format!("lane-tail-until:{project}"), &effective.until)?;
    Ok(())
}

/// A tail declaration, as the captain wrote it: the lanes it silences and
/// its end.
struct TailDecl {
    /// The lanes it names (`-` alone clears the tail).
    lanes: Vec<String>,
    /// Its end, in the form the store keeps ("" when it has none).
    until: String,
}

/// The end of a tail: a task id or an RFC3339 moment.
fn parse_tail_until(value: &str) -> GearResult<TailUntil> {
    let is_task = match value.strip_prefix("T-") {
        Some(number) => !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    };
    if is_task {
        return Ok(TailUntil::Task(value.to_string()));
    }
    match parse_rfc3339(value) {
        Some(at) => Ok(TailUntil::At(at)),
        None => Err(bad(
            1,
            format!("tail: `until=` needs a task id or an RFC3339 time, got {value:?}"),
        )),
    }
}

/// An RFC3339 moment (`2026-10-26T18:00:00Z`, with an optional `+HH:MM` or
/// `-HH:MM` offset) as unix seconds. `None` when the text is not one.
///
/// No time library: the civil date is counted from the Unix epoch, the same
/// way [`utc_date`] counts back.
pub fn parse_rfc3339(text: &str) -> Option<Ts> {
    let text = text.trim();
    let (date, time) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    // The day must exist in that month: 2026-02-31 is not a date, and
    // counting it as one would end the tail in March.
    if date_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
    {
        return None;
    }
    // The zone: `Z`, `+HH:MM`, `-HH:MM` or nothing (UTC).
    let (clock, zone) = match time.find(['+', '-']) {
        Some(at) => (&time[..at], &time[at..]),
        None => {
            // The zone may be the `Z` at the end: it is not part of the clock.
            let clock = time
                .strip_suffix('Z')
                .or_else(|| time.strip_suffix('z'))
                .unwrap_or(time);
            (clock, "Z")
        }
    };
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next().unwrap_or("0").parse().ok()?;
    // The seconds, with an optional fraction (`18:00:00.123`).
    let (seconds_text, fraction) = match clock_parts.next() {
        Some(text) => match text.split_once('.') {
            Some((seconds, fraction)) => (seconds, Some(fraction)),
            None => (text, None),
        },
        None => ("0", None),
    };
    let second: i64 = seconds_text.parse().ok()?;
    if clock_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // The fraction: digits only, and a nonzero one rounds the moment up to the
    // next second — the tail must not end before the instant the captain
    // named.
    let mut second = second;
    if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if fraction.bytes().any(|byte| byte != b'0') {
            second += 1;
        }
    }
    let offset = match zone {
        "Z" | "z" => 0,
        _ => {
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hours, minutes) = zone[1..].split_once(':')?;
            let hours: i64 = hours.parse().ok()?;
            let minutes: i64 = minutes.parse().ok()?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// The civil date of a moment as `YYYY-MM-DDTHH:MM:SSZ` (the inverse of
/// [`parse_rfc3339`]).
fn rfc3339(ts: Ts) -> String {
    let days = ts.div_euclid(86_400);
    let seconds = ts.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// The days in a month of a year (0 for a month that does not exist).
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// The days since the Unix epoch of a civil date (Howard Hinnant's civil
/// calendar algorithms).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = floor_div(year, 400);
    let year_of_era = year - era * 400;
    let month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date of a number of days since the Unix epoch.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = floor_div(z, 146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The floor of a division (Rust's `/` truncates towards zero).
fn floor_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && (a < 0) != (b < 0) {
        q - 1
    } else {
        q
    }
}

/// Fix the departure of every milestone that has tasks and no departure yet:
/// the moment its first tasks were created. This runs on **every** apply, not
/// only when the draft repeats the milestone block, so a queue-only apply that
/// creates the first tasks departs then (review R1) and a later block edit
/// cannot move it (R2). A milestone whose tasks predate the voyage record takes
/// its departure from the oldest plan draft that already carried them.
///
/// A milestone emptied of its tasks loses its departure (T-239), so a reused
/// milestone id starts its clock when its new tasks come: the reset is recorded
/// in `meta`, and `departure_from_history` searches only the drafts applied
/// after it. A milestone that still has tasks never changes.
fn fix_departures(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<()> {
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for task in tx.tasks_of_project(project)? {
        if let Some(milestone) = task.milestone {
            *counts.entry(milestone).or_insert(0) += 1;
        }
    }
    // A voyage whose milestone has no task left keeps no departure: the id is
    // free to be reused, and its history must not start at the emptied drafts.
    let version = tx.plan_version(project)?;
    for voyage in tx.voyages_of_project(project)? {
        if voyage.departure.is_none() || counts.contains_key(&voyage.milestone) {
            continue;
        }
        tx.clear_voyage_departure(project, &voyage.milestone)?;
        tx.set_meta(
            &departure_reset_key(project, &voyage.milestone),
            &version.to_string(),
        )?;
    }
    for (milestone, tasks) in counts {
        let voyage = tx.get_voyage(project, &milestone)?;
        if voyage
            .as_ref()
            .is_some_and(|v| v.departure.is_some() && v.departure_estimate_min.is_some())
        {
            continue;
        }
        let history = departure_from_history(tx, project, &milestone)?;
        let already_departed = voyage.as_ref().is_some_and(|v| v.departure.is_some());
        let (departure, at_departure, estimate) = match history {
            Some(found) => found,
            None if !already_departed => {
                let estimates = tx
                    .tasks_of_project(project)?
                    .iter()
                    .filter(|task| task.milestone.as_deref() == Some(&milestone))
                    .map(|task| gear_board::voyage::estimate_of(tx, project, task))
                    .collect::<GearResult<Vec<_>>>()?;
                (now, tasks, estimates.into_iter().sum())
            }
            None => continue, // No historical evidence: do not invent a baseline.
        };
        tx.set_voyage_departure(project, &milestone, departure, at_departure, now)?;
        if let Some(estimate) = estimate {
            tx.set_voyage_departure_estimate(project, &milestone, estimate)?;
        }
    }
    Ok(())
}

/// The milestones whose voyage had departed before this apply (T-410): their
/// `done:`, `bar:` and `eta:` change only with the user's confirmation. Read
/// before `fix_departures`, so a departure this apply fixes (a reused id,
/// T-239) is not one of them.
fn departed_voyages(tx: &Tx<'_>, project: &str) -> GearResult<std::collections::BTreeSet<String>> {
    Ok(tx
        .voyages_of_project(project)?
        .into_iter()
        .filter(|voyage| voyage.departure.is_some())
        .map(|voyage| voyage.milestone)
        .collect())
}

/// T-461: the departures and the moves a draft would make, read before the
/// queue writes. A milestone that already carried tasks keeps its departure;
/// one that had none — a new voyage, or a reused id after T-239 emptied it —
/// departs when this draft gives it its first tasks.
fn planned_guard(
    tx: &Tx<'_>,
    project: &str,
    draft: &Draft,
) -> GearResult<(BTreeSet<String>, Vec<(String, String, String)>)> {
    let existing: BTreeMap<String, TaskRow> = tx
        .tasks_of_project(project)?
        .into_iter()
        .map(|task| (task.id.clone(), task))
        .collect();
    let mut counts_before: BTreeMap<String, usize> = BTreeMap::new();
    for task in existing.values() {
        if let Some(milestone) = &task.milestone {
            *counts_before.entry(milestone.clone()).or_default() += 1;
        }
    }
    let mut after: BTreeMap<String, Option<String>> = existing
        .iter()
        .map(|(id, task)| (id.clone(), task.milestone.clone()))
        .collect();
    let mut moved: Vec<(String, String, String)> = Vec::new();
    for (index, item) in draft.queue.iter().enumerate() {
        let key = match &item.id {
            Some(id) => id.clone(),
            None => format!("#{index}"),
        };
        let old = after.get(&key).cloned().unwrap_or(None);
        if let (Some(old), Some(new)) = (&old, &item.milestone) {
            if old != new {
                moved.push((key.clone(), old.clone(), new.clone()));
            }
        }
        after.insert(key, item.milestone.clone());
    }
    let mut counts_after: BTreeMap<String, usize> = BTreeMap::new();
    for milestone in after.values().flatten() {
        *counts_after.entry(milestone.clone()).or_default() += 1;
    }
    let mut departing: BTreeSet<String> = BTreeSet::new();
    for milestone in counts_after.keys() {
        if counts_before.get(milestone).copied().unwrap_or(0) > 0 {
            continue;
        }
        let voyage = tx.get_voyage(project, milestone)?;
        if voyage.as_ref().is_some_and(|v| v.departure.is_some()) {
            continue;
        }
        departing.insert(milestone.clone());
    }
    Ok((departing, moved))
}

/// T-461: a new milestone's departure and a move between milestones are
/// applied only with `--confirmed` and a `confirmed:` line in this same
/// `# Decisions`; without both the apply is refused and nothing is written.
fn require_confirmed_changes(
    draft: &Draft,
    departing: &BTreeSet<String>,
    moved: &[(String, String, String)],
) -> GearResult<()> {
    // T-467: a raise of a count is the user's change too: it needs the same
    // `--confirmed` and `confirmed:` line as a departure or a move.
    let raises: Vec<(String, u32, String)> = draft
        .staff
        .iter()
        .filter_map(|item| match item {
            StaffItem::Raise {
                role, to, until, ..
            } => Some((role.clone(), *to, until.clone())),
            _ => None,
        })
        .collect();
    if departing.is_empty() && moved.is_empty() && raises.is_empty() {
        return Ok(());
    }
    if !draft.confirmed {
        let what = if !raises.is_empty() {
            let (role, to, until) = &raises[0];
            format!("the raise of {role} to {to} until {until} in this apply")
        } else if !departing.is_empty() {
            format!(
                "milestone {} departs in this apply",
                departing.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        } else {
            let (id, old, new) = &moved[0];
            format!("{id} moves from {old} to {new} in this apply")
        };
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "{what}: a raise, a departure and a move between milestones are the user's; confirm it with them, then apply again with --confirmed and a `confirmed:` line in # Decisions"
            ),
        ));
    }
    let confirmed = confirmed_decisions(draft);
    if confirmed.is_empty() {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "this apply changes {} but # Decisions carries no `confirmed:` line: add `- <time> confirmed: <what changes, before -> after> | why: <the user's reason>` in this same apply",
                guarded_summary(departing, moved, &raises)
            ),
        ));
    }
    for (role, to, until) in &raises {
        // The captain's line names the role and the raised count; its
        // `until ...` wording is the captain's own, so it is not matched
        // word for word against the staff line's end condition.
        let named = confirmed.iter().any(|decision| {
            decision.what.contains(role.as_str()) && decision.what.contains(&to.to_string())
        });
        if !named {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "the raise of {role} to {to} until {until} needs its own `confirmed:` line: add `- <time> confirmed: {role} <from> -> {to} until {until} | why: <the user's reason>` in this same apply"
                ),
            ));
        }
    }
    Ok(())
}

/// The `confirmed:` lines of a draft (T-461): the captain's record of the
/// user's word, `- <time> confirmed: <what changes, before -> after> | why:
/// <the user's reason>`. A malformed line is a note and does not count.
fn confirmed_decisions(draft: &Draft) -> Vec<crate::decision::Decision> {
    draft
        .decisions
        .iter()
        .map(|line| crate::decision::parse(line))
        .filter(|decision| decision.kind == "confirmed" && !decision.note)
        .collect()
}

/// What a guarded apply changes, for the refusal that names it (T-461).
fn guarded_summary(
    departing: &std::collections::BTreeSet<String>,
    moved: &[(String, String, String)],
    raises: &[(String, u32, String)],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (role, to, until) in raises {
        parts.push(format!("the raise of {role} to {to} until {until}"));
    }
    if !departing.is_empty() {
        parts.push(format!(
            "the departure of {}",
            departing.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    for (id, old, new) in moved {
        parts.push(format!("{id} moving from {old} to {new}"));
    }
    parts.join(" and ")
}

/// T-461: a cancel line changes the state only. Every other field it carries
/// must be the task's current one; a difference is refused, so a cut never
/// hides a move, a lane change or a reworded contract.
fn cancel_changes_only_state(
    tx: &Tx<'_>,
    project: &str,
    item: &QueueItem,
    task: &TaskRow,
) -> GearResult<()> {
    let id = task.id.as_str();
    let mut changed: Vec<String> = Vec::new();
    if item.milestone != task.milestone {
        changed.push(format!(
            "milestone (stored {:?}, got {:?})",
            task.milestone, item.milestone
        ));
    }
    if item.lane != task.lane {
        changed.push(format!(
            "lane (stored {:?}, got {:?})",
            task.lane, item.lane
        ));
    }
    let priority = priority_str(item.priority).to_string();
    if priority != task.priority {
        changed.push(format!(
            "priority (stored {:?}, got {priority:?})",
            task.priority
        ));
    }
    if item.required != task.required {
        changed.push(format!(
            "required (stored {}, got {})",
            task.required, item.required
        ));
    }
    if item.deps != task.deps {
        changed.push(format!(
            "deps (stored {:?}, got {:?})",
            task.deps, item.deps
        ));
    }
    if let Some(estimate) = item.estimate {
        if Some(i64::from(estimate)) != task.estimate_min {
            changed.push(format!(
                "estimate (stored {:?}, got {estimate})",
                task.estimate_min
            ));
        }
    }
    if let Some(followup) = &item.followup_of {
        if Some(followup.clone()) != task.followup_of {
            changed.push(format!(
                "followup-of (stored {:?}, got {followup:?})",
                task.followup_of
            ));
        }
    }
    if let Some(issue) = item.issue {
        let issue = i64::try_from(issue).ok();
        if issue != task.issue {
            changed.push(format!("issue (stored {:?}, got {:?})", task.issue, issue));
        }
    }
    // The station is a field too (M5 S3): a block without `station=` clears
    // it, so a cancel that carries a different one is refused.
    let stored_station = tx.task_station(project, id)?;
    if item.station != stored_station {
        changed.push(format!(
            "station (stored {:?}, got {:?})",
            stored_station, item.station
        ));
    }
    if let Some(contract) = tx.get_contract(project, id, task.contract_version)? {
        if let Some(goal) = &item.goal {
            if *goal != contract.goal {
                changed.push(format!("goal (stored {:?}, got {goal:?})", contract.goal));
            }
        }
        if let Some(accept) = &item.accept {
            if *accept != contract.accept {
                changed.push(format!(
                    "accept (stored {:?}, got {accept:?})",
                    contract.accept
                ));
            }
        }
        if item.evidence_files != contract.evidence_files {
            changed.push(format!(
                "evidence-files (stored {:?}, got {:?})",
                contract.evidence_files, item.evidence_files
            ));
        }
        if item.evidence_due.map(i64::from) != contract.evidence_due_min {
            changed.push(format!(
                "evidence-due (stored {:?}, got {:?})",
                contract.evidence_due_min, item.evidence_due
            ));
        }
        if item.mechanical != contract.mechanical {
            changed.push(format!(
                "mechanical (stored {:?}, got {:?})",
                contract.mechanical, item.mechanical
            ));
        }
    }
    if changed.is_empty() {
        return Ok(());
    }
    Err(bad(
        item.line,
        format!(
            "{id}: a cancel line changes the state only: it also changes {}; write the change on its own line",
            changed.join(", ")
        ),
    ))
}

/// The meta key that remembers the plan version at which a milestone lost its
/// last task (T-239): `departure_from_history` starts after it.
fn departure_reset_key(project: &str, milestone: &str) -> String {
    format!("departure-reset:{project}:{milestone}")
}

/// Where a milestone that predates the voyage record departed from: the oldest
/// plan version whose draft already had tasks on it, with how many. The plan's
/// stored drafts are the persisted history (tasks carry no creation time).
///
/// A milestone that was emptied and reused starts after its reset (T-239): the
/// drafts before it carried the earlier voyage's tasks, not this one's.
fn departure_from_history(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
) -> GearResult<Option<(Ts, i64, Option<i64>)>> {
    let newest = tx.plan_version(project)?;
    let reset = tx
        .get_meta(&departure_reset_key(project, milestone))?
        .and_then(|raw| raw.trim().parse::<i64>().ok());
    let start = reset.map_or(1, |version| version + 1);
    for version in start..=newest {
        let Some(plan) = tx.get_plan(project, version)? else {
            continue;
        };
        let Ok(draft) = crate::draft::parse(&plan.draft) else {
            continue;
        };
        let items: Vec<_> = draft
            .queue
            .iter()
            .filter(|item| item.milestone.as_deref() == Some(milestone))
            .collect();
        if !items.is_empty() {
            let estimate = items
                .iter()
                .map(|item| {
                    item.estimate.map(i64::from).or_else(|| {
                        item.goal
                            .as_deref()
                            .and_then(gear_board::voyage::estimate_from_goal)
                    })
                })
                .sum();
            return Ok(Some((plan.applied, items.len() as i64, estimate)));
        }
    }
    Ok(None)
}

/// Record the draft's milestone blocks as voyages (T-22).
///
/// Every block in the draft must carry `done:`, `bar:` and `eta:` — the one
/// sentence the phase is judged by, the quality bar, and its planned landing;
/// a block missing one is `BadFormat`, naming the milestone. A milestone that
/// the draft does not mention keeps what it has (grandfathered): the
/// requirement applies to what the captain edits, not to old plans.
///
/// Finish a task the captain cancels from the plan (T-93): every open attempt
/// becomes Abandoned, so the lane is free for its next task, and the lane gets
/// one line telling it to stop. The message is created once, when the task is
/// newly cancelled.
fn cancel_task(
    tx: &Tx<'_>,
    project: &str,
    task: &TaskRow,
    by_seat: &str,
    by_gen: i64,
    now: Ts,
) -> GearResult<()> {
    let closed = abandon_open_attempts(tx, project, &task.id, now)?;
    // The `fresh:` mark of a task is about the round that starts it (T-481):
    // a cancelled task carries none.
    tx.clear_task_fresh(project, &task.id)?;
    // The lane that was working on it hears why, in one line.
    let Some(lane) = task.lane.as_deref() else {
        return Ok(());
    };
    let lane_seat = format!("{project}:{lane}");
    if closed && tx.get_seat(&lane_seat)?.is_some() {
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread: format!("{}:cancel", task.id),
            task: Some(task.id.clone()),
            attempt: None,
            sender: by_seat.to_string(),
            sender_gen: by_gen,
            recipient: lane_seat,
            kind: MsgKind::Instruction,
            urgent: false,
            body: format!(
                "{} is cancelled; stop work on it, nothing to hand in",
                task.id
            ),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
    }
    Ok(())
}

/// Close every open attempt of a task (T-93, T-311): a cancelled task and a
/// task that moves to another lane both finish with their attempts abandoned,
/// so the seat is free for the next task. Returns whether one was closed.
fn abandon_open_attempts(tx: &Tx<'_>, project: &str, task_id: &str, now: Ts) -> GearResult<bool> {
    let mut closed = false;
    for mut attempt in tx.attempts_of_task(project, task_id)? {
        if attempt.state.is_terminal() {
            continue;
        }
        if !AttemptState::can_transition(attempt.state, AttemptState::Abandoned) {
            // The model keeps UNKNOWN from Abandoned directly — the engine
            // never guesses an outcome. A cancelled task does not guess
            // either: it stops the attempt first, then abandons it, so the
            // lane is not left busy with a task nobody wants (T-93).
            if attempt.state == AttemptState::Unknown
                && AttemptState::can_transition(AttemptState::Unknown, AttemptState::Stopped)
                && AttemptState::can_transition(AttemptState::Stopped, AttemptState::Abandoned)
            {
                attempt.state = AttemptState::Stopped;
                attempt.updated = now;
                tx.update_attempt(&attempt)?;
            } else {
                return Err(GearError::new(
                    ErrorCode::BadTransition,
                    format!(
                        "attempt {} of task {} is {}, it cannot be abandoned",
                        attempt.n,
                        task_id,
                        attempt.state.as_str()
                    ),
                ));
            }
        }
        attempt.state = AttemptState::Abandoned;
        attempt.updated = now;
        tx.update_attempt(&attempt)?;
        closed = true;
    }
    Ok(closed)
}

/// Clear a task's wait: the wait and its condition go, and the wait pause of
/// every seat that was working on the task closes. The plan does it for a
/// contract change (T-8) and for a task moved to another lane (T-311); the
/// seats are the attempts' ones, read before the attempts are closed.
fn clear_wait(
    tx: &Tx<'_>,
    project: &str,
    task: &mut TaskRow,
    seats: &[String],
    now: Ts,
) -> GearResult<()> {
    if task.wait_on.take().is_none() {
        return Ok(());
    }
    task.wait_cond = None;
    let pauses = tx.pauses_of_project(project)?;
    for seat in seats {
        if pauses
            .iter()
            .any(|p| p.seat == *seat && p.kind == "wait" && p.end_at.is_none())
        {
            tx.close_pause(project, seat, "wait", now)?;
        }
    }
    Ok(())
}

/// The voyage a plan is for: the last milestone block of the file (the
/// current one). A plan with no milestone block — a decisions-only or
/// staff-only apply — belongs to the voyage the project is in, the one that
/// departed last (the same rule the config logging uses).
fn voyage_of(tx: &Tx<'_>, project: &str, draft: &Draft) -> GearResult<String> {
    if let Some((header, _)) = draft.milestones.last() {
        return Ok(crate::draft::milestone_parts(header).0.to_string());
    }
    current_voyage(tx, project)
}

/// The voyage the project is in: the one that departed last.
pub fn current_voyage(tx: &Tx<'_>, project: &str) -> GearResult<String> {
    let mut latest: Option<(Ts, String)> = None;
    for voyage in tx.voyages_of_project(project)? {
        let Some(departure) = voyage.departure else {
            continue;
        };
        if latest
            .as_ref()
            .map(|(at, _)| departure > *at)
            .unwrap_or(true)
        {
            latest = Some((departure, voyage.milestone));
        }
    }
    Ok(latest.map(|(_, milestone)| milestone).unwrap_or_default())
}

/// Record a decision of the captain (§7.3): the line as it was written, its
/// parts, and its key — the sha256 of the line with its whitespace collapsed.
/// A line that is already in the log is skipped, and an edited line is a new
/// decision (the table is append-only).
fn record_decision(
    tx: &Tx<'_>,
    project: &str,
    voyage: &str,
    version: i64,
    line: &str,
    decision: &crate::decision::Decision,
    now: Ts,
) -> GearResult<()> {
    tx.insert_decision_if_new(&gear_store::NewDecision {
        project: project.to_string(),
        key: crate::decision::key(line),
        voyage: voyage.to_string(),
        at: if decision.at.is_empty() {
            crate::decision::apply_time(now)
        } else {
            decision.at.clone()
        },
        kind: decision.kind.clone(),
        what: decision.what.clone(),
        why: decision.why.clone(),
        alt: decision.alt.clone(),
        changes: decision.changes.clone(),
        star: decision.star,
        source: "plan".to_string(),
        line: Some(line.to_string()),
        plan_version: Some(version),
    })?;
    Ok(())
}

/// Record an entry GEAR writes itself (§7.4): the event, its kind and what it
/// was about. The key is the sha256 of the entry's own text, so the same event
/// is recorded once.
fn record_gear_decision(
    tx: &Tx<'_>,
    project: &str,
    voyage: &str,
    version: i64,
    kind: &str,
    what: &str,
    why: Option<String>,
    event: &str,
    now: Ts,
) -> GearResult<()> {
    let text = format!("{kind}|{event}|{what}|{}", why.clone().unwrap_or_default());
    tx.insert_decision_if_new(&gear_store::NewDecision {
        project: project.to_string(),
        key: crate::decision::key_of(&text),
        voyage: voyage.to_string(),
        at: crate::decision::apply_time(now),
        kind: kind.to_string(),
        what: what.to_string(),
        why,
        alt: None,
        changes: None,
        // GEAR's own entries are never starred (§7.2).
        star: false,
        source: "gear".to_string(),
        line: None,
        plan_version: Some(version),
    })?;
    Ok(())
}

/// The text of a staff line as the captain wrote it (§7.4: the `staff`
/// entry's "what" is the line).
fn staff_text(item: &StaffItem) -> String {
    match item {
        StaffItem::Temp { id, task, .. } => format!("temp {id} task={task}"),
        StaffItem::Park {
            lane,
            until,
            until_task,
            ..
        } => match (until, until_task) {
            (Some(until), _) => format!("park {lane} until={until}"),
            (_, Some(task)) => format!("park {lane} until-task={task}"),
            _ => format!("park {lane}"),
        },
        StaffItem::Hold { lane, reason, .. } => format!("hold {lane} reason={reason}"),
        StaffItem::Retire { lane, .. } => format!("retire {lane}"),
        StaffItem::Retry { lane, .. } => format!("retry {lane}"),
        StaffItem::Release { name, .. } => format!("release {name}"),
        StaffItem::Remerge { task, .. } => format!("remerge {task}"),
        StaffItem::Rebind {
            lane, repo, owns, ..
        } => format!("rebind {lane} repo={repo} owns=[{}]", owns.join(", ")),
        StaffItem::RebindCancel { lane, .. } => format!("rebind {lane} cancel"),
        StaffItem::Raise {
            role,
            to,
            until,
            reason,
            ..
        } => format!("raise {role} to={to} until={until} reason={reason}"),
    }
}

/// The why of the decision line that names this task (§7.4: the `cut` entry's
/// why is the one the captain wrote, or `(no why written)`).
///
/// A whole identifier only: `T-1` does not name `T-10`.
fn why_naming(decisions: &[crate::decision::Decision], task: &str) -> Option<String> {
    decisions
        .iter()
        .filter(|decision| crate::decision::names_task(&decision.what, task))
        .find_map(|decision| decision.why.clone())
}

/// The departure is fixed when the milestone gets its first task: the time of
/// that apply (the tasks it creates are created then) and how many tasks the
/// milestone holds. Later applies update the words but never move it.
///
/// The `eta:` must read as a duration (`8h`, `1d 4h`) and nothing else: the
/// board, the stations and the forecast measure against it, so a block whose
/// eta reads as prose is refused with nothing written (T-387), for every
/// block, departed or not — such a voyage cannot depart either.
///
/// A departed voyage's `title`, `done:`, `bar:`, `eta:` and stations change
/// only when the user asked for it and confirmed it (T-410, T-331): without
/// `--confirmed` the whole apply is refused with nothing written, and with it
/// the change applies and a starred decision of kind `user` records it. A
/// voyage that had not departed before this apply states its words freely; a
/// departed voyage with no stations yet gets its first list freely too.
fn record_voyages(
    tx: &Tx<'_>,
    project: &str,
    draft: &crate::draft::Draft,
    departed: &std::collections::BTreeSet<String>,
    now: Ts,
) -> GearResult<()> {
    // The plan version this apply writes (T-458): a repeated confirmed change
    // is a decision of its own, and the version keeps its key apart.
    let version = tx.plan_version(project)?;
    for (header, body) in &draft.milestones {
        let (id, title) = crate::draft::milestone_parts(header);
        let (done, bar, eta) = crate::draft::milestone_fields(body);
        for (field, value) in [("done", &done), ("bar", &bar), ("eta", &eta)] {
            if value.as_deref().map(str::is_empty).unwrap_or(true) {
                return Err(GearError::new(
                    ErrorCode::BadFormat,
                    format!(
                        "milestone {id} needs a `{field}:` line: done, bar and eta make a voyage (T-22)"
                    ),
                ));
            }
        }
        if let Some(eta) = eta.as_deref() {
            if crate::station::eta_seconds(eta).is_none() {
                let got: String = eta.chars().take(40).collect();
                return Err(bad(
                    0,
                    format!(
                        "milestone {id}: eta must be a duration GEAR measures the voyage against, e.g. `eta: 8h` or `eta: 1d 4h`; reasons go in # Decisions; got \"{got}\""
                    ),
                ));
            }
        }
        // T-410 (T-331: title and stations too): a departed voyage's goal, bar,
        // eta and stations change only when the user asked for it and
        // confirmed it. No stored field is exempt: a voyage that departed with
        // no words gets them only with the confirmation. The one exception is
        // the first station list of a voyage that has none yet.
        if departed.contains(id) {
            let stored = tx.get_voyage(project, id)?;
            let stored = stored.as_ref();
            let stored_stations = tx.voyage_stations(project, id)?;
            let mut fields: Vec<(&str, Option<&str>, Option<&str>)> = vec![
                (
                    "done",
                    stored.and_then(|v| v.done.as_deref()),
                    done.as_deref(),
                ),
                ("bar", stored.and_then(|v| v.bar.as_deref()), bar.as_deref()),
                ("eta", stored.and_then(|v| v.eta.as_deref()), eta.as_deref()),
                ("title", stored.map(|v| v.title.as_str()), Some(title)),
            ];
            let old_stations;
            let new_stations;
            if !stored_stations.is_empty() {
                let block_stations = crate::draft::milestone_stations(body).map_err(|e| {
                    GearError::new(ErrorCode::BadFormat, format!("milestone {id}: {e}"))
                })?;
                old_stations = joined_stations(&stored_stations);
                new_stations = joined_stations(&block_stations);
                fields.push(("stations", Some(&old_stations), Some(&new_stations)));
            }
            for (field, old, new) in fields {
                let old = old.unwrap_or("").trim();
                let new = new.unwrap_or("").trim();
                if old == new {
                    continue;
                }
                if !draft.confirmed {
                    return Err(bad(
                        0,
                        format!(
                            "milestone {id}: {field} changed after departure (stored \"{old:.40}\", got \"{new:.40}\"): the goal, the bar and the eta change only when the user asks; confirm it with them, then apply again with --confirmed"
                        ),
                    ));
                }
                // T-461: the flag alone is not the record. The user's word is
                // the `confirmed:` line in the same `# Decisions`; without it
                // the change is refused like a change nobody asked for.
                if confirmed_decisions(draft).is_empty() {
                    return Err(bad(
                        0,
                        format!(
                            "milestone {id}: {field} changed after departure (stored \"{old:.40}\", got \"{new:.40}\"): --confirmed is set but # Decisions carries no `confirmed:` line: add `- <time> confirmed: <what changes, before -> after> | why: <the user's reason>` in this same apply"
                        ),
                    ));
                }
                // The user's confirmation is a starred decision (T-410).
                let what = format!("{field} of {id} changed from \"{old}\" to \"{new}\"");
                let why = "confirmed by the user".to_string();
                // T-458: the key names the plan version too. A voyage that goes
                // back and forth (8h, 10h, 8h) confirms the same transition
                // twice, and the second confirmation is a decision of its own
                // (M5: the key held only the field and the two texts, so the
                // repeat was dropped as a duplicate).
                let key = format!("user|{project}|{id}|{field}|{old}|{new}|v{version}");
                tx.insert_decision_if_new(&gear_store::NewDecision {
                    project: project.to_string(),
                    key: crate::decision::key_of(&key),
                    voyage: id.to_string(),
                    at: crate::decision::apply_time(now),
                    kind: "user".to_string(),
                    what: what.clone(),
                    why: Some(why.clone()),
                    alt: None,
                    changes: None,
                    star: true,
                    source: "gear".to_string(),
                    line: Some(format!("{what} | why: {why}")),
                    plan_version: Some(version),
                })?;
            }
        }
        // The stations the block names, in plan order (M5 S3), then the words;
        // the departure was fixed when the milestone's first tasks
        // were created (fix_departures), so a block edit never moves it.
        let stations = crate::draft::milestone_stations(body)
            .map_err(|e| GearError::new(ErrorCode::BadFormat, format!("milestone {id}: {e}")))?;
        tx.upsert_voyage(&gear_store::VoyageRow {
            project: project.to_string(),
            milestone: id.to_string(),
            title: title.to_string(),
            done,
            bar,
            eta,
            departure: None,
            departure_tasks: None,
            departure_estimate_min: None,
            updated: now,
        })?;
        tx.set_voyage_stations(project, id, &stations)?;
    }
    Ok(())
}

/// The stations of a block as one text for the confirmation guard and its
/// decision (T-331): `<id>: <needs>`, joined by `"; "`.
fn joined_stations(stations: &[gear_store::VoyageStation]) -> String {
    stations
        .iter()
        .map(|station| format!("{}: {}", station.id, station.needs))
        .collect::<Vec<String>>()
        .join("; ")
}

/// The station each task belongs to (M5 S3): a queue block's `station=<id>` is
/// written to its task, and a block without one clears the task's station (a
/// block always carries every field). A station must be one the task's
/// milestone block names, else nothing is written.
fn record_task_stations(
    tx: &Tx<'_>,
    project: &str,
    draft: &crate::draft::Draft,
    created: &[String],
) -> GearResult<()> {
    let mut new_items = 0;
    for item in &draft.queue {
        let task = match &item.id {
            Some(task) => task.clone(),
            None => {
                let at = new_items;
                new_items += 1;
                match created.get(at) {
                    Some(task) => task.clone(),
                    None => continue,
                }
            }
        };
        let Some(station) = item.station.as_deref() else {
            tx.set_task_station(project, &task, None)?;
            continue;
        };
        let milestone = match item.milestone.clone() {
            Some(milestone) => Some(milestone),
            None => tx.get_task(project, &task)?.and_then(|row| row.milestone),
        };
        let Some(milestone) = milestone else {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!("{task} names station {station} but has no milestone"),
            ));
        };
        let stations = tx.voyage_stations(project, &milestone)?;
        if !stations.iter().any(|named| named.id == station) {
            return Err(GearError::new(
                ErrorCode::BadFormat,
                format!(
                    "{task} names station {station}, but milestone {milestone} has no `station {station}:` line"
                ),
            ));
        }
        tx.set_task_station(project, &task, Some(station))?;
    }
    Ok(())
}

/// The task packet a lane reads (`gear --show T-n`, design/05 "Views"),
/// rendered from the contract version the seat's open attempt adopted (else
/// the task's current version). Plain text, in this order:
/// `# <task> — <goal>` / `Lane: <lane> · Contract: v<n> · State: <state>` /
/// `## Goal` goal / `## Acceptance` accept / `## Evidence` files and due (only
/// when set) / `## Report` with the copyable commands
/// `gear send --kind progress "…"` and
/// `gear send --kind result --task <T> --attempt <n> --contract <v> --candidate <commit> "Report: <reports_dir>/T-<n>.md"`
/// (attempt number of the open attempt; `<n>` literally when there is none;
/// the reports folder is the voyage's, T-257, and `<path>` stays for a task
/// without milestone).
/// A pending newer contract version is appended as `## Pending update: v<m>`
/// followed by that version's goal and acceptance and the line saying a bare
/// `gear` adopts it (T-378).
/// Missing task → `NoTarget`. Read-only.
pub fn render_task_packet(
    tx: &gear_store::Tx<'_>,
    project: &str,
    task: &str,
) -> GearResult<String> {
    let row = tx.get_task(project, task)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no task {task} in project {project}"),
        )
    })?;

    // The seat's open attempt for this task fixes the contract version the
    // packet renders from; without one the task's current version applies.
    let mut adopted = row.contract_version;
    let mut attempt_arg = "<n>".to_string();
    let mut open_n: Option<i64> = None;
    if let Some(lane) = &row.lane {
        let seat_id = format!("{project}:{lane}");
        if let Some(open) = tx
            .open_attempts_of_seat(&seat_id)?
            .into_iter()
            .rfind(|candidate| candidate.task == task)
        {
            adopted = open.adopted_version;
            attempt_arg = open.n.to_string();
            open_n = Some(open.n);
        }
    }
    let contract = tx.get_contract(project, task, adopted)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("task {task} has no contract v{adopted}"),
        )
    })?;

    // The rework round this packet is for (T-118): the attempt that failed
    // and the attempt that works on it now. A lane that read the first
    // round's report found nothing to fix in it and handed the same
    // candidate in again (trial B02-06) — so the packet leads with the
    // captain's rework text and the older reports become history.
    //
    // A finished task is not in a rework round any more: its packet keeps the
    // plain review history.
    let failed_round = failed_round_of(tx, project, task)?;
    let rework_n = failed_round
        .filter(|_| !row.state.is_terminal())
        .map(|failed| open_n.map_or(failed + 1, |n| n.max(failed + 1)));

    let mut text = String::new();
    // T-465: the incoming agent's packet starts with the handover of the
    // agent whose seat this one takes over, when the engine wrote one for this
    // generation and this attempt. It is part 1 of handover.md part A and
    // points at the document with the outgoing agent's own summary and notes.
    if let (Some(lane), Some(n)) = (&row.lane, open_n) {
        let seat_id = format!("{project}:{lane}");
        if let Some(seat) = tx.get_seat(&seat_id)? {
            if let Some(raw) = tx.get_meta(&format!("engine.handover.{seat_id}"))? {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if value["generation"].as_i64() == Some(seat.generation)
                        && value["task"].as_str() == Some(task)
                        && value["attempt"].as_i64() == Some(n)
                    {
                        if let Some(block) = value["block"].as_str() {
                            text.push_str(block);
                        }
                    }
                }
            }
        }
    }
    text.push_str(&format!("# {task} — {}\n", contract.goal));
    text.push_str(&format!(
        "Lane: {} · Contract: v{} · State: {}\n",
        row.lane.as_deref().unwrap_or("-"),
        adopted,
        row.state.as_str()
    ));
    // The planned time of the task (T-38): the lane knows its budget.
    if let Some(minutes) = row.estimate_min {
        text.push_str(&format!("Estimate: {minutes} minutes\n"));
    }
    // The round's budget (design §5.1) and what a hand-in past it must carry
    // (design §6.2): the friction report is then required.
    if let Some(n) = open_n {
        let role = if n >= 2 { "rework" } else { "dev" };
        let budget = tx.round_budget_min(project, task, n, role, None)?;
        text.push_str(&format!(
            "Budget: {budget} min. Past it, your hand-in must carry --friction \"…\" \
             (a permission you lacked and worked around, a test or build that took far too long, \
             an unclear contract, a missing tool or fixture, a step that cost much time).\n"
        ));
    }
    // A round that could not continue the earlier conversation because its CLI
    // changed (T-481, design/10 §resume rules): the packet points the lane at
    // the session its earlier work ran in, so it can read it if its CLI can.
    if let Some(n) = open_n {
        if let Some((cli, session)) = tx.session_recovery(project, task, n)? {
            text.push_str(&format!(
                "Earlier work on this task ran in {cli} session {session}; \
                 read it for reference if your CLI can.\n"
            ));
        }
    }
    // The project standard (T-84): the writer judges against it too.
    if let Some(line) = crate::standard::line_for(tx, project)? {
        text.push_str(&line);
        text.push('\n');
    }
    // The voyage this task sails on (T-22): the one sentence the phase is
    // judged by, its quality bar, the planned landing and where it departed.
    if let Some(milestone) = row.milestone.as_deref() {
        if let Some(voyage) = tx.get_voyage(project, milestone)? {
            text.push_str(&format!("{}\n", voyage_line(&voyage)));
        }
    }
    // The rework of this round comes before the contract (T-118): it is what
    // this hand-in is about.
    if let (Some(n), Some(failed)) = (rework_n, failed_round) {
        text.push_str(&format!(
            "\n## Rework (attempt {n}, from the review of attempt {failed})\n"
        ));
        let note = row.rework_note.as_deref().unwrap_or_default().trim();
        if note.is_empty() {
            text.push_str("(no rework note was written; the review of that attempt is below)\n");
        } else {
            text.push_str(note);
            text.push('\n');
        }
    }
    text.push_str(&format!("\n## Goal\n{}\n", contract.goal));
    text.push_str(&format!("\n## Acceptance\n{}\n", contract.accept));
    text.push_str("\n## Evidence\n");
    for file in &contract.evidence_files {
        text.push_str(&format!("{file}\n"));
    }
    if let Some(minutes) = contract.evidence_due_min {
        text.push_str(&format!("Due: {minutes} minutes\n"));
    }
    // A mechanical task (T-272): the gate alone proves the change, so no
    // reviewer looks at it — the lane knows what its hand-in rests on. The
    // engine runs the gate at the merge (T-437), so the lane does not.
    if let Some(reason) = &contract.mechanical {
        text.push_str(&format!(
            "Review: none — mechanical ({reason}): the engine runs the merge gate at the merge, \
             and it is the only check; run only the acceptance commands above, \
             never the gate yourself.\n"
        ));
    }
    text.push_str("\n## Report\n");
    text.push_str("gear send --kind progress \"…\"\n");
    // Where the report goes: the voyage's reports folder (T-257), one folder
    // per voyage, unless the task has no milestone — then any path will do.
    let report = match row.milestone.as_deref() {
        Some(milestone) => format!(
            "Report: {}/T-<n>.md",
            gear_model::layout::reports_dir(
                &crate::tasks::conventions_of(tx, project).voyage_dir,
                project,
                milestone
            )
        ),
        None => "Report: <path>".to_string(),
    };
    text.push_str(&format!(
        "gear send --kind result --task {task} --attempt {attempt_arg} --contract {adopted} --candidate <commit> \"{report}\"\n"
    ));
    // The review records of the task: what a temp reviewer already handed in
    // (T-2). The captain decides on them; they settle nothing by themselves.
    // With a rework round they are history: the rework above is what this
    // round is about (T-118).
    let reviews = tx.review_verdicts_of_task(project, task)?;
    if !reviews.is_empty() {
        if rework_n.is_some() {
            text.push_str("\n## Earlier reviews (history)\n");
        } else {
            text.push_str("\n## Review\n");
        }
        for review in &reviews {
            text.push_str(&format!(
                "attempt {} · {} · {}\n",
                review.attempt,
                if review.passed { "pass" } else { "fail" },
                review.by_seat
            ));
            text.push_str(&format!("{}\n", review.findings));
        }
    }
    if row.contract_version > adopted {
        // The bare header alone left the lane unable to read what the newer
        // contract asks for without `gear plan show` (T-378, F-133), so the
        // packet carries its wording too.
        let pending = tx
            .get_contract(project, task, row.contract_version)?
            .ok_or_else(|| {
                GearError::new(
                    ErrorCode::NoTarget,
                    format!("task {task} has no contract v{}", row.contract_version),
                )
            })?;
        text.push_str(&format!(
            "\n## Pending update: v{}\nGoal (v{}):\n{}\n\nAcceptance (v{}):\n{}\n\nRun `gear` (no arguments) to adopt it, then work and hand in to v{}.\n",
            row.contract_version, row.contract_version, pending.goal, row.contract_version, pending.accept, row.contract_version
        ));
    }
    text.push_str("\nYou implement; you never design. If anything here is unclear or contradicts the design or a message, stop and send gear send --kind question (Question / Options / Context / Default) at once, then go on with your default. Commit and push this branch at least every 15 minutes.\n");
    Ok(text)
}

/// The attempt whose round failed, if the task has one (T-118): the highest
/// attempt a review failed or a captain failed on.
///
/// A review verdict and a captain verdict are the same kind of record — the
/// findings of the round — so the highest of both is the round the task is
/// in. `None` when nothing failed yet: a first hand-in is not a rework.
fn failed_round_of(tx: &gear_store::Tx<'_>, project: &str, task: &str) -> GearResult<Option<i64>> {
    let mut failed: Option<i64> = None;
    let mut note = |attempt: i64, passed: bool| {
        if !passed {
            failed = Some(failed.map_or(attempt, |n: i64| n.max(attempt)));
        }
    };
    for review in tx.review_verdicts_of_task(project, task)? {
        note(review.attempt, review.passed);
    }
    for verdict in tx.verdicts_of_task(project, task)? {
        note(verdict.attempt, verdict.passed);
    }
    Ok(failed)
}

#[cfg(test)]
mod tail_time_tests {
    use super::parse_rfc3339;

    #[test]
    fn parses() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-26T18:00:00Z"), Some(1793037600));
        assert_eq!(
            parse_rfc3339("2026-10-26T18:00:00+01:00"),
            Some(1793037600 - 3600)
        );
        assert_eq!(parse_rfc3339("tomorrow"), None);
    }
}
