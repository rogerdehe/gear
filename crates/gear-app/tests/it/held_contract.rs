//! T-536: a held task stays in its voyage's scope and counts (decision 04:45).
//!
//! A task is Draft only through `state: hold`, so Draft reads as held: the
//! station scope, the voyage totals, `gear report time` and the lane's queue
//! count it, and it is listed as held — but a lane cannot start it.

use gear_app::identity::Fingerprint;
use gear_app::queue;
use gear_app::time::{self, Filter};
use gear_app::{draft, plan, station, Env};
use gear_model::{AttemptState, GearCtx, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

fn captain() -> (Env, Fingerprint) {
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

fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "pi", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p3", "pi", T0)?;
            Ok(())
        })
        .unwrap();
    store
}

/// The voyage of the acceptance: three tasks, the third held through the plan.
const PLAN: &str = "<!-- gear-plan project=p base=0 -->\n# Milestone M1: the held voyage\ndone: it ships\nbar: keep it simple\neta: 4h\ndepart: now\n# Queue\n## NEW [must] lane=heat milestone=M1\ngoal: task one\naccept: a\nestimate: 10\ndeps: -\n\n## NEW [must] lane=heat milestone=M1\ngoal: task two\naccept: a\nestimate: 20\ndeps: -\n\n## NEW [should] lane=data milestone=M1\ngoal: task three\naccept: a\nestimate: 30\ndeps: -\nstate: hold\n";

/// Apply a plan as the captain.
fn apply(store: &mut Store, text: &str, rid: &str) -> gear_model::GearResult<plan::ApplyResult> {
    let mut draft = draft::parse(text).unwrap();
    // T-461: a first voyage's departure is the user's word.
    draft.confirmed = true;
    draft.decisions.push(
        "2026-11-01 10:00 confirmed: milestone M1 departs | why: the user said depart".to_string(),
    );
    let (env, fp) = captain();
    plan::apply(store, &env, Some(&fp), None, &draft, text, rid, T0)
}

fn task(id: &str, lane: &str, state: TaskState, estimate: i64) -> TaskRow {
    TaskRow {
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
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(estimate),
        issue: None,
        updated: T0,
    }
}

#[test]
fn the_station_scope_the_report_and_the_queue_count_the_held_task() {
    let mut store = world();
    apply(&mut store, PLAN, "r1").unwrap();
    // The held task is a Draft of lane data: it is the only task that lane has.
    let held = store.read(|tx| tx.get_task("p", "T-3")).unwrap().unwrap();
    assert_eq!(held.state, TaskState::Draft, "state: hold keeps it a draft");

    // The station counts all three tasks and their estimates.
    let ids = store
        .write(|tx| station::checkpoints(tx, "p", T0 + 3_600))
        .unwrap();
    assert_eq!(ids.len(), 1, "the 25% eta station: {ids:?}");
    let message = store
        .read(|tx| tx.get_message("p", &ids[0]))
        .unwrap()
        .unwrap();
    assert!(
        message.body.contains("3 tasks left in the voyage"),
        "{}",
        message.body
    );
    assert!(
        message.body.contains("tasks left 3 (60m)"),
        "{}",
        message.body
    );

    // `gear report time` counts all three.
    let report = store
        .read(|tx| {
            time::report(
                tx,
                "p",
                &Filter {
                    task: None,
                    voyage: Some("M1".into()),
                },
                T0 + 3_600,
            )
        })
        .unwrap();
    let listed: Vec<&str> = report.tasks.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(listed, vec!["T-1", "T-2", "T-3"], "{listed:?}");

    // The queue lists the held task as left work of its lane, and the lane is
    // not reported as having work it can start.
    let queue = store.read(|tx| queue::queue(tx, "p", T0 + 3_600)).unwrap();
    let data = queue
        .lanes
        .iter()
        .find(|lane| lane.lane == "data")
        .expect("the data lane");
    assert!(data.working.is_none(), "{data:?}");
    assert_eq!(
        data.idle.as_deref(),
        Some("nothing left planned"),
        "{data:?}"
    );
    assert_eq!(data.left.len(), 1, "{data:?}");
    assert_eq!(data.left[0].id, "T-3");
    assert!(data.left[0].held, "{:?}", data.left[0]);
    assert_eq!(data.left_min, 30);
    let heat = queue
        .lanes
        .iter()
        .find(|lane| lane.lane == "heat")
        .expect("the heat lane");
    assert_eq!(heat.left.len(), 2, "{heat:?}");
    assert!(heat.left.iter().all(|task| !task.held), "{heat:?}");
}

#[test]
fn the_queue_lists_a_held_task_as_held() {
    let mut store = world();
    store
        .write(|tx| {
            // The lane works on T-1 and has T-2 held behind it.
            tx.insert_task(&task("T-1", "heat", TaskState::Running, 10))?;
            tx.insert_task(&task("T-2", "heat", TaskState::Draft, 20))?;
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
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)
        })
        .unwrap();

    let queue = store.read(|tx| queue::queue(tx, "p", T0 + 600)).unwrap();
    let heat = queue
        .lanes
        .iter()
        .find(|lane| lane.lane == "heat")
        .expect("the heat lane");
    assert_eq!(heat.left.len(), 1, "{heat:?}");
    assert!(heat.left[0].held, "{:?}", heat.left[0]);
    let text = queue::render(&queue);
    let line = text
        .lines()
        .find(|line| line.starts_with("heat"))
        .expect("the heat lane line");
    assert!(line.contains("T-2 20m held"), "{line}");
}
