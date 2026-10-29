//! Contract for a dead temp seat (B02-04, T-116).
//!
//! A reviewer seat that runs no reviewer — parked (`Unknown`), its binding
//! ended, no agent ever ran in it — is released: it holds no temp slot, the
//! count of temps in use never passes the quota, and the next review takes the
//! seat. A **detached** worktree answer (a checkout whose branch was let go of)
//! opens the review instead of leaving the seat unknown.

use gear_app::tasks;
use gear_engine::release::{release_dead_temps, release_finished_reviews};
use gear_engine::review::start_reviews;
use gear_engine::EngineCfg;
use gear_herdr::fake::{Call, FakeHerdr};
use gear_model::{MsgKind, MsgState, SeatRole, SeatState, TaskState};
use gear_store::testing;
use gear_store::{BindingStatus, ContractRow, ProjectRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
/// The candidate the lane handed in (seven hex characters name a commit).
const CANDIDATE: &str = "abc1234";

/// A project with one lane, one reviewer (codex) and one temp slot.
const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 1

[captain]
pane = "w1:p9"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "codex"
profile = "codex"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
review = [{ model = "gpt-6.1-sol", cli = "codex", args = ["-c", "model_reasoning_effort=high"] }]
"#;

fn cfg() -> EngineCfg {
    let mut cfg = EngineCfg { db_path: std::env::temp_dir().join(format!("gear-engine-tests-{}/gear.db", std::process::id())).to_string_lossy().into_owned(), ..Default::default() };
    cfg.review_git = std::sync::Arc::new(crate::FakeReviewGit);
    for kind in ["codex", "opencode", "claude"] {
        cfg.profiles
            .insert(kind.to_string(), gear_herdr::builtins().remove(kind).unwrap());
    }
    cfg
}

/// A project with `tasks` handed-in tasks (T-1 … T-n): each attempt is
/// `Submitted` with a candidate, each task `Verifying` — a review is owed for
/// every one of them.
fn handed_in_world(tasks: usize) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&ProjectRow {
                id: "p".into(),
                herdr_session: "sess".into(),
                config_toml: CONFIG.into(),
                config_version: 1,
                quota_lanes: 1,
                quota_temps: 1,
                quota_includes_captain: false,
                plan_version: 0,
                created: T0,
            })?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p1", "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p9", "claude", T0)?;
            for n in 1..=tasks {
                hand_in(tx, &format!("T-{n}"), n as i64)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// One task handed in by the lane: a `Submitted` attempt with a candidate, the
/// task `Verifying`.
fn hand_in(tx: &gear_store::Tx<'_>, id: &str, rank: i64) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state: gear_model::TaskState::Ready,
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
        updated: T0,
    })?;
    tx.insert_contract(&ContractRow {
        project: "p".into(),
        task: id.into(),
        version: 1,
        goal: "the goal".into(),
        accept: "the accept criteria".into(),
        evidence_files: vec![],
        evidence_due_min: None,
        body: String::new(),
        created: T0,
        mechanical: None,
    })?;
    tasks::start_attempt(tx, "p", id, "p:heat", 1, T0)?;
    tasks::on_delivery_confirmed(tx, "p", id, 1, T0)?;
    tasks::submit_result(tx, "p", id, 1, 1, CANDIDATE, &format!("ops/reports/{id}.md"), T0)?;
    Ok(())
}

/// A project whose task T-1 was handed in: the attempt is `Submitted` with a
/// candidate, the task is `Verifying` — a review is owed.
fn world() -> Store {
    handed_in_world(1)
}

/// A reviewer seat that runs no reviewer: parked (`Unknown`), its binding
/// ended, no agent in its pane — the trial's B02-04 seat.
fn dead_seat(store: &mut Store, name: &str, task: &str) {
    store
        .write(|tx| {
            testing::seed_temp_seat(tx, "p", name, task, &format!("w9:{name}"), "codex", T0)?;
            tx.set_binding_status(&format!("p:{name}"), 1, BindingStatus::Void)?;
            tx.set_seat_state(
                &format!("p:{name}"),
                SeatState::Unknown,
                Some("worktree_open: uncertain: worktree reply is incomplete"),
                T0,
            )
        })
        .unwrap();
}

/// The temp seats that hold a slot: the count the engine and the board use
/// (design/10: STARTING, ACTIVE, RELEASING and UNKNOWN occupy the quota).
fn temps_in_use(store: &mut Store) -> usize {
    store
        .read(|tx| tx.seats_of_project("p"))
        .unwrap()
        .into_iter()
        .filter(|seat| seat.role == SeatRole::Temp)
        .filter(|seat| {
            matches!(
                seat.state,
                SeatState::Starting | SeatState::Active | SeatState::Releasing | SeatState::Unknown
            )
        })
        .count()
}

fn checkout_submissions(_herdr: &FakeHerdr, branch: &str) -> usize {
    crate::REVIEW_GIT_CALLS.with(|calls| calls.borrow().iter().filter(|args| args.first().is_some_and(|arg| arg == "checkout") && args.iter().any(|arg| arg == branch)).count())
}

/// The agent starts the engine asked for: (name, kind).
fn starts(herdr: &FakeHerdr) -> Vec<(String, String)> {
    herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::AgentStart { name, kind, .. } => Some((name.clone(), kind.clone())),
            _ => None,
        })
        .collect()
}

fn checkout_lands(_store: &mut Store, _herdr: &mut FakeHerdr, _branch: &str) {
    // Git is synchronous; the fake reports success in the call itself.
}

/// T-116: an unknown temp seat is released, and the next review starts in its
/// slot. Without the release the seat holds the only slot for ever and the
/// review never starts (`temps 1/1` with no reviewer running).
#[test]
fn an_unknown_temp_seat_is_released_and_the_next_review_takes_it() {
    let mut store = world();
    dead_seat(&mut store, "review-1", "T-1");
    let mut herdr = FakeHerdr::new();

    // The dead seat holds the only temp slot and runs nothing.
    assert_eq!(temps_in_use(&mut store), 1, "the dead seat holds the slot");
    let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 1).unwrap();
    assert!(report.started.is_empty());
    assert!(report.waiting.is_empty(), "the job waits on the dead seat: {report:?}");
    // Red before the fix: a parked seat is never retried, so the review of
    // T-1 never starts while the seat holds the only slot — and nothing says
    // so.
    assert!(
        !herdr.calls.iter().any(|call| matches!(
            call,
            Call::PaneRun { command, .. } if command.contains("GEAR_CK_OK")
        )),
        "no review is submitted on a parked seat: {:?}",
        herdr.calls
    );
    assert!(starts(&herdr).is_empty(), "no reviewer starts for it");

    // The release frees the slot.
    let released = release_dead_temps(&mut store, &mut herdr, "p", T0 + 2).unwrap();
    assert_eq!(released, vec!["p:review-1".to_string()]);
    assert_eq!(temps_in_use(&mut store), 0, "the slot is free");
    let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Released, "{:?}", seat.state_reason);
    let binding = store.read(|tx| tx.get_binding("p:review-1", 1)).unwrap().unwrap();
    assert_eq!(binding.status, BindingStatus::Void, "its binding ended");

    // The review starts in the released seat: the seat takes the job again,
    // its checkout is submitted, and the reviewer comes up once it landed.
    let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
    assert_eq!(report.started, vec!["p:review-1"], "{report:?}");
    let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
    assert_eq!(seat.task_id.as_deref(), Some("T-1"), "the seat takes the job again");
    checkout_lands(&mut store, &mut herdr, "review/t1");
    let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
    assert!(report.started.is_empty());
    assert_eq!(starts(&herdr), vec![("p-review-t1".to_string(), "codex".to_string())]);
    assert_eq!(temps_in_use(&mut store), 1, "one reviewer, the one slot");
}

/// T-116 (F1 of the review): temps in use never exceed the quota — dead seats
/// do not count, and the seats a release frees do not start beyond the quota,
/// however many reviews are waiting. Finishing one review lets the next one
/// take its slot.
#[test]
fn temps_in_use_never_exceed_the_quota() {
    // Three handed-in tasks and one slot.
    let mut store = handed_in_world(3);
    // Three dead seats with one slot: the trial's `temps 4/2`.
    dead_seat(&mut store, "review-1", "T-9");
    dead_seat(&mut store, "review-2", "T-8");
    dead_seat(&mut store, "review-3", "T-7");
    let mut herdr = FakeHerdr::new();
    assert_eq!(temps_in_use(&mut store), 3, "three dead seats hold three slots");

    let released = release_dead_temps(&mut store, &mut herdr, "p", T0 + 1).unwrap();
    assert_eq!(released.len(), 3, "every dead seat is released");
    assert_eq!(temps_in_use(&mut store), 0, "no dead seat counts");

    // Every review that starts fits the quota: three reviews are waiting and
    // one slot is free, so exactly one starts — and the bound holds on each
    // tick.
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
    assert_eq!(temps_in_use(&mut store), 1, "one reviewer, the one slot");
    assert_eq!(checkout_submissions(&herdr, "review/t1"), 1, "the first review starts");
    assert_eq!(checkout_submissions(&herdr, "review/t2"), 0, "the second waits for a slot");
    assert_eq!(checkout_submissions(&herdr, "review/t3"), 0, "the third waits for a slot");

    // The first review's checkout lands and its reviewer comes up: still one.
    checkout_lands(&mut store, &mut herdr, "review/t1");
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
    assert!(temps_in_use(&mut store) <= 1, "the quota is never passed");
    assert_eq!(checkout_submissions(&herdr, "review/t2"), 0, "no second reviewer yet");

    // The first review finishes: its task settles and the reviewer is released
    // (pane closed, then gone), so its slot is free again.
    store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.state = gear_model::TaskState::Done;
            tx.update_task(&task)
        })
        .unwrap();
    for tick in 0..2 {
        gear_engine::release::release_finished_reviews(&mut store, &mut herdr, "p", T0 + 30 + tick).unwrap();
    }
    assert!(temps_in_use(&mut store) <= 1, "the released reviewer holds no slot");

    // The next waiting review takes the freed slot — one reviewer again.
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 40).unwrap();
    assert_eq!(temps_in_use(&mut store), 1, "one reviewer again, the one slot");
    assert_eq!(
        checkout_submissions(&herdr, "review/t2"),
        1,
        "the waiting review took the slot: {:?}",
        herdr.calls
    );
    assert_eq!(checkout_submissions(&herdr, "review/t3"), 0, "the third still waits");
}

/// T-116 (F2 of the review): a checkout that failed three times stays stuck
/// when its seat is recovered — a fresh generation must not restart the loop —
/// and the freed seat serves another task.
#[test]
fn a_stuck_checkout_is_not_restarted_by_a_recovered_seat() {
    // T-1's checkout fails; T-2 waits for a review and can use the seat.
    let mut store = handed_in_world(2);
    let mut herdr = FakeHerdr::new();

    // Three failed Git operations exhaust the attempt's retry budget.
    crate::REVIEW_GIT_FAIL.with(|fail| fail.set(true));
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 11).unwrap();
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 12).unwrap();
    crate::REVIEW_GIT_FAIL.with(|fail| fail.set(false));
    let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Unknown, "{:?}", seat.state_reason);
    let submissions = checkout_submissions(&herdr, "review/t1");
    assert_eq!(submissions, 0, "failure occurred before checkout");
    // The captain is told once, as information (T-370): the stuck review waits,
    // and the lane is not blocked, so nothing rings and no item is held.
    let told = store.read(|tx| tx.thread_messages("p", "T-1:1")).unwrap();
    let stuck: Vec<_> = told
        .iter()
        .filter(|message| message.body.contains("injected Git failure"))
        .collect();
    assert_eq!(stuck.len(), 1, "the captain is told once: {told:?}");
    assert_eq!(stuck[0].recipient, "p:captain");
    assert_eq!(stuck[0].kind, MsgKind::System);
    assert_eq!(stuck[0].state, MsgState::Info);
    let items = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 40)).unwrap();
    assert!(
        !items.iter().any(|message| message.body.contains("injected Git failure")),
        "nothing rings the captain: {items:?}"
    );

    // The dead seat is recovered (B02-04): its slot frees, but the stuck
    // attempt is not handed to it again — no fourth try for T-1.
    release_dead_temps(&mut store, &mut herdr, "p", T0 + 50).unwrap();
    assert_eq!(temps_in_use(&mut store), 0, "the dead seat holds nothing");
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 60).unwrap();
    assert_eq!(
        checkout_submissions(&herdr, "review/t1"),
        0,
        "a recovered seat does not restart the loop: {:?}",
        herdr.calls
    );

    // The freed seat serves the other task: T-2's checkout is submitted.
    assert_eq!(
        checkout_submissions(&herdr, "review/t2"),
        1,
        "the freed seat took T-2: {:?}",
        herdr.calls
    );
    assert_eq!(temps_in_use(&mut store), 1, "one reviewer, the one slot");
}

#[test]
fn cancelled_pending_start_frees_the_slot_for_the_next_review() {
    let mut store = handed_in_world(2);
    let mut herdr = FakeHerdr::new();
    herdr.refuse_starts = Some(("agent_pane_busy".into(), 1));
    start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
    let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Starting);
    assert_eq!(seat.task_id, None);
    store.write(|tx| { let mut task = tx.get_task("p", "T-1")?.unwrap(); task.state = TaskState::Done; tx.update_task(&task) }).unwrap();
    release_finished_reviews(&mut store, &mut herdr, "p", T0 + 11).unwrap();
    assert_eq!(store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap().state, SeatState::Released);
    assert_eq!(store.read(|tx| tx.get_meta("engine.review_pending.p:review-1")).unwrap().as_deref(), Some(""));
    let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 12).unwrap();
    assert_eq!(report.started, vec!["p:review-1"]);
    assert_eq!(store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap().task_id.as_deref(), Some("T-2"));
}

/// T-116: the engine's tick releases a dead seat **before** the reviews start,
/// so the review of T-1 goes in the same tick — the seat is free by then.
#[test]
fn the_tick_releases_a_dead_seat_before_the_reviews_start() {
    let mut store = world();
    dead_seat(&mut store, "review-1", "T-1");
    let mut herdr = FakeHerdr::new();

    let report = gear_engine::tick(&mut store, &mut herdr, &cfg(), T0 + 1).unwrap();

    assert_eq!(report.dead_temps, vec!["p:review-1".to_string()], "{report:?}");
    // The seat was released and taken by the review in the same tick: it runs
    // a reviewer now, and one reviewer is the one slot.
    let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
    assert_ne!(seat.state, SeatState::Unknown, "the dead seat is gone: {seat:?}");
    assert_eq!(seat.state, SeatState::Active, "the review took it");
    assert_eq!(temps_in_use(&mut store), 1, "one reviewer, the one slot");
    assert!(temps_in_use(&mut store) <= 1, "the quota is never passed");
    assert_eq!(checkout_submissions(&herdr, "review/t1"), 1, "the checkout runs in the same tick");
}

/// T-116: a **detached** worktree answer — a checkout whose branch was let go
/// of, so Herdr names the worktree without a branch — opens the review. The
/// seat is not left unknown.
#[test]
fn a_detached_worktree_answer_opens_the_review() {
    let mut store = world();
    let mut herdr = FakeHerdr::new();

    // A warm seat: released by its last job, with its own worktree kept.
    store
        .write(|tx| {
            let seat = testing::seed_temp_seat(tx, "p", "review-1", "T-0", "w3:p3", "codex", T0)?;
            tx.set_seat_state(&seat.id, SeatState::Released, Some("last job done"), T0)?;
            tx.set_binding_status(&seat.id, 1, BindingStatus::Void)?;
            tx.set_meta("engine.review_path.p:review-1./repo", "/fake-worktrees/home-review-1")?;
            Ok(())
        })
        .unwrap();
    // Its checkout is **detached**: Herdr's answer has no branch, which is
    // what a detached HEAD looks like (T-69 keeps the HEAD, lets the branch
    // go of).
    herdr.add_worktree("", "/fake-worktrees/home-review-1", "w3:p3");

    // The review opens on that worktree: the seat takes the job, the checkout
    // is submitted in the pane Herdr named for it.
    let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 10).unwrap();
    assert_eq!(report.started, vec!["p:review-1"], "{report:?}");
    let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Active, "the seat is not left unknown: {:?}", seat.state_reason);
    assert_eq!(seat.task_id.as_deref(), Some("T-1"));
    assert!(
        herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::WorktreeOpenPath { .. })),
        "the warm worktree is reopened by its path: {:?}",
        herdr.calls
    );
    assert!(
        !herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::WorktreeCreate { .. })),
        "no new worktree is built for a seat that has one: {:?}",
        herdr.calls
    );

    // The checkout lands and the reviewer starts.
    checkout_lands(&mut store, &mut herdr, "review/t1");
    let report = start_reviews(&mut store, &mut herdr, &cfg(), &cfg().profiles, "p", T0 + 20).unwrap();
    assert!(report.started.is_empty());
    assert_eq!(starts(&herdr).len(), 1, "the reviewer starts");
    assert_eq!(temps_in_use(&mut store), 1);
}
