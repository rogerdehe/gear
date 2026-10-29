//! The dependency view contract (T-14): the open tasks as a tree.
//!
//! The fixture is a small DAG: a diamond (T-1 is needed by T-2 and T-3,
//! T-4 needs T-2 and T-3) and one task that stands on its own (T-5).

use std::time::Duration;

use gear_board::{build, render, run_with, Event, Keys, Options, View};
use gear_model::{SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// A project with one lane, a diamond and one independent task.
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
                "w1:p1",
                "claude",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            for (id, rank, state, deps) in [
                ("T-1", 1, TaskState::Ready, vec![]),
                ("T-2", 2, TaskState::Ready, vec!["T-1"]),
                ("T-3", 3, TaskState::Ready, vec!["T-1"]),
                ("T-4", 4, TaskState::Ready, vec!["T-2", "T-3"]),
                ("T-5", 5, TaskState::Ready, vec![]),
            ] {
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
                    lane: Some("data".into()),
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
                    updated: T0,
                })?;
            }
            Ok(())
        })
        .unwrap();
    store
}

fn options() -> Options {
    Options {
        project: Some("p".into()),
        ..Options::default()
    }
}

#[test]
fn the_tree_draws_every_open_task_under_what_it_depends_on() {
    let mut store = world();
    let text = render(&build(&mut store, &options(), T0).unwrap(), View::Deps);

    let expected = [
        "Dependencies",
        "  T-1 queued",
        "    T-2 queued · waits T-1",
        "      T-4 queued · waits T-2 T-3",
        "    T-3 queued · waits T-1",
        "  T-5 queued",
    ]
    .join("\n");
    assert!(text.contains(&expected), "{text}");
}

#[test]
fn the_views_show_one_thing_at_a_time() {
    let mut store = world();
    let board = build(&mut store, &options(), T0).unwrap();

    let lanes = render(&board, View::Lanes);
    let deps = render(&board, View::Deps);

    assert!(lanes.contains("Lane data"), "{lanes}");
    assert!(!lanes.contains("Dependencies"), "{lanes}");
    assert!(deps.contains("Dependencies"), "{deps}");
    assert!(!deps.contains("Lane data"), "{deps}");
}

#[test]
fn a_finished_task_leaves_the_tree_and_its_children_come_up() {
    let mut store = world();
    // T-1 is done: it is not drawn, and T-2 and T-3 wait for nothing.
    store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-1").unwrap().unwrap();
            task.state = TaskState::Done;
            tx.update_task(&task)?;
            Ok(())
        })
        .unwrap();

    let text = render(&build(&mut store, &options(), T0).unwrap(), View::Deps);

    let expected = [
        "Dependencies",
        "  T-2 queued",
        "    T-4 queued · waits T-2 T-3",
        "  T-3 queued",
        "  T-5 queued",
    ]
    .join("\n");
    assert!(text.contains(&expected), "{text}");
    assert!(!text.contains("T-1"), "a finished task is not open: {text}");
}

/// Keys that press the given keys, one per call.
struct Presses(Vec<char>);

impl Keys for Presses {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        if self.0.is_empty() {
            return None;
        }
        Some(Event::Key(self.0.remove(0)))
    }

    fn size(&self) -> (usize, usize) {
        (120, 40)
    }
}

#[test]
fn the_right_arrow_moves_to_the_next_view() {
    let mut store = world();
    let opts = Options {
        project: Some("p".into()),
        refresh: Duration::from_millis(1),
        ..Options::default()
    };
    let mut out: Vec<u8> = Vec::new();
    let mut keys = Presses(vec!['2', 'q']);

    run_with(&mut store, &opts, &mut out, &mut keys, None).unwrap();

    let text = String::from_utf8(out).unwrap();
    // The second view is the graph of the work (T-157).
    assert!(text.contains("dev"), "{text}");
    assert!(text.contains("rev"), "{text}");
}

#[test]
fn the_json_carries_the_tree() {
    let mut store = world();
    let board = build(&mut store, &options(), T0).unwrap();
    let doc: serde_json::Value =
        serde_json::from_str(&gear_board::to_json(&board).unwrap()).unwrap();

    let deps = doc["projects"][0]["deps"].as_array().expect("the tree");
    assert_eq!(deps.len(), 5);
    // The drawn order: T-1, T-2, T-4 (under T-2), T-3, T-5.
    assert_eq!(deps[0]["id"], "T-1");
    assert_eq!(deps[0]["depth"], 0);
    assert_eq!(deps[2]["id"], "T-4");
    assert_eq!(deps[2]["depth"], 2);
    assert_eq!(deps[2]["waits"], serde_json::json!(["T-2", "T-3"]));
}
