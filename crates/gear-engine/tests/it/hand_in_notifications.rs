//! T-45: routine hand-ins and passes stay out of the captain's action queue.
use gear_app::{identity::Fingerprint, mail, review::Outcome, tasks, Env};
use gear_engine::{tick, EngineCfg};
use gear_herdr::{builtin, fake::FakeHerdr, AgentStatus};
use gear_model::{GearCtx, MsgKind, MsgState, OpKind, SeatRole, SeatState, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const NOW: i64 = 1_800_000_000;
const CAPTAIN: &str = "w1:p1";
const LANE: &str = "w1:p2";
const REVIEWER: &str = "w1:p3";

fn world() -> (Store, FakeHerdr, EngineCfg) {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", NOW)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAPTAIN,
                "codex",
                NOW,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", NOW)?;
            testing::seed_temp_seat(tx, "p", "r-1", "T-1", REVIEWER, "codex", NOW)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Ready,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                 issue: None,updated: NOW,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-1".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: NOW,
                mechanical: None,
            })?;
            tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
            tasks::on_delivery_confirmed(tx, "p", "T-1", 1, NOW)?;
            // Retiring lanes may finish their task. No unrelated low-queue alert
            // should mask which message causes a doorbell in this test.
            tx.set_seat_state("p:heat", SeatState::Retiring, None, NOW)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAPTAIN, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "opencode", AgentStatus::Working);
    herdr.add_agent(REVIEWER, "codex", AgentStatus::Working);
    let mut cfg = EngineCfg {
        auto_settle: true,
        ..Default::default()
    };
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.into(), builtin(kind).unwrap());
    }
    (store, herdr, cfg)
}

fn send(store: &mut Store, verdict: Option<Outcome>) -> mail::SendResult {
    let (seat, pane, kind) = if verdict.is_some() {
        ("r-1", REVIEWER, "codex")
    } else {
        ("heat", LANE, "opencode")
    };
    mail::send(
        store,
        &Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: seat.into(),
                generation: 1,
            }),
            pane_id: Some(pane.into()),
            admin: false,
        },
        Some(&Fingerprint {
            pane_id: pane.into(),
            kind: Some(kind.into()),
            session_ref: None,
        }),
        &mail::SendArgs {
            kind: if verdict.is_some() {
                MsgKind::Review
            } else {
                MsgKind::Result
            },
            body: "verified findings".into(),
            task: Some("T-1".into()),
            attempt: Some(1),
            contract: Some(1),
            candidate: Some("abc1234".into()),
            verdict,
            blocking: None,
            base: None,
            about: None,
            to: None,
            reply_to: None,
            urgent: true,
            left: None,
            blocker: None,
            on: None,
            friction: None,
            request_id: if verdict.is_some() {
                "review"
            } else {
                "result"
            }
            .into(),
        },
        NOW + 1,
    )
    .unwrap()
}

fn assert_silent(store: &mut Store, herdr: &FakeHerdr) {
    assert!(store
        .read(|tx| tx.actionable_for("p", "p:captain", NOW + 20))
        .unwrap()
        .is_empty());
    assert!(store
        .read(|tx| tx.messages_to_since("p", "p:captain", 0))
        .unwrap()
        .is_empty());
    assert!(store
        .read(|tx| tx.last_op("p:captain", OpKind::Doorbell))
        .unwrap()
        .is_none());
    assert!(herdr.prompts_to(CAPTAIN).is_empty());
}

#[test]
fn result_is_recorded_without_a_captain_action_or_doorbell() {
    let (mut store, mut herdr, cfg) = world();
    let sent = send(&mut store, None);
    let message = store
        .read(|tx| tx.get_message("p", &sent.msg_id))
        .unwrap()
        .unwrap();
    assert_eq!(message.state, MsgState::Info);
    assert_eq!(message.body, "verified findings");
    assert_eq!(
        store
            .read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .state,
        TaskState::Verifying
    );
    tick(&mut store, &mut herdr, &cfg, NOW + 5).unwrap();
    assert_silent(&mut store, &herdr);
    let resolved = mail::reply(
        &mut store,
        &Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: "captain".into(),
                generation: 1,
            }),
            pane_id: Some(CAPTAIN.into()),
            admin: false,
        },
        Some(&Fingerprint {
            pane_id: CAPTAIN.into(),
            kind: Some("codex".into()),
            session_ref: None,
        }),
        &mail::ReplyArgs {
            ids: vec![sent.msg_id],
            text: "bookkeeping only".into(),
            mode: mail::ReplyMode::Resolve,
            wait_on: None,
            wait_until: None,
            downgrade: false,
            request_id: "resolve-result".into(),
        },
        NOW + 6,
    )
    .unwrap();
    assert!(resolved.replies.is_empty());
    assert!(store
        .read(|tx| tx.messages_to_since("p", "p:heat", 0))
        .unwrap()
        .is_empty());
    tick(&mut store, &mut herdr, &cfg, NOW + 10).unwrap();
    assert_silent(&mut store, &herdr);
}

#[test]
fn passing_review_and_automatic_settlement_do_not_ring_the_captain() {
    let (mut store, mut herdr, cfg) = world();
    send(&mut store, None);
    let review = send(&mut store, Some(Outcome::Pass));
    assert_eq!(
        store
            .read(|tx| tx.get_message("p", &review.msg_id))
            .unwrap()
            .unwrap()
            .state,
        MsgState::Info
    );
    let report = tick(&mut store, &mut herdr, &cfg, NOW + 5).unwrap();
    assert_eq!(report.settled, vec!["p:T-1/1"]);
    assert_eq!(
        store
            .read(|tx| tx.get_task("p", "T-1"))
            .unwrap()
            .unwrap()
            .state,
        TaskState::Done
    );
    assert_silent(&mut store, &herdr);
    tick(&mut store, &mut herdr, &cfg, NOW + 10).unwrap();
    assert_silent(&mut store, &herdr);
}

#[test]
fn failing_review_remains_an_action_item_and_rings_the_captain() {
    let (mut store, mut herdr, cfg) = world();
    send(&mut store, None);
    let review = send(&mut store, Some(Outcome::Fail));
    let report = tick(&mut store, &mut herdr, &cfg, NOW + 5).unwrap();
    assert!(report.settled.is_empty());
    let items = store
        .read(|tx| tx.actionable_for("p", "p:captain", NOW + 5))
        .unwrap();
    assert_eq!(
        items.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec![review.msg_id.as_str()]
    );
    let bell = store
        .read(|tx| tx.last_op("p:captain", OpKind::Doorbell))
        .unwrap()
        .unwrap();
    assert_eq!(bell.payload["covers"], serde_json::json!([review.msg_id]));
    assert_eq!(herdr.prompts_to(CAPTAIN).len(), 1);
}

#[test]
fn automatic_settlement_itself_leaves_only_silent_information() {
    let (mut store, mut herdr, cfg) = world();
    store
        .write(|tx| {
            tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "report", NOW + 1)?;
            tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                project: "p".into(),
                task: "T-1".into(),
                attempt: 1,
                passed: true,
                findings: "verified".into(),
                by_seat: "p:r-1".into(),
                created: NOW + 2,
            })
        })
        .unwrap();
    let report = tick(&mut store, &mut herdr, &cfg, NOW + 5).unwrap();
    assert_eq!(report.settled, vec!["p:T-1/1"]);
    assert_silent(&mut store, &herdr);
    let history = store
        .read(|tx| tx.thread_messages("p", "T-1:review"))
        .unwrap();
    assert!(history
        .iter()
        .any(|m| m.state == MsgState::Info && m.body.contains("accepted")));
}
