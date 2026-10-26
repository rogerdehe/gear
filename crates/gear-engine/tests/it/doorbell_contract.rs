//! Contract for gear_engine::doorbell (T-8, T-130). Pure functions.

use gear_engine::doorbell::{at_once, decide, text, DoorbellAction, DoorbellInput, DoorbellItem};
use gear_model::MsgKind;

const NOW: i64 = 10_000;

fn item(id: &str) -> DoorbellItem {
    DoorbellItem { id: id.into(), urgent: false, at_once: false }
}

fn input(seat: &str) -> DoorbellInput {
    DoorbellInput {
        project: "p".into(),
        seat: format!("p:{seat}"),
        generation: 2,
        pane_id: "w1:p1".into(),
        idle: true,
        slot_busy: false,
        last_settled_at: None,
        no_task: false,
        waiting: vec![item("M-1")],
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
            text: "GEAR: 1 item(s) waiting for you: M-1. Run `gear` now.".into(),
            covers: vec!["M-1".into()],
        }]
    );
}

#[test]
fn urgent_count_and_covers_order_without_duplicates() {
    let mut i = input("captain");
    i.waiting = vec![
        DoorbellItem { id: "M-3".into(), urgent: true, at_once: true },
        DoorbellItem { id: "M-1".into(), urgent: false, at_once: false },
        DoorbellItem { id: "M-7".into(), urgent: false, at_once: false },
    ];
    let out = decide(&[i], 90, NOW);
    assert_eq!(out[0].covers, vec!["M-3", "M-1", "M-7"]);
    assert_eq!(
        out[0].text,
        "GEAR: 3 item(s) waiting for you (1 urgent): M-3 M-1 M-7. Run `gear` now."
    );
}

#[test]
fn silent_when_nothing_busy_or_slot_held() {
    let mut nothing = input("a");
    nothing.waiting = vec![];
    let mut busy = input("b");
    busy.idle = false;
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
    assert_eq!(out.iter().map(|a| a.seat.as_str()).collect::<Vec<_>>(), vec!["p:b"]);
}

#[test]
fn one_action_per_ringing_input_in_input_order() {
    let out = decide(&[input("z"), input("a")], 90, NOW);
    assert_eq!(out.iter().map(|a| a.seat.as_str()).collect::<Vec<_>>(), vec!["p:z", "p:a"]);
}

// ---------------------------------------------------------------- T-130

#[test]
fn a_working_seat_is_rung_at_once_for_an_at_once_message() {
    let mut working = input("lane");
    working.idle = false;
    working.last_settled_at = Some(NOW); // inside the cooldown
    working.waiting = vec![DoorbellItem { id: "M-4".into(), urgent: true, at_once: true }];

    let out = decide(&[working], 90, NOW);
    assert_eq!(out.len(), 1, "typed while the agent works");
    assert_eq!(out[0].text, "GEAR: 1 item(s) waiting for you (1 urgent): M-4. Run `gear` now.");
}

#[test]
fn a_working_seat_is_told_only_about_the_at_once_messages() {
    let mut working = input("lane");
    working.idle = false;
    working.waiting = vec![
        DoorbellItem { id: "M-4".into(), urgent: true, at_once: true },
        DoorbellItem { id: "M-5".into(), urgent: false, at_once: false },
    ];

    let out = decide(&[working], 90, NOW);
    assert_eq!(out[0].covers, vec!["M-4"], "a message that can wait is not typed into a running turn");
}

#[test]
fn an_idle_seat_is_told_about_everything() {
    let mut idle = input("lane");
    idle.waiting = vec![
        DoorbellItem { id: "M-4".into(), urgent: true, at_once: true },
        DoorbellItem { id: "M-5".into(), urgent: false, at_once: false },
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
fn the_text_names_every_id_and_says_run_gear_now() {
    let items = vec![
        DoorbellItem { id: "M-12".into(), urgent: false, at_once: false },
        DoorbellItem { id: "M-13".into(), urgent: false, at_once: false },
    ];
    let said = text(&items, false);
    assert!(said.contains("M-12") && said.contains("M-13"), "{said}");
    assert!(said.contains("Run `gear` now."), "{said}");
    assert!(!said.contains('\n'), "one line: {said}");
}
