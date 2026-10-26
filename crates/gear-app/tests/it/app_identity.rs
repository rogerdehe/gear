//! Tests for `gear_app::identity` (T-13).
//!
//! The doc comments in `crates/gear-app/src/identity.rs` are the spec; every
//! row of the error table gets its own test.

use gear_app::identity::{classify, observe, require_seat, Caller, Fingerprint};
use gear_app::Env;
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{ErrorCode, GearCtx, SeatRole, SeatState};
use gear_store::{testing, BindingStatus, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn env(seat: Option<&str>, gen: i64, pane: Option<&str>, admin: bool) -> Env {
    Env {
        ctx: seat.map(|s| GearCtx { project: "p".into(), seat: s.into(), generation: gen }),
        pane_id: pane.map(|p| p.to_string()),
        admin,
    }
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint { pane_id: pane.to_string(), kind: Some(kind.to_string()), session_ref: None }
}

/// A project with an active captain (`p:captain`) and an active lane
/// (`p:heat`), each bound to its own pane.
fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
            Ok(())
        })
        .unwrap();
    s
}

#[test]
fn observe_without_ctx_is_none() {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);

    // No GEAR_CTX: there is no caller to look for.
    assert_eq!(observe(&env(None, 1, Some(CAP_PANE), false), &mut herdr).unwrap(), None);
    // GEAR_CTX but no pane: nothing to read.
    assert_eq!(observe(&env(Some("captain"), 1, None, false), &mut herdr).unwrap(), None);
}

#[test]
fn observe_returns_occupant() {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);

    let got = observe(&env(Some("captain"), 1, Some(CAP_PANE), false), &mut herdr).unwrap();
    let got = got.expect("the pane hosts an agent");
    assert_eq!(got.pane_id, CAP_PANE);
    assert_eq!(got.kind.as_deref(), Some("codex"));
}

#[test]
fn observe_failure_is_not_registered() {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.observe_fails = true;

    let err = observe(&env(Some("captain"), 1, Some(CAP_PANE), false), &mut herdr).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
}

#[test]
fn no_ctx() {
    let mut s = world();
    // Even with GEAR_ADMIN=1: an agent call without GEAR_CTX is never the user.
    let err = s
        .read(|tx| require_seat(tx, &env(None, 1, Some(CAP_PANE), true), None, None, None))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
}

#[test]
fn unknown_seat() {
    let mut s = world();
    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("ghost"), 1, Some(CAP_PANE), false),
                Some(&fp(CAP_PANE, "codex")),
                None,
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
}

#[test]
fn stale_generation() {
    let mut s = world();
    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 7, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "opencode")),
                None,
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleGeneration);
}

#[test]
fn binding_not_active() {
    let mut s = world();
    s.write(|tx| tx.set_binding_status("p:heat", 1, BindingStatus::Void)).unwrap();

    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "opencode")),
                None,
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleGeneration);
}

#[test]
fn wrong_env_pane() {
    let mut s = world();
    // The occupant comes from the bound pane; only HERDR_PANE_ID is wrong.
    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some("w9:p9"), false),
                Some(&fp(LANE_PANE, "opencode")),
                None,
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
}

#[test]
fn wrong_fingerprint_pane() {
    let mut s = world();
    // The env pane is right; the observed occupant sits in another pane.
    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some(LANE_PANE), false),
                Some(&fp("w9:p9", "opencode")),
                None,
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
}

#[test]
fn agentless_pane_is_rejected() {
    let mut s = world();
    // The shell in the seat's pane still carries GEAR_CTX after the agent
    // exited, but no agent runs there: it is not the seat (design/03).
    let env = env(Some("heat"), 1, Some(LANE_PANE), false);

    let err = s.read(|tx| require_seat(tx, &env, None, None, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);

    // A read-only caller cannot prove it is the seat either.
    let caller = s.read(|tx| classify(tx, &env, None)).unwrap();
    assert_eq!(caller, Caller::Anonymous);
}

#[test]
fn wrong_kind() {
    let mut s = world();
    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "claude")),
                None,
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
}

#[test]
fn forbidden_role() {
    let mut s = world();
    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "opencode")),
                Some(&[SeatRole::Captain]),
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn forbidden_state() {
    let mut s = world();
    s.write(|tx| tx.set_seat_state("p:heat", SeatState::Parked, None, NOW)).unwrap();

    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "opencode")),
                None,
                Some(&[SeatState::Active]),
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn accepted() {
    let mut s = world();
    let (ctx, seat) = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "opencode")),
                Some(&[SeatRole::Lane, SeatRole::Temp]),
                Some(&[SeatState::Active]),
            )
        })
        .unwrap();
    assert_eq!(ctx.project, "p");
    assert_eq!(ctx.seat, "heat");
    assert_eq!(ctx.generation, 1);
    assert_eq!(seat.id, "p:heat");
    assert_eq!(seat.role, SeatRole::Lane);
    assert_eq!(seat.state, SeatState::Active);
}

#[test]
fn classify_seat() {
    let mut s = world();
    let caller = s
        .read(|tx| {
            classify(
                tx,
                &env(Some("heat"), 1, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "opencode")),
            )
        })
        .unwrap();
    match caller {
        Caller::Seat { ctx, seat } => {
            assert_eq!(ctx.seat, "heat");
            assert_eq!(seat.id, "p:heat");
        }
        other => panic!("expected a seat, got {other:?}"),
    }
}

#[test]
fn classify_admin() {
    let mut s = world();
    let caller = s.read(|tx| classify(tx, &env(None, 1, None, true), None)).unwrap();
    assert_eq!(caller, Caller::Admin);
}

#[test]
fn classify_bad_ctx_is_anonymous() {
    let mut s = world();
    // A context that does not check out gets no role at all, never Admin.
    let caller = s
        .read(|tx| {
            classify(
                tx,
                &env(Some("heat"), 7, Some(LANE_PANE), false),
                Some(&fp(LANE_PANE, "opencode")),
            )
        })
        .unwrap();
    assert_eq!(caller, Caller::Anonymous);

    // No context and no admin flag: anonymous.
    let caller = s.read(|tx| classify(tx, &env(None, 1, None, false), None)).unwrap();
    assert_eq!(caller, Caller::Anonymous);
}

/// A pid that is certainly not an ancestor of this test process.
const ELSEWHERE: u32 = 4_000_000_000;

/// T-104: GEAR_CTX and HERDR_PANE_ID are environment variables any agent can
/// set. The seat is the pane the caller really runs in: a lane that claims the
/// captain's pane is refused, and the captain hears which pane the caller is in.
#[test]
fn a_caller_outside_the_bound_pane_is_refused_and_the_captain_hears_it() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    // The captain's pane runs someone else's process; the lane's pane runs this
    // caller (the fixture's default is "this process is in the pane").
    herdr.set_pane_processes(CAP_PANE, Some(ELSEWHERE), &[]);

    let claimed = env(Some("captain"), 1, Some(CAP_PANE), false);
    let error = observe(&claimed, &mut herdr).unwrap_err();

    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(error.detail.contains(CAP_PANE), "names the claim: {}", error.detail);
    assert!(error.detail.contains("runs in pane w1:p2"), "names the truth: {}", error.detail);
    assert!(error.detail.contains("GEAR_CTX"), "says why pids beat env: {}", error.detail);

    // One urgent item for the captain, naming the real pane; a repeat is quiet.
    assert!(gear_app::identity::report_claim(&mut s, &claimed, &mut herdr, &error, NOW).unwrap());
    let items = s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].urgent, "urgent: {items:?}");
    assert_eq!(items[0].state, gear_model::MsgState::Open);
    assert!(items[0].body.contains("runs in w1:p2"), "{}", items[0].body);
    assert!(items[0].body.contains("captain"), "{}", items[0].body);
    assert!(!gear_app::identity::report_claim(&mut s, &claimed, &mut herdr, &error, NOW + 1).unwrap());
    assert_eq!(s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap().len(), 1);
}

/// The same caller in its own pane is the seat it says it is.
#[test]
fn a_caller_in_the_bound_pane_is_accepted() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    let own = env(Some("heat"), 1, Some(LANE_PANE), false);

    let got = observe(&own, &mut herdr).unwrap().expect("the lane's agent");
    assert_eq!(got.pane_id, LANE_PANE);
    let caller = s.read(|tx| classify(tx, &own, Some(&got))).unwrap();
    assert!(matches!(caller, Caller::Seat { ref seat, .. } if seat.id == "p:heat"), "{caller:?}");
}

/// The captain's own commands run in the captain's pane: they still pass.
#[test]
fn the_captains_own_commands_pass() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    let own = env(Some("captain"), 1, Some(CAP_PANE), false);

    let got = observe(&own, &mut herdr).unwrap().expect("the captain's agent");
    let caller = s.read(|tx| classify(tx, &own, Some(&got))).unwrap();
    match caller {
        Caller::Seat { seat, .. } => assert_eq!((seat.id.as_str(), seat.role), ("p:captain", SeatRole::Captain)),
        other => panic!("expected the captain, got {other:?}"),
    }
}
