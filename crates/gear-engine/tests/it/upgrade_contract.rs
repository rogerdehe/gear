//! T-491: a project on v0.5.x upgrades with no hand step. The fixture is a
//! store as v0.5.6 wrote it (its schema and the rows of a voyage in progress),
//! committed with the store's test. Here the v0.5-shaped config must load
//! unchanged, one engine tick and the captain's home page must run on the
//! store, and the voyage, seats and open items must be intact afterwards.

use gear_app::identity::Fingerprint;
use gear_app::Env;
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{GearCtx, MsgState, SeatState};
use gear_store::{Store, SCHEMA_VERSION};
use rusqlite::Connection;
use std::path::PathBuf;

const V056: &str = include_str!("../../../gear-store/tests/it/v056_store.sql");
const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn open_fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("gear.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(V056).unwrap();
    drop(conn);
    let store = Store::open(&path).unwrap();
    (dir, store)
}

fn env() -> Env {
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

#[test]
fn a_v056_project_upgrades_and_carries_its_voyage_seats_and_items_on() {
    let (_dir, mut store) = open_fixture();

    // Its gear.toml of v0.5 loads unchanged: every v0.6 field has a default.
    let config = gear_app::config::latest(&mut store, "p").unwrap();
    assert_eq!(gear_app::config::validate(&config), vec![]);
    assert_eq!(config.lanes.len(), 1);
    assert!(config.schedule.is_none(), "no [schedule] in a v0.5 config");

    // One engine tick on the migrated store.
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Idle, "p-captain");
    herdr.add_agent_named(LANE_PANE, "pi", AgentStatus::Idle, "p-heat");
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    tick(&mut store, &mut herdr, &cfg, T0 + 600).unwrap();

    // The captain's home page runs on it, with the voyage and its open item.
    let fp = Fingerprint {
        pane_id: CAP_PANE.into(),
        kind: Some("codex".into()),
        session_ref: None,
    };
    let page = gear_app::home::build(&mut store, &env(), Some(&fp), T0 + 601).unwrap();
    assert!(
        page.text
            .contains("Voyage M5 the next voyage · done: it sails"),
        "{}",
        page.text
    );
    assert!(page.text.contains("M-1"), "{}", page.text);
    assert!(page.view.shown_msgs.iter().any(|id| id == "M-1"));

    // Nothing was lost: the voyage, the seats, the open item, the task, its
    // attempt and the plan version are all there, and the store is current.
    let voyage = store
        .read(|tx| tx.voyages_of_project("p"))
        .unwrap()
        .into_iter()
        .find(|voyage| voyage.milestone == "M5")
        .expect("the voyage");
    assert_eq!(voyage.title, "the next voyage");
    assert_eq!(voyage.done.as_deref(), Some("it sails"));
    assert_eq!(voyage.departure, Some(1799990000));
    let seats = store.read(|tx| tx.seats_of_project("p")).unwrap();
    for id in ["p:captain", "p:heat"] {
        let seat = seats.iter().find(|seat| seat.id == id).expect(id);
        assert_eq!(seat.state, SeatState::Active, "{id}");
    }
    let message = store
        .read(|tx| tx.get_message("p", "M-1"))
        .unwrap()
        .expect("the open item");
    assert_eq!(message.state, MsgState::Open);
    assert_eq!(message.body, "the voyage is in progress");
    let task = store
        .read(|tx| tx.get_task("p", "T-1"))
        .unwrap()
        .expect("the task");
    assert!(!task.state.is_terminal(), "{}", task.state.as_str());
    assert_eq!(
        store
            .read(|tx| tx.attempts_of_task("p", "T-1"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.read(|tx| tx.plan_version("p")).unwrap(), 1);
    assert_eq!(
        store
            .read(|tx| tx.get_meta("schema_version"))
            .unwrap()
            .as_deref(),
        Some(SCHEMA_VERSION.to_string().as_str())
    );

    // A second open is a no-op: the version and the rows stay.
    drop(store);
    let mut again = Store::open(&_dir.path().join("gear.db")).unwrap();
    assert_eq!(
        again
            .read(|tx| tx.get_meta("schema_version"))
            .unwrap()
            .as_deref(),
        Some(SCHEMA_VERSION.to_string().as_str())
    );
    assert_eq!(
        again
            .read(|tx| tx.get_message("p", "M-1"))
            .unwrap()
            .unwrap()
            .body,
        "the voyage is in progress"
    );
}
