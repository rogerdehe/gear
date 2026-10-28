//! T-126: `gear captain take` — an agent started with plain `codex` (or any
//! CLI) in any pane becomes the project's captain with one command, from the
//! project's checkout, with no `GEAR_CTX` and no pane id to type.
//!
//! The process table is fake here for the same reason as in
//! `captain_handover_contract`: every test names the pids its caller runs under
//! and the fake Herdr says which pids each pane runs.

use std::path::Path;

use gear_app::captain::take;
use gear_app::identity::{observe, resolve_env};
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

const CAP_CALLER: [u32; 2] = [60_001, 60_000];
const LANE_CALLER: [u32; 2] = [50_001, 50_000];
const NEW_CALLER: [u32; 2] = [80_001, 80_000];
const USER_CALLER: [u32; 2] = [70_001, 70_000];
/// A daemon's tool shell: its ancestry reaches no pane of the session.
const SERVICE: [u32; 2] = [4_000_001, 4_000_000];

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
    herdr.add_agent(NEW_PANE, "claude", AgentStatus::Idle);
    herdr.add_shell(SHELL_PANE);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    herdr.set_pane_processes(NEW_PANE, Some(NEW_CALLER[1]), &[]);
    herdr.set_pane_processes(SHELL_PANE, Some(USER_CALLER[1]), &[]);
    herdr
}

/// A caller with no environment at all: the state a plainly started `codex`
/// (or `claude`) is in.
fn bare() -> Env {
    Env {
        ctx: None,
        pane_id: None,
        admin: false,
    }
}

/// The whole move: the fresh pane is the captain at generation+1, the old
/// binding is void, one urgent item says which pane took it and which it left,
/// and `gear` from that pane is the captain's home page with no env.
#[test]
fn take_from_a_fresh_pane_with_no_env_makes_it_the_captain() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();

    let taken = take(
        &mut s,
        &bare(),
        &mut herdr,
        &NEW_CALLER,
        root.path(),
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(taken.to, NEW_PANE);
    assert_eq!(taken.from.as_deref(), Some(CAP_PANE));
    assert_eq!(taken.generation, 2);
    assert_eq!(taken.kind, "claude", "the CLI that is really in the pane");

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
        (NEW_PANE, BindingStatus::Active, "claude")
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

    // One urgent item, for the captain seat, naming both panes.
    let items = s
        .read(|tx| tx.thread_messages("p", "captain:take"))
        .unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].urgent, "{items:?}");
    assert_eq!(items[0].recipient, "p:captain");
    assert!(items[0].body.contains(NEW_PANE), "{}", items[0].body);
    assert!(items[0].body.contains(CAP_PANE), "{}", items[0].body);

    // And `gear` from that pane is the captain, with no environment.
    let resolved = resolve_env(&mut s, &bare(), &mut herdr, &NEW_CALLER).unwrap();
    assert_eq!(
        resolved.ctx,
        Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 2
        })
    );
    let fp = observe(&resolved, &mut herdr, &NEW_CALLER)
        .unwrap()
        .expect("the new agent");
    let page = gear_app::home::build(&mut s, &resolved, Some(&fp), NOW + 1).unwrap();
    assert!(page.text.contains("p:captain"), "{}", page.text);
    assert!(
        page.text.contains("generation 2"),
        "the home page names the new generation: {}",
        page.text
    );
}

/// T-134: the new captain's agent carries the captain's name from the moment
/// it takes the seat — Herdr refuses a prompt to an unnamed agent, and it
/// clears a name when it re-detects the occupant.
#[test]
fn take_names_the_new_captains_agent() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr(); // every agent starts unnamed: the M3.1 state
    assert_eq!(herdr.agent_name(NEW_PANE), None);

    let taken = take(
        &mut s,
        &bare(),
        &mut herdr,
        &NEW_CALLER,
        root.path(),
        None,
        NOW,
    )
    .unwrap();

    assert_eq!(taken.named, gear_herdr::Ensured::Renamed);
    assert_eq!(herdr.agent_name(NEW_PANE), Some("p-captain"));
    assert_eq!(herdr.renames_to("p-captain"), vec![NEW_PANE]);
}

/// A name another pane holds is never stolen: the take reports it and leaves
/// the holder's name alone.
#[test]
fn take_never_steals_a_name_another_pane_holds() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    herdr.set_agent_name(LANE_PANE, Some("p-captain"));

    let taken = take(
        &mut s,
        &bare(),
        &mut herdr,
        &NEW_CALLER,
        root.path(),
        None,
        NOW,
    )
    .unwrap();

    assert_eq!(
        taken.named,
        gear_herdr::Ensured::Taken {
            holder: Some(LANE_PANE.to_string())
        }
    );
    assert_eq!(
        herdr.agent_name(LANE_PANE),
        Some("p-captain"),
        "the holder keeps it"
    );
    assert_eq!(
        herdr.agent_name(NEW_PANE),
        None,
        "and the new captain is not named"
    );
    assert!(
        herdr.renames_to("p-captain").is_empty(),
        "no rename was attempted"
    );
}

/// A lane's pane is not a captain's (T-126): taking it is refused, nothing
/// moves and nobody is rung.
#[test]
fn take_from_a_lanes_pane_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();

    let error = take(
        &mut s,
        &bare(),
        &mut herdr,
        &LANE_CALLER,
        root.path(),
        None,
        NOW,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden, "{}", error.detail);
    assert!(error.detail.contains(LANE_PANE), "{}", error.detail);

    let seat = s.read(|tx| tx.get_seat("p:captain")).unwrap().unwrap();
    assert_eq!((seat.generation, seat.state), (1, SeatState::Active));
    assert!(s
        .read(|tx| tx.thread_messages("p", "captain:take"))
        .unwrap()
        .is_empty());
}

/// It works whether the old captain's pane still exists or not: the seat's
/// binding names the pane it left, and the move does not look at that pane.
#[test]
fn take_works_when_the_old_captains_pane_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    // The old captain's pane was closed with its agent.
    herdr.exit_agent(CAP_PANE);

    let taken = take(
        &mut s,
        &bare(),
        &mut herdr,
        &NEW_CALLER,
        root.path(),
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(
        (taken.from.as_deref(), taken.to.as_str(), taken.generation),
        (Some(CAP_PANE), NEW_PANE, 2)
    );
    let seat = s.read(|tx| tx.get_seat("p:captain")).unwrap().unwrap();
    assert_eq!(seat.generation, 2);
}

/// A pane without an agent is nobody's seat: a captain is a running CLI, never
/// a shell somebody left behind.
#[test]
fn take_needs_an_agent_in_the_pane() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();

    let error = take(
        &mut s,
        &bare(),
        &mut herdr,
        &USER_CALLER,
        root.path(),
        None,
        NOW,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(error.detail.contains("hosts no agent"), "{}", error.detail);
    let seat = s.read(|tx| tx.get_seat("p:captain")).unwrap().unwrap();
    assert_eq!(seat.generation, 1);
}

/// A daemon CLI's tools run in no pane: with exactly one daemon-kind pane bound
/// to no seat, that is the pane `take` means (T-126).
#[test]
fn a_daemon_caller_in_no_pane_takes_the_one_unbound_daemon_pane() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    // The panes a daemon caller could mean: the captain's (bound) and one free
    // codex pane nobody onboarded.
    herdr.add_agent("w1:p5", "codex", AgentStatus::Idle);
    herdr.set_pane_processes("w1:p5", Some(1_000_001), &[]);

    let taken = take(
        &mut s,
        &bare(),
        &mut herdr,
        &SERVICE,
        root.path(),
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(taken.to, "w1:p5", "the one daemon pane with no seat");
    assert_eq!(taken.kind, "codex");
    let new = s
        .read(|tx| tx.get_binding("p:captain", 2))
        .unwrap()
        .unwrap();
    assert_eq!(new.pane_id, "w1:p5");
}

/// A shell outside every pane of the session that could be it says so, and
/// names the command that works.
#[test]
fn take_from_nowhere_names_what_it_needs() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    // No ancestry and no HERDR_PANE_ID, and no unbound daemon pane (the one
    // free pane hosts `claude`).
    let error = take(
        &mut s,
        &bare(),
        &mut herdr,
        &SERVICE,
        root.path(),
        None,
        NOW,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden, "{}", error.detail);
    assert!(error.detail.contains("captain take"), "{}", error.detail);
}

/// Outside every project's checkout, with one registered project, `take` needs
/// no name; a named project that does not exist is refused with the list.
#[test]
fn take_outside_the_checkout_finds_the_only_project_and_refuses_an_unknown_name() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    let elsewhere = tempfile::tempdir().unwrap();
    let taken = take(
        &mut s,
        &bare(),
        &mut herdr,
        &NEW_CALLER,
        elsewhere.path(),
        None,
        NOW,
    )
    .unwrap();
    assert_eq!(taken.project, "p");
    let error = take(
        &mut s,
        &bare(),
        &mut herdr,
        &NEW_CALLER,
        root.path(),
        Some("nope"),
        NOW + 1,
    )
    .unwrap_err();
    assert!(error.detail.contains("Projects: p"), "{}", error.detail);
}
