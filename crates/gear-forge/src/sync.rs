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

use gear_model::{GearResult, MsgKind, MsgState, OpKind, OpState, Ts};
use gear_store::{NewMessage, NewOp, OpRow, Store, Tx, VoyageIssueRow, VoyageRow};
use serde::Deserialize;

use crate::marker;
use crate::{Filter, Forge, ForgeResult, MergeMethod, PrRef};

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
    // The voyage label (T-637) and the intake of its labeled issues (T-638):
    // both need the departed voyage of a GitHub project.
    let voyage = store.read(|tx| voyage_of(tx, project))?;
    let label = match &voyage {
        Some((milestone, repo, label)) => {
            let created = store.read(|tx| {
                Ok(tx
                    .get_meta(&format!("forge.label.{project}.{milestone}"))?
                    .as_deref()
                    == Some("created"))
            })?;
            if created {
                None
            } else {
                Some((milestone.clone(), repo.clone(), label.clone()))
            }
        }
        None => None,
    };
    if facts.is_empty() && label.is_none() && voyage.is_none() {
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
    if let Some((milestone, repo, label)) = label {
        let call = Call {
            key: format!("label:{milestone}"),
            task: milestone.clone(),
            attempt: None,
            what: What::Label {
                repo,
                name: label,
                description: format!("GEAR voyage {milestone} of {project}"),
            },
        };
        match run_call(store, project, forge, &call, &mut ops, &mut budget, now)? {
            Outcome::Done(_) => {
                store.write(|tx| {
                    tx.set_meta(&format!("forge.label.{project}.{milestone}"), "created")
                })?;
                done.push(format!("{project}:label:{milestone}"));
            }
            // Confirmed on an earlier tick: the meta may still be missing.
            Outcome::Already => {
                store.write(|tx| {
                    tx.set_meta(&format!("forge.label.{project}.{milestone}"), "created")
                })?;
            }
            // A failure stands for this tick only: the next tick tries again.
            Outcome::Pending => {}
        }
    }
    // The intake of the voyage's labeled issues (T-638): once the label is
    // created, at most every INTAKE_EVERY_S.
    if let Some((milestone, repo, label)) = &voyage {
        let created = store.read(|tx| {
            Ok(tx
                .get_meta(&format!("forge.label.{project}.{milestone}"))?
                .as_deref()
                == Some("created"))
        })?;
        if created {
            done.extend(intake(store, project, forge, milestone, repo, label, now)?);
        }
    }
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
    /// The branch the task's work goes on (T-252, T-602): the branch the task
    /// itself names when it names one (a member of a feature pushes onto the
    /// feature's branch), else the project's template — `{lane}/t<n>` when it
    /// says nothing.
    branch: String,
    issue: u64,
    goal: String,
    attempts: Vec<AttemptFact>,
    /// The engine's own merge line ("merged T-61 into dev: a1b2c3d"), when the
    /// merge has confirmed.
    merge_line: Option<String>,
    /// Another task bound to the same issue is not done and not cancelled:
    /// the issue stays open until every task bound to it is done or cancelled
    /// (T-617).
    issue_open_elsewhere: bool,
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
    /// The label of a departed voyage (T-637).
    Label {
        repo: String,
        name: String,
        description: String,
    },
}

impl What {
    fn name(&self) -> &'static str {
        match self {
            What::Claim { .. } => "claim",
            What::Comment { .. } => "comment",
            What::Close { .. } => "close",
            What::Label { .. } => "label",
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
    // `merge = "push"`: the engine merges and pushes, and a task has no pull
    // request (Carthage, 2026-11-05: a draft claim was tried at delivery,
    // before the lane had pushed a commit, and refused a hundred times). The
    // landing comments name no pull request; the issue still closes when its
    // last task lands.
    if !fact.github.pr {
        for call in calls_after_claim(project, fact, &PrRef::none(&fact.github.repo)) {
            if !matches!(call.what, What::Close { .. }) {
                continue;
            }
            match run_call(store, project, forge, &call, ops, budget, now)? {
                Outcome::Done(_) => done.push(format!("{project}:{}:{}", fact.task, call.key)),
                Outcome::Already => {}
                Outcome::Pending => return Ok(done),
            }
        }
        return Ok(done);
    }
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
    let branch = fact.branch.clone();
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
    let branch = fact.branch.clone();
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
                "{} #{}\n\n{}",
                if fact.issue_open_elsewhere {
                    "Part of"
                } else {
                    "Closes"
                },
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
            // An issue split into several tasks closes when its last task
            // lands, never on the first (Carthage: #359 is seven tasks).
            if fact.issue_open_elsewhere {
                continue;
            }
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
        What::Label {
            repo,
            name,
            description,
        } => forge
            .create_label(repo, name, description)
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
    let tasks = tx.tasks_of_project(project)?;
    // The issues with a bound task that is not done or cancelled: (issue,
    // task). The issue closes only when every task bound to it is done or
    // cancelled, with at least one done (T-617).
    let mut open: Vec<(i64, String)> = Vec::new();
    for task in &tasks {
        let Some(issue) = task.issue else { continue };
        if matches!(
            task.state,
            gear_model::TaskState::Done | gear_model::TaskState::Cancelled
        ) {
            continue;
        }
        open.push((issue, task.id.clone()));
    }
    for task in tasks {
        let issue_open_elsewhere = task.issue.is_some_and(|issue| {
            open.iter()
                .any(|(other_issue, other)| *other_issue == issue && *other != task.id)
        });
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
        // T-602: one place names the branch of a known task. `gear-forge`
        // cannot depend on `gear-app` (`gear-app::base::task_branch_of` lives
        // there, and gear-app depends on this crate), so the task's own branch
        // is read from the store's accessor — the same key `task_branch_of`
        // reads — and the template is expanded as it does.
        let branch = match tx.task_branch_override(project, &task.id)? {
            Some(branch) => branch,
            None => gear_model::branch::expand(
                config
                    .conventions
                    .task_branch
                    .as_deref()
                    .unwrap_or(gear_model::branch::TASK_BRANCH),
                project,
                &lane,
                &task.id,
            ),
        };
        facts.push(Fact {
            task: task.id,
            branch,
            lane,
            github,
            integration,
            issue,
            goal,
            attempts,
            merge_line,
            issue_open_elsewhere,
        });
    }
    Ok(facts)
}

/// How often a project's labeled issues are read (T-638).
const INTAKE_EVERY_S: Ts = 120;

/// The most issues one poll reads (T-638): `gh` lists 30 by default, and a
/// full page means the list may be partial.
const INTAKE_LIMIT: u32 = 500;

/// The hash of an issue's title and body (T-638): a change on GitHub is a
/// different hash.
fn body_hash(title: &str, body: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    title.hash(&mut hasher);
    body.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// One urgent captain item about a voyage's issue (T-638).
fn intake_item(
    store: &mut Store,
    project: &str,
    milestone: &str,
    number: i64,
    body: &str,
    now: Ts,
) -> GearResult<()> {
    store.write(|tx| {
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.to_string(),
            thread: format!("intake:{milestone}:#{number}"),
            task: None,
            attempt: None,
            sender: "engine".into(),
            sender_gen: 0,
            recipient: format!("{project}:captain"),
            kind: MsgKind::System,
            urgent: true,
            body: body.to_string(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })
    })?;
    Ok(())
}

/// Read the voyage's labeled issues and turn what changed into captain items
/// (T-638): at most every [`INTAKE_EVERY_S`], and never from an error.
fn intake(
    store: &mut Store,
    project: &str,
    forge: &dyn Forge,
    milestone: &str,
    repo: &str,
    label: &str,
    now: Ts,
) -> GearResult<Vec<String>> {
    let key = format!("forge.intake_at.{project}");
    let last = store.read(|tx| tx.get_meta(&key))?;
    if let Some(last) = last.as_deref().and_then(|value| value.parse::<Ts>().ok()) {
        if now - last < INTAKE_EVERY_S {
            return Ok(Vec::new());
        }
    }
    let filter = Filter {
        labels: vec![label.to_string()],
        limit: INTAKE_LIMIT,
    };
    let listed = match forge.issues(repo, &filter) {
        Ok(issues) => issues,
        // An error changes nothing: it is never read as an empty list.
        Err(_) => {
            store.write(|tx| tx.set_meta(&key, &now.to_string()))?;
            return Ok(Vec::new());
        }
    };
    store.write(|tx| tx.set_meta(&key, &now.to_string()))?;
    // A full page may be partial: nothing is marked unlabeled this poll.
    let partial = listed.len() as u32 >= INTAKE_LIMIT;
    let rows = store.read(|tx| tx.voyage_issues(project, milestone))?;
    let mut done = Vec::new();
    let mut seen: Vec<i64> = Vec::new();
    for issue in &listed {
        let Ok(number) = i64::try_from(issue.number) else {
            continue;
        };
        seen.push(number);
        let hash = body_hash(&issue.title, &issue.body);
        let known = rows.iter().find(|row| row.number == number);
        match known {
            None => {
                store.write(|tx| {
                    tx.upsert_voyage_issue(&VoyageIssueRow {
                        project: project.to_string(),
                        number,
                        milestone: milestone.to_string(),
                        title: issue.title.clone(),
                        body_hash: hash.clone(),
                        labeled: 1,
                        first_seen: now,
                        last_seen: now,
                    })
                })?;
                intake_item(
                    store,
                    project,
                    milestone,
                    number,
                    &format!(
                        "Issue #{number} \"{}\" was labeled {label}: plan it as task(s) with `issue: #{number}` and their contracts and deps, or resolve this item to defer it. It never moves the eta or the bar.",
                        issue.title
                    ),
                    now,
                )?;
                done.push(format!("{project}:intake:{milestone}:#{number}"));
                continue;
            }
            Some(row) if row.body_hash != hash => {
                store.write(|tx| {
                    tx.upsert_voyage_issue(&VoyageIssueRow {
                        project: project.to_string(),
                        number,
                        milestone: milestone.to_string(),
                        title: issue.title.clone(),
                        body_hash: hash.clone(),
                        labeled: 1,
                        first_seen: row.first_seen,
                        last_seen: now,
                    })
                })?;
                intake_item(
                    store,
                    project,
                    milestone,
                    number,
                    &format!("Issue #{number} changed on GitHub: check its tasks' contracts."),
                    now,
                )?;
                done.push(format!("{project}:intake:{milestone}:#{number}"));
                continue;
            }
            Some(row) if row.labeled == 0 => {
                store.write(|tx| {
                    tx.upsert_voyage_issue(&VoyageIssueRow {
                        project: project.to_string(),
                        number,
                        milestone: milestone.to_string(),
                        title: issue.title.clone(),
                        body_hash: hash.clone(),
                        labeled: 1,
                        first_seen: row.first_seen,
                        last_seen: now,
                    })
                })?;
                intake_item(
                    store,
                    project,
                    milestone,
                    number,
                    &format!("Issue #{number} carries {label} again."),
                    now,
                )?;
                done.push(format!("{project}:intake:{milestone}:#{number}"));
                continue;
            }
            Some(_) => {}
        }
    }
    if !partial {
        for row in &rows {
            if row.labeled != 1 || seen.contains(&row.number) {
                continue;
            }
            store.write(|tx| {
                tx.upsert_voyage_issue(&VoyageIssueRow {
                    project: project.to_string(),
                    number: row.number,
                    milestone: milestone.to_string(),
                    title: row.title.clone(),
                    body_hash: row.body_hash.clone(),
                    labeled: 0,
                    first_seen: row.first_seen,
                    last_seen: now,
                })
            })?;
            intake_item(
                store,
                project,
                milestone,
                row.number,
                &format!(
                    "Issue #{} no longer carries {label} (closed or unlabeled): its tasks keep running; cancel them in the plan if the user wants.",
                    row.number
                ),
                now,
            )?;
            done.push(format!("{project}:intake:{milestone}:#{}", row.number));
        }
    }
    Ok(done)
}

/// The departed voyage of a GitHub project (T-637/T-638): the milestone, the
/// repo and the label its issues carry.
fn voyage_of(tx: &Tx<'_>, project: &str) -> GearResult<Option<(String, String, String)>> {
    let Some(row) = tx.get_project(project)? else {
        return Ok(None);
    };
    let Ok(config) = parse_config(&row.config_toml) else {
        return Ok(None);
    };
    let Some(github) = config.project_github() else {
        return Ok(None);
    };
    let Some(voyage) = departure_voyage(tx, project)? else {
        return Ok(None);
    };
    let Some(label) = voyage_label(&github.voyage_label, project, &voyage.milestone) else {
        return Ok(None);
    };
    Ok(Some((voyage.milestone, github.repo, label)))
}

/// The voyage a GitHub project's label is for (T-637): the project's voyage
/// with the latest departure, unless its landing order is Resolved (landed).
fn departure_voyage(tx: &Tx<'_>, project: &str) -> GearResult<Option<VoyageRow>> {
    let mut voyages: Vec<VoyageRow> = tx
        .voyages_of_project(project)?
        .into_iter()
        .filter(|voyage| voyage.departure.is_some())
        .collect();
    voyages.sort_by_key(|voyage| voyage.departure);
    let Some(voyage) = voyages.pop() else {
        return Ok(None);
    };
    if voyage_landed(tx, project, &voyage.milestone)? {
        return Ok(None);
    }
    Ok(Some(voyage))
}

/// Whether the voyage's landing order was answered: its message is Resolved.
fn voyage_landed(tx: &Tx<'_>, project: &str, milestone: &str) -> GearResult<bool> {
    let Some(id) = tx.get_meta(&format!("station:{project}:{milestone}:land"))? else {
        return Ok(false);
    };
    Ok(tx
        .get_message(project, &id)?
        .is_some_and(|message| message.state == MsgState::Resolved))
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
    /// `merge = "pr"`: a task lands through its pull request. Anything else
    /// (`push`, the default) opens none.
    pr: bool,
    /// The voyage label template (T-643); empty = off.
    voyage_label: String,
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
    #[serde(default)]
    merge: Option<String>,
    #[serde(default)]
    issues: Option<IssuesSection>,
}

/// `[github.issues]`: the labels of a project's issues (T-643).
#[derive(Debug, Default, Deserialize)]
struct IssuesSection {
    /// The label a departed voyage's issues carry (T-643):
    /// `{project}/{milestone}` by default; `""` turns it off.
    #[serde(default)]
    voyage_label: Option<String>,
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
        let github = github.and_then(github_of)?;
        let integration = integration
            .or_else(|| self.conventions.integration.clone())
            .unwrap_or_else(|| "dev".to_string());
        Some((github, integration))
    }

    /// The GitHub mode of the project root (T-637): `None` when the project
    /// has no forge.
    fn project_github(&self) -> Option<Github> {
        github_of(self.github.as_ref()?)
    }
}

/// The resolved GitHub mode of one `[github]` section (T-643): the repo, the
/// merge mode and the voyage label template.
fn github_of(section: &GithubSection) -> Option<Github> {
    Some(Github {
        repo: section.repo.clone()?,
        pr: section.merge.as_deref() == Some("pr"),
        voyage_label: section
            .issues
            .as_ref()
            .and_then(|issues| issues.voyage_label.clone())
            .unwrap_or_else(|| "{project}/{milestone}".to_string()),
    })
}

/// The GitHub label a departed voyage's issues carry (T-643): the template
/// with `{project}` and `{milestone}` filled in. `None` when it is off (`""`).
fn voyage_label(template: &str, project: &str, milestone: &str) -> Option<String> {
    if template.is_empty() {
        return None;
    }
    Some(
        template
            .replace("{project}", project)
            .replace("{milestone}", milestone),
    )
}

fn parse_config(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str::<Config>(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    use crate::{Filter, Issue, MergeResult, PrRef, PrState};

    const T0: Ts = 1_800_000_000;

    /// A forge that records the label calls it is given, and can refuse one.
    #[derive(Default)]
    struct Recorder {
        labels: RefCell<Vec<String>>,
        fail_label: Cell<bool>,
        issues: RefCell<Vec<Issue>>,
        fail_issues: Cell<bool>,
        issue_filters: RefCell<Vec<(Vec<String>, u32)>>,
    }

    impl Recorder {
        fn labels(&self) -> Vec<String> {
            self.labels.borrow().clone()
        }

        fn set_issues(&self, issues: Vec<Issue>) {
            *self.issues.borrow_mut() = issues;
        }

        fn issue_filters(&self) -> Vec<(Vec<String>, u32)> {
            self.issue_filters.borrow().clone()
        }
    }

    impl Forge for Recorder {
        fn issues(&self, _repo: &str, filter: &Filter) -> ForgeResult<Vec<Issue>> {
            self.issue_filters
                .borrow_mut()
                .push((filter.labels.clone(), filter.limit));
            if self.fail_issues.replace(false) {
                return Err(crate::Error::Failed("the forge refused".to_string()));
            }
            Ok(self.issues.borrow().clone())
        }

        fn open_claim(
            &self,
            repo: &str,
            _branch: &str,
            _base: &str,
            _title: &str,
            _body: &str,
        ) -> ForgeResult<PrRef> {
            Ok(PrRef::none(repo))
        }

        fn comment(&self, _pr: &PrRef, _body: &str) -> ForgeResult<()> {
            Ok(())
        }

        fn merge(
            &self,
            _pr: &PrRef,
            _method: MergeMethod,
            _head: &str,
        ) -> ForgeResult<MergeResult> {
            Ok(MergeResult {
                merged: false,
                message: String::new(),
            })
        }

        fn pr_state(&self, _pr: &PrRef) -> ForgeResult<PrState> {
            Ok(PrState::default())
        }

        fn pr_by_head(&self, _repo: &str, _branch: &str) -> ForgeResult<Option<PrRef>> {
            Ok(None)
        }

        fn close_issue(&self, _repo: &str, _number: u64, _comment: &str) -> ForgeResult<()> {
            Ok(())
        }

        fn create_label(&self, repo: &str, name: &str, description: &str) -> ForgeResult<()> {
            if self.fail_label.replace(false) {
                return Err(crate::Error::Failed("the forge refused".to_string()));
            }
            self.labels
                .borrow_mut()
                .push(format!("label {repo} {name} | {description}"));
            Ok(())
        }
    }

    /// A store with project `p` in GitHub mode and the config given.
    fn world(config: &str) -> Store {
        let mut store = Store::open_in_memory().expect("the store");
        store
            .write(|tx| {
                gear_store::testing::seed_project(tx, "p", "sess", T0)?;
                let mut project = tx.get_project("p")?.expect("the project");
                project.config_toml = config.to_string();
                tx.update_project(&project)
            })
            .expect("the world");
        store
    }

    fn github_config(issues: &str) -> String {
        format!(
            "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/tmp/gear-forge-fixture\"\n\
             \n[github]\nrepo = \"owner/name\"\n{issues}"
        )
    }

    fn departed(store: &mut Store, milestone: &str, departure: Ts) {
        store
            .write(|tx| tx.set_voyage_departure("p", milestone, departure, 1, departure))
            .expect("the departure");
    }

    fn tick(store: &mut Store, forge: &Recorder, at: Ts) -> Vec<String> {
        sync_with(store, "p", forge, at).expect("the sync never fails on a forge error")
    }

    fn label_meta(store: &mut Store, milestone: &str) -> Option<String> {
        store
            .read(|tx| tx.get_meta(&format!("forge.label.p.{milestone}")))
            .expect("the meta")
    }

    /// T-637: a departed voyage of a GitHub project gets its label once.
    #[test]
    fn a_departed_voyage_gets_its_label_once() {
        let mut store = world(&github_config(""));
        departed(&mut store, "M1", T0);
        let forge = Recorder::default();

        let done = tick(&mut store, &forge, T0 + 1);

        assert_eq!(done, vec!["p:label:M1"], "{done:?}");
        assert_eq!(
            forge.labels(),
            vec!["label owner/name p/M1 | GEAR voyage M1 of p"]
        );
        assert_eq!(label_meta(&mut store, "M1").as_deref(), Some("created"));

        let done = tick(&mut store, &forge, T0 + 2);

        assert!(done.is_empty(), "{done:?}");
        assert_eq!(forge.labels().len(), 1, "the label is created once");
    }

    /// T-637: a voyage that has not departed gets no label.
    #[test]
    fn an_undeparted_voyage_gets_no_label() {
        let mut store = world(&github_config(""));
        store
            .write(|tx| {
                tx.upsert_voyage(&VoyageRow {
                    project: "p".into(),
                    milestone: "M1".into(),
                    title: String::new(),
                    done: None,
                    bar: None,
                    eta: None,
                    departure: None,
                    departure_tasks: None,
                    departure_estimate_min: None,
                    updated: T0,
                })
            })
            .expect("the voyage");
        let forge = Recorder::default();

        let done = tick(&mut store, &forge, T0 + 1);

        assert!(done.is_empty(), "{done:?}");
        assert!(forge.labels().is_empty(), "{:?}", forge.labels());
        assert_eq!(label_meta(&mut store, "M1"), None);
    }

    /// T-637: `voyage_label = ""` turns the label off.
    #[test]
    fn an_empty_voyage_label_gets_no_label() {
        let mut store = world(&github_config("[github.issues]\nvoyage_label = \"\"\n"));
        departed(&mut store, "M1", T0);
        let forge = Recorder::default();

        let done = tick(&mut store, &forge, T0 + 1);

        assert!(done.is_empty(), "{done:?}");
        assert!(forge.labels().is_empty(), "{:?}", forge.labels());
        assert_eq!(label_meta(&mut store, "M1"), None);
    }

    /// T-637: a failed label call is retried on a later tick.
    #[test]
    fn a_failed_label_call_is_retried() {
        let mut store = world(&github_config(""));
        departed(&mut store, "M1", T0);
        let forge = Recorder::default();
        forge.fail_label.set(true);

        let done = tick(&mut store, &forge, T0 + 1);

        assert!(done.is_empty(), "{done:?}");
        assert!(forge.labels().is_empty(), "{:?}", forge.labels());
        assert_eq!(label_meta(&mut store, "M1"), None);

        let done = tick(&mut store, &forge, T0 + 2);

        assert_eq!(done, vec!["p:label:M1"], "{done:?}");
        assert_eq!(
            forge.labels(),
            vec!["label owner/name p/M1 | GEAR voyage M1 of p"]
        );
        assert_eq!(label_meta(&mut store, "M1").as_deref(), Some("created"));
    }

    /// T-637: the voyage is the latest departure, and a landed one gets none.
    #[test]
    fn a_landed_voyage_gets_no_label() {
        let mut store = world(&github_config(""));
        departed(&mut store, "M1", T0);
        departed(&mut store, "M2", T0 + 10);
        store
            .write(|tx| {
                let land = tx.insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "station:M2".into(),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: "p:captain".into(),
                    kind: gear_model::MsgKind::System,
                    urgent: true,
                    body: "land M2".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: gear_model::MsgState::Open,
                    created: T0 + 20,
                })?;
                tx.set_meta("station:p:M2:land", &land.id)?;
                tx.update_message_state(
                    "p",
                    &land.id,
                    gear_model::MsgState::Resolved,
                    Some("landed"),
                    None,
                    None,
                    T0 + 30,
                )
            })
            .expect("the land");
        let forge = Recorder::default();

        let done = tick(&mut store, &forge, T0 + 40);

        assert!(done.is_empty(), "{done:?}");
        assert!(forge.labels().is_empty(), "{:?}", forge.labels());
    }

    fn labeled_voyage(store: &mut Store) {
        departed(store, "M1", T0);
        store
            .write(|tx| tx.set_meta("forge.label.p.M1", "created"))
            .expect("the label");
    }

    fn issue(number: u64, title: &str, body: &str) -> Issue {
        Issue {
            number,
            title: title.to_string(),
            body: body.to_string(),
            labels: vec!["p/M1".to_string()],
        }
    }

    fn items(store: &mut Store, thread: &str) -> Vec<gear_store::MessageRow> {
        store
            .read(|tx| tx.thread_messages("p", thread))
            .expect("the items")
    }

    fn rows(store: &mut Store) -> Vec<VoyageIssueRow> {
        store
            .read(|tx| tx.voyage_issues("p", "M1"))
            .expect("the rows")
    }

    /// T-638: a newly labeled issue makes one captain item, and the next poll
    /// makes none.
    #[test]
    fn a_new_labeled_issue_makes_one_item() {
        let mut store = world(&github_config(""));
        labeled_voyage(&mut store);
        let forge = Recorder::default();
        forge.set_issues(vec![issue(12, "A bug", "the body")]);

        let done = tick(&mut store, &forge, T0 + 1);

        assert_eq!(done, vec!["p:intake:M1:#12"], "{done:?}");
        assert_eq!(
            forge.issue_filters(),
            vec![(vec!["p/M1".to_string()], 500)],
            "the label and the full page"
        );
        let told = items(&mut store, "intake:M1:#12");
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].urgent, "{:?}", told[0]);
        assert_eq!(told[0].recipient, "p:captain");
        assert!(
            told[0]
                .body
                .contains("Issue #12 \"A bug\" was labeled p/M1"),
            "{}",
            told[0].body
        );
        assert!(told[0].body.contains("`issue: #12`"), "{}", told[0].body);
        assert!(
            told[0].body.contains("It never moves the eta or the bar."),
            "{}",
            told[0].body
        );
        let stored = rows(&mut store);
        assert_eq!(stored.len(), 1, "{stored:?}");
        assert_eq!(stored[0].labeled, 1);
        assert_eq!(stored[0].body_hash, body_hash("A bug", "the body"));

        let done = tick(&mut store, &forge, T0 + 200);

        assert!(done.is_empty(), "{done:?}");
        assert_eq!(items(&mut store, "intake:M1:#12").len(), 1, "once");
    }

    /// T-638: a changed body makes one changed item.
    #[test]
    fn a_changed_body_makes_one_changed_item() {
        let mut store = world(&github_config(""));
        labeled_voyage(&mut store);
        let forge = Recorder::default();
        forge.set_issues(vec![issue(12, "A bug", "one")]);
        tick(&mut store, &forge, T0 + 1);

        forge.set_issues(vec![issue(12, "A bug", "two")]);
        let done = tick(&mut store, &forge, T0 + 200);

        assert_eq!(done, vec!["p:intake:M1:#12"], "{done:?}");
        let told = items(&mut store, "intake:M1:#12");
        assert_eq!(told.len(), 2, "{told:?}");
        assert_eq!(
            told[1].body,
            "Issue #12 changed on GitHub: check its tasks' contracts."
        );
        assert_eq!(rows(&mut store)[0].body_hash, body_hash("A bug", "two"));
    }

    /// T-638: a vanished issue makes one item and sets labeled 0.
    #[test]
    fn a_vanished_issue_makes_one_item_and_sets_labeled_zero() {
        let mut store = world(&github_config(""));
        labeled_voyage(&mut store);
        let forge = Recorder::default();
        forge.set_issues(vec![issue(12, "A bug", "the body")]);
        tick(&mut store, &forge, T0 + 1);

        forge.set_issues(Vec::new());
        let done = tick(&mut store, &forge, T0 + 200);

        assert_eq!(done, vec!["p:intake:M1:#12"], "{done:?}");
        let told = items(&mut store, "intake:M1:#12");
        assert_eq!(told.len(), 2, "{told:?}");
        assert_eq!(
            told[1].body,
            "Issue #12 no longer carries p/M1 (closed or unlabeled): its tasks keep running; cancel them in the plan if the user wants."
        );
        assert_eq!(rows(&mut store)[0].labeled, 0);
    }

    /// T-638: a forge error makes no item and changes no row.
    #[test]
    fn a_forge_error_makes_no_item_and_changes_no_row() {
        let mut store = world(&github_config(""));
        labeled_voyage(&mut store);
        let forge = Recorder::default();
        forge.set_issues(vec![issue(12, "A bug", "the body")]);
        forge.fail_issues.set(true);

        let done = tick(&mut store, &forge, T0 + 1);

        assert!(done.is_empty(), "{done:?}");
        assert!(items(&mut store, "intake:M1:#12").is_empty());
        assert!(rows(&mut store).is_empty(), "an error is not an empty list");

        let done = tick(&mut store, &forge, T0 + 200);

        assert_eq!(done, vec!["p:intake:M1:#12"], "{done:?}");
        assert_eq!(rows(&mut store).len(), 1);
    }

    /// T-638: two polls within 120 s list once.
    #[test]
    fn two_polls_within_120_s_list_once() {
        let mut store = world(&github_config(""));
        labeled_voyage(&mut store);
        let forge = Recorder::default();
        forge.set_issues(vec![issue(12, "A bug", "the body")]);

        tick(&mut store, &forge, T0 + 1);
        tick(&mut store, &forge, T0 + 100);

        assert_eq!(forge.issue_filters().len(), 1, "one list");
        assert_eq!(items(&mut store, "intake:M1:#12").len(), 1);
    }
}
