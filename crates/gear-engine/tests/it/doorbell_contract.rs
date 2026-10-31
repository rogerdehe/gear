//! Contract for gear_engine::doorbell (T-8, T-130). Pure functions.

use gear_engine::doorbell::{
    at_once, decide, goal_line, text, DoorbellAction, DoorbellInput, DoorbellItem,
};
use gear_model::{MsgKind, SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow, VoyageRow};

const NOW: i64 = 10_000;

fn item(id: &str) -> DoorbellItem {
    DoorbellItem {
        id: id.into(),
        urgent: false,
        at_once: false,
    }
}

fn input(seat: &str) -> DoorbellInput {
    DoorbellInput {
        project: "p".into(),
        seat: format!("p:{seat}"),
        generation: 2,
        pane_id: "w1:p1".into(),
        idle: true,
        status: Some(gear_herdr::AgentStatus::Idle),
        slot_busy: false,
        last_settled_at: None,
        no_task: false,
        waiting: vec![item("M-1")],
        goal: None,
    }
}

#[test]
fn rings_an_idle_seat_with_something_to_show() {
    let out = decide(&[input("captain")], 90, NOW);
    assert_eq!(
        out,
        vec![DoorbellAction {
            project: "p".into(),
            seat: "p:captain".into(),
            generation: 2,
            pane_id: "w1:p1".into(),
            text: "GEAR: 1 item(s) waiting for you: M-1. Run the command `gear` (no arguments) in your own shell; it shows only your items.".into(),
            covers: vec!["M-1".into()],
        }]
    );
}

#[test]
fn urgent_count_and_covers_order_without_duplicates() {
    let mut i = input("captain");
    i.waiting = vec![
        DoorbellItem {
            id: "M-3".into(),
            urgent: true,
            at_once: true,
        },
        DoorbellItem {
            id: "M-1".into(),
            urgent: false,
            at_once: false,
        },
        DoorbellItem {
            id: "M-7".into(),
            urgent: false,
            at_once: false,
        },
    ];
    let out = decide(&[i], 90, NOW);
    assert_eq!(out[0].covers, vec!["M-3", "M-1", "M-7"]);
    assert_eq!(
        out[0].text,
        "GEAR: 3 item(s) waiting for you (1 urgent): M-3 M-1 M-7. Run the command `gear` (no arguments) in your own shell; it shows only your items."
    );
}

#[test]
fn silent_when_nothing_busy_or_slot_held() {
    let mut nothing = input("a");
    nothing.waiting = vec![];
    let mut busy = input("b");
    busy.idle = false;
    busy.status = Some(gear_herdr::AgentStatus::Working);
    let mut slot = input("c");
    slot.slot_busy = true;
    assert_eq!(decide(&[nothing, busy, slot], 90, NOW), vec![]);
}

#[test]
fn cooldown_counts_from_last_settled_doorbell() {
    let mut early = input("a");
    early.last_settled_at = Some(NOW - 89);
    let mut exact = input("b");
    exact.last_settled_at = Some(NOW - 90);
    let out = decide(&[early, exact], 90, NOW);
    assert_eq!(
        out.iter().map(|a| a.seat.as_str()).collect::<Vec<_>>(),
        vec!["p:b"]
    );
}

#[test]
fn one_action_per_ringing_input_in_input_order() {
    let out = decide(&[input("z"), input("a")], 90, NOW);
    assert_eq!(
        out.iter().map(|a| a.seat.as_str()).collect::<Vec<_>>(),
        vec!["p:z", "p:a"]
    );
}

// ---------------------------------------------------------------- T-130

#[test]
fn a_working_seat_is_rung_at_once_for_an_at_once_message() {
    let mut working = input("lane");
    working.idle = false;
    working.status = Some(gear_herdr::AgentStatus::Working);
    working.last_settled_at = Some(NOW); // inside the cooldown
    working.waiting = vec![DoorbellItem {
        id: "M-4".into(),
        urgent: true,
        at_once: true,
    }];

    let out = decide(&[working], 90, NOW);
    assert_eq!(out.len(), 1, "typed while the agent works");
    assert_eq!(out[0].text, "GEAR: 1 item(s) waiting for you (1 urgent): M-4. Run the command `gear` (no arguments) in your own shell; it shows only your items.");
}

#[test]
fn a_working_seat_is_told_only_about_the_at_once_messages() {
    let mut working = input("lane");
    working.idle = false;
    working.status = Some(gear_herdr::AgentStatus::Working);
    working.waiting = vec![
        DoorbellItem {
            id: "M-4".into(),
            urgent: true,
            at_once: true,
        },
        DoorbellItem {
            id: "M-5".into(),
            urgent: false,
            at_once: false,
        },
    ];

    let out = decide(&[working], 90, NOW);
    assert_eq!(
        out[0].covers,
        vec!["M-4"],
        "a message that can wait is not typed into a running turn"
    );
}

#[test]
fn an_idle_seat_is_told_about_everything() {
    let mut idle = input("lane");
    idle.waiting = vec![
        DoorbellItem {
            id: "M-4".into(),
            urgent: true,
            at_once: true,
        },
        DoorbellItem {
            id: "M-5".into(),
            urgent: false,
            at_once: false,
        },
    ];

    let out = decide(&[idle], 90, NOW);
    assert_eq!(out[0].covers, vec!["M-4", "M-5"]);
}

#[test]
fn at_once_rule_names_the_two_ways_a_message_arrives_while_working() {
    use MsgKind::*;
    // urgent: any kind, any agent
    assert!(at_once(true, Question, false, false));
    assert!(at_once(true, Instruction, false, false));
    // a captain instruction for an agent that queues input
    assert!(at_once(false, Instruction, true, true));
    // the same instruction to an agent that does not queue: waits for idle
    assert!(!at_once(false, Instruction, true, false));
    // not a captain instruction
    assert!(!at_once(false, Instruction, false, true));
    assert!(!at_once(false, Question, true, true));
    assert!(!at_once(false, Reply, true, true));
    assert!(!at_once(false, Progress, true, true));
}

#[test]
fn the_text_names_every_id_and_the_command_to_run() {
    let items = vec![
        DoorbellItem {
            id: "M-12".into(),
            urgent: false,
            at_once: false,
        },
        DoorbellItem {
            id: "M-13".into(),
            urgent: false,
            at_once: false,
        },
    ];
    let said = text(&items, false);
    assert!(said.contains("M-12") && said.contains("M-13"), "{said}");
    assert!(
        said.contains(
            "Run the command `gear` (no arguments) in your own shell; it shows only your items."
        ),
        "{said}"
    );
    assert!(!said.contains('\n'), "one line: {said}");
}

/// T-337: the goal line is the one text every captain output uses
/// (`gear_board::stations::goal_text`), so it names the current station and how
/// far it is, besides the done and the bar.
const STATION_GOAL: &str = "every captain doorbell names the station";

/// A project whose voyage M1 departed, with `stations` and `tasks` ready work
/// in it. `None` stations is a voyage the plan does not break into stations.
fn voyage_store(stations: Option<&[&str]>, tasks: usize) -> Store {
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
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M1".into(),
                title: "a voyage".into(),
                done: Some("no red test on dev".into()),
                bar: Some("FASTEST".into()),
                eta: Some("4h".into()),
                departure: Some(NOW),
                departure_tasks: Some(tasks as i64),
                departure_estimate_min: Some(tasks as i64 * 30),
                updated: NOW,
            })?;
            if let Some(stations) = stations {
                let text = stations
                    .iter()
                    .enumerate()
                    .map(|(n, needs)| format!("S{}: {needs}", n + 1))
                    .collect::<Vec<_>>()
                    .join("\n");
                tx.set_meta("voyage-stations:p:M1", &text)?;
            }
            for n in 1..=tasks {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: format!("T-{n}"),
                    rank: n as i64,
                    state: TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("hull".into()),
                    milestone: Some("M1".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(30),
                    issue: None,
                    updated: NOW,
                })?;
                if stations.is_some() {
                    tx.set_meta(&format!("task-station:p:T-{n}"), "S1")?;
                }
            }
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn a_captain_doorbell_ends_with_the_station_and_how_far_it_is() {
    let mut store = voyage_store(Some(&[STATION_GOAL, "then the release"]), 2);
    let goal = store
        .read(|tx| goal_line(tx, "p"))
        .unwrap()
        .expect("a goal line");
    let mut captain = input("captain");
    captain.goal = Some(goal.clone());
    let out = decide(&[captain], 90, NOW);
    assert_eq!(
        out[0].text,
        format!(
            "GEAR: 1 item(s) waiting for you: M-1. Run the command `gear` (no arguments) in your own shell; it shows only your items.\nYour goal, voyage M1: done: no red test on dev · bar: FASTEST · station S1 (0 of 2 reached): {STATION_GOAL} · 2 tasks left in S1 · 2 tasks left in the voyage. Judge every decision against it."
        )
    );
}

#[test]
fn a_voyage_without_stations_ends_with_how_many_tasks_are_left() {
    let mut store = voyage_store(None, 3);
    let goal = store
        .read(|tx| goal_line(tx, "p"))
        .unwrap()
        .expect("a goal line");
    let mut captain = input("captain");
    captain.goal = Some(goal);
    let out = decide(&[captain], 90, NOW);
    assert!(
        out[0].text.ends_with("\nYour goal, voyage M1: done: no red test on dev · bar: FASTEST · 3 tasks left in the voyage. Judge every decision against it."),
        "{}",
        out[0].text
    );
}

#[test]
fn a_lane_doorbell_has_no_goal_line() {
    let mut store = voyage_store(Some(&[STATION_GOAL]), 2);
    let captain_goal = store.read(|tx| goal_line(tx, "p")).unwrap();
    assert!(captain_goal.is_some(), "the captain has one");
    // The engine passes the goal only for the captain's seat, so a lane's
    // input carries none.
    let out = decide(&[input("hull")], 90, NOW);
    assert!(!out[0].text.contains("Your goal"), "{}", out[0].text);
}

#[test]
fn a_project_with_no_departed_voyage_has_no_goal_line() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            // A voyage still on the plan: it has not departed.
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M1".into(),
                title: "a voyage".into(),
                done: Some("no red test on dev".into()),
                bar: Some("FASTEST".into()),
                eta: Some("4h".into()),
                departure: None,
                departure_tasks: Some(2),
                departure_estimate_min: Some(60),
                updated: NOW,
            })
        })
        .unwrap();
    assert_eq!(store.read(|tx| goal_line(tx, "p")).unwrap(), None);
    assert_eq!(
        store.read(|tx| goal_line(tx, "absent")).unwrap(),
        None,
        "no project at all"
    );
}

#[test]
fn a_captain_doorbell_repeats_the_voyage_goal() {
    let mut captain = input("captain");
    captain.goal = Some("Your goal, voyage M1: done: no red test on dev · bar: FASTEST. Judge every decision against it.".into());
    let out = decide(&[captain, input("hull")], 90, NOW);
    assert!(out[0].text.ends_with("\nYour goal, voyage M1: done: no red test on dev · bar: FASTEST. Judge every decision against it."), "{}", out[0].text);
    assert!(!out[1].text.contains("Your goal"), "{}", out[1].text);
}
