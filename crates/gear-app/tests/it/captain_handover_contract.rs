//! T-104: which pane is the captain is the store's binding, and a handover
//! moves it — allowed only from the captain's own real pane or the user's own
//! terminal (a shell with no agent). A lane or reviewer asking is refused and
//! the captain gets one urgent item.
//!
//! The process table is fake here on purpose: every test names the pids its
//! caller runs under, and the fake Herdr says which pids each pane runs, so no
//! test depends on this machine's process tree.

use std::path::Path;

use gear_app::captain::{handover, Asker};
use gear_app::identity::{observe, require_seat};
use gear_app::Env;
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{ErrorCode, GearCtx, SeatRole, SeatState};
use gear_store::{testing, BindingStatus, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const NEW_PANE: &str = "w1:p3";
const SHELL_PANE: &str = "w1:p4";

/// Fake callers: (own pid, the shell it runs under).
const CAP_CALLER: [u32; 2] = [60_001, 60_000];
const LANE_CALLER: [u32; 2] = [50_001, 50_000];
const USER_CALLER: [u32; 2] = [70_001, 70_000];
const NEW_CALLER: [u32; 2] = [80_001, 80_000];

fn config(root: &Path) -> String {
    format!(
        r#"
[project]
id = "p"
herdr_session = "sess"
repo = {root}

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#,
        root = serde_json::json!(root.to_string_lossy()).to_string()
    )
}

fn world(root: &Path) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        let mut project = tx.get_project("p")?.unwrap();
        project.config_toml = config(root);
        tx.update_project(&project)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

fn herdr() -> FakeHerdr {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.add_agent(NEW_PANE, "codex", AgentStatus::Idle);
    herdr.add_shell(SHELL_PANE);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    herdr.set_pane_processes(NEW_PANE, Some(NEW_CALLER[1]), &[]);
    herdr.set_pane_processes(SHELL_PANE, Some(USER_CALLER[1]), &[]);
    herdr
}

fn env(seat: Option<&str>, gen: i64, pane: Option<&str>) -> Env {
    Env {
        ctx: seat.map(|s| GearCtx {
            project: "p".into(),
            seat: s.into(),
            generation: gen,
        }),
        pane_id: pane.map(|p| p.to_string()),
        admin: false,
    }
}

/// From the captain's own pane: the binding moves, the new pane is the captain,
/// the old one is refused.
#[test]
fn a_handover_from_the_captains_pane_moves_the_binding() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    let captain = env(Some("captain"), 1, Some(CAP_PANE));

    let done = handover(
        &mut s,
        &captain,
        &mut herdr,
        &CAP_CALLER,
        root.path(),
        NEW_PANE,
        NOW,
    )
    .unwrap();

    assert_eq!(
        (
            done.from.as_str(),
            done.to.as_str(),
            done.generation,
            done.asker
        ),
        (CAP_PANE, NEW_PANE, 2, Asker::Captain)
    );
    let seat = s.read(|tx| tx.get_seat("p:captain")).unwrap().unwrap();
    assert_eq!(
        (seat.role, seat.generation, seat.state),
        (SeatRole::Captain, 2, SeatState::Active)
    );
    let new = s
        .read(|tx| tx.get_binding("p:captain", 2))
        .unwrap()
        .unwrap();
    assert_eq!(
        (new.pane_id.as_str(), new.status, new.kind.as_str()),
        (NEW_PANE, BindingStatus::Active, "codex")
    );
    let old = s
        .read(|tx| tx.get_binding("p:captain", 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        old.status,
        BindingStatus::Void,
        "the old session cannot write"
    );

    // The new pane is the captain now.
    let moved = env(Some("captain"), 2, Some(NEW_PANE));
    let fp = observe(&moved, &mut herdr, &NEW_CALLER)
        .unwrap()
        .expect("the captain in its new pane");
    let (ctx, _) = s
        .read(|tx| require_seat(tx, &moved, Some(&fp), None, None))
        .unwrap();
    assert_eq!((ctx.seat.as_str(), ctx.generation), ("captain", 2));

    // The old pane still hosts the captain's old agent — and is refused.
    let stale = env(Some("captain"), 2, Some(CAP_PANE));
    let fp = observe(&stale, &mut herdr, &CAP_CALLER)
        .unwrap()
        .expect("the old agent is still there");
    let error = s
        .read(|tx| require_seat(tx, &stale, Some(&fp), None, None))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered, "{}", error.detail);
}

/// A lane asking is refused, one urgent item names its pane, the binding stays,
/// and a repeat does not pile up items.
#[test]
fn a_handover_from_a_lane_is_refused_with_one_item() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    let lane = env(Some("heat"), 1, Some(LANE_PANE));

    let error = handover(
        &mut s,
        &lane,
        &mut herdr,
        &LANE_CALLER,
        root.path(),
        NEW_PANE,
        NOW,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert!(error.detail.contains(LANE_PANE), "{}", error.detail);
    assert!(
        error.detail.contains(CAP_PANE),
        "the captain's own pane: {}",
        error.detail
    );

    let items = s
        .read(|tx| tx.thread_messages("p", "handover:w1:p2"))
        .unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].urgent);
    assert!(items[0].body.contains(LANE_PANE), "{}", items[0].body);

    // The captain did not move, and a repeat claim is one item.
    let seat = s.read(|tx| tx.get_seat("p:captain")).unwrap().unwrap();
    assert_eq!((seat.generation, seat.state), (1, SeatState::Active));
    let _ = handover(
        &mut s,
        &lane,
        &mut herdr,
        &LANE_CALLER,
        root.path(),
        NEW_PANE,
        NOW + 1,
    );
    assert_eq!(
        s.read(|tx| tx.thread_messages("p", "handover:w1:p2"))
            .unwrap()
            .len(),
        1
    );
}

/// The user's own terminal — a shell whose real pane hosts no agent — may hand
/// the captain over; a shell outside every project cannot name one.
#[test]
fn a_handover_from_the_users_own_shell_is_allowed() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    let user = Env {
        ctx: None,
        pane_id: None,
        admin: false,
    };

    let done = handover(
        &mut s,
        &user,
        &mut herdr,
        &USER_CALLER,
        root.path(),
        NEW_PANE,
        NOW,
    )
    .unwrap();
    assert_eq!(
        (done.asker, done.to.as_str(), done.generation),
        (Asker::UserShell, NEW_PANE, 2)
    );
    let new = s
        .read(|tx| tx.get_binding("p:captain", 2))
        .unwrap()
        .unwrap();
    assert_eq!(new.pane_id, NEW_PANE);

    // A shell somewhere else: the project cannot be named.
    let elsewhere = tempfile::tempdir().unwrap();
    let error = handover(
        &mut s,
        &user,
        &mut herdr,
        &USER_CALLER,
        elsewhere.path(),
        CAP_PANE,
        NOW + 1,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("no registered project"),
        "{}",
        error.detail
    );
}
