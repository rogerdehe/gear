//! The captain's tail: a lane the captain silences, and the tail that ends
//! by itself (design §10.7 row B6).
//!
//! `- tail: <lanes>` in the plan's `# Decisions` silences a lane's dry/idle
//! warnings. With an end — `until=T-<n>` or `until=<RFC3339>` — the tail ends
//! by itself: the lanes work again when the task is over, or when the time has
//! come.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::{apply, lane_in_tail_at, tailed_lanes, tail_until};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        tx.insert_task(&TaskRow {
            project: "p".into(),
            id: "T-1".into(),
            rank: 1,
            state: TaskState::Ready,
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
            updated: NOW,
        })?;
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

/// Apply a plan whose only content is the given decision line.
fn apply_decision(s: &mut Store, decision: &str, rid: &str) -> gear_model::GearResult<()> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Decisions\n- {decision}\n");
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), None, &parse(&text).unwrap(), &text, rid, NOW).map(|_| ())
}

fn in_tail(s: &mut Store, lane: &str, now: i64) -> bool {
    s.read(|tx| lane_in_tail_at(tx, "p", lane, now)).unwrap()
}

/// B6: a tail that waits for a task ends when that task is over.
#[test]
fn a_tail_until_a_task_ends_when_the_task_is_done() {
    let mut s = world();
    apply_decision(&mut s, "tail: heat until=T-1", "r1").unwrap();

    assert!(in_tail(&mut s, "heat", NOW), "the tail silences the lane");
    assert_eq!(
        s.read(|tx| tail_until(tx, "p")).unwrap(),
        Some(gear_app::plan::TailUntil::Task("T-1".into()))
    );

    // The task is done: the tail ends by itself.
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();
    assert!(!in_tail(&mut s, "heat", NOW), "the tail is over: the lane works again");
}

/// B6: a tail that waits for a moment ends at that moment.
#[test]
fn a_tail_until_a_time_ends_at_that_time() {
    let mut s = world();
    // 2026-10-26T18:00:00Z is 1793037600 (seconds since the epoch).
    apply_decision(&mut s, "tail: heat until=2026-10-26T18:00:00Z", "r1").unwrap();

    assert!(in_tail(&mut s, "heat", 1793037599), "a second before: still tailed");
    assert!(!in_tail(&mut s, "heat", 1793037600), "the moment has come: the tail is over");
    assert_eq!(
        s.read(|tx| tail_until(tx, "p")).unwrap(),
        Some(gear_app::plan::TailUntil::At(1793037600))
    );
}

/// A tail without an end lasts until the captain clears it.
#[test]
fn a_tail_without_an_end_lasts_until_it_is_cleared() {
    let mut s = world();
    apply_decision(&mut s, "tail: heat", "r1").unwrap();
    assert!(in_tail(&mut s, "heat", NOW));
    assert_eq!(s.read(|tx| tail_until(tx, "p")).unwrap(), None);

    // The captain clears it.
    apply_decision(&mut s, "tail: -", "r2").unwrap();
    assert!(!in_tail(&mut s, "heat", NOW), "the tail is cleared");
}

/// The lanes the tail silences are named (B5: the apply says which ones, so a
/// lane that gets no warning is not a mystery).
#[test]
fn the_tailed_lanes_are_named() {
    let mut s = world();
    assert_eq!(s.read(|tx| tailed_lanes(tx, "p", NOW)).unwrap(), Vec::<String>::new());

    apply_decision(&mut s, "tail: heat until=T-1", "r1").unwrap();
    assert_eq!(
        s.read(|tx| tailed_lanes(tx, "p", NOW)).unwrap(),
        vec!["heat".to_string()]
    );

    // And when the tail is over, the lane is named no more.
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();
    assert_eq!(s.read(|tx| tailed_lanes(tx, "p", NOW)).unwrap(), Vec::<String>::new());
}

/// A tail that names an unknown lane, an unknown task or a broken time is
/// refused: nothing is tailed.
#[test]
fn a_broken_tail_is_refused() {
    for (rid, decision, needle) in [
        ("a", "tail: nobody", "unknown lane nobody"),
        ("b", "tail: heat until=T-99", "unknown task T-99"),
        ("c", "tail: heat until=tomorrow", "needs a task id or an RFC3339 time"),
        ("d", "tail: heat until=T-1 until=T-2", "takes one `until=`"),
        ("e", "tail: - until=T-1", "clears the tail, it takes no `until=`"),
    ] {
        let mut s = world();
        let error = apply_decision(&mut s, decision, rid).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat, "{rid}: {}", error.detail);
        assert!(error.detail.contains(needle), "{rid}: {}", error.detail);
        assert!(!in_tail(&mut s, "heat", NOW), "{rid}: nothing was tailed");
    }
}
