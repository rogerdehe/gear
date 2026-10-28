//! Contract for gear_engine::dispatch::lane_view (T-41). Needs the slice-2
//! store incl. `open_attempts_of_seat` (T-40).

use gear_engine::dispatch::lane_view;
use gear_model::{AttemptState, OpKind, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, NewOp, SeatRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

fn task(id: &str, rank: i64, lane: &str, state: TaskState, prio: &str, deps: &[&str]) -> TaskRow {
    TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state,
        priority: prio.into(),
        lane: Some(lane.into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: deps.iter().map(|d| d.to_string()).collect(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: None,
        issue: None,
        updated: T0,
    }
}

fn attempt(t: &str, state: AttemptState) -> AttemptRow {
    AttemptRow {
        session_ref: None,
        session_cli: None,
        session_action: None,
        sent_at: None,
        project: "p".into(),
        task: t.into(),
        n: 1,
        seat: "p:heat".into(),
        generation: 1,
        contract_version: 1,
        adopted_version: 1,
        pending_update: None,
        state,
        candidate: None,
        report: None,
        safe_point: None,
        started: T0,
        updated: T0,
    }
}

fn world(tasks: &[TaskRow], attempts: &[AttemptRow]) -> (Store, SeatRow) {
    let mut s = Store::open_in_memory().unwrap();
    let seat = s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            let seat = testing::seed_active_seat(
                tx,
                "p",
                "heat",
                SeatRole::Lane,
                "w1:p2",
                "opencode",
                T0,
            )?;
            for t in tasks {
                tx.insert_task(t)?;
            }
            for a in attempts {
                tx.insert_attempt(a)?;
            }
            Ok(seat)
        })
        .unwrap();
    (s, seat)
}

#[test]
fn ready_tasks_are_this_lanes_ready_with_done_deps_in_rank_order() {
    let (mut s, seat) = world(
        &[
            task("T-1", 1, "heat", TaskState::Done, "must", &[]),
            task("T-2", 4, "heat", TaskState::Ready, "must", &["T-1"]),
            task("T-3", 2, "heat", TaskState::Ready, "should", &[]),
            task("T-4", 3, "heat", TaskState::Ready, "must", &["T-9"]), // dep not done
            task("T-9", 9, "wire", TaskState::Ready, "must", &[]),      // other lane
            task("T-5", 5, "heat", TaskState::Ready, "wont", &[]),      // wont
            task("T-6", 6, "heat", TaskState::Draft, "must", &[]),      // not ready
        ],
        &[attempt("T-1", AttemptState::Accepted)],
    );
    // T-1 is Done, so its work is merged (T-236).
    s.write(|tx| tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, T0))
        .unwrap();
    let v = s.read(|tx| lane_view(tx, &seat, None, T0)).unwrap();
    assert_eq!(
        v.ready_tasks
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        vec!["T-3", "T-2"]
    );
    assert_eq!(
        (
            v.attempt.clone(),
            v.pending_op.clone(),
            v.last_deliver_at,
            v.nudges_since_progress,
            v.safe_for_next
        ),
        (None, None, None, 0, false)
    );
}

/// T-236 (F-76): Done is set at the review pass, before the merge reaches the
/// integration branch, so a dependency counts only once its work is merged.
#[test]
fn a_done_dependency_counts_only_once_its_work_is_merged() {
    let (mut s, seat) = world(
        &[
            task("T-1", 1, "heat", TaskState::Done, "must", &[]),
            task("T-2", 2, "heat", TaskState::Ready, "must", &["T-1"]),
        ],
        &[attempt("T-1", AttemptState::Accepted)],
    );
    let ready = |s: &mut Store| {
        s.read(|tx| lane_view(tx, &seat, None, T0))
            .unwrap()
            .ready_tasks
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>()
    };
    assert!(
        ready(&mut s).is_empty(),
        "a Done dependency whose work is not merged does not count"
    );
    s.write(|tx| tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, T0 + 60))
        .unwrap();
    assert_eq!(
        ready(&mut s),
        vec!["T-2".to_string()],
        "the merged dependency releases its dependant"
    );
}

#[test]
fn open_attempt_pending_op_and_last_delivery() {
    let (mut s, seat) = world(
        &[task("T-1", 1, "heat", TaskState::Running, "must", &[])],
        &[attempt("T-1", AttemptState::Started)],
    );
    s.write(|tx| {
        for kind in [OpKind::Deliver, OpKind::Doorbell] {
            tx.insert_op(&NewOp {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                kind,
                task: Some("T-1".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({"text": "x"}),
                created: T0 + 5,
            })?;
        }
        Ok(())
    })
    .unwrap();
    let v = s.read(|tx| lane_view(tx, &seat, None, T0 + 10)).unwrap();
    assert_eq!(v.attempt.map(|a| a.task), Some("T-1".to_string()));
    assert_eq!(v.pending_op.map(|o| o.id), Some("O-1".to_string()));
    assert_eq!(v.last_deliver_at, Some(T0 + 5));
}

#[test]
fn waiting_task_blocks_continue_and_assign() {
    let (mut s, seat) = world(
        &[
            task("T-1", 1, "heat", TaskState::Waiting, "must", &[]),
            task("T-2", 2, "heat", TaskState::Ready, "must", &[]),
        ],
        &[attempt("T-1", AttemptState::Stopped)],
    );
    let v = s.read(|tx| lane_view(tx, &seat, None, T0)).unwrap();
    assert!(v.attempt.is_none() && v.ready_tasks.is_empty());
}
