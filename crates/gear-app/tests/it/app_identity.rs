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
    assert_eq!(observe(&env(None, 1, Some(CAP_PANE), false), &mut herdr, &me()).unwrap(), None);
    // GEAR_CTX but no pane: nothing to read.
    assert_eq!(observe(&env(Some("captain"), 1, None, false), &mut herdr, &me()).unwrap(), None);
}

#[test]
fn observe_returns_occupant() {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);

    let got = observe(&env(Some("captain"), 1, Some(CAP_PANE), false), &mut herdr, &me()).unwrap();
    let got = got.expect("the pane hosts an agent");
    assert_eq!(got.pane_id, CAP_PANE);
    assert_eq!(got.kind.as_deref(), Some("codex"));
}

#[test]
fn observe_failure_is_not_registered() {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.observe_fails = true;

    let err =
        observe(&env(Some("captain"), 1, Some(CAP_PANE), false), &mut herdr, &me()).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);
}

/// This test process as the caller: the fake claims the process that asks, so
/// its own pid is the ancestry that matches.
fn me() -> Vec<u32> {
    vec![std::process::id()]
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

/// A fake process table: a caller's own pid and the shell it runs under. Panes
/// are configured with these pids, so no test depends on the OS's own tree.
const CAP_CALLER: [u32; 2] = [60_001, 60_000];
const LANE_CALLER: [u32; 2] = [50_001, 50_000];

/// T-104: GEAR_CTX and HERDR_PANE_ID are environment variables any agent can
/// set. The seat is the pane the caller really runs in: a lane that claims the
/// captain's pane is refused, and the captain hears which pane the caller is in.
#[test]
fn a_caller_outside_the_bound_pane_is_refused_and_the_captain_hears_it() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);

    let claimed = env(Some("captain"), 1, Some(CAP_PANE), false);
    let error = observe(&claimed, &mut herdr, &LANE_CALLER).unwrap_err();

    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(error.detail.contains(CAP_PANE), "names the claim: {}", error.detail);
    assert!(error.detail.contains("runs in pane w1:p2"), "names the truth: {}", error.detail);
    assert!(error.detail.contains("GEAR_CTX"), "says why pids beat env: {}", error.detail);

    // One urgent item for the captain, naming the real pane; a repeat is quiet.
    assert!(gear_app::identity::report_claim(&mut s, &claimed, &mut herdr, &error, &LANE_CALLER, NOW)
        .unwrap());
    let items = s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].urgent, "urgent: {items:?}");
    assert_eq!(items[0].state, gear_model::MsgState::Open);
    assert!(items[0].body.contains("runs in w1:p2"), "{}", items[0].body);
    assert!(items[0].body.contains("captain"), "{}", items[0].body);
    assert!(!gear_app::identity::report_claim(&mut s, &claimed, &mut herdr, &error, &LANE_CALLER, NOW + 1)
        .unwrap());
    assert_eq!(s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap().len(), 1);
}

/// The caller in its own pane is the seat it says it is.
#[test]
fn a_caller_in_the_bound_pane_is_accepted() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    let own = env(Some("heat"), 1, Some(LANE_PANE), false);

    let got = observe(&own, &mut herdr, &LANE_CALLER).unwrap().expect("the lane's agent");
    assert_eq!(got.pane_id, LANE_PANE);
    let caller = s.read(|tx| classify(tx, &own, Some(&got))).unwrap();
    assert!(matches!(caller, Caller::Seat { ref seat, .. } if seat.id == "p:heat"), "{caller:?}");
}

/// The captain's own commands run in the captain's pane: they still pass, and a
/// captain restarted in the same pane (a new session) is still the captain —
/// the role is the store's binding, not a process.
#[test]
fn the_captains_own_commands_pass_and_survive_a_restart() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    let own = env(Some("captain"), 1, Some(CAP_PANE), false);

    let got = observe(&own, &mut herdr, &CAP_CALLER).unwrap().expect("the captain's agent");
    let caller = s.read(|tx| classify(tx, &own, Some(&got))).unwrap();
    match caller {
        Caller::Seat { seat, .. } => {
            assert_eq!((seat.id.as_str(), seat.role), ("p:captain", SeatRole::Captain))
        }
        other => panic!("expected the captain, got {other:?}"),
    }

    // The captain restarted in the same pane: a new session, the same seat.
    herdr.set_agent_session(CAP_PANE, "ses_captain_2");
    let got = observe(&own, &mut herdr, &CAP_CALLER).unwrap().expect("the restarted captain");
    assert_eq!(got.session_ref.as_deref(), Some("ses_captain_2"));
    let caller = s.read(|tx| classify(tx, &own, Some(&got))).unwrap();
    assert!(matches!(caller, Caller::Seat { ref seat, .. } if seat.id == "p:captain"), "{caller:?}");
}

/// F1 of the review: `ps` must never come from `$PATH`. A shell that prepends a
/// fake `ps` printing the captain pane's pid must not add it to the ancestry —
/// the walk asks the OS itself (or `/bin/ps` by absolute path).
#[test]
fn a_path_provided_ps_cannot_fabricate_ancestry() {
    let forged: u32 = 424_242;
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("ps");
    std::fs::write(&fake, format!("#!/bin/sh\nprintf '{forged}\\n'\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&fake).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake, perms).unwrap();

    // The probe is this test binary, run as a child with the fake `ps` first —
    // and only it — in PATH.
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args(["--exact", "app_identity::ancestry_probe", "--nocapture"])
        .env("PATH", dir.path())
        .env("GEAR_ANCESTRY_PROBE", forged.to_string())
        .output()
        .expect("run the probe");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("ANCESTRY="), "the probe ran: {text}");
}

/// The probe half of `a_path_provided_ps_cannot_fabricate_ancestry`: prints the
/// ancestry it computes and asserts the forged pid is not in it. It runs only
/// when the outer test asks (`GEAR_ANCESTRY_PROBE`), so the normal suite is a
/// no-op.
#[test]
fn ancestry_probe() {
    let Ok(forged) = std::env::var("GEAR_ANCESTRY_PROBE") else {
        return;
    };
    let forged: u32 = forged.parse().expect("a pid");
    let ancestors = gear_app::identity::process_ancestors();
    println!("ANCESTRY={ancestors:?}");
    assert!(
        !ancestors.contains(&forged),
        "a $PATH ps must not add pid {forged}: {ancestors:?}"
    );
}

/// F2 of the review: a correct pane id with someone else's context is still a
/// claim. It is refused with one item naming the real pane — not a silent
/// NotRegistered left for require_seat to raise with no item.
#[test]
fn a_partial_claim_is_refused_with_one_item() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    // The captain's context, the lane's own (genuine) pane id: the process is
    // really in w1:p2, the captain is bound to w1:p1.
    let claimed = env(Some("captain"), 1, Some(LANE_PANE), false);

    let error =
        gear_app::identity::prove_seat(&mut s, &claimed, &mut herdr, &LANE_CALLER, NOW).unwrap_err();

    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(error.detail.contains(CAP_PANE), "the seat's own pane: {}", error.detail);
    let items = s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap();
    assert_eq!(items.len(), 1, "one urgent item: {items:?}");
    assert!(items[0].urgent);
    assert!(items[0].body.contains("w1:p2"), "{}", items[0].body);
    // A repeat is quiet.
    let _ = gear_app::identity::prove_seat(&mut s, &claimed, &mut herdr, &LANE_CALLER, NOW + 1);
    assert_eq!(s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap().len(), 1);
}
