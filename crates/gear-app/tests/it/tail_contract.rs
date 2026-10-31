//! The captain's tail: a lane the captain silences, and the tail that ends
//! by itself (design §10.7 row B6).
//!
//! `- tail: <lanes>` in the plan's `# Decisions` silences a lane's dry/idle
//! warnings. With an end — `until=T-<n>` or `until=<RFC3339>` — the tail ends
//! by itself: the lanes work again when the task is over, or when the time has
//! come.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::{
    apply, lane_in_tail, lane_in_tail_at, parse_rfc3339, tail_until, tailed_lanes,
};
use gear_app::tasks::{on_delivery_confirmed, settle, start_attempt, submit_result, Verdict};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

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
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
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

/// Apply a plan whose only content is the given decision line.
fn apply_decision(s: &mut Store, decision: &str, rid: &str) -> gear_model::GearResult<()> {
    apply_decisions(s, &[decision], rid)
}

/// Apply a plan whose `# Decisions` carries the given lines — the history the
/// captain's plan file keeps.
fn apply_decisions(s: &mut Store, decisions: &[&str], rid: &str) -> gear_model::GearResult<()> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    let lines: String = decisions.iter().map(|d| format!("- {d}\n")).collect();
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Decisions\n{lines}");
    let (env, fp) = cap();
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(&text).unwrap(),
        &text,
        rid,
        NOW,
    )
    .map(|_| ())
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
    assert!(
        !in_tail(&mut s, "heat", NOW),
        "the tail is over: the lane works again"
    );
}

/// B6: a tail that waits for a moment ends at that moment.
#[test]
fn a_tail_until_a_time_ends_at_that_time() {
    let mut s = world();
    // 2026-10-26T18:00:00Z is 1793037600 (seconds since the epoch).
    apply_decision(&mut s, "tail: heat until=2026-10-26T18:00:00Z", "r1").unwrap();

    assert!(
        in_tail(&mut s, "heat", 1793037599),
        "a second before: still tailed"
    );
    assert!(
        !in_tail(&mut s, "heat", 1793037600),
        "the moment has come: the tail is over"
    );
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
    assert_eq!(
        s.write(|tx| tailed_lanes(tx, "p", NOW)).unwrap(),
        Vec::<String>::new()
    );

    apply_decision(&mut s, "tail: heat until=T-1", "r1").unwrap();
    assert_eq!(
        s.write(|tx| tailed_lanes(tx, "p", NOW)).unwrap(),
        vec!["heat".to_string()]
    );

    // And when the tail is over, the lane is named no more.
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)
    })
    .unwrap();
    assert_eq!(
        s.write(|tx| tailed_lanes(tx, "p", NOW)).unwrap(),
        Vec::<String>::new()
    );
}

/// A tail that names an unknown lane, an unknown task or a broken time is
/// refused: nothing is tailed.
#[test]
fn a_broken_tail_is_refused() {
    for (rid, decision, needle) in [
        ("a", "tail: nobody", "unknown lane nobody"),
        ("b", "tail: heat until=T-99", "unknown task T-99"),
        (
            "c",
            "tail: heat until=tomorrow",
            "needs a task id or an RFC3339 time",
        ),
        ("d", "tail: heat until=T-1 until=T-2", "takes one `until=`"),
        (
            "e",
            "tail: - until=T-1",
            "clears the tail, it takes no `until=`",
        ),
    ] {
        let mut s = world();
        let error = apply_decision(&mut s, decision, rid).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat, "{rid}: {}", error.detail);
        assert!(error.detail.contains(needle), "{rid}: {}", error.detail);
        assert!(!in_tail(&mut s, "heat", NOW), "{rid}: nothing was tailed");
    }
}

/// B6 (F1): a deadline may carry fractional seconds and a zone offset — the
/// stamps a serializer writes.
#[test]
fn a_deadline_may_carry_fractional_seconds_and_an_offset() {
    // 2026-10-26T18:00:00Z is 1793037600 (seconds since the epoch).
    assert_eq!(parse_rfc3339("2026-10-26T18:00:00Z"), Some(1793037600));
    // A zero fraction is that second.
    assert_eq!(parse_rfc3339("2026-10-26T18:00:00.000Z"), Some(1793037600));
    // A nonzero fraction rounds up: the tail must not end before the instant
    // the captain named.
    assert_eq!(parse_rfc3339("2026-10-26T18:00:00.123Z"), Some(1793037601));
    // An offset: +02:00 is two hours ahead of UTC.
    assert_eq!(parse_rfc3339("2026-10-26T20:00:00+02:00"), Some(1793037600));
    assert_eq!(parse_rfc3339("2026-10-26T16:00:00-02:00"), Some(1793037600));
}

/// F3: a day beyond the length of its month is not a date.
#[test]
fn a_day_beyond_the_month_is_refused() {
    assert_eq!(parse_rfc3339("2026-02-31T18:00:00Z"), None);
    assert_eq!(parse_rfc3339("2026-04-31T18:00:00Z"), None);
}

/// F3: the 29th of February exists only in a leap year.
#[test]
fn the_twenty_ninth_of_february_needs_a_leap_year() {
    assert_eq!(parse_rfc3339("2026-02-29T18:00:00Z"), None);
    // 2028 is a leap year: the day exists (1835460000).
    assert_eq!(parse_rfc3339("2028-02-29T18:00:00Z"), Some(1835460000));
}

/// B6 (F2): a tail that has ended is not revived by a task that is reopened
/// for a rework, and the same declaration replayed does not revive it either.
#[test]
fn a_tail_that_ended_is_not_revived_by_a_reopened_task() {
    let mut s = world();
    // The captain's `# Decisions` history: the lane is tailed, then the tail
    // gets an end. Both lines stay in the plan file.
    apply_decisions(&mut s, &["tail: heat", "tail: heat until=T-1"], "r1").unwrap();
    assert!(in_tail(&mut s, "heat", NOW));

    // The lane works on T-1 and hands its work in.
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, NOW + 1)?;
        submit_result(tx, "p", "T-1", 1, 1, "c1", "report", NOW + 2)
    })
    .unwrap();

    // The captain passes it: T-1 is done, and the tail it waited for is over.
    s.write(|tx| settle(tx, &verdict(true), NOW + 3)).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Done
    );
    // The path the engine reads the tail through writes the end once.
    assert!(!s.write(|tx| lane_in_tail(tx, "p", "heat")).unwrap());
    assert_eq!(
        s.read(|tx| tx.get_meta("lane-tail:p:heat"))
            .unwrap()
            .as_deref(),
        Some("0"),
        "the end is written"
    );

    // The captain's fail verdict reopens the task for a rework: the tail
    // stays off.
    s.write(|tx| settle(tx, &verdict(false), NOW + 4)).unwrap();
    assert_eq!(
        s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap().state,
        TaskState::Ready,
        "the task is reopened for a rework"
    );
    assert!(
        !in_tail(&mut s, "heat", NOW),
        "a reopened task does not revive a tail that ended"
    );

    // The same decisions, replayed (their lines are still in the plan file),
    // do not revive it either: the earlier line in the retained history never
    // clears the marker of the tail that ended.
    apply_decisions(&mut s, &["tail: heat", "tail: heat until=T-1"], "r2").unwrap();
    assert!(
        !in_tail(&mut s, "heat", NOW),
        "the retained history does not revive the tail"
    );
    assert_eq!(
        s.read(|tx| tx.get_meta("lane-tail:p:heat"))
            .unwrap()
            .as_deref(),
        Some("0"),
        "the lane stays untailed"
    );

    // A new declaration is a new tail.
    s.write(|tx| {
        tx.insert_task(&gear_store::TaskRow {
            project: "p".into(),
            id: "T-2".into(),
            rank: 2,
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
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            updated: NOW,
        })
    })
    .unwrap();
    apply_decision(&mut s, "tail: heat until=T-2", "r3").unwrap();
    assert!(
        in_tail(&mut s, "heat", NOW),
        "a new declaration is a new tail"
    );
}

/// The verdict the captain settles an attempt with.
fn verdict(pass: bool) -> Verdict {
    Verdict {
        project: "p".into(),
        task: "T-1".into(),
        attempt: 1,
        contract_version: 1,
        candidate: "c1".into(),
        pass,
        evidence: pass.then(|| "logs/v.md".to_string()),
        rework: (!pass).then(|| "edge cases".to_string()),
        integrated_by: None,
        by_seat: "p:captain".into(),
    }
}
