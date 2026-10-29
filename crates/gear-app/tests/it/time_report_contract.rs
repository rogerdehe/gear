//! Contract for the time report (T-38): the captain reads where the time
//! went — per task and per voyage.

use gear_app::draft;
use gear_app::identity::Fingerprint;
use gear_app::plan;
use gear_app::review::{self, Outcome, Review};
use gear_app::tasks::{on_delivery_confirmed, settle_review, start_attempt, submit_result};
use gear_app::time::{self, Filter};
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const REVIEW_PANE: &str = "w1:p3";
const CANDIDATE: &str = "abc1234";

fn captain() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
    )
}

/// The world: a captain, the lane `heat` and a reviewer bound to T-1.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
        testing::seed_temp_seat(tx, "p", "r-1", "T-1", REVIEW_PANE, "codex", T0)?;
        Ok(())
    })
    .unwrap();
    s
}

/// Apply a plan as the captain.
fn apply(s: &mut Store, text: &str, rid: &str) -> gear_model::GearResult<plan::ApplyResult> {
    let draft = draft::parse(text).unwrap();
    let (env, fp) = captain();
    plan::apply(s, &env, Some(&fp), None, &draft, text, rid, T0)
}

/// The plan: one new task of the voyage M2, planned for 60 minutes.
const NEW_TASK: &str =
    "## NEW [must] lane=heat milestone=M2\ngoal: A\naccept: a\nestimate: 60\ndeps: -\n";

/// The voyage M2 the task sails on (T-333): a plan that names M2 carries its
/// `# Milestone` block.
const M2_BLOCK: &str = "# Milestone M2: delivery\ndone: it ships\nbar: it holds\neta: 8h\n\n";

/// T-171: the work and the waiting time of a voyage (§5.4): the lane's work,
/// its quota wait and its declared wait, the wall clock of the voyage and the
/// time the fleet was stopped by the quota.
#[test]
fn the_report_of_a_voyage_separates_work_from_waiting() {
    let mut s = world();
    apply(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{M2_BLOCK}# Queue\n{NEW_TASK}"),
        "r1",
    )
    .unwrap();
    s.write(|tx| start_attempt(tx, "p", "T-1", "p:heat", 1, T0)).unwrap();
    s.write(|tx| on_delivery_confirmed(tx, "p", "T-1", 1, T0 + 5 * 60)).unwrap();
    // The lane's round: half an hour of work.
    let key = gear_store::RoundKey {
        project: "p".into(),
        task: "T-1".into(),
        attempt: 1,
        role: "dev".into(),
    };
    s.write(|tx| {
        tx.insert_round(&gear_store::NewRound {
            key: key.clone(),
            seat: "p:heat".into(),
            generation: 1,
            cli: "opencode".into(),
            model: Some("gpt".into()),
            start_at: T0,
        })?;
        tx.set_round_active(&key, 30 * 60)?;
        // The quota stopped the lane for two hours, and it waited for a task
        // for twenty minutes.
        tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0)?;
        tx.close_pause("p", "p:heat", "quota", T0 + 2 * 3600)?;
        tx.open_pause("p", "p:heat", "wait", "T-9", T0 + 2 * 3600)?;
        tx.close_pause("p", "p:heat", "wait", T0 + 2 * 3600 + 20 * 60)?;
        Ok(())
    })
    .unwrap();

    let report = s
        .read(|tx| {
            time::report(
                tx,
                "p",
                &Filter { task: None, voyage: Some("M2".into()) },
                T0 + 4 * 3600,
            )
        })
        .unwrap();
    let work = report.work.as_ref().expect("the voyage's work time");
    assert_eq!(work.lanes.len(), 1, "{work:?}");
    let lane = &work.lanes[0];
    assert_eq!(lane.lane, "heat");
    assert_eq!(lane.work_s, 30 * 60, "the active time of the lane's rounds");
    assert_eq!(lane.quota_wait_s, 2 * 3600, "the quota pause");
    assert_eq!(lane.declared_wait_s, 20 * 60, "the declared wait");
    assert_eq!(lane.idle_s, 4 * 3600 - 30 * 60 - 2 * 3600 - 20 * 60, "the rest");
    // The wall clock of the voyage and the time the fleet was stopped.
    assert_eq!(work.elapsed_min, Some(240));
    assert_eq!(work.fleet_stopped_min, 120);
    assert_eq!(work.fleet_stops, 1);
    assert_eq!(work.elapsed_without_quota_min, Some(120));

    let text = time::render(&report);
    assert!(text.contains("Voyage M2 · elapsed 4h00m"), "{text}");
    assert!(text.contains("fleet stopped by quota 2h00m (1 stop)"), "{text}");
    assert!(text.contains("elapsed without quota stops 2h00m"), "{text}");
    assert!(
        text.contains("lane heat · work 0h30m · quota wait 2h00m · declared wait 0h20m"),
        "{text}"
    );
}

/// The report on a seeded store: the durations and the ratio.
#[test]
fn the_report_prints_the_durations_and_the_ratio() {
    let mut s = world();
    apply(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{M2_BLOCK}# Queue\n{NEW_TASK}"),
        "r1",
    )
    .unwrap();
    s.write(|tx| start_attempt(tx, "p", "T-1", "p:heat", 1, T0)).unwrap();
    s.write(|tx| on_delivery_confirmed(tx, "p", "T-1", 1, T0 + 5 * 60)).unwrap();
    s.write(|tx| {
        submit_result(tx, "p", "T-1", 1, 1, CANDIDATE, "Report: notes/T-1.md", T0 + 15 * 60)
    })
    .unwrap();
    s.write(|tx| {
        review::submit_review(
            tx,
            &Review {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                outcome: Outcome::Pass,
                findings: "F1: looks good".into(),
                blocking: String::new(),
                by_seat: "p:r-1".into(),
            },
            T0 + 20 * 60,
        )
    })
    .unwrap();
    s.write(|tx| settle_review(tx, "p", "T-1", 1, T0 + 25 * 60)).unwrap();

    let report = s
        .read(|tx| time::report(tx, "p", &Filter::default(), T0))
        .unwrap();
    let task = &report.tasks[0];
    assert_eq!(task.estimate_min, Some(60));
    assert_eq!(task.dev(), Some(10), "start → hand-in");
    assert_eq!(task.review(), Some(5), "hand-in → verdict");
    assert_eq!(task.wait(), Some(5), "verdict → settle");
    assert_eq!(task.merge(), None, "nothing is merged yet");
    assert_eq!(task.actual_min, Some(20), "the start of the work → the settle");
    assert_eq!(task.ratio(), Some(20.0 / 60.0));

    let text = time::render(&report);
    assert!(text.contains("T-1 · estimate 60m"), "{text}");
    assert!(text.contains("dev 10m · review 5m · wait 5m"), "{text}");
    assert!(text.contains("actual 20m"), "{text}");
    assert!(text.contains("ratio 0.33"), "{text}");
    // The voyage of the task is reported too.
    assert!(text.contains("Voyage M2 · 1 task · estimate 60m"), "{text}");
    assert!(text.contains("ratio 0.33"), "{text}");
}

/// The report is the captain's: a lane may not read it.
#[test]
fn only_the_captain_may_read_the_report() {
    let mut s = world();
    apply(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{M2_BLOCK}# Queue\n{NEW_TASK}"),
        "r1",
    )
    .unwrap();
    let lane = (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }),
            pane_id: Some(LANE_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None },
    );
    let err = time::build(&mut s, &lane.0, Some(&lane.1), &Filter::default(), T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden, "{}", err.detail);
}

/// A task that does not exist is `NoTarget`, not an empty report.
#[test]
fn an_unknown_task_is_no_target() {
    let mut s = world();
    apply(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{M2_BLOCK}# Queue\n{NEW_TASK}"),
        "r1",
    )
    .unwrap();
    let filter = Filter { task: Some("T-9".into()), voyage: None };
    let err = s.read(|tx| time::report(tx, "p", &filter, T0)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoTarget, "{}", err.detail);
}

/// A rejected first attempt has no merge; the successful rework supplies it.
#[test]
fn a_successful_rework_reports_its_merge_for_task_and_voyage() {
    use gear_model::AttemptState;
    use gear_store::{AttemptRow, AttemptStep};

    let mut s = world();
    apply(
        &mut s,
        &format!("<!-- gear-plan project=p base=0 -->\n{M2_BLOCK}# Queue\n{NEW_TASK}"),
        "r1",
    )
    .unwrap();
    s.write(|tx| {
        for (n, state, offset) in [
            (1, AttemptState::Rejected, 0),
            (2, AttemptState::Accepted, 30),
        ] {
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(), task: "T-1".into(), n, seat: "p:heat".into(),
                generation: 1, contract_version: 1, adopted_version: 1, pending_update: None,
                state, candidate: Some(CANDIDATE.into()), report: None, safe_point: None,
                started: T0 + offset * 60, updated: T0 + offset * 60,
            })?;
            for (step, minutes) in [
                (AttemptStep::Started, offset),
                (AttemptStep::HandedIn, offset + 10),
                (AttemptStep::Verdict, offset + 15),
                (AttemptStep::Settled, offset + 20),
            ] {
                tx.set_attempt_time("p", "T-1", n, step, T0 + minutes * 60)?;
            }
        }
        tx.set_attempt_time("p", "T-1", 2, AttemptStep::Merged, T0 + 55 * 60)?;
        Ok(())
    })
    .unwrap();

    let report = s.read(|tx| time::report(tx, "p", &Filter::default(), T0)).unwrap();
    let task = &report.tasks[0];
    assert_eq!(task.rounds[0].merge, None);
    assert_eq!(task.rounds[1].merge, Some(5));
    assert_eq!(task.merge(), Some(5));
    assert_eq!(report.voyages[0].merge, Some(5));
    assert_eq!(task.dev(), Some(10));
    assert_eq!(task.review(), Some(5));
    assert_eq!(task.wait(), Some(5));
    assert_eq!(task.rework_rounds, 1);
    assert_eq!(task.rework_min, Some(25));
    assert_eq!(task.actual_min, Some(55));
    assert_eq!(task.ratio(), Some(55.0 / 60.0));
    let text = time::render(&report);
    for prefix in ["T-1 ·", "Voyage M2 ·"] {
        let line = text.lines().find(|line| line.starts_with(prefix)).unwrap();
        assert!(line.contains("merge 5m"), "{line}");
        assert!(line.contains("rework 1 round (25m)"), "{line}");
        assert!(line.contains("actual 55m · ratio 0.92"), "{line}");
    }
}
