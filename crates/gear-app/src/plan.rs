//! `gear plan apply` (design/05 "Apply flow"), first cut: the `# Queue`
//! section. One transaction via `run_request`; an invalid draft writes
//! nothing. `# Verdicts` (T-50) and `# Staff` park/hold/retire (T-55) are applied here; `temp` staff lines and `# Review` come in later tasks and are
//! rejected here when non-empty (`BadFormat`, naming the section).

use crate::draft::{Draft, Priority, QueueItem, StaffItem};
use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::state::require_transition;
use gear_model::{AttemptState, ErrorCode, GearError, GearResult, MsgKind, MsgState, SeatRole, SeatState, TaskState, Ts};
use gear_store::{ContractRow, NewMessage, PlanRow, RequestOutcome, Store, TaskRow, Tx};
use std::collections::{BTreeMap, BTreeSet};

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
///    `# Verdicts` (T-50):
///    after the queue writes, each line → `tasks::settle` (by_seat = the caller's
///    seat) in draft order, in the same transaction — any error aborts the whole
///    apply; settled lines are listed in `settled`;
/// 5. validation, nothing written on failure (`BadFormat`, detail names the task/line):
///    - an existing id must exist; a `NEW` block needs `lane`, `goal`, `accept`
///      and `estimate` (the planned time in minutes, T-38);
///    - every dep names an existing task or a task of this draft; no cycle;
///    - `state=` only on existing tasks: `cancelled` from any non-terminal state,
///      `ready` only from Draft (a Done task reopens by a `# Verdicts` fail,
///      T-90, not by a queue line);
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
    draft: &Draft,
    text: &str,
    request_id: &str,
    now: Ts,
) -> GearResult<ApplyResult> {
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
            let result = apply_queue(tx, &ctx.project, &seat.id, ctx.generation, draft, text, now)?;
            // The milestone blocks of the draft become voyages (T-22), after the
            // queue so the task count at departure is the plan's.
            record_tail_decisions(tx, &ctx.project, draft)?;
            fix_departures(tx, &ctx.project, now)?;
            record_voyages(tx, &ctx.project, draft, now)?;
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

    // 5. validation, nothing written on failure.
    let mut graph: BTreeMap<String, Vec<String>> = existing
        .iter()
        .map(|(id, task)| (id.clone(), task.deps.clone()))
        .collect();
    for (index, item) in draft.queue.iter().enumerate() {
        match &item.id {
            Some(id) => {
                let task = existing.get(id).ok_or_else(|| {
                    bad(item.line, format!("task {id} does not exist"))
                })?;
                match item.state.as_deref() {
                    None => {}
                    Some("cancelled") => {
                        // Cancelling a cancelled task is a no-op, not an error
                        // (T-93: a plan keeps saying so after the cancel, and
                        // re-applying it failed three times in M3). A finished
                        // task is not cancelled after the fact.
                        if matches!(task.state, TaskState::Done | TaskState::Failed) {
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
        for dep in &item.deps {
            if !known.contains(dep) {
                return Err(bad(
                    item.line,
                    format!("dep {dep} does not exist and is not created by this draft"),
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

    // 6. writes.
    let mut created: Vec<String> = Vec::new();
    let mut contract_changed: Vec<(String, i64)> = Vec::new();
    let mut mentioned: BTreeSet<String> = BTreeSet::new();
    for (position, item) in draft.queue.iter().enumerate() {
        let rank = position as i64 + 1;
        match &item.id {
            None => {
                let id = new_ids
                    .get(&position)
                    .cloned()
                    .ok_or_else(|| bad(item.line, "cannot assign an id to this NEW block"))?;
                let task = TaskRow {
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
                    updated: now,
                };
                tx.insert_task(&task)?;
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
                task.lane = item.lane.clone();
                task.milestone = item.milestone.clone();
                task.required = item.required;
                task.deps = item.deps.clone();
                // The planned time (T-38): the draft may change it; a draft
                // that says nothing keeps the estimate the task has.
                if let Some(estimate) = item.estimate {
                    task.estimate_min = Some(i64::from(estimate));
                }
                match item.state.as_deref() {
                    Some("cancelled") => {
                        // T-93: cancelling a task finishes it. Its open attempt
                        // is abandoned — the seat is free for the next task —
                        // and the lane is told in one line. A cancelled task
                        // that kept a started attempt left the lane looking
                        // busy with a full queue behind it (M3: T-80).
                        if task.state != TaskState::Cancelled {
                            cancel_task(tx, project, &task, by_seat, by_gen, now)?;
                        }
                        task.state = TaskState::Cancelled;
                    }
                    Some("ready") => task.state = TaskState::Ready,
                    _ => {}
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
                    if goal_changed || accept_changed || files_changed || due_changed {
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
                        };
                        tx.insert_contract(&next)?;
                        task.contract_version = version;
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

    // Staff (T-55): park/hold/retire move a lane seat of this project in this
    // transaction; a target already reached is skipped.
    let mut staffed: Vec<String> = Vec::new();
    for item in &draft.staff {
        let (lane, verb, target, reason) = match item {
            StaffItem::Park { lane, until, until_task, .. } => {
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
                return Err(bad(staff_line(item), "`temp` staff lines are not in this cut"))
            }
        };
        let seat_id = format!("{project}:{lane}");
        let seat = tx.get_seat(&seat_id)?.ok_or_else(|| {
            bad(staff_line(item), format!("no seat {seat_id} in this project"))
        })?;
        if seat.role != SeatRole::Lane {
            return Err(bad(
                staff_line(item),
                format!("seat {seat_id} is a {} seat, staff lines name lanes", seat.role.as_str()),
            ));
        }
        if seat.state == target {
            // Staff is a target, not a command: already there, nothing to do.
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
    }

    let version = current + 1;
    // Settle the verdicts, after the queue writes and before the plan row:
    // an error aborts the whole apply, so nothing of it is kept.
    let mut settled: Vec<String> = Vec::new();
    for line in &draft.verdicts {
        let verdict = crate::tasks::Verdict {
            project: project.to_string(),
            task: line.task.clone(),
            attempt: line.attempt,
            contract_version: line.contract,
            candidate: line.candidate.clone(),
            pass: line.pass,
            evidence: line.evidence.clone(),
            rework: line.rework.clone(),
            by_seat: by_seat.to_string(),
        };
        crate::tasks::settle(tx, &verdict, now)?;
        settled.push(format!(
            "{} attempt {} {}",
            line.task,
            line.attempt,
            if line.pass { "pass" } else { "fail" }
        ));
    }

    tx.insert_plan(&PlanRow {
        project: project.to_string(),
        version,
        draft: text.to_string(),
        applied: now,
        by_seat: Some(by_seat.to_string()),
        by_gen: Some(by_gen),
    })?;

    Ok(ApplyResult { version, created, contract_changed, not_mentioned, settled, staffed })
}

fn bad(line: usize, detail: impl std::fmt::Display) -> GearError {
    GearError::new(ErrorCode::BadFormat, format!("line {line}: {detail}"))
}

/// Staff and reviews belong to later cuts; a draft that carries them
/// is rejected whole.
fn reject_sections(draft: &Draft) -> GearResult<()> {
    // Staff targets park/hold/retire are applied (T-55); temp workers are not
    // in this cut yet.
    if let Some(item) = draft.staff.iter().find(|item| matches!(item, StaffItem::Temp { .. })) {
        return Err(bad(staff_line(item), "`temp` staff lines are not in this cut"));
    }
    if let Some(item) = draft.reviews.first() {
        let line = item.line;
        return Err(bad(line, "the `# Review` section is not supported in this cut"));
    }
    Ok(())
}

/// The draft line of a staff item.
fn staff_line(item: &StaffItem) -> usize {
    match item {
        StaffItem::Temp { line, .. }
        | StaffItem::Park { line, .. }
        | StaffItem::Hold { line, .. }
        | StaffItem::Retire { line, .. } => *line,
    }
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
        body.push_str(&format!("evidence-files: {}\n", item.evidence_files.join(", ")));
    }
    if let Some(minutes) = item.evidence_due {
        body.push_str(&format!("evidence-due: {minutes}\n"));
    }
    body
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
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
            Ok(())
        })
        .unwrap();
        s
    }

    fn cap() -> (Env, Fingerprint) {
        (
            Env {
                ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
                pane_id: Some(CAP_PANE.into()),
                admin: false,
            },
            Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
        )
    }

    fn draft_text(base: i64, queue: &str) -> String {
        format!("<!-- gear-plan project=p base={base} -->\n# Queue\n{queue}")
    }

    fn run(s: &mut Store, text: &str, rid: &str) -> GearResult<ApplyResult> {
        let draft = parse(text).unwrap();
        let (env, fp) = cap();
        apply(s, &env, Some(&fp), &draft, text, rid, NOW)
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
            &draft_text(1, "## T-1 [must] lane=heat\ngoal: A\naccept: a\nevidence-due: 90\ndeps: -\n"),
            "r2",
        )
        .unwrap();

        assert_eq!(r.contract_changed, vec![("T-1".to_string(), 2)]);
        let contract = s.read(|tx| tx.get_contract("p", "T-1", 2)).unwrap().unwrap();
        assert_eq!(contract.evidence_due_min, Some(90));
        assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().contract_version, 2);
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
            &draft_text(1, "## T-1 [must] lane=heat\ngoal: A\naccept: a\nevidence-files:\ndeps: -\n"),
            "r2",
        )
        .unwrap();

        assert_eq!(r.contract_changed, vec![("T-1".to_string(), 2)]);
        let contract = s.read(|tx| tx.get_contract("p", "T-1", 2)).unwrap().unwrap();
        assert!(contract.evidence_files.is_empty(), "the files were cleared");
    }

    #[test]
    fn a_stale_base_is_reported_before_an_unsupported_section() {
        let mut s = world();
        run(&mut s, &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n"), "r1").unwrap();

        let text = "<!-- gear-plan project=p base=0 -->\n# Staff\n- temp R-4 task=T-1 profile=opencode-review\n";
        let draft = parse(text).unwrap();
        let (env, fp) = cap();
        let err = apply(&mut s, &env, Some(&fp), &draft, text, "r2", NOW).unwrap_err();
        assert_eq!(err.code, ErrorCode::StaleVersion, "step 3 is checked before step 4");
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
            "Voyage {} {} · not described yet (no `# Milestone {}` block)",
            voyage.milestone, voyage.title, voyage.milestone
        ),
    };
    match (voyage.departure, voyage.departure_tasks) {
        (Some(departure), Some(tasks)) => {
            line.push_str(&format!(" · departed {} with {tasks} tasks", utc_date(departure)))
        }
        _ => line.push_str(" · not departed yet"),
    }
    line
}

/// `YYYY-MM-DD` for unix seconds (civil-from-days, no extra crate) — enough to
/// date a departure without pulling a time library in.
fn utc_date(ts: i64) -> String {
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
pub fn lane_in_tail(tx: &Tx<'_>, project: &str, lane: &str) -> GearResult<bool> {
    Ok(tx.get_meta(&format!("lane-tail:{project}:{lane}"))?.as_deref() == Some("1"))
}

/// A lane that is about to stand idle after an apply (T-58, ops/lessons.md 7a).
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

/// The lanes that are **about to** stand idle after an apply (T-58, ops/lessons.md
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
pub fn idle_lanes(tx: &Tx<'_>, project: &str) -> GearResult<Vec<IdleLane>> {
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
        // The captain's tail: these lanes may stand idle (design/05).
        if lane_in_tail(tx, project, &seat.name)? {
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
            .or_else(|| lane_tasks.iter().find(|task| task.state == TaskState::Waiting))
            .or_else(|| lane_tasks.iter().find(|task| task.state == TaskState::Verifying))
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
fn record_tail_decisions(tx: &Tx<'_>, project: &str, draft: &Draft) -> GearResult<()> {
    for decision in &draft.decisions {
        let Some(names) = decision.strip_prefix("tail:") else { continue; };
        let names = names.trim();
        let lanes: BTreeSet<_> = names.split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty()).collect();
        if lanes.is_empty() {
            return Err(bad(1, "tail: needs lane names, or - to clear"));
        }
        let seats = tx.seats_of_project(project)?;
        for name in &lanes {
            if *name != "-" && !seats.iter().any(|s| s.role == SeatRole::Lane && s.name == *name) {
                return Err(bad(1, format!("tail: unknown lane {name}")));
            }
        }
        if lanes.contains("-") && lanes.len() != 1 {
            return Err(bad(1, "tail: - cannot be combined with lane names"));
        }
        for seat in seats.iter().filter(|s| s.role == SeatRole::Lane) {
            tx.set_meta(&format!("lane-tail:{project}:{}", seat.name),
                if lanes.contains(seat.name.as_str()) { "1" } else { "0" })?;
        }
    }
    Ok(())
}

/// Fix the departure of every milestone that has tasks and no departure yet:
/// the moment its first tasks were created. This runs on **every** apply, not
/// only when the draft repeats the milestone block, so a queue-only apply that
/// creates the first tasks departs then (review R1) and a later block edit
/// cannot move it (R2). A milestone whose tasks predate the voyage record takes
/// its departure from the oldest plan draft that already carried them.
fn fix_departures(tx: &Tx<'_>, project: &str, now: Ts) -> GearResult<()> {
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for task in tx.tasks_of_project(project)? {
        if let Some(milestone) = task.milestone {
            *counts.entry(milestone).or_insert(0) += 1;
        }
    }
    for (milestone, tasks) in counts {
        let voyage = tx.get_voyage(project, &milestone)?;
        if voyage.as_ref().is_some_and(|v| v.departure.is_some() && v.departure_estimate_min.is_some()) {
            continue;
        }
        let history = departure_from_history(tx, project, &milestone)?;
        let already_departed = voyage.as_ref().is_some_and(|v| v.departure.is_some());
        let (departure, at_departure, estimate) = match history {
            Some(found) => found,
            None if !already_departed => {
                let estimates = tx.tasks_of_project(project)?.iter()
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

/// Where a milestone that predates the voyage record departed from: the oldest
/// plan version whose draft already had tasks on it, with how many. The plan's
/// stored drafts are the persisted history (tasks carry no creation time).
fn departure_from_history(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
) -> GearResult<Option<(Ts, i64, Option<i64>)>> {
    let newest = tx.plan_version(project)?;
    for version in 1..=newest {
        let Some(plan) = tx.get_plan(project, version)? else {
            continue;
        };
        let Ok(draft) = crate::draft::parse(&plan.draft) else {
            continue;
        };
        let items: Vec<_> = draft.queue.iter()
            .filter(|item| item.milestone.as_deref() == Some(milestone)).collect();
        if !items.is_empty() {
            let estimate = items.iter().map(|item| {
                item.estimate.map(i64::from).or_else(|| item.goal.as_deref()
                    .and_then(gear_board::voyage::estimate_from_goal))
            }).sum();
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
    let mut closed = false;
    for mut attempt in tx.attempts_of_task(project, &task.id)? {
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
                        task.id,
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
    // The lane that was working on it hears why, in one line.
    let Some(lane) = task.lane.as_deref() else {
        return Ok(());
    };
    let lane_seat = format!("{project}:{lane}");
    if closed && tx.get_seat(&lane_seat)?.is_some() {
        tx.insert_message(&NewMessage {
            project: project.to_string(),
            thread: format!("{}:cancel", task.id),
            task: Some(task.id.clone()),
            attempt: None,
            sender: by_seat.to_string(),
            sender_gen: by_gen,
            recipient: lane_seat,
            kind: MsgKind::Instruction,
            urgent: false,
            body: format!("{} is cancelled; stop work on it, nothing to hand in", task.id),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })?;
    }
    Ok(())
}

/// The departure is fixed when the milestone gets its first task: the time of
/// that apply (the tasks it creates are created then) and how many tasks the
/// milestone holds. Later applies update the words but never move it.
fn record_voyages(tx: &Tx<'_>, project: &str, draft: &crate::draft::Draft, now: Ts) -> GearResult<()> {
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
        // The words; the departure was fixed when the milestone's first tasks
        // were created (fix_departures), so a block edit never moves it.
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
/// `gear send --kind result --task <T> --attempt <n> --contract <v> --candidate <commit> "Report: <path>"`
/// (attempt number of the open attempt; `<n>` literally when there is none).
/// A pending newer contract version is appended as `## Pending update: v<m>`.
/// Missing task → `NoTarget`. Read-only.
pub fn render_task_packet(tx: &gear_store::Tx<'_>, project: &str, task: &str) -> GearResult<String> {
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
    if let Some(lane) = &row.lane {
        let seat_id = format!("{project}:{lane}");
        if let Some(open) = tx
            .open_attempts_of_seat(&seat_id)?
            .into_iter()
            .filter(|candidate| candidate.task == task)
            .last()
        {
            adopted = open.adopted_version;
            attempt_arg = open.n.to_string();
        }
    }
    let contract = tx.get_contract(project, task, adopted)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("task {task} has no contract v{adopted}"),
        )
    })?;

    let mut text = String::new();
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
    // The voyage this task sails on (T-22): the one sentence the phase is
    // judged by, its quality bar, the planned landing and where it departed.
    if let Some(milestone) = row.milestone.as_deref() {
        if let Some(voyage) = tx.get_voyage(project, milestone)? {
            text.push_str(&format!("{}\n", voyage_line(&voyage)));
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
    text.push_str("\n## Report\n");
    text.push_str("gear send --kind progress \"…\"\n");
    text.push_str(&format!(
        "gear send --kind result --task {task} --attempt {attempt_arg} --contract {adopted} --candidate <commit> \"Report: <path>\"\n"
    ));
    // The review records of the task: what a temp reviewer already handed in
    // (T-2). The captain decides on them; they settle nothing by themselves.
    let reviews = tx.review_verdicts_of_task(project, task)?;
    if !reviews.is_empty() {
        text.push_str("\n## Review\n");
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
        text.push_str(&format!("\n## Pending update: v{}\n", row.contract_version));
    }
    Ok(text)
}
