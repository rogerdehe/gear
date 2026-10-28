//! The GitHub side of a project, driven from the store (T-91).
//!
//! `sync` is the one thing the engine calls once per tick (one line in
//! `gear-engine/src/lib.rs`). It reads what the store already knows — a task's
//! first delivery, its hand-in, its verdict, its merge — and makes the forge
//! match:
//!
//! - a task's first delivery opens the draft claim pull request for its branch;
//! - a hand-in comments a COMPLETION;
//! - a verdict comments the verdict and the findings;
//! - a merge comments the merge commit (`merge = "push"`) or merges the pull
//!   request itself (`merge = "pr"`), and then closes the issue.
//!
//! Every call is an op (`OpKind::Forge`): intended, then confirmed or failed
//! with its reason, at most [`MAX_ATTEMPTS`] times per call. An op the engine
//! found unfinished is an unknown outcome and is never repeated; a failed one
//! is retried on the next tick. A forge outage therefore queues the work and
//! never stops the project's local work: the store is the truth, the forge
//! catches up.
//!
//! A project without `[github]` is local-only: nothing is read, nothing is
//! called.

use std::collections::BTreeMap;

use gear_model::{GearResult, OpKind, OpState, Ts};
use gear_store::{NewOp, OpRow, Store, Tx};
use serde::Deserialize;

use crate::marker;
use crate::{Forge, ForgeResult, MergeMethod, PrRef};

/// The most forge calls one project's sync makes in one tick (T-91): a backlog
/// catches up over the next ticks instead of holding the tick for every call's
/// timeout. A call never stops being retried — the tick's pace is the wait.
pub const MAX_CALLS_PER_TICK: usize = 8;

/// What one call did this tick.
enum Outcome {
    /// It ran now, with its result.
    Done(String),
    /// It was done on an earlier tick: nothing happened, and nothing is
    /// reported again.
    Already,
    /// It waits for a later tick: it failed now, an earlier attempt left an
    /// unknown outcome, or this tick's budget is spent. A waiting call stops
    /// the task's later calls, so the order a reader sees cannot invert.
    Pending,
}

/// The state an op the engine did not finish is left in: the call may have
/// happened, so it is never repeated.
const RESTARTED: &str =
    "the engine stopped before the answer; the call may have happened, so it is not repeated";

/// One `gh` call that ran past its bound: the forge may have taken it.
const TIMED_OUT: &str = "the call ran past its bound; the forge may have taken it";

/// The forge of a project in GitHub mode, for one tick.
pub fn sync(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<String>> {
    let forge = crate::GitHub::new();
    sync_with(store, project, &forge, now)
}

/// [`sync`] with the forge given, so a test drives a fake (T-91).
pub fn sync_with(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    now: Ts,
) -> GearResult<Vec<String>> {
    let facts = store.read(|tx| facts(tx, project))?;
    if facts.is_empty() {
        return Ok(Vec::new());
    }
    let seat = forge_seat(project);
    // The forge ops so far, by their key: the record of what has been called.
    let mut ops: BTreeMap<String, Vec<OpRow>> = BTreeMap::new();
    // The land ops the engine inserted (design §11.2 step 4): the sync runs
    // them, because the engine only knows the head the gate checked.
    let mut land_ops: Vec<OpRow> = Vec::new();
    for op in store.read(|tx| tx.ops_of_seat_kind(&seat, OpKind::Forge))? {
        if op.payload.get("forge").and_then(|forge| forge.as_str()) == Some("land") {
            land_ops.push(op.clone());
        }
        ops.entry(key_of(&op)).or_default().push(op);
    }
    let mut done = Vec::new();
    let mut budget = MAX_CALLS_PER_TICK;
    for fact in &facts {
        done.extend(run_task(
            store,
            project,
            forge,
            fact,
            &mut ops,
            &land_ops,
            &mut budget,
            now,
        )?);
    }
    Ok(done)
}

/// The pseudo-seat a project's forge ops belong to: the engine acts for the
/// project, not for a seat.
fn forge_seat(project: &str) -> String {
    format!("{project}:forge")
}

/// The key of one forge call, from the op's payload.
fn key_of(op: &OpRow) -> String {
    op.payload
        .get("key")
        .and_then(|key| key.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One task of a GitHub-mode repo, with what its forge calls need.
struct Fact {
    task: String,
    lane: String,
    /// The repo the task's lane works in: `owner/name` on GitHub.
    github: Github,
    integration: String,
    /// The project's task-branch template (T-252), `None` = GEAR's own
    /// `{lane}/t{n}`.
    task_branch: Option<String>,
    issue: u64,
    goal: String,
    attempts: Vec<AttemptFact>,
    /// The engine's own merge line ("merged T-61 into dev: a1b2c3d"), when the
    /// merge has confirmed.
    merge_line: Option<String>,
}

struct AttemptFact {
    n: i64,
    delivered: Option<Ts>,
    handed_in: Option<Ts>,
    /// (passed, findings, the seat that handed it in)
    verdict: Option<(bool, String, String)>,
    merged: Option<Ts>,
    candidate: Option<String>,
}

/// One forge call to make.
struct Call {
    key: String,
    task: String,
    attempt: Option<i64>,
    what: What,
}

enum What {
    Claim {
        repo: String,
        branch: String,
        base: String,
        title: String,
        body: String,
    },
    Comment {
        pr: PrRef,
        body: String,
    },
    Close {
        repo: String,
        issue: u64,
        comment: String,
    },
}

impl What {
    fn name(&self) -> &'static str {
        match self {
            What::Claim { .. } => "claim",
            What::Comment { .. } => "comment",
            What::Close { .. } => "close",
        }
    }
}

/// One task's calls: the claim first, then what the store says happened.
fn run_task(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    fact: &Fact,
    ops: &mut BTreeMap<String, Vec<OpRow>>,
    land_ops: &[OpRow],
    budget: &mut usize,
    now: Ts,
) -> GearResult<Vec<String>> {
    let mut done = Vec::new();
    // 1. The claim: the pull request every later call names. It is opened at the
    //    task's first delivery, not before, and it keeps being tried while the
    //    branch is not there yet (T-91, F3).
    let mut pr = claim_of(fact, ops);
    // A claim whose outcome is unknown (T-91 F4): the pull request may be
    // there after all — the branch names it.
    if pr.is_none() {
        if let Some(op) = unknown_claim_of(fact, ops) {
            pr = adopt_claim(store, project, forge, fact, &op, budget, now)?;
        }
    }
    let delivered = fact
        .attempts
        .first()
        .is_some_and(|attempt| attempt.delivered.is_some());
    if pr.is_none() && delivered {
        let call = claim_call(project, fact);
        match run_call(store, project, forge, &call, ops, budget, now)? {
            Outcome::Done(result) => {
                pr = pr_of(&result);
                done.push(format!("{project}:{}:claim", fact.task));
            }
            // Without a pull request there is nothing the later calls can name.
            Outcome::Already | Outcome::Pending => return Ok(done),
        }
    }
    let Some(pr) = pr else { return Ok(done) };
    // 1a. The land op the engine inserted (design §11.2 step 4): the pull
    //     request is merged here, with the head the gate checked.
    if let Some(land) = land_op_of(fact, land_ops, &pr) {
        match run_land(store, project, forge, &land, budget, now)? {
            Outcome::Done(_) => done.push(format!(
                "{project}:{}:{}:land",
                fact.task,
                land.op.attempt.unwrap_or_default()
            )),
            Outcome::Already => {}
            Outcome::Pending => return Ok(done),
        }
    }
    // 2. The rest of the task's life, against that pull request. A call that
    //    waits holds back every later one: a close never runs before its merge,
    //    and a verdict never overtakes a hand-in (T-91, F2).
    for call in calls_after_claim(project, fact, &pr) {
        match run_call(store, project, forge, &call, ops, budget, now)? {
            Outcome::Done(_) => done.push(format!("{project}:{}:{}", fact.task, call.key)),
            Outcome::Already => {}
            Outcome::Pending => return Ok(done),
        }
    }
    Ok(done)
}

/// The pull request a confirmed claim opened, if there is one.
fn claim_of(fact: &Fact, ops: &BTreeMap<String, Vec<OpRow>>) -> Option<PrRef> {
    let key = format!("{}:claim", fact.task);
    ops.get(&key)?
        .iter()
        .find(|op| op.state == OpState::Confirmed)
        .and_then(|op| op.result.as_deref())
        .and_then(pr_of)
}

/// A claim whose outcome is unknown (T-91 F4): the pull request may be there
/// after all.
fn unknown_claim_of(fact: &Fact, ops: &BTreeMap<String, Vec<OpRow>>) -> Option<OpRow> {
    ops.get(&format!("{}:claim", fact.task))?
        .iter()
        .rev()
        .find(|op| op.state == OpState::Unknown)
        .cloned()
}

/// The pull request inside a claim's result.
fn pr_of(result: &str) -> Option<PrRef> {
    let value: serde_json::Value = serde_json::from_str(result).ok()?;
    Some(PrRef {
        repo: value.get("repo")?.as_str()?.to_string(),
        number: value.get("number")?.as_u64()?,
        url: value.get("url")?.as_str()?.to_string(),
    })
}

/// Adopt the pull request of an interrupted claim (T-91 F4): the branch says
/// which one it is, so the claim is confirmed by it.
fn adopt_claim(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    fact: &Fact,
    op: &OpRow,
    budget: &mut usize,
    now: Ts,
) -> GearResult<Option<PrRef>> {
    if *budget == 0 {
        // This tick's calls are spent: the adoption waits for the next one.
        return Ok(None);
    }
    *budget -= 1;
    let branch = branch_of(fact.task_branch.as_deref(), project, &fact.lane, &fact.task);
    // A forge that cannot answer keeps the claim unknown: the next tick
    // tries again.
    let pr = match forge.pr_by_head(&fact.github.repo, &branch) {
        Ok(Some(pr)) => pr,
        Ok(None) => return Ok(None),
        Err(_) => return Ok(None),
    };
    let result =
        serde_json::json!({ "repo": pr.repo, "number": pr.number, "url": pr.url }).to_string();
    store.write(|tx| tx.set_op_state(project, &op.id, OpState::Confirmed, Some(&result), now))?;
    Ok(Some(pr))
}

/// The land op of a task, with what it says (design §11.2 step 4): the pull
/// request to merge, the head the gate checked and the method.
struct LandOp {
    op: OpRow,
    pr: PrRef,
    head: String,
    method: MergeMethod,
}

/// The land op of a task, if the engine inserted one.
fn land_op_of(fact: &Fact, land_ops: &[OpRow], pr: &PrRef) -> Option<LandOp> {
    land_ops
        .iter()
        .filter(|op| op.task.as_deref() == Some(fact.task.as_str()))
        .max_by_key(|op| op.attempt.unwrap_or_default())
        .and_then(|op| {
            let head = op.payload.get("head")?.as_str()?.trim().to_string();
            if head.is_empty() {
                return None;
            }
            let method = match op.payload.get("method").and_then(|method| method.as_str()) {
                Some("merge") => MergeMethod::Merge,
                _ => MergeMethod::Squash,
            };
            // The engine names the pull request it merged; the claim's is the
            // same one.
            let number = op
                .payload
                .get("pr")
                .and_then(|number| number.as_u64())
                .unwrap_or(pr.number);
            Some(LandOp {
                op: op.clone(),
                pr: PrRef {
                    // The confirmed claim's pull request: its repo and its
                    // number are the ones the claim names (F4).
                    repo: pr.repo.clone(),
                    number,
                    url: pr.url.clone(),
                },
                head,
                method,
            })
        })
}

/// Run the land op, or reconcile one whose outcome is unknown (design §11.2
/// step 5, §11.3).
fn run_land(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    land: &LandOp,
    budget: &mut usize,
    now: Ts,
) -> GearResult<Outcome> {
    match land.op.state {
        // The merge happened: nothing to do.
        OpState::Confirmed => return Ok(Outcome::Already),
        // It failed: the captain hears about it (the engine's item).
        OpState::Failed => return Ok(Outcome::Pending),
        // The call may have happened: the pull request says what did.
        OpState::Unknown | OpState::Executing | OpState::Submitted => {
            return reconcile_land(store, project, forge, land, budget, now);
        }
        _ => {}
    }
    merge_and_record(store, project, forge, land, budget, now)
}

/// Reconcile a land op whose outcome is unknown (design §11.3): the pull
/// request's own state says what happened.
fn reconcile_land(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    land: &LandOp,
    budget: &mut usize,
    now: Ts,
) -> GearResult<Outcome> {
    if *budget == 0 {
        // This tick's calls are spent: the reconciliation waits for the next
        // one.
        return Ok(Outcome::Pending);
    }
    *budget -= 1;
    // A forge that cannot answer keeps the op as it is: the next tick
    // reconciles it again.
    let Ok(state) = forge.pr_state(&land.pr) else {
        return Ok(Outcome::Pending);
    };
    if let Some(result) = merged_result(&state) {
        store.write(|tx| {
            tx.set_op_state(project, &land.op.id, OpState::Confirmed, Some(&result), now)
        })?;
        return Ok(Outcome::Done(result));
    }
    if state.is_open() && state.head_ref_oid.as_deref() == Some(land.head.as_str()) {
        // The merge did not happen: run it again.
        return merge_and_record(store, project, forge, land, budget, now);
    }
    // Any other state: the op fails and the captain hears about it.
    let reason = if state.state.is_empty() {
        "the pull request is in another state".to_string()
    } else {
        format!("the pull request is {}", state.state)
    };
    store.write(|tx| tx.set_op_state(project, &land.op.id, OpState::Failed, Some(&reason), now))?;
    Ok(Outcome::Pending)
}

/// Merge the pull request with the gated head and write what the op says.
fn merge_and_record(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    land: &LandOp,
    budget: &mut usize,
    now: Ts,
) -> GearResult<Outcome> {
    if *budget == 0 {
        // This tick's calls are spent: the merge waits for the next one.
        return Ok(Outcome::Pending);
    }
    *budget -= 1;
    // The op is Executing before the call: an engine that stops here leaves
    // an op whose outcome is unknown, never one that looks untouched (F3).
    store.write(|tx| {
        tx.set_op_state(
            project,
            &land.op.id,
            OpState::Executing,
            Some("the merge runs"),
            now,
        )
    })?;
    match forge.merge(&land.pr, land.method, &land.head) {
        Ok(_) => {
            // The merge commit is read from the pull request (§11.2 step 5).
            // A pull request that is not merged yet leaves the op unknown: the
            // next tick reconciles it by the pull request's own state (F2).
            if *budget == 0 {
                store.write(|tx| {
                    tx.set_op_state(
                        project,
                        &land.op.id,
                        OpState::Unknown,
                        Some("the merge ran; its commit is not read yet"),
                        now,
                    )
                })?;
                return Ok(Outcome::Pending);
            }
            *budget -= 1;
            let Ok(state) = forge.pr_state(&land.pr) else {
                store.write(|tx| {
                    tx.set_op_state(
                        project,
                        &land.op.id,
                        OpState::Unknown,
                        Some("the merge ran; its commit is not read yet"),
                        now,
                    )
                })?;
                return Ok(Outcome::Pending);
            };
            let Some(result) = merged_result(&state) else {
                store.write(|tx| {
                    tx.set_op_state(
                        project,
                        &land.op.id,
                        OpState::Unknown,
                        Some("the merge was accepted; the pull request is not merged yet"),
                        now,
                    )
                })?;
                return Ok(Outcome::Pending);
            };
            store.write(|tx| {
                tx.set_op_state(project, &land.op.id, OpState::Confirmed, Some(&result), now)
            })?;
            Ok(Outcome::Done(result))
        }
        Err(crate::Error::Timeout(_)) => {
            // The forge may have taken it: the next tick reconciles.
            store.write(|tx| {
                tx.set_op_state(project, &land.op.id, OpState::Unknown, Some(TIMED_OUT), now)
            })?;
            Ok(Outcome::Pending)
        }
        Err(why) => {
            let reason = why.to_string();
            store.write(|tx| {
                tx.set_op_state(project, &land.op.id, OpState::Failed, Some(&reason), now)
            })?;
            Ok(Outcome::Pending)
        }
    }
}

/// The result of a merged pull request: its merge commit. None when the pull
/// request is not merged yet or names no commit (F2).
fn merged_result(state: &crate::PrState) -> Option<String> {
    if !state.is_merged() {
        return None;
    }
    let commit = state
        .merge_commit
        .clone()
        .filter(|commit| !commit.trim().is_empty())?;
    Some(serde_json::json!({ "merge_commit": commit }).to_string())
}

/// The claim of a task whose first attempt was delivered (T-91): a draft pull
/// request for its branch, naming the issue it closes and carrying the CLAIM
/// marker.
fn claim_call(project: &str, fact: &Fact) -> Call {
    let branch = branch_of(fact.task_branch.as_deref(), project, &fact.lane, &fact.task);
    Call {
        key: format!("{}:claim", fact.task),
        task: fact.task.clone(),
        attempt: fact.attempts.first().map(|attempt| attempt.n),
        what: What::Claim {
            repo: fact.github.repo.clone(),
            branch,
            base: fact.integration.clone(),
            title: format!("{} (#{})", first_words(&fact.goal), fact.issue),
            body: format!(
                "Closes #{}\n\n{}",
                fact.issue,
                marker::claim(&format!("{project}:{}", fact.lane), fact.issue)
            ),
        },
    }
}

/// The calls a task makes once its pull request exists.
fn calls_after_claim(project: &str, fact: &Fact, pr: &PrRef) -> Vec<Call> {
    let lane_seat = format!("{project}:{}", fact.lane);
    let mut calls = Vec::new();
    for attempt in &fact.attempts {
        let base = format!("{}:{}", fact.task, attempt.n);
        if attempt.handed_in.is_some() {
            calls.push(Call {
                key: format!("{base}:completion"),
                task: fact.task.clone(),
                attempt: Some(attempt.n),
                what: What::Comment {
                    pr: pr.clone(),
                    body: format!(
                        "Handed in ({}).\n\n{}",
                        attempt.candidate.clone().unwrap_or_else(|| "-".to_string()),
                        marker::hand_in(&lane_seat, attempt.n as u32)
                    ),
                },
            });
        }
        if let Some((passed, findings, by_seat)) = &attempt.verdict {
            let verdict = if *passed { "pass" } else { "fail" };
            let body = if findings.trim().is_empty() {
                format!("Verdict: {verdict}.")
            } else {
                format!("Verdict: {verdict}.\n\n{findings}")
            };
            calls.push(Call {
                key: format!("{base}:verdict"),
                task: fact.task.clone(),
                attempt: Some(attempt.n),
                what: What::Comment {
                    pr: pr.clone(),
                    body: format!(
                        "{body}\n\n{}",
                        marker::completion(by_seat, attempt.n as u32, None)
                    ),
                },
            });
        }
        if attempt.merged.is_some() {
            let landed = fact
                .merge_line
                .as_deref()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .unwrap_or("merged");
            // The landing ran the gate, so this is the one place `ci` is known
            // (T-98): the verdict's reviewer and round, and the gate's state.
            let reviewer = attempt
                .verdict
                .as_ref()
                .map(|(_, _, by_seat)| by_seat.as_str())
                .unwrap_or_default();
            let ci = if reviewer.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\n{}",
                    marker::completion(reviewer, attempt.n as u32, Some("pass"))
                )
            };
            calls.push(Call {
                key: format!("{base}:merge"),
                task: fact.task.clone(),
                attempt: Some(attempt.n),
                what: What::Comment {
                    pr: pr.clone(),
                    body: format!("Landed in {}: {landed}{ci}", fact.integration),
                },
            });
            calls.push(Call {
                key: format!("{base}:close"),
                task: fact.task.clone(),
                attempt: Some(attempt.n),
                what: What::Close {
                    repo: fact.github.repo.clone(),
                    issue: fact.issue,
                    comment: format!("Landed in {} ({landed}).", fact.integration),
                },
            });
        }
    }
    calls
}

/// Make one call, or skip it because the ops say it is done, unknown or out of
/// attempts. One op per call, whatever its state.
fn run_call(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    call: &Call,
    ops: &mut BTreeMap<String, Vec<OpRow>>,
    budget: &mut usize,
    now: Ts,
) -> GearResult<Outcome> {
    let prior = ops.get(&call.key).cloned().unwrap_or_default();
    if let Some(op) = prior
        .iter()
        .rev()
        .find(|op| matches!(op.state, OpState::Confirmed | OpState::Unknown))
    {
        // Confirmed once, or left with an unknown outcome: never repeated. A
        // repeated confirmation is not news (T-91, F5).
        return Ok(match op.state {
            OpState::Confirmed => Outcome::Already,
            _ => Outcome::Pending,
        });
    }
    if let Some(op) = prior
        .iter()
        .find(|op| matches!(op.state, OpState::Intended | OpState::Executing))
    {
        // The engine stopped between the intent and the answer: the call may
        // have happened, so it is never repeated (T-91, F4).
        store
            .write(|tx| tx.set_op_state(project, &op.id, OpState::Unknown, Some(RESTARTED), now))?;
        return Ok(Outcome::Pending);
    }
    if *budget == 0 {
        // This tick's calls are spent: the rest wait for the next one.
        return Ok(Outcome::Pending);
    }
    *budget -= 1;
    let op = store.write(|tx| {
        tx.insert_op(&NewOp {
            project: project.to_string(),
            seat: forge_seat(project),
            generation: 0,
            kind: OpKind::Forge,
            task: Some(call.task.clone()),
            attempt: call.attempt,
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({
                "forge": call.what.name(),
                "key": call.key,
                "attempts": prior.len() + 1,
            }),
            created: now,
        })
    })?;
    let outcome = act(forge, &call.what);
    let (state, result) = match &outcome {
        Ok(result) => (OpState::Confirmed, result.clone()),
        Err(crate::Error::Timeout(_)) => (OpState::Unknown, TIMED_OUT.to_string()),
        Err(why) => (OpState::Failed, why.to_string()),
    };
    store.write(|tx| tx.set_op_state(project, &op.id, state, Some(&result), now))?;
    // The op the call just wrote is part of the record for the rest of the
    // tick too.
    let mut row = op;
    row.state = state;
    row.result = Some(result.clone());
    row.updated = now;
    ops.entry(call.key.clone()).or_default().push(row);
    Ok(match outcome {
        Ok(result) => Outcome::Done(result),
        // A failure stands for this tick only: the next tick tries again, so an
        // outage of any length catches up (T-91, F3).
        Err(_) => Outcome::Pending,
    })
}

/// The one call, against the forge.
fn act(forge: &dyn Forge, what: &What) -> ForgeResult<String> {
    match what {
        What::Claim {
            repo,
            branch,
            base,
            title,
            body,
        } => forge.open_claim(repo, branch, base, title, body).map(|pr| {
            serde_json::json!({ "repo": pr.repo, "number": pr.number, "url": pr.url }).to_string()
        }),
        What::Comment { pr, body } => forge.comment(pr, body).map(|_| String::new()),
        What::Close {
            repo,
            issue,
            comment,
        } => forge
            .close_issue(repo, *issue, comment)
            .map(|_| String::new()),
    }
}

/// The tasks of the project that a forge drives, with their facts.
fn facts(tx: &Tx<'_>, project: &str) -> GearResult<Vec<Fact>> {
    let Some(row) = tx.get_project(project)? else {
        return Ok(Vec::new());
    };
    let Ok(config) = parse_config(&row.config_toml) else {
        // A config that does not parse is the engine's problem elsewhere; the
        // forge does nothing rather than guess.
        return Ok(Vec::new());
    };
    let mut facts = Vec::new();
    for task in tx.tasks_of_project(project)? {
        let Some(issue) = task.issue.and_then(|n| u64::try_from(n).ok()) else {
            continue;
        };
        let Some(lane) = task.lane.clone() else {
            continue;
        };
        let Some((github, integration)) = config.mode_of_lane(&lane) else {
            continue;
        };
        let goal = tx
            .get_contract(project, &task.id, task.contract_version)?
            .map(|contract| contract.goal)
            .unwrap_or_default();
        let mut attempts = Vec::new();
        for (n, times) in tx.attempt_times_of_task(project, &task.id)? {
            let candidate = tx
                .get_attempt(project, &task.id, n)?
                .and_then(|attempt| attempt.candidate);
            let verdict = tx
                .get_review_verdict(project, &task.id, n)?
                .map(|verdict| (verdict.passed, verdict.findings, verdict.by_seat));
            attempts.push(AttemptFact {
                n,
                delivered: times.delivered,
                handed_in: times.handed_in,
                verdict,
                merged: times.merged,
                candidate,
            });
        }
        let merge_line = tx
            .ops_of_seat_kind(&format!("{project}:{lane}"), OpKind::Merge)?
            .into_iter()
            .find(|op| {
                op.task.as_deref() == Some(task.id.as_str()) && op.state == OpState::Confirmed
            })
            .and_then(|op| op.result);
        facts.push(Fact {
            task: task.id,
            lane,
            github,
            integration,
            task_branch: config.conventions.task_branch.clone(),
            issue,
            goal,
            attempts,
            merge_line,
        });
    }
    Ok(facts)
}

/// The branch of a task (T-252): the project's `[conventions].task_branch`
/// template — `{lane}/t<n>` when it says nothing, the same rule as
/// `gear_engine::dispatch::task_branch`.
fn branch_of(template: Option<&str>, project: &str, lane: &str, task: &str) -> String {
    gear_model::branch::expand(
        template.unwrap_or(gear_model::branch::TASK_BRANCH),
        project,
        lane,
        task,
    )
}

/// The first words of a goal, for a pull request's title.
fn first_words(goal: &str) -> String {
    const MAX: usize = 60;
    const WORDS: usize = 8;
    let goal = goal.trim();
    let mut title = String::new();
    for word in goal.split_whitespace().take(WORDS) {
        if !title.is_empty() && title.len() + 1 + word.len() > MAX {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    if title.is_empty() {
        "a task".to_string()
    } else {
        title
    }
}

// ------------------------------------------------------------ the config
//
// The part of a project's config the forge reads. The whole config is
// `gear-app`'s (`config::ProjectConfig`), and it depends on this crate for the
// issue type, so this crate reads the same TOML from the store with the fields a
// GitHub mode needs. The two must agree on the shape; `gear-app`'s config
// tests pin it, and a shape this crate cannot read is local-only here.

/// The `[github]` mode of one repo, resolved (a section without a repo names
/// no place to read or write, so it is local-only).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Github {
    repo: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default)]
    github: Option<GithubSection>,
    #[serde(default)]
    repos: Vec<RepoSection>,
    #[serde(default)]
    lanes: Vec<LaneSection>,
    #[serde(default)]
    conventions: Conventions,
}

#[derive(Debug, Default, Deserialize)]
struct Conventions {
    #[serde(default)]
    integration: Option<String>,
    /// The project's task-branch template (T-252); `None` = `{lane}/t{n}`.
    #[serde(default)]
    task_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubSection {
    #[serde(default)]
    repo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepoSection {
    name: String,
    #[serde(default)]
    integration: Option<String>,
    /// A child repo's own `[github]`, and only that: it never inherits the
    /// project's section, which names another repository.
    #[serde(default)]
    github: Option<GithubSection>,
}

#[derive(Debug, Deserialize)]
struct LaneSection {
    name: String,
    #[serde(default)]
    repo: Option<String>,
}

impl Config {
    /// The GitHub mode and integration branch of the repo a lane works in;
    /// `None` = the lane's repo is local-only.
    fn mode_of_lane(&self, lane: &str) -> Option<(Github, String)> {
        let bound = self
            .lanes
            .iter()
            .find(|section| section.name == lane)
            .and_then(|section| section.repo.as_deref());
        let (github, integration) =
            match bound.and_then(|name| self.repos.iter().find(|repo| repo.name == name)) {
                Some(section) => (section.github.as_ref(), section.integration.clone()),
                None => (self.github.as_ref(), None),
            };
        let github = github.and_then(|section| {
            Some(Github {
                repo: section.repo.clone()?,
            })
        })?;
        let integration = integration
            .or_else(|| self.conventions.integration.clone())
            .unwrap_or_else(|| "dev".to_string());
        Some((github, integration))
    }
}

fn parse_config(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str::<Config>(text)
}
