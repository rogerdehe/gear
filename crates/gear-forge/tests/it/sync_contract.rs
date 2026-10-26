//! Contract for `gear_forge::sync` (T-91): the GitHub side of a project, driven
//! from the store.
//!
//! A fake `Forge` is the project's forge: the test seeds a store, ticks `sync`
//! as the engine does, and checks the calls a team would see on GitHub — once
//! per event and in order, no duplicate after a restart, and an outage that
//! queues the call without stopping local work.

use std::cell::{Cell, RefCell};

use gear_forge::{sync, sync_with, Error, Filter, Forge, ForgeResult, Issue, MergeMethod, MergeResult, PrRef};
use gear_model::{AttemptState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, ContractRow, NewOp, ReviewVerdictRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// The forge as GitHub would answer, and the list of calls it was given.
#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<String>>,
    down: Cell<bool>,
    /// Fail the next call whose text contains this, once.
    fail_once: RefCell<Option<String>>,
    number: Cell<u64>,
}

impl Fake {
    fn up() -> Fake {
        Fake::default()
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn fail_once(&self, when: &str) {
        *self.fail_once.borrow_mut() = Some(when.to_string());
    }

    fn reachable(&self, what: &str) -> ForgeResult<()> {
        let once = self.fail_once.borrow_mut().take();
        match once {
            Some(when) if what.contains(&when) => {
                return Err(Error::Failed("the forge refused this call".to_string()))
            }
            Some(when) => *self.fail_once.borrow_mut() = Some(when),
            None => {}
        }
        if self.down.get() {
            Err(Error::Failed("the forge is down".to_string()))
        } else {
            Ok(())
        }
    }
}

impl Forge for Fake {
    fn issues(&self, _repo: &str, _filter: &Filter) -> ForgeResult<Vec<Issue>> {
        Ok(Vec::new())
    }

    fn open_claim(
        &self,
        repo: &str,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> ForgeResult<PrRef> {
        self.reachable("claim")?;
        self.calls
            .borrow_mut()
            .push(format!("claim {repo} {branch} {base} {title} | {body}"));
        let number = self.number.get() + 1;
        self.number.set(number);
        Ok(PrRef {
            repo: repo.to_string(),
            number,
            url: format!("https://github.com/{repo}/pull/{number}"),
        })
    }

    fn comment(&self, pr: &PrRef, body: &str) -> ForgeResult<()> {
        self.reachable(body)?;
        self.calls
            .borrow_mut()
            .push(format!("comment {}#{} | {body}", pr.repo, pr.number));
        Ok(())
    }

    fn merge(&self, pr: &PrRef, method: MergeMethod) -> ForgeResult<MergeResult> {
        self.reachable("merge")?;
        self.calls
            .borrow_mut()
            .push(format!("merge {}#{} {}", pr.repo, pr.number, method.flag()));
        Ok(MergeResult {
            merged: true,
            message: "merged".to_string(),
        })
    }

    fn close_issue(&self, repo: &str, number: u64, comment: &str) -> ForgeResult<()> {
        self.reachable("close")?;
        self.calls
            .borrow_mut()
            .push(format!("close {repo}#{number} | {comment}"));
        Ok(())
    }
}

/// A config with project `p` in GitHub mode.
fn config() -> String {
    "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/tmp/gear-sync-fixture\"\n\
     \n[github]\nrepo = \"owner/name\"\n\
     \n[conventions]\nintegration = \"dev\"\n"
        .to_string()
}

/// A store with a project in GitHub mode, its seats and task `T-1` (lane
/// `heat`) carrying `issue: #12`; the task is delivered but nothing else has
/// happened yet.
fn world() -> Store {
    let mut store = Store::open_in_memory().expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            let mut project = tx.get_project("p")?.expect("the project");
            project.config_toml = config();
            tx.update_project(&project)?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-1".into(),
                version: 1,
                goal: "The forge layer, GitHub first".into(),
                accept: "the calls happen once".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
            })?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Running,
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
                updated: T0,
                issue: Some(12),
            })?;
            Ok(())
        })
        .expect("the world");
    store
}

/// The task's first attempt, delivered at `T0 + 1`.
fn deliver(store: &mut Store) {
    store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
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
                started: T0,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Delivered, T0 + 1)
        })
        .expect("the delivery");
}

fn hand_in(store: &mut Store, candidate: &str) {
    store
        .write(|tx| {
            let mut attempt = tx.get_attempt("p", "T-1", 1)?.expect("the attempt");
            attempt.state = AttemptState::Submitted;
            attempt.candidate = Some(candidate.to_string());
            tx.update_attempt(&attempt)?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, T0 + 60)
        })
        .expect("the hand-in");
}

fn verdict(store: &mut Store, passed: bool, findings: &str) {
    store
        .write(|tx| {
            tx.insert_review_verdict(&ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed,
                findings: findings.to_string(),
                by_seat: "p:review-1".into(),
                created: T0 + 120,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Verdict, T0 + 120)
        })
        .expect("the verdict");
}

fn merged(store: &mut Store, line: &str) {
    store
        .write(|tx| {
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, T0 + 180)?;
            let op = tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind: OpKind::Merge,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({}),
                created: T0 + 180,
            })?;
            tx.set_op_state("p", &op.id, OpState::Confirmed, Some(line), T0 + 180)
        })
        .expect("the merge");
}

/// Run one engine tick's sync.
fn tick(store: &mut Store, forge: &Fake, at: i64) -> Vec<String> {
    sync_with(store, "p", forge, T0 + at).expect("the sync never fails on a forge error")
}

#[test]
fn a_first_delivery_opens_the_claim_and_nothing_else() {
    let mut store = world();
    deliver(&mut store);
    let forge = Fake::up();

    let done = tick(&mut store, &forge, 2);

    assert_eq!(done, vec!["p:T-1:claim"], "{done:?}");
    let calls = forge.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let claim = &calls[0];
    assert!(claim.starts_with("claim owner/name heat/t1 dev"), "{claim}");
    assert!(claim.contains("The forge layer, GitHub first (#12)"), "{claim}");
    assert!(claim.contains("Closes #12"), "{claim}");
    assert!(
        claim.contains("<!-- ORCHESTRATE:CLAIM agent=p:heat issue=12 -->"),
        "{claim}"
    );
}

#[test]
fn a_second_tick_makes_no_call() {
    let mut store = world();
    deliver(&mut store);
    let forge = Fake::up();
    tick(&mut store, &forge, 2);
    let first = forge.calls();

    let done = tick(&mut store, &forge, 3);

    assert!(done.is_empty(), "{done:?}");
    assert_eq!(forge.calls(), first, "an event is called once");
    // And after a whole lifecycle, a tick reports nothing it reported before
    // (T-91, F5).
    hand_in(&mut store, "abc1234");
    verdict(&mut store, true, "");
    merged(&mut store, "merged T-1 into dev: def5678");
    let first = tick(&mut store, &forge, 200);
    assert_eq!(first.len(), 4, "{first:?}");
    assert!(tick(&mut store, &forge, 201).is_empty(), "a confirmed event is not news again");
}

#[test]
fn a_restart_does_not_repeat_a_call_it_may_have_made() {
    let mut store = world();
    deliver(&mut store);
    let forge = Fake::up();
    // The engine wrote the intent and stopped before the answer.
    store
        .write(|tx| {
            tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:forge".into(),
                generation: 0,
                kind: OpKind::Forge,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "forge": "claim", "key": "T-1:claim" }),
                created: T0 + 1,
            })
        })
        .expect("the unfinished op");

    tick(&mut store, &forge, 2);

    assert!(forge.calls().is_empty(), "a claim may have been opened: {:?}", forge.calls());
    let ops = store
        .read(|tx| tx.ops_of_seat_kind("p:forge", OpKind::Forge))
        .expect("the ops");
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].state, OpState::Unknown);
    // And the next tick does not bring it back either.
    tick(&mut store, &forge, 3);
    assert!(forge.calls().is_empty(), "{:?}", forge.calls());
}

#[test]
fn a_hand_in_a_verdict_and_a_merge_are_commented_in_order() {
    let mut store = world();
    deliver(&mut store);
    hand_in(&mut store, "abc1234");
    verdict(&mut store, false, "F1: the gate is red");
    merged(&mut store, "merged T-1 into dev: def5678");
    let forge = Fake::up();

    tick(&mut store, &forge, 200);

    let calls = forge.calls();
    // The claim, the hand-in, the verdict, the landed comment and the close.
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert!(calls[0].starts_with("claim owner/name heat/t1 dev"), "{calls:?}");
    assert!(calls[1].starts_with("comment owner/name#1"), "{calls:?}");
    assert!(calls[1].contains("Handed in (abc1234)"), "{calls:?}");
    assert!(
        calls[1].contains("<!-- ORCHESTRATE:COMPLETION agent=p:heat rounds=1 -->"),
        "{calls:?}"
    );
    assert!(calls[2].contains("Verdict: fail."), "{calls:?}");
    assert!(calls[2].contains("F1: the gate is red"), "{calls:?}");
    assert!(
        calls[2].contains("<!-- ORCHESTRATE:COMPLETION reviewer=p:review-1 rounds=1 -->"),
        "{calls:?}"
    );
    assert!(calls[3].contains("Landed in dev: merged T-1 into dev: def5678"), "{calls:?}");
    assert!(
        calls[3].contains("<!-- ORCHESTRATE:COMPLETION reviewer=p:review-1 rounds=1 ci=pass -->"),
        "the landing carries the gate's state: {calls:?}"
    );
}

#[test]
fn a_landing_comments_the_merge_and_closes_the_issue() {
    let mut store = world();
    deliver(&mut store);
    hand_in(&mut store, "abc1234");
    verdict(&mut store, true, "");
    merged(&mut store, "merged T-1 into dev: def5678");
    let forge = Fake::up();

    tick(&mut store, &forge, 200);

    let calls = forge.calls();
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert!(calls[3].contains("comment owner/name#1 | Landed in dev: merged T-1 into dev: def5678"), "{calls:?}");
    assert!(calls[4].starts_with("close owner/name#12"), "{calls:?}");
    assert!(calls[4].contains("Landed in dev"), "{calls:?}");
}

#[test]
fn an_outage_queues_the_call_and_the_next_tick_catches_up() {
    let mut store = world();
    deliver(&mut store);
    let forge = Fake::up();
    forge.down.set(true);

    // The tick does not fail: the local work goes on (T-91).
    let done = tick(&mut store, &forge, 2);
    assert!(done.is_empty(), "{done:?}");
    let ops = store
        .read(|tx| tx.ops_of_seat_kind("p:forge", OpKind::Forge))
        .expect("the ops");
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0].state, OpState::Failed);
    assert!(ops[0].result.as_deref().unwrap_or_default().contains("forge is down"), "{ops:?}");

    // The forge is back: the same call is made once.
    forge.down.set(false);
    let done = tick(&mut store, &forge, 3);
    assert_eq!(done, vec!["p:T-1:claim"], "{done:?}");
    assert_eq!(forge.calls().len(), 1, "{:?}", forge.calls());
    // And nothing more.
    tick(&mut store, &forge, 4);
    assert_eq!(forge.calls().len(), 1, "{:?}", forge.calls());
}

/// T-91, F3: an outage never drops the work. A call keeps being tried on every
/// tick, so a forge that is down for longer than the old three attempts still
/// catches up — once, in order.
#[test]
fn an_outage_of_any_length_catches_up_once() {
    let mut store = world();
    deliver(&mut store);
    let forge = Fake::up();
    forge.down.set(true);
    for at in 2..=5 {
        assert!(tick(&mut store, &forge, at).is_empty(), "tick {at}");
    }
    assert!(forge.calls().is_empty(), "{:?}", forge.calls());

    forge.down.set(false);
    let done = tick(&mut store, &forge, 6);

    assert_eq!(done, vec!["p:T-1:claim"], "{done:?}");
    assert_eq!(forge.calls().len(), 1, "{:?}", forge.calls());
    // And the later events keep their order behind the recovered claim.
    hand_in(&mut store, "abc1234");
    verdict(&mut store, true, "");
    let done = tick(&mut store, &forge, 7);
    assert_eq!(done.len(), 2, "{done:?}");
    let calls = forge.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(calls[1].contains("Handed in"), "{calls:?}");
    assert!(calls[2].contains("Verdict: pass"), "{calls:?}");
}

/// T-91, F3: one tick makes a bounded number of calls, so a backlog catches up
/// over the next ticks instead of holding the tick for every call's timeout.
#[test]
fn a_backlog_is_worked_off_a_bounded_number_of_calls_at_a_time() {
    let mut store = world();
    for n in 2..=10 {
        let task = format!("T-{n}");
        store
            .write(|tx| {
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: task.clone(),
                    version: 1,
                    goal: "a task".into(),
                    accept: "it lands".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                })?;
                Ok(())
            })
            .expect("a contract");
    }
    deliver(&mut store);
    // Every task is delivered, so every one of them wants its claim.
    store
        .write(|tx| {
            for n in 2..=10 {
                let task = format!("T-{n}");
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: task.clone(),
                    rank: n,
                    state: TaskState::Running,
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
                    updated: T0,
                    issue: Some(100 + n),
                })?;
                tx.insert_attempt(&AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: task.clone(),
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
                    started: T0,
                    updated: T0,
                })?;
                tx.set_attempt_time("p", &task, 1, AttemptStep::Delivered, T0 + 1)?;
            }
            Ok(())
        })
        .expect("ten delivered tasks");
    let forge = Fake::up();

    let done = tick(&mut store, &forge, 2);
    assert_eq!(done.len(), gear_forge::sync::MAX_CALLS_PER_TICK, "{done:?}");
    assert_eq!(forge.calls().len(), gear_forge::sync::MAX_CALLS_PER_TICK, "{:?}", forge.calls());

    let done = tick(&mut store, &forge, 3);
    assert_eq!(forge.calls().len(), 10, "the rest catches up: {:?}", forge.calls());
    assert_eq!(done.len(), 2, "{done:?}");
}

/// T-91, F2: a merge that fails leaves the close for the retry, so a reader
/// never sees an issue closed before the merge landed.
#[test]
fn a_failed_merge_holds_back_the_close() {
    let mut store = world();
    deliver(&mut store);
    hand_in(&mut store, "abc1234");
    verdict(&mut store, true, "");
    merged(&mut store, "merged T-1 into dev: def5678");
    let forge = Fake::up();
    forge.fail_once("Landed in dev");

    let done = tick(&mut store, &forge, 200);

    assert!(!forge.calls().iter().any(|call| call.starts_with("close")), "{:?}", forge.calls());
    assert_eq!(
        done.len(),
        3,
        "the claim, the hand-in and the verdict are done; the merge and its close wait: {done:?}"
    );

    // The retry lands the merge comment and only then the close.
    let done = tick(&mut store, &forge, 201);
    assert_eq!(done.len(), 2, "{done:?}");
    let calls = forge.calls();
    assert!(calls.last().unwrap().starts_with("close owner/name#12"), "{calls:?}");
}

/// T-91, F2: a hand-in that fails holds back the verdict, so the comments keep
/// the order they happened in.
#[test]
fn a_failed_completion_holds_back_the_verdict() {
    let mut store = world();
    deliver(&mut store);
    hand_in(&mut store, "abc1234");
    verdict(&mut store, true, "");
    let forge = Fake::up();
    forge.fail_once("Handed in");

    let done = tick(&mut store, &forge, 200);
    assert_eq!(done, vec!["p:T-1:claim"], "{done:?}");
    assert_eq!(forge.calls().len(), 1, "only the claim: {:?}", forge.calls());

    tick(&mut store, &forge, 201);
    let calls = forge.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(calls[1].contains("Handed in"), "the completion comes first: {calls:?}");
    assert!(calls[2].contains("Verdict: pass"), "then the verdict: {calls:?}");
}

#[test]
fn a_local_project_makes_no_call() {
    let mut store = world();
    store
        .write(|tx| {
            let mut project = tx.get_project("p")?.expect("the project");
            project.config_toml = "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/tmp/x\"\n".into();
            tx.update_project(&project)
        })
        .expect("a local-only config");
    deliver(&mut store);
    let forge = Fake::up();

    let done = sync(&mut store, "p", T0 + 2).expect("a local project");

    assert!(done.is_empty(), "{done:?}");
    assert!(forge.calls().is_empty());
}
