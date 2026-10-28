//! Tests for `gear_app::identity` (T-13).
//!
//! The doc comments in `crates/gear-app/src/identity.rs` are the spec; every
//! row of the error table gets its own test.

use gear_app::identity::{classify, observe, require_seat, Caller, DaemonSessions, Fingerprint};
use gear_app::Env;
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{ErrorCode, GearCtx, SeatRole, SeatState};
use gear_store::{testing, BindingStatus, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const CLAUDE_PANE: &str = "w1:p3";
const CODEX_PANE: &str = "w1:p4";

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

/// The caller's ancestry in the signed form `gear whoami` takes (design §10.7,
/// row B9).
fn me_i32() -> Vec<i32> {
    me().into_iter().map(|pid| pid as i32).collect()
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
const CLAUDE_CALLER: [u32; 2] = [55_001, 55_000];

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

/// T-314: a refusal has to be traceable afterwards. It names the Herdr door it
/// asked and the ancestry it matched against that door's panes.
#[test]
fn the_refusal_names_the_herdr_socket_and_the_ancestry() {
    let mut herdr = FakeHerdr::new();
    // A kind whose tools run in the pane: no daemon exception can accept it.
    herdr.add_agent(CAP_PANE, "claude", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.socket = Some("/s/herdr.sock".into());
    let claimed = env(Some("captain"), 1, Some(CAP_PANE), false);
    let ancestors = [30, 20, 10];

    let error = observe(&claimed, &mut herdr, &ancestors).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(
        error
            .detail
            .contains("asked Herdr at /s/herdr.sock; this process's ancestry: 30 < 20 < 10"),
        "names the socket it asked and the ancestry it saw: {}",
        error.detail
    );

    // A door that cannot name its socket says so, rather than leaving a blank.
    herdr.socket = None;
    let error = observe(&claimed, &mut herdr, &ancestors).unwrap_err();
    assert!(error.detail.contains("an unnamed Herdr"), "{}", error.detail);
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

/// T-131: the ancestry of this process itself is found in process — its parent
/// is the pid the OS reports, and nothing was started to learn it. The old walk
/// spawned `/bin/ps` once per ancestor; when program launch was slow those `ps`
/// piled up stuck, `gear` hung, and its callers killed it.
#[test]
fn this_process_ancestry_comes_from_the_os_in_process() {
    let ancestors = gear_app::identity::process_ancestors();
    assert_eq!(ancestors.first().copied(), Some(std::process::id()), "own pid first: {ancestors:?}");
    let parent = std::os::unix::process::parent_id();
    assert_eq!(
        ancestors.get(1).copied(),
        Some(parent),
        "the second entry is the parent pid the OS reports: {ancestors:?}"
    );
}

/// F1 of the review: `ps` must never come from `$PATH`. A shell that prepends a
/// fake `ps` printing the captain pane's pid must not add it to the ancestry —
/// the walk asks the OS itself, by a read in process (T-131).
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
        gear_app::identity::prove_seat(&mut s, &claimed, &mut herdr, &LANE_CALLER, &DaemonSessions::default(), NOW).unwrap_err();

    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(error.detail.contains(CAP_PANE), "the seat's own pane: {}", error.detail);
    let items = s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap();
    assert_eq!(items.len(), 1, "one urgent item: {items:?}");
    assert!(items[0].urgent);
    assert!(items[0].body.contains("w1:p2"), "{}", items[0].body);
    // A repeat is quiet.
    let _ = gear_app::identity::prove_seat(&mut s, &claimed, &mut herdr, &LANE_CALLER, &DaemonSessions::default(), NOW + 1);
    assert_eq!(s.read(|tx| tx.thread_messages("p", "identity:w1:p2")).unwrap().len(), 1);
}

/// OpenCode and Codex-in-daemon-mode run their tools in a shared service,
/// outside every pane: such a caller keeps its claim on a pane of a daemon kind
/// (still checked against the binding), but it can never claim a captain whose
/// agent runs its tools in its pane (Claude Code). Codex joined the list with
/// T-126 — its TUI is a client of `codex app-server`, the same shape as
/// `opencode serve` — so a Codex tool shell is the seat of its own Codex pane.
#[test]
fn a_daemon_caller_keeps_its_claim_on_a_daemon_pane() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "claude", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.add_agent(CLAUDE_PANE, "claude", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    herdr.set_pane_processes(CLAUDE_PANE, Some(CLAUDE_CALLER[1]), &[]);
    // The service's tool shell: its ancestry reaches no pane of the session.
    let service: Vec<u32> = vec![4_000_001, 4_000_000, 1];

    let lane = env(Some("heat"), 1, Some(LANE_PANE), false);
    let got = observe(&lane, &mut herdr, &service).unwrap().expect("the lane's agent");
    let caller = s.read(|tx| classify(tx, &lane, Some(&got))).unwrap();
    assert!(matches!(caller, Caller::Seat { ref seat, .. } if seat.id == "p:heat"), "{caller:?}");

    // T-126: a Codex caller in no pane is the seat of the Codex pane it
    // claims (its own daemon), where Claude Code's `observe` still refuses.
    herdr.add_agent(CODEX_PANE, "codex", AgentStatus::Idle);
    herdr.set_pane_processes(CODEX_PANE, Some(CLAUDE_CALLER[1]), &[]);
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "heat3", SeatRole::Lane, CODEX_PANE, "codex", NOW)?;
        Ok(())
    })
    .unwrap();
    let codex = env(Some("heat3"), 1, Some(CODEX_PANE), false);
    let got = observe(&codex, &mut herdr, &service).unwrap().expect("the codex pane");
    assert_eq!(got.kind.as_deref(), Some("codex"));
    let caller = s.read(|tx| classify(tx, &codex, Some(&got))).unwrap();
    assert!(matches!(caller, Caller::Seat { ref seat, .. } if seat.id == "p:heat3"), "{caller:?}");

    // The captain's pane runs Claude Code: a caller in no pane cannot claim it.
    let captain = env(Some("captain"), 1, Some(CAP_PANE), false);
    let error = observe(&captain, &mut herdr, &service).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered, "{}", error.detail);

    // A CLI whose tools run in its pane is not one of those: a caller in no
    // pane claiming it is nobody.
    let claude = env(Some("heat"), 1, Some(CLAUDE_PANE), false);
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "heat2", SeatRole::Lane, CLAUDE_PANE, "claude", NOW)?;
        Ok(())
    }).unwrap();
    let error = observe(&claude, &mut herdr, &service).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered, "{}", error.detail);
}

// ------------------------------------------------- T-126: no GEAR_CTX needed

/// A caller in a **bound** pane is that seat without `GEAR_CTX` (T-126): the
/// pane it really runs in carries a seat through its active binding, and the
/// generation comes from the seat — the very `GearCtx` the env would have
/// carried. `require_seat` then proves it as usual.
#[test]
fn a_caller_without_ctx_in_a_bound_pane_is_that_seat() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);

    let bare = env(None, 1, None, false);
    let resolved =
        gear_app::identity::resolve_env(&mut s, &bare, &mut herdr, &LANE_CALLER).unwrap();
    assert_eq!(
        resolved.ctx,
        Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 })
    );
    assert_eq!(resolved.pane_id.as_deref(), Some(LANE_PANE), "the real pane is kept");

    let fp = observe(&resolved, &mut herdr, &LANE_CALLER).unwrap().expect("the lane's agent");
    let (ctx, seat) = s.read(|tx| require_seat(tx, &resolved, Some(&fp), None, None)).unwrap();
    assert_eq!((ctx.seat.as_str(), seat.id.as_str()), ("heat", "p:heat"));
}

/// A pane with no seat leaves the caller anonymous (T-126): the user's own
/// shell, or a CLI nobody onboarded. The resolved pane is still recorded.
#[test]
fn a_caller_without_ctx_in_an_unbound_pane_stays_anonymous() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CLAUDE_PANE, "claude", AgentStatus::Idle);
    herdr.set_pane_processes(CLAUDE_PANE, Some(CLAUDE_CALLER[1]), &[]);

    let bare = env(None, 1, None, false);
    let resolved =
        gear_app::identity::resolve_env(&mut s, &bare, &mut herdr, &CLAUDE_CALLER).unwrap();
    assert_eq!(resolved.ctx, None, "no seat is bound to this pane");
    assert_eq!(resolved.pane_id.as_deref(), Some(CLAUDE_PANE));
    let caller = s.read(|tx| classify(tx, &resolved, None)).unwrap();
    assert!(matches!(caller, Caller::Anonymous), "{caller:?}");
}

/// A caller whose tools run in a daemon runs in no pane: the one daemon-kind
/// pane of the session that is bound to a seat names its seat (T-126). Two
/// such panes are ambiguous — the caller must name itself then.
#[test]
fn a_daemon_caller_without_ctx_finds_the_one_bound_daemon_pane() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    // The captain is the only daemon pane; the lane's bound pane is gone, so
    // the daemon has exactly one seat to point at.
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(CLAUDE_PANE, "claude", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(CLAUDE_PANE, Some(CLAUDE_CALLER[1]), &[]);
    let service: Vec<u32> = vec![4_000_001, 4_000_000, 1];

    let bare = env(None, 1, None, false);
    let resolved = gear_app::identity::resolve_env(&mut s, &bare, &mut herdr, &service).unwrap();
    assert_eq!(
        resolved.ctx,
        Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 })
    );
    assert_eq!(resolved.pane_id.as_deref(), Some(CAP_PANE));

    // A second bound daemon pane: no answer, and the error names the one
    // command that works.
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    let error = gear_app::identity::resolve_env(&mut s, &bare, &mut herdr, &service).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(error.detail.contains("no pane"), "{}", error.detail);
    assert!(error.detail.contains("gear captain take"), "{}", error.detail);
}

/// `GEAR_CTX` stays an override (T-126): a set context is never second-guessed
/// by the resolution, whatever pane the caller runs in.
#[test]
fn a_set_ctx_is_not_replaced_by_the_pane() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);

    let captain = env(Some("captain"), 1, Some(CAP_PANE), false);
    let same = gear_app::identity::resolve_env(&mut s, &captain, &mut herdr, &LANE_CALLER).unwrap();
    assert_eq!(same, captain);
}

/// Codex in daemon mode runs its tools in the shared `codex app-server`, outside
/// every pane (trial, 2026-10-26): a Codex captain's tool call keeps its claim,
/// still checked against the binding; a caller that really runs in another pane
/// is still refused.
#[test]
fn a_codex_daemon_caller_keeps_its_captain_seat() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    let daemon: Vec<u32> = vec![5_000_001, 5_000_000, 1];

    let captain = env(Some("captain"), 1, Some(CAP_PANE), false);
    let got = observe(&captain, &mut herdr, &daemon).unwrap().expect("the captain's agent");
    let caller = s.read(|tx| classify(tx, &captain, Some(&got))).unwrap();
    assert!(matches!(caller, Caller::Seat { ref seat, .. } if seat.id == "p:captain"), "{caller:?}");

    // A lane's shell (really in the lane's pane) claiming the captain is refused.
    let error = observe(&captain, &mut herdr, &LANE_CALLER).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered);
}

// ---------------------------------------------------------------- whoami (B9)

/// §10.7 row B9: `gear whoami` names the pane the caller runs in, the seat
/// bound to it, the store, and why a claim fails.
#[test]
fn whoami_in_a_bound_pane_names_the_seat() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    // The pane claims this process, so the caller runs in it.
    herdr.set_pane_processes(LANE_PANE, Some(std::process::id()), &[]);

    let who = gear_app::identity::whoami(
        &mut s,
        &env(Some("heat"), 1, Some(LANE_PANE), false),
        &mut herdr,
        &me_i32(),
    );

    assert_eq!(who.pane.as_deref(), Some(LANE_PANE));
    assert_eq!(who.seat.as_deref(), Some("p:heat"));
    assert_eq!(who.reason, None, "the claim holds: {:?}", who.reason);
    let text = who.to_string();
    assert!(text.contains(LANE_PANE), "{text}");
    assert!(text.contains("p:heat"), "{text}");
    assert!(text.contains("store:"), "{text}");
}

/// A pane with no seat bound to it: whoami says why the caller has no seat.
#[test]
fn whoami_in_an_unbound_pane_says_why() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p9", "opencode", AgentStatus::Idle);
    herdr.set_pane_processes("w1:p9", Some(std::process::id()), &[]);

    let who = gear_app::identity::whoami(
        &mut s,
        &env(Some("heat"), 1, Some("w1:p9"), false),
        &mut herdr,
        &me_i32(),
    );

    assert_eq!(who.pane.as_deref(), Some("w1:p9"));
    assert_eq!(who.seat, None, "no seat is bound to that pane");
    let reason = who.reason.clone().expect("the reason a claim fails");
    assert!(reason.contains("no seat is bound"), "{reason}");
    assert!(who.to_string().contains("reason:"), "{}", who);
}

/// A claim of a seat the pane does not own: whoami names the seat the pane
/// really has.
#[test]
fn whoami_names_the_seat_the_pane_really_has() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(std::process::id()), &[]);

    // The caller claims the lane's seat from the captain's pane.
    let who = gear_app::identity::whoami(
        &mut s,
        &env(Some("heat"), 1, Some(CAP_PANE), false),
        &mut herdr,
        &me_i32(),
    );

    assert_eq!(who.seat.as_deref(), Some("p:captain"));
    let reason = who.reason.expect("the reason a claim fails");
    assert!(reason.contains("p:heat"), "{reason}");
    assert!(reason.contains("p:captain"), "{reason}");
}

/// T-165 (review F3): a seat that was restarted carries a new generation. The
/// caller still carries the old one: `require_seat` refuses it
/// (STALE_GENERATION), so whoami must say so — not "nothing is wrong".
#[test]
fn whoami_reports_the_stale_generation_of_a_restarted_seat() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(LANE_PANE, Some(std::process::id()), &[]);

    // The seat is at generation 1; the caller carries generation 0.
    let who = gear_app::identity::whoami(
        &mut s,
        &env(Some("heat"), 0, Some(LANE_PANE), false),
        &mut herdr,
        &me_i32(),
    );

    assert_eq!(who.pane.as_deref(), Some(LANE_PANE));
    assert_eq!(who.seat.as_deref(), Some("p:heat"), "the seat of the pane");
    let reason = who.reason.expect("the reason a claim fails");
    assert!(reason.contains("generation"), "{reason}");
}

/// T-165 (review F3): a caller without `GEAR_CTX` that runs in a bound pane
/// **is** the seat of that pane (the rule every command resolves with). The
/// claim holds: whoami must not say "no GEAR_CTX in the environment".
#[test]
fn whoami_resolves_a_context_free_bound_pane() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(LANE_PANE, Some(std::process::id()), &[]);

    let who = gear_app::identity::whoami(
        &mut s,
        &env(None, 1, Some(LANE_PANE), false),
        &mut herdr,
        &me_i32(),
    );

    assert_eq!(who.pane.as_deref(), Some(LANE_PANE));
    assert_eq!(who.seat.as_deref(), Some("p:heat"), "the seat of the pane");
    assert_eq!(who.reason, None, "the claim holds: {:?}", who.reason);
}

// --------------------------------- T-328: a daemon caller proves its session

/// The daemon sessions a tool shell of the same `opencode serve` carries: the
/// shell names the session it works for, and an empty value is no value.
#[test]
fn daemon_sessions_come_from_the_tool_shells_variables() {
    let vars = |wanted: &str| match wanted {
        "OPENCODE_SESSION_ID" => Some("ses_x".to_string()),
        "CODEX_THREAD_ID" => Some(String::new()),
        _ => None,
    };
    let sessions = DaemonSessions::from_vars(vars);
    assert_eq!(sessions.opencode.as_deref(), Some("ses_x"));
    assert_eq!(sessions.codex, None, "an empty value is no value");
    assert_eq!(DaemonSessions::from_vars(|_| None), DaemonSessions::default());
}

/// T-328: an OpenCode tool shell in no pane proves it is the seat's **own**
/// session by `OPENCODE_SESSION_ID` — the id GEAR records when it starts the
/// seat. Another lane's session is a claim, refused and reported like the other
/// refusals; a seat with no recorded session has nothing to prove against; a
/// caller inside its own pane is unaffected (its ancestry proves it, T-104).
#[test]
fn an_opencode_daemon_caller_proves_its_own_session() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    // The service's tool shell: its ancestry reaches no pane of the session.
    let service: Vec<u32> = vec![4_000_001, 4_000_000, 1];
    let lane = env(Some("heat"), 1, Some(LANE_PANE), false);
    s.write(|tx| tx.set_binding_session_ref("p:heat", 1, "ses_a")).unwrap();

    // The seat's own session: the claim holds.
    let own = DaemonSessions { opencode: Some("ses_a".into()), codex: None };
    let fp = gear_app::identity::prove_seat(&mut s, &lane, &mut herdr, &service, &own, NOW)
        .unwrap()
        .expect("the lane's pane");
    assert_eq!(fp.kind.as_deref(), Some("opencode"));

    // Another same-daemon lane's session: refused, and the captain told once.
    let foreign = DaemonSessions { opencode: Some("ses_b".into()), codex: None };
    let error =
        gear_app::identity::prove_seat(&mut s, &lane, &mut herdr, &service, &foreign, NOW)
            .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(error.detail.contains("claimed by OpenCode session ses_b"), "{}", error.detail);
    assert!(error.detail.contains("the seat's sessions are ses_a"), "{}", error.detail);
    // The tool shell runs in no pane, so the report names the unknown pane.
    let items = s.read(|tx| tx.thread_messages("p", "identity:unknown")).unwrap();
    assert_eq!(items.len(), 1, "one urgent item: {items:?}");
    assert!(items[0].urgent);

    // No `OPENCODE_SESSION_ID` at all: no OpenCode tool shell, no claim.
    let error = gear_app::identity::prove_seat(
        &mut s,
        &lane,
        &mut herdr,
        &service,
        &DaemonSessions::default(),
        NOW,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotRegistered);
    assert!(error.detail.contains("without OPENCODE_SESSION_ID"), "{}", error.detail);
    assert!(error.detail.contains("not the seat's OpenCode session"), "{}", error.detail);

    // A seat with no known session has nothing to prove against (as before).
    herdr.add_agent(CODEX_PANE, "opencode", AgentStatus::Idle);
    herdr.set_pane_processes(CODEX_PANE, Some(CLAUDE_CALLER[1]), &[]);
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "heat2", SeatRole::Lane, CODEX_PANE, "opencode", NOW)
    })
    .unwrap();
    let bare = env(Some("heat2"), 1, Some(CODEX_PANE), false);
    assert!(gear_app::identity::prove_seat(
        &mut s,
        &bare,
        &mut herdr,
        &service,
        &DaemonSessions::default(),
        NOW,
    )
    .unwrap()
    .is_some());

    // A caller whose ancestry reaches its own pane is unaffected: the pane is
    // its proof, and no session variable is needed.
    assert!(gear_app::identity::prove_seat(
        &mut s,
        &lane,
        &mut herdr,
        &LANE_CALLER,
        &DaemonSessions::default(),
        NOW,
    )
    .unwrap()
    .is_some());
}

/// T-328: Q: a Codex thread id that is not one of the seat's sessions passes —
/// that the id equals the session GEAR records is not yet seen live, and this
/// voyage's reviewers are Codex — but the captain is told once per seat and
/// generation. The seat's own thread, and no thread at all, say nothing.
#[test]
fn a_codex_daemon_caller_of_a_foreign_thread_owes_one_notice() {
    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    let service: Vec<u32> = vec![4_000_001, 4_000_000, 1];
    let captain = env(Some("captain"), 1, Some(CAP_PANE), false);
    s.write(|tx| tx.set_binding_session_ref("p:captain", 1, "th_1")).unwrap();
    let thread = "identity-codex:p:captain:1";

    // The seat's own thread: quiet from the start.
    let own = DaemonSessions { opencode: None, codex: Some("th_1".into()) };
    assert!(gear_app::identity::prove_seat(&mut s, &captain, &mut herdr, &service, &own, NOW)
        .unwrap()
        .is_some());
    assert!(s.read(|tx| tx.thread_messages("p", thread)).unwrap().is_empty());

    // No thread at all: it says nothing either.
    assert!(gear_app::identity::prove_seat(
        &mut s,
        &captain,
        &mut herdr,
        &service,
        &DaemonSessions::default(),
        NOW + 1,
    )
    .unwrap()
    .is_some());
    assert!(s.read(|tx| tx.thread_messages("p", thread)).unwrap().is_empty());

    // A foreign thread: the claim passes, and the captain is told once.
    let foreign = DaemonSessions { opencode: None, codex: Some("th_2".into()) };
    assert!(gear_app::identity::prove_seat(&mut s, &captain, &mut herdr, &service, &foreign, NOW + 2)
        .unwrap()
        .is_some());
    let items = s.read(|tx| tx.thread_messages("p", thread)).unwrap();
    assert_eq!(items.len(), 1, "one notice: {items:?}");
    assert_eq!(items[0].state, gear_model::MsgState::Info);
    assert!(!items[0].urgent);
    assert!(items[0].body.contains("Codex thread th_2"), "{}", items[0].body);
    assert!(items[0].body.contains("the seat's sessions are th_1"), "{}", items[0].body);

    // A second call writes no second one.
    gear_app::identity::prove_seat(&mut s, &captain, &mut herdr, &service, &foreign, NOW + 3)
        .unwrap();
    assert_eq!(s.read(|tx| tx.thread_messages("p", thread)).unwrap().len(), 1);
}
