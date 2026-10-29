//! T-208: the information line of an overlap with the integration branch.
//!
//! The engine's scan (T-188) writes the meta keys
//! `parallel:<project>[:<repo>]:integration:<task>`. The board shows them as
//! information lines: they never wait for the captain, and a cleared key
//! shows nothing.

use gear_board::{build, frame, integration_overlaps, render, Options, View};
use gear_model::SeatRole;
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

/// A project with a captain and one lane.
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
            Ok(())
        })
        .unwrap();
    store
}

/// The overlap of a task, as the scan writes it.
fn seed_overlap(store: &mut Store, key: &str, task: &str, integration: &str, files: &[&str]) {
    let value = serde_json::json!({
        "task": task,
        "integration": integration,
        "files": files,
    })
    .to_string();
    store.write(|tx| tx.set_meta(key, &value)).unwrap();
}

fn options() -> Options {
    Options {
        project: Some("p".into()),
        ..Options::default()
    }
}

#[test]
fn one_overlap_is_one_line_on_the_board_and_in_the_frame() {
    let mut store = world();
    seed_overlap(
        &mut store,
        "parallel:p:integration:T-2",
        "T-2",
        "dev",
        &["crates/gear-board/src/lib.rs"],
    );

    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = render(&board, View::Lanes);
    assert!(
        text.contains("overlap: T-2 shares crates/gear-board/src/lib.rs with dev"),
        "{text}"
    );

    // The interactive frame shows the same line.
    let screen = frame::frame(&board, &crate::screen(frame::Tab::Now, 0, (120, 40)), None);
    assert!(
        screen.contains("overlap: T-2 shares crates/gear-board/src/lib.rs with dev"),
        "{screen}"
    );

    // The overlaps are the ones the scan wrote.
    let overlaps = integration_overlaps(&mut store, "p").unwrap();
    assert_eq!(overlaps.len(), 1);
    assert_eq!(overlaps[0].task, "T-2");
    assert_eq!(
        overlaps[0].files,
        vec!["crates/gear-board/src/lib.rs".to_string()]
    );
}

#[test]
fn an_overlap_of_a_child_repo_names_its_repo() {
    let mut store = world();
    seed_overlap(
        &mut store,
        "parallel:p:web:integration:T-3",
        "T-3",
        "dev",
        &["a.rs"],
    );

    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = render(&board, View::Lanes);
    assert!(text.contains("overlap: T-3 shares a.rs with dev"), "{text}");

    let overlaps = integration_overlaps(&mut store, "p").unwrap();
    assert_eq!(overlaps.len(), 1);
    assert_eq!(overlaps[0].repo, "web");
    assert_eq!(overlaps[0].integration, "dev");
}

#[test]
fn a_cleared_overlap_shows_nothing() {
    let mut store = world();
    seed_overlap(
        &mut store,
        "parallel:p:integration:T-2",
        "T-2",
        "dev",
        &["a.rs"],
    );
    // The scan clears the key when the overlap is gone.
    store
        .write(|tx| tx.set_meta("parallel:p:integration:T-2", ""))
        .unwrap();

    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = render(&board, View::Lanes);
    assert!(!text.contains("overlap:"), "{text}");

    let screen = frame::frame(&board, &crate::screen(frame::Tab::Now, 0, (120, 40)), None);
    assert!(!screen.contains("overlap:"), "{screen}");

    assert!(integration_overlaps(&mut store, "p").unwrap().is_empty());
}

#[test]
fn an_overlap_never_waits_for_the_captain() {
    let mut store = world();
    seed_overlap(
        &mut store,
        "parallel:p:integration:T-2",
        "T-2",
        "dev",
        &["a.rs"],
    );

    let board = build(&mut store, &options(), T0 + 60).unwrap();
    // The captain's inbox is empty: an overlap is an information line.
    assert_eq!(board.projects[0].waiting.count, 0);
    let text = render(&board, View::Lanes);
    assert!(text.contains("Waiting for the captain: 0 items"), "{text}");
}

/// F1 of the review of T-208: the bookkeeping keys of the scan are not
/// overlaps. The old scan kept `pair:integration:<task>:files` and
/// `pair:integration:<task>:item` when it resolved the legacy captain item,
/// so a cleared overlap key must still print nothing.
#[test]
fn the_bookkeeping_keys_of_the_scan_are_not_overlaps() {
    let mut store = world();
    store
        .write(|tx| {
            // The overlap is gone: the scan cleared its key.
            tx.set_meta("parallel:p:integration:T-2", "")?;
            // The bookkeeping of the legacy item stays.
            tx.set_meta("parallel:p:pair:integration:T-2:files", "[]")?;
            tx.set_meta("parallel:p:pair:integration:T-2:item", "M-9")?;
            Ok(())
        })
        .unwrap();

    let overlaps = integration_overlaps(&mut store, "p").unwrap();
    assert!(overlaps.is_empty(), "{overlaps:?}");

    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = render(&board, View::Lanes);
    assert!(!text.contains("overlap:"), "{text}");

    let screen = frame::frame(&board, &crate::screen(frame::Tab::Now, 0, (120, 40)), None);
    assert!(!screen.contains("overlap:"), "{screen}");
}
