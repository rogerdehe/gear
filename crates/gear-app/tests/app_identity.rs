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
fn wrong_pane() {
    let mut s = world();
    let err = s
        .read(|tx| {
            require_seat(
                tx,
                &env(Some("heat"), 1, Some("w9:p9"), false),
                Some(&fp("w9:p9", "opencode")),
                None,
                None,
            )
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
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
