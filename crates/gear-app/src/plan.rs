//! `gear plan apply` (design/05 "Apply flow"), first cut: the `# Queue`
//! section. One transaction via `run_request`; an invalid draft writes
//! nothing. `# Staff`, `# Verdicts` and `# Review` come in later tasks and are
//! rejected here when non-empty (`BadFormat`, naming the section).

use crate::draft::{Draft, Priority, QueueItem, StaffItem};
use crate::identity::{require_seat, Fingerprint};
use crate::Env;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole, SeatState, TaskState, Ts};
use gear_store::{ContractRow, PlanRow, RequestOutcome, Store, TaskRow, Tx};
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
}

/// Apply `draft` (parsed from `text`, which is stored verbatim as the plan row).
///
/// 1. identity: captain seat (`require_seat` with role Captain, states Active);
/// 2. `draft.project` = the caller's project, else `BadFormat`;
/// 3. `draft.base` ≠ current `plan_version` → `StaleVersion`;
/// 4. non-empty Staff/Review → `BadFormat` (not in this cut); `# Verdicts` (T-50):
///    after the queue writes, each line → `tasks::settle` (by_seat = the caller's
///    seat) in draft order, in the same transaction — any error aborts the whole
///    apply; settled lines are listed in `settled`;
/// 5. validation, nothing written on failure (`BadFormat`, detail names the task/line):
///    - an existing id must exist; a `NEW` block needs `lane`, `goal` and `accept`;
///    - every dep names an existing task or a task of this draft; no cycle;
///    - `state=` only on existing tasks: `cancelled` from any non-terminal state,
///      `ready` only from Draft (DONE→READY with `reopen` comes with verdicts);
/// 6. writes: `NEW` → `T-<n>` via `next_id`, state Ready, contract v1; an existing
///    task whose goal/accept/evidence differs from its current contract → new
///    contract version; rank = position in the draft (1, 2, …), unmentioned
///    non-terminal tasks keep their relative order after them; lane, milestone,
///    required, priority and deps as written; `insert_plan(version = base + 1)`;
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
                        if task.state.is_terminal() {
                            return Err(bad(
                                item.line,
                                format!("task {id} is already {}", task.state.as_str()),
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
                match item.state.as_deref() {
                    Some("cancelled") => task.state = TaskState::Cancelled,
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

    let version = current + 1;
    tx.insert_plan(&PlanRow {
        project: project.to_string(),
        version,
        draft: text.to_string(),
        applied: now,
        by_seat: Some(by_seat.to_string()),
        by_gen: Some(by_gen),
    })?;

    Ok(ApplyResult { version, created, contract_changed, not_mentioned, settled: Vec::new() })
}

fn bad(line: usize, detail: impl std::fmt::Display) -> GearError {
    GearError::new(ErrorCode::BadFormat, format!("line {line}: {detail}"))
}

/// Staff, verdicts and reviews belong to later cuts; a draft that carries them
/// is rejected whole.
fn reject_sections(draft: &Draft) -> GearResult<()> {
    if let Some(item) = draft.staff.first() {
        let line = match item {
            StaffItem::Temp { line, .. }
            | StaffItem::Park { line, .. }
            | StaffItem::Hold { line, .. }
            | StaffItem::Retire { line, .. } => *line,
        };
        return Err(bad(line, "the `# Staff` section is not supported in this cut"));
    }
    if let Some(item) = draft.verdicts.first() {
        let line = item.line;
        return Err(bad(line, "the `# Verdicts` section is not supported in this cut"));
    }
    if let Some(item) = draft.reviews.first() {
        let line = item.line;
        return Err(bad(line, "the `# Review` section is not supported in this cut"));
    }
    Ok(())
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
            &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\nevidence-due: 60\ndeps: -\n"),
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
            &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\nevidence-files: notes/x.md\ndeps: -\n"),
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
        run(&mut s, &draft_text(0, "## NEW [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n"), "r1").unwrap();

        let text = "<!-- gear-plan project=p base=0 -->\n# Staff\n- retire heat\n";
        let draft = parse(text).unwrap();
        let (env, fp) = cap();
        let err = apply(&mut s, &env, Some(&fp), &draft, text, "r2", NOW).unwrap_err();
        assert_eq!(err.code, ErrorCode::StaleVersion, "step 3 is checked before step 4");
    }
}
