//! Contract for gear_engine::doorbell::decide (T-8). Pure function.

use gear_engine::doorbell::{decide, DoorbellAction, DoorbellInput};

const NOW: i64 = 10_000;

fn input(seat: &str) -> DoorbellInput {
    DoorbellInput {
        project: "p".into(),
        seat: format!("p:{seat}"),
        generation: 2,
        pane_id: "w1:p1".into(),
        idle: true,
        slot_busy: false,
        last_settled_at: None,
        actionable: vec!["M-1".into()],
        unshown: vec![],
        urgent: 0,
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
            text: "GEAR: 1 item(s) waiting for you. Run `gear` to see them.".into(),
            covers: vec!["M-1".into()],
        }]
    );
}

#[test]
fn urgent_count_and_covers_order_without_duplicates() {
    let mut i = input("captain");
    i.actionable = vec!["M-3".into(), "M-1".into()];
    i.unshown = vec!["M-1".into(), "M-7".into()];
    i.urgent = 1;
    let out = decide(&[i], 90, NOW);
    assert_eq!(out[0].covers, vec!["M-3", "M-1", "M-7"]);
    assert_eq!(out[0].text, "GEAR: 3 item(s) waiting for you (1 urgent). Run `gear` to see them.");
}

#[test]
fn unshown_alone_is_enough() {
    let mut i = input("heat");
    i.actionable = vec![];
    i.unshown = vec!["M-9".into()];
    assert_eq!(decide(&[i], 90, NOW)[0].covers, vec!["M-9"]);
}

#[test]
fn silent_when_nothing_busy_or_slot_held() {
    let mut nothing = input("a");
    nothing.actionable = vec![];
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
