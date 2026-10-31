//! Contract for gear_app::plan::apply, queue cut (T-36, design/05). Needs the
//! slice-2 store (T-33).

use gear_app::draft::{parse, Draft};
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, NewMessage, Store};

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
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
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

fn draft(base: i64, queue: &str) -> (Draft, String) {
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Queue\n{queue}");
    (parse(&text).unwrap(), text)
}

fn run(
    s: &mut Store,
    d: &(Draft, String),
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &d.0, &d.1, rid, NOW)
}

const TWO_NEW: &str = "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [should] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: T-1\n";

#[test]
fn new_tasks_get_ids_rank_and_contract_v1() {
    let mut s = world();
    let r = run(&mut s, &draft(0, TWO_NEW), "r1").unwrap();
    assert_eq!(
        (r.version, r.created.clone()),
        (1, vec!["T-1".to_string(), "T-2".to_string()])
    );
    let tasks = s.read(|tx| tx.tasks_of_project("p")).unwrap();
    assert_eq!(
        tasks
            .iter()
            .map(|t| (t.id.as_str(), t.rank, t.state))
            .collect::<Vec<_>>(),
        vec![("T-1", 1, TaskState::Ready), ("T-2", 2, TaskState::Ready)]
    );
    assert_eq!(tasks[1].deps, vec!["T-1".to_string()]);
    assert_eq!(
        s.read(|tx| tx.get_contract("p", "T-2", 1))
            .unwrap()
            .unwrap()
            .goal,
        "B"
    );
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 1);
}

#[test]
fn stale_base_and_replay() {
    let mut s = world();
    let d = draft(0, TWO_NEW);
    let first = run(&mut s, &d, "r1").unwrap();
    assert_eq!(
        run(&mut s, &d, "r1").unwrap(),
        first,
        "same request id + text replays"
    );
    assert_eq!(
        run(&mut s, &d, "r2").unwrap_err().code,
        ErrorCode::StaleVersion
    );
}

#[test]
fn reorder_change_contract_and_unmentioned_keep_order() {
    let mut s = world();
    run(
        &mut s,
        &draft(
            0,
            &format!(
                "{TWO_NEW}\n## NEW [could] lane=heat\ngoal: C\naccept: c\nestimate: 30\ndeps: -\n"
            ),
        ),
        "r1",
    )
    .unwrap();
    // v1 → v2: mention only T-3 (first) and T-1 (goal changed); T-2 not mentioned
    let r = run(&mut s, &draft(1, "## T-3 [now] lane=heat\ngoal: C\naccept: c\ndeps: -\n\n## T-1 [must] lane=heat\ngoal: A2\naccept: a\ndeps: -\n"), "r2").unwrap();
    assert_eq!(r.contract_changed, vec![("T-1".to_string(), 2)]);
    assert_eq!(r.not_mentioned, vec!["T-2".to_string()]);
    let order: Vec<String> = s
        .read(|tx| tx.tasks_of_project("p"))
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(order, vec!["T-3", "T-1", "T-2"]);
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .contract_version,
        2
    );
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-3"))
            .unwrap()
            .unwrap()
            .contract_version,
        1,
        "unchanged contract keeps its version"
    );
}

#[test]
fn invalid_drafts_write_nothing() {
    let mut s = world();
    run(&mut s, &draft(0, TWO_NEW), "r1").unwrap();
    for (rid, q) in [
        (
            "a",
            "## T-9 [must] lane=heat\ngoal: x\naccept: x\ndeps: -\n",
        ), // unknown task
        (
            "b",
            "## NEW [must]\ngoal: x\naccept: x\nestimate: 30\ndeps: -\n",
        ), // NEW without lane
        (
            "c",
            "## T-1 [must] lane=heat\ngoal: A\naccept: a\ndeps: T-2\n",
        ), // cycle T-1 ↔ T-2
        (
            "d",
            "## T-1 [must] lane=heat\ngoal: A\naccept: a\ndeps: T-7\n",
        ), // unknown dep
        (
            "e",
            "## T-1 [must] lane=heat\ngoal: A\naccept: a\ndeps: -\nstate: ready\n",
        ), // Ready → ready not allowed
    ] {
        assert_eq!(
            run(&mut s, &draft(1, q), rid).unwrap_err().code,
            ErrorCode::BadFormat,
            "{rid}"
        );
    }
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 1);
    let staff = "<!-- gear-plan project=p base=1 -->\n# Staff\n- temp R-4 task=T-1 profile=opencode-review\n"; // temp workers are not in slice 2 (T-55)
    let (env, fp) = cap();
    assert_eq!(
        apply(
            &mut s,
            &env,
            Some(&fp),
            None,
            &parse(staff).unwrap(),
            staff,
            "f",
            NOW
        )
        .unwrap_err()
        .code,
        ErrorCode::BadFormat
    );
}

#[test]
fn cancel_and_lane_cannot_apply() {
    let mut s = world();
    run(&mut s, &draft(0, TWO_NEW), "r1").unwrap();
    run(
        &mut s,
        &draft(
            1,
            "## T-2 [should] lane=heat\ngoal: B\naccept: b\ndeps: T-1\nstate: cancelled\n",
        ),
        "r2",
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-2")).unwrap().unwrap().state,
        TaskState::Cancelled
    );
    let lane = Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "heat".into(),
            generation: 1,
        }),
        pane_id: Some("w1:p2".into()),
        admin: false,
    };
    let lfp = Fingerprint {
        pane_id: "w1:p2".into(),
        kind: Some("opencode".into()),
        session_ref: None,
    };
    let d = draft(2, "");
    assert_eq!(
        apply(&mut s, &lane, Some(&lfp), None, &d.0, &d.1, "r3", NOW)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
}

// ---- the lanes that are about to stand idle (T-58, gear/voyages/M2.1/lessons.md 7a)

/// A task of the project: its lane, its state and the tasks it waits for.
fn task(id: &str, lane: &str, state: TaskState, deps: &[&str]) -> gear_store::TaskRow {
    gear_store::TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank: 1,
        state,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: deps.iter().map(|dep| dep.to_string()).collect(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: None,
        issue: None,
        updated: NOW,
    }
}

/// The lanes that are about to stand idle: the lane and its current task
/// (None when it has none).
fn idle(s: &mut Store) -> Vec<(String, Option<String>, Vec<String>)> {
    s.read(|tx| gear_app::plan::idle_lanes(tx, "p", NOW))
        .unwrap()
        .into_iter()
        .map(|lane| (lane.lane, lane.current, lane.blocked))
        .collect()
}

/// A lane working on its last task, with nothing queued behind it, is told
/// before it stands idle.
#[test]
fn a_lane_with_nothing_behind_its_current_task_is_told() {
    let mut s = world();
    s.write(|tx| {
        // The lane "heat" is working on T-2; the task queued behind it waits
        // for T-1, which another lane is still working on.
        tx.insert_task(&task("T-2", "heat", TaskState::Running, &[]))?;
        tx.insert_task(&task("T-3", "heat", TaskState::Ready, &["T-1"]))?;
        tx.insert_task(&task("T-1", "wire", TaskState::Running, &[]))
    })
    .unwrap();

    assert_eq!(
        idle(&mut s),
        vec![(
            "heat".to_string(),
            Some("T-2".to_string()),
            vec!["T-3".to_string()]
        )],
        "nothing startable behind the current task"
    );
}

/// An idle lane whose only queued task waits for a dependency is told too.
#[test]
fn an_idle_lane_that_waits_for_a_dependency_is_told() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&task("T-2", "heat", TaskState::Ready, &["T-1"]))?;
        tx.insert_task(&task("T-1", "wire", TaskState::Running, &[]))
    })
    .unwrap();

    assert_eq!(
        idle(&mut s),
        vec![("heat".to_string(), None, vec!["T-2".to_string()])],
        "the queued task waits for a dependency"
    );
}

/// A lane with a task it can start now is not told: it has work.
#[test]
fn a_lane_with_work_to_start_is_not_told() {
    let mut s = world();
    s.write(|tx| {
        // The dependency is done: the task can start now.
        tx.insert_task(&task("T-2", "heat", TaskState::Ready, &["T-1"]))?;
        tx.insert_task(&task("T-1", "wire", TaskState::Done, &[]))
    })
    .unwrap();

    assert_eq!(idle(&mut s), vec![]);
}

/// A lane that is working on a task with a startable one queued behind it is
/// not told: the next task waits for it.
#[test]
fn a_lane_with_a_startable_task_behind_its_current_one_is_not_told() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&task("T-2", "heat", TaskState::Running, &[]))?;
        tx.insert_task(&task("T-3", "heat", TaskState::Ready, &[]))
    })
    .unwrap();

    assert_eq!(idle(&mut s), vec![]);
}

/// A lane that helps on an unanswered instruction of the captain is busy,
/// not idle: it is not told it stands idle (T-323).
#[test]
fn a_helping_lane_is_not_told() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_message(&NewMessage {
            meta: None,
            project: "p".into(),
            thread: "p:c".into(),
            task: None,
            attempt: None,
            sender: "p:captain".into(),
            sender_gen: 1,
            recipient: "p:heat".into(),
            kind: MsgKind::Instruction,
            urgent: false,
            body: "Check the quota line".into(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: NOW - 60,
        })
        .map(|_| ())
    })
    .unwrap();

    assert_eq!(idle(&mut s), vec![]);
}

/// A lane in the captain's tail may stand idle: it was declared as finishing.
#[test]
fn a_tail_lane_is_not_told() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&task("T-2", "heat", TaskState::Ready, &["T-1"]))?;
        tx.insert_task(&task("T-1", "wire", TaskState::Running, &[]))?;
        tx.set_meta("lane-tail:p:heat", "1")
    })
    .unwrap();

    assert_eq!(idle(&mut s), vec![]);
}

/// The warning the captain reads, one line per lane.
#[test]
fn the_warning_names_the_lane_and_what_waits() {
    let lane = gear_app::plan::IdleLane {
        lane: "heat".into(),
        current: Some("T-2".into()),
        blocked: vec!["T-5".into(), "T-6".into()],
    };
    assert_eq!(
        lane.warning(),
        "warning: lane heat has nothing to start after T-2 (2 queued behind deps: T-5, T-6)"
    );

    let idle_now = gear_app::plan::IdleLane {
        lane: "heat".into(),
        current: None,
        blocked: vec![],
    };
    assert_eq!(
        idle_now.warning(),
        "warning: lane heat has nothing to start after now (0 queued behind deps)"
    );
}
