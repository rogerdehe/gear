//! The voyage clock under the user's hand: a paused project's hours do not
//! count in its voyage's elapsed time, and a voyage can depart again with its
//! time stations starting from zero.

use gear_app::voyage_clock::{redepart, resume};
use gear_model::TaskState;
use gear_store::{testing, Store, TaskRow, Tx, VoyageRow};

const DEPART: i64 = 1_800_000_000;

fn task(id: &str, state: TaskState) -> TaskRow {
    TaskRow {
        project: "p".into(),
        id: id.into(),
        rank: 0,
        state,
        priority: "must".into(),
        lane: None,
        milestone: Some("M1".into()),
        required: true,
        contract_version: 1,
        deps: Vec::new(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(30),
        issue: None,
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        updated: DEPART,
    }
}

fn world(states: &[TaskState]) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", DEPART)?;
        for (n, state) in states.iter().enumerate() {
            tx.insert_task(&task(&format!("T-{}", n + 1), *state))?;
        }
        tx.upsert_voyage(&VoyageRow {
            project: "p".into(),
            milestone: "M1".into(),
            title: "t".into(),
            done: None,
            bar: None,
            eta: Some("10h".into()),
            departure: Some(DEPART),
            departure_tasks: Some(1),
            departure_estimate_min: Some(30),
            updated: DEPART,
        })
    })
    .unwrap();
    s
}

fn departure(s: &mut Store) -> i64 {
    s.read(|tx| tx.get_voyage("p", "M1"))
        .unwrap()
        .unwrap()
        .departure
        .unwrap()
}

#[test]
fn the_hours_a_project_spent_paused_do_not_count_in_its_elapsed_time() {
    let mut s = world(&[TaskState::Done, TaskState::Ready]);
    let paused = DEPART + 2 * 3600;
    let back = paused + 20 * 3600;
    s.write(|tx| tx.set_meta(&Tx::project_paused_key("p"), &format!("{paused} the user")))
        .unwrap();
    let moved = s.write(|tx| resume(tx, "p", back)).unwrap();
    assert_eq!(moved, vec![("M1".to_string(), 20 * 60)]);
    assert_eq!(
        back - departure(&mut s),
        2 * 3600,
        "only the sailing hours count"
    );
    assert!(s.read(|tx| tx.project_paused("p")).unwrap().is_none());
}

#[test]
fn a_finished_voyage_keeps_its_departure_on_resume() {
    let mut s = world(&[TaskState::Done]);
    s.write(|tx| tx.set_meta(&Tx::project_paused_key("p"), &format!("{} x", DEPART + 60)))
        .unwrap();
    let moved = s.write(|tx| resume(tx, "p", DEPART + 7200)).unwrap();
    assert!(moved.is_empty());
    assert_eq!(departure(&mut s), DEPART);
}

#[test]
fn a_voyage_departs_again_with_its_time_stations_from_zero() {
    let mut s = world(&[
        TaskState::Done,
        TaskState::Done,
        TaskState::Done,
        TaskState::Ready,
    ]);
    let key = "station:p:M1";
    s.write(|tx| {
        tx.set_meta(&format!("{key}:course"), "21")?;
        tx.set_meta(&format!("{key}:eta:1"), "M-1")?;
        tx.set_meta(&format!("{key}:eta:2"), "M-2")?;
        tx.set_meta(&format!("{key}:done"), "0")
    })
    .unwrap();
    let now = DEPART + 21 * 3600;
    let (tasks, estimate) = s.write(|tx| redepart(tx, "p", "M1", now)).unwrap();
    assert_eq!((tasks, estimate), (4, 120));
    let voyage = s.read(|tx| tx.get_voyage("p", "M1")).unwrap().unwrap();
    assert_eq!(voyage.departure, Some(now));
    assert_eq!(voyage.departure_tasks, Some(4));
    assert_eq!(voyage.departure_estimate_min, Some(120));
    s.read(|tx| {
        assert!(tx.get_meta(&format!("{key}:course"))?.is_none());
        assert!(tx.meta_keys_with_prefix(&format!("{key}:eta:"))?.is_empty());
        // The three done tasks do not ring "3 done" again.
        assert_eq!(tx.get_meta(&format!("{key}:done"))?.as_deref(), Some("1"));
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_voyage_redeparted_while_paused_starts_counting_at_the_resume() {
    let mut s = world(&[TaskState::Ready]);
    let paused = DEPART + 3600;
    s.write(|tx| tx.set_meta(&Tx::project_paused_key("p"), &format!("{paused} x")))
        .unwrap();
    let reset = paused + 10 * 3600;
    s.write(|tx| redepart(tx, "p", "M1", reset)).unwrap();
    let back = reset + 5 * 3600;
    s.write(|tx| resume(tx, "p", back)).unwrap();
    assert_eq!(departure(&mut s), back);
}
