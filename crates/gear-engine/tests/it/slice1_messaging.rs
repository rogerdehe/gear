//! Slice 1 acceptance: the messaging loop end to end (design/07, /08, /13).
//!
//! worker `gear send` → engine doorbell to idle captain → captain `gear` (view)
//! → doorbell confirmed → captain `gear reply` → doorbell to worker for the
//! unshown reply → worker `gear` → confirmed → silence.
//!
//! Do not edit these tests to make them pass; report back if one looks wrong.

use gear_app::identity::Fingerprint;
use gear_app::{home, mail, Env};
use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

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
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    // A real agent carries its seat name (T-134): an unnamed one refuses a
    // prompt.
    herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Idle, "p-captain");
    herdr.add_agent_named(LANE_PANE, "opencode", AgentStatus::Idle, "p-heat");
    let mut cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        ..Default::default()
    };
    // Seeded seats use profile name == kind ("codex", "opencode").
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.to_string(), test_profile(kind));
    }
    World { store, herdr, cfg }
}

fn test_profile(kind: &str) -> gear_herdr::Profile {
    gear_herdr::Profile {
        kind: kind.into(),
        new_session: String::new(),
        compact: String::new(),
        usage_log: None,
        error_markers: Vec::new(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec![],
        quota_markers: vec![],
        quota_bar: None,
        auth_markers: vec![],
        backoff_markers: vec![],
        blocked_markers: vec![],
        tail_lines: 16,
        confirm_rounds: 2,
        unattended_args: vec!["--auto".into()],
        instructions_args: vec![],
        session_ref: false,
        resume_args: None,
        exit_keys: vec![],
        exit_command: None,
        verified: true,
    }
}

fn env(seat: &str, gen: i64, pane: &str) -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: seat.into(),
            generation: gen,
        }),
        pane_id: Some(pane.into()),
        admin: false,
    }
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint {
        pane_id: pane.into(),
        kind: Some(kind.into()),
        session_ref: None,
    }
}

fn ask(w: &mut World, body: &str, rid: &str, now: i64) -> mail::SendResult {
    let args = mail::SendArgs {
        kind: MsgKind::Question,
        body: body.into(),
        task: None,
        attempt: None,
        contract: None,
        candidate: None,
        verdict: None,
        blocking: None,
        base: None,
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        request_id: rid.into(),
    };
    mail::send(
        &mut w.store,
        &env("heat", 1, LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        &args,
        now,
    )
    .unwrap()
}

fn show_home(w: &mut World, seat: &str, pane: &str, kind: &str, now: i64) -> String {
    let h = home::build(
        &mut w.store,
        &env(seat, 1, pane),
        Some(&fp(pane, kind)),
        now,
    )
    .unwrap();
    home::record_view(&mut w.store, &h.view).unwrap();
    h.text
}

#[test]
fn full_messaging_loop() {
    let mut w = world();

    // 1. worker asks; delivered to the captain's inbox immediately.
    let sent = ask(&mut w, "Schema v1 or v2?", "r1", T0 + 1);
    assert_eq!(sent.recipient, "p:captain");
    assert_eq!(sent.msg_id, "M-1");
    assert!(!sent.replayed);
    let m1 = w
        .store
        .read(|tx| tx.get_message("p", "M-1"))
        .unwrap()
        .unwrap();
    assert_eq!(m1.state, MsgState::Open);

    // 2. engine rings the idle captain once.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    let rings = w.herdr.prompts_to(CAP_PANE);
    assert_eq!(rings.len(), 1);
    assert!(
        rings[0].contains("gear"),
        "doorbell points to `gear`: {}",
        rings[0]
    );
    assert!(
        !rings[0].contains("v2 schema"),
        "doorbell carries no message body"
    );

    // 3. no evidence yet: slot held, no resend even after cooldown.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 60).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);

    // 4. captain views home: sees thread, id and original text; view recorded.
    let text = show_home(&mut w, "captain", CAP_PANE, "codex", T0 + 61);
    assert!(text.contains("M-1"));
    assert!(text.contains("Schema v1 or v2?"));

    // 5. engine confirms the doorbell from the view.
    let rep = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 65).unwrap();
    assert_eq!(rep.confirmed.len(), 1);
    let ring_op = w
        .store
        .read(|tx| tx.last_op("p:captain", OpKind::Doorbell))
        .unwrap()
        .unwrap();
    assert_eq!(ring_op.state, OpState::Confirmed);

    // 6. reading home did not resolve anything.
    let m1 = w
        .store
        .read(|tx| tx.get_message("p", "M-1"))
        .unwrap()
        .unwrap();
    assert_eq!(m1.state, MsgState::Open);

    // 7. captain resolves M-1; reply lands in the worker inbox as Info.
    let r = mail::reply(
        &mut w.store,
        &env("captain", 1, CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        &mail::ReplyArgs {
            ids: vec!["M-1".into()],
            text: "Use v2".into(),
            mode: mail::ReplyMode::Resolve,
            wait_on: None,
            wait_until: None,
            downgrade: false,
            request_id: "r2".into(),
        },
        T0 + 70,
    )
    .unwrap();
    assert_eq!(r.handled, vec![("M-1".to_string(), "resolved".to_string())]);
    assert_eq!(r.replies.len(), 1);
    let reply = w
        .store
        .read(|tx| tx.get_message("p", &r.replies[0]))
        .unwrap()
        .unwrap();
    assert_eq!(reply.recipient, "p:heat");
    assert_eq!(reply.kind, MsgKind::Reply);
    assert_eq!(reply.state, MsgState::Info);
    assert_eq!(reply.thread, m1.thread);

    // 8. engine rings the worker for the unshown reply; captain is not rung again.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 75).unwrap();
    assert_eq!(w.herdr.prompts_to(LANE_PANE).len(), 1);
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);

    // 9. worker views the reply; engine confirms; then silence.
    let text = show_home(&mut w, "heat", LANE_PANE, "opencode", T0 + 80);
    assert!(text.contains("Use v2"));
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 85).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 200).unwrap();
    assert_eq!(
        w.herdr.prompts_to(LANE_PANE).len(),
        1,
        "shown Info reply needs no more doorbells"
    );
}

#[test]
fn send_is_replayed_for_same_request_id() {
    let mut w = world();
    let a = ask(&mut w, "x", "same", T0);
    let b = ask(&mut w, "x", "same", T0 + 1);
    assert_eq!(a.msg_id, b.msg_id);
    assert!(b.replayed);
    assert!(w
        .store
        .read(|tx| tx.get_message("p", "M-2"))
        .unwrap()
        .is_none());
}

#[test]
fn stale_generation_and_wrong_pane_are_rejected() {
    let mut w = world();
    let args = mail::SendArgs {
        kind: MsgKind::Question,
        body: "x".into(),
        task: None,
        attempt: None,
        contract: None,
        candidate: None,
        verdict: None,
        blocking: None,
        base: None,
        about: None,
        to: None,
        reply_to: None,
        urgent: false,
        left: None,
        blocker: None,
        on: None,
        friction: None,
        request_id: "r".into(),
    };
    let e = mail::send(
        &mut w.store,
        &env("heat", 2, LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        &args,
        T0,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::StaleGeneration);

    let e = mail::send(
        &mut w.store,
        &env("heat", 1, "w9:p9"),
        Some(&fp("w9:p9", "opencode")),
        &args,
        T0,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotRegistered);

    let e = mail::send(
        &mut w.store,
        &env("heat", 1, LANE_PANE),
        Some(&fp(LANE_PANE, "claude")),
        &args,
        T0,
    )
    .unwrap_err();
    assert_eq!(
        e.code,
        ErrorCode::NotRegistered,
        "different agent kind in the pane"
    );

    let no_ctx = Env {
        ctx: None,
        pane_id: Some(LANE_PANE.into()),
        admin: false,
    };
    let e = mail::send(&mut w.store, &no_ctx, None, &args, T0).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotRegistered);
}

#[test]
fn lane_cannot_reply_or_target_seats() {
    let mut w = world();
    ask(&mut w, "q", "r1", T0);
    let e = mail::reply(
        &mut w.store,
        &env("heat", 1, LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        &mail::ReplyArgs {
            ids: vec!["M-1".into()],
            text: "x".into(),
            mode: mail::ReplyMode::Resolve,
            wait_on: None,
            wait_until: None,
            downgrade: false,
            request_id: "r9".into(),
        },
        T0 + 1,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Forbidden);
}

#[test]
fn busy_or_unobservable_captain_is_not_rung() {
    let mut w = world();
    ask(&mut w, "q", "r1", T0);

    w.herdr.set_status(CAP_PANE, AgentStatus::Working);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert!(w.herdr.prompts_to(CAP_PANE).is_empty());

    // M4 §10.2: Herdr's state decides, never the screen. The captain is idle,
    // so it is rung — the old busy sign on its screen keeps nothing from it.
    w.herdr.set_status(CAP_PANE, AgentStatus::Idle);
    w.herdr.set_screen(CAP_PANE, "esc interrupt");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    assert_eq!(
        w.herdr.prompts_to(CAP_PANE).len(),
        1,
        "Herdr's state decides"
    );

    w.herdr.set_screen(CAP_PANE, "");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 15).unwrap();
    assert!(w.herdr.prompts_to(CAP_PANE).len() >= 1);

    let mut w = world();
    ask(&mut w, "q", "r1", T0);
    w.herdr.observe_fails = true;
    let rep = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(rep.observe_failed, vec!["p".to_string()]);
    assert!(
        w.herdr.calls.is_empty(),
        "no side effects when observation failed"
    );
}

#[test]
fn uncertain_doorbell_is_never_resent() {
    let mut w = world();
    ask(&mut w, "q", "r1", T0);
    w.herdr.inject_next = Some(gear_herdr::fake::Inject::Uncertain);
    let rep = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(rep.executed.len(), 1);
    assert_eq!(rep.executed[0].1, "unknown");
    for i in 1..=5 {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5 + i * 60).unwrap();
    }
    assert_eq!(
        w.herdr.prompts_to(CAP_PANE).len(),
        1,
        "unknown op holds the slot; no resend"
    );
}

#[test]
fn reply_only_closes_listed_ids() {
    let mut w = world();
    ask(&mut w, "q1", "r1", T0);
    ask(&mut w, "q2", "r2", T0 + 1);
    mail::reply(
        &mut w.store,
        &env("captain", 1, CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        &mail::ReplyArgs {
            ids: vec!["M-2".into()],
            text: "ok".into(),
            mode: mail::ReplyMode::Resolve,
            wait_on: None,
            wait_until: None,
            downgrade: false,
            request_id: "r3".into(),
        },
        T0 + 2,
    )
    .unwrap();
    let m1 = w
        .store
        .read(|tx| tx.get_message("p", "M-1"))
        .unwrap()
        .unwrap();
    let m2 = w
        .store
        .read(|tx| tx.get_message("p", "M-2"))
        .unwrap()
        .unwrap();
    assert_eq!(m1.state, MsgState::Open);
    assert_eq!(m2.state, MsgState::Resolved);
}
