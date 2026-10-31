//! T-468: a dialog never holds a seat.
//!
//! A lane or reviewer may not stop on a dialog at all: its blocking screen is
//! closed in the same tick and the seat is told to ask the captain with
//! `gear send --kind question`. A captain's dialog — the one it shows the user
//! — is cancelled when nobody answers it within ten minutes, and the work goes
//! on as it would have without it.

use gear_engine::{dialog, observe, tick, EngineCfg};
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::AgentStatus;
use gear_model::SeatRole;
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";
const TEMP: &str = "w1:p3";

/// The Codex 0.165 startup screen (M5, gear/voyages/M6/backlog.md).
const CODEX_DIALOG: &str = "Where should the new conversation run?\n  1. Use current Git worktree\n  2. Create new Git worktree";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

fn world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "rev-1", SeatRole::Temp, TEMP, "codex", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "codex", AgentStatus::Blocked);
    herdr.add_agent(TEMP, "codex", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    World { store, herdr, cfg }
}

fn run(w: &mut World, seconds: i64) -> gear_engine::TickReport {
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + seconds).unwrap()
}

fn keys_sent(w: &World, pane: &str) -> Vec<Vec<String>> {
    w.herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::SendKeys { pane: p, keys } if p == pane => Some(keys.clone()),
            _ => None,
        })
        .collect()
}

fn typed(w: &World, pane: &str) -> Vec<String> {
    w.herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::PaneType { pane: p, text } if p == pane => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_blocked_lane_is_closed_in_the_same_tick_and_told_to_ask_the_captain() {
    let mut w = world();
    let report = run(&mut w, 0);
    assert_eq!(report.dialogs, vec!["p:heat"]);
    assert!(
        keys_sent(&w, LANE).contains(&vec!["esc".to_string()]),
        "the dialog is closed: {:?}",
        w.herdr.calls
    );
    assert_eq!(
        typed(&w, LANE),
        vec![dialog::LANE_TEXT.to_string()],
        "the seat is told what to do instead"
    );
    assert!(
        keys_sent(&w, LANE).contains(&vec!["enter".to_string()]),
        "the text is submitted"
    );
    assert!(dialog::LANE_TEXT.contains("gear send --kind question"));
}

#[test]
fn a_blocked_reviewer_is_closed_too() {
    let mut w = world();
    w.herdr.set_status(LANE, AgentStatus::Idle);
    w.herdr.set_status(TEMP, AgentStatus::Blocked);
    let report = run(&mut w, 0);
    assert_eq!(report.dialogs, vec!["p:rev-1"]);
    assert!(keys_sent(&w, TEMP).contains(&vec!["esc".to_string()]));
    assert_eq!(typed(&w, TEMP), vec![dialog::LANE_TEXT.to_string()]);
    assert!(typed(&w, LANE).is_empty(), "the idle lane has no dialog");
}

#[test]
fn a_dialog_that_stays_open_is_closed_again_but_the_text_is_not_repeated() {
    let mut w = world();
    assert_eq!(run(&mut w, 0).dialogs, vec!["p:heat"]);
    assert!(
        run(&mut w, 10).dialogs.is_empty(),
        "just closed: let it take"
    );
    assert_eq!(
        run(&mut w, 30).dialogs,
        vec!["p:heat"],
        "still open after half a minute: close it again"
    );
    assert_eq!(typed(&w, LANE).len(), 1, "the seat is told once per spell");
}

#[test]
fn the_screen_that_closed_starts_a_new_spell() {
    let mut w = world();
    assert_eq!(run(&mut w, 0).dialogs, vec!["p:heat"]);
    // The lane works again: the spell is over.
    w.herdr.set_status(LANE, AgentStatus::Idle);
    assert!(run(&mut w, 5).dialogs.is_empty());
    // A new dialog: closed at once, and told again.
    w.herdr.set_status(LANE, AgentStatus::Blocked);
    assert_eq!(run(&mut w, 6).dialogs, vec!["p:heat"]);
    assert_eq!(typed(&w, LANE).len(), 2, "a new spell is told again");
}

#[test]
fn a_captain_dialog_is_cancelled_after_ten_unanswered_minutes() {
    let mut w = world();
    w.herdr.set_status(LANE, AgentStatus::Idle);
    w.herdr.set_screen(CAP, CODEX_DIALOG);
    assert!(run(&mut w, 0).dialogs.is_empty(), "the ten minutes start");
    assert!(
        keys_sent(&w, CAP).is_empty(),
        "the captain's dialog is left alone for now"
    );
    assert!(run(&mut w, 599).dialogs.is_empty());
    assert_eq!(
        run(&mut w, 600).dialogs,
        vec!["p:captain"],
        "nobody answered for ten minutes: cancel it"
    );
    assert!(
        keys_sent(&w, CAP).contains(&vec!["enter".to_string()]),
        "the profile's keys take the screen"
    );
    assert!(
        typed(&w, CAP).is_empty(),
        "the captain is told nothing: the work goes on"
    );
}

#[test]
fn a_captain_dialog_that_is_answered_never_starts_the_clock() {
    let mut w = world();
    w.herdr.set_status(LANE, AgentStatus::Idle);
    w.herdr.set_screen(CAP, CODEX_DIALOG);
    assert!(run(&mut w, 0).dialogs.is_empty());
    // The user answered it: the screen is gone.
    w.herdr.set_screen(CAP, "");
    assert!(run(&mut w, 30).dialogs.is_empty());
    // A later dialog gets its own ten minutes.
    w.herdr.set_screen(CAP, CODEX_DIALOG);
    assert!(run(&mut w, 60).dialogs.is_empty());
    assert!(run(&mut w, 659).dialogs.is_empty());
    assert_eq!(run(&mut w, 660).dialogs, vec!["p:captain"]);
}

/// The watch on its own, so a test sees the dialog step alone.
#[test]
fn the_dialog_step_closes_the_screen_of_a_blocked_seat() {
    let mut w = world();
    let agents = observe::snapshot(&mut w.herdr).unwrap();
    let observed = w
        .store
        .read(|tx| observe::reconcile(tx, &mut w.herdr, "p", &agents, &w.cfg.profiles))
        .unwrap();
    let closed = dialog::check(&mut w.store, &mut w.herdr, &observed, &w.cfg.profiles, T0).unwrap();
    assert_eq!(closed, vec!["p:heat"]);
    assert!(keys_sent(&w, LANE).contains(&vec!["esc".to_string()]));
}
