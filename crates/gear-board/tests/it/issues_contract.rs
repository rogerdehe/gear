//! T-649: the board's voyage view shows the voyage's labeled issues by state.

use gear_board::frame::{self, Tab};
use gear_board::{Board, Options};
use gear_model::{AttemptState, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, NewMessage, Store, TaskRow, VoyageIssueRow, VoyageRow,
};

const NOW: i64 = 1_800_000_000;

fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                "w1:p1",
                "codex",
                NOW,
            )?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p2", "pi", NOW)?;
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = "[github]\nrepo = \"o/r\"\n".into();
            tx.update_project(&project)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M2".into(),
                title: "voyage M2".into(),
                done: Some("it is delivered".into()),
                bar: None,
                eta: Some("8h".into()),
                departure: Some(NOW - 3600),
                departure_tasks: Some(7),
                departure_estimate_min: Some(70),
                updated: NOW - 3600,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

fn issue(store: &mut Store, number: i64, labeled: i64) {
    store
        .write(|tx| {
            tx.upsert_voyage_issue(&VoyageIssueRow {
                project: "p".into(),
                number,
                milestone: "M2".into(),
                title: format!("issue {number}"),
                body_hash: "hash".into(),
                labeled,
                first_seen: NOW,
                last_seen: NOW,
            })
        })
        .unwrap();
}

fn task(store: &mut Store, id: &str, rank: i64, state: TaskState, issue: i64) {
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: id.into(),
                rank,
                state,
                priority: "must".into(),
                lane: Some("hull".into()),
                milestone: Some("M2".into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(10),
                issue: Some(issue),
                updated: NOW,
            })
        })
        .unwrap();
}

fn attempt(store: &mut Store, id: &str, merged: bool) {
    store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: id.into(),
                n: 1,
                seat: "p:hull".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: None,
                report: None,
                safe_point: None,
                started: NOW - 600,
                updated: NOW - 300,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            tx.set_attempt_time("p", id, 1, AttemptStep::Started, NOW - 600)?;
            tx.set_attempt_time("p", id, 1, AttemptStep::Settled, NOW - 300)?;
            if merged {
                tx.set_attempt_time("p", id, 1, AttemptStep::Merged, NOW - 300)?;
            }
            Ok(())
        })
        .unwrap();
}

fn intake(store: &mut Store, number: i64) {
    store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: format!("intake:M2:#{number}"),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: MsgKind::System,
                urgent: false,
                body: "the issue".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Resolved,
                created: NOW,
            })
        })
        .unwrap();
}

fn board(store: &mut Store) -> Board {
    let options = Options {
        project: Some("p".into()),
        milestone: Some("M2".into()),
        ..Default::default()
    };
    gear_board::build(store, &options, NOW).unwrap()
}

fn now_view(board: &Board) -> String {
    frame::frame(board, &crate::screen(Tab::Now, 0, (160, 60)), None)
}

#[test]
fn the_voyage_view_lists_every_state_with_its_issues() {
    let mut store = world();
    // done #1
    task(&mut store, "T-1", 1, TaskState::Done, 1);
    attempt(&mut store, "T-1", true);
    issue(&mut store, 1, 1);
    // pending #3: no task, no intake
    issue(&mut store, 3, 1);
    // planned #4
    task(&mut store, "T-4", 4, TaskState::Ready, 4);
    issue(&mut store, 4, 1);
    // in progress #5
    task(&mut store, "T-5", 5, TaskState::Running, 5);
    attempt(&mut store, "T-5", false);
    issue(&mut store, 5, 1);
    // awaiting verification #6
    task(&mut store, "T-6", 6, TaskState::Done, 6);
    attempt(&mut store, "T-6", false);
    issue(&mut store, 6, 1);
    // deferred #7
    issue(&mut store, 7, 1);
    intake(&mut store, 7);
    // unlabeled #8
    issue(&mut store, 8, 0);

    let board = board(&mut store);
    let text = now_view(&board);
    assert!(text.contains("Issues (p/M2)"), "{text}");
    for line in [
        "pending #3",
        "planned #4",
        "in progress #5",
        "awaiting verification #6",
        "done #1",
        "deferred #7",
        "unlabeled #8",
    ] {
        assert!(text.contains(line), "missing {line:?}: {text}");
    }
}

#[test]
fn a_voyage_without_labeled_issues_shows_no_block() {
    let mut store = world();
    task(&mut store, "T-1", 1, TaskState::Done, 1);
    attempt(&mut store, "T-1", true);
    let board = board(&mut store);
    let text = now_view(&board);
    assert!(!text.contains("Issues"), "{text}");
}
