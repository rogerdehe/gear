//! T-75: the captain's home page shows a seat whose provider is in a
//! back-off. The engine records the wait (the board reads it); the page says
//! "provider back-off" instead of "working", so a Codex seat retrying a 429
//! is not read as a stall.

use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::Env;
use gear_model::{GearCtx, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "codex", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

fn captain_env() -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    }
}

fn fp() -> Fingerprint {
    Fingerprint {
        pane_id: CAP_PANE.into(),
        kind: Some("codex".into()),
        session_ref: None,
    }
}

#[test]
fn the_home_page_shows_the_seat_waiting_for_its_provider() {
    let mut s = world();
    // The engine observed Codex retrying: the seat waits for the provider.
    s.write(|tx| tx.set_seats_in_backoff("p", &["p:heat".to_string()]))
        .unwrap();

    let page = home::build(&mut s, &captain_env(), Some(&fp()), NOW).unwrap();

    assert!(page.text.contains("provider back-off"), "{}", page.text);
    // The lane line names the lane and its agent state.
    let line = page
        .text
        .lines()
        .find(|line| line.contains("heat ·"))
        .unwrap_or_else(|| panic!("no lane line: {}", page.text));
    assert!(line.contains("agent provider back-off"), "{line}");
}

#[test]
fn the_home_page_says_nothing_about_a_back_off_when_there_is_none() {
    let mut s = world();

    let page = home::build(&mut s, &captain_env(), Some(&fp()), NOW).unwrap();

    assert!(!page.text.contains("provider back-off"), "{}", page.text);
}

#[test]
fn the_wait_clears_from_the_page_when_the_provider_answers() {
    let mut s = world();
    s.write(|tx| tx.set_seats_in_backoff("p", &["p:heat".to_string()]))
        .unwrap();
    assert!(home::build(&mut s, &captain_env(), Some(&fp()), NOW)
        .unwrap()
        .text
        .contains("provider back-off"));

    // The next tick sees the ordinary screen: the seat is not waiting any more.
    s.write(|tx| tx.set_seats_in_backoff("p", &[])).unwrap();

    let page = home::build(&mut s, &captain_env(), Some(&fp()), NOW + 10).unwrap();
    assert!(!page.text.contains("provider back-off"), "{}", page.text);
}
