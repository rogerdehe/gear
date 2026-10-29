//! T-340: a frame depends only on the store and the `now` it was built at.
//!
//! The header's clock is the `now` the board was built with, never the wall
//! clock: the board is drawn twice with the same `now` and reads the same.
//! The `now` is 90 s behind the wall clock, so a build that still read the
//! clock would put another minute on the title's row.

use gear_board::frame::{canvas, Tab};
use gear_board::idle;
use gear_model::TaskState;
use gear_store::{testing, Store, TaskRow};

const NOW: i64 = 1_800_000_000;

/// Now, in epoch seconds.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// A project with one open task.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("hull".into()),
                milestone: Some("M4".into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(20),
                issue: None,
                updated: NOW,
            })?;
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn two_builds_with_the_same_now_draw_the_same_frame() {
    // 90 s apart on the wall clock: a frame that read the clock would differ.
    let now = now_seconds() - 90;
    let mut store = world();
    let first = gear_board::build(
        &mut store,
        &gear_board::Options {
            project: Some("p".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();
    let second = gear_board::build(
        &mut store,
        &gear_board::Options {
            project: Some("p".into()),
            ..Default::default()
        },
        now,
    )
    .unwrap();

    let first_frame = canvas(&first, &crate::screen(Tab::Now, 0, (120, 40)), None).text();
    let second_frame = canvas(&second, &crate::screen(Tab::Now, 0, (120, 40)), None).text();

    assert_eq!(first.now, now, "the board keeps the now it was built at");
    assert_eq!(
        first_frame, second_frame,
        "the frame depends on the store and now only"
    );
    let title = first_frame.lines().next().unwrap();
    let clock = idle::clock(now);
    assert!(
        title.trim_end().ends_with(&clock),
        "row 0 ends with the clock of now ({clock}): {title:?}"
    );
}
